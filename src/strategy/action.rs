//! Strategy action types

use crate::market::OrderRequest;

/// Action returned by a strategy in response to market events
///
/// Contains a list of orders that should be executed on the market.
/// The strategy doesn't know or care how/where these orders are executed.
#[derive(Debug, Clone, Default)]
pub struct StrategyAction {
    /// Orders to be placed on the market
    pub orders: Vec<OrderRequest>,
}

impl StrategyAction {
    /// Create an empty action (no orders)
    pub fn none() -> Self {
        Self { orders: vec![] }
    }

    /// Create an action with a single order
    pub fn single(order: OrderRequest) -> Self {
        Self {
            orders: vec![order],
        }
    }

    /// Create an action with multiple orders
    pub fn multiple(orders: Vec<OrderRequest>) -> Self {
        Self { orders }
    }

    /// Check if this action has any orders
    pub fn has_orders(&self) -> bool {
        !self.orders.is_empty()
    }

    /// Get the number of orders
    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    /// Add an order to this action (builder pattern)
    pub fn with_order(mut self, order: OrderRequest) -> Self {
        self.orders.push(order);
        self
    }

    /// Merge another action into this one
    pub fn merge(mut self, other: StrategyAction) -> Self {
        self.orders.extend(other.orders);
        self
    }

    /// Take all orders, leaving this action empty
    pub fn take_orders(&mut self) -> Vec<OrderRequest> {
        std::mem::take(&mut self.orders)
    }

    /// Iterate over orders
    pub fn iter(&self) -> impl Iterator<Item = &OrderRequest> {
        self.orders.iter()
    }
}

impl From<OrderRequest> for StrategyAction {
    fn from(order: OrderRequest) -> Self {
        Self::single(order)
    }
}

impl From<Vec<OrderRequest>> for StrategyAction {
    fn from(orders: Vec<OrderRequest>) -> Self {
        Self::multiple(orders)
    }
}

impl From<Option<OrderRequest>> for StrategyAction {
    fn from(order: Option<OrderRequest>) -> Self {
        match order {
            Some(o) => Self::single(o),
            None => Self::none(),
        }
    }
}

impl IntoIterator for StrategyAction {
    type Item = OrderRequest;
    type IntoIter = std::vec::IntoIter<OrderRequest>;

    fn into_iter(self) -> Self::IntoIter {
        self.orders.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_none() {
        let action = StrategyAction::none();
        assert!(!action.has_orders());
        assert_eq!(action.order_count(), 0);
    }

    #[test]
    fn test_action_single() {
        let order = OrderRequest::buy(1, "BTC", 1.0, 50000.0);
        let action = StrategyAction::single(order);
        assert!(action.has_orders());
        assert_eq!(action.order_count(), 1);
    }

    #[test]
    fn test_action_multiple() {
        let orders = vec![
            OrderRequest::buy(1, "BTC", 1.0, 50000.0),
            OrderRequest::sell(2, "BTC", 1.0, 51000.0),
        ];
        let action = StrategyAction::multiple(orders);
        assert_eq!(action.order_count(), 2);
    }

    #[test]
    fn test_action_builder() {
        let action = StrategyAction::none()
            .with_order(OrderRequest::buy(1, "BTC", 1.0, 50000.0))
            .with_order(OrderRequest::sell(2, "BTC", 1.0, 51000.0));
        assert_eq!(action.order_count(), 2);
    }

    #[test]
    fn test_action_merge() {
        let action1 = StrategyAction::single(OrderRequest::buy(1, "BTC", 1.0, 50000.0));
        let action2 = StrategyAction::single(OrderRequest::sell(2, "ETH", 1.0, 3000.0));
        let merged = action1.merge(action2);
        assert_eq!(merged.order_count(), 2);
    }

    #[test]
    fn test_action_from_option() {
        let action: StrategyAction = Some(OrderRequest::buy(1, "BTC", 1.0, 50000.0)).into();
        assert!(action.has_orders());

        let action: StrategyAction = None.into();
        assert!(!action.has_orders());
    }

    #[test]
    fn test_action_iter() {
        let action = StrategyAction::multiple(vec![
            OrderRequest::buy(1, "BTC", 1.0, 50000.0),
            OrderRequest::buy(2, "ETH", 2.0, 3000.0),
        ]);

        let order_ids: Vec<u64> = action.iter().map(|o| o.order_id).collect();
        assert_eq!(order_ids, vec![1, 2]);
    }

    #[test]
    fn test_action_into_iter() {
        let action = StrategyAction::multiple(vec![
            OrderRequest::buy(1, "BTC", 1.0, 50000.0),
            OrderRequest::buy(2, "ETH", 2.0, 3000.0),
        ]);

        let orders: Vec<OrderRequest> = action.into_iter().collect();
        assert_eq!(orders.len(), 2);
    }
}

