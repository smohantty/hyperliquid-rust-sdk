use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use log::{info, warn, error};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Strategy, StrategyFactory, StrategyStatus};
use crate::market::{OrderFill, OrderRequest, OrderSide, AssetPrecision};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GridMode {
    Arithmetic,
    Geometric,
}

#[derive(Debug, Clone)]
struct GridLevel {
    price: f64,
    size: f64, // Base asset quantity for this level
    order_id: Option<u64>,
    /// The side of the active order at this level
    side: Option<OrderSide>,
}

pub struct GridStrategy {
    asset: String,
    lower_price: f64,
    upper_price: f64,
    grid_levels: usize,
    mode: GridMode,
    precision: AssetPrecision,
    
    /// User can provide either order_size (fixed base qty) OR total_investment (quote qty)
    order_size: Option<f64>,
    total_investment: Option<f64>,

    levels: Vec<GridLevel>,
    /// Map order_id -> (level_index, side)
    active_orders: HashMap<u64, (usize, OrderSide)>,
    
    initialized: bool,
    position: f64,
    realized_pnl: f64,
    trade_count: u32,
    total_fees: f64,
    
    /// Initial price used to determine buy/sell sides
    initial_price: f64,
    /// Last seen market price (for dashboard)
    last_price: f64,
    /// Flag to ensure we only place initial orders once in on_price_update
    initial_placement_done: bool,
}

