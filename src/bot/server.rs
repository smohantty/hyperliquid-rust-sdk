use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use axum::{
    routing::get,
    Router,
    extract::State,
    response::{Html, Json},
};
use log::info;
use crate::bot::Bot;

type BotState = Arc<RwLock<Bot<Box<dyn crate::strategy::Strategy + Send + Sync>>>>;

/// Start the dashboard server
pub(crate) async fn start_server(bot: BotState, port: u16, host: String) {
    let app = Router::new()
        .route("/", get(dashboard_handler))
        .route("/api/status", get(status_handler))
        .with_state(bot);

    let addr_str = format!("{}:{}", host, port);
    let addr: SocketAddr = addr_str.parse().expect("Invalid address");
    
    info!("Dashboard server running on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn dashboard_handler(State(bot): State<BotState>) -> Html<String> {
    let bot = bot.read().await;
    Html(bot.render_dashboard())
}

async fn status_handler(State(bot): State<BotState>) -> Json<serde_json::Value> {
    let bot = bot.read().await;
    Json(bot.status_json())
}
