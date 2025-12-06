//! Perpetual futures-specific grid trading functionality

use log::{error, info, warn};

use super::config::{GridConfig, MarketType};
use super::errors::{GridError, GridResult};
use super::executor::GridExchange;
use super::manager::GridManager;
use super::state::StateManager;
use super::strategy::GridStrategy;
use super::types::{BotStatus, GridFill, OrderResult, RiskStatus};

/// Default max margin ratio before high risk
const DEFAULT_HIGH_RISK_MARGIN_RATIO: f64 = 0.85;

/// Perpetual futures-specific grid manager
///
/// Perp grids:
/// - Use leverage for capital efficiency
/// - Require margin/risk monitoring
/// - Can go both long and short
/// - Have funding rate considerations
pub struct PerpGridManager {
    pub(crate) inner: GridManager,
    /// Warning margin ratio threshold
    warning_margin_ratio: f64,
    /// High risk margin ratio threshold
    high_risk_margin_ratio: f64,
    /// Critical margin ratio threshold (auto shutdown)
    critical_margin_ratio: f64,
}

impl PerpGridManager {
    /// Create a new perp grid manager
    pub fn new(config: GridConfig, strategy: GridStrategy, state_manager: StateManager) -> GridResult<Self> {
        if config.market_type != MarketType::Perp {
            return Err(GridError::InvalidConfig(
                "PerpGridManager requires Perp market type".into(),
            ));
        }

        let high_risk_ratio = config.max_margin_ratio.unwrap_or(DEFAULT_HIGH_RISK_MARGIN_RATIO);

        Ok(Self {
            inner: GridManager::new(config, strategy, state_manager),
            warning_margin_ratio: high_risk_ratio * 0.8,
            high_risk_margin_ratio: high_risk_ratio,
            critical_margin_ratio: high_risk_ratio * 1.1,
        })
    }

    /// Set custom risk thresholds
    pub fn with_risk_thresholds(
        mut self,
        warning: f64,
        high_risk: f64,
        critical: f64,
    ) -> Self {
        self.warning_margin_ratio = warning;
        self.high_risk_margin_ratio = high_risk;
        self.critical_margin_ratio = critical;
        self
    }

    /// Initialize the perp grid
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

        // Set leverage if configured
        if let Some(leverage) = self.inner.config().leverage {
            info!("Setting leverage to {}x", leverage);
            exchange
                .update_leverage(self.inner.config().asset.as_str(), leverage, true)
                .await?;
        }

        // Check initial margin status
        let risk_status = self.check_risk(exchange).await?;
        if risk_status == RiskStatus::Critical {
            return Err(GridError::RiskLimitExceeded(
                "Account margin too low to start grid".into(),
            ));
        }

        info!(
            "Perp grid initialized: asset={}, price={}, range=[{}, {}], leverage={:?}",
            self.inner.config().asset,
            current_price,
            self.inner.config().lower_price,
            self.inner.config().upper_price,
            self.inner.config().leverage
        );

