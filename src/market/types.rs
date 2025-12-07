//! Core data types for the Market interface

use serde::{Deserialize, Serialize};

/// Order side (buy or sell)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    /// Returns true if this is a buy order
    pub fn is_buy(&self) -> bool {
        matches!(self, OrderSide::Buy)
    }

    /// Returns the opposite side
    pub fn opposite(&self) -> Self {
        match self {
            OrderSide::Buy => OrderSide::Sell,
            OrderSide::Sell => OrderSide::Buy,
        }
    }
}

/// Time in force for limit orders
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TimeInForce {
    /// Good till cancelled (default)
    #[default]
    Gtc,
    /// Immediate or cancel
    Ioc,
    /// Add liquidity only (post only, maker only)
    Alo,
}

impl TimeInForce {
    /// Convert to exchange string format
    pub fn as_str(&self) -> &'static str {
        match self {
            TimeInForce::Gtc => "Gtc",
            TimeInForce::Ioc => "Ioc",
            TimeInForce::Alo => "Alo",
        }
    }
}

/// Order request input to the Market
///
/// Represents a new limit order to be placed in the market (spot or perp).
/// The user provides their own `order_id` which will be returned
/// in the fill callback when the order is executed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    /// User-provided order identifier (returned in fill callback)
    pub order_id: u64,
    /// Asset identifier (e.g., "BTC", "ETH", "BTC-PERP")
    pub asset: String,
    /// Order side (buy or sell)
    pub side: OrderSide,
    /// Order quantity (must be > 0)
    pub qty: f64,
    /// Limit price (must be > 0)
    pub limit_price: f64,
    /// Reduce only flag (for perps - only reduce existing position)
    pub reduce_only: bool,
    /// Time in force
    pub tif: TimeInForce,
}

impl OrderRequest {
    /// Create a new order request with default settings (Gtc, not reduce_only)
    ///
    /// # Arguments
    /// * `order_id` - User-provided identifier (returned in fill callback)
    /// * `asset` - Asset to trade
    /// * `side` - Buy or Sell
    /// * `qty` - Order quantity (must be > 0)
    /// * `limit_price` - Limit price (must be > 0)
    ///
    /// # Panics
    /// Panics if qty <= 0 or limit_price <= 0
    pub fn new(
        order_id: u64,
        asset: impl Into<String>,
        side: OrderSide,
        qty: f64,
        limit_price: f64,
    ) -> Self {
        assert!(qty > 0.0, "qty must be greater than 0");
        assert!(limit_price > 0.0, "limit_price must be greater than 0");
        Self {
            order_id,
            asset: asset.into(),
            side,
            qty,
            limit_price,
            reduce_only: false,
            tif: TimeInForce::default(),
        }
    }

    /// Create a buy order
    pub fn buy(order_id: u64, asset: impl Into<String>, qty: f64, limit_price: f64) -> Self {
        Self::new(order_id, asset, OrderSide::Buy, qty, limit_price)
    }

    /// Create a sell order
    pub fn sell(order_id: u64, asset: impl Into<String>, qty: f64, limit_price: f64) -> Self {
        Self::new(order_id, asset, OrderSide::Sell, qty, limit_price)
    }

    /// Set reduce_only flag (builder pattern)
    pub fn reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = reduce_only;
        self
    }

    /// Set time in force (builder pattern)
    pub fn tif(mut self, tif: TimeInForce) -> Self {
        self.tif = tif;
        self
    }

    /// Check if this is a buy order
    pub fn is_buy(&self) -> bool {
        self.side.is_buy()
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
    fn test_order_side() {
        assert!(OrderSide::Buy.is_buy());
        assert!(!OrderSide::Sell.is_buy());
        assert_eq!(OrderSide::Buy.opposite(), OrderSide::Sell);
        assert_eq!(OrderSide::Sell.opposite(), OrderSide::Buy);
    }

    #[test]
    fn test_time_in_force() {
        assert_eq!(TimeInForce::Gtc.as_str(), "Gtc");
        assert_eq!(TimeInForce::Ioc.as_str(), "Ioc");
        assert_eq!(TimeInForce::Alo.as_str(), "Alo");
        assert_eq!(TimeInForce::default(), TimeInForce::Gtc);
    }

    #[test]
    fn test_order_request_new() {
        let order = OrderRequest::new(100, "BTC", OrderSide::Buy, 1.5, 50000.0);
        assert_eq!(order.order_id, 100);
        assert_eq!(order.asset, "BTC");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.qty, 1.5);
        assert_eq!(order.limit_price, 50000.0);
        assert!(!order.reduce_only);
        assert_eq!(order.tif, TimeInForce::Gtc);
        assert!(order.is_valid());
        assert!(order.is_buy());
    }

    #[test]
    fn test_order_request_buy_sell() {
        let buy = OrderRequest::buy(1, "ETH", 2.0, 3000.0);
        assert_eq!(buy.side, OrderSide::Buy);
        assert!(buy.is_buy());

        let sell = OrderRequest::sell(2, "ETH", 2.0, 3100.0);
        assert_eq!(sell.side, OrderSide::Sell);
        assert!(!sell.is_buy());
    }

    #[test]
    fn test_order_request_builder() {
        let order = OrderRequest::buy(1, "BTC", 1.0, 50000.0)
            .reduce_only(true)
            .tif(TimeInForce::Ioc);

        assert!(order.reduce_only);
        assert_eq!(order.tif, TimeInForce::Ioc);
    }

    #[test]
    #[should_panic(expected = "qty must be greater than 0")]
    fn test_order_request_invalid_qty() {
        OrderRequest::new(1, "BTC", OrderSide::Buy, 0.0, 50000.0);
    }

    #[test]
    #[should_panic(expected = "limit_price must be greater than 0")]
    fn test_order_request_invalid_price() {
        OrderRequest::new(1, "BTC", OrderSide::Buy, 1.0, 0.0);
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

