//! Bot Module
//!
//! The Bot combines a Market and a Strategy, handling the event loop
//! and executing orders returned by the strategy.
//!
//! # Example
//!
//! ```ignore
//! use hyperliquid_rust_sdk::bot::{Bot, BotConfig, MarketMode};
//! use hyperliquid_rust_sdk::strategy::Strategy;
//!
//! // Create config
//! let config = BotConfig::paper("HYPE/USDC", 10_000.0);
//! // or: BotConfig::live("HYPE/USDC", wallet, Some(BaseUrl::Mainnet));
//!
//! // Create bot with your strategy
//! let mut bot = Bot::new(config, MyStrategy::new()).await?;
//!
//! // Run the bot (processes events and executes strategy)
//! bot.run().await;
//! ```

mod config;
mod runner;

pub use config::{BotConfig, MarketMode};
pub use runner::Bot;

