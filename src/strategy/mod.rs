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
//!     next_order_id: u64,
//! }
//!
//! impl Strategy for SimpleStrategy {
//!     fn on_price_update(&mut self, asset: &str, price: f64) -> StrategyAction {
//!         if !self.has_position && price < 50000.0 {
//!             self.next_order_id += 1;
//!             StrategyAction::single(OrderRequest::buy(self.next_order_id, asset, 0.1, price))
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
//!         let action = self.strategy.on_price_update(asset, price);
//!         for order in action {
//!             self.market.place_order(order);
//!         }
//!     }
//!
//!     fn on_order_filled(&mut self, fill: OrderFill) {
//!         let action = self.strategy.on_order_filled(&fill);
//!         for order in action {
//!             self.market.place_order(order);
//!         }
//!     }
//! }
//! ```

mod action;
mod traits;

pub use action::StrategyAction;
pub use traits::{NoOpStrategy, Strategy};

