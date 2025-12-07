//! Strategy trait definition

use crate::market::{OrderFill, OrderRequest};

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
}

/// A no-op strategy that never generates orders
///
/// Useful for testing markets without strategy logic.
#[derive(Debug, Default)]
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
