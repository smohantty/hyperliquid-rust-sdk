//! Bot - MarketListener that wraps a Strategy

use log::{debug, info};

use crate::market::{MarketListener, OrderFill, OrderRequest};
use crate::strategy::{Strategy, StrategyStatus};

/// Bot wraps a Strategy and implements MarketListener
///
/// The bot receives market events (price updates, fills), calls the strategy,
/// and returns the orders from the strategy. The market then places these orders.
///
/// # Example
///
/// ```ignore
/// use hyperliquid_rust_sdk::bot::Bot;
/// use hyperliquid_rust_sdk::market::{HyperliquidMarket, HyperliquidMarketInput};
///
/// // Create bot with strategy
/// let bot = Bot::new(MyStrategy::new());
///
/// // Pass bot as listener to market
/// let mut market = HyperliquidMarket::new(input, bot).await?;
///
/// // Market runs event loop, calls bot callbacks, places returned orders
/// market.start().await;
/// ```
pub struct Bot<S: Strategy> {
    /// The trading strategy
    strategy: S,
}

impl<S: Strategy> Bot<S> {
    /// Create a new bot wrapping the given strategy
    pub fn new(strategy: S) -> Self {
        Self { strategy }
    }

    /// Get a reference to the underlying strategy
    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    /// Get a mutable reference to the underlying strategy
    pub fn strategy_mut(&mut self) -> &mut S {
        &mut self.strategy
    }

    /// Call strategy's on_start and return initial orders
    pub fn start(&mut self) -> Vec<OrderRequest> {
        self.strategy.on_start()
    }

    /// Call strategy's on_stop and return final orders
    pub fn stop(&mut self) -> Vec<OrderRequest> {
        self.strategy.on_stop()
    }

    /// Get the strategy's current status
    ///
    /// Returns a `StrategyStatus` containing PnL, position, and other metrics.
    /// Useful for monitoring dashboards and APIs.
    pub fn status(&self) -> StrategyStatus {
        self.strategy.status()
    }

    /// Get the strategy's status as JSON
    ///
    /// Convenience method for HTTP APIs.
    pub fn status_json(&self) -> serde_json::Value {
        serde_json::to_value(self.strategy.status()).unwrap_or_default()
    }

    pub fn render_dashboard(&self) -> String {
        // Use generic dashboard for all strategies
        crate::bot::dashboard::render_dashboard(&self.strategy.status())
    }
}

impl<S: Strategy> MarketListener for Bot<S> {
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
        debug!(
            "Bot[{}]: price update {} = {:.4}",
            self.strategy.name(),
            asset,
            price
        );
        let orders = self.strategy.on_price_update(asset, price);
        if !orders.is_empty() {
            info!(
                "Bot[{}]: strategy returned {} order(s) on price update",
                self.strategy.name(),
                orders.len()
            );
        }
        orders
    }

    fn on_order_filled(&mut self, fill: OrderFill) -> Vec<OrderRequest> {
        self.strategy.on_order_filled(&fill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::NoOpStrategy;

    #[test]
    fn test_bot_new() {
        let bot = Bot::new(NoOpStrategy);
        assert_eq!(bot.strategy().name(), "noop");
    }

    #[test]
    fn test_bot_noop_strategy() {
        let mut bot = Bot::new(NoOpStrategy);

        // NoOp strategy returns no orders
        let orders = bot.on_price_update("BTC", 50000.0);
        assert!(orders.is_empty());

        let fill = OrderFill::new(1, "BTC", 1.0, 50000.0);
        let orders = bot.on_order_filled(fill);
        assert!(orders.is_empty());
    }

    // Test strategy that generates orders
    struct TestStrategy {
        should_buy: bool,
        next_order_id: u64,
    }

    impl TestStrategy {
        fn new(should_buy: bool) -> Self {
            Self {
                should_buy,
                next_order_id: 0,
            }
        }
    }

    impl Strategy for TestStrategy {
        fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
            if self.should_buy {
                self.next_order_id += 1;
                vec![OrderRequest::buy(self.next_order_id, asset, 1.0, price)]
            } else {
                vec![]
            }
        }

        fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
            // After buy fills, place a sell
            self.next_order_id += 1;
            vec![OrderRequest::sell(
                self.next_order_id,
                &fill.asset,
                fill.qty,
                fill.price * 1.01,
            )]
        }

        fn on_start(&mut self) -> Vec<OrderRequest> {
            if self.should_buy {
                self.next_order_id += 1;
                vec![OrderRequest::buy(self.next_order_id, "BTC", 0.1, 50000.0)]
            } else {
                vec![]
            }
        }
    }

    #[test]
    fn test_bot_returns_orders_on_price_update() {
        let mut bot = Bot::new(TestStrategy::new(true));

        let orders = bot.on_price_update("BTC", 50000.0);

        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "BTC");
        assert_eq!(orders[0].limit_price, 50000.0);
    }

    #[test]
    fn test_bot_returns_orders_on_fill() {
        let mut bot = Bot::new(TestStrategy::new(false));

        let fill = OrderFill::new(1, "ETH", 2.0, 3000.0);
        let orders = bot.on_order_filled(fill);

        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "ETH");
        assert!((orders[0].limit_price - 3030.0).abs() < 0.01); // 1% above fill
    }

    #[test]
    fn test_bot_start() {
        let mut bot = Bot::new(TestStrategy::new(true));

        let orders = bot.start();

        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "BTC");
    }

    #[test]
    fn test_bot_strategy_access() {
        let mut bot = Bot::new(TestStrategy::new(true));

        assert!(bot.strategy().should_buy);

        bot.strategy_mut().should_buy = false;
        assert!(!bot.strategy().should_buy);
    }

    #[test]
    fn test_bot_status() {
        let bot = Bot::new(NoOpStrategy);

        let status = bot.status();
        assert_eq!(status.name, "noop");
    }

    #[test]
    fn test_bot_status_json() {
        let bot = Bot::new(NoOpStrategy);

        let json = bot.status_json();
        assert!(json.is_object());
        assert_eq!(json["name"], "noop");
    }

    #[test]
    fn test_bot_render_dashboard() {
        let bot = Bot::new(NoOpStrategy);

        let html = bot.render_dashboard();
        assert!(html.contains("noop"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    // Strategy with custom status
    struct StatusStrategy {
        position: f64,
        pnl: f64,
    }

    impl Strategy for StatusStrategy {
        fn on_price_update(&mut self, _asset: &str, _price: f64) -> Vec<OrderRequest> {
            vec![]
        }

        fn on_order_filled(&mut self, _fill: &OrderFill) -> Vec<OrderRequest> {
            vec![]
        }

        fn name(&self) -> &str {
            "status_test"
        }

        fn status(&self) -> StrategyStatus {
            StrategyStatus::new("status_test", "BTC")
                .with_status("Running")
                .with_position(self.position)
                .with_pnl(self.pnl, 0.0, 1.0)
                .with_custom(serde_json::json!({
                    "custom_field": "test_value"
                }))
        }
    }

    #[test]
    fn test_bot_custom_strategy_status() {
        let bot = Bot::new(StatusStrategy {
            position: 1.5,
            pnl: 100.0,
        });

        let status = bot.status();
        assert_eq!(status.name, "status_test");
        assert_eq!(status.asset, "BTC");
        assert_eq!(status.position, 1.5);
        assert_eq!(status.realized_pnl, 100.0);
        assert!((status.net_profit() - 99.0).abs() < 0.001);
        assert_eq!(status.custom["custom_field"], "test_value");
    }
}
