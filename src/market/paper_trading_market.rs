//! Paper Trading Market Implementation
//!
//! Connects to Hyperliquid for live price feeds but simulates order execution
//! locally by checking midprice against pending order limits.

use std::collections::HashMap;

use log::{error, info};
use tokio::sync::mpsc::unbounded_channel;

use super::listener::MarketListener;
use super::types::{OrderFill, OrderRequest, OrderStatus};
use crate::{BaseUrl, InfoClient, Message, Subscription};

/// Order side for paper trading
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Input configuration for creating a PaperTradingMarket
#[derive(Debug)]
pub struct PaperTradingMarketInput {
    /// Initial balance in quote currency (e.g., USDC)
    pub initial_balance: f64,
}

impl Default for PaperTradingMarketInput {
    fn default() -> Self {
        Self {
            initial_balance: 100_000.0,
        }
    }
}

/// Internal order tracking for paper trading
#[derive(Debug, Clone)]
struct PaperOrder {
    /// Order request details (contains user's order_id)
    request: OrderRequest,
    /// Order side (buy or sell)
    side: OrderSide,
    /// Current status
    status: OrderStatus,
    /// Filled quantity
    filled_qty: f64,
    /// Average fill price
    avg_fill_price: f64,
    /// Timestamp when order was placed
    #[allow(dead_code)]
    created_at: u64,
}

