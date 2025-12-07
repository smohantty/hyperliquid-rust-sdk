//! Strategy trait definition

use crate::market::{OrderFill, OrderRequest};

use super::StrategyAction;

/// Strategy interface for trading logic
///
/// A strategy receives market events (price updates, order fills) and returns
/// actions (orders to place). The strategy is completely decoupled from the
/// market/exchange - it doesn't know how or where orders are executed.
///
/// # Lifecycle
///
/// 1. Market calls `on_price_update` when prices change
/// 2. Strategy returns `StrategyAction` with orders to place
/// 3. Market executes those orders
/// 4. When orders fill, market calls `on_order_filled`
/// 5. Strategy updates internal state and may return more orders
///
/// # Testing
///
/// Strategies can be tested in isolation by directly calling the trait methods:
///
/// ```rust
/// use hyperliquid_rust_sdk::strategy::{Strategy, StrategyAction};
/// use hyperliquid_rust_sdk::market::{OrderRequest, OrderFill};
///
/// fn test_strategy(strategy: &mut impl Strategy) {
///     // Simulate price update
///     let action = strategy.on_price_update("BTC", 50000.0);
///     assert!(action.has_orders());
///
///     // Simulate fill
///     let fill = OrderFill::new(1, "BTC", 0.1, 50000.0);
///     let action = strategy.on_order_filled(&fill);
///     // Assert on action...
/// }
/// ```
///
/// # Example Implementation
///
/// ```rust
/// use hyperliquid_rust_sdk::strategy::{Strategy, StrategyAction};
/// use hyperliquid_rust_sdk::market::{OrderRequest, OrderFill};
///
/// struct MomentumStrategy {
///     last_price: Option<f64>,
///     position: f64,
///     next_order_id: u64,
/// }
///
/// impl Strategy for MomentumStrategy {
///     fn on_price_update(&mut self, asset: &str, price: f64) -> StrategyAction {
///         let action = if let Some(last) = self.last_price {
///             let change = (price - last) / last;
///             if change > 0.01 && self.position <= 0.0 {
///                 // Price up 1%+, go long
///                 self.next_order_id += 1;
///                 StrategyAction::single(OrderRequest::buy(
///                     self.next_order_id, asset, 1.0, price
///                 ))
///             } else if change < -0.01 && self.position >= 0.0 {
///                 // Price down 1%+, go short
///                 self.next_order_id += 1;
///                 StrategyAction::single(OrderRequest::sell(
///                     self.next_order_id, asset, 1.0, price
///                 ))
///             } else {
///                 StrategyAction::none()
///             }
///         } else {
///             StrategyAction::none()
///         };
///         self.last_price = Some(price);
///         action
///     }
///
///     fn on_order_filled(&mut self, fill: &OrderFill) -> StrategyAction {
///         // Update position tracking
///         // (simplified - real impl would track buy vs sell)
///         self.position += fill.qty;
///         StrategyAction::none()
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
    /// Actions to take in response to the price update
    fn on_price_update(&mut self, asset: &str, price: f64) -> StrategyAction;

    /// Called when an order is filled
    ///
    /// # Arguments
    /// * `fill` - Details about the filled order
    ///
    /// # Returns
    /// Actions to take in response to the fill
    fn on_order_filled(&mut self, fill: &OrderFill) -> StrategyAction;

    /// Called to initialize the strategy (optional)
    ///
    /// Override this to perform setup when the strategy starts.
    /// Default implementation does nothing.
    fn on_start(&mut self) -> StrategyAction {
        StrategyAction::none()
    }

    /// Called when the strategy is stopped (optional)
    ///
    /// Override this to perform cleanup. Can return orders to close positions.
    /// Default implementation does nothing.
    fn on_stop(&mut self) -> StrategyAction {
        StrategyAction::none()
    }

    /// Get the strategy name (optional)
    ///
    /// Useful for logging and debugging.
    fn name(&self) -> &str {
        "unnamed_strategy"
    }
}

/// A no-op strategy that never generates orders
///
/// Useful for testing markets without strategy logic.
#[derive(Debug, Default)]
pub struct NoOpStrategy;

impl Strategy for NoOpStrategy {
    fn on_price_update(&mut self, _asset: &str, _price: f64) -> StrategyAction {
        StrategyAction::none()
    }

    fn on_order_filled(&mut self, _fill: &OrderFill) -> StrategyAction {
        StrategyAction::none()
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

        let action = strategy.on_price_update("BTC", 50000.0);
        assert!(!action.has_orders());

        let fill = OrderFill::new(1, "BTC", 1.0, 50000.0);
        let action = strategy.on_order_filled(&fill);
        assert!(!action.has_orders());

        assert_eq!(strategy.name(), "noop");
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
        fn on_price_update(&mut self, asset: &str, price: f64) -> StrategyAction {
            if !self.has_position && price <= self.threshold {
                self.next_order_id += 1;
                StrategyAction::single(OrderRequest::buy(
                    self.next_order_id,
                    asset,
                    1.0,
                    price,
                ))
            } else {
                StrategyAction::none()
            }
        }

        fn on_order_filled(&mut self, _fill: &OrderFill) -> StrategyAction {
            self.has_position = true;
            StrategyAction::none()
        }

        fn name(&self) -> &str {
            "threshold"
        }
    }

    #[test]
    fn test_threshold_strategy() {
        let mut strategy = ThresholdStrategy::new(50000.0);

        // Price above threshold - no action
        let action = strategy.on_price_update("BTC", 51000.0);
        assert!(!action.has_orders());

        // Price at threshold - should buy
        let action = strategy.on_price_update("BTC", 50000.0);
        assert!(action.has_orders());
        assert_eq!(action.order_count(), 1);
        assert_eq!(action.orders[0].order_id, 1);
        assert_eq!(action.orders[0].limit_price, 50000.0);

        // Simulate fill
        let fill = OrderFill::new(1, "BTC", 1.0, 50000.0);
        let action = strategy.on_order_filled(&fill);
        assert!(!action.has_orders());

        // Price drops again - no action (already has position)
        let action = strategy.on_price_update("BTC", 49000.0);
        assert!(!action.has_orders());
    }

    #[test]
    fn test_strategy_lifecycle() {
        let mut strategy = ThresholdStrategy::new(50000.0);

        // on_start default
        let action = strategy.on_start();
        assert!(!action.has_orders());

        // on_stop default
        let action = strategy.on_stop();
        assert!(!action.has_orders());
    }
}

