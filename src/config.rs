use config::{Config, File};
pub use config::ConfigError;
use serde::Deserialize;
use serde_json::Value; // Add this import

/// Main configuration struct
#[derive(Debug, Deserialize)]
pub struct Settings {
    /// Network configuration (env, mode, wallet)
    pub network: NetworkConfig,
    /// Strategy configuration (type, asset, params)
    pub strategy: StrategyConfig,
    /// Logging configuration
    #[serde(default)]
    pub log: LogConfig,
    /// Server configuration
    #[serde(default)]
    pub server: ServerConfig,
}

#[derive(Debug, Deserialize)]
pub struct NetworkConfig {
    /// Environment: "mainnet" or "testnet"
    pub env: String,
    /// Mode: "live" or "paper"
    pub mode: String,
    /// Wallet private key (hex string)
    /// In production, consider loading this from ENV variables only
    pub wallet_private_key: String,
}

#[derive(Debug, Deserialize)]
pub struct StrategyConfig {
    /// Strategy type name (e.g., "grid", "market_maker")
    #[serde(rename = "type")]
    pub type_name: String,
    /// Asset to trade (e.g., "HYPE/USDC", "BTC")
    pub asset: String,
    /// Strategy-specific parameters
    #[serde(default)]
    pub params: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Deserialize, Default)]
pub struct LogConfig {
    /// Log level: "error", "warn", "info", "debug", "trace"
    #[serde(default = "default_log_level")]
    pub level: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    /// Enable local dashboard server
    #[serde(default = "default_server_enabled")]
    pub enabled: bool,
    /// Server port (default 3000)
    #[serde(default = "default_server_port")]
    pub port: u16,
    /// Server host (default 127.0.0.1)
    #[serde(default = "default_server_host")]
    pub host: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            enabled: default_server_enabled(),
            port: default_server_port(),
            host: default_server_host(),
        }
    }
}

fn default_server_enabled() -> bool {
    false
}

fn default_server_port() -> u16 {
    3000
}

fn default_server_host() -> String {
    "127.0.0.1".to_string()
}

impl Settings {
    /// Load settings from a configuration file
    pub fn new(config_path: &str) -> Result<Self, ConfigError> {
        let s = Config::builder()
            // Start with defaults if needed
            // Add configuration file
            .add_source(File::with_name(config_path))
            // Add environment variables (overrides file)
            // e.g. APP_NETWORK__WALLET_PRIVATE_KEY=...
            .add_source(config::Environment::with_prefix("APP").separator("__"))
            .build()?;

        s.try_deserialize()
    }
}
