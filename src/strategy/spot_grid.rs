use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Strategy, StrategyFactory, StrategyStatus};
use crate::market::{AssetPrecision, OrderFill, OrderRequest, OrderSide};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    pub price: f64,
    pub size: f64,
    pub side: OrderSide,
    pub time: u64, // Unix timestamp in seconds
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GridMode {
    Arithmetic,
    Geometric,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ZoneState {
    WaitingBuy,  // Order placed at lower_price
    WaitingSell, // Order placed at upper_price
}

#[derive(Debug, Clone)]
struct GridZone {
    index: usize,
    lower_price: f64,
    upper_price: f64,
    size: f64, // Base asset quantity for this zone

    state: ZoneState,
    /// Price of the last fill that set the current state.
    /// Tracks cost basis for PnL calculation.
    entry_price: f64,

    /// Accumulated PnL for this specific zone
    total_pnl: f64,
    /// Number of completed roundtrips for this zone
    roundtrip_count: u32,

    /// The Active Order ID for this zone
    order_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundTrip {
    pub entry_time: u64, // Approximate (time of prev fill not tracked currently, using current time for simplicity or needs field)
    pub exit_time: u64,
    pub entry_price: f64,
    pub exit_price: f64,
    pub side: String, // "Long" or "Short"
    pub size: f64,
    pub pnl: f64,
    pub entry_lvl: usize,
    pub exit_lvl: usize,
}

pub struct SpotGridStrategy {
    asset: String,
    lower_price: f64,
    upper_price: f64,
    grid_levels: usize, // Number of "lines". Zones = grid_levels - 1
    mode: GridMode,
    precision: AssetPrecision,

    /// User can provide either order_size (fixed base qty) OR total_investment (quote qty)
    order_size: Option<f64>,
    total_investment: Option<f64>,

    zones: Vec<GridZone>,
    /// Map order_id -> zone_index
    active_orders: HashMap<u64, usize>,

    initialized: bool,
    position: f64,
    realized_pnl: f64,
    trade_count: u32,
    total_fees: f64,

    /// Recent trades for dashboard
    recent_trades: VecDeque<TradeRecord>,

    completed_roundtrips: VecDeque<RoundTrip>,

    /// Initial price used to determine buy/sell sides
    initial_price: f64,
    /// Last seen market price (for dashboard)
    last_price: f64,
}

impl SpotGridStrategy {
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
            zones: Vec::new(),
            active_orders: HashMap::new(),
            initialized: false,
            position: 0.0,
            realized_pnl: 0.0,
            trade_count: 0,
            total_fees: 0.0,
            recent_trades: VecDeque::with_capacity(50),
            completed_roundtrips: VecDeque::with_capacity(50),
            initial_price,
            last_price: initial_price,
        };
        strategy.initialize_zones();
        strategy
    }

    fn initialize_zones(&mut self) {
        if self.grid_levels < 2 {
            warn!("Grid levels must be at least 2 (to form 1 zone)");
            return;
        }

        self.zones.clear();
        self.active_orders.clear();

        // Generate Price Lines first
        let mut prices = Vec::with_capacity(self.grid_levels);
        match self.mode {
            GridMode::Arithmetic => {
                let step = (self.upper_price - self.lower_price) / (self.grid_levels as f64 - 1.0);
                for i in 0..self.grid_levels {
                    let mut price = self.lower_price + (i as f64 * step);
                    price = self.precision.round_price(price, false);
                    prices.push(price);
                }
            }
            GridMode::Geometric => {
                let ratio = (self.upper_price / self.lower_price)
                    .powf(1.0 / (self.grid_levels as f64 - 1.0));
                for i in 0..self.grid_levels {
                    let mut price = self.lower_price * ratio.powi(i as i32);
                    price = self.precision.round_price(price, false);
                    prices.push(price);
                }
            }
        }

        // Create Zones from adjacent prices
        let num_zones = self.grid_levels - 1;

        let quote_per_zone = self.total_investment.map(|inv| inv / num_zones as f64);
        let fixed_base_size = self.order_size;

        for i in 0..num_zones {
            let lower = prices[i];
            let upper = prices[i + 1];

            let raw_size = if let Some(q_val) = quote_per_zone {
                q_val / lower
            } else {
                fixed_base_size.unwrap_or(1.0)
            };
            let size = self.precision.round_size(raw_size);

            // Determine Initial State
            // - If InitialPrice < Upper: We assume we hold inventory (or are below zone). We want to Sell at Upper.
            // - If InitialPrice >= Upper: We are sold out. We want to Buy at Lower.

            let initial_state = if self.initial_price < upper {
                ZoneState::WaitingSell
            } else {
                ZoneState::WaitingBuy
            };

            // Initial Entry Price Logic:
            let entry_price = if initial_state == ZoneState::WaitingSell {
                self.initial_price
            } else {
                0.0
            };

            // Adjust position tracking
            if initial_state == ZoneState::WaitingSell {
                self.position += size;
            }

            self.zones.push(GridZone {
                index: i,
                lower_price: lower,
                upper_price: upper,
                size,
                state: initial_state,
                entry_price,
                total_pnl: 0.0,
                roundtrip_count: 0,
                order_id: None,
            });
        }

        info!("Initialized {} zones", self.zones.len());
        self.initialized = true;
    }

    fn generate_order_id() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }

    /// Place orders for all zones based on their current state.
    /// Used during initial setup.
    fn refresh_orders(&mut self) -> Vec<OrderRequest> {
        let mut orders = vec![];
        let asset = self.asset.clone();

        for i in 0..self.zones.len() {
            let zone = &mut self.zones[i];

            if zone.order_id.is_none() {
                let order_id = Self::generate_order_id();

                let (price, side) = match zone.state {
                    ZoneState::WaitingBuy => (zone.lower_price, OrderSide::Buy),
                    ZoneState::WaitingSell => (zone.upper_price, OrderSide::Sell),
                };

                let req = if side == OrderSide::Buy {
                    OrderRequest::buy(order_id, &asset, zone.size, price)
                } else {
                    OrderRequest::sell(order_id, &asset, zone.size, price)
                };

                zone.order_id = Some(order_id);
                self.active_orders.insert(order_id, i);
                orders.push(req);
            }
        }

        orders
    }
}

