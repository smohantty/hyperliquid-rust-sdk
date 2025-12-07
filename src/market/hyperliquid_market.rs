//! Hyperliquid Market Implementation
//!
//! Connects to the Hyperliquid exchange and implements the Market interface.

use std::collections::HashMap;
use std::sync::Arc;

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use log::{debug, error, info};
use tokio::sync::{mpsc::unbounded_channel, RwLock};

use super::listener::MarketListener;
use super::types::{AssetInfo, OrderFill, OrderRequest, OrderStatus};
use crate::{
    BaseUrl, ClientCancelRequest, ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient,
    ExchangeDataStatus, ExchangeResponseStatus, InfoClient, Message, Subscription, UserData,
};

/// Input configuration for creating a HyperliquidMarket
#[derive(Debug)]
pub struct HyperliquidMarketInput {
    /// Asset to trade (e.g., "BTC", "ETH")
    pub asset: String,
    /// Wallet containing private key for signing
    pub wallet: PrivateKeySigner,
    /// Base URL (Mainnet or Testnet)
    pub base_url: Option<BaseUrl>,
}

/// Internal order tracking for Hyperliquid
#[derive(Debug, Clone)]
struct TrackedOrder {
    /// Exchange order ID (oid) - internal to Hyperliquid
    exchange_oid: Option<u64>,
    /// Original request (contains user's order_id)
    request: OrderRequest,
    /// Current status
    status: OrderStatus,
    /// Filled quantity
    filled_qty: f64,
    /// Average fill price
    avg_fill_price: f64,
}

impl TrackedOrder {
    fn new(request: OrderRequest) -> Self {
        Self {
            exchange_oid: None,
            request,
            status: OrderStatus::Pending,
            filled_qty: 0.0,
            avg_fill_price: 0.0,
        }
    }

    fn fill(&mut self, qty: f64, price: f64) {
        let total_value = self.avg_fill_price * self.filled_qty + price * qty;
        self.filled_qty += qty;
        self.avg_fill_price = if self.filled_qty > 0.0 {
            total_value / self.filled_qty
        } else {
            0.0
        };

        if self.filled_qty >= self.request.qty {
            self.status = OrderStatus::Filled(self.avg_fill_price);
        } else {
            self.status = OrderStatus::PartiallyFilled(self.filled_qty);
        }
    }
}

/// Hyperliquid Market implementation
///
/// Connects to the Hyperliquid exchange via WebSocket and REST APIs.
/// Implements the Market interface requirements (M1-M11).
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use tokio::sync::RwLock;
/// use hyperliquid_rust_sdk::market::{HyperliquidMarket, HyperliquidMarketInput};
/// use hyperliquid_rust_sdk::bot::Bot;
///
/// let bot = Arc::new(RwLock::new(Bot::new(my_strategy)));
/// let input = HyperliquidMarketInput {
///     asset: "BTC".to_string(),
///     wallet: wallet,
///     base_url: Some(BaseUrl::Testnet),
/// };
///
/// let mut market = HyperliquidMarket::new(input, bot.clone()).await?;
/// // bot can also be used by HTTP server
/// market.start().await;
/// ```
pub struct HyperliquidMarket<L: MarketListener> {
    /// Asset being traded
    pub asset: String,
    /// Cached asset info (balances and precision)
    asset_info: AssetInfo,
    /// Shared listener instance for external access
    listener: Arc<RwLock<L>>,
    /// Info client for market data
    pub info_client: InfoClient,
    /// Exchange client for order management
    pub exchange_client: ExchangeClient,
    /// User's wallet address
    pub user_address: Address,
    /// Current prices by asset
    prices: HashMap<String, f64>,
    /// Orders by user-provided order_id
    orders: HashMap<u64, TrackedOrder>,
    /// Maps exchange OID to user's order_id
    exchange_oid_to_order_id: HashMap<u64, u64>,
}

