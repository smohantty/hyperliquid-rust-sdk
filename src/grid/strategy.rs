//! Grid strategy - handles grid level calculation and fill processing

use serde::{Deserialize, Serialize};

use super::config::{AssetPrecision, GridConfig};
use super::types::{GridFill, GridLevel, GridOrderRequest, LevelStatus, OrderSide};

/// Type of grid spacing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GridType {
    /// Uniform price spacing: each level is separated by the same dollar amount
    /// Good for stable assets with predictable price ranges
    #[default]
    Arithmetic,
    /// Percentage-based spacing: each level is separated by the same percentage
    /// Good for volatile assets where percentage moves matter more than absolute moves
    Geometric,
}

/// Result of calculating initial position requirements
#[derive(Debug, Clone)]
pub struct InitialPosition {
    /// Number of sell orders above current price
    pub num_sell_levels: u32,
    /// Amount of base asset needed for initial sells
    pub base_amount_needed: f64,
    /// Initial buy order to acquire base asset (if needed)
    pub initial_buy_order: Option<GridOrderRequest>,
    /// All grid orders to place after initialization
    pub grid_orders: Vec<GridOrderRequest>,
}

/// Result of handling a fill
#[derive(Debug, Clone)]
pub struct FillResult {
    /// Replacement order to place (if any)
    pub replacement_order: Option<GridOrderRequest>,
    /// Profit from this fill (if completing a round trip)
    pub profit: Option<f64>,
    /// Fee from this fill
    pub fee: f64,
    /// Whether this completed a round trip
    pub round_trip_complete: bool,
}

/// Grid strategy - calculates levels and handles fills
#[derive(Debug, Clone, Copy, Default)]
pub struct GridStrategy {
    pub grid_type: GridType,
}

impl GridStrategy {
    /// Create a new arithmetic (uniform spacing) grid strategy
    pub fn arithmetic() -> Self {
        Self {
            grid_type: GridType::Arithmetic,
        }
    }

    /// Create a new geometric (percentage spacing) grid strategy
    pub fn geometric() -> Self {
        Self {
            grid_type: GridType::Geometric,
        }
    }

    /// Calculate all grid price levels based on grid type
    pub fn calculate_grid_levels(&self, config: &GridConfig, precision: &AssetPrecision) -> Vec<GridLevel> {
        let num_levels = config.num_levels();

        (0..num_levels)
            .map(|i| {
                let raw_price = match self.grid_type {
                    GridType::Arithmetic => {
                        // Uniform spacing: lower + step * i
                        let price_step = config.price_step();
                        config.lower_price + price_step * i as f64
                    }
                    GridType::Geometric => {
                        // Percentage spacing: lower * ratio^i
                        let ratio = (config.upper_price / config.lower_price)
                            .powf(1.0 / config.num_grids as f64);
                        config.lower_price * ratio.powi(i as i32)
                    }
                };

                let price = precision.round_price(raw_price, false);
                GridLevel::new(i, price, OrderSide::Buy)
            })
            .collect()
    }

