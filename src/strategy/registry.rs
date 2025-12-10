use std::collections::HashMap;
use serde_json::Value;
use super::Strategy;

/// Factory trait for creating strategies
pub trait StrategyFactory: Send + Sync {
    /// Create a new strategy instance with the given asset and parameters
    fn create(&self, asset: &str, params: HashMap<String, Value>) -> Box<dyn Strategy + Send + Sync>;
}

/// Registry for strategy factories
pub struct StrategyRegistry {
    factories: HashMap<String, Box<dyn StrategyFactory>>,
}

impl StrategyRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register a strategy factory
    pub fn register<F>(&mut self, name: &str, factory: F)
    where
        F: StrategyFactory + 'static,
    {
        self.factories.insert(name.to_string(), Box::new(factory));
    }

    /// Create a strategy by name
    pub fn create_strategy(
        &self,
        name: &str,
        asset: &str,
        params: HashMap<String, Value>,
    ) -> Option<Box<dyn Strategy + Send + Sync>> {
        self.factories.get(name).map(|f| f.create(asset, params))
    }
}

// Add Default impl
impl Default for StrategyRegistry {
    fn default() -> Self {
        Self::new()
    }
}
