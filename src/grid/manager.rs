//! Core grid manager - shared logic for spot and perp grids

use log::{debug, error, info, warn};

use super::config::{AssetPrecision, GridConfig};
use super::errors::{GridError, GridResult};
use super::executor::GridExchange;
use super::state::StateManager;
use super::strategy::{GridStrategy, InitialPosition};
use super::types::{
    BotStatus, GridFill, GridOrderRequest, LevelStatus, OrderResult, OrderResultStatus,
    OrderSide,
};

/// Core grid manager - handles grid logic independent of market type
pub struct GridManager {
    config: GridConfig,
    strategy: GridStrategy,
    pub(crate) state_manager: StateManager,
    /// Asset precision fetched from exchange meta
    pub(crate) precision: Option<AssetPrecision>,
}

impl GridManager {
    /// Create a new grid manager
    pub fn new(config: GridConfig, strategy: GridStrategy, state_manager: StateManager) -> Self {
        Self {
            config,
            strategy,
            state_manager,
            precision: None,
        }
    }

    /// Fetch and store asset precision from exchange
    pub async fn fetch_precision<E: GridExchange>(&mut self, exchange: &E) -> GridResult<AssetPrecision> {
        let precision = exchange
            .get_asset_precision(&self.config.asset, self.config.market_type)
            .await?;

        info!(
            "Fetched asset precision for {}: sz_decimals={}, price_decimals={}",
            self.config.asset, precision.sz_decimals, precision.price_decimals
        );

        self.precision = Some(precision);
        Ok(precision)
    }

    /// Get stored precision or return error
    pub fn precision(&self) -> GridResult<&AssetPrecision> {
        self.precision
            .as_ref()
            .ok_or_else(|| GridError::Initialization("Asset precision not fetched".into()))
    }

    /// Initialize grid levels using the strategy
    pub async fn initialize_levels(&self) -> GridResult<()> {
        let precision = self.precision()?;
        let levels = self.strategy.calculate_grid_levels(&self.config, precision);

        self.state_manager
            .update(|state| {
                state.levels = levels;
            })
            .await?;

        info!(
            "Initialized {} grid levels from {} to {}",
            self.config.num_levels(),
            self.config.lower_price,
            self.config.upper_price
        );

        Ok(())
    }

    /// Get current bot status
    pub async fn status(&self) -> BotStatus {
        self.state_manager.read().await.status
    }

    /// Set bot status
    pub async fn set_status(&self, status: BotStatus) -> GridResult<()> {
        self.state_manager
            .update(|state| {
                state.status = status;
            })
            .await?;

        info!("Bot status changed to {:?}", status);
        Ok(())
    }

    /// Check if price is within grid range
    pub fn is_price_in_range(&self, price: f64) -> bool {
        price >= self.config.lower_price && price <= self.config.upper_price
    }

    /// Check if trigger condition is met
    pub async fn check_trigger(&self, current_price: f64) -> bool {
        if let Some(trigger_price) = self.config.trigger_price {
            current_price <= trigger_price
        } else {
            true // No trigger = always ready
        }
    }

    /// Calculate initial position requirements
    pub async fn calculate_initial_position(&self, current_price: f64) -> GridResult<InitialPosition> {
        let state = self.state_manager.read().await;
        let precision = self.precision()?;

        if state.levels.is_empty() {
            return Err(GridError::Initialization("Grid levels not initialized".into()));
        }

        let init = self
            .strategy
            .calculate_initial_position(&self.config, precision, current_price, &state.levels);

        info!(
            "Initial position: {} sell levels, {} base needed",
            init.num_sell_levels, init.base_amount_needed
        );

        Ok(init)
    }

    /// Place an order and update state
    pub async fn place_order<E: GridExchange>(
        &self,
        exchange: &E,
        order: &GridOrderRequest,
    ) -> GridResult<OrderResult> {
        debug!(
            "Placing {} order at level {} price {} size {}",
            if order.side == OrderSide::Buy { "BUY" } else { "SELL" },
            order.level_index,
            order.price,
            order.size
        );

        // Place order on exchange
        let result = exchange.place_order(&self.config.asset, order).await?;

        // Update state with OID
        self.state_manager
            .update(|state| {
                // Register the order mapping (OID -> level)
                state.register_order(order.level_index, result.oid);

                // Update level status
                if let Some(level) = state.get_level_mut(order.level_index) {
                    level.oid = Some(result.oid);
                    level.intended_side = order.side;

                    match &result.status {
                        OrderResultStatus::Resting => {
                            level.status = LevelStatus::Active;
                        }
                        OrderResultStatus::Filled { .. } => {
                            level.status = LevelStatus::Filled;
                        }
                        OrderResultStatus::WaitingForTrigger => {
                            level.status = LevelStatus::Pending;
                        }
                        OrderResultStatus::Rejected(_) => {
                            level.status = LevelStatus::Empty;
                        }
                    }
                }
            })
            .await?;

        info!(
            "Order placed: level={}, oid={}, status={:?}",
            order.level_index, result.oid, result.status
        );

        Ok(result)
    }

