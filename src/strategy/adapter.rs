//! Strategy adapter for connecting strategies to markets

use std::cell::RefCell;
use std::rc::Rc;

use crate::market::{MarketListener, OrderFill, OrderRequest};

use super::{Strategy, StrategyAction};

/// Adapter that bridges a Strategy to the MarketListener interface
///
/// This adapter implements `MarketListener` and forwards events to a `Strategy`.
/// The strategy's returned actions (orders) are collected and can be retrieved
/// for execution on a market.
///
/// # Usage Pattern
///
/// ```ignore
/// // Create strategy and wrap in adapter
/// let strategy = MyStrategy::new();
/// let adapter = StrategyAdapter::new(strategy);
///
/// // Use adapter as market listener
/// let market = HyperliquidMarket::new(input, adapter).await?;
///
/// // In your event loop, after market processes events:
/// for order in adapter.take_pending_orders() {
///     market.place_order(order).await;
/// }
/// ```
///
/// # Thread Safety
///
/// This adapter uses `RefCell` for interior mutability, making it suitable for
/// single-threaded async contexts. For multi-threaded usage, consider wrapping
/// the strategy in appropriate synchronization primitives.
pub struct StrategyAdapter<S: Strategy> {
    strategy: RefCell<S>,
    pending_orders: RefCell<Vec<OrderRequest>>,
}

impl<S: Strategy> StrategyAdapter<S> {
    /// Create a new adapter wrapping the given strategy
    pub fn new(strategy: S) -> Self {
        Self {
            strategy: RefCell::new(strategy),
            pending_orders: RefCell::new(Vec::new()),
        }
    }

    /// Take all pending orders, leaving the queue empty
    ///
    /// Call this after market events to get orders that need to be executed.
    pub fn take_pending_orders(&self) -> Vec<OrderRequest> {
        self.pending_orders.borrow_mut().drain(..).collect()
    }

    /// Check if there are pending orders
    pub fn has_pending_orders(&self) -> bool {
        !self.pending_orders.borrow().is_empty()
    }

    /// Get the count of pending orders
    pub fn pending_order_count(&self) -> usize {
        self.pending_orders.borrow().len()
    }

    /// Get a reference to the underlying strategy
    pub fn strategy(&self) -> std::cell::Ref<'_, S> {
        self.strategy.borrow()
    }

    /// Get a mutable reference to the underlying strategy
    pub fn strategy_mut(&self) -> std::cell::RefMut<'_, S> {
        self.strategy.borrow_mut()
    }

    /// Initialize the strategy and collect any initial orders
    pub fn start(&self) -> Vec<OrderRequest> {
        let action = self.strategy.borrow_mut().on_start();
        self.process_action(action);
        self.take_pending_orders()
    }

    /// Stop the strategy and collect any cleanup orders
    pub fn stop(&self) -> Vec<OrderRequest> {
        let action = self.strategy.borrow_mut().on_stop();
        self.process_action(action);
        self.take_pending_orders()
    }

    /// Process a strategy action by adding orders to the pending queue
    fn process_action(&self, action: StrategyAction) {
        if action.has_orders() {
            self.pending_orders.borrow_mut().extend(action.orders);
        }
    }
}

impl<S: Strategy> MarketListener for StrategyAdapter<S> {
    fn on_order_filled(&mut self, fill: OrderFill) {
        let action = self.strategy.borrow_mut().on_order_filled(&fill);
        self.process_action(action);
    }

    fn on_price_update(&mut self, asset: &str, price: f64) {
        let action = self.strategy.borrow_mut().on_price_update(asset, price);
        self.process_action(action);
    }
}

/// Shared adapter for use with async markets
///
/// Wraps a `StrategyAdapter` in `Rc<RefCell<>>` for shared ownership
/// in async contexts where the adapter needs to be accessed from multiple places.
pub type SharedStrategyAdapter<S> = Rc<RefCell<StrategyAdapter<S>>>;

