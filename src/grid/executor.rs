//! Exchange abstraction for grid trading - enables mocking for tests

use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::Address;
use async_trait::async_trait;
use log::warn;
use tokio::sync::Mutex;

use crate::{
    ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient, ExchangeDataStatus,
    ExchangeResponseStatus, InfoClient,
};

use super::config::{AssetPrecision, GridConfig, MarketType};
use super::errors::{GridError, GridResult};
use super::types::{GridOrderRequest, MarginInfo, OrderResult, OrderResultStatus, OrderSide, Position};

/// Exchange operations trait - can be mocked for testing
#[async_trait]
pub trait GridExchange: Send + Sync {
    /// Place a limit order
    async fn place_order(&self, asset: &str, order: &GridOrderRequest) -> GridResult<OrderResult>;

    /// Cancel an order by oid
    async fn cancel_order(&self, asset: &str, oid: u64) -> GridResult<bool>;

    /// Bulk cancel all orders for an asset
    async fn cancel_all_orders(&self, asset: &str) -> GridResult<u32>;

    /// Get current mid price
    async fn get_mid_price(&self, asset: &str) -> GridResult<f64>;

    /// Get user's current position (for perps)
    async fn get_position(&self, asset: &str) -> GridResult<Option<Position>>;

    /// Get account margin info (for perps risk check)
    async fn get_margin_info(&self) -> GridResult<MarginInfo>;

    /// Update leverage for perp trading
    async fn update_leverage(&self, asset: &str, leverage: u32, is_cross: bool) -> GridResult<()>;

    /// Get asset precision from exchange meta
    async fn get_asset_precision(&self, asset: &str, market_type: MarketType) -> GridResult<AssetPrecision>;
}

// ============================================================================
// Real Hyperliquid Implementation
// ============================================================================

/// Real Hyperliquid exchange implementation
pub struct HyperliquidExchange {
    exchange_client: Arc<ExchangeClient>,
    info_client: Arc<Mutex<InfoClient>>,
    user_address: Address,
    max_retries: u32,
    retry_base_delay_ms: u64,
    market_type: MarketType,
    /// Cached spot index (e.g., "@107" for HYPE)
    spot_index_cache: Arc<Mutex<Option<String>>>,
}