        Ok(())
    }

    /// Start the grid (place initial orders)
    pub async fn start<E: GridExchange>(&self, exchange: &E, current_price: f64) -> GridResult<()> {
        let status = self.inner.status().await;

        match status {
            BotStatus::Initializing => {
                // For perps, we don't need to acquire base asset
                // We can go directly to placing grid orders

                // Calculate initial position
                let init_position = self.inner.calculate_initial_position(current_price).await?;

                // Place all grid orders (without initial buy for perps)
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

    /// Handle a fill event
    pub async fn handle_fill<E: GridExchange>(
        &self,
        exchange: &E,
        fill: &GridFill,
    ) -> GridResult<Option<OrderResult>> {
        // Check risk before processing fill
        let risk_status = self.check_risk(exchange).await?;
        self.handle_risk_status(exchange, risk_status).await?;

        // Regular fill handling
        self.inner.handle_fill(exchange, fill).await
    }

    /// Check risk status
    pub async fn check_risk<E: GridExchange>(&self, exchange: &E) -> GridResult<RiskStatus> {
        let margin_info = exchange.get_margin_info().await?;
        let margin_ratio = margin_info.margin_ratio();

        let status = if margin_ratio >= self.critical_margin_ratio {
            RiskStatus::Critical
        } else if margin_ratio >= self.high_risk_margin_ratio {
            RiskStatus::HighRisk
        } else if margin_ratio >= self.warning_margin_ratio {
            RiskStatus::Warning
        } else {
            RiskStatus::Safe
        };

        if status != RiskStatus::Safe {
            warn!(
                "Risk check: margin_ratio={:.2}%, status={:?}",
                margin_ratio * 100.0,
                status
            );
        }

        Ok(status)
    }

    /// Handle risk status
    async fn handle_risk_status<E: GridExchange>(
        &self,
        exchange: &E,
        status: RiskStatus,
    ) -> GridResult<()> {
        match status {
            RiskStatus::Safe => Ok(()),
            RiskStatus::Warning => {
                warn!("Risk warning: margin usage approaching limits");
                Ok(())
            }
            RiskStatus::HighRisk => {
                warn!("High risk: margin usage at critical level");
                // Could implement position reduction here
                Ok(())
            }
            RiskStatus::Critical => {
                error!("Critical risk: initiating emergency shutdown");
                self.emergency_shutdown(exchange).await
            }
        }
    }

    /// Emergency shutdown - cancel all orders immediately
    pub async fn emergency_shutdown<E: GridExchange>(&self, exchange: &E) -> GridResult<()> {
        error!("Emergency shutdown initiated");

        self.inner.set_status(BotStatus::Stopping).await?;
        let cancelled = self.inner.cancel_all_orders(exchange).await?;

        error!("Emergency shutdown complete: cancelled {} orders", cancelled);

        self.inner.set_status(BotStatus::Stopped).await?;
        self.inner.save_state().await?;

        Err(GridError::RiskLimitExceeded(
            "Emergency shutdown due to margin limit".into(),
        ))
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
        // Check risk before resuming
        let risk_status = self.check_risk(exchange).await?;
        if risk_status == RiskStatus::Critical || risk_status == RiskStatus::HighRisk {
            return Err(GridError::RiskLimitExceeded(
                "Cannot resume: margin usage too high".into(),
            ));
        }

        let current_price = exchange.get_mid_price(self.inner.config().asset.as_str()).await?;

        // Recalculate and place orders
        let init_position = self.inner.calculate_initial_position(current_price).await?;
        self.inner.place_grid_orders(exchange, init_position.grid_orders).await?;

        self.inner.set_status(BotStatus::Running).await?;
        Ok(())
    }

    /// Get current position
    pub async fn get_position<E: GridExchange>(
        &self,
        exchange: &E,
    ) -> GridResult<Option<super::types::Position>> {
        exchange.get_position(self.inner.config().asset.as_str()).await
    }

    /// Get margin info
    pub async fn get_margin_info<E: GridExchange>(
        &self,
        exchange: &E,
    ) -> GridResult<super::types::MarginInfo> {
        exchange.get_margin_info().await
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

    async fn create_test_perp_manager() -> (PerpGridManager, MockExchange) {
        // $4500 total investment, 10 grids, BTC price range 40000-50000
        let config = GridConfig::new("BTC", 40000.0, 50000.0, 10, 4500.0, MarketType::Perp)
            .with_leverage(5)
            .with_max_margin_ratio(0.8);

        let strategy = GridStrategy::arithmetic();
        let precision = AssetPrecision::for_perp(4);
        let levels = strategy.calculate_grid_levels(&config, &precision);
        let state_manager = StateManager::load_or_create(&config, levels).unwrap();

        let mut manager = PerpGridManager::new(config, strategy, state_manager).unwrap();
        manager.inner.precision = Some(precision);

        let exchange = MockExchange::new(45000.0);

        // Set up healthy margin
        *exchange.margin_info.lock().await = MarginInfo {
            account_value: 10000.0,
            margin_used: 2000.0,
            available_margin: 8000.0,
            withdrawable: 8000.0,
        };

        (manager, exchange)
    }

    #[tokio::test]
    async fn test_perp_grid_initialization() {
        let (mut manager, exchange) = create_test_perp_manager().await;

        manager.initialize(&exchange).await.unwrap();

        let summary = manager.get_state_summary().await;
        assert_eq!(summary.num_levels, 11);
    }

    #[tokio::test]
    async fn test_perp_grid_start() {
        let (mut manager, exchange) = create_test_perp_manager().await;

        manager.initialize(&exchange).await.unwrap();
        manager.inner.set_status(BotStatus::Initializing).await.unwrap();

        manager.start(&exchange, 45000.0).await.unwrap();

        let summary = manager.get_state_summary().await;
        assert_eq!(summary.status, BotStatus::Running);
    }

    #[tokio::test]
    async fn test_risk_check_safe() {
        let (manager, exchange) = create_test_perp_manager().await;

        let status = manager.check_risk(&exchange).await.unwrap();
        assert_eq!(status, RiskStatus::Safe);
    }

    #[tokio::test]
    async fn test_risk_check_critical() {
        let (manager, exchange) = create_test_perp_manager().await;

        // Set critical margin
        *exchange.margin_info.lock().await = MarginInfo {
            account_value: 10000.0,
            margin_used: 9800.0, // 98% used
            available_margin: 200.0,
            withdrawable: 200.0,
        };

        let status = manager.check_risk(&exchange).await.unwrap();
        assert_eq!(status, RiskStatus::Critical);
    }

    #[tokio::test]
    async fn test_perp_grid_stop() {
        let (mut manager, exchange) = create_test_perp_manager().await;

        manager.initialize(&exchange).await.unwrap();
        manager.inner.set_status(BotStatus::Running).await.unwrap();

        manager.stop(&exchange).await.unwrap();

        let summary = manager.get_state_summary().await;
        assert_eq!(summary.status, BotStatus::Stopped);
    }
}
