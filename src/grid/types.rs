//! Core data types for grid trading

use serde::{Deserialize, Serialize};

/// Order side for grid levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    /// Returns the opposite side
    pub fn opposite(&self) -> Self {
        match self {
            OrderSide::Buy => OrderSide::Sell,
            OrderSide::Sell => OrderSide::Buy,
        }
    }

    /// Convert to exchange side string
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderSide::Buy => "B",
            OrderSide::Sell => "A",
        }
    }
}

impl From<&str> for OrderSide {
    fn from(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "B" | "BUY" => OrderSide::Buy,
            _ => OrderSide::Sell,
        }
    }
}

/// Bot execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BotStatus {
    /// Waiting for trigger price to be hit
    WaitingForEntry,
    /// Acquiring quote asset (e.g., USDC) for trading
    AcquiringFunds,
    /// Placing initial base position
    Initializing,
    /// Normal grid operation
    Running,
    /// Paused (manual or risk-triggered)
    Paused,
    /// Shutting down
    Stopping,
    /// Fully stopped
    Stopped,
}

impl BotStatus {
    /// Check if the bot is in an active trading state
    pub fn is_active(&self) -> bool {
        matches!(self, BotStatus::Running | BotStatus::Initializing)
    }

    /// Check if the bot should process price updates
    pub fn should_process_prices(&self) -> bool {
        matches!(
            self,
            BotStatus::WaitingForEntry | BotStatus::Running | BotStatus::Initializing
        )
    }
}

/// Status of an individual grid level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LevelStatus {
    /// No order placed at this level
    Empty,
    /// Order sent, waiting for exchange confirmation
    Pending,
    /// Order resting on the order book
    Active,
    /// Order was filled, waiting for replacement
    Filled,
    /// Order cancelled
    Cancelled,
}

/// Individual grid level tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridLevel {
    /// Index of this level (0 = lowest price)
    pub index: u32,
    /// Price at this level
    pub price: f64,
    /// Intended action when price crosses this level
    pub intended_side: OrderSide,
    /// Exchange order ID (if resting/filled)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oid: Option<u64>,
    /// Current status of this level
    pub status: LevelStatus,
    /// Last fill price at this level (for PnL calculation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fill_price: Option<f64>,
}

impl GridLevel {
    /// Create a new grid level
    pub fn new(index: u32, price: f64, intended_side: OrderSide) -> Self {
        Self {
            index,
            price,
            intended_side,
            oid: None,
            status: LevelStatus::Empty,
            last_fill_price: None,
        }
    }

    /// Check if this level has an active order
    pub fn has_active_order(&self) -> bool {
        matches!(self.status, LevelStatus::Active | LevelStatus::Pending)
    }

    /// Reset the level to empty state
    pub fn reset(&mut self) {
        self.oid = None;
        self.status = LevelStatus::Empty;
    }

    /// Mark as pending (order sent, waiting for confirmation)
    pub fn mark_pending(&mut self) {
        self.status = LevelStatus::Pending;
    }

    /// Mark as active with exchange oid
    pub fn mark_active(&mut self, oid: u64) {
        self.oid = Some(oid);
        self.status = LevelStatus::Active;
    }

    /// Mark as filled
    pub fn mark_filled(&mut self, fill_price: f64) {
        self.status = LevelStatus::Filled;
        self.last_fill_price = Some(fill_price);
    }
}

/// Request to place a grid order
#[derive(Debug, Clone)]
pub struct GridOrderRequest {
    /// Level index this order belongs to
    pub level_index: u32,
    /// Order price
    pub price: f64,
    /// Order size
    pub size: f64,
    /// Order side
    pub side: OrderSide,
    /// Whether this is a reduce-only order
    pub reduce_only: bool,
}

impl GridOrderRequest {
    /// Create a new grid order request
    pub fn new(level_index: u32, price: f64, size: f64, side: OrderSide) -> Self {
        Self {
            level_index,
            price,
            size,
            side,
            reduce_only: false,
        }
    }

    /// Set reduce_only flag
    pub fn reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = reduce_only;
        self
    }
}

/// Fill event from exchange
#[derive(Debug, Clone)]
pub struct GridFill {
    /// Exchange order ID
    pub oid: u64,
    /// Fill price
    pub price: f64,
    /// Fill size
    pub size: f64,
    /// Order side
    pub side: OrderSide,
    /// Fee paid
    pub fee: f64,
    /// Fee token
    pub fee_token: String,
    /// Asset/coin
    pub coin: String,
    /// Timestamp
    pub timestamp: u64,
    /// Closed PnL (for perps)
    pub closed_pnl: f64,
}

/// Profit tracking for grid
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GridProfit {
    /// Total realized PnL
    pub realized_pnl: f64,
    /// Total fees paid
    pub total_fees: f64,
    /// Number of completed round trips (buy->sell or sell->buy)
    pub num_round_trips: u32,
    /// Total volume traded
    pub total_volume: f64,
}

impl GridProfit {
    /// Add a completed trade to profit tracking
    pub fn add_trade(&mut self, pnl: f64, fee: f64, volume: f64) {
        self.realized_pnl += pnl;
        self.total_fees += fee;
        self.total_volume += volume;
    }

    /// Increment round trip counter
    pub fn complete_round_trip(&mut self) {
        self.num_round_trips += 1;
    }

    /// Net profit after fees
    pub fn net_profit(&self) -> f64 {
        self.realized_pnl - self.total_fees
    }
}

/// Position information for perps
#[derive(Debug, Clone, Default)]
pub struct Position {
    /// Position size (positive = long, negative = short)
    pub size: f64,
    /// Entry price
    pub entry_price: Option<f64>,
    /// Unrealized PnL
    pub unrealized_pnl: f64,
    /// Liquidation price
    pub liquidation_price: Option<f64>,
    /// Margin used
    pub margin_used: f64,
}

/// Margin information for risk checks
#[derive(Debug, Clone, Default)]
pub struct MarginInfo {
    /// Total account value
    pub account_value: f64,
    /// Total margin used
    pub margin_used: f64,
    /// Available margin
    pub available_margin: f64,
    /// Withdrawable amount
    pub withdrawable: f64,
}

impl MarginInfo {
    /// Calculate margin ratio (margin_used / account_value)
    pub fn margin_ratio(&self) -> f64 {
        if self.account_value > 0.0 {
            self.margin_used / self.account_value
        } else {
            0.0
        }
    }
}

/// Order result from exchange
#[derive(Debug, Clone)]
pub struct OrderResult {
    /// Exchange order ID
    pub oid: u64,
    /// Result status
    pub status: OrderResultStatus,
}

/// Status of order placement
#[derive(Debug, Clone)]
pub enum OrderResultStatus {
    /// Order is resting on the book
    Resting,
    /// Order was immediately filled
    Filled {
        avg_price: f64,
        filled_size: f64,
    },
    /// Order was rejected
    Rejected(String),
    /// Order is waiting for trigger (stop orders)
    WaitingForTrigger,
}

/// Risk status for perp trading
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskStatus {
    /// Safe to continue trading
    Safe,
    /// Warning level - reduce exposure
    Warning,
    /// High risk - should stop trading
    HighRisk,
    /// Critical - immediate shutdown required
    Critical,
}