impl Strategy for SpotGridStrategy {
    fn on_price_update(&mut self, asset: &str, price: f64) -> Vec<OrderRequest> {
        if asset != self.asset {
            return vec![];
        }

        self.last_price = price;

        // Initial Placement
        if self.initialized && self.active_orders.is_empty() && self.trade_count == 0 {
            return self.refresh_orders();
        }

        vec![]
    }

    fn on_order_filled(&mut self, fill: &OrderFill) -> Vec<OrderRequest> {
        let mut orders = vec![];
        let p_dec = self.precision.price_decimals as usize;
        let s_dec = self.precision.sz_decimals as usize;

        if let Some(zone_idx) = self.active_orders.remove(&fill.order_id) {
            let zone = &mut self.zones[zone_idx];

            if zone.order_id != Some(fill.order_id) {
                warn!("Fill Order ID mismatch for zone {}", zone_idx);
                return vec![];
            }

            zone.order_id = None;
            self.trade_count += 1;

            let green = "\x1b[32m";
            let red = "\x1b[31m";
            let reset = "\x1b[0m";

            // Determine filled side based on previous state
            let side_filled = match zone.state {
                ZoneState::WaitingBuy => OrderSide::Buy,
                ZoneState::WaitingSell => OrderSide::Sell,
            };

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let current_trade = TradeRecord {
                price: fill.price,
                size: fill.qty,
                side: side_filled,
                time: now,
            };
            self.recent_trades.push_front(current_trade.clone());
            if self.recent_trades.len() > 50 {
                self.recent_trades.pop_back();
            }

            // TOGGLE STATE & CALCULATE PNL
            match side_filled {
                OrderSide::Buy => {
                    self.position += fill.qty;
                    info!(
                        "{}Zone {:02} | BUY  | {:.*} | {:.*}   <<< BOUGHT @ Lower{}",
                        green, zone_idx, p_dec, fill.price, s_dec, fill.qty, reset
                    );

                    // For Spot Grid, a Buy is opening/refilling inventory.
                    // We simply set the entry_price for the subsequent Sell.
                    // We do NOT count Sell->Buy as a profit cycle (Short PnL) in this mode.

                    // Update entry_price to this Buy Price (Cost Basis)
                    zone.entry_price = fill.price;
                    zone.state = ZoneState::WaitingSell;
                }
                OrderSide::Sell => {
                    self.position -= fill.qty;
                    info!(
                        "{}Zone {:02} | SELL | {:.*} | {:.*}   <<< SOLD @ Upper{}",
                        red, zone_idx, p_dec, fill.price, s_dec, fill.qty, reset
                    );

                    // If we were WaitingSell, we "Closed a Long".
                    if zone.entry_price > 0.0 {
                        let pnl = (fill.price - zone.entry_price) * fill.qty;
                        self.realized_pnl += pnl;

                        // Increment Zone Stats
                        zone.total_pnl += pnl;
                        zone.roundtrip_count += 1;

                        let rt = RoundTrip {
                            entry_time: 0, // Not tracked
                            exit_time: now,
                            entry_price: zone.entry_price,
                            exit_price: fill.price,
                            side: "Long".to_string(),
                            size: fill.qty,
                            pnl,
                            entry_lvl: zone_idx,
                            exit_lvl: zone_idx,
                        };
                        self.completed_roundtrips.push_front(rt);
                    }

                    // Update entry_price to this Sell Price (Opening Short)
                    zone.entry_price = fill.price;
                    zone.state = ZoneState::WaitingBuy;
                }
            }

            // PLACE NEW ORDER FOR THIS ZONE
            let (target_price, target_side) = match zone.state {
                ZoneState::WaitingBuy => (zone.lower_price, OrderSide::Buy),
                ZoneState::WaitingSell => (zone.upper_price, OrderSide::Sell),
            };

            let order_id = Self::generate_order_id();
            let req = if target_side == OrderSide::Buy {
                OrderRequest::buy(order_id, &self.asset, zone.size, target_price)
            } else {
                OrderRequest::sell(order_id, &self.asset, zone.size, target_price)
            };

            zone.order_id = Some(order_id);
            self.active_orders.insert(order_id, zone_idx);
            orders.push(req);
        }

        orders
    }

