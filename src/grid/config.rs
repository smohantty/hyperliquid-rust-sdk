//! Grid trading configuration

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::helpers::truncate_float;

use super::errors::{GridError, GridResult};

/// Market type for grid trading
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketType {
    /// Spot trading
    Spot,
    /// Perpetual futures trading
    Perp,
}

/// Method for acquiring initial base position
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InitialPositionMethod {
    /// Place a limit buy order at trigger price
    LimitBuy,
    /// Place an IOC market buy
    MarketBuy,
    /// Assume base asset is already available
    Skip,
}

impl Default for InitialPositionMethod {
    fn default() -> Self {
        Self::LimitBuy
    }
}

/// Grid spacing type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GridSpacing {
    /// Uniform price spacing (e.g., $100, $110, $120)
    Arithmetic,
    /// Percentage-based spacing (e.g., +10%, +10%, +10%)
    Geometric,
}

impl Default for GridSpacing {
    fn default() -> Self {
        Self::Arithmetic
    }
}

/// Asset precision information fetched from exchange meta
///
/// According to Hyperliquid docs:
/// - Prices can have up to 5 significant figures
/// - Price decimals = MAX_DECIMALS - szDecimals (6 for perps, 8 for spot)
/// - Size decimals = szDecimals from meta
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AssetPrecision {
    /// Decimal places for size (szDecimals from meta)
    pub sz_decimals: u32,
    /// Decimal places for price (calculated from market type)
    pub price_decimals: u32,
    /// Maximum decimals constant (6 for perps, 8 for spot)
    pub max_decimals: u32,
}

impl AssetPrecision {
    /// Create precision for a perp asset
    pub fn for_perp(sz_decimals: u32) -> Self {
        const MAX_DECIMALS_PERP: u32 = 6;
        Self {
            sz_decimals,
            price_decimals: MAX_DECIMALS_PERP.saturating_sub(sz_decimals),
            max_decimals: MAX_DECIMALS_PERP,
        }
    }

    /// Create precision for a spot asset
    pub fn for_spot(sz_decimals: u32) -> Self {
        const MAX_DECIMALS_SPOT: u32 = 8;
        Self {
            sz_decimals,
            price_decimals: MAX_DECIMALS_SPOT.saturating_sub(sz_decimals),
            max_decimals: MAX_DECIMALS_SPOT,
        }
    }

    /// Round a price to the correct precision using truncate_float
    /// 
    /// Enforces Hyperliquid's tick size rules:
    /// - Max 5 significant figures
    /// - Max price_decimals decimal places (MAX_DECIMALS - szDecimals)
    /// 
    /// Uses truncate_float for the rounding operation.
    pub fn round_price(&self, price: f64, round_up: bool) -> f64 {
        // Integer prices are always allowed
        if price.fract() == 0.0 {
            return price;
        }

        let abs_price = price.abs();

        // Calculate decimal places needed for 5 significant figures
        let first_digit_pos = if abs_price >= 1.0 {
            abs_price.log10().floor() as i32
        } else {
            abs_price.log10().ceil() as i32 - 1
        };

        // For 5 sig figs: need positions first_digit_pos down to (first_digit_pos - 4)
        let decimals_for_5_sig = if first_digit_pos >= 0 {
            (4i32 - first_digit_pos).max(0) as u32
        } else {
            (-first_digit_pos + 4) as u32
        };

        // Use minimum of sig figs constraint and max decimals constraint
        let decimals = decimals_for_5_sig.min(self.price_decimals);

        // For proper rounding (not just truncation), check next digit
        let should_round_up = if round_up {
            true
        } else {
            // Check digit after target decimals for round-to-nearest
            let multiplier = 10f64.powi(decimals as i32 + 1);
            let next_digit = ((abs_price * multiplier) as u64) % 10;
            next_digit >= 5
        };

        truncate_float(price, decimals, should_round_up)
    }

    /// Round a size to the correct precision
    pub fn round_size(&self, size: f64) -> f64 {
        truncate_float(size, self.sz_decimals, false)
    }
}

impl Default for AssetPrecision {
    fn default() -> Self {
        // Default to perp with 0 sz_decimals (conservative)
        Self::for_perp(0)
    }
}