impl<L: MarketListener> HyperliquidMarket<L> {
    /// Create a new HyperliquidMarket
    ///
    /// # Arguments
    /// * `input` - Configuration for the market
    /// * `listener` - Shared listener wrapped in Arc<RwLock<L>>
    pub async fn new(
        input: HyperliquidMarketInput,
        listener: Arc<RwLock<L>>,
    ) -> Result<Self, crate::Error> {
        let user_address = input.wallet.address();
        let base_url = input.base_url.unwrap_or(BaseUrl::Mainnet);

        let info_client = InfoClient::new(None, Some(base_url)).await?;
        let exchange_client =
            ExchangeClient::new(None, input.wallet, Some(base_url), None, None).await?;

        // Fetch and cache asset info (precision is static)
        let asset_info = Self::fetch_asset_info(&info_client, &input.asset, user_address).await?;

        Ok(Self {
            asset: input.asset,
            asset_info,
            listener,
            info_client,
            exchange_client,
            user_address,
            prices: HashMap::new(),
            orders: HashMap::new(),
            exchange_oid_to_order_id: HashMap::new(),
        })
    }

    /// Fetch asset info from exchange (internal helper)
    async fn fetch_asset_info(
        info_client: &InfoClient,
        asset: &str,
        user_address: Address,
    ) -> Result<AssetInfo, crate::Error> {
        let is_spot = asset.contains('/');

        // Get balances
        let (base_balance, usdc_balance) = if is_spot {
            let balances = info_client.user_token_balances(user_address).await?;
            let base_name = asset.split('/').next().unwrap_or(asset);

            let base_bal = balances
                .balances
                .iter()
                .find(|b| b.coin == base_name)
                .and_then(|b| b.total.parse::<f64>().ok())
                .unwrap_or(0.0);

            let usdc_bal = balances
                .balances
                .iter()
                .find(|b| b.coin == "USDC")
                .and_then(|b| b.total.parse::<f64>().ok())
                .unwrap_or(0.0);

            (base_bal, usdc_bal)
        } else {
            let state = info_client.user_state(user_address).await?;

            let position = state
                .asset_positions
                .iter()
                .find(|p| p.position.coin == asset)
                .map(|p| p.position.szi.parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);

            let margin = state
                .margin_summary
                .account_value
                .parse::<f64>()
                .unwrap_or(0.0);

            (position, margin)
        };

        // Get precision
        let (sz_decimals, price_decimals) = if is_spot {
            let spot_meta = info_client.spot_meta().await?;
            let base_name = asset.split('/').next().unwrap_or(asset);

            let index_to_token: std::collections::HashMap<_, _> = spot_meta
                .tokens
                .iter()
                .map(|t| (t.index, t))
                .collect();

            let mut found_sz = 4u32;
            for spot_asset in &spot_meta.universe {
                if let Some(token) = index_to_token.get(&spot_asset.tokens[0]) {
                    if token.name == base_name || asset == spot_asset.name {
                        found_sz = token.sz_decimals as u32;
                        break;
                    }
                }
            }

            (found_sz, 6u32)
        } else {
            let meta = info_client.meta().await?;
            let asset_meta = meta
                .universe
                .iter()
                .find(|a| a.name == asset)
                .ok_or_else(|| crate::Error::AssetNotFound)?;

            (asset_meta.sz_decimals, 5u32)
        };

        Ok(AssetInfo::new(
            asset,
            base_balance,
            usdc_balance,
            sz_decimals,
            price_decimals,
        ))
    }

    /// Start the market event loop
    ///
    /// Subscribes to AllMids (price updates) and UserEvents (fills)
    /// and processes them in a loop. Orders returned by the listener
    /// are automatically placed.
    pub async fn start(&mut self) {
        let (sender, mut receiver) = unbounded_channel();

        // Subscribe to UserEvents for fills
        if let Err(e) = self
            .info_client
            .subscribe(
                Subscription::UserEvents {
                    user: self.user_address,
                },
                sender.clone(),
            )
            .await
        {
            error!("Failed to subscribe to UserEvents: {e}");
            return;
        }

        // Subscribe to AllMids for price updates
        if let Err(e) = self
            .info_client
            .subscribe(Subscription::AllMids, sender)
            .await
        {
            error!("Failed to subscribe to AllMids: {e}");
            return;
        }

        info!("HyperliquidMarket started for asset {}", self.asset);

        loop {
            match receiver.recv().await {
                Some(message) => {
                    // Process message and get orders to place
                    let pending_orders = self.handle_message(message);

                    // Place orders returned by listener
                    for order in pending_orders {
                        self.place_order(order).await;
                    }
                }
                None => {
                    error!("Channel closed");
                    break;
                }
            }
        }
    }

