//! Market implementation
//!
//! Base market implementation for order storage and listener notifications.
//! This is a simple container - concrete implementations handle fill logic.
//!
//! The listener is held via `Arc<RwLock<L>>` to allow shared access between
//! the market (for callbacks) and external code (e.g., HTTP servers for status).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::listener::MarketListener;
use super::types::{OrderFill, OrderRequest, OrderStatus};

/// Internal order tracking - simple status only
#[derive(Debug, Clone)]
struct InternalOrder {
    #[allow(dead_code)]
    request: OrderRequest,
    status: OrderStatus,
}

impl InternalOrder {
    fn new(request: OrderRequest) -> Self {
        Self {
            request,
            status: OrderStatus::Pending,
        }
    }
}

/// Market implementation with price management, order handling, and listener notifications
///
/// # Requirements Implemented
/// - M1: Price Management - update and retrieve prices
/// - M2: Order Acceptance - accept orders with user-provided IDs
/// - M3: Order Execution Notification - notify listener on fills
/// - M4: Price Update Notification - notify listener on price changes
/// - M5: Shared Listener - listener via Arc<RwLock<L>> for shared access
/// - M6: Synchronous Invocation - all notifications are synchronous
///
/// # Shared Listener Pattern
///
/// The listener is wrapped in `Arc<RwLock<L>>` so the same listener instance
/// can be accessed by both the market (for callbacks) and external code
/// (e.g., an HTTP server displaying bot status).
///
/// ```ignore
/// let bot = Arc::new(RwLock::new(Bot::new(strategy)));
/// let market = Market::new(bot.clone());  // Market uses same bot
/// let server = start_server(bot.clone()); // Server uses same bot
/// ```
pub struct Market<L: MarketListener> {
    /// Shared listener instance (M5)
    listener: Arc<RwLock<L>>,
    /// Current prices by asset
    prices: HashMap<String, f64>,
    /// Order storage (keyed by user-provided order_id)
    orders: HashMap<u64, InternalOrder>,
}

impl<L: MarketListener> Market<L> {
    /// Create a new Market with the given shared listener
    ///
    /// # Arguments
    /// * `listener` - Shared listener wrapped in Arc<RwLock<L>>
    pub fn new(listener: Arc<RwLock<L>>) -> Self {
        Self {
            listener,
            prices: HashMap::new(),
            orders: HashMap::new(),
        }
    }

    /// Update the price for an asset (M7)
    ///
    /// Updates internal price state, notifies the listener, and places any
    /// orders returned by the listener.
    ///
    /// # Arguments
    /// * `asset` - The asset identifier
    /// * `price` - The new price
    pub fn update_price(&mut self, asset: &str, price: f64) {
        self.prices.insert(asset.to_string(), price);
        // M6: Synchronous notification, listener returns orders to place
        // Use blocking_write since we're in sync context
        let orders = if let Ok(mut listener) = self.listener.try_write() {
            listener.on_price_update(asset, price)
        } else {
            vec![]
        };
        // Place returned orders
        for order in orders {
            self.place_order(order);
        }
    }

    /// Place a new order (M8)
    ///
    /// Accepts an order request with a user-provided order_id and stores it as pending.
    /// Fill logic is not handled here - use `execute_fill` to process fills.
    ///
    /// # Arguments
    /// * `order` - The order request (contains user-provided order_id)
    pub fn place_order(&mut self, order: OrderRequest) {
        let order_id = order.order_id;
        let internal_order = InternalOrder::new(order);
        self.orders.insert(order_id, internal_order);
    }