    /// Determine initial position requirements
    pub fn calculate_initial_position(
        &self,
        config: &GridConfig,
        precision: &AssetPrecision,
        current_price: f64,
        levels: &[GridLevel],
    ) -> InitialPosition {
        // Find the level closest to current price
        let current_level_idx = levels
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let diff_a = (a.price - current_price).abs();
                let diff_b = (b.price - current_price).abs();
                diff_a.partial_cmp(&diff_b).unwrap()
            })
            .map(|(i, _)| i)
            .unwrap_or(0);

        // Count sell levels (above current price)
        let num_sell_levels = levels
            .iter()
            .filter(|l| l.price > current_price)
            .count() as u32;

        // Calculate base amount needed for sell orders
        // Each sell level needs: usd_per_grid / level_price
        let base_amount_needed: f64 = levels
            .iter()
            .filter(|l| l.price > current_price)
            .map(|l| config.calculate_order_size_at_price(l.price, precision))
            .sum();

        // Create grid orders with per-level sizing
        let mut grid_orders = Vec::new();

        for (idx, level) in levels.iter().enumerate() {
            let side = if level.price < current_price {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            };

            // Skip the level closest to current price (it's the "filled" level)
            if idx == current_level_idx {
                continue;
            }

            // Calculate order size for this specific price level
            // Same USD value at each level = different base asset amounts
            let order_size = config.calculate_order_size_at_price(level.price, precision);

            grid_orders.push(GridOrderRequest::new(
                level.index,
                level.price,
                order_size,
                side,
            ));
        }

        // Initial buy order to acquire base for sells (if needed)
        let initial_buy_order = if base_amount_needed > 0.0 {
            // Buy slightly above current price to ensure fill
            let buy_price = precision.round_price(current_price * 1.001, true);
            Some(GridOrderRequest::new(
                u32::MAX, // Special marker for init buy
                buy_price,
                base_amount_needed,
                OrderSide::Buy,
            ))
        } else {
            None
        };

        InitialPosition {
            num_sell_levels,
            base_amount_needed,
            initial_buy_order,
            grid_orders,
        }
    }

    /// Handle a fill event - returns replacement order and profit info
    pub fn handle_fill(
        &self,
        fill: &GridFill,
        level_index: u32,
        levels: &mut [GridLevel],
        config: &GridConfig,
        precision: &AssetPrecision,
    ) -> FillResult {
        let num_levels = levels.len();

        // Check if level exists
        if level_index as usize >= num_levels {
            return FillResult {
                replacement_order: None,
                profit: None,
                fee: fill.fee,
                round_trip_complete: false,
            };
        }

        // Determine the adjacent level index for replacement order
        let adjacent_idx = match fill.side {
            OrderSide::Buy => {
                // Bought at this level, place sell at next level up
                if level_index < num_levels as u32 - 1 {
                    Some(level_index + 1)
                } else {
                    None
                }
            }
            OrderSide::Sell => {
                // Sold at this level, place buy at next level down
                if level_index > 0 {
                    Some(level_index - 1)
                } else {
                    None
                }
            }
        };

        // Get adjacent level price for replacement order
        let replacement_order = adjacent_idx.and_then(|adj_idx| {
            levels.get(adj_idx as usize).map(|adj_level| {
                let new_side = fill.side.opposite();
                let order_size = config.calculate_order_size_at_price(adj_level.price, precision);
                GridOrderRequest::new(adj_idx, adj_level.price, order_size, new_side)
            })
        });

        // Update the filled level
        let filled_level = &mut levels[level_index as usize];
        filled_level.mark_filled(fill.price);
        filled_level.intended_side = fill.side.opposite();
        filled_level.status = LevelStatus::Empty;

        FillResult {
            replacement_order,
            profit: None,
            fee: fill.fee,
            round_trip_complete: false,
        }
    }

    /// Calculate profit from a round-trip trade
    pub fn calculate_profit(&self, buy_price: f64, sell_price: f64, size: f64, fee: f64) -> f64 {
        let gross = (sell_price - buy_price) * size;
        gross - fee
    }

    /// Determine which side an order should be based on current price
    pub fn determine_order_side(&self, level_price: f64, current_price: f64) -> OrderSide {
        if level_price < current_price {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::config::MarketType;

    #[test]
    fn test_arithmetic_grid_levels() {
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1500.0, MarketType::Spot);
        let precision = AssetPrecision::for_spot(4);
        let strategy = GridStrategy::arithmetic();

        let levels = strategy.calculate_grid_levels(&config, &precision);

        assert_eq!(levels.len(), 11); // 10 grids = 11 levels
        assert!((levels[0].price - 100.0).abs() < 0.01);
        assert!((levels[10].price - 200.0).abs() < 0.01);

        // Check spacing is uniform (same dollar amount between levels)
        let step = levels[1].price - levels[0].price;
        for i in 1..levels.len() {
            let actual_step = levels[i].price - levels[i - 1].price;
            assert!((actual_step - step).abs() < 0.1);
        }
    }

    #[test]
    fn test_geometric_grid_levels() {
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1500.0, MarketType::Spot);
        let precision = AssetPrecision::for_spot(4);
        let strategy = GridStrategy::geometric();

        let levels = strategy.calculate_grid_levels(&config, &precision);

        assert_eq!(levels.len(), 11);
        assert!((levels[0].price - 100.0).abs() < 0.01);
        assert!((levels[10].price - 200.0).abs() < 1.0);

        // Check percentage spacing is uniform (same ratio between levels)
        let ratio = levels[1].price / levels[0].price;
        for i in 1..levels.len() - 1 {
            let actual_ratio = levels[i + 1].price / levels[i].price;
            assert!((actual_ratio - ratio).abs() < 0.01);
        }
    }

    #[test]
    fn test_initial_position_calculation() {
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1500.0, MarketType::Spot);
        let precision = AssetPrecision::for_spot(4);
        let strategy = GridStrategy::arithmetic();

        let levels = strategy.calculate_grid_levels(&config, &precision);
        let current_price = 150.0;
        let init = strategy.calculate_initial_position(&config, &precision, current_price, &levels);

        // Should have ~5 sell levels above 150
        assert!(init.num_sell_levels >= 4 && init.num_sell_levels <= 6);

        // Should have an initial buy order
        assert!(init.initial_buy_order.is_some());

        // Grid orders should not include the level at current price
        assert_eq!(init.grid_orders.len(), 10); // 11 levels - 1 skipped

        // Lower price = more coins (larger order size)
        let buy_orders: Vec<_> = init.grid_orders.iter().filter(|o| o.side == OrderSide::Buy).collect();
        if buy_orders.len() >= 2 {
            let low_price_order = buy_orders.iter().min_by(|a, b| a.price.partial_cmp(&b.price).unwrap()).unwrap();
            let high_price_order = buy_orders.iter().max_by(|a, b| a.price.partial_cmp(&b.price).unwrap()).unwrap();
            assert!(low_price_order.size > high_price_order.size);
        }
    }

    #[test]
    fn test_determine_order_side() {
        let strategy = GridStrategy::arithmetic();

        assert_eq!(strategy.determine_order_side(100.0, 150.0), OrderSide::Buy);
        assert_eq!(strategy.determine_order_side(200.0, 150.0), OrderSide::Sell);
    }

    #[test]
    fn test_calculate_profit() {
        let strategy = GridStrategy::arithmetic();

        let profit = strategy.calculate_profit(100.0, 110.0, 1.0, 0.5);
        assert!((profit - 9.5).abs() < 0.01); // (110-100)*1 - 0.5 = 9.5
    }
}
