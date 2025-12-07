//! Bot - MarketListener that wraps a Strategy

use crate::market::{MarketListener, OrderFill, OrderRequest};
use crate::strategy::Strategy;

/// Bot wraps a Strategy and implements MarketListener
///
/// The bot receives market events (price updates, fills), calls the strategy,
/// and collects the orders returned by the strategy. The application is
/// responsible for retrieving and executing these orders on the market.
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
/// // After market processes events, get orders from bot
/// for order in market.listener_mut().take_pending_orders() {
///     market.place_order(order).await;
/// }
/// ```
pub struct Bot<S: Strategy> {
    /// The trading strategy
    strategy: S,
    /// Orders pending execution (collected from strategy)
    pending_orders: Vec<OrderRequest>,
}

impl<S: Strategy> Bot<S> {
    /// Create a new bot wrapping the given strategy
    pub fn new(strategy: S) -> Self {
        Self {
            strategy,
            pending_orders: Vec::new(),
        }
    }

    /// Take all pending orders, leaving the queue empty
    ///
    /// Call this after market events to get orders that need to be executed.
    pub fn take_pending_orders(&mut self) -> Vec<OrderRequest> {
        std::mem::take(&mut self.pending_orders)
    }

    /// Check if there are pending orders
    pub fn has_pending_orders(&self) -> bool {
        !self.pending_orders.is_empty()
    }

    /// Get the count of pending orders
    pub fn pending_order_count(&self) -> usize {
        self.pending_orders.len()
    }

    /// Get a reference to the underlying strategy
    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    /// Get a mutable reference to the underlying strategy
    pub fn strategy_mut(&mut self) -> &mut S {
        &mut self.strategy
    }

    /// Call strategy's on_start and collect initial orders
    pub fn start(&mut self) {
        let orders = self.strategy.on_start();
        self.pending_orders.extend(orders);
    }

    /// Call strategy's on_stop and collect final orders
    pub fn stop(&mut self) {
        let orders = self.strategy.on_stop();
        self.pending_orders.extend(orders);
    }
}

impl<S: Strategy> MarketListener for Bot<S> {
    fn on_price_update(&mut self, asset: &str, price: f64) {
        let orders = self.strategy.on_price_update(asset, price);
        self.pending_orders.extend(orders);
    }

    fn on_order_filled(&mut self, fill: OrderFill) {
        let orders = self.strategy.on_order_filled(&fill);
        self.pending_orders.extend(orders);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::NoOpStrategy;

    #[test]
    fn test_bot_new() {
        let bot = Bot::new(NoOpStrategy);
        assert!(!bot.has_pending_orders());
        assert_eq!(bot.pending_order_count(), 0);
    }

    #[test]
    fn test_bot_noop_strategy() {
        let mut bot = Bot::new(NoOpStrategy);

        // NoOp strategy returns no orders
        bot.on_price_update("BTC", 50000.0);
        assert!(!bot.has_pending_orders());

        let fill = OrderFill::new(1, "BTC", 1.0, 50000.0);
        bot.on_order_filled(fill);
        assert!(!bot.has_pending_orders());
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
    fn test_bot_collects_orders_on_price_update() {
        let mut bot = Bot::new(TestStrategy::new(true));

        bot.on_price_update("BTC", 50000.0);

        assert!(bot.has_pending_orders());
        assert_eq!(bot.pending_order_count(), 1);

        let orders = bot.take_pending_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "BTC");
        assert_eq!(orders[0].limit_price, 50000.0);

        // After take, should be empty
        assert!(!bot.has_pending_orders());
    }

    #[test]
    fn test_bot_collects_orders_on_fill() {
        let mut bot = Bot::new(TestStrategy::new(false));

        let fill = OrderFill::new(1, "ETH", 2.0, 3000.0);
        bot.on_order_filled(fill);

        assert!(bot.has_pending_orders());
        let orders = bot.take_pending_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "ETH");
        assert!((orders[0].limit_price - 3030.0).abs() < 0.01); // 1% above fill
    }

    #[test]
    fn test_bot_start_stop() {
        let mut bot = Bot::new(TestStrategy::new(true));

        bot.start();
        assert!(bot.has_pending_orders());

        let orders = bot.take_pending_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "BTC");
    }

    #[test]
    fn test_bot_accumulates_orders() {
        let mut bot = Bot::new(TestStrategy::new(true));

        // Multiple price updates accumulate orders
        bot.on_price_update("BTC", 50000.0);
        bot.on_price_update("BTC", 51000.0);
        bot.on_price_update("BTC", 52000.0);

        assert_eq!(bot.pending_order_count(), 3);

        let orders = bot.take_pending_orders();
        assert_eq!(orders.len(), 3);
        assert_eq!(orders[0].order_id, 1);
        assert_eq!(orders[1].order_id, 2);
        assert_eq!(orders[2].order_id, 3);
    }

    #[test]
    fn test_bot_strategy_access() {
        let mut bot = Bot::new(TestStrategy::new(true));

        assert!(bot.strategy().should_buy);

        bot.strategy_mut().should_buy = false;
        assert!(!bot.strategy().should_buy);
    }
}

