//! Market Interface Module
//!
//! This module provides a generic market interface for price management,
//! order handling, and listener notifications.
//!
//! # Requirements Implemented
//!
//! ## Market Responsibilities
//! - **M1**: Price Management - Update and retrieve asset prices
//! - **M2**: Order Acceptance - Accept orders with user-provided IDs
//! - **M3**: Order Execution Notification - Notify listener on fills
//! - **M4**: Price Update Notification - Notify listener on price changes
//! - **M5**: Listener Ownership - Market owns a single listener instance
//! - **M6**: Synchronous Invocation - All notifications are synchronous
//!
//! ## Market API
//! - **M7**: `update_price` - Update price and notify listener
//! - **M8**: `place_order` - Accept order with user-provided ID
//! - **M9**: `execute_fill` - Inject external fill and notify listener
//! - **M10**: `current_price` - Query last known price
//! - **M11**: `order_status` - Query order status
//!
//! # Market Implementations
//!
//! | Implementation | Description |
//! |---------------|-------------|
//! | `Market` | In-memory market for testing |
//! | `HyperliquidMarket` | Live trading on Hyperliquid exchange |
//! | `PaperTradingMarket` | Paper trading with live price feeds |
//!
//! # Examples
//!
//! ## Basic Market (in-memory)
//!
//! ```rust
//! use hyperliquid_rust_sdk::market::{Market, OrderRequest, OrderSide, NoOpListener};
//!
//! // Create a market with a no-op listener
//! let mut market = Market::new(NoOpListener);
//!
//! // Update price
//! market.update_price("BTC", 50000.0);
//!
//! // Place a buy order (order_id=1)
//! let order = OrderRequest::buy(1, "BTC", 1.0, 51000.0);
//! market.place_order(order);
//!
//! // Query status
//! if let Some(status) = market.order_status(1) {
//!     println!("Order status: {:?}", status);
//! }
//! ```
//!
//! ## Hyperliquid Market (live exchange)
//!
//! ```ignore
//! use hyperliquid_rust_sdk::market::{
//!     HyperliquidMarket, HyperliquidMarketInput, OrderRequest, NoOpListener
//! };
//! use hyperliquid_rust_sdk::BaseUrl;
//!
//! let input = HyperliquidMarketInput {
//!     asset: "BTC".to_string(),
//!     wallet: wallet,
//!     base_url: Some(BaseUrl::Testnet),
//! };
//!
//! let mut market = HyperliquidMarket::new(input, NoOpListener).await?;
//!
//! // Place a limit buy order
//! let order = OrderRequest::buy(1, "BTC", 0.1, 50000.0);
//! market.place_order(order).await;
//!
//! // Start the event loop (runs indefinitely)
//! market.start().await;
//! ```
//!
//! ## Paper Trading Market (simulated fills with live Mainnet prices)
//!
//! ```ignore
//! use hyperliquid_rust_sdk::market::{
//!     PaperTradingMarket, PaperTradingMarketInput, OrderRequest, NoOpListener
//! };
//!
//! let input = PaperTradingMarketInput {
//!     initial_balance: 10_000.0,
//! };
//!
//! let mut market = PaperTradingMarket::new(input, NoOpListener).await?;
//!
//! // Place a simulated buy order - fills when midprice <= limit
//! let order = OrderRequest::buy(1, "BTC", 0.1, 50000.0);
//! market.place_order(order);
//!
//! // Start event loop (orders fill when midprice crosses limit)
//! market.start().await;
//! ```

mod hyperliquid_market;
mod listener;
mod market;
mod paper_trading_market;
mod types;

pub use hyperliquid_market::{HyperliquidMarket, HyperliquidMarketInput};
pub use listener::{MarketListener, NoOpListener};
pub use market::Market;
pub use paper_trading_market::{PaperPosition, PaperTradingMarket, PaperTradingMarketInput};
pub use types::{AssetInfo, OrderFill, OrderRequest, OrderSide, OrderStatus};

