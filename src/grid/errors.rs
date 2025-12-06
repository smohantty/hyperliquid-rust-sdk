//! Grid-specific error types

use thiserror::Error;

/// Errors that can occur in grid trading operations
#[derive(Error, Debug, Clone)]
pub enum GridError {
    #[error("Invalid grid configuration: {0}")]
    InvalidConfig(String),

    #[error("Grid level not found: index {0}")]
    LevelNotFound(u32),

    #[error("Order not found: oid {0}")]
    OrderNotFound(u64),

    #[error("Price out of grid range: {price} not in [{lower}, {upper}]")]
    PriceOutOfRange {
        price: f64,
        lower: f64,
        upper: f64,
    },

    #[error("Exchange error: {0}")]
    Exchange(String),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("State persistence error: {0}")]
    StatePersistence(String),

    #[error("Risk limit exceeded: {0}")]
    RiskLimitExceeded(String),

    #[error("Insufficient balance: required {required}, available {available}")]
    InsufficientBalance { required: f64, available: f64 },

    #[error("Order placement failed after {attempts} attempts: {reason}")]
    OrderPlacementFailed { attempts: u32, reason: String },

    #[error("Initialization error: {0}")]
    Initialization(String),

    #[error("Bot is in invalid state for operation: {current_state}")]
    InvalidState { current_state: String },

    #[error("Asset not found: {0}")]
    AssetNotFound(String),

    #[error("Channel send error: {0}")]
    ChannelSend(String),

    #[error("Channel receive error: {0}")]
    ChannelRecv(String),

    #[error("JSON parse error: {0}")]
    JsonParse(String),

    #[error("SDK error: {0}")]
    Sdk(String),
}

impl From<crate::Error> for GridError {
    fn from(err: crate::Error) -> Self {
        GridError::Sdk(err.to_string())
    }
}

impl From<serde_json::Error> for GridError {
    fn from(err: serde_json::Error) -> Self {
        GridError::JsonParse(err.to_string())
    }
}

impl From<std::io::Error> for GridError {
    fn from(err: std::io::Error) -> Self {
        GridError::StatePersistence(err.to_string())
    }
}

/// Result type for grid operations
pub type GridResult<T> = std::result::Result<T, GridError>;

