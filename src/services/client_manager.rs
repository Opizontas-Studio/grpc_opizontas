use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::transport::{Channel, Endpoint, Uri};

pub type ClientPool = Arc<RwLock<HashMap<String, Channel>>>;

#[derive(Debug, Clone)]
pub struct GrpcClientManager {
    pub clients: ClientPool,
}

impl Default for GrpcClientManager {
    fn default() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl GrpcClientManager {
    pub async fn get_or_create_client(&self, address: &str) -> Result<Channel, Box<dyn std::error::Error + Send + Sync>> {
        // 先尝试从缓存获取客户端
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(address) {
                return Ok(client.clone());
            }
        }

        // 缓存中没有，创建新的客户端连接
        let uri: Uri = address.parse().map_err(|e| format!("Invalid URI {}: {}", address, e))?;
        let endpoint = Endpoint::from(uri);
        
        let channel = endpoint
            .connect()
            .await
            .map_err(|e| format!("Failed to connect to {}: {}", address, e))?;

        // 将新连接加入缓存
        {
            let mut clients = self.clients.write().await;
            clients.insert(address.to_string(), channel.clone());
        }

        println!("Created new gRPC client connection to: {}", address);
        Ok(channel)
    }

    pub async fn remove_client(&self, address: &str) {
        let mut clients = self.clients.write().await;
        if clients.remove(address).is_some() {
            println!("Removed gRPC client connection to: {}", address);
        }
    }

    pub async fn clear_all(&self) {
        let mut clients = self.clients.write().await;
        let count = clients.len();
        clients.clear();
        println!("Cleared {} gRPC client connections", count);
    }
}