    fn name(&self) -> &str {
        "spot_grid"
    }

    fn status(&self) -> StrategyStatus {
        let mut asks = Vec::new();
        let mut bids = Vec::new();

        let mut unmatched_pnl = 0.0;
        let mut invested_value = 0.0;
        let mut active_grids = 0;

        for zone in &self.zones {
            let side = match zone.state {
                ZoneState::WaitingBuy => OrderSide::Buy,
                ZoneState::WaitingSell => OrderSide::Sell,
            };

            // Calculate Stats
            match zone.state {
                ZoneState::WaitingSell => {
                    // We hold inventory.
                    // Unmatched PnL = (Current Price - Entry Price) * Size
                    if self.last_price > 0.0 && zone.entry_price > 0.0 {
                        unmatched_pnl += (self.last_price - zone.entry_price) * zone.size;
                    }
                    // Invested: Value of held token at entry
                    if zone.entry_price > 0.0 {
                        invested_value += zone.entry_price * zone.size;
                    } else {
                        // Fallback if entry not set (shouldn't happen for active holding)
                        invested_value += zone.lower_price * zone.size;
                    }
                }
                ZoneState::WaitingBuy => {
                    // We have open Buy order. Invested = Capital reserved.
                    invested_value += zone.lower_price * zone.size;
                }
            }
            if zone.order_id.is_some() {
                active_grids += 1;
            }

            let price = match zone.state {
                ZoneState::WaitingBuy => zone.lower_price,
                ZoneState::WaitingSell => zone.upper_price,
            };

            let dist = if self.last_price > 0.0 {
                (price - self.last_price).abs() / self.last_price * 100.0
            } else {
                0.0
            };

            let item = json!({
                "level_idx": zone.index,
                "price": price,
                "size": zone.size,
                "dist": dist,
                "side": side,
                "has_order": zone.order_id.is_some(),
                "total_pnl": zone.total_pnl,
                "roundtrip_count": zone.roundtrip_count
            });

            match side {
                OrderSide::Buy => bids.push(item),
                OrderSide::Sell => asks.push(item),
            }
        }

        asks.sort_by(|a, b| {
            let p_a = a["price"].as_f64().unwrap_or(0.0);
            let p_b = b["price"].as_f64().unwrap_or(0.0);
            p_b.partial_cmp(&p_a).unwrap() // Descending
        });
        bids.sort_by(|a, b| {
            let p_a = a["price"].as_f64().unwrap_or(0.0);
            let p_b = b["price"].as_f64().unwrap_or(0.0);
            p_b.partial_cmp(&p_a).unwrap() // Descending
        });

        let mut custom = serde_json::Map::new();

        custom.insert("levels".to_string(), json!(self.grid_levels));
        custom.insert("lower_price".to_string(), json!(self.lower_price));
        custom.insert("upper_price".to_string(), json!(self.upper_price));
        custom.insert("current_price".to_string(), json!(self.last_price));
        custom.insert(
            "grid_type".to_string(),
            json!(match self.mode {
                GridMode::Arithmetic => "Arithmetic",
                GridMode::Geometric => "Geometric",
            }),
        );

        custom.insert("unmatched_pnl".to_string(), json!(unmatched_pnl));
        custom.insert("invested_value".to_string(), json!(invested_value));
        custom.insert("active_grids".to_string(), json!(active_grids));
        // Avg Qty (Take first zone as approx)
        let qty_order = if !self.zones.is_empty() {
            self.zones[0].size
        } else {
            0.0
        };
        custom.insert("qty_order".to_string(), json!(qty_order));

        let total_roundtrips: u32 = self.zones.iter().map(|z| z.roundtrip_count).sum();
        custom.insert("total_roundtrips".to_string(), json!(total_roundtrips));

        custom.insert(
            "book".to_string(),
            json!({
                "asks": asks,
                "bids": bids
            }),
        );

        if let Ok(trades) = serde_json::to_value(&self.recent_trades) {
            custom.insert("recent_trades".to_string(), trades);
        }

        if let Ok(rt) = serde_json::to_value(&self.completed_roundtrips) {
            custom.insert("roundtrips".to_string(), rt);
        }

        if let Ok(prec) = serde_json::to_value(&self.precision) {
            custom.insert("asset_precision".to_string(), prec);
        }

        StrategyStatus::new("spot_grid", &self.asset)
            .with_status("Running")
            .with_position(self.position)
            .with_pnl(self.realized_pnl, 0.0, self.total_fees)
            .with_custom(serde_json::Value::Object(custom))
    }
}