    /// Place initial buy order for acquiring base asset
    pub async fn place_initial_buy<E: GridExchange>(
        &self,
        exchange: &E,
        order: &GridOrderRequest,
    ) -> GridResult<OrderResult> {
        info!(
            "Placing initial buy order: price={}, size={}",
            order.price, order.size
        );

        let result = exchange.place_order(&self.config.asset, order).await?;

        // Store the init buy OID
        self.state_manager
            .update(|state| {
                state.init_buy_oid = Some(result.oid);
            })
            .await?;

        Ok(result)
    }

    /// Place all grid orders
    pub async fn place_grid_orders<E: GridExchange>(
        &self,
        exchange: &E,
        orders: Vec<GridOrderRequest>,
    ) -> GridResult<Vec<OrderResult>> {
        let mut results = Vec::with_capacity(orders.len());

        for order in orders {
            match self.place_order(exchange, &order).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    error!("Failed to place order at level {}: {}", order.level_index, e);
                    // Continue with other orders
                }
            }
        }

        info!("Placed {}/{} grid orders", results.len(), results.capacity());
        Ok(results)
    }

    /// Handle a fill event
    pub async fn handle_fill<E: GridExchange>(
        &self,
        exchange: &E,
        fill: &GridFill,
    ) -> GridResult<Option<OrderResult>> {
        let precision = self.precision()?;

        // Check if this is the initial buy order
        {
            let state = self.state_manager.read().await;
            if let Some(init_oid) = state.init_buy_oid {
                if fill.oid == init_oid {
                    info!("Initial buy order filled: size={}, price={}", fill.size, fill.price);
                    drop(state);

                    self.state_manager
                        .update(|state| {
                            state.init_buy_oid = None;
                            state.init_position_acquired = true;
                        })
                        .await?;

                    return Ok(None);
                }
            }
        }

        // Find the level for this fill by OID
        let level_index = {
            let state = self.state_manager.read().await;
            state.find_level_index_by_oid(fill.oid)
        };

        let level_index = match level_index {
            Some(idx) => idx,
            None => {
                warn!("Received fill for unknown order: oid={}", fill.oid);
                return Ok(None);
            }
        };

        info!(
            "Fill received: level={}, side={:?}, price={}, size={}",
            level_index, fill.side, fill.price, fill.size
        );

        // Process the fill using strategy
        let fill_result = {
            let mut state = self.state_manager.write().await;

            let result = self.strategy.handle_fill(fill, level_index, &mut state.levels, &self.config, precision);

            // Unregister the filled order
            state.unregister_order(fill.oid);

            // Update profit tracking
            if let Some(profit) = result.profit {
                state.profit.add_trade(profit, result.fee, fill.size * fill.price);
                if result.round_trip_complete {
                    state.profit.complete_round_trip();
                }
            }

            // Update position tracking
            match fill.side {
                OrderSide::Buy => state.current_position += fill.size,
                OrderSide::Sell => state.current_position -= fill.size,
            }

            // Update last mid price
            state.last_mid_price = fill.price;

            result
        };

        // Force save state after fill
        self.state_manager.force_save().await?;

        // Place replacement order if needed
        if let Some(replacement) = fill_result.replacement_order {
            let result = self.place_order(exchange, &replacement).await?;
            return Ok(Some(result));
        }

        Ok(None)
    }

    /// Cancel all active orders
    pub async fn cancel_all_orders<E: GridExchange>(&self, exchange: &E) -> GridResult<u32> {
        let count = exchange.cancel_all_orders(&self.config.asset).await?;

        // Reset all levels to empty
        self.state_manager
            .update(|state| {
                for level in &mut state.levels {
                    if level.has_active_order() {
                        level.reset();
                    }
                }
                state.oid_to_level.clear();
            })
            .await?;

        info!("Cancelled {} orders", count);
        Ok(count)
    }

    /// Cancel a specific order by level
    pub async fn cancel_order<E: GridExchange>(
        &self,
        exchange: &E,
        level_index: u32,
    ) -> GridResult<bool> {
        let oid = {
            let state = self.state_manager.read().await;
            let level = state.get_level(level_index).ok_or(GridError::LevelNotFound(level_index))?;
            level.oid
        };

        let cancelled = if let Some(oid) = oid {
            exchange.cancel_order(&self.config.asset, oid).await?
        } else {
            false
        };

        if cancelled {
            self.state_manager
                .update(|state| {
                    let level_oid = if let Some(level) = state.get_level(level_index) {
                        level.oid
                    } else {
                        return;
                    };

                    if let Some(oid) = level_oid {
                        state.unregister_order(oid);
                    }

                    if let Some(level) = state.get_level_mut(level_index) {
                        level.reset();
                    }
                })
                .await?;
        }

        Ok(cancelled)
    }

    /// Get profit summary
    pub async fn get_profit(&self) -> super::types::GridProfit {
        self.state_manager.read().await.profit.clone()
    }

    /// Get current grid state summary
    pub async fn get_state_summary(&self) -> GridStateSummary {
        let state = self.state_manager.read().await;

        GridStateSummary {
            status: state.status,
            num_levels: state.levels.len(),
            active_buys: state.count_active_buys(),
            active_sells: state.count_active_sells(),
            current_position: state.current_position,
            last_mid_price: state.last_mid_price,
            realized_pnl: state.profit.realized_pnl,
            total_fees: state.profit.total_fees,
            round_trips: state.profit.num_round_trips,
        }
    }

    /// Update price levels based on current price (for side assignment)
    pub async fn update_level_sides(&self, current_price: f64) -> GridResult<()> {
        self.state_manager
            .update(|state| {
                for level in &mut state.levels {
                    if level.status == LevelStatus::Empty {
                        level.intended_side = self.strategy.determine_order_side(level.price, current_price);
                    }
                }
            })
            .await?;
        Ok(())
    }

    /// Force save state
    pub async fn save_state(&self) -> GridResult<()> {
        self.state_manager.force_save().await
    }

    /// Get the config
    pub fn config(&self) -> &GridConfig {
        &self.config
    }
}

