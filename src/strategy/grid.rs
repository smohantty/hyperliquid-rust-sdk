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
    /// Map order_id -> level_index
    active_orders: HashMap<u64, usize>,
    
    initialized: bool,
    position: f64,
    realized_pnl: f64,
    trade_count: u32,
    total_fees: f64,
    
    /// Initial price used to determine buy/sell sides
    initial_price: f64,
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
            initialized: false, // Will be set to true immediately in initialize_levels
            position: 0.0,
            realized_pnl: 0.0,
            trade_count: 0,
            total_fees: 0.0,
            initial_price,
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
            
        // Initial Order Placement Logic (Pure Math)
        // Determine sides based on initial_price
        let initial_price = self.initial_price;
        let _asset = self.asset.clone();
        
        // Define a small epsilon for "at the money" check, e.g., 0.1% of price or related to tick
        let epsilon = initial_price * 0.0005; 
        
        for (_idx, level) in self.levels.iter_mut().enumerate() {
            let side = if (level.price - initial_price).abs() < epsilon {
                // Price is "on" this level (Empty Level)
                None
            } else if level.price < initial_price {
                Some(OrderSide::Buy)
            } else {
                Some(OrderSide::Sell)
            };

            level.side = side;
        }
        
        // We mark as initialized so on_price_update proceeds to check for needed orders
        self.initialized = true;
        self.log_grid_status();
    }

    fn log_grid_status(&self) {
        info!("--- Grid Status (Current Price ~{:.2} Range) ---", self.levels.iter().filter(|l| l.side.is_none()).next().map(|l| l.price).unwrap_or(0.0));
        for (idx, level) in self.levels.iter().enumerate() {
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
                 "Lvl {:02} | {} | {:.4} | {:.4} {}", 
                 idx, status, level.price, level.size, marker
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
}

impl Strategy for GridStrategy {
    fn on_price_update(&mut self, asset: &str, _price: f64) -> Vec<OrderRequest> {
        if asset != self.asset {
            return vec![];
        }

        let mut orders = vec![];

        // "Pure Math" strategy:
        // We traverse levels. If a level has a side but no order_id, we place it.
        // The side was determined at initialization (or updated after fills).
        
        for (idx, level) in self.levels.iter_mut().enumerate() {
            if level.order_id.is_none() {
                if let Some(side) = level.side {
                    let order_id = Self::generate_order_id();
                    let req = if side == OrderSide::Buy {
                        OrderRequest::buy(order_id, asset, level.size, level.price)
                    } else {
                        OrderRequest::sell(order_id, asset, level.size, level.price)
                    };
                    
                    level.order_id = Some(order_id);
                    self.active_orders.insert(order_id, idx);
                    orders.push(req);
                }
            }
        }
        
        orders
    }

    fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
        let mut orders = vec![];

        if let Some(level_idx) = self.active_orders.remove(&fill.order_id) {
            let filled_side = self.levels[level_idx].side;
            
            // 1. The Current Level becomes "Empty"
            // As per Binance Spot Grid: when a level fills, it enters a waiting state.
            // It waits for the price to oscillate back.
            self.levels[level_idx].order_id = None;
            self.levels[level_idx].side = None; 
            
            self.trade_count += 1;
            
            let asset = self.asset.clone();

            if let Some(side) = filled_side {
                if side == OrderSide::Buy {
                    self.position += fill.qty;
                    info!("Grid Buy filled at level {} (Qty: {:.4} @ {:.4}). Level {} is now Empty.", level_idx, fill.qty, fill.price, level_idx);
                    
                    // 2. The Level Above (Opposite) becomes Active (Sell)
                    // If we bought at X, we want to sell at X+1.
                    if level_idx + 1 < self.levels.len() {
                        let target_idx = level_idx + 1;
                        let target_level = &mut self.levels[target_idx];
                        
                        // We set the target level to Sell.
                        // If it was already a Sell (e.g. from init), we just ensure it's active.
                        // If it was Empty (because price was there previously), it now becomes filled with a Sell.
                        if target_level.side != Some(OrderSide::Sell) || target_level.order_id.is_none() {
                             target_level.side = Some(OrderSide::Sell);
                             
                             // Place order immediately if not present
                            if target_level.order_id.is_none() {
                                let order_id = Self::generate_order_id();
                                let req = OrderRequest::sell(order_id, &asset, target_level.size, target_level.price);
                                target_level.order_id = Some(order_id);
                                self.active_orders.insert(order_id, target_idx);
                                orders.push(req);
                                info!("Placed paired Sell at level {} ({:.4})", target_idx, target_level.price);
                            }
                        }
                    }

                } else {
                    self.position -= fill.qty;
                    
                    // PnL tracking
                    let buy_price = if level_idx > 0 { self.levels[level_idx - 1].price } else { 0.0 };
                     if buy_price > 0.0 {
                        let profit = (fill.price - buy_price) * fill.qty;
                        self.realized_pnl += profit;
                    }

                    info!("Grid Sell filled at level {} (Qty: {:.4} @ {:.4}). Level {} is now Empty.", level_idx, fill.qty, fill.price, level_idx);
                    
                    // 2. The Level Below (Opposite) becomes Active (Buy)
                    // If we sold at X, we want to buy back at X-1.
                    if level_idx > 0 {
                        let target_idx = level_idx - 1;
                        let target_level = &mut self.levels[target_idx];
                        
                        // We set the target level to Buy.
                        // If it was Empty (price was there), it now becomes filled with a Buy.
                        if target_level.side != Some(OrderSide::Buy) || target_level.order_id.is_none() {
                             target_level.side = Some(OrderSide::Buy);
                             
                             if target_level.order_id.is_none() {
                                let order_id = Self::generate_order_id();
                                let req = OrderRequest::buy(order_id, &asset, target_level.size, target_level.price);
                                target_level.order_id = Some(order_id);
                                self.active_orders.insert(order_id, target_idx);
                                orders.push(req);
                                info!("Placed paired Buy at level {} ({:.4})", target_idx, target_level.price);
                             }
                        }
                    }
                }
            }
        }
        
        // Detailed grid status logging as requested
        self.log_grid_status();
        
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
