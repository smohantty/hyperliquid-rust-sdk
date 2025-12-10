//! Bot - MarketListener that wraps a Strategy

use log::{debug, info};

use crate::market::{MarketListener, OrderFill, OrderRequest};
use crate::strategy::{Strategy, StrategyStatus};

/// Bot wraps a Strategy and implements MarketListener
///
/// The bot receives market events (price updates, fills), calls the strategy,
/// and returns the orders from the strategy. The market then places these orders.
///
/// # Example
///
/// ```ignore
/// use hyperliquid_rust_sdk::bot::Bot;
/// use hyperliquid_rust_sdk::market::{HyperliquidMarket, HyperliquidMarketInput};
///
/// // Create bot with strategy
/// let bot = Bot::new(MyStrategy::new());
///
/// // Pass bot as listener to market
/// let mut market = HyperliquidMarket::new(input, bot).await?;
///
/// // Market runs event loop, calls bot callbacks, places returned orders
/// market.start().await;
/// ```
pub struct Bot<S: Strategy> {
    /// The trading strategy
    strategy: S,
}

impl<S: Strategy> Bot<S> {
    /// Create a new bot wrapping the given strategy
    pub fn new(strategy: S) -> Self {
        Self { strategy }
    }

    /// Get a reference to the underlying strategy
    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    /// Get a mutable reference to the underlying strategy
    pub fn strategy_mut(&mut self) -> &mut S {
        &mut self.strategy
    }

    /// Call strategy's on_start and return initial orders
    pub fn start(&mut self) -> Vec<OrderRequest> {
        self.strategy.on_start()
    }

    /// Call strategy's on_stop and return final orders
    pub fn stop(&mut self) -> Vec<OrderRequest> {
        self.strategy.on_stop()
    }

    /// Get the strategy's current status
    ///
    /// Returns a `StrategyStatus` containing PnL, position, and other metrics.
    /// Useful for monitoring dashboards and APIs.
    pub fn status(&self) -> StrategyStatus {
        self.strategy.status()
    }

    /// Get the strategy's status as JSON
    ///
    /// Convenience method for HTTP APIs.
    pub fn status_json(&self) -> serde_json::Value {
        serde_json::to_value(self.strategy.status()).unwrap_or_default()
    }

    /// Render the strategy's dashboard
    ///
    /// Returns custom HTML from the strategy if provided,
    /// otherwise generates a default dashboard from the status.
    pub fn render_dashboard(&self) -> String {
        self.strategy
            .render_dashboard()
            .unwrap_or_else(|| render_default_dashboard(&self.strategy.status()))
    }
}

