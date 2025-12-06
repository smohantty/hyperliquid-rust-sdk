//! Grid Trading Bot with HTTP Status Server
//!
//! This binary runs a grid trading bot with a web dashboard for monitoring.
//!
//! ## Setup
//!
//! 1. Create a `.env` file:
//!    ```
//!    PRIVATE_KEY=0xYourPrivateKeyHere
//!    USE_MAINNET=0
//!    STATUS_PORT=3000  # Optional, default 3000
//!    ```
//!
//! 2. Run the bot:
//!    ```bash
//!    cargo run --bin grid_bot -- --config config.json
//!    ```
//!
//! 3. View status at: http://localhost:3000

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alloy::signers::local::PrivateKeySigner;
use axum::{
    extract::State,
    response::{Html, Json},
    routing::get,
    Router,
};
use log::{debug, error, info, warn};
use serde::Serialize;
use tokio::sync::{mpsc::unbounded_channel, RwLock};
use tokio::time::interval;

use hyperliquid_rust_sdk::{
    grid::{
        config::AssetPrecision, GridConfig, GridStrategy, MarketType,
        StateManager, BotStatus, OrderSide, LevelStatus,
    },
    BaseUrl, ExchangeClient, InfoClient, Message, Subscription, TradeInfo,
    ClientOrderRequest, ClientOrder, ClientLimit, ClientCancelRequest,
    ExchangeResponseStatus, ExchangeDataStatus,
};

/// Shared state for HTTP status server
#[derive(Clone)]
struct AppState {
    bot_status: Arc<RwLock<BotStatusData>>,
}

/// Bot status data exposed via HTTP
#[derive(Debug, Clone, Serialize)]
struct BotStatusData {
    // Basic info
    asset: String,
    market_type: String,
    status: String,

    // Price info
    current_price: f64,
    lower_price: f64,
    upper_price: f64,

    // Grid info
    num_grids: u32,
    total_levels: usize,
    active_buys: usize,
    active_sells: usize,

    // Investment
    total_investment: f64,
    usd_per_grid: f64,

    // Position & PnL
    current_position: f64,
    realized_pnl: f64,
    total_fees: f64,
    net_profit: f64,
    round_trips: u32,

    // Grid levels (summary)
    levels: Vec<LevelInfo>,

    // Timestamps
    uptime_secs: u64,
    last_updated: String,
}

#[derive(Debug, Clone, Serialize)]
struct LevelInfo {
    index: u32,
    price: f64,
    side: String,
    status: String,
    has_order: bool,
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Try to load .env file from current directory or search upward
    match dotenvy::dotenv() {
        Ok(path) => info!("Loaded environment from: {}", path.display()),
        Err(_) => {
            // Try finding .env in parent directories
            let mut current_dir = std::env::current_dir().ok();
            let mut found = false;
            for _ in 0..5 {
                if let Some(dir) = &current_dir {
                    let env_path = dir.join(".env");
                    if env_path.exists() {
                        if dotenvy::from_path(&env_path).is_ok() {
                            info!("Loaded environment from: {}", env_path.display());
                            found = true;
                            break;
                        }
                    }
                    current_dir = dir.parent().map(|p| p.to_path_buf());
                } else {
                    break;
                }
            }
            if !found {
                warn!("No .env file found (searched current directory and parents). PRIVATE_KEY must be in .env file.");
            }
        }
    }

    // Parse arguments
    // Handle both: `./grid_bot --config file.json` and `cargo run -- --config file.json`
    let args: Vec<String> = env::args().collect();
    let config = if args.len() >= 3 {
        // Find --config argument (skip "--" separator from cargo run)
        let config_idx = args.iter().position(|a| a == "--config");
        if let Some(idx) = config_idx {
            if idx + 1 < args.len() {
                let config_path = PathBuf::from(&args[idx + 1]);
                match GridConfig::load_from_file(&config_path) {
                    Ok(config) => config,
                    Err(e) => {
                        error!("Failed to load config from {}: {}", config_path.display(), e);
                        return;
                    }
                }
            } else {
                error!("--config requires a file path");
                return;
            }
        } else {
            info!("No config file provided, using example configuration");
            create_example_config()
        }
    } else {
        info!("No config file provided, using example configuration");
        create_example_config()
    };

