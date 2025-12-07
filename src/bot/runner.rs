//! Bot runner - the main event loop

use std::collections::HashMap;

use alloy::primitives::Address;
use log::{debug, error, info};
use tokio::sync::mpsc::unbounded_channel;

use super::config::{BotConfig, MarketMode};
use crate::market::{AssetInfo, OrderFill, OrderRequest, OrderSide, OrderStatus};
use crate::strategy::Strategy;
use crate::{
    BaseUrl, ClientCancelRequest, ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient,
    ExchangeDataStatus, ExchangeResponseStatus, InfoClient, Message, Subscription, UserData,
};

/// Order tracking for the bot
#[derive(Debug, Clone)]
struct TrackedOrder {
    request: OrderRequest,
    exchange_oid: Option<u64>,
    status: OrderStatus,
    filled_qty: f64,
    avg_fill_price: f64,
}

impl TrackedOrder {
    fn new(request: OrderRequest) -> Self {
        Self {
            request,
            exchange_oid: None,
            status: OrderStatus::Pending,
            filled_qty: 0.0,
            avg_fill_price: 0.0,
        }
    }

    fn fill(&mut self, qty: f64, price: f64) {
        let total_value = self.avg_fill_price * self.filled_qty + price * qty;
        self.filled_qty += qty;
        self.avg_fill_price = if self.filled_qty > 0.0 {
            total_value / self.filled_qty
        } else {
            0.0
        };

        if self.filled_qty >= self.request.qty {
            self.status = OrderStatus::Filled(self.avg_fill_price);
        } else {
            self.status = OrderStatus::PartiallyFilled(self.filled_qty);
        }
    }
}

/// Paper trading state
#[derive(Debug, Default)]
struct PaperState {
    balance: f64,
    position: f64,
    total_fees: f64,
    fee_rate: f64,
}

impl PaperState {
    fn new(initial_balance: f64) -> Self {
        Self {
            balance: initial_balance,
            position: 0.0,
            total_fees: 0.0,
            fee_rate: 0.0001, // 0.01%
        }
    }
}

/// Bot that combines market connectivity with strategy execution
///
/// The bot handles:
/// - Connecting to the exchange (live) or simulating (paper)
/// - Receiving price updates and fill notifications
/// - Calling the strategy to generate orders
/// - Executing orders on the market
///
/// # Example
///
/// ```ignore
/// use hyperliquid_rust_sdk::bot::{Bot, BotConfig};
/// use hyperliquid_rust_sdk::strategy::Strategy;
///
/// struct MyStrategy { /* ... */ }
/// impl Strategy for MyStrategy { /* ... */ }
///
/// let config = BotConfig::paper("HYPE/USDC", 10_000.0);
/// let mut bot = Bot::new(config, MyStrategy::new()).await?;
/// bot.run().await;
/// ```
pub struct Bot<S: Strategy> {
    /// Bot configuration
    config: BotConfig,
    /// Trading strategy
    strategy: S,
    /// Cached asset info (precision)
    asset_info: AssetInfo,
    /// Info client for market data
    info_client: InfoClient,
    /// Exchange client (None for paper trading)
    exchange_client: Option<ExchangeClient>,
    /// User's wallet address (for live trading)
    user_address: Option<Address>,
    /// Current prices by asset
    prices: HashMap<String, f64>,
    /// Orders by user-provided order_id
    orders: HashMap<u64, TrackedOrder>,
    /// Maps exchange OID to user's order_id
    exchange_oid_to_order_id: HashMap<u64, u64>,
    /// Paper trading state
    paper_state: Option<PaperState>,
}

