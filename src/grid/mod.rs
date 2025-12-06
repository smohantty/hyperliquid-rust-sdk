//! Grid Trading Module for Hyperliquid
//!
//! This module provides grid trading functionality for both Spot and Perpetual
//! futures markets on Hyperliquid.
//!
//! # Architecture
//!
//! The grid module is organized into several sub-modules:
//!
//! - [`config`] - Grid configuration and validation
//! - [`types`] - Core data types (GridLevel, OrderSide, etc.)
//! - [`errors`] - Grid-specific error types
//! - [`state`] - State management with JSON persistence
//! - [`strategy`] - Grid strategy (arithmetic/geometric)
//! - [`executor`] - Exchange abstraction (mockable for testing)
//!
//! # Example Usage
//!
//! The main entry point is the `grid_bot` binary. See `src/bin/grid_bot.rs` for
//! a complete example following the market_maker pattern.
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
//!
//! # Testing
//!
//! The module provides mock implementations for testing without connecting
//! to the real exchange:
//!
//! ```rust,ignore
//! use hyperliquid_rust_sdk::grid::executor::mock::MockExchange;
//!
//! let exchange = MockExchange::new(150.0);
//! // Use in tests...
//! ```

pub mod config;
pub mod errors;
pub mod executor;
pub mod manager;
pub mod state;
pub mod strategy;
pub mod types;

// Re-export commonly used types
pub use config::{AssetPrecision, GridConfig, InitialPositionMethod, MarketType};
pub use errors::{GridError, GridResult};
pub use executor::{GridExchange, HyperliquidExchange};
pub use manager::{GridManager, GridStateSummary};
pub use state::{GridState, StateManager};
pub use strategy::{FillResult, GridStrategy, GridType, InitialPosition};
pub use types::{
    BotStatus, GridFill, GridLevel, GridOrderRequest, GridProfit, LevelStatus, MarginInfo,
    OrderResult, OrderResultStatus, OrderSide, Position, RiskStatus,
};