    /// Handle incoming WebSocket messages
    /// Returns orders that need to be placed (from listener callbacks)
    fn handle_message(&mut self, message: Message) -> Vec<OrderRequest> {
        let mut pending_orders = Vec::new();

        match message {
            Message::AllMids(all_mids) => {
                let mids = all_mids.data.mids;
                for (asset, price_str) in mids {
                    if let Ok(price) = price_str.parse::<f64>() {
                        // Update internal price state (M1)
                        self.prices.insert(asset.clone(), price);
                        // Only notify listener for our configured asset
                        if asset == self.asset {
                            if let Ok(mut listener) = self.listener.try_write() {
                                let orders = listener.on_price_update(&asset, price);
                                pending_orders.extend(orders);
                            }
                        }
                    }
                }
            }
            Message::User(user_events) => {
                let user_data = user_events.data;
                if let UserData::Fills(fills) = user_data {
                    for fill in fills {
                        let oid = fill.oid;
                        let qty: f64 = fill.sz.parse().unwrap_or(0.0);
                        let price: f64 = fill.px.parse().unwrap_or(0.0);
                        let closed_pnl: f64 = fill.closed_pnl.parse().unwrap_or(0.0);

                        debug!(
                            "Fill received: oid={}, qty={}, price={}, side={}, closed_pnl={}",
                            oid, qty, price, fill.side, closed_pnl
                        );

                        // Find order by exchange OID and update
                        if let Some(&user_order_id) = self.exchange_oid_to_order_id.get(&oid) {
                            if let Some(order) = self.orders.get_mut(&user_order_id) {
                                let was_active = order.status.is_active();
                                order.fill(qty, price);

                                if fill.side == "B" {
                                    info!("Fill: bought {} {} at {}", qty, fill.coin, price);
                                } else {
                                    info!("Fill: sold {} {} at {}", qty, fill.coin, price);
                                }

                                // Only notify when order is fully filled (M3)
                                if was_active && matches!(order.status, OrderStatus::Filled(_)) {
                                    let order_fill = OrderFill::new(
                                        user_order_id,          // User's order_id
                                        &fill.coin,
                                        order.request.qty,      // Total order qty
                                        order.avg_fill_price,   // Average fill price
                                    );

                                    info!(
                                        "Order {} fully filled: {} {} at avg price {}",
                                        user_order_id, order.request.qty, fill.coin, order.avg_fill_price
                                    );

                                    // M6: Synchronous notification, collect returned orders
                                    if let Ok(mut listener) = self.listener.try_write() {
                                        let orders = listener.on_order_filled(order_fill);
                                        pending_orders.extend(orders);
                                    }
                                }
                            }
                        } else {
                            // External fill not tracked by us
                            debug!("Received fill for unknown order oid={}", oid);
                        }
                    }
                }
            }
            _ => {
                debug!("Received unhandled message type");
            }
        }

        pending_orders
    }

    /// Update the price for an asset (M7)
    ///
    /// Manually updates internal price state and notifies the listener.
    /// Returns orders that the listener wants to place.
    /// Note: Prices are also updated automatically via WebSocket subscription.
    ///
    /// # Arguments
    /// * `asset` - The asset identifier
    /// * `price` - The new price
    ///
    /// # Returns
    /// Orders to place (caller should place them with `place_order`)
    pub fn update_price(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
        self.prices.insert(asset.to_string(), price);
        // M6: Synchronous notification, return orders to place
        if let Ok(mut listener) = self.listener.try_write() {
            listener.on_price_update(asset, price)
        } else {
            vec![]
        }
    }