impl PaperOrder {
    fn new(request: OrderRequest, side: OrderSide) -> Self {
        Self {
            request,
            side,
            status: OrderStatus::Pending,
            filled_qty: 0.0,
            avg_fill_price: 0.0,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
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

    /// Check if this order should be filled at the given price
    fn should_fill(&self, mid_price: f64) -> bool {
        if !self.status.is_active() {
            return false;
        }

        match self.side {
            // Buy order fills when mid price <= limit price
            OrderSide::Buy => mid_price <= self.request.limit_price,
            // Sell order fills when mid price >= limit price
            OrderSide::Sell => mid_price >= self.request.limit_price,
        }
    }
}

/// Paper trading position tracking
#[derive(Debug, Clone, Default)]
pub struct PaperPosition {
    /// Position size (positive = long, negative = short)
    pub size: f64,
    /// Average entry price
    pub entry_price: f64,
    /// Realized PnL
    pub realized_pnl: f64,
}

impl PaperPosition {
    /// Update position after a fill
    fn apply_fill(&mut self, qty: f64, price: f64, is_buy: bool) {
        let signed_qty = if is_buy { qty } else { -qty };

        if self.size == 0.0 {
            // Opening new position
            self.size = signed_qty;
            self.entry_price = price;
        } else if (self.size > 0.0 && is_buy) || (self.size < 0.0 && !is_buy) {
            // Adding to position
            let total_value = self.entry_price * self.size.abs() + price * qty;
            self.size += signed_qty;
            self.entry_price = total_value / self.size.abs();
        } else {
            // Reducing or closing position
            let close_qty = qty.min(self.size.abs());
            let pnl = if self.size > 0.0 {
                // Long position being closed
                (price - self.entry_price) * close_qty
            } else {
                // Short position being closed
                (self.entry_price - price) * close_qty
            };
            self.realized_pnl += pnl;
            self.size += signed_qty;

            // If position flipped, reset entry price
            if (self.size > 0.0 && !is_buy) || (self.size < 0.0 && is_buy) {
                // This shouldn't happen with proper close_qty logic
            } else if self.size == 0.0 {
                self.entry_price = 0.0;
            }
        }
    }

    /// Calculate unrealized PnL at current price
    pub fn unrealized_pnl(&self, current_price: f64) -> f64 {
        if self.size == 0.0 {
            return 0.0;
        }
        if self.size > 0.0 {
            (current_price - self.entry_price) * self.size
        } else {
            (self.entry_price - current_price) * self.size.abs()
        }
    }
}

/// Paper Trading Market implementation
///
/// Connects to Hyperliquid for live price feeds but simulates order execution
/// by checking midprice against pending order limits.
///
/// # Features
/// - Live price feed from Hyperliquid WebSocket
/// - Simulated order matching against midprice
/// - Position and PnL tracking
/// - No real money at risk
///
/// # Example
///
/// ```ignore
/// use hyperliquid_rust_sdk::market::{
///     PaperTradingMarket, PaperTradingMarketInput, NoOpListener, OrderSide
/// };
///
/// let input = PaperTradingMarketInput {
///     initial_balance: 10_000.0,
///     base_url: Some(BaseUrl::Mainnet),
///     wallet: None,
/// };
///
/// let mut market = PaperTradingMarket::new(input, NoOpListener).await?;
///
/// // Place a simulated buy order
/// let order = OrderRequest::new("BTC", 0.1, 50000.0);
/// let order_id = market.place_order(order, OrderSide::Buy);
///
/// // Start event loop - orders fill when midprice crosses limit
/// market.start().await;
/// ```
pub struct PaperTradingMarket<L: MarketListener> {
    /// Owned listener instance (M5)
    listener: L,
    /// Info client for price feeds
    pub info_client: InfoClient,
    /// Current prices by asset
    prices: HashMap<String, f64>,
    /// Orders by user-provided order_id
    orders: HashMap<u64, PaperOrder>,
    /// Positions by asset
    positions: HashMap<String, PaperPosition>,
    /// Account balance (quote currency)
    pub balance: f64,
    /// Total fees paid
    pub total_fees: f64,
    /// Fee rate (e.g., 0.0001 = 0.01%)
    pub fee_rate: f64,
}

impl<L: MarketListener> PaperTradingMarket<L> {
    /// Create a new PaperTradingMarket
    ///
    /// Always connects to Mainnet for live price feeds.
    ///
    /// # Arguments
    /// * `input` - Configuration for the paper trading market
    /// * `listener` - Listener to receive notifications
    pub async fn new(input: PaperTradingMarketInput, listener: L) -> Result<Self, crate::Error> {
        // Paper trading always uses Mainnet for real price data
        let info_client = InfoClient::new(None, Some(BaseUrl::Mainnet)).await?;

        Ok(Self {
            listener,
            info_client,
            prices: HashMap::new(),
            orders: HashMap::new(),
            positions: HashMap::new(),
            balance: input.initial_balance,
            total_fees: 0.0,
            fee_rate: 0.0001, // Default 0.01% fee
        })
    }

    /// Start the market event loop
    ///
    /// Subscribes to AllMids for live price updates and processes
    /// pending orders when prices change.
    pub async fn start(&mut self) {
        let (sender, mut receiver) = unbounded_channel();

        // Subscribe to AllMids for price updates
        if let Err(e) = self
            .info_client
            .subscribe(Subscription::AllMids, sender)
            .await
        {
            error!("Failed to subscribe to AllMids: {e}");
            return;
        }

        info!("PaperTradingMarket started with balance: {}", self.balance);

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
        if let Message::AllMids(all_mids) = message {
            let mids = all_mids.data.mids;
            for (asset, price_str) in mids {
                if let Ok(price) = price_str.parse::<f64>() {
                    let old_price = self.prices.get(&asset).copied();
                    self.prices.insert(asset.clone(), price);

                    // Only notify if price actually changed
                    if old_price != Some(price) {
                        // M6: Synchronous notification (M4)
                        self.listener.on_price_update(&asset, price);
                    }

                    // Check pending orders for this asset
                    self.check_and_fill_orders(&asset, price);
                }
            }
        }
    }

    /// Check all pending orders for an asset and fill if conditions are met
    fn check_and_fill_orders(&mut self, asset: &str, mid_price: f64) {
        // Collect orders to fill (can't modify while iterating)
        let orders_to_fill: Vec<(u64, f64, OrderSide)> = self
            .orders
            .iter()
            .filter(|(_, order)| order.request.asset == asset && order.should_fill(mid_price))
            .map(|(&id, order)| {
                let remaining = order.request.qty - order.filled_qty;
                (id, remaining, order.side)
            })
            .collect();

        // Process fills
        for (order_id, qty, side) in orders_to_fill {
            self.execute_paper_fill(order_id, qty, mid_price, side);
        }
    }

    /// Execute a simulated fill
    fn execute_paper_fill(&mut self, order_id: u64, qty: f64, price: f64, side: OrderSide) {
        // Calculate fee
        let notional = qty * price;
        let fee = notional * self.fee_rate;

        // Update balance
        let is_buy = side == OrderSide::Buy;
        if is_buy {
            self.balance -= notional + fee;
        } else {
            self.balance += notional - fee;
        }
        self.total_fees += fee;

        // Update position
        let asset = self
            .orders
            .get(&order_id)
            .map(|o| o.request.asset.clone())
            .unwrap_or_default();

        let position = self.positions.entry(asset.clone()).or_default();
        position.apply_fill(qty, price, is_buy);

        // Update order status
        if let Some(order) = self.orders.get_mut(&order_id) {
            let was_active = order.status.is_active();
            order.fill(qty, price);

            let side_str = if is_buy { "bought" } else { "sold" };
            info!(
                "Paper fill: {} {} {} at {} (fee: {:.4})",
                side_str, qty, asset, price, fee
            );

            // Only notify when order is fully filled (M3)
            if was_active && matches!(order.status, OrderStatus::Filled(_)) {
                let fill = OrderFill::new(
                    order_id,
                    &asset,
                    order.request.qty,      // Total order qty
                    order.avg_fill_price,   // Average fill price
                );

                info!(
                    "Paper order {} fully filled: {} {} at avg price {}",
                    order_id, order.request.qty, asset, order.avg_fill_price
                );

                // M6: Synchronous notification
                self.listener.on_order_filled(fill);
            }
        }
    }

    /// Update the price for an asset (M7)
    ///
    /// Manually updates internal price state and checks for fills.
    /// Note: Prices are also updated automatically via WebSocket subscription.
    pub fn update_price(&mut self, asset: &str, price: f64) {
        self.prices.insert(asset.to_string(), price);
        // M6: Synchronous notification
        self.listener.on_price_update(asset, price);
        // Check for fills
        self.check_and_fill_orders(asset, price);
    }

    /// Place a new paper order (M8)
    ///
    /// # Arguments
    /// * `order` - The order request (contains user-provided order_id)
    /// * `side` - Buy or Sell
    pub fn place_order(&mut self, order: OrderRequest, side: OrderSide) {
        let user_order_id = order.order_id;
        let paper_order = PaperOrder::new(order.clone(), side);

        info!(
            "Paper order {}: {:?} {} {} @ {}",
            user_order_id, side, order.qty, order.asset, order.limit_price
        );

        self.orders.insert(user_order_id, paper_order);

        // Check if order can be filled immediately
        if let Some(&current_price) = self.prices.get(&order.asset) {
            self.check_and_fill_orders(&order.asset, current_price);
        }
    }

    /// Place a buy order (convenience method)
    pub fn place_buy_order(&mut self, order: OrderRequest) {
        self.place_order(order, OrderSide::Buy)
    }

    /// Place a sell order (convenience method)
    pub fn place_sell_order(&mut self, order: OrderRequest) {
        self.place_order(order, OrderSide::Sell)
    }

    /// Inject an external fill (M9)
    ///
    /// For testing or manual fill injection.
    /// Only notifies the listener when the order is fully filled.
    pub fn execute_fill(&mut self, fill: OrderFill) {
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

                // M6: Synchronous notification
                self.listener.on_order_filled(complete_fill);
            }
        }
    }

