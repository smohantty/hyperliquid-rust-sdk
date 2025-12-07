//! Core data types for the Market interface

use serde::{Deserialize, Serialize};

/// Order request input to the Market
///
/// Represents a new order to be placed in the market.
/// The user provides their own `order_id` which will be returned
/// in the fill callback when the order is executed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    /// User-provided order identifier (returned in fill callback)
    pub order_id: u64,
    /// Asset identifier (e.g., "BTC", "ETH")
    pub asset: String,
    /// Order quantity (must be > 0)
    pub qty: f64,
    /// Limit price (must be > 0)
    pub limit_price: f64,
}

impl OrderRequest {
    /// Create a new order request
    ///
    /// # Arguments
    /// * `order_id` - User-provided identifier (returned in fill callback)
    /// * `asset` - Asset to trade
    /// * `qty` - Order quantity (must be > 0)
    /// * `limit_price` - Limit price (must be > 0)
    ///
    /// # Panics
    /// Panics if qty <= 0 or limit_price <= 0
    pub fn new(order_id: u64, asset: impl Into<String>, qty: f64, limit_price: f64) -> Self {
        assert!(qty > 0.0, "qty must be greater than 0");
        assert!(limit_price > 0.0, "limit_price must be greater than 0");
        Self {
            order_id,
            asset: asset.into(),
            qty,
            limit_price,
        }
    }

    /// Validate the order request
    pub fn is_valid(&self) -> bool {
        self.qty > 0.0 && self.limit_price > 0.0
    }
}

/// Order fill notification from Market to Listener
///
/// Contains details about an executed order fill.
/// The `order_id` matches the user-provided ID from the original `OrderRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderFill {
    /// User-provided order identifier (same as in OrderRequest)
    pub order_id: u64,
    /// Asset identifier
    pub asset: String,
    /// Filled quantity
    pub qty: f64,
    /// Execution price
    pub price: f64,
}

impl OrderFill {
    /// Create a new order fill
    pub fn new(order_id: u64, asset: impl Into<String>, qty: f64, price: f64) -> Self {
        Self {
            order_id,
            asset: asset.into(),
            qty,
            price,
        }
    }

    /// Calculate the total value of this fill
    pub fn value(&self) -> f64 {
        self.qty * self.price
    }
}

/// Order status variants
///
/// Represents the current state of an order in the market.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Order is pending execution
    Pending,
    /// Order is partially filled with the given quantity
    PartiallyFilled(f64),
    /// Order is fully filled at the given average price
    Filled(f64),
    /// Order has been cancelled
    Cancelled,
}

impl OrderStatus {
    /// Check if the order is still active (pending or partially filled)
    pub fn is_active(&self) -> bool {
        matches!(self, OrderStatus::Pending | OrderStatus::PartiallyFilled(_))
    }

    /// Check if the order is complete (filled or cancelled)
    pub fn is_complete(&self) -> bool {
        matches!(self, OrderStatus::Filled(_) | OrderStatus::Cancelled)
    }

    /// Get the filled quantity if partially or fully filled
    pub fn filled_qty(&self) -> Option<f64> {
        match self {
            OrderStatus::PartiallyFilled(qty) => Some(*qty),
            OrderStatus::Filled(price) => Some(*price), // Note: Filled stores price, not qty
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_request_new() {
        let order = OrderRequest::new(100, "BTC", 1.5, 50000.0);
        assert_eq!(order.order_id, 100);
        assert_eq!(order.asset, "BTC");
        assert_eq!(order.qty, 1.5);
        assert_eq!(order.limit_price, 50000.0);
        assert!(order.is_valid());
    }

    #[test]
    #[should_panic(expected = "qty must be greater than 0")]
    fn test_order_request_invalid_qty() {
        OrderRequest::new(1, "BTC", 0.0, 50000.0);
    }

    #[test]
    #[should_panic(expected = "limit_price must be greater than 0")]
    fn test_order_request_invalid_price() {
        OrderRequest::new(1, "BTC", 1.0, 0.0);
    }

    #[test]
    fn test_order_fill() {
        let fill = OrderFill::new(1, "BTC", 0.5, 50000.0);
        assert_eq!(fill.order_id, 1);
        assert_eq!(fill.asset, "BTC");
        assert_eq!(fill.qty, 0.5);
        assert_eq!(fill.price, 50000.0);
        assert_eq!(fill.value(), 25000.0);
    }

    #[test]
    fn test_order_status() {
        assert!(OrderStatus::Pending.is_active());
        assert!(OrderStatus::PartiallyFilled(0.5).is_active());
        assert!(!OrderStatus::Filled(50000.0).is_active());
        assert!(!OrderStatus::Cancelled.is_active());

        assert!(!OrderStatus::Pending.is_complete());
        assert!(OrderStatus::Filled(50000.0).is_complete());
        assert!(OrderStatus::Cancelled.is_complete());
    }
}

