//! Bot Module
//!
//! The Bot is a MarketListener that wraps a Strategy.
//! It receives market events, calls the strategy, and collects orders to execute.
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
//! // Market runs event loop, bot receives callbacks
//! // After processing, get pending orders from bot and place them
//! market.start().await;
//! ```

mod bot;

pub use bot::Bot;