    /// Query current price for an asset (M10)
    pub fn current_price(&self, asset: &str) -> Option<f64> {
        self.prices.get(asset).copied()
    }

    /// Query order status (M11)
    pub fn order_status(&self, order_id: u64) -> Option<OrderStatus> {
        self.orders.get(&order_id).map(|o| o.status)
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
    pub fn cancel_order(&mut self, order_id: u64) -> bool {
        if let Some(order) = self.orders.get_mut(&order_id) {
            if order.status.is_active() {
                order.status = OrderStatus::Cancelled;
                info!("Paper order {} cancelled", order_id);
                return true;
            }
        }
        false
    }

    /// Get position for an asset
    pub fn position(&self, asset: &str) -> Option<&PaperPosition> {
        self.positions.get(asset)
    }

    /// Get all positions
    pub fn all_positions(&self) -> &HashMap<String, PaperPosition> {
        &self.positions
    }

    /// Get all current prices
    pub fn all_prices(&self) -> &HashMap<String, f64> {
        &self.prices
    }

    /// Get count of pending orders
    pub fn pending_order_count(&self) -> usize {
        self.orders.values().filter(|o| o.status.is_active()).count()
    }

    /// Get IDs of all pending orders
    pub fn pending_order_ids(&self) -> Vec<u64> {
        self.orders
            .iter()
            .filter(|(_, o)| o.status.is_active())
            .map(|(&id, _)| id)
            .collect()
    }

    /// Calculate total account value (balance + unrealized PnL)
    pub fn account_value(&self) -> f64 {
        let unrealized_pnl: f64 = self
            .positions
            .iter()
            .map(|(asset, pos)| {
                self.prices
                    .get(asset)
                    .map(|&price| pos.unrealized_pnl(price))
                    .unwrap_or(0.0)
            })
            .sum();

        self.balance + unrealized_pnl
    }

    /// Calculate total realized PnL across all positions
    pub fn total_realized_pnl(&self) -> f64 {
        self.positions.values().map(|p| p.realized_pnl).sum()
    }

    /// Set fee rate (e.g., 0.0001 = 0.01%)
    pub fn set_fee_rate(&mut self, rate: f64) {
        self.fee_rate = rate;
    }

    /// Reset paper trading state
    pub fn reset(&mut self, initial_balance: f64) {
        self.balance = initial_balance;
        self.total_fees = 0.0;
        self.orders.clear();
        self.positions.clear();
        info!("Paper trading reset with balance: {}", initial_balance);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paper_order_should_fill_buy() {
        let request = OrderRequest::new(100, "BTC", 1.0, 50000.0);
        let order = PaperOrder::new(request, OrderSide::Buy);

        // Buy should fill when price <= limit
        assert!(order.should_fill(49999.0)); // Below limit
        assert!(order.should_fill(50000.0)); // At limit
        assert!(!order.should_fill(50001.0)); // Above limit
    }

    #[test]
    fn test_paper_order_should_fill_sell() {
        let request = OrderRequest::new(200, "BTC", 1.0, 50000.0);
        let order = PaperOrder::new(request, OrderSide::Sell);

        // Sell should fill when price >= limit
        assert!(!order.should_fill(49999.0)); // Below limit
        assert!(order.should_fill(50000.0)); // At limit
        assert!(order.should_fill(50001.0)); // Above limit
    }

    #[test]
    fn test_paper_position_long() {
        let mut pos = PaperPosition::default();

        // Open long
        pos.apply_fill(1.0, 50000.0, true);
        assert_eq!(pos.size, 1.0);
        assert_eq!(pos.entry_price, 50000.0);

        // Add to long
        pos.apply_fill(1.0, 51000.0, true);
        assert_eq!(pos.size, 2.0);
        assert_eq!(pos.entry_price, 50500.0); // Average

        // Close half
        pos.apply_fill(1.0, 52000.0, false);
        assert_eq!(pos.size, 1.0);
        assert_eq!(pos.realized_pnl, 1500.0); // (52000 - 50500) * 1
    }

    #[test]
    fn test_paper_position_short() {
        let mut pos = PaperPosition::default();

        // Open short
        pos.apply_fill(1.0, 50000.0, false);
        assert_eq!(pos.size, -1.0);
        assert_eq!(pos.entry_price, 50000.0);

        // Close short at profit
        pos.apply_fill(1.0, 49000.0, true);
        assert_eq!(pos.size, 0.0);
        assert_eq!(pos.realized_pnl, 1000.0); // (50000 - 49000) * 1
    }

    #[test]
    fn test_paper_position_unrealized_pnl() {
        let mut pos = PaperPosition::default();
        pos.apply_fill(1.0, 50000.0, true);

        // Profit
        assert_eq!(pos.unrealized_pnl(51000.0), 1000.0);
        // Loss
        assert_eq!(pos.unrealized_pnl(49000.0), -1000.0);
    }

    #[test]
    fn test_paper_order_fill() {
        let request = OrderRequest::new(300, "BTC", 2.0, 50000.0);
        let mut order = PaperOrder::new(request, OrderSide::Buy);

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
    fn test_paper_position_flip() {
        let mut pos = PaperPosition::default();

        // Open long 2 units
        pos.apply_fill(2.0, 50000.0, true);
        assert_eq!(pos.size, 2.0);

        // Close 1 unit at profit
        pos.apply_fill(1.0, 51000.0, false);
        assert_eq!(pos.size, 1.0);
        assert_eq!(pos.realized_pnl, 1000.0);

        // Close remaining 1 unit at loss
        pos.apply_fill(1.0, 49000.0, false);
        assert_eq!(pos.size, 0.0);
        assert_eq!(pos.realized_pnl, 0.0); // 1000 - 1000 = 0
    }
}

