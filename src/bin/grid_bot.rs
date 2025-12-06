//! Example Grid Trading Bot Binary
//!
//! This binary demonstrates how to run a grid trading bot using the Hyperliquid SDK.
//!
//! ## Setup
//!
//! 1. Create a `.env` file in the project root:
//!    ```
//!    PRIVATE_KEY=0xYourPrivateKeyHere
//!    USE_MAINNET=0   # Optional: set to 1 for mainnet
//!    ```
//!
//! 2. Run the bot:
//!    ```bash
//!    cargo run --bin grid_bot -- --config config.json
//!    ```
//!
//! ## Security
//!
//! - Never commit your `.env` file to version control
//! - Add `.env` to your `.gitignore`
//! - The `.env` file is loaded automatically from the current directory

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use alloy::signers::local::PrivateKeySigner;
use log::{error, info, warn};
use tokio::sync::Mutex;

use hyperliquid_rust_sdk::{
    grid::{
        GridConfig, GridStrategy, HyperliquidExchange, HyperliquidFillFeed,
        HyperliquidPriceFeed, MarketType, PerpGridRunner, RunnerConfig, SpotGridRunner,
    },
    BaseUrl, ExchangeClient, InfoClient,
};

#[tokio::main]
async fn main() {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Load .env file (optional - won't fail if missing)
    match dotenvy::dotenv() {
        Ok(path) => info!("Loaded environment from: {}", path.display()),
        Err(_) => info!("No .env file found, using system environment variables"),
    }

    // Parse arguments
    let args: Vec<String> = env::args().collect();

    let config = if args.len() > 2 && args[1] == "--config" {
        // Load from config file
        let config_path = PathBuf::from(&args[2]);
        match GridConfig::load_from_file(&config_path) {
            Ok(config) => config,
            Err(e) => {
                error!("Failed to load config: {}", e);
                return;
            }
        }
    } else {
        // Use example config
        info!("No config file provided, using example configuration");
        create_example_config()
    };

    // Get private key from environment (loaded from .env or system env)
    let private_key = match env::var("PRIVATE_KEY") {
        Ok(key) => key,
        Err(_) => {
            error!("PRIVATE_KEY not found!");
            error!("");
            error!("Create a .env file with:");
            error!("  PRIVATE_KEY=0xYourPrivateKeyHere");
            error!("");
            error!("Or set it as an environment variable.");
            return;
        }
    };

    // Parse wallet
    let wallet: PrivateKeySigner = match private_key.parse() {
        Ok(w) => w,
        Err(e) => {
            error!("Failed to parse private key: {}", e);
            return;
        }
    };

    info!("Starting grid bot for {}", config.asset);
    info!("Grid range: {} - {}", config.lower_price, config.upper_price);
    info!("Number of grids: {}", config.num_grids);
    info!("Total investment: ${}", config.total_investment);
    info!("USD per grid: ${:.2}", config.usd_per_grid());
    if let Some(state_file) = &config.state_file {
        info!("State file: {}", state_file.display());
    }

    // Determine base URL (use testnet by default for safety)
    let base_url = if env::var("USE_MAINNET").map(|v| v == "1" || v.to_lowercase() == "true").unwrap_or(false) {
        warn!("⚠️  Using MAINNET - Real funds at risk!");
        BaseUrl::Mainnet
    } else {
        info!("Using TESTNET (set USE_MAINNET=1 in .env for mainnet)");
        BaseUrl::Testnet
    };

    // Create clients
    let info_client = match InfoClient::with_reconnect(None, Some(base_url)).await {
        Ok(client) => Arc::new(Mutex::new(client)),
        Err(e) => {
            error!("Failed to create info client: {}", e);
            return;
        }
    };

    let exchange_client = match ExchangeClient::new(None, wallet, Some(base_url), None, None).await {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create exchange client: {}", e);
            return;
        }
    };

    // Create separate info client for exchange (non-reconnecting)
    let exchange_info_client = match InfoClient::new(None, Some(base_url)).await {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create exchange info client: {}", e);
            return;
        }
    };

    let user_address = exchange_client.wallet.address();
    info!("Wallet address: {}", user_address);

    // Create components
    let exchange = HyperliquidExchange::new(exchange_client, exchange_info_client, &config);
    let price_feed = HyperliquidPriceFeed::new(info_client.clone());
    let fill_feed = HyperliquidFillFeed::new(info_client.clone(), user_address, config.asset.clone());

    let strategy = GridStrategy::arithmetic();
    let runner_config = RunnerConfig::default();

    // Run based on market type
    let result = match config.market_type {
        MarketType::Spot => {
            info!("Starting SPOT grid bot");
            let mut runner = match SpotGridRunner::new(
                config,
                strategy,
                exchange,
                price_feed,
                fill_feed,
                runner_config,
            ) {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to create runner: {}", e);
                    return;
                }
            };

            runner.run().await
        }
        MarketType::Perp => {
            info!("Starting PERP grid bot");
            let mut runner = match PerpGridRunner::new(
                config,
                strategy,
                exchange,
                price_feed,
                fill_feed,
                runner_config,
            ) {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to create runner: {}", e);
                    return;
                }
            };

            runner.run().await
        }
    };

    match result {
        Ok(_) => info!("Grid bot stopped successfully"),
        Err(e) => error!("Grid bot error: {}", e),
    }
}

fn create_example_config() -> GridConfig {
    // Example: PURR/USDC spot grid on testnet
    // $100 total investment, 10 grids => $10/grid
    // Precision is automatically fetched from exchange meta endpoint
    // State file is auto-generated as: grid_PURR-USDC_spot_{timestamp}.json
    GridConfig::new(
        "PURR/USDC",
        0.001,  // Lower price
        0.002,  // Upper price
        10,     // Number of grids
        100.0,  // Total investment in USD
        MarketType::Spot,
    )
}

/// ## .env File Format
///
/// Create a `.env` file in the project root:
///
/// ```env
/// # Required: Your private key (with 0x prefix)
/// PRIVATE_KEY=0x1234567890abcdef...
///
/// # Optional: Set to 1 or true for mainnet (default: testnet)
/// USE_MAINNET=0
///
/// # Optional: Log level (default: info)
/// RUST_LOG=info
/// ```
///
/// ## Config File Format (JSON)
///
/// Note: `tick_precision` and `lot_precision` are NOT needed in config.
/// They are automatically fetched from the exchange's meta endpoint.
///
/// The `total_investment` is the total USD amount to invest. Order size per grid
/// is automatically calculated as: total_investment / num_grids / price_at_level
///
/// State file is auto-generated with format: `grid_{asset}_{spot|perp}_{timestamp}.json`
/// You can override it by setting `state_file` in config, or omit to use auto-generated name.
///
/// ### Spot Example (minimal)
///
/// ```json
/// {
///   "asset": "PURR/USDC",
///   "lower_price": 0.001,
///   "upper_price": 0.002,
///   "num_grids": 10,
///   "total_investment": 100.0,
///   "market_type": "Spot"
/// }
/// ```
///
/// ### Perp Example
///
/// ```json
/// {
///   "asset": "BTC",
///   "lower_price": 40000.0,
///   "upper_price": 50000.0,
///   "num_grids": 20,
///   "total_investment": 10000.0,
///   "market_type": "Perp",
///   "leverage": 5,
///   "max_margin_ratio": 0.8
/// }
/// ```
#[allow(dead_code)]
fn config_examples() {}
