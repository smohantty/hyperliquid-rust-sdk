use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use log::{info, warn};
use alloy::signers::local::PrivateKeySigner;

use crate::config::{self, Settings};
use crate::strategy::StrategyRegistry;
use crate::bot::Bot;
use crate::market::{HyperliquidMarket, HyperliquidMarketInput, PaperTradingMarket, PaperTradingMarketInput};
use crate::BaseUrl;

/// Runner for the trading bot
pub struct BotRunner {
    config: Settings,
    registry: StrategyRegistry,
}

impl BotRunner {
    /// Create a new runner from a configuration file
    pub fn new(config_path: impl AsRef<Path>, registry: StrategyRegistry) -> Result<Self, config::ConfigError> {
        let config = Settings::new(config_path.as_ref().to_str().unwrap())?;
        Ok(Self { config, registry })
    }

    /// Run the bot
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        // 1. Setup Logging
        if std::env::var("RUST_LOG").is_err() {
            std::env::set_var("RUST_LOG", &self.config.log.level);
        }
        env_logger::try_init().ok();

        info!("Starting BotRunner...");

        // 2. Setup Network & Client
        let network_config = &self.config.network;
        let is_mainnet = network_config.env.to_lowercase() == "mainnet";
        let base_url = if is_mainnet { BaseUrl::Mainnet } else { BaseUrl::Testnet };
        let wallet: PrivateKeySigner = network_config.wallet_private_key.parse()?;
        
        // 3. Resolve Asset Precision
        let strategy_config = &self.config.strategy;
        let asset = &strategy_config.asset;
        
        info!("Fetching metadata for {}...", asset);
        let mut params = strategy_config.params.clone();
        
        // We need an InfoClient to fetch meta
        let info_client = crate::InfoClient::new(None, Some(base_url)).await?;
        
        // Try Spot first (common for grid bots here)
        let precision = if let Ok(spot_meta) = info_client.spot_meta().await {
            let base_name = asset.split('/').next().unwrap_or(asset);
            let index_to_name: std::collections::HashMap<usize, &str> = spot_meta.tokens.iter().map(|t| (t.index, t.name.as_str())).collect();
            
            let mut found = None;
            for spot_asset in &spot_meta.universe {
                if let Some(t1) = index_to_name.get(&spot_asset.tokens[0]) {
                    if *t1 == base_name || *asset == spot_asset.name {
                        if let Some(base_token) = spot_meta.tokens.iter().find(|t| t.index == spot_asset.tokens[0]) {
                            found = Some(crate::market::AssetPrecision::for_spot(base_token.sz_decimals as u32));
                        }
                        break;
                    }
                }
            }
            found
        } else {
            None
        };
        
        // If not Spot, try Perp
        let precision = if precision.is_some() {
            precision
        } else if let Ok(meta) = info_client.meta().await {
             if let Some(asset_meta) = meta.universe.iter().find(|a| a.name == *asset) {
                Some(crate::market::AssetPrecision::for_perp(asset_meta.sz_decimals))
            } else {
                None
            }
        } else {
            None
        };
        
        if let Some(p) = precision {
            info!("Resolved precision: sz_decimals={}, price_decimals={}", p.sz_decimals, p.price_decimals);
            params.insert("sz_decimals".to_string(), serde_json::Value::from(p.sz_decimals));
            params.insert("price_decimals".to_string(), serde_json::Value::from(p.price_decimals));
            params.insert("max_decimals".to_string(), serde_json::Value::from(p.max_decimals));
        } else {
            warn!("Could not resolve precision for {}. Using defaults/config values.", asset);
        }

        // 3.5. Fetch Initial Price and Wait for Trigger
        let trigger_price = params.get("trigger_price").and_then(|v| v.as_f64());
        info!("Fetching initial price...");
        let initial_price = loop {
            // We need to resolve the asset to a coin index or name for the API
            // For now, assume info_client handles standard asset names or we get all mids
            if let Ok(mids) = info_client.all_mids().await {
                // Try to find price for the asset
                // The asset key in mids might differ slightly (e.g. "HYPE" vs "HYPE/USDC")
                // For Spot, it's usually the base coin name
                let price_opt = if let Some(px) = mids.get(asset) {
                    Some(px)
                } else {
                    let base = asset.split('/').next().unwrap_or(asset);
                    mids.get(base)
                };

                if let Some(price_str) = price_opt {
                    if let Ok(price) = price_str.parse::<f64>() {
                        
                        if let Some(trigger) = trigger_price {
                            info!("Current price: {}, Trigger price: {}", price, trigger);
                            if price <= trigger {
                                info!("Trigger price reached!");
                                break price;
                            } else {
                                info!("Waiting for trigger...");
                                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                continue;
                            }
                        } else {
                            break price;
                        }
                    }
                }
            }
            
            warn!("Failed to fetch price for {}, retrying in 5s...", asset);
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        };
        
        info!("Starting strategy with initial price: {}", initial_price);
        params.insert("initial_price".to_string(), serde_json::Value::from(initial_price));

        // 4. Instantiate Strategy
        let strategy = self.registry
            .create_strategy(&strategy_config.type_name, asset, params)
            .ok_or_else(|| format!("Unknown strategy type: {}", strategy_config.type_name))?;
        
        info!("Strategy '{}' initialized for {}", strategy.name(), asset);

        // 5. Create Bot Wrapper
        let bot = Arc::new(RwLock::new(Bot::new(strategy)));

        // 6. Create Market based on mode
        match network_config.mode.as_str() {
            "live" => {
                info!("Initializing LIVE market on {}...", if is_mainnet { "Mainnet" } else { "Testnet" });
                let input = HyperliquidMarketInput {
                    asset: asset.clone(),
                    wallet,
                    base_url: Some(base_url),
                };
                let mut market = HyperliquidMarket::new(input, bot.clone()).await?;
                info!("Live market ready. Starting event loop...");
                market.start().await;
            },
            "paper" => {
                info!("Initializing PAPER market...");
                let input = PaperTradingMarketInput::new(asset, 10_000.0);
                let mut market = PaperTradingMarket::new(input, bot.clone()).await?;
                info!("Paper market ready. Starting event loop...");
                market.start().await;
            },
            _ => return Err(format!("Unknown mode: {}", network_config.mode).into()),
        }

        Ok(())
    }
}
