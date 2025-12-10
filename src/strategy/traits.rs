//! Strategy trait definition

use crate::market::{OrderFill, OrderRequest};
use serde::{Deserialize, Serialize};

/// Strategy status for monitoring and display
///
/// Contains common fields that most strategies want to expose.
/// Strategies can extend this with custom data in the `custom` field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StrategyStatus {
    /// Strategy name
    pub name: String,
    /// Current status description (e.g., "Running", "WaitingForEntry")
    pub status: String,
    /// Current asset being traded
    pub asset: String,
    /// Current price
    pub current_price: f64,
    /// Current position size
    pub position: f64,
    /// Realized PnL
    pub realized_pnl: f64,
    /// Unrealized PnL
    pub unrealized_pnl: f64,
    /// Total fees paid
    pub total_fees: f64,
    /// Number of completed trades (round trips)
    pub trade_count: u32,
    /// Active order count
    pub active_orders: usize,
    /// Strategy-specific custom data (JSON)
    #[serde(default)]
    pub custom: serde_json::Value,
}

impl StrategyStatus {
    /// Create a new status with basic info
    pub fn new(name: impl Into<String>, asset: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            asset: asset.into(),
            status: "Initialized".to_string(),
            ..Default::default()
        }
    }

    /// Net profit (realized PnL - fees)
    pub fn net_profit(&self) -> f64 {
        self.realized_pnl - self.total_fees
    }

    /// Total PnL (realized + unrealized - fees)
    pub fn total_pnl(&self) -> f64 {
        self.realized_pnl + self.unrealized_pnl - self.total_fees
    }

    /// Builder: set status
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }

    /// Builder: set price
    pub fn with_price(mut self, price: f64) -> Self {
        self.current_price = price;
        self
    }

    /// Builder: set position
    pub fn with_position(mut self, position: f64) -> Self {
        self.position = position;
        self
    }

    /// Builder: set PnL values
    pub fn with_pnl(mut self, realized: f64, unrealized: f64, fees: f64) -> Self {
        self.realized_pnl = realized;
        self.unrealized_pnl = unrealized;
        self.total_fees = fees;
        self
    }

    /// Builder: set custom data
    pub fn with_custom(mut self, custom: serde_json::Value) -> Self {
        self.custom = custom;
        self
    }
}

/// Strategy interface for trading logic
///
/// A strategy receives market events (price updates, order fills) and returns
/// orders to place. The strategy is completely decoupled from the market/exchange
/// - it doesn't know how or where orders are executed.
///
/// # Lifecycle
///
/// 1. Market calls `on_price_update` when prices change
/// 2. Strategy returns `Vec<OrderRequest>` with orders to place
/// 3. Market executes those orders
/// 4. When orders fill, market calls `on_order_filled`
/// 5. Strategy updates internal state and may return more orders
///
/// # Testing
///
/// Strategies can be tested in isolation by directly calling the trait methods:
///
/// ```rust
/// use hyperliquid_rust_sdk::strategy::Strategy;
/// use hyperliquid_rust_sdk::market::{OrderRequest, OrderFill};
///
/// fn test_strategy(strategy: &mut impl Strategy) {
///     // Simulate price update
///     let orders = strategy.on_price_update("BTC", 50000.0);
///     assert!(!orders.is_empty());
///
///     // Simulate fill
///     let fill = OrderFill::new(1, "BTC", 0.1, 50000.0);
///     let orders = strategy.on_order_filled(&fill);
///     // Assert on orders...
/// }
/// ```
///
/// # Example Implementation
///
/// ```rust
/// use hyperliquid_rust_sdk::strategy::Strategy;
/// use hyperliquid_rust_sdk::market::{OrderRequest, OrderFill};
///
/// struct MomentumStrategy {
///     last_price: Option<f64>,
///     position: f64,
///     next_order_id: u64,
/// }
///
/// impl Strategy for MomentumStrategy {
///     fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
///         let orders = if let Some(last) = self.last_price {
///             let change = (price - last) / last;
///             if change > 0.01 && self.position <= 0.0 {
///                 // Price up 1%+, go long
///                 self.next_order_id += 1;
///                 vec![OrderRequest::buy(self.next_order_id, asset, 1.0, price)]
///             } else if change < -0.01 && self.position >= 0.0 {
///                 // Price down 1%+, go short
///                 self.next_order_id += 1;
///                 vec![OrderRequest::sell(self.next_order_id, asset, 1.0, price)]
///             } else {
///                 vec![]
///             }
///         } else {
///             vec![]
///         };
///         self.last_price = Some(price);
///         orders
///     }
///
///     fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
///         // Update position tracking
///         // (simplified - real impl would track buy vs sell)
///         self.position += fill.qty;
///         vec![]
///     }
/// }
/// ```
pub trait Strategy {
    /// Called when a price update is received
    ///
    /// # Arguments
    /// * `asset` - The asset that had a price update
    /// * `price` - The new price
    ///
    /// # Returns
    /// Orders to place in response to the price update
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest>;