impl HyperliquidExchange {
    pub fn new(
        exchange_client: ExchangeClient,
        info_client: InfoClient,
        config: &GridConfig,
    ) -> Self {
        let user_address = exchange_client.wallet.address();
        Self {
            exchange_client: Arc::new(exchange_client),
            info_client: Arc::new(Mutex::new(info_client)),
            user_address,
            max_retries: config.max_order_retries,
            retry_base_delay_ms: config.retry_base_delay_ms,
            market_type: config.market_type,
            spot_index_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Resolve spot asset name to index format (e.g., "HYPE/USDC" -> "@107")
    async fn resolve_spot_asset(&self, asset: &str) -> GridResult<String> {
        // Check cache first
        {
            let cache = self.spot_index_cache.lock().await;
            if let Some(ref cached) = *cache {
                return Ok(cached.clone());
            }
        }

        let info = self.info_client.lock().await;
        let spot_meta = info.spot_meta().await.map_err(|e| GridError::Exchange(e.to_string()))?;

        let index_to_name: std::collections::HashMap<usize, &str> = spot_meta
            .tokens
            .iter()
            .map(|t| (t.index, t.name.as_str()))
            .collect();

        let base_name = asset.split('/').next().unwrap_or(asset);

        for spot_asset in &spot_meta.universe {
            if let Some(t1) = index_to_name.get(&spot_asset.tokens[0]) {
                if *t1 == base_name || asset == spot_asset.name {
                    let spot_index = format!("@{}", spot_asset.index);
                    
                    drop(info);
                    let mut cache = self.spot_index_cache.lock().await;
                    *cache = Some(spot_index.clone());
                    
                    return Ok(spot_index);
                }
            }
        }

        Err(GridError::AssetNotFound(format!(
            "Could not find spot index for '{}'", asset
        )))
    }

    /// Get the correct asset identifier for API calls
    async fn get_asset_key(&self, asset: &str) -> GridResult<String> {
        match self.market_type {
            MarketType::Perp => Ok(asset.to_string()),
            MarketType::Spot => self.resolve_spot_asset(asset).await,
        }
    }

    /// Execute with exponential backoff retry
    async fn with_retry<T, F, Fut>(&self, operation: F) -> GridResult<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = GridResult<T>>,
    {
        let mut attempts = 0;
        let mut last_error = GridError::Exchange("Unknown error".into());

        while attempts < self.max_retries {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    attempts += 1;
                    last_error = e;

                    if attempts < self.max_retries {
                        let delay = self.retry_base_delay_ms * 2u64.pow(attempts - 1);
                        warn!(
                            "Operation failed (attempt {}/{}), retrying in {}ms: {}",
                            attempts, self.max_retries, delay, last_error
                        );
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                }
            }
        }

        Err(GridError::OrderPlacementFailed {
            attempts: self.max_retries,
            reason: last_error.to_string(),
        })
    }
}

#[async_trait]
impl GridExchange for HyperliquidExchange {
    async fn place_order(&self, asset: &str, order: &GridOrderRequest) -> GridResult<OrderResult> {
        let asset_key = self.get_asset_key(asset).await?;
        let exchange = self.exchange_client.clone();
        let order = order.clone();

        self.with_retry(|| {
            let exchange = exchange.clone();
            let asset_key = asset_key.clone();
            let order = order.clone();

            async move {
                let client_order = ClientOrderRequest {
                    asset: asset_key.clone(),
                    is_buy: order.side == OrderSide::Buy,
                    reduce_only: order.reduce_only,
                    limit_px: order.price,
                    sz: order.size,
                    cloid: None,
                    order_type: ClientOrder::Limit(ClientLimit {
                        tif: "Gtc".to_string(),
                    }),
                };

                let response = exchange
                    .order(client_order, None)
                    .await
                    .map_err(|e| GridError::Exchange(e.to_string()))?;

                match response {
                    ExchangeResponseStatus::Ok(resp) => {
                        if let Some(data) = resp.data {
                            if let Some(status) = data.statuses.first() {
                                match status {
                                    ExchangeDataStatus::Resting(r) => Ok(OrderResult {
                                        oid: r.oid,
                                        status: OrderResultStatus::Resting,
                                    }),
                                    ExchangeDataStatus::Filled(f) => Ok(OrderResult {
                                        oid: f.oid,
                                        status: OrderResultStatus::Filled {
                                            avg_price: f.avg_px.parse().unwrap_or(order.price),
                                            filled_size: f.total_sz.parse().unwrap_or(order.size),
                                        },
                                    }),
                                    ExchangeDataStatus::Error(e) => {
                                        Err(GridError::Exchange(e.clone()))
                                    }
                                    ExchangeDataStatus::WaitingForTrigger => Ok(OrderResult {
                                        oid: 0,
                                        status: OrderResultStatus::WaitingForTrigger,
                                    }),
                                    _ => Err(GridError::Exchange("Unexpected status".into())),
                                }
                            } else {
                                Err(GridError::Exchange("No status in response".into()))
                            }
                        } else {
                            Err(GridError::Exchange("No data in response".into()))
                        }
                    }
                    ExchangeResponseStatus::Err(e) => Err(GridError::Exchange(e)),
                }
            }
        })
        .await
    }

    async fn cancel_order(&self, asset: &str, oid: u64) -> GridResult<bool> {
        let asset_key = self.get_asset_key(asset).await?;
        
        let cancel_req = crate::ClientCancelRequest {
            asset: asset_key,
            oid,
        };

        let response = self
            .exchange_client
            .cancel(cancel_req, None)
            .await
            .map_err(|e| GridError::Exchange(e.to_string()))?;

        match response {
            ExchangeResponseStatus::Ok(resp) => {
                if let Some(data) = resp.data {
                    if let Some(status) = data.statuses.first() {
                        return Ok(matches!(status, ExchangeDataStatus::Success));
                    }
                }
                Ok(false)
            }
            ExchangeResponseStatus::Err(e) => Err(GridError::Exchange(e)),
        }
    }