impl GridStrategy {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        asset: String,
        lower_price: f64,
        upper_price: f64,
        grid_levels: usize,
        mode: GridMode,
        order_size: Option<f64>,
        total_investment: Option<f64>,
        precision: AssetPrecision,
        initial_price: f64,
    ) -> Self {
        let mut strategy = Self {
            asset,
            lower_price,
            upper_price,
            grid_levels,
            mode,
            precision,
            order_size,
            total_investment,
            levels: Vec::with_capacity(grid_levels),
            active_orders: HashMap::new(),
            initialized: false, 
            position: 0.0,
            realized_pnl: 0.0,
            trade_count: 0,
            total_fees: 0.0,
            initial_price,
            last_price: initial_price,
            initial_placement_done: false,
        };
        strategy.initialize_levels();
        strategy
    }

    fn initialize_levels(&mut self) {
        if self.grid_levels < 2 {
            warn!("Grid levels must be at least 2");
            return;
        }

        self.levels.clear();
        self.active_orders.clear();
        
        // Calculate size per level (constant base or constant quote)
        let quote_per_level = self.total_investment.map(|inv| inv / self.grid_levels as f64);
        let fixed_base_size = self.order_size;

        match self.mode {
            GridMode::Arithmetic => {
                let step = (self.upper_price - self.lower_price) / (self.grid_levels as f64 - 1.0);
                for i in 0..self.grid_levels {
                    let mut price = self.lower_price + (i as f64 * step);
                    price = self.precision.round_price(price, false);
                    
                    let raw_size = if let Some(q_val) = quote_per_level {
                        q_val / price
                    } else {
                        fixed_base_size.unwrap_or(1.0)
                    };
                    let size = self.precision.round_size(raw_size);

                    self.levels.push(GridLevel {
                        price,
                        size,
                        order_id: None,
                        side: None,
                    });
                }
            }
            GridMode::Geometric => {
                let ratio = (self.upper_price / self.lower_price).powf(1.0 / (self.grid_levels as f64 - 1.0));
                for i in 0..self.grid_levels {
                    let mut price = self.lower_price * ratio.powi(i as i32);
                    price = self.precision.round_price(price, false);
                    
                    let raw_size = if let Some(q_val) = quote_per_level {
                        q_val / price
                    } else {
                        fixed_base_size.unwrap_or(1.0)
                    };
                    let size = self.precision.round_size(raw_size);

                    self.levels.push(GridLevel {
                        price,
                        size,
                        order_id: None,
                        side: None,
                    });
                }
            }
        }
        
        info!("Initialized {} levels ({:?}) from {:.4} to {:.4}", 
            self.grid_levels, self.mode, self.lower_price, self.upper_price);
            
        // Initial setup via reconcile using initial_price
        // We set initialized=true so reconcile works, but we also just run it immediately.
        self.initialized = true;
        
        // This will set up the initial grid structure (Buy Below, Sell Above)
        // We don't return orders here (init phase), they will be picked up by first on_price_update
        // if we called it. But actually we want to populate 'side' in levels.
        // We can reuse the logic from reconcile_orders but we need to do it without generating orders yet?
        // Actually, let's just use reconcile_orders logic inline or call a helper that sets sides.
        
        // For simplicity, we just set sides manually here like before, to match "init state".
        // Or better: Use reconcile logic to set sides, but ignore order generation.
        
        let initial_price = self.initial_price;
        let mut closest_idx = 0;
        let mut min_diff = f64::MAX;
        
        for (idx, level) in self.levels.iter().enumerate() {
            let diff = (level.price - initial_price).abs();
            if diff < min_diff {
                min_diff = diff;
                closest_idx = idx;
            }
        }
        
        let closest_price = self.levels[closest_idx].price;
        
        for (idx, level) in self.levels.iter_mut().enumerate() {
            let side = if idx == closest_idx {
                None
            } else if level.price < closest_price {
                Some(OrderSide::Buy)
            } else {
                Some(OrderSide::Sell)
            };
            level.side = side;
        }

        self.log_grid_status(self.initial_price);
    }

    fn log_grid_status(&self, current_price: f64) {
        let p_dec = self.precision.price_decimals as usize;
        let s_dec = self.precision.sz_decimals as usize;
        
        info!("--- Grid Status (Current Price ~{:.*} Range) ---", p_dec, current_price);
        for (idx, level) in self.levels.iter().enumerate().rev() {
             let status = if level.order_id.is_some() {
                 if let Some(s) = level.side {
                     match s {
                        OrderSide::Buy => "BUY ",
                        OrderSide::Sell => "SELL",
                     }
                 } else { 
                     "??? " 
                 }
             } else {
                 "EMPTY"
             };
             
             let marker = if level.side.is_none() { "<<" } else { "  " };
             
             info!(
                 "Lvl {:02} | {} | {:.*} | {:.*} {}", 
                 idx, status, p_dec, level.price, s_dec, level.size, marker
             );
        }
        info!("------------------------------------------------");
    }

    fn generate_order_id() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }
    
    /// Reconciles the grid structure based on the current market price.
    /// Ensures that levels below current price are Buys, levels above are Sells,
    /// and the closest level is Empty. Places missing orders.
    fn reconcile_orders(&mut self, current_price: f64) -> Vec<OrderRequest> {
        let mut orders = vec![];
        let asset = self.asset.clone();
        
        let p_dec = self.precision.price_decimals as usize;
        let s_dec = self.precision.sz_decimals as usize;
        
        // 1. Identify where we SHOULD be
        let mut closest_idx = 0;
        let mut min_diff = f64::MAX;
        for (idx, level) in self.levels.iter().enumerate() {
            let diff = (level.price - current_price).abs();
            if diff < min_diff {
                min_diff = diff;
                closest_idx = idx;
            }
        }
        let closest_level_price = self.levels[closest_idx].price;

        // 2. Iterate all levels and correct them
        for (idx, level) in self.levels.iter_mut().enumerate().rev() {
            // Determine Ideal Side
            let ideal_side = if idx == closest_idx {
                None // Empty
            } else if level.price < closest_level_price {
                Some(OrderSide::Buy)
            } else {
                Some(OrderSide::Sell)
            };
            
            if idx == closest_idx {
                 info!("Lvl {:02} | EMPTY | {:.*} | {:.*}   <<< CURRENT PRICE GAP", idx, p_dec, level.price, s_dec, level.size);
            }
            
            // Check if matches current state
            // If we have an open order, check if it matches ideal side.
            if let Some(_current_order_id) = level.order_id {
                // We have an active order.
                // If ideal_side is None, implies we should NOT have an order here.
                // However, canceling orders in this paper market context or simplistic grid might be overkill 
                // if price is just noisy. But to fix "Interleaved" bug, we should be strict?
                // For now: TRUST EXISTING ORDERS if they are on the "Correct Side" generally.
                // But strictly: If ideal is Buy, and we have Sell... we have a problem.
                // Paper market fills usually fix this quickly.
                // We leave existing orders alone unless they are wildly wrong?
                // Actually, if we just let `active_orders` be, we only place NEW orders.
                
                // Keep strictly to: "Only place if `order_id` is None".
                // But we must update `level.side` to match ideal? 
                // No, `level.side` should match the `active_order` if present.
                // If we overwrite `level.side` but keep the old order, we get the bug.
                
                // Correct Logic:
                // If `order_id` is present, `level.side` MUST reflect that order.
                // We do NOT change `level.side` if order_id is some.
            } else {
                // No active order. We are free to set side and place order.
                if level.side != ideal_side {
                    level.side = ideal_side;
                }
                
                if let Some(side) = level.side {
                    // We need an order here
                    let order_id = Self::generate_order_id();
                    let req = if side == OrderSide::Buy {
                        OrderRequest::buy(order_id, &asset, level.size, level.price)
                    } else {
                        OrderRequest::sell(order_id, &asset, level.size, level.price)
                    };
                    
                    level.order_id = Some(order_id);
                    self.active_orders.insert(order_id, (idx, side));
                    orders.push(req);
                    
                    let side_str = if side == OrderSide::Buy { "BUY " } else { "SELL" };
                    info!("Lvl {:02} | {} | {:.*} | {:.*}   <<< PLACING ORDER (Filling Gap)", idx, side_str, p_dec, level.price, s_dec, level.size);
                }
            }
        }
        
        orders
    }
}

