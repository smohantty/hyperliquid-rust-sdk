//! Strategy Interface Module
//!
//! This module provides a generic strategy interface for building trading strategies.
//! Strategies are decoupled from markets - they receive price updates and fill notifications,
//! and return orders to be executed.
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
//! use hyperliquid_rust_sdk::strategy::Strategy;
//! use hyperliquid_rust_sdk::market::{OrderRequest, OrderFill};
//!
//! struct SimpleStrategy {
//!     has_position: bool,
//!     next_order_id: u64,
//! }
//!
//! impl Strategy for SimpleStrategy {
//!     fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
//!         if !self.has_position && price < 50000.0 {
//!             self.next_order_id += 1;
//!             vec![OrderRequest::buy(self.next_order_id, asset, 0.1, price)]
//!         } else {
//!             vec![]
//!         }
//!     }
//!
//!     fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
//!         self.has_position = true;
//!         vec![]
//!     }
//! }
//! ```
//!
//! # Usage with a Bot
//!
//! The bot owns the strategy and calls it from MarketListener callbacks:
//!
//! ```ignore
//! struct Bot {
//!     strategy: MyStrategy,
//!     market: HyperliquidMarket<Self>,
//! }
//!
//! impl MarketListener for Bot {
//!     fn on_price_update(&mut self, asset: &str, price: f64) {
//!         for order in self.strategy.on_price_update(asset, price) {
//!             self.market.place_order(order);
//!         }
//!     }
//!
//!     fn on_order_filled(&mut self, fill: OrderFill) {
//!         for order in self.strategy.on_order_filled(&fill) {
//!             self.market.place_order(order);
//!         }
//!     }
//! }
//! ```

pub mod registry;
pub mod spot_grid;
mod traits;

pub use registry::{StrategyFactory, StrategyRegistry};
pub use traits::{NoOpStrategy, Strategy, StrategyStatus};