    async fn cancel_all_orders(&self, asset: &str) -> GridResult<u32> {
        let info = self.info_client.lock().await;
        let open_orders = info
            .open_orders(self.user_address)
            .await
            .map_err(|e| GridError::Exchange(e.to_string()))?;

        let asset_key = self.get_asset_key(asset).await?;
        let asset_orders: Vec<_> = open_orders
            .into_iter()
            .filter(|o| o.coin == asset_key)
            .collect();

        let count = asset_orders.len() as u32;
        drop(info);

        for order in asset_orders {
            if let Err(e) = self.cancel_order(asset, order.oid).await {
                warn!("Failed to cancel order {}: {}", order.oid, e);
            }
        }

        Ok(count)
    }

    async fn get_mid_price(&self, asset: &str) -> GridResult<f64> {
        let asset_key = self.get_asset_key(asset).await?;
        
        let info = self.info_client.lock().await;
        let all_mids = info
            .all_mids()
            .await
            .map_err(|e| GridError::Exchange(e.to_string()))?;

        all_mids
            .get(&asset_key)
            .and_then(|s| s.parse::<f64>().ok())
            .ok_or_else(|| GridError::AssetNotFound(format!("{} (key: {})", asset, asset_key)))
    }

    async fn get_position(&self, asset: &str) -> GridResult<Option<Position>> {
        let info = self.info_client.lock().await;
        let user_state = info
            .user_state(self.user_address)
            .await
            .map_err(|e| GridError::Exchange(e.to_string()))?;

        let position = user_state
            .asset_positions
            .iter()
            .find(|p| p.position.coin == asset)
            .map(|p| Position {
                size: p.position.szi.parse().unwrap_or(0.0),
                entry_price: p.position.entry_px.as_ref().and_then(|s| s.parse().ok()),
                unrealized_pnl: p.position.unrealized_pnl.parse().unwrap_or(0.0),
                liquidation_price: p.position.liquidation_px.as_ref().and_then(|s| s.parse().ok()),
                margin_used: p.position.margin_used.parse().unwrap_or(0.0),
            });

        Ok(position)
    }

    async fn get_margin_info(&self) -> GridResult<MarginInfo> {
        let info = self.info_client.lock().await;
        let user_state = info
            .user_state(self.user_address)
            .await
            .map_err(|e| GridError::Exchange(e.to_string()))?;

        Ok(MarginInfo {
            account_value: user_state.margin_summary.account_value.parse().unwrap_or(0.0),
            margin_used: user_state.margin_summary.total_margin_used.parse().unwrap_or(0.0),
            available_margin: user_state.withdrawable.parse().unwrap_or(0.0),
            withdrawable: user_state.withdrawable.parse().unwrap_or(0.0),
        })
    }

    async fn update_leverage(&self, asset: &str, leverage: u32, is_cross: bool) -> GridResult<()> {
        self.exchange_client
            .update_leverage(leverage, asset, is_cross, None)
            .await
            .map_err(|e| GridError::Exchange(e.to_string()))?;
        Ok(())
    }

