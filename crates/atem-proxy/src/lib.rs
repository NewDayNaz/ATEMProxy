//! Transparent ATEM UDP proxy: one upstream session, many native clients.

pub mod cache;
pub mod config;
pub mod filter;
pub mod locks;
pub mod mdns;
pub mod proxy;
pub mod server;
pub mod transfer;
pub mod upstream;

pub use config::Config;
pub use proxy::run_proxy;
