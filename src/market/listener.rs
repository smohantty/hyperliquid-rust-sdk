//! MarketListener trait definition
//!
//! Defines how external components receive notifications from the Market.

use super::types::{OrderFill, OrderRequest};

/// MarketListener interface for receiving market notifications
///
/// Components that need to receive notifications about order fills and price
/// updates should implement this trait. The Market owns a single listener
/// instance and invokes these callbacks synchronously.
///
/// Callbacks return orders that the market should place. This allows the
/// listener (e.g., a trading bot) to react to events and place orders
/// without needing a reference back to the market.
pub trait MarketListener {
    /// Called when an order is filled
    ///
    /// This notification is invoked synchronously when:
    /// - An order placed via `place_order` is immediately fillable
    /// - An external fill is injected via `execute_fill`
    ///
    /// # Arguments
    /// * `fill` - Details about the order fill
    ///
    /// # Returns
    /// Orders to place in response to this fill
    fn on_order_filled(&mut self, fill: OrderFill) -> Vec<OrderRequest>;

    /// Called when an asset's price is updated
    ///
    /// This notification is invoked synchronously when:
    /// - `update_price` is called on the Market
    ///
    /// # Arguments
    /// * `asset` - The asset identifier whose price changed
    /// * `price` - The new price
    ///
    /// # Returns
    /// Orders to place in response to this price update
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest>;
}

/// A no-op listener for testing or when notifications aren't needed
#[derive(Debug, Default)]
pub struct NoOpListener;

impl MarketListener for NoOpListener {
    fn on_order_filled(&mut self, _fill: OrderFill) -> Vec<OrderRequest> {
        vec![]
    }

    fn on_price_update(&mut self, _asset: &str, _price: f64) -> Vec<OrderRequest> {
        vec![]
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
        fn on_order_filled(&mut self, fill: OrderFill) -> Vec<OrderRequest> {
            self.fills.push(fill);
            vec![]
        }

        fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
            self.price_updates.push((asset.to_string(), price));
            vec![]
        }
    }

    #[test]
    fn test_noop_listener() {
        let mut listener = NoOpListener;
        let orders = listener.on_order_filled(OrderFill::new(1, "BTC", 1.0, 50000.0));
        assert!(orders.is_empty());

        let orders = listener.on_price_update("BTC", 50000.0);
        assert!(orders.is_empty());
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

    // Test listener that returns orders
    struct OrderingListener {
        next_order_id: u64,
    }

    impl MarketListener for OrderingListener {
        fn on_order_filled(&mut self, fill: OrderFill) -> Vec<OrderRequest> {
            // After a fill, place a new order
            self.next_order_id += 1;
            vec![OrderRequest::sell(self.next_order_id, &fill.asset, fill.qty, fill.price * 1.01)]
        }

        fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
            self.next_order_id += 1;
            vec![OrderRequest::buy(self.next_order_id, asset, 0.1, price)]
        }
    }

    #[test]
    fn test_listener_returns_orders() {
        let mut listener = OrderingListener { next_order_id: 0 };

        let orders = listener.on_price_update("BTC", 50000.0);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].order_id, 1);
        assert_eq!(orders[0].limit_price, 50000.0);

        let orders = listener.on_order_filled(OrderFill::new(1, "BTC", 0.1, 50000.0));
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].order_id, 2);
        assert!((orders[0].limit_price - 50500.0).abs() < 0.01);
    }
}

