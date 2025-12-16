//! Test Bot Application
//!
//! Demonstrates integration of Bot, Market, Strategy, and HTTP Server.
//!
//! Run with: cargo run --bin test_bot

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::State,
    response::{Html, Json},
    routing::get,
    Router,
};
use log::info;
use tokio::sync::RwLock;

use hyperliquid_rust_sdk::{
    bot::Bot,
    market::{OrderFill, OrderRequest, PaperTradingMarket, PaperTradingMarketInput},
    strategy::{Strategy, StrategyStatus},
};

/// Simple test strategy that tracks price and generates occasional orders
struct TestStrategy {
    asset: String,
    current_price: f64,
    position: f64,
    realized_pnl: f64,
    trade_count: u32,
    next_order_id: u64,
    buy_threshold: f64,
    sell_threshold: f64,
}

impl TestStrategy {
    fn new(asset: &str) -> Self {
        Self {
            asset: asset.to_string(),
            current_price: 0.0,
            position: 0.0,
            realized_pnl: 0.0,
            trade_count: 0,
            next_order_id: 0,
            buy_threshold: 0.0, // Will be set on first price
            sell_threshold: 0.0,
        }
    }
}

impl Strategy for TestStrategy {
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
        if asset != self.asset {
            return vec![];
        }

        self.current_price = price;

        // Set thresholds on first price
        if self.buy_threshold == 0.0 {
            self.buy_threshold = price * 0.99; // Buy 1% below
            self.sell_threshold = price * 1.01; // Sell 1% above
            info!(
                "Strategy initialized: buy below {:.4}, sell above {:.4}",
                self.buy_threshold, self.sell_threshold
            );
        }

        let mut orders = vec![];

        // Simple strategy: buy low, sell high
        if self.position <= 0.0 && price <= self.buy_threshold {
            self.next_order_id += 1;
            orders.push(OrderRequest::buy(self.next_order_id, asset, 0.1, price));
            info!("Strategy: placing BUY order at {:.4}", price);
        } else if self.position > 0.0 && price >= self.sell_threshold {
            self.next_order_id += 1;
            orders.push(OrderRequest::sell(
                self.next_order_id,
                asset,
                self.position,
                price,
            ));
            info!("Strategy: placing SELL order at {:.4}", price);
        }

        orders
    }

    fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
        info!(
            "Strategy: order {} filled - {} @ {:.4}",
            fill.order_id, fill.qty, fill.price
        );

        // Update position (simplified - real impl would check buy/sell)
        if self.position <= 0.0 {
            // Was a buy
            self.position += fill.qty;
            // Reset thresholds based on fill price
            self.sell_threshold = fill.price * 1.02; // Sell 2% above entry
        } else {
            // Was a sell
            let profit = (fill.price - self.sell_threshold / 1.02) * fill.qty;
            self.realized_pnl += profit;
            self.position -= fill.qty;
            self.trade_count += 1;
            // Reset buy threshold
            self.buy_threshold = fill.price * 0.98;
        }

        vec![]
    }

    fn name(&self) -> &str {
        "test_strategy"
    }

    fn status(&self) -> StrategyStatus {
        StrategyStatus::new(self.name(), &self.asset)
            .with_status(if self.position > 0.0 { "Long" } else { "Flat" })
            .with_price(self.current_price)
            .with_position(self.position)
            .with_pnl(self.realized_pnl, 0.0, 0.0)
            .with_custom(serde_json::json!({
                "buy_threshold": self.buy_threshold,
                "sell_threshold": self.sell_threshold,
                "trade_count": self.trade_count,
            }))
    }
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let asset = "HYPE/USDC";
    info!("Starting test bot for {}", asset);

    // 1. Create strategy wrapped in Bot
    let strategy = TestStrategy::new(asset);
    let bot = Arc::new(RwLock::new(Bot::new(strategy)));

    // 2. Create market with shared bot (Design C - no adapter needed!)
    let input = PaperTradingMarketInput::new(asset, 10_000.0);
    let mut market = match PaperTradingMarket::new(input, bot.clone()).await {
        Ok(m) => m,
        Err(e) => {
            log::error!("Failed to create market: {}", e);
            return;
        }
    };

    info!("Market created, starting HTTP server...");

    // 3. Start HTTP server - uses same bot for status!
    let port = 3001;
    let app = Router::new()
        .route("/", get(dashboard_handler))
        .route("/api/status", get(status_handler))
        .with_state(bot.clone());

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("📊 Dashboard at http://localhost:{}", port);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    // 4. Run market event loop (this blocks)
    info!("Starting market event loop...");
    market.start().await;
}

// HTTP Handlers - simple because bot is already shared
async fn dashboard_handler(State(bot): State<Arc<RwLock<Bot<TestStrategy>>>>) -> Html<String> {
    Html(bot.read().await.render_dashboard())
}

async fn status_handler(
    State(bot): State<Arc<RwLock<Bot<TestStrategy>>>>,
) -> Json<serde_json::Value> {
    Json(bot.read().await.status_json())
}