impl<S: Strategy> Bot<S> {
    /// Create a new bot
    ///
    /// # Arguments
    /// * `config` - Bot configuration (live or paper trading)
    /// * `strategy` - Trading strategy to use
    pub async fn new(config: BotConfig, strategy: S) -> Result<Self, crate::Error> {
        match config.mode.clone() {
            MarketMode::Live { wallet, base_url } => {
                let base_url = base_url.unwrap_or(BaseUrl::Mainnet);
                let user_address = wallet.address();

                let info_client = InfoClient::new(None, Some(base_url)).await?;
                let exchange_client =
                    ExchangeClient::new(None, wallet.clone(), Some(base_url), None, None).await?;

                let asset_info =
                    Self::fetch_asset_info(&info_client, &config.asset, Some(user_address)).await?;

                info!(
                    "Bot created for {} (live), sz_decimals={}, price_decimals={}",
                    config.asset, asset_info.sz_decimals, asset_info.price_decimals
                );

                Ok(Self {
                    config,
                    strategy,
                    asset_info,
                    info_client,
                    exchange_client: Some(exchange_client),
                    user_address: Some(user_address),
                    prices: HashMap::new(),
                    orders: HashMap::new(),
                    exchange_oid_to_order_id: HashMap::new(),
                    paper_state: None,
                })
            }
            MarketMode::Paper { initial_balance } => {
                // Paper trading always uses Mainnet for price data
                let info_client = InfoClient::new(None, Some(BaseUrl::Mainnet)).await?;
                let asset_info =
                    Self::fetch_asset_info(&info_client, &config.asset, None).await?;

                info!(
                    "Bot created for {} (paper), balance={}, sz_decimals={}, price_decimals={}",
                    config.asset, initial_balance, asset_info.sz_decimals, asset_info.price_decimals
                );

                Ok(Self {
                    config,
                    strategy,
                    asset_info,
                    info_client,
                    exchange_client: None,
                    user_address: None,
                    prices: HashMap::new(),
                    orders: HashMap::new(),
                    exchange_oid_to_order_id: HashMap::new(),
                    paper_state: Some(PaperState::new(initial_balance)),
                })
            }
        }
    }

    /// Fetch asset info from exchange
    async fn fetch_asset_info(
        info_client: &InfoClient,
        asset: &str,
        user_address: Option<Address>,
    ) -> Result<AssetInfo, crate::Error> {
        let is_spot = asset.contains('/');

        // Get balances (if we have user address)
        let (base_balance, usdc_balance) = if let Some(addr) = user_address {
            if is_spot {
                let balances = info_client.user_token_balances(addr).await?;
                let base_name = asset.split('/').next().unwrap_or(asset);

                let base_bal = balances
                    .balances
                    .iter()
                    .find(|b| b.coin == base_name)
                    .and_then(|b| b.total.parse::<f64>().ok())
                    .unwrap_or(0.0);

                let usdc_bal = balances
                    .balances
                    .iter()
                    .find(|b| b.coin == "USDC")
                    .and_then(|b| b.total.parse::<f64>().ok())
                    .unwrap_or(0.0);

                (base_bal, usdc_bal)
            } else {
                let state = info_client.user_state(addr).await?;
                let position = state
                    .asset_positions
                    .iter()
                    .find(|p| p.position.coin == asset)
                    .map(|p| p.position.szi.parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);
                let margin = state
                    .margin_summary
                    .account_value
                    .parse::<f64>()
                    .unwrap_or(0.0);
                (position, margin)
            }
        } else {
            (0.0, 0.0)
        };

        // Get precision
        let (sz_decimals, price_decimals) = if is_spot {
            let spot_meta = info_client.spot_meta().await?;
            let base_name = asset.split('/').next().unwrap_or(asset);

            let index_to_token: std::collections::HashMap<_, _> = spot_meta
                .tokens
                .iter()
                .map(|t| (t.index, t))
                .collect();

            let mut found_sz = 4u32;
            for spot_asset in &spot_meta.universe {
                if let Some(token) = index_to_token.get(&spot_asset.tokens[0]) {
                    if token.name == base_name || asset == spot_asset.name {
                        found_sz = token.sz_decimals as u32;
                        break;
                    }
                }
            }

            (found_sz, 6u32)
        } else {
            let meta = info_client.meta().await?;
            let asset_meta = meta
                .universe
                .iter()
                .find(|a| a.name == asset)
                .ok_or_else(|| crate::Error::AssetNotFound)?;

            (asset_meta.sz_decimals, 5u32)
        };

        Ok(AssetInfo::new(
            asset,
            base_balance,
            usdc_balance,
            sz_decimals,
            price_decimals,
        ))
    }

    /// Get the asset info
    pub fn asset_info(&self) -> &AssetInfo {
        &self.asset_info
    }