    /// Place a new order on Hyperliquid (M8)
    ///
    /// # Arguments
    /// * `order` - The order request (contains user-provided order_id, side, reduce_only, tif)
    pub async fn place_order(&mut self, order: OrderRequest) {
        let user_order_id = order.order_id;
        let mut tracked_order = TrackedOrder::new(order.clone());

        // Place order on exchange
        let exchange_order = ClientOrderRequest {
            asset: order.asset.clone(),
            is_buy: order.side.is_buy(),
            reduce_only: order.reduce_only,
            limit_px: order.limit_price,
            sz: order.qty,
            cloid: None,
            order_type: ClientOrder::Limit(ClientLimit {
                tif: "Gtc".to_string(),
            }),
        };

        match self.exchange_client.order(exchange_order, None).await {
            Ok(response) => match response {
                ExchangeResponseStatus::Ok(resp) => {
                    if let Some(data) = resp.data {
                        if !data.statuses.is_empty() {
                            match &data.statuses[0] {
                                ExchangeDataStatus::Filled(filled) => {
                                    tracked_order.exchange_oid = Some(filled.oid);
                                    tracked_order.status = OrderStatus::Filled(order.limit_price);
                                    self.exchange_oid_to_order_id.insert(filled.oid, user_order_id);

                                    info!("Order {} filled immediately, oid={}", user_order_id, filled.oid);

                                    // Create fill notification with user's order_id
                                    let fill = OrderFill::new(
                                        user_order_id,
                                        &order.asset,
                                        order.qty,
                                        order.limit_price,
                                    );

                                    // Store order before notifying
                                    self.orders.insert(user_order_id, tracked_order);

                                    // M6: Synchronous notification, place returned orders
                                    let pending_orders = if let Ok(mut listener) = self.listener.try_write() {
                                        listener.on_order_filled(fill)
                                    } else {
                                        vec![]
                                    };
                                    for pending in pending_orders {
                                        // Recursive call for orders returned by listener
                                        Box::pin(self.place_order(pending)).await;
                                    }

                                    return;
                                }
                                ExchangeDataStatus::Resting(resting) => {
                                    tracked_order.exchange_oid = Some(resting.oid);
                                    tracked_order.status = OrderStatus::Pending;
                                    self.exchange_oid_to_order_id.insert(resting.oid, user_order_id);

                                    info!("Order {} resting, oid={}", user_order_id, resting.oid);
                                }
                                ExchangeDataStatus::Error(e) => {
                                    error!("Order {} error: {}", user_order_id, e);
                                    tracked_order.status = OrderStatus::Cancelled;
                                }
                                _ => {
                                    debug!("Order {} unknown status", user_order_id);
                                }
                            }
                        }
                    }
                }
                ExchangeResponseStatus::Err(e) => {
                    error!("Order {} exchange error: {}", user_order_id, e);
                    tracked_order.status = OrderStatus::Cancelled;
                }
            },
            Err(e) => {
                error!("Order {} request error: {}", user_order_id, e);
                tracked_order.status = OrderStatus::Cancelled;
            }
        }

        self.orders.insert(user_order_id, tracked_order);
    }

    /// Inject an external fill (M9)
    ///
    /// Accepts an externally described fill and updates order state.
    /// Only notifies the listener when the order is fully filled.
    /// Returns orders that the listener wants to place.
    ///
    /// # Arguments
    /// * `fill` - The fill details (order_id is user-provided)
    ///
    /// # Returns
    /// Orders to place (caller should place them with `place_order`)
    pub fn execute_fill(&mut self, fill: OrderFill) -> Vec<OrderRequest> {
        // Update order state if it exists
        if let Some(order) = self.orders.get_mut(&fill.order_id) {
            let was_active = order.status.is_active();
            order.fill(fill.qty, fill.price);

            // Only notify when order is fully filled
            if was_active && matches!(order.status, OrderStatus::Filled(_)) {
                let complete_fill = OrderFill::new(
                    fill.order_id,
                    &order.request.asset,
                    order.request.qty,      // Total order qty
                    order.avg_fill_price,   // Average fill price
                );

                // M6: Synchronous notification, return orders to place
                if let Ok(mut listener) = self.listener.try_write() {
                    return listener.on_order_filled(complete_fill);
                }
            }
        }
        vec![]
    }

    /// Query current price for an asset (M10)
    ///
    /// # Arguments
    /// * `asset` - The asset identifier
    ///
    /// # Returns
    /// The last known price if available
    pub fn current_price(&self, asset: &str) -> Option<f64> {
        self.prices.get(asset).copied()
    }

    /// Query order status (M11)
    ///
    /// # Arguments
    /// * `order_id` - The user-provided order identifier
    ///
    /// # Returns
    /// The current order status if the order exists
    pub fn order_status(&self, order_id: u64) -> Option<OrderStatus> {
        self.orders.get(&order_id).map(|o| o.status)
    }

