//! Bot Module
//!
//! The Bot is a MarketListener that wraps a Strategy.
//! It receives market events, calls the strategy, and returns orders to place.
//! The market automatically places orders returned by the listener callbacks.
//!
//! # Usage Pattern
//!
//! ```ignore
//! use hyperliquid_rust_sdk::bot::Bot;
//! use hyperliquid_rust_sdk::market::{HyperliquidMarket, HyperliquidMarketInput};
//! use hyperliquid_rust_sdk::strategy::Strategy;
//!
//! // Create your strategy
//! let strategy = MyStrategy::new();
//!
//! // Create bot wrapping the strategy
//! let bot = Bot::new(strategy);
//!
//! // Create market with bot as listener
//! let mut market = HyperliquidMarket::new(input, bot).await?;
//!
//! // Market runs event loop, bot receives callbacks and returns orders
//! // Market automatically places orders returned by the listener
//! market.start().await;
//! ```

mod bot;

pub use bot::{render_default_dashboard, Bot};
pub mod runner;
pub use runner::BotRunner;