    /// Get the current price for the configured asset
    pub fn current_price(&self) -> Option<f64> {
        self.prices.get(&self.config.asset).copied()
    }

    /// Get all current prices
    pub fn all_prices(&self) -> &HashMap<String, f64> {
        &self.prices
    }

    /// Get order status
    pub fn order_status(&self, order_id: u64) -> Option<OrderStatus> {
        self.orders.get(&order_id).map(|o| o.status)
    }

    /// Check if running in paper mode
    pub fn is_paper(&self) -> bool {
        self.paper_state.is_some()
    }

    /// Get paper trading balance (None if live trading)
    pub fn paper_balance(&self) -> Option<f64> {
        self.paper_state.as_ref().map(|s| s.balance)
    }

    /// Get paper trading position (None if live trading)
    pub fn paper_position(&self) -> Option<f64> {
        self.paper_state.as_ref().map(|s| s.position)
    }

    /// Run the bot event loop
    ///
    /// This starts the bot and runs indefinitely, processing price updates
    /// and fills, calling the strategy, and executing orders.
    pub async fn run(&mut self) {
        let (sender, mut receiver) = unbounded_channel();

        // Subscribe to price updates
        if let Err(e) = self
            .info_client
            .subscribe(Subscription::AllMids, sender.clone())
            .await
        {
            error!("Failed to subscribe to AllMids: {e}");
            return;
        }

        // Subscribe to user fills (for live trading)
        if let Some(addr) = self.user_address {
            if let Err(e) = self
                .info_client
                .subscribe(Subscription::UserEvents { user: addr }, sender)
                .await
            {
                error!("Failed to subscribe to UserEvents: {e}");
                return;
            }
        }

        info!("Bot started for {}", self.config.asset);

        // Call strategy's on_start
        let initial_orders = self.strategy.on_start();
        for order in initial_orders {
            self.place_order(order).await;
        }

        // Main event loop
        loop {
            match receiver.recv().await {
                Some(message) => self.handle_message(message).await,
                None => {
                    error!("Channel closed");
                    break;
                }
            }
        }

        // Call strategy's on_stop
        let final_orders = self.strategy.on_stop();
        for order in final_orders {
            self.place_order(order).await;
        }
    }

