//! Grid Trading Bot Binary
//!
//! This binary runs a grid trading bot using the Hyperliquid SDK.
//! Follows the same WebSocket pattern as the market_maker example.
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

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use alloy::signers::local::PrivateKeySigner;
use log::{debug, error, info, warn};
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::interval;

use hyperliquid_rust_sdk::{
    grid::{
        config::AssetPrecision, GridConfig, GridStrategy, MarketType,
        StateManager, BotStatus, OrderSide,
    },
    BaseUrl, ExchangeClient, InfoClient, Message, Subscription, TradeInfo,
    ClientOrderRequest, ClientOrder, ClientLimit, ClientCancelRequest,
    ExchangeResponseStatus, ExchangeDataStatus,
};

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Load .env file
    match dotenvy::dotenv() {
        Ok(path) => info!("Loaded environment from: {}", path.display()),
        Err(_) => info!("No .env file found, using system environment variables"),
    }

    // Parse arguments
    let args: Vec<String> = env::args().collect();
    let config = if args.len() > 2 && args[1] == "--config" {
        let config_path = PathBuf::from(&args[2]);
        match GridConfig::load_from_file(&config_path) {
            Ok(config) => config,
            Err(e) => {
                error!("Failed to load config: {}", e);
                return;
            }
        }
    } else {
        info!("No config file provided, using example configuration");
        create_example_config()
    };

    // Get private key
    let private_key = match env::var("PRIVATE_KEY") {
        Ok(key) => key,
        Err(_) => {
            error!("PRIVATE_KEY not found! Create a .env file with: PRIVATE_KEY=0xYourPrivateKeyHere");
            return;
        }
    };

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

    let base_url = if env::var("USE_MAINNET").map(|v| v == "1" || v.to_lowercase() == "true").unwrap_or(false) {
        warn!("⚠️  Using MAINNET - Real funds at risk!");
        BaseUrl::Mainnet
    } else {
        info!("Using TESTNET");
        BaseUrl::Testnet
    };

    // Create clients (following market_maker pattern)
    let mut info_client = match InfoClient::new(None, Some(base_url)).await {
        Ok(client) => client,
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

    let user_address = exchange_client.wallet.address();
    info!("Wallet address: {}", user_address);

    // Resolve spot asset to index format (e.g., "HYPE/USDC" -> "@107")
    let asset_key = match resolve_spot_asset(&info_client, &config.asset).await {
        Ok(key) => {
            info!("Resolved {} -> {}", config.asset, key);
            key
        }
        Err(e) => {
            error!("Failed to resolve asset: {}", e);
            return;
        }
    };

    // Fetch asset precision
    let precision = match fetch_asset_precision(&info_client, &config.asset, config.market_type).await {
        Ok(p) => {
            info!("Asset precision: sz_decimals={}, price_decimals={}", p.sz_decimals, p.price_decimals);
            p
        }
        Err(e) => {
            error!("Failed to fetch precision: {}", e);
            return;
        }
    };

    // Get initial mid price
    let initial_price = match info_client.all_mids().await {
        Ok(mids) => {
            mids.get(&asset_key)
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0)
        }
        Err(e) => {
            error!("Failed to get mid price: {}", e);
            return;
        }
    };

    if initial_price <= 0.0 {
        error!("Invalid initial price for {}: {}", asset_key, initial_price);
        return;
    }

    info!("Initial price: {}", initial_price);

    if initial_price < config.lower_price || initial_price > config.upper_price {
        error!("Price {} is outside grid range [{}, {}]", initial_price, config.lower_price, config.upper_price);
        return;
    }

    // Create grid state
    let strategy = GridStrategy::arithmetic();
    let levels = strategy.calculate_grid_levels(&config, &precision);
    info!("Created {} grid levels", levels.len());

    let state_manager = match StateManager::load_or_create(&config, levels) {
        Ok(sm) => sm,
        Err(e) => {
            error!("Failed to create state manager: {}", e);
            return;
        }
    };

    // Create a simple grid bot following market_maker pattern
    let mut bot = GridBot {
        config,
        asset_key,
        precision,
        strategy,
        state_manager,
        exchange_client,
        latest_price: initial_price,
        status: BotStatus::Initializing,
    };

    // Subscribe to WebSocket feeds (following market_maker pattern)
    let (sender, mut receiver) = unbounded_channel();

    // Subscribe to user fills
    info_client
        .subscribe(Subscription::UserFills { user: user_address }, sender.clone())
        .await
        .unwrap();

    // Subscribe to all mids for price updates
    info_client
        .subscribe(Subscription::AllMids, sender)
        .await
        .unwrap();

    info!("Subscribed to WebSocket feeds");

    // Place initial grid orders
    if let Err(e) = bot.place_initial_orders().await {
        error!("Failed to place initial orders: {}", e);
        return;
    }

    bot.status = BotStatus::Running;
    info!("Grid bot is now RUNNING");

    // Main loop (following market_maker pattern)
    let mut save_timer = interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            Some(message) = receiver.recv() => {
                match message {
                    Message::AllMids(all_mids) => {
                        if let Some(price_str) = all_mids.data.mids.get(&bot.asset_key) {
                            if let Ok(price) = price_str.parse::<f64>() {
                                bot.latest_price = price;
                                debug!("Price update: {}", price);
                            }
                        }
                    }
                    Message::UserFills(user_fills) => {
                        for fill in user_fills.data.fills {
                            if fill.coin == bot.asset_key {
                                info!("Fill: {} {} @ {} (oid: {})", fill.side, fill.sz, fill.px, fill.oid);
                                if let Err(e) = bot.handle_fill(&fill).await {
                                    error!("Error handling fill: {}", e);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = save_timer.tick() => {
                if let Err(e) = bot.state_manager.force_save().await {
                    warn!("Failed to save state: {}", e);
                }
            }
        }
    }
}

struct GridBot {
    config: GridConfig,
    asset_key: String,
    precision: AssetPrecision,
    strategy: GridStrategy,
    state_manager: StateManager,
    exchange_client: ExchangeClient,
    latest_price: f64,
    status: BotStatus,
}

impl GridBot {
    async fn place_initial_orders(&mut self) -> Result<(), String> {
        let init_pos = self.strategy.calculate_initial_position(
            &self.config, &self.precision, self.latest_price,
            &self.state_manager.read().await.levels,
        );

        info!("Placing {} grid orders...", init_pos.grid_orders.len());

        let mut placed = 0;
        for order in init_pos.grid_orders {
            match self.place_order(order.price, order.size, order.side == OrderSide::Buy).await {
                Ok(oid) => {
                    placed += 1;
                    self.state_manager.update(|state| {
                        state.register_order(order.level_index, oid);
                        if let Some(level) = state.get_level_mut(order.level_index) {
                            level.oid = Some(oid);
                            level.status = hyperliquid_rust_sdk::grid::LevelStatus::Active;
                        }
                    }).await.ok();
                }
                Err(e) => {
                    warn!("Failed to place order at {}: {}", order.price, e);
                }
            }
        }

        info!("Placed {} orders", placed);
        Ok(())
    }

    async fn place_order(&self, price: f64, size: f64, is_buy: bool) -> Result<u64, String> {
        let order = ClientOrderRequest {
            asset: self.asset_key.clone(),
            is_buy,
            reduce_only: false,
            limit_px: price,
            sz: size,
            cloid: None,
            order_type: ClientOrder::Limit(ClientLimit { tif: "Gtc".to_string() }),
        };

        match self.exchange_client.order(order, None).await {
            Ok(ExchangeResponseStatus::Ok(resp)) => {
                if let Some(data) = resp.data {
                    if let Some(status) = data.statuses.first() {
                        match status {
                            ExchangeDataStatus::Resting(r) => return Ok(r.oid),
                            ExchangeDataStatus::Filled(f) => return Ok(f.oid),
                            ExchangeDataStatus::Error(e) => return Err(e.clone()),
                            _ => {}
                        }
                    }
                }
                Err("No status in response".to_string())
            }
            Ok(ExchangeResponseStatus::Err(e)) => Err(e),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn cancel_order(&self, oid: u64) -> Result<bool, String> {
        let cancel = ClientCancelRequest {
            asset: self.asset_key.clone(),
            oid,
        };

        match self.exchange_client.cancel(cancel, None).await {
            Ok(ExchangeResponseStatus::Ok(resp)) => {
                if let Some(data) = resp.data {
                    if let Some(ExchangeDataStatus::Success) = data.statuses.first() {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Ok(ExchangeResponseStatus::Err(e)) => Err(e),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn handle_fill(&mut self, fill: &TradeInfo) -> Result<(), String> {
        let oid = fill.oid;
        let fill_price: f64 = fill.px.parse().unwrap_or(0.0);
        let fill_size: f64 = fill.sz.parse().unwrap_or(0.0);
        let is_buy = fill.side == "B";

        // Find the level for this fill
        let level_idx = {
            let state = self.state_manager.read().await;
            state.find_level_index_by_oid(oid)
        };

        let level_idx = match level_idx {
            Some(idx) => idx,
            None => {
                debug!("Fill for unknown oid {}", oid);
                return Ok(());
            }
        };

        info!("Processing fill for level {}: {} {} @ {}", level_idx, if is_buy { "BUY" } else { "SELL" }, fill_size, fill_price);

        // Update state
        self.state_manager.update(|state| {
            state.unregister_order(oid);
            if let Some(level) = state.get_level_mut(level_idx) {
                level.mark_filled(fill_price);
                level.intended_side = if is_buy { OrderSide::Sell } else { OrderSide::Buy };
                level.status = hyperliquid_rust_sdk::grid::LevelStatus::Empty;
            }
            // Update position
            if is_buy {
                state.current_position += fill_size;
            } else {
                state.current_position -= fill_size;
            }
        }).await.map_err(|e| e.to_string())?;

        // Place replacement order at adjacent level
        let (adj_idx, new_is_buy) = if is_buy {
            (level_idx.saturating_add(1), false) // Place sell above
        } else {
            (level_idx.saturating_sub(1), true) // Place buy below
        };

        let adj_price = {
            let state = self.state_manager.read().await;
            state.get_level(adj_idx).map(|l| l.price)
        };

        if let Some(price) = adj_price {
            let size = self.config.calculate_order_size_at_price(price, &self.precision);
            info!("Placing replacement {} @ {} size {}", if new_is_buy { "BUY" } else { "SELL" }, price, size);

            match self.place_order(price, size, new_is_buy).await {
                Ok(new_oid) => {
                    self.state_manager.update(|state| {
                        state.register_order(adj_idx, new_oid);
                        if let Some(level) = state.get_level_mut(adj_idx) {
                            level.oid = Some(new_oid);
                            level.status = hyperliquid_rust_sdk::grid::LevelStatus::Active;
                        }
                    }).await.ok();
                    info!("Replacement order placed: oid={}", new_oid);
                }
                Err(e) => {
                    error!("Failed to place replacement: {}", e);
                }
            }
        }

        // Log profit summary
        let summary = self.state_manager.read().await;
        info!("Position: {:.4}, PnL: {:.2}", summary.current_position, summary.profit.net_profit());

        Ok(())
    }
}

async fn resolve_spot_asset(info_client: &InfoClient, asset: &str) -> Result<String, String> {
    let spot_meta = info_client.spot_meta().await.map_err(|e| e.to_string())?;

    let index_to_name: std::collections::HashMap<usize, &str> = spot_meta
        .tokens
        .iter()
        .map(|t| (t.index, t.name.as_str()))
        .collect();

    let base_name = asset.split('/').next().unwrap_or(asset);

    for spot_asset in &spot_meta.universe {
        if let Some(t1) = index_to_name.get(&spot_asset.tokens[0]) {
            if *t1 == base_name || asset == spot_asset.name {
                return Ok(format!("@{}", spot_asset.index));
            }
        }
    }

    Err(format!("Asset not found: {}", asset))
}

async fn fetch_asset_precision(info_client: &InfoClient, asset: &str, market_type: MarketType) -> Result<AssetPrecision, String> {
    match market_type {
        MarketType::Perp => {
            let meta = info_client.meta().await.map_err(|e| e.to_string())?;
            let asset_meta = meta.universe.iter().find(|a| a.name == asset)
                .ok_or_else(|| format!("Asset not found: {}", asset))?;
            Ok(AssetPrecision::for_perp(asset_meta.sz_decimals))
        }
        MarketType::Spot => {
            let spot_meta = info_client.spot_meta().await.map_err(|e| e.to_string())?;
            let base_name = asset.split('/').next().unwrap_or(asset);

            let index_to_name: std::collections::HashMap<usize, &str> = spot_meta
                .tokens
                .iter()
                .map(|t| (t.index, t.name.as_str()))
                .collect();

            for spot_asset in &spot_meta.universe {
                if let Some(t1) = index_to_name.get(&spot_asset.tokens[0]) {
                    if *t1 == base_name || asset == spot_asset.name {
                        let base_token = spot_meta.tokens.iter()
                            .find(|t| t.index == spot_asset.tokens[0])
                            .ok_or_else(|| "Token not found".to_string())?;
                        return Ok(AssetPrecision::for_spot(base_token.sz_decimals as u32));
                    }
                }
            }

            Err(format!("Asset not found: {}", asset))
        }
    }
}

fn create_example_config() -> GridConfig {
    GridConfig::new("PURR/USDC", 0.001, 0.002, 10, 100.0, MarketType::Spot)
}
