//! MarketListener trait definition
//!
//! Defines how external components receive notifications from the Market.

use super::types::OrderFill;

/// MarketListener interface for receiving market notifications
///
/// Components that need to receive notifications about order fills and price
/// updates should implement this trait. The Market owns a single listener
/// instance and invokes these callbacks synchronously.
pub trait MarketListener {
    /// Called when an order is filled
    ///
    /// This notification is invoked synchronously when:
    /// - An order placed via `place_order` is immediately fillable
    /// - An external fill is injected via `execute_fill`
    ///
    /// # Arguments
    /// * `fill` - Details about the order fill
    fn on_order_filled(&mut self, fill: OrderFill);

    /// Called when an asset's price is updated
    ///
    /// This notification is invoked synchronously when:
    /// - `update_price` is called on the Market
    ///
    /// # Arguments
    /// * `asset` - The asset identifier whose price changed
    /// * `price` - The new price
    fn on_price_update(&mut self, asset: &str, price: f64);
}

/// A no-op listener for testing or when notifications aren't needed
#[derive(Debug, Default)]
pub struct NoOpListener;

impl MarketListener for NoOpListener {
    fn on_order_filled(&mut self, _fill: OrderFill) {
        // No-op
    }

    fn on_price_update(&mut self, _asset: &str, _price: f64) {
        // No-op
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestListener {
        fills: Vec<OrderFill>,
        price_updates: Vec<(String, f64)>,
    }

    impl MarketListener for TestListener {
        fn on_order_filled(&mut self, fill: OrderFill) {
            self.fills.push(fill);
        }

        fn on_price_update(&mut self, asset: &str, price: f64) {
            self.price_updates.push((asset.to_string(), price));
        }
    }

    #[test]
    fn test_noop_listener() {
        let mut listener = NoOpListener;
        listener.on_order_filled(OrderFill::new(1, "BTC", 1.0, 50000.0));
        listener.on_price_update("BTC", 50000.0);
        // Should not panic
    }

    #[test]
    fn test_custom_listener() {
        let mut listener = TestListener::default();

        listener.on_order_filled(OrderFill::new(1, "BTC", 1.0, 50000.0));
        listener.on_price_update("ETH", 3000.0);

        assert_eq!(listener.fills.len(), 1);
        assert_eq!(listener.fills[0].order_id, 1);
        assert_eq!(listener.price_updates.len(), 1);
        assert_eq!(listener.price_updates[0], ("ETH".to_string(), 3000.0));
    }
}

