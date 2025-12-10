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
}

impl OrderRequest {
    /// Create a new order request with default settings (not reduce_only)
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

/// Asset information including balances and precision
///
/// Contains all the information needed to trade an asset:
/// - Asset name and balances (base and quote)
/// - Precision for size and price (decimal places)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetInfo {
    /// Asset name (e.g., "BTC", "ETH", "HYPE/USDC")
    pub name: String,
    /// Base asset balance (e.g., BTC balance for BTC/USDC)
    pub balance: f64,
    /// Quote currency balance (USDC)
    pub usdc_balance: f64,
    /// Size decimals (number of decimal places for quantity)
    pub sz_decimals: u32,
    /// Price decimals (number of decimal places for price)
    pub price_decimals: u32,
}

impl AssetInfo {
    /// Create new asset info
    pub fn new(
        name: impl Into<String>,
        balance: f64,
        usdc_balance: f64,
        sz_decimals: u32,
        price_decimals: u32,
    ) -> Self {
        Self {
            name: name.into(),
            balance,
            usdc_balance,
            sz_decimals,
            price_decimals,
        }
    }

    /// Get the size step (minimum size increment)
    pub fn sz_step(&self) -> f64 {
        10f64.powi(-(self.sz_decimals as i32))
    }

    /// Get the price step (minimum price increment)
    pub fn price_step(&self) -> f64 {
        10f64.powi(-(self.price_decimals as i32))
    }

    /// Round size to valid precision
    pub fn round_size(&self, size: f64) -> f64 {
        let factor = 10f64.powi(self.sz_decimals as i32);
        (size * factor).floor() / factor
    }

    /// Round price to valid precision
    ///
    /// # Arguments
    /// * `price` - The price to round
    /// * `round_up` - If true, round up (for sell orders), else round down (for buy orders)
    pub fn round_price(&self, price: f64, round_up: bool) -> f64 {
        let factor = 10f64.powi(self.price_decimals as i32);
        if round_up {
            (price * factor).ceil() / factor
        } else {
            (price * factor).floor() / factor
        }
    }

    /// Check if we have sufficient balance for a buy order
    pub fn can_buy(&self, qty: f64, price: f64) -> bool {
        let cost = qty * price;
        self.usdc_balance >= cost
    }

    /// Check if we have sufficient balance for a sell order
    pub fn can_sell(&self, qty: f64) -> bool {
        self.balance >= qty
    }
}

impl Default for AssetInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            balance: 0.0,
            usdc_balance: 0.0,
            sz_decimals: 4,
            price_decimals: 2,
        }
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
    fn test_order_request_new() {
        let order = OrderRequest::new(100, "BTC", OrderSide::Buy, 1.5, 50000.0);
        assert_eq!(order.order_id, 100);
        assert_eq!(order.asset, "BTC");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.qty, 1.5);
        assert_eq!(order.limit_price, 50000.0);
        assert!(!order.reduce_only);
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
            .reduce_only(true);

        assert!(order.reduce_only);
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

    #[test]
    fn test_asset_info_new() {
        let info = AssetInfo::new("BTC", 1.5, 10000.0, 4, 2);
        assert_eq!(info.name, "BTC");
        assert_eq!(info.balance, 1.5);
        assert_eq!(info.usdc_balance, 10000.0);
        assert_eq!(info.sz_decimals, 4);
        assert_eq!(info.price_decimals, 2);
    }

    #[test]
    fn test_asset_info_steps() {
        let info = AssetInfo::new("ETH", 0.0, 0.0, 4, 2);
        assert!((info.sz_step() - 0.0001).abs() < 1e-10);
        assert!((info.price_step() - 0.01).abs() < 1e-10);
    }

    #[test]
    fn test_asset_info_round_size() {
        let info = AssetInfo::new("BTC", 0.0, 0.0, 4, 2);
        assert_eq!(info.round_size(1.23456), 1.2345);
        assert_eq!(info.round_size(1.23451), 1.2345);
        assert_eq!(info.round_size(0.00001), 0.0);
    }

    #[test]
    fn test_asset_info_round_price() {
        let info = AssetInfo::new("BTC", 0.0, 0.0, 4, 2);

        // Round down (for buy orders)
        assert_eq!(info.round_price(50000.456, false), 50000.45);

        // Round up (for sell orders)
        assert_eq!(info.round_price(50000.451, true), 50000.46);
    }

    #[test]
    fn test_asset_info_can_buy_sell() {
        let info = AssetInfo::new("BTC", 1.0, 50000.0, 4, 2);

        // Can buy: 0.5 BTC at 50000 = 25000 USDC, we have 50000
        assert!(info.can_buy(0.5, 50000.0));

        // Cannot buy: 2 BTC at 50000 = 100000 USDC, we only have 50000
        assert!(!info.can_buy(2.0, 50000.0));

        // Can sell: 0.5 BTC, we have 1.0
        assert!(info.can_sell(0.5));

        // Cannot sell: 2 BTC, we only have 1.0
        assert!(!info.can_sell(2.0));
    }

    #[test]
    fn test_asset_info_default() {
        let info = AssetInfo::default();
        assert_eq!(info.name, "");
        assert_eq!(info.balance, 0.0);
        assert_eq!(info.usdc_balance, 0.0);
        assert_eq!(info.sz_decimals, 4);
        assert_eq!(info.price_decimals, 2);
    }
}

use crate::helpers::truncate_float;

/// Asset precision information fetched from exchange meta
///
/// According to Hyperliquid docs:
/// - Prices can have up to 5 significant figures
/// - Price decimals = MAX_DECIMALS - szDecimals (6 for perps, 8 for spot)
/// - Size decimals = szDecimals from meta
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AssetPrecision {
    /// Decimal places for size (szDecimals from meta)
    pub sz_decimals: u32,
    /// Decimal places for price (calculated from market type)
    pub price_decimals: u32,
    /// Maximum decimals constant (6 for perps, 8 for spot)
    pub max_decimals: u32,
}

impl AssetPrecision {
    /// Create precision for a perp asset
    pub fn for_perp(sz_decimals: u32) -> Self {
        const MAX_DECIMALS_PERP: u32 = 6;
        Self {
            sz_decimals,
            price_decimals: MAX_DECIMALS_PERP.saturating_sub(sz_decimals),
            max_decimals: MAX_DECIMALS_PERP,
        }
    }

    /// Create precision for a spot asset
    pub fn for_spot(sz_decimals: u32) -> Self {
        const MAX_DECIMALS_SPOT: u32 = 6;
        Self {
            sz_decimals,
            price_decimals: MAX_DECIMALS_SPOT.saturating_sub(sz_decimals + 1),
            max_decimals: MAX_DECIMALS_SPOT,
        }
    }

    /// Round a price to the correct precision using truncate_float
    ///
    /// Enforces Hyperliquid's tick size rules:
    /// - Max 5 significant figures
    /// - Max price_decimals decimal places (MAX_DECIMALS - szDecimals)
    pub fn round_price(&self, price: f64, round_up: bool) -> f64 {
        truncate_float(price, self.price_decimals, round_up)
    }

    /// Round a size to the correct precision
    pub fn round_size(&self, size: f64) -> f64 {
        truncate_float(size, self.sz_decimals, false)
    }
}

impl Default for AssetPrecision {
    fn default() -> Self {
        Self::for_perp(0)
    }
}

