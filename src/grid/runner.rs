//! Grid bot runner - main execution loop

use std::sync::Arc;
use std::time::Duration;

use log::{debug, error, info, warn};
use tokio::time::interval;

use super::config::{GridConfig, MarketType};
use super::errors::{GridError, GridResult};
use super::executor::{FillFeed, GridExchange, PriceFeed};
use super::perp::PerpGridManager;
use super::spot::SpotGridManager;
use super::state::StateManager;
use super::strategy::GridStrategy;
use super::types::{BotStatus, GridFill};

/// Grid bot runner configuration
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub risk_check_interval_secs: u64,
    pub state_save_interval_secs: u64,
    pub auto_restart: bool,
    pub max_consecutive_errors: u32,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            risk_check_interval_secs: 30,
            state_save_interval_secs: 30,
            auto_restart: true,
            max_consecutive_errors: 5,
        }
    }
}

/// Grid bot runner for spot markets
pub struct SpotGridRunner<E: GridExchange, P: PriceFeed, F: FillFeed> {
    manager: SpotGridManager,
    exchange: Arc<E>,
    price_feed: P,
    fill_feed: F,
    runner_config: RunnerConfig,
}

impl<E: GridExchange + 'static, P: PriceFeed + 'static, F: FillFeed + 'static> SpotGridRunner<E, P, F> {
    pub fn new(
        config: GridConfig,
        strategy: GridStrategy,
        exchange: E,
        price_feed: P,
        fill_feed: F,
        runner_config: RunnerConfig,
    ) -> GridResult<Self> {
        let state_manager = StateManager::load_or_create(&config, Vec::new())?;
        let manager = SpotGridManager::new(config, strategy, state_manager)?;
        Ok(Self { manager, exchange: Arc::new(exchange), price_feed, fill_feed, runner_config })
    }

    pub async fn run(&mut self) -> GridResult<()> {
        info!("Starting spot grid bot");
        self.manager.initialize(self.exchange.as_ref()).await?;
        let mut price_rx = self.price_feed.subscribe(self.manager.inner().config().asset.as_str()).await?;
        let mut fill_rx = self.fill_feed.subscribe().await?;
        let mut save_timer = interval(Duration::from_secs(self.runner_config.state_save_interval_secs));
        let mut consecutive_errors = 0u32;

        loop {
            tokio::select! {
                Some(price) = price_rx.recv() => {
                    match self.handle_price(price).await {
                        Ok(_) => consecutive_errors = 0,
                        Err(e) => { error!("Error handling price: {}", e); consecutive_errors += 1; }
                    }
                }
                Some(fill) = fill_rx.recv() => {
                    match self.handle_fill(fill).await {
                        Ok(_) => consecutive_errors = 0,
                        Err(e) => { error!("Error handling fill: {}", e); consecutive_errors += 1; }
                    }
                }
                _ = save_timer.tick() => {
                    if let Err(e) = self.manager.save_state().await { warn!("Failed to save state: {}", e); }
                }
            }
            if self.manager.status().await == BotStatus::Stopped { info!("Bot stopped"); break; }
            if consecutive_errors >= self.runner_config.max_consecutive_errors {
                error!("Too many errors, shutting down");
                self.manager.stop(self.exchange.as_ref()).await?;
                return Err(GridError::Exchange("Too many errors".into()));
            }
        }
        self.price_feed.unsubscribe().await?;
        self.fill_feed.unsubscribe().await?;
        Ok(())
    }

    async fn handle_price(&mut self, price: f64) -> GridResult<()> {
        match self.manager.status().await {
            BotStatus::WaitingForEntry => {
                if self.manager.inner().check_trigger(price).await {
                    info!("Trigger hit at {}", price);
                    self.manager.inner().set_status(BotStatus::Initializing).await?;
                    self.manager.start(self.exchange.as_ref(), price).await?;
                }
            }
            BotStatus::Initializing => debug!("Initializing at price {}", price),
            BotStatus::Running => {
                if !self.manager.inner().is_price_in_range(price) { warn!("Price {} out of range", price); }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_fill(&mut self, fill: GridFill) -> GridResult<()> {
        let status = self.manager.status().await;
        if !status.is_active() && status != BotStatus::Initializing { return Ok(()); }
        self.manager.handle_fill(self.exchange.as_ref(), &fill).await?;
        let s = self.manager.get_state_summary().await;
        info!("Fill: pos={}, pnl={:.2}, trips={}", s.current_position, s.realized_pnl, s.round_trips);
        Ok(())
    }

    pub async fn stop(&mut self) -> GridResult<()> { self.manager.stop(self.exchange.as_ref()).await }
    pub async fn get_state_summary(&self) -> super::manager::GridStateSummary { self.manager.get_state_summary().await }
}

/// Grid bot runner for perpetual futures
pub struct PerpGridRunner<E: GridExchange, P: PriceFeed, F: FillFeed> {
    manager: PerpGridManager,
    exchange: Arc<E>,
    price_feed: P,
    fill_feed: F,
    runner_config: RunnerConfig,
}

impl<E: GridExchange + 'static, P: PriceFeed + 'static, F: FillFeed + 'static> PerpGridRunner<E, P, F> {
    pub fn new(
        config: GridConfig,
        strategy: GridStrategy,
        exchange: E,
        price_feed: P,
        fill_feed: F,
        runner_config: RunnerConfig,
    ) -> GridResult<Self> {
        let state_manager = StateManager::load_or_create(&config, Vec::new())?;
        let manager = PerpGridManager::new(config, strategy, state_manager)?;
        Ok(Self { manager, exchange: Arc::new(exchange), price_feed, fill_feed, runner_config })
    }

    pub async fn run(&mut self) -> GridResult<()> {
        info!("Starting perp grid bot");
        self.manager.initialize(self.exchange.as_ref()).await?;
        let mut price_rx = self.price_feed.subscribe(self.manager.inner().config().asset.as_str()).await?;
        let mut fill_rx = self.fill_feed.subscribe().await?;
        let mut risk_timer = interval(Duration::from_secs(self.runner_config.risk_check_interval_secs));
        let mut save_timer = interval(Duration::from_secs(self.runner_config.state_save_interval_secs));
        let mut consecutive_errors = 0u32;

        loop {
            tokio::select! {
                Some(price) = price_rx.recv() => {
                    match self.handle_price(price).await {
                        Ok(_) => consecutive_errors = 0,
                        Err(e) => { error!("Error handling price: {}", e); consecutive_errors += 1; }
                    }
                }
                Some(fill) = fill_rx.recv() => {
                    match self.handle_fill(fill).await {
                        Ok(_) => consecutive_errors = 0,
                        Err(e) => { error!("Error handling fill: {}", e); consecutive_errors += 1; }
                    }
                }
                _ = risk_timer.tick() => {
                    if let Err(e) = self.check_risk().await { error!("Risk check failed: {}", e); break; }
                }
                _ = save_timer.tick() => {
                    if let Err(e) = self.manager.save_state().await { warn!("Failed to save: {}", e); }
                }
            }
            if self.manager.status().await == BotStatus::Stopped { info!("Bot stopped"); break; }
            if consecutive_errors >= self.runner_config.max_consecutive_errors {
                error!("Too many errors, shutting down");
                self.manager.stop(self.exchange.as_ref()).await?;
                return Err(GridError::Exchange("Too many errors".into()));
            }
        }
        self.price_feed.unsubscribe().await?;
        self.fill_feed.unsubscribe().await?;
        Ok(())
    }

    async fn handle_price(&mut self, price: f64) -> GridResult<()> {
        match self.manager.status().await {
            BotStatus::WaitingForEntry => {
                if self.manager.inner().check_trigger(price).await {
                    self.manager.inner().set_status(BotStatus::Initializing).await?;
                    self.manager.start(self.exchange.as_ref(), price).await?;
                }
            }
            BotStatus::Initializing => debug!("Initializing at {}", price),
            BotStatus::Running => {
                if !self.manager.inner().is_price_in_range(price) { warn!("Price {} out of range", price); }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_fill(&mut self, fill: GridFill) -> GridResult<()> {
        if !self.manager.status().await.is_active() { return Ok(()); }
        self.manager.handle_fill(self.exchange.as_ref(), &fill).await?;
        let s = self.manager.get_state_summary().await;
        info!("Fill: pos={}, pnl={:.2}, trips={}", s.current_position, s.realized_pnl, s.round_trips);
        Ok(())
    }

    async fn check_risk(&self) -> GridResult<()> {
        if self.manager.status().await != BotStatus::Running { return Ok(()); }
        match self.manager.check_risk(self.exchange.as_ref()).await? {
            super::types::RiskStatus::Safe => Ok(()),
            super::types::RiskStatus::Warning => { warn!("Risk warning"); Ok(()) }
            super::types::RiskStatus::HighRisk => { warn!("High risk"); Ok(()) }
            super::types::RiskStatus::Critical => self.manager.emergency_shutdown(self.exchange.as_ref()).await,
        }
    }

    pub async fn stop(&mut self) -> GridResult<()> { self.manager.stop(self.exchange.as_ref()).await }
    pub async fn get_state_summary(&self) -> super::manager::GridStateSummary { self.manager.get_state_summary().await }
    pub async fn get_position(&self) -> GridResult<Option<super::types::Position>> { self.manager.get_position(self.exchange.as_ref()).await }
    pub async fn get_margin_info(&self) -> GridResult<super::types::MarginInfo> { self.manager.get_margin_info(self.exchange.as_ref()).await }
}

/// Enum to hold either spot or perp runner
pub enum GridRunnerKind<E: GridExchange, P: PriceFeed, F: FillFeed> {
    Spot(SpotGridRunner<E, P, F>),
    Perp(PerpGridRunner<E, P, F>),
}

impl<E: GridExchange + 'static, P: PriceFeed + 'static, F: FillFeed + 'static> GridRunnerKind<E, P, F> {
    pub async fn run(&mut self) -> GridResult<()> {
        match self { Self::Spot(r) => r.run().await, Self::Perp(r) => r.run().await }
    }
    pub async fn stop(&mut self) -> GridResult<()> {
        match self { Self::Spot(r) => r.stop().await, Self::Perp(r) => r.stop().await }
    }
    pub async fn get_state_summary(&self) -> super::manager::GridStateSummary {
        match self { Self::Spot(r) => r.get_state_summary().await, Self::Perp(r) => r.get_state_summary().await }
    }
}

pub async fn create_runner<E: GridExchange + 'static, P: PriceFeed + 'static, F: FillFeed + 'static>(
    config: GridConfig, strategy: GridStrategy, exchange: E, price_feed: P, fill_feed: F, runner_config: RunnerConfig,
) -> GridResult<GridRunnerKind<E, P, F>> {
    match config.market_type {
        MarketType::Spot => Ok(GridRunnerKind::Spot(SpotGridRunner::new(config, strategy, exchange, price_feed, fill_feed, runner_config)?)),
        MarketType::Perp => Ok(GridRunnerKind::Perp(PerpGridRunner::new(config, strategy, exchange, price_feed, fill_feed, runner_config)?)),
    }
}