/// Grid bot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridConfig {
    /// Asset/coin to trade (e.g., "BTC", "ETH", "PURR/USDC")
    pub asset: String,

    /// Lower price boundary for the grid
    pub lower_price: f64,

    /// Upper price boundary for the grid
    pub upper_price: f64,

    /// Number of grid levels (creates num_grids + 1 price points)
    pub num_grids: u32,

    /// Total investment amount in USD
    /// The order size per grid level is calculated as:
    /// order_size = total_investment / num_grids / current_price
    pub total_investment: f64,

    /// Optional trigger price to start the bot
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_price: Option<f64>,

    /// Market type (Spot or Perp)
    pub market_type: MarketType,

    /// For perps: leverage setting (1-100)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leverage: Option<u32>,

    /// For perps: max margin ratio before risk shutdown (0.0-1.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_margin_ratio: Option<f64>,

    /// Method for acquiring initial base position
    #[serde(default)]
    pub initial_position_method: InitialPositionMethod,

    /// Grid spacing type (Arithmetic or Geometric)
    #[serde(default)]
    pub grid_spacing: GridSpacing,

    /// State persistence file path
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_file: Option<PathBuf>,

    /// State save interval in seconds
    #[serde(default = "default_save_interval")]
    pub state_save_interval_secs: u64,

    /// Maximum retry attempts for order placement
    #[serde(default = "default_max_retries")]
    pub max_order_retries: u32,

    /// Base delay for exponential backoff (milliseconds)
    #[serde(default = "default_retry_base_delay")]
    pub retry_base_delay_ms: u64,
}

fn default_save_interval() -> u64 {
    30
}

fn default_max_retries() -> u32 {
    5
}

fn default_retry_base_delay() -> u64 {
    100
}

impl GridConfig {
    /// Create a new grid configuration with required parameters
    ///
    /// State file is automatically generated with format:
    /// `grid_{asset}_{spot|perp}_{YYYYMMDD_HHMMSS}.json`
    ///
    /// # Arguments
    /// * `asset` - Asset/coin to trade (e.g., "BTC", "PURR/USDC")
    /// * `lower_price` - Lower price boundary for the grid
    /// * `upper_price` - Upper price boundary for the grid
    /// * `num_grids` - Number of grid levels
    /// * `total_investment` - Total USD amount to invest in the grid
    /// * `market_type` - Spot or Perp
    pub fn new(
        asset: impl Into<String>,
        lower_price: f64,
        upper_price: f64,
        num_grids: u32,
        total_investment: f64,
        market_type: MarketType,
    ) -> Self {
        let asset_str = asset.into();
        let state_file = Self::generate_state_filename(&asset_str, market_type);

        Self {
            asset: asset_str,
            lower_price,
            upper_price,
            num_grids,
            total_investment,
            market_type,
            trigger_price: None,
            leverage: None,
            max_margin_ratio: None,
            initial_position_method: InitialPositionMethod::default(),
            grid_spacing: GridSpacing::default(),
            state_file: Some(state_file),
            state_save_interval_secs: default_save_interval(),
            max_order_retries: default_max_retries(),
            retry_base_delay_ms: default_retry_base_delay(),
        }
    }

    /// Builder: set grid spacing type
    pub fn with_grid_spacing(mut self, spacing: GridSpacing) -> Self {
        self.grid_spacing = spacing;
        self
    }

    /// Generate a unique state filename based on asset, market type, and timestamp
    ///
    /// Format: `grid_{asset}_{spot|perp}_{YYYYMMDD_HHMMSS}.json`
    /// Example: `grid_BTC_perp_20251206_143052.json`
    /// Example: `grid_PURR-USDC_spot_20251206_143052.json`
    pub fn generate_state_filename(asset: &str, market_type: MarketType) -> PathBuf {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let market_str = match market_type {
            MarketType::Spot => "spot",
            MarketType::Perp => "perp",
        };
        // Replace '/' with '-' for filesystem compatibility (e.g., "PURR/USDC" -> "PURR-USDC")
        let safe_asset = asset.replace('/', "-");
        PathBuf::from(format!("grid_{}_{market_str}_{timestamp}.json", safe_asset))
    }

    /// Builder: set trigger price
    pub fn with_trigger_price(mut self, price: f64) -> Self {
        self.trigger_price = Some(price);
        self
    }

    /// Builder: set leverage (perps only)
    pub fn with_leverage(mut self, leverage: u32) -> Self {
        self.leverage = Some(leverage);
        self
    }

    /// Builder: set max margin ratio (perps only)
    pub fn with_max_margin_ratio(mut self, ratio: f64) -> Self {
        self.max_margin_ratio = Some(ratio);
        self
    }

    /// Builder: set initial position method
    pub fn with_initial_position_method(mut self, method: InitialPositionMethod) -> Self {
        self.initial_position_method = method;
        self
    }