impl Strategy for GridStrategy {
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
        if asset != self.asset {
            return vec![];
        }
        
        self.last_price = price;
        
        // Only run reconcile on the FIRST update to place initial orders.
        // Afterwards, we only reconcile on fills (Event Driven).
        if !self.initial_placement_done {
            self.initial_placement_done = true;
            return self.reconcile_orders(price);
        }
        
        vec![]
    }

    fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
        let mut orders = vec![];
        let p_dec = self.precision.price_decimals as usize;
        let s_dec = self.precision.sz_decimals as usize;

        if let Some((level_idx, side)) = self.active_orders.remove(&fill.order_id) {
            
            // Mark level as empty (it filled)
            // But beware: We only clear IF the level's order_id matches.
            // (It should, unless we raced).
            if self.levels[level_idx].order_id == Some(fill.order_id) {
                self.levels[level_idx].order_id = None;
                self.levels[level_idx].side = None; // Temporarily None until Reconcile fixes it
            }
            
            self.trade_count += 1;
            
            // ANSI Colors for visibility
            let green = "\x1b[32m";
            let red = "\x1b[31m";
            let reset = "\x1b[0m";
            let level_price = self.levels[level_idx].price;

            if side == OrderSide::Buy {
                self.position += fill.qty;
                info!("{}Lvl {:02} | BUY  | {:.*} | {:.*}   <<< ORDER FILLED (Exec: {:.*}, Gap created){}", 
                    green, level_idx, p_dec, level_price, s_dec, fill.qty, p_dec, fill.price, reset);
            } else {
                self.position -= fill.qty;
                 // PnL tracking
                let buy_price = if level_idx > 0 { self.levels[level_idx - 1].price } else { 0.0 };
                 if buy_price > 0.0 {
                    let profit = (fill.price - buy_price) * fill.qty;
                    self.realized_pnl += profit;
                }
                info!("{}Lvl {:02} | SELL | {:.*} | {:.*}   <<< ORDER FILLED (Exec: {:.*}, Gap created){}", 
                    red, level_idx, p_dec, level_price, s_dec, fill.qty, p_dec, fill.price, reset);
            }
            
            // NEW LOGIC: Place counter-order based on filled level
            // If BUY filled at i, place SELL at i+1
            // If SELL filled at i, place BUY at i-1
            
            let next_level_idx = if side == OrderSide::Buy {
                if level_idx + 1 < self.levels.len() {
                    Some(level_idx + 1)
                } else {
                    None
                }
            } else {
                if level_idx > 0 {
                    Some(level_idx - 1)
                } else {
                    None
                }
            };

            if let Some(target_idx) = next_level_idx {
                let target_level = &mut self.levels[target_idx];
                
                // Only place if empty. If occupied, we assume the existing order is valid.
                if target_level.order_id.is_none() {
                    let target_side = if side == OrderSide::Buy { OrderSide::Sell } else { OrderSide::Buy };
                    target_level.side = Some(target_side);
                    
                    let order_id = Self::generate_order_id();
                    let req = if target_side == OrderSide::Buy {
                        OrderRequest::buy(order_id, &self.asset, target_level.size, target_level.price)
                    } else {
                        OrderRequest::sell(order_id, &self.asset, target_level.size, target_level.price)
                    };
                    
                    target_level.order_id = Some(order_id);
                    self.active_orders.insert(order_id, (target_idx, target_side));
                    orders.push(req);
                    
                    let side_str = if target_side == OrderSide::Buy { "BUY " } else { "SELL" };
                    info!("Lvl {:02} | {} | {:.*} | {:.*}   <<< PLACING ORDER (Counter)", target_idx, side_str, p_dec, target_level.price, s_dec, target_level.size);
                } else {
                    // Log that we skipped because occupied (expected in dense grid)
                    // info!("Lvl {:02} already occupied, skipping counter-order", target_idx);
                }
            }

            self.log_grid_status(fill.price);
        }
        
        orders
    }
    
    fn name(&self) -> &str {
        "spot_grid"
    }
    
    fn status(&self) -> StrategyStatus {
        // Gather and Sort Orders for API
        #[derive(serde::Serialize)]
        struct BookLevel {
            price: f64,
            size: f64,
            level_idx: usize,
            dist: f64,
        }
        
        let mut asks: Vec<BookLevel> = Vec::new();
        let mut bids: Vec<BookLevel> = Vec::new();

        for (idx, level) in self.levels.iter().enumerate() {
            if let Some(side) = level.side {
                if level.order_id.is_some() {
                    let dist_pct = (level.price - self.last_price) / self.last_price * 100.0;
                    let bl = BookLevel {
                        price: level.price,
                        size: level.size,
                        level_idx: idx,
                        dist: dist_pct,
                    };
                    if side == OrderSide::Sell {
                        asks.push(bl);
                    } else {
                        bids.push(bl);
                    }
                }
            }
        }
        
        asks.sort_by(|a, b| b.price.partial_cmp(&a.price).unwrap());
        bids.sort_by(|a, b| b.price.partial_cmp(&a.price).unwrap());

        StrategyStatus::new(self.name(), &self.asset)
            .with_status("Running")
            .with_position(self.position)
            .with_pnl(self.realized_pnl, 0.0, self.total_fees)
            .with_custom(serde_json::json!({
                "levels": self.grid_levels,
                "range": format!("{:.2} - {:.2}", self.lower_price, self.upper_price),
                "active_orders": self.active_orders.len(),
                "trades": self.trade_count,
                "book": {
                    "asks": asks,
                    "bids": bids
                }
            }))
    }

    fn render_dashboard(&self) -> Option<String> {
        let status = self.status();
        let p_dec = self.precision.price_decimals;
        let s_dec = self.precision.sz_decimals;

        Some(format!(
            r##"<!DOCTYPE html>
<html>
<head>
    <title>{name} - Grid CLOB</title>
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
    <style>
        :root {{
            --bg-primary: #12121a;
            --bg-secondary: #0a0a0f;
            --text-primary: #ffffff;
            --text-muted: #5a5a70;
            --green: #00d4aa;
            --red: #ff4d6a;
        }}
        * {{ margin: 0; padding: 0; box-sizing: border-box; }}
        body {{
            font-family: 'Inter', sans-serif;
            background: var(--bg-primary);
            color: var(--text-primary);
            padding: 20px;
        }}
        .container {{ max-width: 700px; margin: 0 auto; }}
        
        header {{ margin-bottom: 20px; display: flex; justify-content: space-between; align-items: center; }}
        h1 {{ font-size: 18px; }}
        .stats {{ font-size: 13px; color: var(--text-muted); }}
        
        .clob-header {{
            display: grid;
            grid-template-columns: 50px 1fr 1fr 1fr;
            gap: 10px;
            padding: 8px 12px;
            font-size: 12px;
            color: var(--text-muted);
            border-bottom: 1px solid #2a2a3a;
            background: var(--bg-secondary);
            position: sticky;
            top: 0;
            z-index: 10;
        }}
        .col {{ text-align: right; z-index: 1; }}
        .col.price {{ text-align: right; }} 
        .col.lvl {{ text-align: left; color: var(--text-muted); }}
        .col.dist {{ color: var(--text-muted); }}
        
        .book {{
            background: var(--bg-secondary);
            border: 1px solid #2a2a3a;
            border-radius: 8px;
            overflow: hidden;
            font-family: 'JetBrains Mono', monospace;
            font-size: 13px;
            display: flex;
            flex-direction: column;
            max-height: 80vh;
        }}

        .clob-content {{
            overflow-y: auto;
            flex: 1;
        }}

        .clob-content::-webkit-scrollbar {{ width: 8px; }}
        .clob-content::-webkit-scrollbar-track {{ background: var(--bg-secondary); }}
        .clob-content::-webkit-scrollbar-thumb {{ background: #2a2a3a; border-radius: 4px; }}
        .clob-content::-webkit-scrollbar-thumb:hover {{ background: #3a3a4a; }}
        
        .row {{
            display: grid;
            grid-template-columns: 50px 1fr 1fr 1fr;
            gap: 10px;
            padding: 4px 12px;
        }}
        .row:hover {{ background: #2a2a3a; }}
        
        .ask .price {{ color: var(--red); }}
        .bid .price {{ color: var(--green); }}
        
        .spread-row {{
            text-align: center;
            padding: 8px 0;
            color: var(--text-muted);
            font-size: 11px;
            border-top: 1px solid #2a2a3a;
            border-bottom: 1px solid #2a2a3a;
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <div>
                <h1>{name} / {asset}</h1>
                <div class="stats" id="rangeInfo">{levels} Levels | Range: {range}</div>
            </div>
            <div style="text-align: right">
                <div>PnL: <span id="pnlVal" style="color: {pnl_color}">${pnl:.2}</span></div>
                <div class="stats">Pos: <span id="posVal">{pos:.4}</span></div>
            </div>
        </header>

        <div class="book">
            <div class="clob-header">
                <div class="col lvl">Lvl</div>
                <div class="col price">Price</div>
                <div class="col dist">Dist%</div>
                <div class="col">Size</div>
            </div>
            <div class="clob-content" id="clobContent">
                <!-- Content via JS -->
                 <div style="text-align:center; padding: 20px; color: var(--text-muted)">Loading...</div>
            </div>
        </div>
    </div>
    
    <script>
        const P_DEC = {p_dec};
        const S_DEC = {s_dec};
        let firstLoad = true;

        async function updateDashboard() {{
            try {{
                const res = await fetch('/api/status');
                const data = await res.json();
                
                // Update Header Stats
                const pnl = data.pnl_quote;
                const pos = data.position;
                const pnlEl = document.getElementById('pnlVal');
                pnlEl.innerText = '$' + pnl.toFixed(2);
                pnlEl.style.color = pnl >= 0 ? 'var(--green)' : 'var(--red)';
                document.getElementById('posVal').innerText = pos.toFixed(4);
                
                // Update Book
                // We reconstruct the HTML string and replace innerHTML.
                // This is fast enough for 100 levels active.
                
                const book = data.custom.book;
                if (!book) return;

                let html = '';
                
                // Asks
                for (const ask of book.asks) {{
                    html += `<div class="row ask">
                        <div class="col lvl">${{ask.level_idx}}</div>
                        <div class="col price">${{ask.price.toFixed(P_DEC)}}</div>
                        <div class="col dist">${{ask.dist.toFixed(2)}}%</div>
                        <div class="col size">${{ask.size.toFixed(S_DEC)}}</div>
                    </div>`;
                }}

                // Spread
                if (book.asks.length > 0 && book.bids.length > 0) {{
                    const bestAsk = book.asks[book.asks.length - 1].price;
                    const bestBid = book.bids[0].price;
                    const spread = bestAsk - bestBid;
                    const spreadPct = (spread / bestAsk) * 100;
                    html += `<div class="spread-row"><span>Spread: ${{spread.toFixed(P_DEC)}} (${{spreadPct.toFixed(4)}}%)</span></div>`;
                }} else {{
                    html += `<div class="spread-row"><span>No Active Spread</span></div>`;
                }}

                // Bids
                for (const bid of book.bids) {{
                    html += `<div class="row bid">
                        <div class="col lvl">${{bid.level_idx}}</div>
                        <div class="col price">${{bid.price.toFixed(P_DEC)}}</div>
                        <div class="col dist">${{bid.dist.toFixed(2)}}%</div>
                        <div class="col size">${{bid.size.toFixed(S_DEC)}}</div>
                    </div>`;
                }}

                const container = document.getElementById('clobContent');
                
                // Check if user is scrolling (naive check: if not at top/centered, maybe don't disrupt?)
                // Actually replacing innerHTML keeps scroll position mostly intact if height doesn't drastically change.
                // But auto-centering logic should only run once.
                
                container.innerHTML = html;

                if (firstLoad) {{
                    const spreadEl = document.querySelector('.spread-row');
                    if (spreadEl) {{
                        const topPos = spreadEl.offsetTop;
                        const containerHeight = container.clientHeight;
                        const spreadHeight = spreadEl.clientHeight;
                        container.scrollTop = topPos - (containerHeight / 2) + (spreadHeight / 2);
                    }}
                    firstLoad = false;
                }}
            }} catch (e) {{
                console.error("Failed to update dashboard", e);
            }}
        }}

        // Initial Load
        updateDashboard();
        // Poll every 1s
        setInterval(updateDashboard, 1000);
    </script>
</body>
</html>"##,
            name = status.name,
            asset = status.asset,
            levels = self.grid_levels,
            range = format!("{:.2} - {:.2}", self.lower_price, self.upper_price),
            pnl_color = if status.net_profit() >= 0.0 { "var(--green)" } else { "var(--red)" },
            pnl = status.net_profit(),
            pos = status.position,
            p_dec = p_dec,
            s_dec = s_dec
        ))
    }
}

pub struct GridStrategyFactory;

impl StrategyFactory for GridStrategyFactory {
    fn create(&self, asset: &str, params: HashMap<String, Value>) -> Box<dyn Strategy + Send + Sync> {
        let lower_price = params.get("lower_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let upper_price = params.get("upper_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let grid_levels = params.get("grid_levels").and_then(|v| v.as_u64()).unwrap_or(2) as usize;
        
        let mode_str = params.get("grid_mode").and_then(|v| v.as_str()).unwrap_or("arithmetic");
        let mode = match mode_str.to_lowercase().as_str() {
            "geometric" => GridMode::Geometric,
            "arithmetic" => GridMode::Arithmetic,
            _ => {
                warn!("Unknown grid mode '{}', defaulting to arithmetic", mode_str);
                GridMode::Arithmetic
            }
        };

        // Option 1: Explicit order size
        let order_size = params.get("order_size").and_then(|v| v.as_f64());
        
        // Option 2: Total investment (Quote)
        let total_investment = params.get("total_investment").and_then(|v| v.as_f64());
        
        // Initial Price (Required for pure math setup)
        let initial_price = params.get("initial_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
        
        // Asset Precision
        let sz_decimals = params.get("sz_decimals").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let price_decimals = params.get("price_decimals").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
        let max_decimals = params.get("max_decimals").and_then(|v| v.as_u64()).unwrap_or(6) as u32;
        
        let precision = AssetPrecision {
            sz_decimals,
            price_decimals,
            max_decimals,
        };

        if lower_price <= 0.0 || upper_price <= lower_price {
            error!("Invalid grid price parameters");
        }
        
        if initial_price <= 0.0 {
            error!("Initial price must be > 0");
        }
        
        if order_size.is_none() && total_investment.is_none() {
             error!("Must specify either order_size or total_investment");
        }
        
        Box::new(GridStrategy::new(
            asset.to_string(),
            lower_price,
            upper_price,
            grid_levels,
            mode,
            order_size,
            total_investment,
            precision,
            initial_price
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::{OrderFill, OrderSide, AssetPrecision};

    fn create_test_strategy() -> GridStrategy {
        GridStrategy::new(
            "SOL-USDC".to_string(),
            100.0,
            120.0,
            3, // Levels at 100, 110, 120
            GridMode::Arithmetic,
            Some(1.0),
            None,
            AssetPrecision { sz_decimals: 2, price_decimals: 2, max_decimals: 6 },
            110.0 // Start at middle
        )
    }

    #[test]
    fn test_grid_initialization() {
        let mut strategy = create_test_strategy();
        
        // Initial reconcile 
        let orders = strategy.on_price_update("SOL-USDC", 110.0);
        
        // At 110:
        // Lvl 0 (100) < 110 -> Buy
        // Lvl 1 (110) == 110 -> Empty
        // Lvl 2 (120) > 110 -> Sell
        
        assert_eq!(orders.len(), 2);
        
        let l0 = &strategy.levels[0];
        assert!(l0.order_id.is_some());
        assert_eq!(l0.side, Some(OrderSide::Buy));
        
        let l1 = &strategy.levels[1];
        assert!(l1.order_id.is_none());
        assert!(l1.side.is_none());
        
        let l2 = &strategy.levels[2];
        assert!(l2.order_id.is_some());
        assert_eq!(l2.side, Some(OrderSide::Sell));
    }

    #[test]
    fn test_buy_fill_behavior() {
        let mut strategy = create_test_strategy();
        let _ = strategy.on_price_update("SOL-USDC", 110.0);
        
        // Verify state: L0(Buy), L1(Empty), L2(Sell)
        let l0_oid = strategy.levels[0].order_id.unwrap();
        
        // Simulate Fill at L0 (Buy)
        let fill = OrderFill {
            order_id: l0_oid,
            asset: "SOL-USDC".to_string(),
            price: 100.0,
            qty: 1.0,
        };
        
        let orders = strategy.on_order_filled(&fill);
        
        // Expectation:
        // L0 Filled (Buy). Target -> L1.
        // L1 matches current logic? L1 is correct counter-level (110).
        // Check L1 state. L1 was Empty.
        // Should place SELL at L1.
        
        assert_eq!(orders.len(), 1);
        let order = &orders[0];
        assert_eq!(order.side, OrderSide::Sell);
        assert_eq!(order.limit_price, 110.0);
        
        // Verify internal state
        assert!(strategy.levels[0].order_id.is_none()); // L0 now empty
        assert!(strategy.levels[1].order_id.is_some()); // L1 now has order
        assert_eq!(strategy.levels[1].side, Some(OrderSide::Sell));
    }

    #[test]
    fn test_sell_fill_behavior() {
        let mut strategy = create_test_strategy();
        let _ = strategy.on_price_update("SOL-USDC", 110.0);
        
        // Verify state: L0(Buy), L1(Empty), L2(Sell)
        let l2_oid = strategy.levels[2].order_id.unwrap();
        
        // Simulate Fill at L2 (Sell)
        let fill = OrderFill {
            order_id: l2_oid,
            asset: "SOL-USDC".to_string(),
            price: 120.0,
            qty: 1.0,
        };
        
        let orders = strategy.on_order_filled(&fill);
        
        // Expectation:
        // L2 Filled (Sell). Target -> L1.
        // L1 was Empty.
        // Should place BUY at L1.
        
        assert_eq!(orders.len(), 1);
        let order = &orders[0];
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.limit_price, 110.0);
        
        // Verify internal state
        assert!(strategy.levels[2].order_id.is_none()); // L2 now empty
        assert!(strategy.levels[1].order_id.is_some()); // L1 now has order
        assert_eq!(strategy.levels[1].side, Some(OrderSide::Buy));
    }
}
