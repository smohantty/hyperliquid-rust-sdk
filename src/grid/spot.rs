//! Spot-specific grid trading functionality

use log::{info, warn};

use super::config::{GridConfig, InitialPositionMethod, MarketType};
use super::errors::{GridError, GridResult};
use super::executor::GridExchange;
use super::manager::GridManager;
use super::state::StateManager;
use super::strategy::GridStrategy;
use super::types::{BotStatus, GridFill, OrderResult};

/// Spot-specific grid manager
///
/// Spot grids trade actual assets and require:
/// - Base asset for sell orders
/// - Quote asset (USDC) for buy orders
pub struct SpotGridManager {
    pub(crate) inner: GridManager,
}

impl SpotGridManager {
    /// Create a new spot grid manager
    pub fn new(config: GridConfig, strategy: GridStrategy, state_manager: StateManager) -> GridResult<Self> {
        if config.market_type != MarketType::Spot {
            return Err(GridError::InvalidConfig(
                "SpotGridManager requires Spot market type".into(),
            ));
        }

        Ok(Self {
            inner: GridManager::new(config, strategy, state_manager),
        })
    }

    /// Initialize the spot grid
    pub async fn initialize<E: GridExchange>(&mut self, exchange: &E) -> GridResult<()> {
        // Fetch asset precision from exchange meta
        self.inner.fetch_precision(exchange).await?;

        // Initialize grid levels
        self.inner.initialize_levels().await?;

        // Get current price
        let current_price = exchange.get_mid_price(self.inner.config().asset.as_str()).await?;

        if !self.inner.is_price_in_range(current_price) {
            return Err(GridError::PriceOutOfRange {
                price: current_price,
                lower: self.inner.config().lower_price,
                upper: self.inner.config().upper_price,
            });
        }

        // Update level sides based on current price
        self.inner.update_level_sides(current_price).await?;

        info!(
            "Spot grid initialized: asset={}, price={}, range=[{}, {}]",
            self.inner.config().asset,
            current_price,
            self.inner.config().lower_price,
            self.inner.config().upper_price
        );

        Ok(())
    }

    /// Start the grid (place initial orders)
    pub async fn start<E: GridExchange>(&self, exchange: &E, current_price: f64) -> GridResult<()> {
        let status = self.inner.status().await;

        match status {
            BotStatus::Initializing => {
                // Calculate initial position
                let init_position = self.inner.calculate_initial_position(current_price).await?;

                // Handle initial base acquisition based on config
                match self.inner.config().initial_position_method {
                    InitialPositionMethod::LimitBuy => {
                        if let Some(init_order) = init_position.initial_buy_order {
                            info!("Placing initial buy order for {} base asset", init_position.base_amount_needed);
                            self.inner.place_initial_buy(exchange, &init_order).await?;
                            // Stay in Initializing until fill received
                            return Ok(());
                        }
                    }
                    InitialPositionMethod::MarketBuy => {
                        if let Some(mut init_order) = init_position.initial_buy_order {
                            // For market buy, use IOC and higher price for slippage
                            let precision = self.inner.precision()?;
                            init_order.price = precision.round_price(current_price * 1.01, true);
                            info!("Placing market buy order for {} base asset", init_position.base_amount_needed);
                            self.inner.place_initial_buy(exchange, &init_order).await?;
                        }
                    }
                    InitialPositionMethod::Skip => {
                        info!("Skipping initial position acquisition");
                    }
                }

                // Place all grid orders
                self.inner.place_grid_orders(exchange, init_position.grid_orders).await?;

                // Set status to running
                self.inner.set_status(BotStatus::Running).await?;
            }
            BotStatus::WaitingForEntry => {
                // Check if trigger met
                if self.inner.check_trigger(current_price).await {
                    self.inner.set_status(BotStatus::Initializing).await?;
                    return Box::pin(self.start(exchange, current_price)).await;
                }
            }
            _ => {
                warn!("Cannot start grid in status {:?}", status);
            }
        }

        Ok(())
    }

    /// Handle initial buy fill and place grid orders
    pub async fn handle_init_fill<E: GridExchange>(
        &self,
        exchange: &E,
        fill: &GridFill,
    ) -> GridResult<()> {
        // Handle the fill (will mark init_position_acquired = true)
        self.inner.handle_fill(exchange, fill).await?;

        // Check if we should place grid orders now
        let should_place_grid = {
            let state = self.inner.state_manager.read().await;
            state.init_position_acquired && state.status == BotStatus::Initializing
        };

        if should_place_grid {
            let current_price = exchange.get_mid_price(self.inner.config().asset.as_str()).await?;
            let init_position = self.inner.calculate_initial_position(current_price).await?;

            self.inner.place_grid_orders(exchange, init_position.grid_orders).await?;
            self.inner.set_status(BotStatus::Running).await?;

            info!("Grid orders placed, bot is now running");
        }

        Ok(())
    }

