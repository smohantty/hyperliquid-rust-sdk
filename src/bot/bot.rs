//! Bot - MarketListener that wraps a Strategy

use crate::market::{MarketListener, OrderFill, OrderRequest};
use crate::strategy::Strategy;

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
}

impl<S: Strategy> MarketListener for Bot<S> {
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
        self.strategy.on_price_update(asset, price)
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
}