    /// Handle incoming WebSocket message
    async fn handle_message(&mut self, message: Message) {
        match message {
            Message::AllMids(all_mids) => {
                for (asset, price_str) in all_mids.data.mids {
                    if let Ok(price) = price_str.parse::<f64>() {
                        self.prices.insert(asset.clone(), price);

                        // Only process for our configured asset
                        if asset == self.config.asset || self.is_asset_match(&asset) {
                            // Call strategy
                            let orders = self.strategy.on_price_update(&asset, price);

                            // Execute orders
                            for order in orders {
                                self.place_order(order).await;
                            }

                            // For paper trading, check if any orders should fill
                            if self.is_paper() {
                                self.check_paper_fills(price).await;
                            }
                        }
                    }
                }
            }
            Message::User(user_events) => {
                if let UserData::Fills(fills) = user_events.data {
                    for fill in fills {
                        let oid = fill.oid;
                        let qty: f64 = fill.sz.parse().unwrap_or(0.0);
                        let price: f64 = fill.px.parse().unwrap_or(0.0);

                        debug!(
                            "Fill received: oid={}, qty={}, price={}, side={}",
                            oid, qty, price, fill.side
                        );

                        // Find order by exchange OID
                        if let Some(&user_order_id) = self.exchange_oid_to_order_id.get(&oid) {
                            if let Some(order) = self.orders.get_mut(&user_order_id) {
                                let was_active = order.status.is_active();
                                order.fill(qty, price);

                                // Only notify strategy when fully filled
                                if was_active
                                    && matches!(order.status, OrderStatus::Filled(_))
                                {
                                    let order_fill = OrderFill::new(
                                        user_order_id,
                                        &fill.coin,
                                        order.request.qty,
                                        order.avg_fill_price,
                                    );

                                    info!(
                                        "Order {} filled: {} @ {}",
                                        user_order_id, order.request.qty, order.avg_fill_price
                                    );

                                    // Call strategy
                                    let new_orders = self.strategy.on_order_filled(&order_fill);

                                    // Execute new orders
                                    for new_order in new_orders {
                                        self.place_order(new_order).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                debug!("Received unhandled message type");
            }
        }
    }

    /// Check if asset matches (handles spot asset resolution)
    fn is_asset_match(&self, received_asset: &str) -> bool {
        // For spot assets, the feed might use @index format
        if self.config.asset.contains('/') {
            // Our asset is spot, received might be @123 format
            received_asset.starts_with('@')
        } else {
            false
        }
    }

    /// Place an order
    async fn place_order(&mut self, order: OrderRequest) {
        let order_id = order.order_id;
        let mut tracked = TrackedOrder::new(order.clone());

        if self.is_paper() {
            // Paper trading - just track the order
            self.orders.insert(order_id, tracked);
            info!(
                "Paper order {}: {:?} {} @ {}",
                order_id, order.side, order.qty, order.limit_price
            );
        } else if let Some(exchange_client) = &self.exchange_client {
            // Live trading - place on exchange
            let exchange_order = ClientOrderRequest {
                asset: self.config.asset.clone(),
                is_buy: order.side.is_buy(),
                reduce_only: order.reduce_only,
                limit_px: self.asset_info.round_price(order.limit_price, !order.side.is_buy()),
                sz: self.asset_info.round_size(order.qty),
                cloid: None,
                order_type: ClientOrder::Limit(ClientLimit {
                    tif: "Gtc".to_string(),
                }),
            };

            match exchange_client.order(exchange_order, None).await {
                Ok(ExchangeResponseStatus::Ok(resp)) => {
                    if let Some(data) = resp.data {
                        if !data.statuses.is_empty() {
                            match &data.statuses[0] {
                                ExchangeDataStatus::Filled(filled) => {
                                    tracked.exchange_oid = Some(filled.oid);
                                    tracked.status = OrderStatus::Filled(order.limit_price);
                                    self.exchange_oid_to_order_id.insert(filled.oid, order_id);

                                    info!("Order {} filled immediately", order_id);

                                    // Notify strategy
                                    let fill = OrderFill::new(
                                        order_id,
                                        &order.asset,
                                        order.qty,
                                        order.limit_price,
                                    );
                                    let new_orders = self.strategy.on_order_filled(&fill);
                                    
                                    // Store order first
                                    self.orders.insert(order_id, tracked);
                                    
                                    // Then place new orders
                                    for new_order in new_orders {
                                        Box::pin(self.place_order(new_order)).await;
                                    }
                                    return;
                                }
                                ExchangeDataStatus::Resting(resting) => {
                                    tracked.exchange_oid = Some(resting.oid);
                                    self.exchange_oid_to_order_id.insert(resting.oid, order_id);
                                    info!("Order {} resting, oid={}", order_id, resting.oid);
                                }
                                ExchangeDataStatus::Error(e) => {
                                    error!("Order {} error: {}", order_id, e);
                                    tracked.status = OrderStatus::Cancelled;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Ok(ExchangeResponseStatus::Err(e)) => {
                    error!("Order {} exchange error: {}", order_id, e);
                    tracked.status = OrderStatus::Cancelled;
                }
                Err(e) => {
                    error!("Order {} request error: {}", order_id, e);
                    tracked.status = OrderStatus::Cancelled;
                }
            }

            self.orders.insert(order_id, tracked);
        }
    }

    /// Check and execute paper fills
    async fn check_paper_fills(&mut self, current_price: f64) {
        let orders_to_fill: Vec<(u64, f64, OrderSide)> = self
            .orders
            .iter()
            .filter(|(_, order)| {
                if !order.status.is_active() {
                    return false;
                }
                match order.request.side {
                    OrderSide::Buy => current_price <= order.request.limit_price,
                    OrderSide::Sell => current_price >= order.request.limit_price,
                }
            })
            .map(|(&id, order)| (id, order.request.qty - order.filled_qty, order.request.side))
            .collect();

        for (order_id, qty, side) in orders_to_fill {
            self.execute_paper_fill(order_id, qty, current_price, side)
                .await;
        }
    }

    /// Execute a paper fill
    async fn execute_paper_fill(&mut self, order_id: u64, qty: f64, price: f64, side: OrderSide) {
        let is_buy = side.is_buy();

        // Update paper state
        if let Some(state) = &mut self.paper_state {
            let notional = qty * price;
            let fee = notional * state.fee_rate;

            if is_buy {
                state.balance -= notional + fee;
                state.position += qty;
            } else {
                state.balance += notional - fee;
                state.position -= qty;
            }
            state.total_fees += fee;
        }

        // Update order
        if let Some(order) = self.orders.get_mut(&order_id) {
            let was_active = order.status.is_active();
            order.fill(qty, price);

            info!(
                "Paper fill: {} {} @ {} (order {})",
                if is_buy { "bought" } else { "sold" },
                qty,
                price,
                order_id
            );

            // Notify strategy when fully filled
            if was_active && matches!(order.status, OrderStatus::Filled(_)) {
                let fill = OrderFill::new(
                    order_id,
                    &self.config.asset,
                    order.request.qty,
                    order.avg_fill_price,
                );

                let new_orders = self.strategy.on_order_filled(&fill);

                for new_order in new_orders {
                    Box::pin(self.place_order(new_order)).await;
                }
            }
        }
    }

    /// Cancel an order
    pub async fn cancel_order(&mut self, order_id: u64) -> bool {
        // Check if order exists and is active
        let (is_active, exchange_oid) = {
            let Some(order) = self.orders.get(&order_id) else {
                return false;
            };
            (order.status.is_active(), order.exchange_oid)
        };

        if !is_active {
            return false;
        }

        // Paper trading - just mark as cancelled
        if self.is_paper() {
            if let Some(order) = self.orders.get_mut(&order_id) {
                order.status = OrderStatus::Cancelled;
            }
            info!("Paper order {} cancelled", order_id);
            return true;
        }

        // Live trading - cancel on exchange
        if let (Some(exchange_client), Some(oid)) = (&self.exchange_client, exchange_oid) {
            let cancel = ClientCancelRequest {
                asset: self.config.asset.clone(),
                oid,
            };

            match exchange_client.cancel(cancel, None).await {
                Ok(ExchangeResponseStatus::Ok(resp)) => {
                    if let Some(data) = resp.data {
                        if let Some(ExchangeDataStatus::Success) = data.statuses.first() {
                            if let Some(order) = self.orders.get_mut(&order_id) {
                                order.status = OrderStatus::Cancelled;
                            }
                            info!("Order {} cancelled", order_id);
                            return true;
                        }
                    }
                }
                Ok(ExchangeResponseStatus::Err(e)) => {
                    error!("Cancel error: {}", e);
                }
                Err(e) => {
                    error!("Cancel request error: {}", e);
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_config_live() {
        // Can't test without a real wallet, but we can test config creation
        let config = BotConfig::paper("BTC", 10000.0);
        assert!(config.is_paper());
        assert!(!config.is_live());
        assert_eq!(config.asset, "BTC");
    }

    #[test]
    fn test_bot_config_paper() {
        let config = BotConfig::paper("HYPE/USDC", 50000.0);
        assert!(config.is_paper());
        assert_eq!(config.asset, "HYPE/USDC");
    }

    #[test]
    fn test_tracked_order_fill() {
        let request = OrderRequest::buy(1, "BTC", 2.0, 50000.0);
        let mut order = TrackedOrder::new(request);

        assert_eq!(order.status, OrderStatus::Pending);

        // Partial fill
        order.fill(1.0, 49900.0);
        assert_eq!(order.status, OrderStatus::PartiallyFilled(1.0));

        // Complete fill
        order.fill(1.0, 50100.0);
        match order.status {
            OrderStatus::Filled(avg) => {
                assert!((avg - 50000.0).abs() < 0.01);
            }
            _ => panic!("Expected Filled status"),
        }
    }

    #[test]
    fn test_paper_state() {
        let mut state = PaperState::new(10000.0);
        assert_eq!(state.balance, 10000.0);
        assert_eq!(state.position, 0.0);

        // Simulate a buy
        let qty = 1.0;
        let price = 100.0;
        let notional = qty * price;
        let fee = notional * state.fee_rate;

        state.balance -= notional + fee;
        state.position += qty;
        state.total_fees += fee;

        assert!(state.balance < 10000.0);
        assert_eq!(state.position, 1.0);
        assert!(state.total_fees > 0.0);
    }
}