    /// Handle a fill event
    pub async fn handle_fill<E: GridExchange>(
        &self,
        exchange: &E,
        fill: &GridFill,
    ) -> GridResult<Option<OrderResult>> {
        // Check if this is init buy fill
        {
            let state = self.inner.state_manager.read().await;
            if let Some(init_oid) = state.init_buy_oid {
                if fill.oid == init_oid {
                    drop(state);
                    self.handle_init_fill(exchange, fill).await?;
                    return Ok(None);
                }
            }
        }

        // Regular fill handling
        self.inner.handle_fill(exchange, fill).await
    }

    /// Stop the grid
    pub async fn stop<E: GridExchange>(&self, exchange: &E) -> GridResult<()> {
        self.inner.set_status(BotStatus::Stopping).await?;

        // Cancel all orders
        let cancelled = self.inner.cancel_all_orders(exchange).await?;
        info!("Cancelled {} orders during shutdown", cancelled);

        self.inner.set_status(BotStatus::Stopped).await?;
        self.inner.save_state().await?;

        Ok(())
    }

    /// Pause the grid (cancel orders but keep state)
    pub async fn pause<E: GridExchange>(&self, exchange: &E) -> GridResult<()> {
        self.inner.set_status(BotStatus::Paused).await?;
        self.inner.cancel_all_orders(exchange).await?;
        Ok(())
    }

    /// Resume the grid
    pub async fn resume<E: GridExchange>(&self, exchange: &E) -> GridResult<()> {
        let current_price = exchange.get_mid_price(self.inner.config().asset.as_str()).await?;

        // Recalculate and place orders
        let init_position = self.inner.calculate_initial_position(current_price).await?;
        self.inner.place_grid_orders(exchange, init_position.grid_orders).await?;

        self.inner.set_status(BotStatus::Running).await?;
        Ok(())
    }

    /// Get the inner manager for direct access
    pub fn inner(&self) -> &GridManager {
        &self.inner
    }

    /// Get current status
    pub async fn status(&self) -> BotStatus {
        self.inner.status().await
    }

    /// Get profit summary
    pub async fn get_profit(&self) -> super::types::GridProfit {
        self.inner.get_profit().await
    }

    /// Get state summary
    pub async fn get_state_summary(&self) -> super::manager::GridStateSummary {
        self.inner.get_state_summary().await
    }

    /// Save state
    pub async fn save_state(&self) -> GridResult<()> {
        self.inner.save_state().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::executor::mock::MockExchange;

    async fn create_test_spot_manager() -> (SpotGridManager, MockExchange) {
        // $1500 total investment, 10 grids, price range 100-200
        let config = GridConfig::new("PURR/USDC", 100.0, 200.0, 10, 1500.0, MarketType::Spot)
            .with_initial_position_method(InitialPositionMethod::Skip);

        let strategy = GridStrategy::arithmetic();
        let precision = AssetPrecision::for_spot(2);
        let levels = strategy.calculate_grid_levels(&config, &precision);
        let state_manager = StateManager::load_or_create(&config, levels).unwrap();

        let mut manager = SpotGridManager::new(config, strategy, state_manager).unwrap();
        manager.inner.precision = Some(precision);

        let exchange = MockExchange::new(150.0);

        (manager, exchange)
    }

    #[tokio::test]
    async fn test_spot_grid_initialization() {
        let (mut manager, exchange) = create_test_spot_manager().await;

        manager.initialize(&exchange).await.unwrap();

        let summary = manager.get_state_summary().await;
        assert_eq!(summary.num_levels, 11);
    }

    #[tokio::test]
    async fn test_spot_grid_start() {
        let (mut manager, exchange) = create_test_spot_manager().await;

        manager.initialize(&exchange).await.unwrap();
        manager.inner.set_status(BotStatus::Initializing).await.unwrap();

        manager.start(&exchange, 150.0).await.unwrap();

        let summary = manager.get_state_summary().await;
        assert_eq!(summary.status, BotStatus::Running);
        assert!(summary.active_buys > 0 || summary.active_sells > 0);
    }

    #[tokio::test]
    async fn test_spot_grid_stop() {
        let (mut manager, exchange) = create_test_spot_manager().await;

        manager.initialize(&exchange).await.unwrap();
        manager.inner.set_status(BotStatus::Running).await.unwrap();

        manager.stop(&exchange).await.unwrap();

        let summary = manager.get_state_summary().await;
        assert_eq!(summary.status, BotStatus::Stopped);
    }
}
