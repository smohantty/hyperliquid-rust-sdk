use hyperliquid_rust_sdk::{
    bot::BotRunner,
    strategy::{
        spot_grid::SpotGridStrategyFactory, NoOpStrategy, Strategy, StrategyFactory,
        StrategyRegistry,
    },
};
use serde_json::Value;
use std::collections::HashMap;

// Define a factory for the NoOpStrategy
struct NoOpStrategyFactory;

impl StrategyFactory for NoOpStrategyFactory {
    fn create(
        &self,
        _asset: &str,
        _params: HashMap<String, Value>,
    ) -> Box<dyn Strategy + Send + Sync> {
        Box::new(NoOpStrategy)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize Registry
    let mut registry = StrategyRegistry::new();

    // 2. Register Strategies
    // In a real app, you'd register all your strategies here
    registry.register("noop", NoOpStrategyFactory);
    registry.register("spot_grid", SpotGridStrategyFactory);

    // 3. Create Runner
    let args: Vec<String> = std::env::args().collect();
    let default_config = "config.toml".to_string();
    let config_path = args.get(1).unwrap_or(&default_config);
    if !std::path::Path::new(config_path).exists() {
        eprintln!(
            "Config file '{}' not found. Please create one.",
            config_path
        );
        std::process::exit(1);
    }

    let runner = BotRunner::new(config_path, registry)?;

    // 4. Run
    if let Err(e) = runner.run().await {
        eprintln!("Bot execution error: {}", e);
        std::process::exit(1);
    }

    Ok(())
}