/// Summary of grid state
#[derive(Debug, Clone)]
pub struct GridStateSummary {
    pub status: BotStatus,
    pub num_levels: usize,
    pub active_buys: usize,
    pub active_sells: usize,
    pub current_position: f64,
    pub last_mid_price: f64,
    pub realized_pnl: f64,
    pub total_fees: f64,
    pub round_trips: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::config::MarketType;
    use crate::grid::executor::mock::MockExchange;

    async fn create_test_manager() -> (GridManager, MockExchange) {
        // $1500 total investment, 10 grids, price range 100-200
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1500.0, MarketType::Spot);

        let strategy = GridStrategy::arithmetic();
        let precision = AssetPrecision::for_spot(4);
        let levels = strategy.calculate_grid_levels(&config, &precision);
        let state_manager = StateManager::load_or_create(&config, levels).unwrap();

        let mut manager = GridManager::new(config, strategy, state_manager);
        manager.precision = Some(precision);

        let exchange = MockExchange::new(150.0);

        (manager, exchange)
    }

    #[tokio::test]
    async fn test_initialize_levels() {
        let (manager, _) = create_test_manager().await;
        manager.initialize_levels().await.unwrap();

        let summary = manager.get_state_summary().await;
        assert_eq!(summary.num_levels, 11);
    }

    #[tokio::test]
    async fn test_place_order() {
        let (manager, exchange) = create_test_manager().await;
        manager.initialize_levels().await.unwrap();

        let order = GridOrderRequest::new(0, 100.0, 0.1, OrderSide::Buy);
        let result = manager.place_order(&exchange, &order).await.unwrap();

        assert!(result.oid > 0);
        assert!(matches!(result.status, OrderResultStatus::Resting));

        let orders = exchange.orders.lock().await;
        assert_eq!(orders.len(), 1);
    }

    #[tokio::test]
    async fn test_cancel_all_orders() {
        let (manager, exchange) = create_test_manager().await;
        manager.initialize_levels().await.unwrap();

        let order1 = GridOrderRequest::new(0, 100.0, 0.1, OrderSide::Buy);
        let order2 = GridOrderRequest::new(10, 200.0, 0.1, OrderSide::Sell);

        manager.place_order(&exchange, &order1).await.unwrap();
        manager.place_order(&exchange, &order2).await.unwrap();

        let cancelled = manager.cancel_all_orders(&exchange).await.unwrap();
        assert_eq!(cancelled, 2);
    }

    #[tokio::test]
    async fn test_is_price_in_range() {
        let (manager, _) = create_test_manager().await;

        assert!(manager.is_price_in_range(150.0));
        assert!(manager.is_price_in_range(100.0));
        assert!(manager.is_price_in_range(200.0));
        assert!(!manager.is_price_in_range(99.0));
        assert!(!manager.is_price_in_range(201.0));
    }
}
