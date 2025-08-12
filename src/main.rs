pub mod registry {
    tonic::include_proto!("registry");
}
pub mod config;
pub mod server;
pub mod services;
pub mod gateway_client;

use jemallocator::Jemalloc;
use tracing_subscriber::{EnvFilter, FmtSubscriber};
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化 tracing
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    tracing::info!("Starting gateway server...");
    server::start().await?;
    Ok(())
}
