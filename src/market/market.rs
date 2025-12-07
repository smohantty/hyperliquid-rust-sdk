//! Market implementation
//!
//! Core market implementation that manages prices, orders, and notifies listeners.

use std::collections::HashMap;

use super::listener::MarketListener;
use super::types::{OrderFill, OrderRequest, OrderStatus};

/// Internal order tracking
#[derive(Debug, Clone)]
struct InternalOrder {
    request: OrderRequest,
    status: OrderStatus,
    filled_qty: f64,
    avg_fill_price: f64,
}

impl InternalOrder {
    fn new(request: OrderRequest) -> Self {
        Self {
            request,
            status: OrderStatus::Pending,
            filled_qty: 0.0,
            avg_fill_price: 0.0,
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
}

/// Market implementation with price management, order handling, and listener notifications
///
/// # Requirements Implemented
/// - M1: Price Management - update and retrieve prices
/// - M2: Order Acceptance - accept orders and return unique IDs
/// - M3: Order Execution Notification - notify listener on fills
/// - M4: Price Update Notification - notify listener on price changes
/// - M5: Listener Ownership - owns a single listener instance
/// - M6: Synchronous Invocation - all notifications are synchronous
pub struct Market<L: MarketListener> {
    /// Owned listener instance (M5)
    listener: L,
    /// Current prices by asset
    prices: HashMap<String, f64>,
    /// Order storage
    orders: HashMap<u64, InternalOrder>,
    /// Next order ID to assign
    next_order_id: u64,
}

impl<L: MarketListener> Market<L> {
    /// Create a new Market with the given listener
    ///
    /// # Arguments
    /// * `listener` - The listener that will receive notifications
    pub fn new(listener: L) -> Self {
        Self {
            listener,
            prices: HashMap::new(),
            orders: HashMap::new(),
            next_order_id: 1,
        }
    }

    /// Update the price for an asset (M7)
    ///
    /// Updates internal price state and synchronously notifies the listener.
    ///
    /// # Arguments
    /// * `asset` - The asset identifier
    /// * `price` - The new price
    pub fn update_price(&mut self, asset: &str, price: f64) {
        self.prices.insert(asset.to_string(), price);
        // M6: Synchronous notification
        self.listener.on_price_update(asset, price);
    }

    /// Place a new order (M8)
    ///
    /// Accepts an order request, assigns a unique order ID, and stores it as pending.
    /// Fill logic is not handled here - use `execute_fill` to process fills.
    ///
    /// # Arguments
    /// * `order` - The order request
    ///
    /// # Returns
    /// A unique order ID
    pub fn place_order(&mut self, order: OrderRequest) -> u64 {
        let order_id = self.next_order_id;
        self.next_order_id += 1;

        let internal_order = InternalOrder::new(order);
        self.orders.insert(order_id, internal_order);
        order_id
    }

    /// Inject an external fill (M9)
    ///
    /// Accepts an externally described fill and updates order state.
    /// Only notifies the listener when the order is fully filled.
    ///
    /// # Arguments
    /// * `fill` - The fill details
    pub fn execute_fill(&mut self, fill: OrderFill) {
        // Update order state if it exists
        if let Some(internal_order) = self.orders.get_mut(&fill.order_id) {
            let was_active = internal_order.status.is_active();
            internal_order.fill(fill.qty, fill.price);

            // Only notify when order is fully filled
            if was_active && matches!(internal_order.status, OrderStatus::Filled(_)) {
                let complete_fill = OrderFill::new(
                    fill.order_id,
                    &internal_order.request.asset,
                    internal_order.request.qty,     // Total order qty
                    internal_order.avg_fill_price,  // Average fill price
                );

                // M6: Synchronous notification
                self.listener.on_order_filled(complete_fill);
            }
        }
    }

    /// Query current price for an asset (M10)
    ///
    /// # Arguments
    /// * `asset` - The asset identifier
    ///
    /// # Returns
    /// The last known price if available
    pub fn current_price(&self, asset: &str) -> Option<f64> {
        self.prices.get(asset).copied()
    }

    /// Query order status (M11)
    ///
    /// # Arguments
    /// * `order_id` - The order identifier
    ///
    /// # Returns
    /// The current order status if the order exists
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
    ///
    /// # Arguments
    /// * `order_id` - The order to cancel
    ///
    /// # Returns
    /// `true` if the order was cancelled, `false` if not found or already complete
    pub fn cancel_order(&mut self, order_id: u64) -> bool {
        if let Some(internal_order) = self.orders.get_mut(&order_id) {
            if internal_order.status.is_active() {
                internal_order.status = OrderStatus::Cancelled;
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::listener::NoOpListener;

    #[derive(Default)]
    struct RecordingListener {
        fills: Vec<OrderFill>,
        price_updates: Vec<(String, f64)>,
    }

    impl MarketListener for RecordingListener {
        fn on_order_filled(&mut self, fill: OrderFill) {
            self.fills.push(fill);
        }

        fn on_price_update(&mut self, asset: &str, price: f64) {
            self.price_updates.push((asset.to_string(), price));
        }
    }

    #[test]
    fn test_market_new() {
        let market = Market::new(NoOpListener);
        assert!(market.current_price("BTC").is_none());
    }

    #[test]
    fn test_update_price_m7() {
        let mut market = Market::new(RecordingListener::default());

        market.update_price("BTC", 50000.0);

        assert_eq!(market.current_price("BTC"), Some(50000.0));
        assert_eq!(market.listener().price_updates.len(), 1);
        assert_eq!(
            market.listener().price_updates[0],
            ("BTC".to_string(), 50000.0)
        );
    }

    #[test]
    fn test_place_order_pending_m8() {
        let mut market = Market::new(RecordingListener::default());

        // Orders are always placed as pending - fill logic is external
        let order = OrderRequest::new("BTC", 1.0, 50000.0);
        let order_id = market.place_order(order);

        assert_eq!(order_id, 1);
        assert_eq!(market.order_status(order_id), Some(OrderStatus::Pending));
        assert!(market.listener().fills.is_empty());
    }

    #[test]
    fn test_place_order_always_pending_m8() {
        let mut market = Market::new(RecordingListener::default());

        // Even with price set, place_order just stores as pending
        // Fill logic is handled by concrete implementations
        market.update_price("BTC", 49000.0);

        let order = OrderRequest::new("BTC", 1.0, 50000.0);
        let order_id = market.place_order(order);

        // Order should be pending - no automatic fill
        assert_eq!(market.order_status(order_id), Some(OrderStatus::Pending));
        assert!(market.listener().fills.is_empty());
    }

    #[test]
    fn test_execute_fill_partial_no_notify_m9() {
        let mut market = Market::new(RecordingListener::default());

        let order = OrderRequest::new("BTC", 2.0, 50000.0);
        let order_id = market.place_order(order);

        // Inject partial fill - should NOT notify listener
        let fill = OrderFill::new(order_id, "BTC", 1.0, 49500.0);
        market.execute_fill(fill);

        assert_eq!(
            market.order_status(order_id),
            Some(OrderStatus::PartiallyFilled(1.0))
        );
        // No notification on partial fill
        assert!(market.listener().fills.is_empty());
    }

    #[test]
    fn test_execute_fill_complete_notify_m9() {
        let mut market = Market::new(RecordingListener::default());

        let order = OrderRequest::new("BTC", 2.0, 50000.0);
        let order_id = market.place_order(order);

        // Inject partial fill - no notification
        let fill = OrderFill::new(order_id, "BTC", 1.0, 49500.0);
        market.execute_fill(fill);
        assert!(market.listener().fills.is_empty());

        // Inject remaining fill - NOW notify with complete order details
        let fill2 = OrderFill::new(order_id, "BTC", 1.0, 49600.0);
        market.execute_fill(fill2);

        // Should now be fully filled with average price
        match market.order_status(order_id) {
            Some(OrderStatus::Filled(avg_price)) => {
                assert!((avg_price - 49550.0).abs() < 0.01);
            }
            _ => panic!("Expected Filled status"),
        }

        // Listener notified once with complete fill
        assert_eq!(market.listener().fills.len(), 1);
        assert_eq!(market.listener().fills[0].order_id, order_id);
        assert_eq!(market.listener().fills[0].qty, 2.0); // Total qty
        assert!((market.listener().fills[0].price - 49550.0).abs() < 0.01); // Avg price
    }

    #[test]
    fn test_current_price_m10() {
        let mut market = Market::new(NoOpListener);

        assert!(market.current_price("BTC").is_none());

        market.update_price("BTC", 50000.0);
        assert_eq!(market.current_price("BTC"), Some(50000.0));

        market.update_price("BTC", 51000.0);
        assert_eq!(market.current_price("BTC"), Some(51000.0));

        market.update_price("ETH", 3000.0);
        assert_eq!(market.current_price("ETH"), Some(3000.0));
        assert_eq!(market.current_price("BTC"), Some(51000.0));
    }

    #[test]
    fn test_order_status_m11() {
        let mut market = Market::new(NoOpListener);

        assert!(market.order_status(999).is_none());

        let order = OrderRequest::new("BTC", 1.0, 50000.0);
        let order_id = market.place_order(order);

        assert!(market.order_status(order_id).is_some());
    }

    #[test]
    fn test_cancel_order() {
        let mut market = Market::new(NoOpListener);

        let order = OrderRequest::new("BTC", 1.0, 50000.0);
        let order_id = market.place_order(order);

        assert!(market.cancel_order(order_id));
        assert_eq!(market.order_status(order_id), Some(OrderStatus::Cancelled));

        // Cannot cancel already cancelled order
        assert!(!market.cancel_order(order_id));
    }

    #[test]
    fn test_unique_order_ids() {
        let mut market = Market::new(NoOpListener);

        let id1 = market.place_order(OrderRequest::new("BTC", 1.0, 50000.0));
        let id2 = market.place_order(OrderRequest::new("BTC", 1.0, 50000.0));
        let id3 = market.place_order(OrderRequest::new("ETH", 1.0, 3000.0));

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_synchronous_notification_m6() {
        // This test verifies that notifications happen synchronously
        // by checking that listener state is updated before the function returns
        let mut market = Market::new(RecordingListener::default());

        market.update_price("BTC", 50000.0);
        // Immediately after update_price, listener should have the update
        assert_eq!(market.listener().price_updates.len(), 1);

        // Place order and fill it via execute_fill
        let order = OrderRequest::new("BTC", 1.0, 50000.0);
        let order_id = market.place_order(order);

        // Execute a complete fill
        let fill = OrderFill::new(order_id, "BTC", 1.0, 49000.0);
        market.execute_fill(fill);

        // Immediately after execute_fill completes the order, listener should have the fill
        assert_eq!(market.listener().fills.len(), 1);
    }
}

