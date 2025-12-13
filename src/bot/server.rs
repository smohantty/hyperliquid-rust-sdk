use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use axum::{
    routing::get,
    Router,
    extract::{State, Query},
    response::{Html, Json},
};
use log::info;
use serde::Deserialize;
use crate::bot::Bot;
use crate::InfoClient;

type BotState = Arc<RwLock<Bot<Box<dyn crate::strategy::Strategy + Send + Sync>>>>;

#[derive(Clone)]
struct ServerState {
    bot: BotState,
    info_client: Arc<InfoClient>,
}

/// Start the dashboard server
pub(crate) async fn start_server(bot: BotState, info_client: Arc<InfoClient>, port: u16, host: String) {
    let state = ServerState { bot, info_client };

    let app = Router::new()
        .route("/", get(dashboard_handler))
        .route("/api/status", get(status_handler))
        .route("/api/candles", get(candles_handler))
        .with_state(state);

    let addr_str = format!("{}:{}", host, port);
    let addr: SocketAddr = addr_str.parse().expect("Invalid address");
    
    info!("Dashboard server running on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn dashboard_handler(State(state): State<ServerState>) -> Html<String> {
    let bot = state.bot.read().await;
    Html(bot.render_dashboard())
}

async fn status_handler(State(state): State<ServerState>) -> Json<serde_json::Value> {
    let bot = state.bot.read().await;
    Json(bot.status_json())
}

#[derive(Deserialize)]
struct CandlesParams {
    coin: String,
    interval: Option<String>,
    // Optional start/end. If missing, fetch recent.
    start: Option<u64>,
    end: Option<u64>,
}

async fn candles_handler(
    State(state): State<ServerState>,
    Query(params): Query<CandlesParams>,
) -> Json<serde_json::Value> {
    let interval = params.interval.unwrap_or_else(|| "15m".to_string());
    
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let end = params.end.unwrap_or(now);
    let start = params.start.unwrap_or(end - 24 * 60 * 60 * 1000); 

    // Always use base coin name for API (e.g. HYPE/USDC -> HYPE)
    let coin = params.coin.split('/').next().unwrap_or(&params.coin).to_string();

    log::info!("Fetching candles for {} ({}) range: {} - {}", coin, interval, start, end);

    match state.info_client.candles_snapshot(coin.clone(), interval, start, end).await {
        Ok(candles) => {
             log::info!("Fetched {} candles for {}", candles.len(), coin);
             Json(serde_json::to_value(candles).unwrap_or(serde_json::Value::Null))
        },
        Err(e) => {
            log::error!("Failed to fetch candles for {}: {}", coin, e);
            Json(serde_json::json!({ "error": e.to_string() }))
        }
    }
}