    /// Builder: override the auto-generated state file path
    ///
    /// By default, state file is auto-generated as:
    /// `grid_{asset}_{spot|perp}_{timestamp}.json`
    ///
    /// Use this to specify a custom path if needed.
    pub fn with_state_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.state_file = Some(path.into());
        self
    }

    /// Builder: disable state persistence
    pub fn without_state_file(mut self) -> Self {
        self.state_file = None;
        self
    }

    /// Builder: set state save interval
    pub fn with_save_interval(mut self, secs: u64) -> Self {
        self.state_save_interval_secs = secs;
        self
    }

    /// Builder: set retry parameters
    pub fn with_retry_config(mut self, max_retries: u32, base_delay_ms: u64) -> Self {
        self.max_order_retries = max_retries;
        self.retry_base_delay_ms = base_delay_ms;
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> GridResult<()> {
        if self.lower_price >= self.upper_price {
            return Err(GridError::InvalidConfig(
                "lower_price must be less than upper_price".into(),
            ));
        }

        if self.num_grids < 2 {
            return Err(GridError::InvalidConfig(
                "num_grids must be at least 2".into(),
            ));
        }

        if self.total_investment <= 0.0 {
            return Err(GridError::InvalidConfig(
                "total_investment must be positive".into(),
            ));
        }

        if self.asset.is_empty() {
            return Err(GridError::InvalidConfig("asset cannot be empty".into()));
        }

        if let Some(trigger) = self.trigger_price {
            if trigger < self.lower_price || trigger > self.upper_price {
                return Err(GridError::InvalidConfig(
                    "trigger_price must be within grid range".into(),
                ));
            }
        }

        if self.market_type == MarketType::Perp {
            if let Some(leverage) = self.leverage {
                if leverage == 0 || leverage > 100 {
                    return Err(GridError::InvalidConfig(
                        "leverage must be between 1 and 100".into(),
                    ));
                }
            }

            if let Some(ratio) = self.max_margin_ratio {
                if !(0.0..=1.0).contains(&ratio) {
                    return Err(GridError::InvalidConfig(
                        "max_margin_ratio must be between 0.0 and 1.0".into(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Calculate the price step between grid levels
    pub fn price_step(&self) -> f64 {
        (self.upper_price - self.lower_price) / self.num_grids as f64
    }

    /// Calculate order size for a specific price level
    ///
    /// Each grid level invests the same USD amount, so:
    /// - Lower prices = larger order size (buy more when cheap)
    /// - Higher prices = smaller order size (sell less when expensive)
    ///
    /// Formula: order_size = (total_investment / num_grids) / level_price
    pub fn calculate_order_size_at_price(&self, price: f64, precision: &AssetPrecision) -> f64 {
        let usd_per_grid = self.usd_per_grid();
        let raw_size = usd_per_grid / price;
        precision.round_size(raw_size)
    }

    /// Get the USD value per grid level
    pub fn usd_per_grid(&self) -> f64 {
        self.total_investment / self.num_grids as f64
    }

    /// Round a price using asset precision
    pub fn round_price(&self, price: f64, precision: &AssetPrecision, round_up: bool) -> f64 {
        precision.round_price(price, round_up)
    }

    /// Round a size using asset precision
    pub fn round_size(&self, size: f64, precision: &AssetPrecision) -> f64 {
        precision.round_size(size)
    }

    /// Calculate total number of price levels (num_grids + 1)
    pub fn num_levels(&self) -> u32 {
        self.num_grids + 1
    }

    /// Count buy levels below a price
    pub fn count_buy_levels(&self, price: f64) -> u32 {
        let step = self.price_step();
        let mut count = 0;
        let mut level_price = self.lower_price;

        while level_price < price && count <= self.num_grids {
            count += 1;
            level_price += step;
        }

        count.saturating_sub(1)
    }

    /// Count sell levels above a price
    pub fn count_sell_levels(&self, price: f64) -> u32 {
        self.num_grids - self.count_buy_levels(price)
    }

    /// Load config from JSON file
    pub fn load_from_file(path: impl AsRef<std::path::Path>) -> GridResult<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Save config to JSON file
    pub fn save_to_file(&self, path: impl AsRef<std::path::Path>) -> GridResult<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        // $1000 total investment
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1000.0, MarketType::Spot);
        assert!(config.validate().is_ok());

        // Invalid: lower >= upper
        let config = GridConfig::new("BTC", 200.0, 100.0, 10, 1000.0, MarketType::Spot);
        assert!(config.validate().is_err());

        // Invalid: num_grids < 2
        let config = GridConfig::new("BTC", 100.0, 200.0, 1, 1000.0, MarketType::Spot);
        assert!(config.validate().is_err());

        // Invalid: total_investment <= 0
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 0.0, MarketType::Spot);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_price_step() {
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1000.0, MarketType::Spot);
        assert!((config.price_step() - 10.0).abs() < 0.0001);
    }

    #[test]
    fn test_count_levels() {
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1000.0, MarketType::Spot);

        // At price 150, should have ~5 buy levels and ~5 sell levels
        let buy_levels = config.count_buy_levels(150.0);
        let sell_levels = config.count_sell_levels(150.0);
        assert!(buy_levels >= 4 && buy_levels <= 6);
        assert!(sell_levels >= 4 && sell_levels <= 6);
    }

    #[test]
    fn test_usd_per_grid() {
        // $1000 total, 10 grids => $100/grid
        let config = GridConfig::new("BTC", 100.0, 200.0, 10, 1000.0, MarketType::Spot);
        assert!((config.usd_per_grid() - 100.0).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_order_size_at_price() {
        // $1000 total, 10 grids => $100/grid
        let config = GridConfig::new("BTC", 40.0, 60.0, 10, 1000.0, MarketType::Spot);
        let precision = AssetPrecision::for_spot(4);

        // At price $50: order_size = $100 / $50 = 2.0 BTC
        let order_size_50 = config.calculate_order_size_at_price(50.0, &precision);
        assert!((order_size_50 - 2.0).abs() < 0.0001);

        // At price $40 (lower): order_size = $100 / $40 = 2.5 BTC (more coins)
        let order_size_40 = config.calculate_order_size_at_price(40.0, &precision);
        assert!((order_size_40 - 2.5).abs() < 0.0001);

        // At price $100 (higher): order_size = $100 / $100 = 1.0 BTC (fewer coins)
        let order_size_100 = config.calculate_order_size_at_price(100.0, &precision);
        assert!((order_size_100 - 1.0).abs() < 0.0001);

        // Verify: lower price = more coins, higher price = fewer coins
        assert!(order_size_40 > order_size_50);
        assert!(order_size_50 > order_size_100);
    }

    #[test]
    fn test_asset_precision_perp() {
        // BTC with szDecimals = 4
        let precision = AssetPrecision::for_perp(4);
        assert_eq!(precision.sz_decimals, 4);
        assert_eq!(precision.price_decimals, 2); // 6 - 4 = 2
        assert_eq!(precision.max_decimals, 6);
    }

    #[test]
    fn test_asset_precision_spot() {
        // Token with szDecimals = 2
        let precision = AssetPrecision::for_spot(2);
        assert_eq!(precision.sz_decimals, 2);
        assert_eq!(precision.price_decimals, 6); // 8 - 2 = 6
        assert_eq!(precision.max_decimals, 8);
    }

    #[test]
    fn test_precision_rounding() {
        let precision = AssetPrecision::for_perp(3); // price_decimals = 3

        let rounded = precision.round_price(123.45678, false);
        assert!((rounded - 123.456).abs() < 0.0001);

        let rounded_size = precision.round_size(1.23456);
        assert!((rounded_size - 1.234).abs() < 0.0001);
    }

    #[test]
    fn test_price_rounding_hyperliquid_rules() {
        // Test with spot asset: szDecimals=2, price_decimals=6
        let precision = AssetPrecision::for_spot(2);

        // Test cases - should round to correct decimals and 5 sig figs
        // 15.21732 -> should become 15.217 (3 decimals for 5 sig figs)
        let price1 = precision.round_price(15.21732, false);
        assert!((price1 - 15.217).abs() < 0.0001);

        // 15.43779 -> should become 15.438 (3 decimals, rounds up)
        let price2 = precision.round_price(15.43779, false);
        assert!((price2 - 15.438).abs() < 0.0001);

        // 17.320508 -> should become 17.321 (3 decimals for 5 sig figs)
        let price3 = precision.round_price(17.320508, false);
        assert!((price3 - 17.321).abs() < 0.0001);

        // Integer prices should pass through unchanged
        let price4 = precision.round_price(15.0, false);
        assert!((price4 - 15.0).abs() < 0.0001);

        // Test with perp: szDecimals=1, price_decimals=5
        let perp_precision = AssetPrecision::for_perp(1);
        // 1234.56 -> should become 1234.6 (1 decimal for 5 sig figs, rounds up)
        let perp_price = perp_precision.round_price(1234.56, false);
        assert!((perp_price - 1234.6).abs() < 0.01);
    }
}