    /// Mark order as filled and notify listener (M9)
    ///
    /// Called by concrete implementations when an order is completely filled.
    /// The fill contains the final quantity and price (concrete implementations
    /// handle partial fill tracking internally).
    ///
    /// # Arguments
    /// * `fill` - The complete fill details (qty = total filled, price = final price)
    pub fn execute_fill(&mut self, fill: OrderFill) {
        // Update order state if it exists
        if let Some(internal_order) = self.orders.get_mut(&fill.order_id) {
            if internal_order.status.is_active() {
                // Mark as filled with the provided price
                internal_order.status = OrderStatus::Filled(fill.price);

                // M6: Synchronous notification, listener returns orders to place
                let orders = if let Ok(mut listener) = self.listener.try_write() {
                    listener.on_order_filled(fill)
                } else {
                    vec![]
                };
                // Place returned orders
                for order in orders {
                    self.place_order(order);
                }
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

    /// Get the shared listener reference
    ///
    /// Returns the `Arc<RwLock<L>>` so callers can access the listener
    /// for status queries, dashboards, etc.
    pub fn listener(&self) -> Arc<RwLock<L>> {
        self.listener.clone()
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
    use crate::market::OrderRequest;

    /// Helper to create a shared listener
    fn shared<L>(listener: L) -> Arc<RwLock<L>> {
        Arc::new(RwLock::new(listener))
    }

    #[derive(Default)]
    struct RecordingListener {
        fills: Vec<OrderFill>,
        price_updates: Vec<(String, f64)>,
    }

    impl MarketListener for RecordingListener {
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
    fn test_market_new() {
        let market = Market::new(shared(NoOpListener));
        assert!(market.current_price("BTC").is_none());
    }

    #[test]
    fn test_update_price_m7() {
        let listener = shared(RecordingListener::default());
        let mut market = Market::new(listener.clone());

        market.update_price("BTC", 50000.0);

        assert_eq!(market.current_price("BTC"), Some(50000.0));
        // Access listener through the shared reference
        let l = listener.try_read().unwrap();
        assert_eq!(l.price_updates.len(), 1);
        assert_eq!(l.price_updates[0], ("BTC".to_string(), 50000.0));
    }

    #[test]
    fn test_place_order_pending_m8() {
        let listener = shared(RecordingListener::default());
        let mut market = Market::new(listener.clone());

        // User provides order_id, orders are always placed as pending
        let order = OrderRequest::buy(100, "BTC", 1.0, 50000.0);
        market.place_order(order);

        assert_eq!(market.order_status(100), Some(OrderStatus::Pending));
        assert!(listener.try_read().unwrap().fills.is_empty());
    }

    #[test]
    fn test_place_order_always_pending_m8() {
        let listener = shared(RecordingListener::default());
        let mut market = Market::new(listener.clone());

        // Even with price set, place_order just stores as pending
        // Fill logic is handled by concrete implementations
        market.update_price("BTC", 49000.0);

        let order = OrderRequest::buy(200, "BTC", 1.0, 50000.0);
        market.place_order(order);

        // Order should be pending - no automatic fill
        assert_eq!(market.order_status(200), Some(OrderStatus::Pending));
        assert!(listener.try_read().unwrap().fills.is_empty());
    }

    #[test]
    fn test_execute_fill_m9() {
        let listener = shared(RecordingListener::default());
        let mut market = Market::new(listener.clone());

        let order = OrderRequest::buy(300, "BTC", 1.0, 50000.0);
        market.place_order(order);

        // Execute fill - marks as filled and notifies with same order_id
        let fill = OrderFill::new(300, "BTC", 1.0, 49500.0);
        market.execute_fill(fill);

        // Order should be filled at the provided price
        assert_eq!(
            market.order_status(300),
            Some(OrderStatus::Filled(49500.0))
        );

        // Listener was notified with user's order_id
        let l = listener.try_read().unwrap();
        assert_eq!(l.fills.len(), 1);
        assert_eq!(l.fills[0].order_id, 300);
        assert_eq!(l.fills[0].qty, 1.0);
        assert_eq!(l.fills[0].price, 49500.0);
    }

    #[test]
    fn test_execute_fill_already_filled_m9() {
        let listener = shared(RecordingListener::default());
        let mut market = Market::new(listener.clone());

        let order = OrderRequest::buy(400, "BTC", 1.0, 50000.0);
        market.place_order(order);

        // First fill
        let fill1 = OrderFill::new(400, "BTC", 1.0, 49500.0);
        market.execute_fill(fill1);
        assert_eq!(listener.try_read().unwrap().fills.len(), 1);

        // Second fill on same order - should be ignored (already filled)
        let fill2 = OrderFill::new(400, "BTC", 1.0, 49000.0);
        market.execute_fill(fill2);

        // Still only one notification, status unchanged
        assert_eq!(listener.try_read().unwrap().fills.len(), 1);
        assert_eq!(
            market.order_status(400),
            Some(OrderStatus::Filled(49500.0))
        );
    }

    #[test]
    fn test_current_price_m10() {
        let mut market = Market::new(shared(NoOpListener));

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
        let mut market = Market::new(shared(NoOpListener));

        assert!(market.order_status(999).is_none());

        let order = OrderRequest::buy(500, "BTC", 1.0, 50000.0);
        market.place_order(order);

        assert!(market.order_status(500).is_some());
    }

    #[test]
    fn test_cancel_order() {
        let mut market = Market::new(shared(NoOpListener));

        let order = OrderRequest::buy(600, "BTC", 1.0, 50000.0);
        market.place_order(order);

        assert!(market.cancel_order(600));
        assert_eq!(market.order_status(600), Some(OrderStatus::Cancelled));

        // Cannot cancel already cancelled order
        assert!(!market.cancel_order(600));
    }

    #[test]
    fn test_user_provided_order_ids() {
        let mut market = Market::new(shared(NoOpListener));

        // User provides their own order IDs
        market.place_order(OrderRequest::buy(1001, "BTC", 1.0, 50000.0));
        market.place_order(OrderRequest::sell(1002, "BTC", 1.0, 50000.0));
        market.place_order(OrderRequest::buy(2001, "ETH", 1.0, 3000.0));

        assert!(market.order_status(1001).is_some());
        assert!(market.order_status(1002).is_some());
        assert!(market.order_status(2001).is_some());
        assert!(market.order_status(9999).is_none());
    }

    #[test]
    fn test_synchronous_notification_m6() {
        // This test verifies that notifications happen synchronously
        // by checking that listener state is updated before the function returns
        let listener = shared(RecordingListener::default());
        let mut market = Market::new(listener.clone());

        market.update_price("BTC", 50000.0);
        // Immediately after update_price, listener should have the update
        assert_eq!(listener.try_read().unwrap().price_updates.len(), 1);

        // Place order and fill it via execute_fill
        let order = OrderRequest::buy(700, "BTC", 1.0, 50000.0);
        market.place_order(order);

        // Execute a complete fill
        let fill = OrderFill::new(700, "BTC", 1.0, 49000.0);
        market.execute_fill(fill);

        // Immediately after execute_fill completes the order, listener should have the fill
        let l = listener.try_read().unwrap();
        assert_eq!(l.fills.len(), 1);
        assert_eq!(l.fills[0].order_id, 700);
    }

    #[test]
    fn test_shared_listener_access() {
        // Test that the same listener can be accessed from market and externally
        let listener = shared(RecordingListener::default());
        let mut market = Market::new(listener.clone());

        // Update through market
        market.update_price("BTC", 50000.0);

        // Access through external reference (simulating HTTP server)
        let external_ref = market.listener();
        let l = external_ref.try_read().unwrap();
        assert_eq!(l.price_updates.len(), 1);

        // Both references point to same data
        drop(l);
        market.update_price("ETH", 3000.0);
        assert_eq!(listener.try_read().unwrap().price_updates.len(), 2);
    }
}

