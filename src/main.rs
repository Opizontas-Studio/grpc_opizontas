pub mod registry {
    tonic::include_proto!("registry");
}
pub mod server;
pub mod services;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting gateway server...");
    server::start().await?;
    Ok(())
}