    // PRIVATE_KEY must be in .env file only (for security)
    let private_key = match env::var("PRIVATE_KEY") {
        Ok(key) => key,
        Err(_) => {
            error!("PRIVATE_KEY not found in .env file!");
            error!("Create a .env file in the project root with:");
            error!("  PRIVATE_KEY=0xYourPrivateKeyHere");
            error!("Note: PRIVATE_KEY must be set in .env file for security reasons.");
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

    let initial_price = match info_client.all_mids().await {
        Ok(mids) => mids.get(&asset_key).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0),
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

    let strategy = GridStrategy {
        grid_mode: config.grid_mode,
    };
    info!("Grid mode: {:?}", config.grid_mode);
    let levels = strategy.calculate_grid_levels(&config, &precision);
    info!("Created {} grid levels", levels.len());

    let state_manager = match StateManager::load_or_create(&config, levels) {
        Ok(sm) => sm,
        Err(e) => {
            error!("Failed to create state manager: {}", e);
            return;
        }
    };

    // Create shared status for HTTP server
    let bot_status = Arc::new(RwLock::new(BotStatusData {
        asset: config.asset.clone(),
        market_type: format!("{:?}", config.market_type),
        status: "Initializing".to_string(),
        current_price: initial_price,
        lower_price: config.lower_price,
        upper_price: config.upper_price,
        num_grids: config.num_grids,
        total_levels: config.num_levels() as usize,
        active_buys: 0,
        active_sells: 0,
        total_investment: config.total_investment,
        usd_per_grid: config.usd_per_grid(),
        current_position: 0.0,
        realized_pnl: 0.0,
        total_fees: 0.0,
        net_profit: 0.0,
        round_trips: 0,
        levels: Vec::new(),
        uptime_secs: 0,
        last_updated: chrono::Utc::now().to_rfc3339(),
    }));

    let app_state = AppState { bot_status: bot_status.clone() };

    // Start HTTP status server
    let status_port: u16 = env::var("STATUS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let app = Router::new()
        .route("/", get(dashboard_handler))
        .route("/status", get(status_handler))
        .route("/api/status", get(status_handler))
        .with_state(app_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], status_port));
    info!("📊 Status server starting at http://localhost:{}", status_port);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    // Create bot
    let mut bot = GridBot {
        config,
        asset_key,
        precision,
        strategy,
        state_manager,
        exchange_client,
        latest_price: initial_price,
        status: BotStatus::Initializing,
        bot_status,
        start_time: std::time::Instant::now(),
    };

    // Subscribe to WebSocket feeds
    let (sender, mut receiver) = unbounded_channel();

    info_client
        .subscribe(Subscription::UserFills { user: user_address }, sender.clone())
        .await
        .unwrap();

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
    bot.update_status().await;
    info!("Grid bot is now RUNNING");

    // Main loop
    let mut save_timer = interval(Duration::from_secs(30));
    let mut status_timer = interval(Duration::from_secs(2));

    loop {
        tokio::select! {
            Some(message) = receiver.recv() => {
                match message {
                    Message::AllMids(all_mids) => {
                        if let Some(price_str) = all_mids.data.mids.get(&bot.asset_key) {
                            if let Ok(price) = price_str.parse::<f64>() {
                                bot.latest_price = price;

                                // Check if price entered grid range (for WaitingForEntry status)
                                if bot.status == BotStatus::WaitingForEntry {
                                    let in_range = price >= bot.config.lower_price && price <= bot.config.upper_price;
                                    let trigger_met = bot.config.trigger_price.map_or(true, |tp| price <= tp);

                                    if in_range && trigger_met {
                                        info!("Price {} entered grid range! Starting grid bot...", price);
                                        if let Err(e) = bot.place_initial_orders().await {
                                            error!("Failed to place initial orders: {}", e);
                                        } else {
                                            bot.status = BotStatus::Running;
                                            bot.update_status().await;
                                            info!("Grid bot is now RUNNING");
                                        }
                                    } else {
                                        debug!("Price {} still outside range [{}, {}]", price, bot.config.lower_price, bot.config.upper_price);
                                    }
                                } else {
                                    debug!("Price update: {}", price);
                                }
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
            _ = status_timer.tick() => {
                bot.update_status().await;
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
    bot_status: Arc<RwLock<BotStatusData>>,
    start_time: std::time::Instant,
}

impl GridBot {
    async fn update_status(&self) {
        let state = self.state_manager.read().await;

        let levels: Vec<LevelInfo> = state.levels.iter().map(|l| LevelInfo {
            index: l.index,
            price: l.price,
            side: format!("{:?}", l.intended_side),
            status: format!("{:?}", l.status),
            has_order: l.has_active_order(),
        }).collect();

        let active_buys = state.count_active_buys();
        let active_sells = state.count_active_sells();

        let mut status = self.bot_status.write().await;
        status.status = format!("{:?}", self.status);
        status.current_price = self.latest_price;
        status.active_buys = active_buys;
        status.active_sells = active_sells;
        status.current_position = state.current_position;
        status.realized_pnl = state.profit.realized_pnl;
        status.total_fees = state.profit.total_fees;
        status.net_profit = state.profit.net_profit();
        status.round_trips = state.profit.num_round_trips;
        status.levels = levels;
        status.uptime_secs = self.start_time.elapsed().as_secs();
        status.last_updated = chrono::Utc::now().to_rfc3339();
    }

    async fn place_initial_orders(&mut self) -> Result<(), String> {
        let init_pos = self.strategy.calculate_initial_position(
            &self.config, &self.precision, self.latest_price,
            &self.state_manager.read().await.levels,
        );

        info!("Placing {} grid orders...", init_pos.grid_orders.len());

        let mut placed = 0;
        for order in init_pos.grid_orders {
            info!("place_initial_orders: level={}, side={:?}, price={}, size={}",
                   order.level_index, order.side, order.price, order.size);
            match self.place_order(order.price, order.size, order.side == OrderSide::Buy, Some(order.level_index)).await {
                Ok(oid) => {
                    placed += 1;
                    self.state_manager.update(|state| {
                        state.register_order(order.level_index, oid);
                        if let Some(level) = state.get_level_mut(order.level_index) {
                            level.oid = Some(oid);
                            level.status = LevelStatus::Active;
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

    async fn place_order(&self, price: f64, size: f64, is_buy: bool, level_index: Option<u32>) -> Result<u64, String> {
        // Always round price and size before sending to exchange
        // This ensures compliance with Hyperliquid's tick/lot size rules
        // For limit orders: buys round down (false), sells round up (true)
        let rounded_price = self.precision.round_price(price, !is_buy);
        let rounded_size = self.precision.round_size(size);

        let level_info = level_index.map(|idx| format!("level={} ", idx)).unwrap_or_default();
        debug!("place_order: {}price={} -> {}, size={} -> {} (is_buy={})",
               level_info, price, rounded_price, size, rounded_size, is_buy);

        let order = ClientOrderRequest {
            asset: self.asset_key.clone(),
            is_buy,
            reduce_only: false,
            limit_px: rounded_price,
            sz: rounded_size,
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

    #[allow(dead_code)]
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

        self.state_manager.update(|state| {
            state.unregister_order(oid);
            if let Some(level) = state.get_level_mut(level_idx) {
                level.mark_filled(fill_price);
                level.intended_side = if is_buy { OrderSide::Sell } else { OrderSide::Buy };
                level.status = LevelStatus::Empty;
            }
            if is_buy {
                state.current_position += fill_size;
            } else {
                state.current_position -= fill_size;
            }
        }).await.map_err(|e| e.to_string())?;

        let (adj_idx, new_is_buy) = if is_buy {
            (level_idx.saturating_add(1), false)
        } else {
            (level_idx.saturating_sub(1), true)
        };

        let adj_price = {
            let state = self.state_manager.read().await;
            state.get_level(adj_idx).map(|l| l.price)
        };

        if let Some(price) = adj_price {
            let size = self.config.calculate_order_size_at_price(price, &self.precision);
            info!("Placing replacement {} @ {} size {}", if new_is_buy { "BUY" } else { "SELL" }, price, size);

            match self.place_order(price, size, new_is_buy, Some(adj_idx)).await {
                Ok(new_oid) => {
                    self.state_manager.update(|state| {
                        state.register_order(adj_idx, new_oid);
                        if let Some(level) = state.get_level_mut(adj_idx) {
                            level.oid = Some(new_oid);
                            level.status = LevelStatus::Active;
                        }
                    }).await.ok();
                    info!("Replacement order placed: oid={}", new_oid);
                }
                Err(e) => {
                    error!("Failed to place replacement: {}", e);
                }
            }
        }

        let summary = self.state_manager.read().await;
        info!("Position: {:.4}, PnL: {:.2}", summary.current_position, summary.profit.net_profit());

        self.update_status().await;
        Ok(())
    }
}

// HTTP Handlers
async fn status_handler(State(state): State<AppState>) -> Json<BotStatusData> {
    Json(state.bot_status.read().await.clone())
}

async fn dashboard_handler(State(state): State<AppState>) -> Html<String> {
    let status = state.bot_status.read().await;
    let current_price = status.current_price;

    // Build levels HTML with current price marker
    let mut levels_html = String::new();
    let mut price_marker_inserted = false;

    // Sort levels by price descending (highest first) for display
    let mut sorted_levels = status.levels.clone();
    sorted_levels.sort_by(|a, b| b.price.partial_cmp(&a.price).unwrap());

    for l in &sorted_levels {
        // Insert current price marker when we pass it
        if !price_marker_inserted && l.price < current_price {
            levels_html.push_str(&format!(
                r#"<tr class="price-marker">
                    <td colspan="4">
                        <div class="price-line">
                            <span class="price-label">$ {:.4}</span>
                        </div>
                    </td>
                </tr>"#,
                current_price
            ));
            price_marker_inserted = true;
        }

        // Determine styling
        let side_color = if l.side == "Buy" { "#22c55e" } else { "#ef4444" };
        let row_class = if l.has_order { "level-active" } else { "level-empty" };

        levels_html.push_str(&format!(
            r#"<tr class="{}">
                <td class="level-num">{}</td>
                <td class="level-price">${:.4}</td>
                <td><span class="side-badge" style="color: {}">{}</span></td>
                <td class="level-status">{}</td>
            </tr>"#,
            row_class, l.index, l.price, side_color, l.side,
            if l.has_order { "●" } else { "—" }
        ));
    }

    // If price is below all levels, add marker at bottom
    if !price_marker_inserted {
        levels_html.push_str(&format!(
            r#"<tr class="price-marker">
                <td colspan="4">
                    <div class="price-line">
                        <span class="price-label">$ {:.4}</span>
                    </div>
                </td>
            </tr>"#,
            current_price
        ));
    }

    let html = format!(r##"
<!DOCTYPE html>
<html>
<head>
    <title>{asset} Grid Bot</title>
    <meta http-equiv="refresh" content="5">
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
    <style>
        :root {{
            --bg-primary: #0a0a0f;
            --bg-secondary: #12121a;
            --bg-card: #1a1a24;
            --border: #2a2a3a;
            --text-primary: #ffffff;
            --text-secondary: #8888a0;
            --text-muted: #5a5a70;
            --green: #00d4aa;
            --red: #ff4d6a;
            --yellow: #ffd93d;
            --blue: #4d9fff;
        }}
        * {{ margin: 0; padding: 0; box-sizing: border-box; }}
        body {{
            font-family: 'Inter', -apple-system, sans-serif;
            background: var(--bg-primary);
            color: var(--text-primary);
            min-height: 100vh;
        }}
        .container {{ max-width: 1400px; margin: 0 auto; padding: 24px; }}

        header {{
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 32px;
            padding-bottom: 24px;
            border-bottom: 1px solid var(--border);
        }}
        .logo {{
            display: flex;
            align-items: center;
            gap: 12px;
        }}
        .logo h1 {{
            font-size: 20px;
            font-weight: 600;
        }}
        .logo .asset {{
            color: var(--blue);
        }}
        .status-pill {{
            display: flex;
            align-items: center;
            gap: 8px;
            padding: 8px 16px;
            border-radius: 20px;
            font-size: 13px;
            font-weight: 500;
            background: {status_bg};
        }}
        .status-dot {{
            width: 8px;
            height: 8px;
            border-radius: 50%;
            background: {status_dot};
            animation: pulse 2s infinite;
        }}
        @keyframes pulse {{
            0%, 100% {{ opacity: 1; }}
            50% {{ opacity: 0.5; }}
        }}

        .stats-grid {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 16px;
            margin-bottom: 32px;
        }}
        .stat-card {{
            background: var(--bg-card);
            border: 1px solid var(--border);
            border-radius: 12px;
            padding: 20px;
        }}
        .stat-label {{
            font-size: 12px;
            color: var(--text-muted);
            text-transform: uppercase;
            letter-spacing: 0.5px;
            margin-bottom: 8px;
        }}
        .stat-value {{
            font-family: 'JetBrains Mono', monospace;
            font-size: 24px;
            font-weight: 600;
        }}
        .stat-value.green {{ color: var(--green); }}
        .stat-value.red {{ color: var(--red); }}
        .stat-sub {{
            font-size: 12px;
            color: var(--text-muted);
            margin-top: 4px;
        }}

        .levels-card {{
            background: var(--bg-card);
            border: 1px solid var(--border);
            border-radius: 12px;
            overflow: hidden;
        }}
        .levels-header {{
            padding: 16px 20px;
            border-bottom: 1px solid var(--border);
            display: flex;
            justify-content: space-between;
            align-items: center;
        }}
        .levels-title {{
            font-size: 14px;
            font-weight: 600;
        }}
        .levels-count {{
            font-size: 12px;
            color: var(--text-muted);
        }}

        table {{
            width: 100%;
            border-collapse: collapse;
        }}
        th {{
            text-align: left;
            padding: 12px 20px;
            font-size: 11px;
            font-weight: 500;
            color: var(--text-muted);
            text-transform: uppercase;
            letter-spacing: 0.5px;
            background: var(--bg-secondary);
        }}
        td {{
            padding: 10px 20px;
            font-size: 13px;
            border-bottom: 1px solid var(--border);
        }}
        tr:hover {{ background: rgba(255,255,255,0.02); }}

        .level-num {{
            font-family: 'JetBrains Mono', monospace;
            color: var(--text-muted);
            font-size: 12px;
        }}
        .level-price {{
            font-family: 'JetBrains Mono', monospace;
            font-weight: 500;
        }}
        .side-badge {{
            font-weight: 600;
            font-size: 12px;
        }}
        .level-status {{
            font-size: 16px;
        }}
        .level-active {{ background: rgba(255,255,255,0.03); }}
        .level-empty td {{ color: var(--text-secondary); }}

        .price-marker td {{
            padding: 0 !important;
            border: none !important;
        }}
        .price-line {{
            display: flex;
            align-items: center;
            padding: 4px 20px;
        }}
        .price-line::before,
        .price-line::after {{
            content: '';
            flex: 1;
            height: 1px;
            background: var(--yellow);
        }}
        .price-label {{
            padding: 4px 16px;
            font-family: 'JetBrains Mono', monospace;
            font-size: 14px;
            font-weight: 600;
            color: var(--yellow);
        }}

        footer {{
            margin-top: 24px;
            padding-top: 16px;
            border-top: 1px solid var(--border);
            display: flex;
            justify-content: space-between;
            font-size: 12px;
            color: var(--text-muted);
        }}
        footer a {{ color: var(--blue); text-decoration: none; }}
        footer a:hover {{ text-decoration: underline; }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <div class="logo">
                <h1>Grid Bot · <span class="asset">{asset}</span></h1>
            </div>
            <div class="status-pill">
                <span class="status-dot"></span>
                {status}
            </div>
        </header>

        <div class="stats-grid">
            <div class="stat-card">
                <div class="stat-label">Current Price</div>
                <div class="stat-value">${current_price:.4}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Grid Range</div>
                <div class="stat-value">${lower_price:.2} — ${upper_price:.2}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Position</div>
                <div class="stat-value">{position:.4}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Net Profit</div>
                <div class="stat-value {profit_class}">${net_profit:.4}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Round Trips</div>
                <div class="stat-value">{round_trips}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Active Orders</div>
                <div class="stat-value"><span class="green">{active_buys}</span> / <span class="red">{active_sells}</span></div>
                <div class="stat-sub">Buy / Sell</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Investment</div>
                <div class="stat-value">${total_investment:.0}</div>
                <div class="stat-sub">${usd_per_grid:.2} per grid</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Uptime</div>
                <div class="stat-value">{uptime}</div>
            </div>
        </div>

        <div class="levels-card">
            <div class="levels-header">
                <span class="levels-title">Grid Levels</span>
                <span class="levels-count">{num_grids} grids · {total_levels} levels</span>
            </div>
            <table>
                <thead>
                    <tr>
                        <th>Level</th>
                        <th>Price</th>
                        <th>Side</th>
                        <th>Order</th>
                    </tr>
                </thead>
                <tbody>
                    {levels_html}
                </tbody>
            </table>
        </div>

        <footer>
            <span>Updated: {last_updated}</span>
            <span>Auto-refresh: 5s · <a href="/api/status">JSON API</a></span>
        </footer>
    </div>
</body>
</html>
"##,
        asset = status.asset,
        status = status.status,
        status_bg = if status.status == "Running" { "rgba(0,212,170,0.15)" } else { "rgba(255,77,106,0.15)" },
        status_dot = if status.status == "Running" { "#00d4aa" } else { "#ff4d6a" },
        current_price = status.current_price,
        lower_price = status.lower_price,
        upper_price = status.upper_price,
        position = status.current_position,
        net_profit = status.net_profit,
        profit_class = if status.net_profit >= 0.0 { "green" } else { "red" },
        round_trips = status.round_trips,
        active_buys = status.active_buys,
        active_sells = status.active_sells,
        total_investment = status.total_investment,
        usd_per_grid = status.usd_per_grid,
        uptime = format_duration(status.uptime_secs),
        num_grids = status.num_grids,
        total_levels = status.total_levels,
        levels_html = levels_html,
        last_updated = status.last_updated,
    );

    Html(html)
}

fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    format!("{}h {}m {}s", hours, mins, secs)
}

async fn resolve_spot_asset(info_client: &InfoClient, asset: &str) -> Result<String, String> {
    let spot_meta = info_client.spot_meta().await.map_err(|e| e.to_string())?;
    let index_to_name: std::collections::HashMap<usize, &str> = spot_meta.tokens.iter().map(|t| (t.index, t.name.as_str())).collect();
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
            let asset_meta = meta.universe.iter().find(|a| a.name == asset).ok_or_else(|| format!("Asset not found: {}", asset))?;
            Ok(AssetPrecision::for_perp(asset_meta.sz_decimals))
        }
        MarketType::Spot => {
            let spot_meta = info_client.spot_meta().await.map_err(|e| e.to_string())?;
            let base_name = asset.split('/').next().unwrap_or(asset);
            let index_to_name: std::collections::HashMap<usize, &str> = spot_meta.tokens.iter().map(|t| (t.index, t.name.as_str())).collect();

            for spot_asset in &spot_meta.universe {
                if let Some(t1) = index_to_name.get(&spot_asset.tokens[0]) {
                    if *t1 == base_name || asset == spot_asset.name {
                        let base_token = spot_meta.tokens.iter().find(|t| t.index == spot_asset.tokens[0]).ok_or_else(|| "Token not found".to_string())?;
                        return Ok(AssetPrecision::for_spot(base_token.sz_decimals as u32));
                    }
                }
            }
            Err(format!("Asset not found: {}", asset))
        }
    }
}

fn create_example_config() -> GridConfig {
    GridConfig::new("HYPE/USDC", 25.0, 33.0, 20, 100.0, MarketType::Spot)
}