    async fn get_asset_precision(&self, asset: &str, market_type: MarketType) -> GridResult<AssetPrecision> {
        let info = self.info_client.lock().await;

        match market_type {
            MarketType::Perp => {
                let meta = info.meta().await.map_err(|e| GridError::Exchange(e.to_string()))?;

                let asset_meta = meta
                    .universe
                    .iter()
                    .find(|a| a.name == asset)
                    .ok_or_else(|| GridError::AssetNotFound(asset.to_string()))?;

                Ok(AssetPrecision::for_perp(asset_meta.sz_decimals))
            }
            MarketType::Spot => {
                let spot_meta = info.spot_meta().await.map_err(|e| GridError::Exchange(e.to_string()))?;
                let base_name = asset.split('/').next().unwrap_or(asset);

                let index_to_name: std::collections::HashMap<usize, &str> = spot_meta
                    .tokens
                    .iter()
                    .map(|t| (t.index, t.name.as_str()))
                    .collect();

                for spot_asset in &spot_meta.universe {
                    if let Some(t1) = index_to_name.get(&spot_asset.tokens[0]) {
                        if *t1 == base_name || asset == spot_asset.name {
                            let base_token = spot_meta
                                .tokens
                                .iter()
                                .find(|t| t.index == spot_asset.tokens[0])
                                .ok_or_else(|| GridError::AssetNotFound(asset.to_string()))?;

                            return Ok(AssetPrecision::for_spot(base_token.sz_decimals as u32));
                        }
                    }
                }

                Err(GridError::AssetNotFound(asset.to_string()))
            }
        }
    }
}

// ============================================================================
// Mock Implementation for Testing
// ============================================================================

/// Mock exchange for testing grid bots without a real exchange connection.
pub mod mock {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Mock exchange for testing
    pub struct MockExchange {
        pub orders: Arc<Mutex<Vec<GridOrderRequest>>>,
        pub cancelled_oids: Arc<Mutex<Vec<u64>>>,
        pub mid_price: Arc<Mutex<f64>>,
        pub position: Arc<Mutex<Option<Position>>>,
        pub margin_info: Arc<Mutex<MarginInfo>>,
        pub asset_precision: Arc<Mutex<AssetPrecision>>,
        next_oid: AtomicU64,
        pub should_fail: Arc<Mutex<bool>>,
    }

    impl MockExchange {
        pub fn new(mid_price: f64) -> Self {
            Self {
                orders: Arc::new(Mutex::new(Vec::new())),
                cancelled_oids: Arc::new(Mutex::new(Vec::new())),
                mid_price: Arc::new(Mutex::new(mid_price)),
                position: Arc::new(Mutex::new(None)),
                margin_info: Arc::new(Mutex::new(MarginInfo::default())),
                asset_precision: Arc::new(Mutex::new(AssetPrecision::for_perp(4))),
                next_oid: AtomicU64::new(1),
                should_fail: Arc::new(Mutex::new(false)),
            }
        }

        pub async fn set_mid_price(&self, price: f64) {
            *self.mid_price.lock().await = price;
        }

        pub async fn set_should_fail(&self, fail: bool) {
            *self.should_fail.lock().await = fail;
        }

        pub async fn set_asset_precision(&self, precision: AssetPrecision) {
            *self.asset_precision.lock().await = precision;
        }
    }

    #[async_trait]
    impl GridExchange for MockExchange {
        async fn place_order(&self, _asset: &str, order: &GridOrderRequest) -> GridResult<OrderResult> {
            if *self.should_fail.lock().await {
                return Err(GridError::Exchange("Mock failure".into()));
            }

            self.orders.lock().await.push(order.clone());
            let oid = self.next_oid.fetch_add(1, Ordering::SeqCst);

            Ok(OrderResult {
                oid,
                status: OrderResultStatus::Resting,
            })
        }

        async fn cancel_order(&self, _asset: &str, oid: u64) -> GridResult<bool> {
            self.cancelled_oids.lock().await.push(oid);
            Ok(true)
        }

        async fn cancel_all_orders(&self, _asset: &str) -> GridResult<u32> {
            let count = self.orders.lock().await.len() as u32;
            self.orders.lock().await.clear();
            Ok(count)
        }

        async fn get_mid_price(&self, _asset: &str) -> GridResult<f64> {
            Ok(*self.mid_price.lock().await)
        }

        async fn get_position(&self, _asset: &str) -> GridResult<Option<Position>> {
            Ok(self.position.lock().await.clone())
        }

        async fn get_margin_info(&self) -> GridResult<MarginInfo> {
            Ok(self.margin_info.lock().await.clone())
        }

        async fn update_leverage(&self, _asset: &str, _leverage: u32, _is_cross: bool) -> GridResult<()> {
            Ok(())
        }

        async fn get_asset_precision(&self, _asset: &str, _market_type: MarketType) -> GridResult<AssetPrecision> {
            Ok(*self.asset_precision.lock().await)
        }
    }
}