/// Create a shared strategy adapter
pub fn shared_adapter<S: Strategy>(strategy: S) -> SharedStrategyAdapter<S> {
    Rc::new(RefCell::new(StrategyAdapter::new(strategy)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::OrderSide;

    // Test strategy that buys on first price update
    struct OneShotStrategy {
        triggered: bool,
    }

    impl OneShotStrategy {
        fn new() -> Self {
            Self { triggered: false }
        }
    }

    impl Strategy for OneShotStrategy {
        fn on_price_update(&mut self, asset: &str, price: f64) -> StrategyAction {
            if !self.triggered {
                self.triggered = true;
                StrategyAction::single(OrderRequest::buy(1, asset, 1.0, price))
            } else {
                StrategyAction::none()
            }
        }

        fn on_order_filled(&mut self, _fill: &OrderFill) -> StrategyAction {
            StrategyAction::none()
        }
    }

    #[test]
    fn test_adapter_collects_orders() {
        let strategy = OneShotStrategy::new();
        let mut adapter = StrategyAdapter::new(strategy);

        assert!(!adapter.has_pending_orders());

        // Trigger strategy
        adapter.on_price_update("BTC", 50000.0);

        assert!(adapter.has_pending_orders());
        assert_eq!(adapter.pending_order_count(), 1);

        // Take orders
        let orders = adapter.take_pending_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "BTC");
        assert_eq!(orders[0].limit_price, 50000.0);

        // Queue is now empty
        assert!(!adapter.has_pending_orders());
    }

    #[test]
    fn test_adapter_no_double_orders() {
        let strategy = OneShotStrategy::new();
        let mut adapter = StrategyAdapter::new(strategy);

        // First update triggers order
        adapter.on_price_update("BTC", 50000.0);
        assert_eq!(adapter.pending_order_count(), 1);

        // Second update doesn't (strategy is triggered)
        adapter.on_price_update("BTC", 51000.0);
        assert_eq!(adapter.pending_order_count(), 1);
    }

    // Strategy that generates order on fill
    struct ChainStrategy {
        chain_count: u32,
        max_chain: u32,
        next_order_id: u64,
    }

    impl ChainStrategy {
        fn new(max_chain: u32) -> Self {
            Self {
                chain_count: 0,
                max_chain,
                next_order_id: 0,
            }
        }
    }

    impl Strategy for ChainStrategy {
        fn on_price_update(&mut self, asset: &str, price: f64) -> StrategyAction {
            if self.chain_count == 0 {
                self.next_order_id += 1;
                self.chain_count += 1;
                StrategyAction::single(OrderRequest::buy(self.next_order_id, asset, 1.0, price))
            } else {
                StrategyAction::none()
            }
        }

        fn on_order_filled(&mut self, fill: &OrderFill) -> StrategyAction {
            if self.chain_count < self.max_chain {
                self.next_order_id += 1;
                self.chain_count += 1;
                StrategyAction::single(OrderRequest::sell(
                    self.next_order_id,
                    &fill.asset,
                    fill.qty,
                    fill.price * 1.01, // 1% profit target
                ))
            } else {
                StrategyAction::none()
            }
        }
    }

    #[test]
    fn test_adapter_chain_orders() {
        let strategy = ChainStrategy::new(3);
        let mut adapter = StrategyAdapter::new(strategy);

        // Initial price update generates first order
        adapter.on_price_update("BTC", 50000.0);
        let orders = adapter.take_pending_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].side, OrderSide::Buy);

        // Fill generates second order
        let fill = OrderFill::new(1, "BTC", 1.0, 50000.0);
        adapter.on_order_filled(fill);
        let orders = adapter.take_pending_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].side, OrderSide::Sell);

        // Fill generates third order
        let fill = OrderFill::new(2, "BTC", 1.0, 50500.0);
        adapter.on_order_filled(fill);
        let orders = adapter.take_pending_orders();
        assert_eq!(orders.len(), 1);

        // Max chain reached - no more orders
        let fill = OrderFill::new(3, "BTC", 1.0, 51005.0);
        adapter.on_order_filled(fill);
        let orders = adapter.take_pending_orders();
        assert_eq!(orders.len(), 0);
    }

    #[test]
    fn test_adapter_start_stop() {
        struct LifecycleStrategy;

        impl Strategy for LifecycleStrategy {
            fn on_price_update(&mut self, _: &str, _: f64) -> StrategyAction {
                StrategyAction::none()
            }

            fn on_order_filled(&mut self, _: &OrderFill) -> StrategyAction {
                StrategyAction::none()
            }

            fn on_start(&mut self) -> StrategyAction {
                StrategyAction::single(OrderRequest::buy(1, "BTC", 1.0, 50000.0))
            }

            fn on_stop(&mut self) -> StrategyAction {
                StrategyAction::single(OrderRequest::sell(2, "BTC", 1.0, 51000.0))
            }
        }

        let adapter = StrategyAdapter::new(LifecycleStrategy);

        let orders = adapter.start();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].side, OrderSide::Buy);

        let orders = adapter.stop();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].side, OrderSide::Sell);
    }

    #[test]
    fn test_adapter_strategy_access() {
        let strategy = OneShotStrategy::new();
        let adapter = StrategyAdapter::new(strategy);

        assert!(!adapter.strategy().triggered);

        // Trigger via mutable access
        adapter.strategy_mut().triggered = true;
        assert!(adapter.strategy().triggered);
    }
}