    /// Get the shared listener reference
    ///
    /// Returns the `Arc<RwLock<L>>` so callers can access the listener
    /// for status queries, dashboards, etc.
    pub fn listener(&self) -> Arc<RwLock<L>> {
        self.listener.clone()
    }

    /// Cancel an order
    ///
    /// # Arguments
    /// * `order_id` - The user-provided order ID to cancel
    ///
    /// # Returns
    /// `true` if the order was cancelled successfully
    pub async fn cancel_order(&mut self, order_id: u64) -> bool {
        let Some(order) = self.orders.get(&order_id) else {
            return false;
        };

        if !order.status.is_active() {
            return false;
        }

        let Some(exchange_oid) = order.exchange_oid else {
            // Order not yet on exchange
            if let Some(order) = self.orders.get_mut(&order_id) {
                order.status = OrderStatus::Cancelled;
            }
            return true;
        };

        let cancel_request = ClientCancelRequest {
            asset: order.request.asset.clone(),
            oid: exchange_oid,
        };

        match self.exchange_client.cancel(cancel_request, None).await {
            Ok(response) => match response {
                ExchangeResponseStatus::Ok(resp) => {
                    if let Some(data) = resp.data {
                        if !data.statuses.is_empty() {
                            match &data.statuses[0] {
                                ExchangeDataStatus::Success => {
                                    if let Some(order) = self.orders.get_mut(&order_id) {
                                        order.status = OrderStatus::Cancelled;
                                    }
                                    info!("Order {} cancelled", order_id);
                                    return true;
                                }
                                ExchangeDataStatus::Error(e) => {
                                    error!("Cancel error: {}", e);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                ExchangeResponseStatus::Err(e) => {
                    error!("Cancel exchange error: {}", e);
                }
            },
            Err(e) => {
                error!("Cancel request error: {}", e);
            }
        }

        false
    }

    /// Get the exchange OID for a user-provided order ID
    pub fn get_exchange_oid(&self, order_id: u64) -> Option<u64> {
        self.orders.get(&order_id).and_then(|o| o.exchange_oid)
    }

    /// Get all current prices
    pub fn all_prices(&self) -> &HashMap<String, f64> {
        &self.prices
    }

    /// Get cached asset information (balances and precision)
    ///
    /// Returns the cached AssetInfo. Precision is static, but balances
    /// may be stale. Use `refresh_balances()` to update balances.
    pub fn asset_info(&self) -> &AssetInfo {
        &self.asset_info
    }

    /// Refresh cached balances from the exchange
    ///
    /// Updates the balance and usdc_balance fields in the cached AssetInfo.
    /// Precision fields remain unchanged (they are static).
    pub async fn refresh_balances(&mut self) -> Result<(), crate::Error> {
        let updated = Self::fetch_asset_info(&self.info_client, &self.asset, self.user_address).await?;
        self.asset_info.balance = updated.balance;
        self.asset_info.usdc_balance = updated.usdc_balance;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::types::OrderSide;

    // Integration tests would require actual exchange connection
    // Unit tests for internal logic

    #[test]
    fn test_tracked_order_fill() {
        let request = OrderRequest::buy(100, "BTC", 2.0, 50000.0);
        let mut order = TrackedOrder::new(request);

        assert_eq!(order.status, OrderStatus::Pending);

        // Partial fill
        order.fill(1.0, 49900.0);
        assert_eq!(order.status, OrderStatus::PartiallyFilled(1.0));
        assert_eq!(order.avg_fill_price, 49900.0);

        // Complete fill
        order.fill(1.0, 50100.0);
        match order.status {
            OrderStatus::Filled(avg) => {
                assert!((avg - 50000.0).abs() < 0.01);
            }
            _ => panic!("Expected Filled status"),
        }
    }

    #[test]
    fn test_order_request_validation() {
        let order = OrderRequest::buy(200, "ETH", 1.5, 3000.0);
        assert!(order.is_valid());
        assert_eq!(order.order_id, 200);
        assert_eq!(order.asset, "ETH");
        assert_eq!(order.qty, 1.5);
        assert_eq!(order.limit_price, 3000.0);
        assert_eq!(order.side, OrderSide::Buy);
        assert!(!order.reduce_only);
    }
}

