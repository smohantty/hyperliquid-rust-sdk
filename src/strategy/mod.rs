//! Strategy Interface Module
//!
//! This module provides a generic strategy interface for building trading strategies.
//! Strategies are decoupled from markets - they receive price updates and fill notifications,
//! and return actions (orders) to be executed.
//!
//! # Design Philosophy
//!
//! - **Decoupled**: Strategies don't know about markets or execution venues
//! - **Testable**: Can be tested by calling `on_price_update` and `on_order_filled` directly
//! - **Composable**: Multiple strategies can be combined or wrapped
//! - **Stateful**: Strategies maintain their own internal state
//!
//! # Example
//!
//! ```rust
//! use hyperliquid_rust_sdk::strategy::{Strategy, StrategyAction};
//! use hyperliquid_rust_sdk::market::{OrderRequest, OrderFill};
//!
//! struct SimpleStrategy {
//!     has_position: bool,
//! }
//!
//! impl Strategy for SimpleStrategy {
//!     fn on_price_update(&mut self, asset: &str, price: f64) -> StrategyAction {
//!         if !self.has_position && price < 50000.0 {
//!             // Buy when price drops below threshold
//!             StrategyAction::single(OrderRequest::buy(1, asset, 0.1, price))
//!         } else {
//!             StrategyAction::none()
//!         }
//!     }
//!
//!     fn on_order_filled(&mut self, fill: &OrderFill) -> StrategyAction {
//!         self.has_position = true;
//!         StrategyAction::none()
//!     }
//! }
//! ```
//!
//! # Connecting Strategy to Market
//!
//! Use `StrategyAdapter` to bridge a strategy to a market:
//!
//! ```ignore
//! use hyperliquid_rust_sdk::strategy::StrategyAdapter;
//!
//! let strategy = MyStrategy::new();
//! let adapter = StrategyAdapter::new(strategy);
//! let market = HyperliquidMarket::new(input, adapter).await?;
//! ```

mod action;
mod adapter;
mod traits;

pub use action::StrategyAction;
pub use adapter::{shared_adapter, SharedStrategyAdapter, StrategyAdapter};
pub use traits::{NoOpStrategy, Strategy};

