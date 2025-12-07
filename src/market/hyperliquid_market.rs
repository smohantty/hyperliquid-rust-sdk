//! Hyperliquid Market Implementation
//!
//! Connects to the Hyperliquid exchange and implements the Market interface.

use std::collections::HashMap;

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use log::{debug, error, info};
use tokio::sync::mpsc::unbounded_channel;

use super::listener::MarketListener;
use super::types::{OrderFill, OrderRequest, OrderStatus};
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
    /// Our internal order ID
    #[allow(dead_code)]
    internal_id: u64,
    /// Exchange order ID (oid)
    exchange_oid: Option<u64>,
    /// Original request
    request: OrderRequest,
    /// Current status
    status: OrderStatus,
    /// Filled quantity
    filled_qty: f64,
    /// Average fill price
    avg_fill_price: f64,
}

impl TrackedOrder {
    fn new(internal_id: u64, request: OrderRequest) -> Self {
        Self {
            internal_id,
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
/// use hyperliquid_rust_sdk::market::{HyperliquidMarket, HyperliquidMarketInput, NoOpListener};
///
/// let input = HyperliquidMarketInput {
///     asset: "BTC".to_string(),
///     wallet: wallet,
///     base_url: Some(BaseUrl::Testnet),
/// };
///
/// let mut market = HyperliquidMarket::new(input, NoOpListener).await;
/// market.start().await;
/// ```
pub struct HyperliquidMarket<L: MarketListener> {
    /// Asset being traded
    pub asset: String,
    /// Owned listener instance (M5)
    listener: L,
    /// Info client for market data
    pub info_client: InfoClient,
    /// Exchange client for order management
    pub exchange_client: ExchangeClient,
    /// User's wallet address
    pub user_address: Address,
    /// Current prices by asset
    prices: HashMap<String, f64>,
    /// Orders by internal ID
    orders_by_internal_id: HashMap<u64, TrackedOrder>,
    /// Orders by exchange OID
    orders_by_exchange_oid: HashMap<u64, u64>, // exchange_oid -> internal_id
    /// Next internal order ID
    next_order_id: u64,
}

impl<L: MarketListener> HyperliquidMarket<L> {
    /// Create a new HyperliquidMarket
    ///
    /// # Arguments
    /// * `input` - Configuration for the market
    /// * `listener` - Listener to receive notifications
    pub async fn new(
        input: HyperliquidMarketInput,
        listener: L,
    ) -> Result<Self, crate::Error> {
        let user_address = input.wallet.address();
        let base_url = input.base_url.unwrap_or(BaseUrl::Mainnet);

        let info_client = InfoClient::new(None, Some(base_url)).await?;
        let exchange_client =
            ExchangeClient::new(None, input.wallet, Some(base_url), None, None).await?;

        Ok(Self {
            asset: input.asset,
            listener,
            info_client,
            exchange_client,
            user_address,
            prices: HashMap::new(),
            orders_by_internal_id: HashMap::new(),
            orders_by_exchange_oid: HashMap::new(),
            next_order_id: 1,
        })
    }

    /// Start the market event loop
    ///
    /// Subscribes to AllMids (price updates) and UserEvents (fills)
    /// and processes them in a loop.
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
                Some(message) => self.handle_message(message),
                None => {
                    error!("Channel closed");
                    break;
                }
            }
        }
    }

