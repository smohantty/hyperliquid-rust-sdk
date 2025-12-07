//! Bot configuration

use alloy::signers::local::PrivateKeySigner;

use crate::BaseUrl;

/// Market mode for the bot
#[derive(Debug, Clone)]
pub enum MarketMode {
    /// Live trading on Hyperliquid exchange
    Live {
        wallet: PrivateKeySigner,
        base_url: Option<BaseUrl>,
    },
    /// Paper trading with simulated fills
    Paper {
        initial_balance: f64,
    },
}

/// Bot configuration
#[derive(Debug, Clone)]
pub struct BotConfig {
    /// Asset to trade (e.g., "BTC", "HYPE/USDC")
    pub asset: String,
    /// Market mode (live or paper)
    pub mode: MarketMode,
}

impl BotConfig {
    /// Create config for live trading
    ///
    /// # Arguments
    /// * `asset` - Asset to trade
    /// * `wallet` - Wallet for signing transactions
    /// * `base_url` - Optional base URL (defaults to Mainnet)
    pub fn live(
        asset: impl Into<String>,
        wallet: PrivateKeySigner,
        base_url: Option<BaseUrl>,
    ) -> Self {
        Self {
            asset: asset.into(),
            mode: MarketMode::Live { wallet, base_url },
        }
    }

    /// Create config for paper trading
    ///
    /// # Arguments
    /// * `asset` - Asset to trade
    /// * `initial_balance` - Starting USDC balance
    pub fn paper(asset: impl Into<String>, initial_balance: f64) -> Self {
        Self {
            asset: asset.into(),
            mode: MarketMode::Paper { initial_balance },
        }
    }

    /// Check if this is live trading mode
    pub fn is_live(&self) -> bool {
        matches!(self.mode, MarketMode::Live { .. })
    }

    /// Check if this is paper trading mode
    pub fn is_paper(&self) -> bool {
        matches!(self.mode, MarketMode::Paper { .. })
    }
}

