//! Grid Trading Module for Hyperliquid
//!
//! Simple grid trading bot for Spot and Perpetual futures markets.
//!
//! # Usage
//!
//! The main entry point is the `grid_bot` binary:
//!
//! ```bash
//! # Create config file
//! cat > config.json << EOF
//! {
//!   "asset": "HYPE/USDC",
//!   "lower_price": 20.0,
//!   "upper_price": 40.0,
//!   "num_grids": 50,
//!   "total_investment": 10000.0,
//!   "market_type": "Spot"
//! }
//! EOF
//!
//! # Run the bot
//! cargo run --bin grid_bot -- --config config.json
//! ```

pub mod config;
pub mod errors;
pub mod state;
pub mod strategy;
pub mod types;

// Re-export commonly used types
pub use config::{AssetPrecision, GridConfig, InitialPositionMethod, MarketType};
pub use errors::{GridError, GridResult};
pub use state::{GridState, StateManager};
pub use strategy::{FillResult, GridStrategy, GridType, InitialPosition};
pub use types::{
    BotStatus, GridFill, GridLevel, GridOrderRequest, GridProfit, LevelStatus, MarginInfo,
    OrderResult, OrderResultStatus, OrderSide, Position, RiskStatus,
};