/// Render a default HTML dashboard from strategy status
///
/// This provides a clean, modern dashboard that works for any strategy.
/// Strategies can override `render_dashboard()` for custom layouts.
pub fn render_default_dashboard(status: &StrategyStatus) -> String {
    let profit_class = if status.net_profit() >= 0.0 {
        "green"
    } else {
        "red"
    };

    let status_bg = if status.status == "Running" {
        "rgba(0,212,170,0.15)"
    } else {
        "rgba(255,200,100,0.15)"
    };

    let status_dot = if status.status == "Running" {
        "#00d4aa"
    } else {
        "#ffc864"
    };

    // Format custom data as HTML if present
    let custom_html = if status.custom.is_null() {
        String::new()
    } else {
        format_custom_data(&status.custom)
    };

    format!(
        r##"<!DOCTYPE html>
<html>
<head>
    <title>{name} - Trading Bot</title>
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
        .container {{ max-width: 1200px; margin: 0 auto; padding: 24px; }}

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

        .custom-section {{
            background: var(--bg-card);
            border: 1px solid var(--border);
            border-radius: 12px;
            padding: 20px;
            margin-bottom: 24px;
        }}
        .custom-section h3 {{
            font-size: 14px;
            font-weight: 600;
            margin-bottom: 16px;
            color: var(--text-secondary);
        }}
        .custom-data {{
            font-family: 'JetBrains Mono', monospace;
            font-size: 13px;
        }}
        .custom-row {{
            display: flex;
            justify-content: space-between;
            padding: 8px 0;
            border-bottom: 1px solid var(--border);
        }}
        .custom-row:last-child {{ border-bottom: none; }}
        .custom-key {{ color: var(--text-muted); }}
        .custom-value {{ color: var(--text-primary); }}

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
                <h1>{name} · <span class="asset">{asset}</span></h1>
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
                <div class="stat-label">Position</div>
                <div class="stat-value">{position:.6}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Net Profit</div>
                <div class="stat-value {profit_class}">${net_profit:.4}</div>
                <div class="stat-sub">Realized: ${realized_pnl:.4} | Fees: ${total_fees:.4}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Unrealized PnL</div>
                <div class="stat-value">${unrealized_pnl:.4}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Trades</div>
                <div class="stat-value">{trade_count}</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Active Orders</div>
                <div class="stat-value">{active_orders}</div>
            </div>
        </div>

        {custom_html}

        <footer>
            <span>Auto-refresh: 5s</span>
            <span><a href="/api/status">JSON API</a></span>
        </footer>
    </div>
</body>
</html>"##,
        name = status.name,
        asset = status.asset,
        status = status.status,
        status_bg = status_bg,
        status_dot = status_dot,
        current_price = status.current_price,
        position = status.position,
        net_profit = status.net_profit(),
        profit_class = profit_class,
        realized_pnl = status.realized_pnl,
        unrealized_pnl = status.unrealized_pnl,
        total_fees = status.total_fees,
        trade_count = status.trade_count,
        active_orders = status.active_orders,
        custom_html = custom_html,
    )
}

/// Format custom JSON data as HTML rows
fn format_custom_data(value: &serde_json::Value) -> String {
    if let Some(obj) = value.as_object() {
        if obj.is_empty() {
            return String::new();
        }

        let rows: String = obj
            .iter()
            .map(|(k, v)| {
                let formatted_value = match v {
                    serde_json::Value::Number(n) => {
                        if let Some(f) = n.as_f64() {
                            format!("{:.4}", f)
                        } else {
                            n.to_string()
                        }
                    }
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => v.to_string(),
                };
                format!(
                    r#"<div class="custom-row"><span class="custom-key">{}</span><span class="custom-value">{}</span></div>"#,
                    k, formatted_value
                )
            })
            .collect();

        format!(
            r#"<div class="custom-section"><h3>Strategy Details</h3><div class="custom-data">{}</div></div>"#,
            rows
        )
    } else {
        String::new()
    }
}

