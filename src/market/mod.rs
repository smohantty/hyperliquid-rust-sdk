//! Market Interface Module
//!
//! This module provides a generic market interface for price management,
//! order handling, and listener notifications.
//!
//! # Requirements Implemented
//!
//! ## Market Responsibilities
//! - **M1**: Price Management - Update and retrieve asset prices
//! - **M2**: Order Acceptance - Accept orders and return unique IDs
//! - **M3**: Order Execution Notification - Notify listener on fills
//! - **M4**: Price Update Notification - Notify listener on price changes
//! - **M5**: Listener Ownership - Market owns a single listener instance
//! - **M6**: Synchronous Invocation - All notifications are synchronous
//!
//! ## Market API
//! - **M7**: `update_price` - Update price and notify listener
//! - **M8**: `place_order` - Accept order and return unique ID
//! - **M9**: `execute_fill` - Inject external fill and notify listener
//! - **M10**: `current_price` - Query last known price
//! - **M11**: `order_status` - Query order status
//!
//! # Example
//!
//! ```rust
//! use hyperliquid_rust_sdk::market::{Market, MarketListener, OrderRequest, OrderFill, NoOpListener};
//!
//! // Create a market with a no-op listener
//! let mut market = Market::new(NoOpListener);
//!
//! // Update price
//! market.update_price("BTC", 50000.0);
//!
//! // Place an order
//! let order = OrderRequest::new("BTC", 1.0, 51000.0);
//! let order_id = market.place_order(order);
//!
//! // Query status
//! if let Some(status) = market.order_status(order_id) {
//!     println!("Order status: {:?}", status);
//! }
//! ```

mod listener;
mod market;
mod types;

pub use listener::{MarketListener, NoOpListener};
pub use market::Market;
pub use types::{OrderFill, OrderRequest, OrderStatus};

