//! Grid Trading Module for Hyperliquid
//!
//! This module provides a modular grid trading bot implementation that works
//! with both Spot and Perpetual futures markets on Hyperliquid.
//!
//! # Architecture
//!
//! The grid module is organized into several sub-modules:
//!
//! - [`config`] - Grid configuration and validation
//! - [`types`] - Core data types (GridLevel, OrderSide, etc.)
//! - [`errors`] - Grid-specific error types
//! - [`state`] - State management with JSON persistence
//! - [`strategy`] - Grid strategy (arithmetic or geometric spacing)
//! - [`executor`] - Exchange abstraction (mockable for testing)
//! - [`manager`] - Core grid logic
//! - [`spot`] - Spot-specific grid manager
//! - [`perp`] - Perp-specific grid manager with risk checks
//! - [`runner`] - Main execution loop
//!
//! # Example Usage
//!
//! ```rust,ignore
//! use hyperliquid_rust_sdk::grid::{
//!     GridConfig, MarketType, GridStrategy,
//!     SpotGridRunner, RunnerConfig,
//! };
//!
//! // Create configuration
//! // $100 total investment, 20 grids, price range $0.01 - $0.02
//! let config = GridConfig::new("PURR/USDC", 0.01, 0.02, 20, 100.0, MarketType::Spot)
//!     .with_state_file("grid_state.json");
//!
//! // Create strategy (arithmetic = uniform spacing, geometric = percentage spacing)
//! let strategy = GridStrategy::arithmetic();
//!
//! // Create runner (with real exchange, price feed, fill feed)
//! let mut runner = SpotGridRunner::new(
//!     config,
//!     strategy,
//!     exchange,
//!     price_feed,
//!     fill_feed,
//!     RunnerConfig::default(),
//! )?;
//!
//! // Run the bot
//! runner.run().await?;
//! ```
//!
//! # Testing
//!
//! The module provides mock implementations for testing without connecting
//! to the real exchange:
//!
//! ```rust,ignore
//! use hyperliquid_rust_sdk::grid::executor::mock::{MockExchange, MockPriceFeed, MockFillFeed};
//!
//! let exchange = MockExchange::new(150.0);
//! let price_feed = MockPriceFeed::new();
//! let fill_feed = MockFillFeed::new();
//!
//! // Use in tests...
//! ```

pub mod config;
pub mod errors;
pub mod executor;
pub mod manager;
pub mod perp;
pub mod runner;
pub mod spot;
pub mod state;
pub mod strategy;
pub mod types;

// Re-export commonly used types
pub use config::{AssetPrecision, GridConfig, InitialPositionMethod, MarketType};
pub use errors::{GridError, GridResult};
pub use executor::{FillFeed, GridExchange, PriceFeed};
pub use manager::{GridManager, GridStateSummary};
pub use perp::PerpGridManager;
pub use runner::{GridRunnerKind, PerpGridRunner, RunnerConfig, SpotGridRunner};
pub use spot::SpotGridManager;
pub use state::{GridState, StateManager};
pub use strategy::{FillResult, GridStrategy, GridType, InitialPosition};
pub use types::{
    BotStatus, GridFill, GridLevel, GridOrderRequest, GridProfit, LevelStatus, MarginInfo,
    OrderResult, OrderResultStatus, OrderSide, Position, RiskStatus,
};

// Re-export Hyperliquid implementations
pub use executor::{HyperliquidExchange, HyperliquidFillFeed, HyperliquidPriceFeed};
