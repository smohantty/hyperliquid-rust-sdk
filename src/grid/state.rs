//! Grid state management with JSON persistence

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::config::GridConfig;
use super::errors::{GridError, GridResult};
use super::types::{BotStatus, GridLevel, GridProfit, LevelStatus, OrderSide};

/// Persistent grid state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridState {
    /// Current bot status
    pub status: BotStatus,

    /// All grid levels
    pub levels: Vec<GridLevel>,

    /// Mapping from oid to level index (for fast lookup on fills)
    #[serde(default)]
    pub oid_to_level: HashMap<u64, u32>,

    /// Profit tracking
    pub profit: GridProfit,

    /// Current position size (for tracking)
    pub current_position: f64,

    /// Last known mid price
    pub last_mid_price: f64,

    /// Timestamp of last state update
    pub last_updated: u64,

    /// OID of initial buy order (if waiting for fill)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_buy_oid: Option<u64>,

    /// Whether initial position has been acquired
    pub init_position_acquired: bool,

    /// Configuration snapshot (for recovery validation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_snapshot: Option<ConfigSnapshot>,
}

/// Minimal config snapshot for state validation
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigSnapshot {
    asset: String,
    lower_price: f64,
    upper_price: f64,
    num_grids: u32,
}

impl GridState {
    /// Create a new empty state
    pub fn new() -> Self {
        Self {
            status: BotStatus::WaitingForEntry,
            levels: Vec::new(),
            oid_to_level: HashMap::new(),
            profit: GridProfit::default(),
            current_position: 0.0,
            last_mid_price: 0.0,
            last_updated: 0,
            init_buy_oid: None,
            init_position_acquired: false,
            config_snapshot: None,
        }
    }

    /// Initialize state from config
    pub fn from_config(config: &GridConfig, levels: Vec<GridLevel>) -> Self {
        let mut state = Self::new();
        state.levels = levels;
        state.config_snapshot = Some(ConfigSnapshot {
            asset: config.asset.clone(),
            lower_price: config.lower_price,
            upper_price: config.upper_price,
            num_grids: config.num_grids,
        });

        // If no trigger price, go directly to initializing
        if config.trigger_price.is_none() {
            state.status = BotStatus::Initializing;
        }

        state
    }

    /// Validate that loaded state matches current config
    pub fn validate_against_config(&self, config: &GridConfig) -> GridResult<()> {
        if let Some(snapshot) = &self.config_snapshot {
            if snapshot.asset != config.asset {
                return Err(GridError::InvalidConfig(format!(
                    "State asset '{}' doesn't match config asset '{}'",
                    snapshot.asset, config.asset
                )));
            }

            if (snapshot.lower_price - config.lower_price).abs() > 0.0001
                || (snapshot.upper_price - config.upper_price).abs() > 0.0001
                || snapshot.num_grids != config.num_grids
            {
                return Err(GridError::InvalidConfig(
                    "State grid parameters don't match config".into(),
                ));
            }
        }
        Ok(())
    }

    /// Get a level by index
    pub fn get_level(&self, index: u32) -> Option<&GridLevel> {
        self.levels.get(index as usize)
    }

    /// Get a mutable level by index
    pub fn get_level_mut(&mut self, index: u32) -> Option<&mut GridLevel> {
        self.levels.get_mut(index as usize)
    }

    /// Find level by oid
    pub fn find_level_by_oid(&self, oid: u64) -> Option<&GridLevel> {
        self.oid_to_level
            .get(&oid)
            .and_then(|&idx| self.get_level(idx))
    }

    /// Find level index by oid
    pub fn find_level_index_by_oid(&self, oid: u64) -> Option<u32> {
        self.oid_to_level.get(&oid).copied()
    }

    /// Register an order with its OID
    pub fn register_order(&mut self, level_index: u32, oid: u64) {
        self.oid_to_level.insert(oid, level_index);
    }

    /// Unregister an order
    pub fn unregister_order(&mut self, oid: u64) {
        self.oid_to_level.remove(&oid);
    }

    /// Get all levels with active orders
    pub fn active_levels(&self) -> impl Iterator<Item = &GridLevel> {
        self.levels.iter().filter(|l| l.has_active_order())
    }

    /// Get all levels that need orders placed
    pub fn empty_levels(&self) -> impl Iterator<Item = &GridLevel> {
        self.levels
            .iter()
            .filter(|l| l.status == LevelStatus::Empty)
    }

    /// Count active buy orders
    pub fn count_active_buys(&self) -> usize {
        self.levels
            .iter()
            .filter(|l| l.has_active_order() && l.intended_side == OrderSide::Buy)
            .count()
    }

    /// Count active sell orders
    pub fn count_active_sells(&self) -> usize {
        self.levels
            .iter()
            .filter(|l| l.has_active_order() && l.intended_side == OrderSide::Sell)
            .count()
    }

