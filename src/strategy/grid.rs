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
        for (idx, level) in self.levels.iter_mut().enumerate() {
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
            
            // Reconcile immediately using the fill price as the anchor
            let new_orders = self.reconcile_orders(fill.price);
            orders.extend(new_orders);
            
            self.log_grid_status(fill.price);
        }
        
        orders
    }
    
    fn name(&self) -> &str {
        "spot_grid"
    }
    
    fn status(&self) -> StrategyStatus {
        StrategyStatus::new(self.name(), &self.asset)
            .with_status("Running")
            .with_position(self.position)
            .with_pnl(self.realized_pnl, 0.0, self.total_fees)
            .with_custom(serde_json::json!({
                "levels": self.grid_levels,
                "range": format!("{:.2} - {:.2}", self.lower_price, self.upper_price),
                "active_orders": self.active_orders.len(),
                "trades": self.trade_count
            }))
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