    /// Handle incoming WebSocket messages
    fn handle_message(&mut self, message: Message) {
        match message {
            Message::AllMids(all_mids) => {
                let mids = all_mids.data.mids;
                for (asset, price_str) in mids {
                    if let Ok(price) = price_str.parse::<f64>() {
                        // Update internal price state (M1)
                        self.prices.insert(asset.clone(), price);
                        // M6: Synchronous notification (M4)
                        self.listener.on_price_update(&asset, price);
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
                        if let Some(&internal_id) = self.orders_by_exchange_oid.get(&oid) {
                            if let Some(order) = self.orders_by_internal_id.get_mut(&internal_id) {
                                order.fill(qty, price);

                                // Create fill notification (M3)
                                let order_fill = OrderFill::new(
                                    internal_id,
                                    &fill.coin,
                                    qty,
                                    price,
                                );

                                // M6: Synchronous notification
                                self.listener.on_order_filled(order_fill);

                                if fill.side == "B" {
                                    info!("Fill: bought {} {} at {}", qty, fill.coin, price);
                                } else {
                                    info!("Fill: sold {} {} at {}", qty, fill.coin, price);
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
    }

    /// Update the price for an asset (M7)
    ///
    /// Manually updates internal price state and notifies the listener.
    /// Note: Prices are also updated automatically via WebSocket subscription.
    ///
    /// # Arguments
    /// * `asset` - The asset identifier
    /// * `price` - The new price
    pub fn update_price(&mut self, asset: &str, price: f64) {
        self.prices.insert(asset.to_string(), price);
        // M6: Synchronous notification
        self.listener.on_price_update(asset, price);
    }

    /// Place a new order on Hyperliquid (M8)
    ///
    /// # Arguments
    /// * `order` - The order request
    ///
    /// # Returns
    /// A unique internal order ID
    pub async fn place_order(&mut self, order: OrderRequest) -> u64 {
        let internal_id = self.next_order_id;
        self.next_order_id += 1;

        let mut tracked_order = TrackedOrder::new(internal_id, order.clone());

        // Determine if buy or sell based on limit price vs current price
        // For simplicity: if limit_price >= current_price, it's a buy
        let is_buy = self
            .prices
            .get(&order.asset)
            .map(|&current| order.limit_price >= current)
            .unwrap_or(true);

        // Place order on exchange
        let exchange_order = ClientOrderRequest {
            asset: order.asset.clone(),
            is_buy,
            reduce_only: false,
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
                                    self.orders_by_exchange_oid.insert(filled.oid, internal_id);

                                    info!("Order {} filled immediately, oid={}", internal_id, filled.oid);

                                    // Create fill notification
                                    let fill = OrderFill::new(
                                        internal_id,
                                        &order.asset,
                                        order.qty,
                                        order.limit_price,
                                    );

                                    // Store order before notifying
                                    self.orders_by_internal_id.insert(internal_id, tracked_order);

                                    // M6: Synchronous notification
                                    self.listener.on_order_filled(fill);

                                    return internal_id;
                                }
                                ExchangeDataStatus::Resting(resting) => {
                                    tracked_order.exchange_oid = Some(resting.oid);
                                    tracked_order.status = OrderStatus::Pending;
                                    self.orders_by_exchange_oid.insert(resting.oid, internal_id);

                                    info!("Order {} resting, oid={}", internal_id, resting.oid);
                                }
                                ExchangeDataStatus::Error(e) => {
                                    error!("Order {} error: {}", internal_id, e);
                                    tracked_order.status = OrderStatus::Cancelled;
                                }
                                _ => {
                                    debug!("Order {} unknown status", internal_id);
                                }
                            }
                        }
                    }
                }
                ExchangeResponseStatus::Err(e) => {
                    error!("Order {} exchange error: {}", internal_id, e);
                    tracked_order.status = OrderStatus::Cancelled;
                }
            },
            Err(e) => {
                error!("Order {} request error: {}", internal_id, e);
                tracked_order.status = OrderStatus::Cancelled;
            }
        }

        self.orders_by_internal_id.insert(internal_id, tracked_order);
        internal_id
    }

    /// Inject an external fill (M9)
    ///
    /// Accepts an externally described fill and immediately notifies the listener.
    ///
    /// # Arguments
    /// * `fill` - The fill details
    pub fn execute_fill(&mut self, fill: OrderFill) {
        // Update order state if it exists
        if let Some(order) = self.orders_by_internal_id.get_mut(&fill.order_id) {
            order.fill(fill.qty, fill.price);
        }

        // M6: Synchronous notification
        self.listener.on_order_filled(fill);
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
    /// * `order_id` - The internal order identifier
    ///
    /// # Returns
    /// The current order status if the order exists
    pub fn order_status(&self, order_id: u64) -> Option<OrderStatus> {
        self.orders_by_internal_id.get(&order_id).map(|o| o.status)
    }

    /// Get a reference to the listener
    pub fn listener(&self) -> &L {
        &self.listener
    }

    /// Get a mutable reference to the listener
    pub fn listener_mut(&mut self) -> &mut L {
        &mut self.listener
    }

    /// Cancel an order
    ///
    /// # Arguments
    /// * `order_id` - The internal order ID to cancel
    ///
    /// # Returns
    /// `true` if the order was cancelled successfully
    pub async fn cancel_order(&mut self, order_id: u64) -> bool {
        let Some(order) = self.orders_by_internal_id.get(&order_id) else {
            return false;
        };

        if !order.status.is_active() {
            return false;
        }

        let Some(exchange_oid) = order.exchange_oid else {
            // Order not yet on exchange
            if let Some(order) = self.orders_by_internal_id.get_mut(&order_id) {
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
                                    if let Some(order) = self.orders_by_internal_id.get_mut(&order_id) {
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

    /// Get the exchange OID for an internal order ID
    pub fn get_exchange_oid(&self, order_id: u64) -> Option<u64> {
        self.orders_by_internal_id
            .get(&order_id)
            .and_then(|o| o.exchange_oid)
    }

    /// Get all current prices
    pub fn all_prices(&self) -> &HashMap<String, f64> {
        &self.prices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests would require actual exchange connection
    // Unit tests for internal logic

    #[test]
    fn test_tracked_order_fill() {
        let request = OrderRequest::new("BTC", 2.0, 50000.0);
        let mut order = TrackedOrder::new(1, request);

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
        let order = OrderRequest::new("ETH", 1.5, 3000.0);
        assert!(order.is_valid());
        assert_eq!(order.asset, "ETH");
        assert_eq!(order.qty, 1.5);
        assert_eq!(order.limit_price, 3000.0);
    }
}