    /// Called when an order is filled
    ///
    /// # Arguments
    /// * `fill` - Details about the filled order
    ///
    /// # Returns
    /// Orders to place in response to the fill
    fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest>;

    /// Called to initialize the strategy (optional)
    ///
    /// Override this to perform setup when the strategy starts.
    /// Default implementation returns no orders.
    fn on_start(&mut self) -> Vec<OrderRequest> {
        vec![]
    }

    /// Called when the strategy is stopped (optional)
    ///
    /// Override this to perform cleanup. Can return orders to close positions.
    /// Default implementation returns no orders.
    fn on_stop(&mut self) -> Vec<OrderRequest> {
        vec![]
    }

    /// Get the strategy name (optional)
    ///
    /// Useful for logging and debugging.
    fn name(&self) -> &str {
        "unnamed_strategy"
    }

    /// Get the strategy's current status for monitoring
    ///
    /// Returns a `StrategyStatus` containing common metrics like PnL, position,
    /// active orders, etc. Override this to provide strategy-specific data.
    ///
    /// This is called periodically by the bot to update dashboards/APIs.
    fn status(&self) -> StrategyStatus {
        StrategyStatus::new(self.name(), "")
    }

    /// Render a custom HTML dashboard (optional)
    ///
    /// Override this to provide a custom HTML dashboard for the strategy.
    /// If `None` is returned, a default dashboard will be generated from `status()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// fn render_dashboard(&self) -> Option<String> {
    ///     Some(format!(r#"
    ///         <div class="my-strategy">
    ///             <h1>{}</h1>
    ///             <p>Position: {}</p>
    ///         </div>
    ///     "#, self.name(), self.position))
    /// }
    /// ```
    fn render_dashboard(&self) -> Option<String> {
        None
    }
}

// Implement Strategy for Box<dyn Strategy> to allow dynamic dispatch
impl Strategy for Box<dyn Strategy + Send + Sync> {
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
        (**self).on_price_update(asset, price)
    }

    fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
        (**self).on_order_filled(fill)
    }

    fn on_start(&mut self) -> Vec<OrderRequest> {
        (**self).on_start()
    }

    fn on_stop(&mut self) -> Vec<OrderRequest> {
        (**self).on_stop()
    }

    fn name(&self) -> &str {
        (**self).name()
    }

    fn status(&self) -> StrategyStatus {
        (**self).status()
    }

    fn render_dashboard(&self) -> Option<String> {
        (**self).render_dashboard()
    }
}

/// A no-op strategy that never generates orders
///
/// Useful for testing markets without strategy logic.
#[derive(Debug, Default, Clone)]
pub struct NoOpStrategy;

impl Strategy for NoOpStrategy {
    fn on_price_update(&mut self, _asset: &str, _price: f64) -> Vec<OrderRequest> {
        vec![]
    }

    fn on_order_filled(&mut self, _fill: &OrderFill) -> Vec<OrderRequest> {
        vec![]
    }