impl<S: Strategy> MarketListener for Bot<S> {
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
        debug!("Bot[{}]: price update {} = {:.4}", self.strategy.name(), asset, price);
        let orders = self.strategy.on_price_update(asset, price);
        if !orders.is_empty() {
            info!(
                "Bot[{}]: strategy returned {} order(s) on price update",
                self.strategy.name(),
                orders.len()
            );
        }
        orders
    }

    fn on_order_filled(&mut self, fill: OrderFill) -> Vec<OrderRequest> {
        self.strategy.on_order_filled(&fill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::NoOpStrategy;

    #[test]
    fn test_bot_new() {
        let bot = Bot::new(NoOpStrategy);
        assert_eq!(bot.strategy().name(), "noop");
    }

    #[test]
    fn test_bot_noop_strategy() {
        let mut bot = Bot::new(NoOpStrategy);

        // NoOp strategy returns no orders
        let orders = bot.on_price_update("BTC", 50000.0);
        assert!(orders.is_empty());

        let fill = OrderFill::new(1, "BTC", 1.0, 50000.0);
        let orders = bot.on_order_filled(fill);
        assert!(orders.is_empty());
    }

    // Test strategy that generates orders
    struct TestStrategy {
        should_buy: bool,
        next_order_id: u64,
    }

    impl TestStrategy {
        fn new(should_buy: bool) -> Self {
            Self {
                should_buy,
                next_order_id: 0,
            }
        }
    }

    impl Strategy for TestStrategy {
        fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
            if self.should_buy {
                self.next_order_id += 1;
                vec![OrderRequest::buy(self.next_order_id, asset, 1.0, price)]
            } else {
                vec![]
            }
        }

        fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
            // After buy fills, place a sell
            self.next_order_id += 1;
            vec![OrderRequest::sell(
                self.next_order_id,
                &fill.asset,
                fill.qty,
                fill.price * 1.01,
            )]
        }

        fn on_start(&mut self) -> Vec<OrderRequest> {
            if self.should_buy {
                self.next_order_id += 1;
                vec![OrderRequest::buy(self.next_order_id, "BTC", 0.1, 50000.0)]
            } else {
                vec![]
            }
        }
    }

    #[test]
    fn test_bot_returns_orders_on_price_update() {
        let mut bot = Bot::new(TestStrategy::new(true));

        let orders = bot.on_price_update("BTC", 50000.0);

        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "BTC");
        assert_eq!(orders[0].limit_price, 50000.0);
    }

    #[test]
    fn test_bot_returns_orders_on_fill() {
        let mut bot = Bot::new(TestStrategy::new(false));

        let fill = OrderFill::new(1, "ETH", 2.0, 3000.0);
        let orders = bot.on_order_filled(fill);

        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "ETH");
        assert!((orders[0].limit_price - 3030.0).abs() < 0.01); // 1% above fill
    }

    #[test]
    fn test_bot_start() {
        let mut bot = Bot::new(TestStrategy::new(true));

        let orders = bot.start();

        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].asset, "BTC");
    }

    #[test]
    fn test_bot_strategy_access() {
        let mut bot = Bot::new(TestStrategy::new(true));

        assert!(bot.strategy().should_buy);

        bot.strategy_mut().should_buy = false;
        assert!(!bot.strategy().should_buy);
    }

    #[test]
    fn test_bot_status() {
        let bot = Bot::new(NoOpStrategy);

        let status = bot.status();
        assert_eq!(status.name, "noop");
    }

    #[test]
    fn test_bot_status_json() {
        let bot = Bot::new(NoOpStrategy);

        let json = bot.status_json();
        assert!(json.is_object());
        assert_eq!(json["name"], "noop");
    }

    #[test]
    fn test_bot_render_dashboard() {
        let bot = Bot::new(NoOpStrategy);

        let html = bot.render_dashboard();
        assert!(html.contains("noop"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    // Strategy with custom status
    struct StatusStrategy {
        position: f64,
        pnl: f64,
    }

    impl Strategy for StatusStrategy {
        fn on_price_update(&mut self, _asset: &str, _price: f64) -> Vec<OrderRequest> {
            vec![]
        }

        fn on_order_filled(&mut self, _fill: &OrderFill) -> Vec<OrderRequest> {
            vec![]
        }

        fn name(&self) -> &str {
            "status_test"
        }

        fn status(&self) -> StrategyStatus {
            StrategyStatus::new("status_test", "BTC")
                .with_status("Running")
                .with_position(self.position)
                .with_pnl(self.pnl, 0.0, 1.0)
                .with_custom(serde_json::json!({
                    "custom_field": "test_value"
                }))
        }
    }

    #[test]
    fn test_bot_custom_strategy_status() {
        let bot = Bot::new(StatusStrategy {
            position: 1.5,
            pnl: 100.0,
        });

        let status = bot.status();
        assert_eq!(status.name, "status_test");
        assert_eq!(status.asset, "BTC");
        assert_eq!(status.position, 1.5);
        assert_eq!(status.realized_pnl, 100.0);
        assert!((status.net_profit() - 99.0).abs() < 0.001);
        assert_eq!(status.custom["custom_field"], "test_value");
    }

    #[test]
    fn test_default_dashboard_renders_custom_data() {
        let status = StrategyStatus::new("test", "BTC").with_custom(serde_json::json!({
            "level_count": 10,
            "grid_spacing": 0.5
        }));

        let html = render_default_dashboard(&status);
        assert!(html.contains("Strategy Details"));
        assert!(html.contains("level_count"));
        assert!(html.contains("grid_spacing"));
    }
}