pub struct SpotGridStrategyFactory;

impl StrategyFactory for SpotGridStrategyFactory {
    fn create(
        &self,
        asset: &str,
        params: HashMap<String, Value>,
    ) -> Box<dyn Strategy + Send + Sync> {
        let lower_price = params
            .get("lower_price")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let upper_price = params
            .get("upper_price")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let grid_levels = params
            .get("grid_levels")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;

        let mode_str = params
            .get("grid_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("arithmetic");
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
        let initial_price = params
            .get("initial_price")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        // Asset Precision
        let sz_decimals = params
            .get("sz_decimals")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let price_decimals = params
            .get("price_decimals")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as u32;
        let max_decimals = params
            .get("max_decimals")
            .and_then(|v| v.as_u64())
            .unwrap_or(6) as u32;

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

        Box::new(SpotGridStrategy::new(
            asset.to_string(),
            lower_price,
            upper_price,
            grid_levels,
            mode,
            order_size,
            total_investment,
            precision,
            initial_price,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::AssetPrecision;

    fn create_test_strategy() -> SpotGridStrategy {
        SpotGridStrategy::new(
            "SOL-USDC".to_string(),
            100.0,
            120.0,
            3, // Levels (Lines): 100, 110, 120. Zones: (100-110), (110-120).
            GridMode::Arithmetic,
            Some(1.0),
            None,
            AssetPrecision {
                sz_decimals: 2,
                price_decimals: 2,
                max_decimals: 6,
            },
            110.0, // Init at 110 (Middle)
        )
    }

    #[test]
    fn test_grid_initialization() {
        let mut strategy = create_test_strategy();

        // Check Zones
        assert_eq!(strategy.zones.len(), 2);

        // Zone 0: 100-110. Init Price 110.
        // 110 < 110 is False.
        // So Not < Upper? Wait. 110 is NOT < 110.
        // Logic: if initial < upper { WaitingSell } else { WaitingBuy }.
        // 110 < 110 is False.
        // So WaitingBuy.
        // Correct.
        let z0 = &strategy.zones[0];
        assert_eq!(z0.lower_price, 100.0);
        assert_eq!(z0.upper_price, 110.0);
        assert_eq!(z0.state, ZoneState::WaitingBuy);
        assert_eq!(z0.entry_price, 0.0);
        assert_eq!(z0.total_pnl, 0.0);
        assert_eq!(z0.roundtrip_count, 0);

        // Zone 1: 110-120. Init Price 110.
        // 110 < 120 is True.
        // So WaitingSell.
        let z1 = &strategy.zones[1];
        assert_eq!(z1.lower_price, 110.0);
        assert_eq!(z1.upper_price, 120.0);
        assert_eq!(z1.state, ZoneState::WaitingSell);
        assert_eq!(z1.entry_price, 110.0);
        assert_eq!(z1.total_pnl, 0.0);
        assert_eq!(z1.roundtrip_count, 0);

        // Trigger Orders
        let orders = strategy.on_price_update("SOL-USDC", 110.0);
        assert_eq!(orders.len(), 2);
    }
}