    fn name(&self) -> &str {
        "noop"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_strategy() {
        let mut strategy = NoOpStrategy;

        let orders = strategy.on_price_update("BTC", 50000.0);
        assert!(orders.is_empty());

        let fill = OrderFill::new(1, "BTC", 1.0, 50000.0);
        let orders = strategy.on_order_filled(&fill);
        assert!(orders.is_empty());

        assert_eq!(strategy.name(), "noop");
    }

    #[test]
    fn test_strategy_status() {
        let status = StrategyStatus::new("TestStrategy", "BTC")
            .with_status("Running")
            .with_price(50000.0)
            .with_position(1.5)
            .with_pnl(100.0, 50.0, 10.0);

        assert_eq!(status.name, "TestStrategy");
        assert_eq!(status.asset, "BTC");
        assert_eq!(status.status, "Running");
        assert_eq!(status.current_price, 50000.0);
        assert_eq!(status.position, 1.5);
        assert_eq!(status.realized_pnl, 100.0);
        assert_eq!(status.unrealized_pnl, 50.0);
        assert_eq!(status.total_fees, 10.0);
        assert!((status.net_profit() - 90.0).abs() < 0.001);
        assert!((status.total_pnl() - 140.0).abs() < 0.001);
    }

    #[test]
    fn test_strategy_status_custom_data() {
        let custom = serde_json::json!({
            "grid_levels": 10,
            "lower_price": 45000.0,
            "upper_price": 55000.0
        });

        let status = StrategyStatus::new("GridStrategy", "BTC").with_custom(custom);

        assert!(status.custom.get("grid_levels").is_some());
        assert_eq!(status.custom["grid_levels"], 10);
    }

    #[test]
    fn test_default_status() {
        let strategy = NoOpStrategy;
        let status = strategy.status();

        assert_eq!(status.name, "noop");
        assert!(status.asset.is_empty());
    }

    #[test]
    fn test_default_render_dashboard() {
        let strategy = NoOpStrategy;
        let dashboard = strategy.render_dashboard();

        // Default returns None
        assert!(dashboard.is_none());
    }

    // Example: Simple threshold strategy for testing
    struct ThresholdStrategy {
        threshold: f64,
        has_position: bool,
        next_order_id: u64,
    }

    impl ThresholdStrategy {
        fn new(threshold: f64) -> Self {
            Self {
                threshold,
                has_position: false,
                next_order_id: 0,
            }
        }
    }

    impl Strategy for ThresholdStrategy {
        fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
            if !self.has_position && price <= self.threshold {
                self.next_order_id += 1;
                vec![OrderRequest::buy(self.next_order_id, asset, 1.0, price)]
            } else {
                vec![]
            }
        }

        fn on_order_filled(&mut self, _fill: &OrderFill) -> Vec<OrderRequest> {
            self.has_position = true;
            vec![]
        }

        fn name(&self) -> &str {
            "threshold"
        }
    }

    #[test]
    fn test_threshold_strategy() {
        let mut strategy = ThresholdStrategy::new(50000.0);

        // Price above threshold - no orders
        let orders = strategy.on_price_update("BTC", 51000.0);
        assert!(orders.is_empty());

        // Price at threshold - should buy
        let orders = strategy.on_price_update("BTC", 50000.0);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].order_id, 1);
        assert_eq!(orders[0].limit_price, 50000.0);

        // Simulate fill
        let fill = OrderFill::new(1, "BTC", 1.0, 50000.0);
        let orders = strategy.on_order_filled(&fill);
        assert!(orders.is_empty());

        // Price drops again - no orders (already has position)
        let orders = strategy.on_price_update("BTC", 49000.0);
        assert!(orders.is_empty());
    }

    #[test]
    fn test_strategy_lifecycle() {
        let mut strategy = ThresholdStrategy::new(50000.0);

        // on_start default
        let orders = strategy.on_start();
        assert!(orders.is_empty());

        // on_stop default
        let orders = strategy.on_stop();
        assert!(orders.is_empty());
    }
}