    /// Update timestamp
    pub fn touch(&mut self) {
        self.last_updated = chrono::Utc::now().timestamp_millis() as u64;
    }

    /// Load state from file
    pub fn load_from_file(path: impl AsRef<Path>) -> GridResult<Self> {
        let content = std::fs::read_to_string(path)?;
        let state: Self = serde_json::from_str(&content)?;
        Ok(state)
    }

    /// Save state to file
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> GridResult<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Save state to file atomically (write to temp, then rename)
    pub fn save_to_file_atomic(&self, path: impl AsRef<Path>) -> GridResult<()> {
        let path = path.as_ref();
        let temp_path = path.with_extension("tmp");

        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&temp_path, content)?;
        std::fs::rename(&temp_path, path)?;

        Ok(())
    }
}

impl Default for GridState {
    fn default() -> Self {
        Self::new()
    }
}

/// State manager with automatic persistence
pub struct StateManager {
    state: Arc<RwLock<GridState>>,
    save_path: Option<std::path::PathBuf>,
    save_interval: Duration,
    last_save: Arc<RwLock<Instant>>,
}

impl StateManager {
    /// Create a new state manager
    pub fn new(state: GridState, config: &GridConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(state)),
            save_path: config.state_file.clone(),
            save_interval: Duration::from_secs(config.state_save_interval_secs),
            last_save: Arc::new(RwLock::new(Instant::now())),
        }
    }

    /// Get read access to state
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, GridState> {
        self.state.read().await
    }

    /// Get write access to state
    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, GridState> {
        self.state.write().await
    }

    /// Update state and optionally persist
    pub async fn update<F, R>(&self, f: F) -> GridResult<R>
    where
        F: FnOnce(&mut GridState) -> R,
    {
        let result = {
            let mut state = self.state.write().await;
            state.touch();
            f(&mut state)
        };

        // Check if we should save
        self.maybe_save().await?;

        Ok(result)
    }

    /// Force save state to file
    pub async fn force_save(&self) -> GridResult<()> {
        if let Some(path) = &self.save_path {
            let state = self.state.read().await;
            state.save_to_file_atomic(path)?;
            *self.last_save.write().await = Instant::now();
            debug!("State saved to {:?}", path);
        }
        Ok(())
    }

    /// Save if enough time has passed since last save
    async fn maybe_save(&self) -> GridResult<()> {
        let should_save = {
            let last_save = self.last_save.read().await;
            last_save.elapsed() >= self.save_interval
        };

        if should_save {
            self.force_save().await?;
        }

        Ok(())
    }

    /// Load state from file or create new
    pub fn load_or_create(config: &GridConfig, levels: Vec<GridLevel>) -> GridResult<Self> {
        let state = if let Some(path) = &config.state_file {
            if path.exists() {
                info!("Loading existing state from {:?}", path);
                match GridState::load_from_file(path) {
                    Ok(state) => {
                        state.validate_against_config(config)?;
                        info!(
                            "Loaded state: status={:?}, {} levels, {} active orders",
                            state.status,
                            state.levels.len(),
                            state.active_levels().count()
                        );
                        state
                    }
                    Err(e) => {
                        warn!("Failed to load state: {}, creating new state", e);
                        GridState::from_config(config, levels)
                    }
                }
            } else {
                info!("No existing state file, creating new state");
                GridState::from_config(config, levels)
            }
        } else {
            GridState::from_config(config, levels)
        };

        Ok(Self::new(state, config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::config::MarketType;

    #[test]
    fn test_state_serialization() {
        let mut state = GridState::new();
        state.levels.push(GridLevel::new(0, 100.0, OrderSide::Buy));
        state.levels.push(GridLevel::new(1, 110.0, OrderSide::Sell));

        let json = serde_json::to_string(&state).unwrap();
        let loaded: GridState = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.levels.len(), 2);
        assert_eq!(loaded.levels[0].price, 100.0);
    }

    #[test]
    fn test_order_registration() {
        let mut state = GridState::new();
        state.levels.push(GridLevel::new(0, 100.0, OrderSide::Buy));

        let oid = 12345u64;
        state.register_order(0, oid);

        assert_eq!(state.find_level_index_by_oid(oid), Some(0));

        state.unregister_order(oid);
        assert_eq!(state.find_level_index_by_oid(oid), None);
    }

    #[test]
    fn test_config_validation() {
        // $1000 total investment
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1000.0, MarketType::Spot);
        let levels = vec![GridLevel::new(0, 100.0, OrderSide::Buy)];
        let state = GridState::from_config(&config, levels);

        assert!(state.validate_against_config(&config).is_ok());

        // Different asset should fail
        let other_config = GridConfig::new("ETH", 100.0, 200.0, 10, 1000.0, MarketType::Spot);
        assert!(state.validate_against_config(&other_config).is_err());
    }
}
