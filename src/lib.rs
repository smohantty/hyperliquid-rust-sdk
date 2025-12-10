#![deny(unreachable_pub)]
pub mod bot;
pub mod config;
pub mod runner;
mod consts;
mod eip712;
mod errors;
mod exchange;

mod helpers;
mod info;
pub mod market;
mod market_maker;
mod meta;
mod prelude;
mod req;
mod signature;
pub mod strategy;
mod ws;
pub use consts::{EPSILON, LOCAL_API_URL, MAINNET_API_URL, TESTNET_API_URL};
pub use eip712::Eip712;
pub use errors::Error;
pub use exchange::*;
pub use helpers::{bps_diff, truncate_float, BaseUrl};
pub use info::{info_client::*, *};
pub use market_maker::{MarketMaker, MarketMakerInput, MarketMakerRestingOrder};
pub use meta::{AssetContext, AssetMeta, Meta, MetaAndAssetCtxs, SpotAssetMeta, SpotMeta};
pub use ws::*;
