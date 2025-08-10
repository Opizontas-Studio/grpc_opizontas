use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::interval;
use tokio_util::task::TaskTracker;
use tonic::transport::{Channel, Endpoint, Uri};

// 连接池配置
#[derive(Debug, Clone)]
pub struct ConnectionPoolConfig {
    pub max_connections: usize,
    pub connection_ttl: Duration,
    pub idle_timeout: Duration,
    pub cleanup_interval: Duration,
}

impl Default for ConnectionPoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 100,
            connection_ttl: Duration::from_secs(300), // 5分钟
            idle_timeout: Duration::from_secs(60),    // 1分钟
            cleanup_interval: Duration::from_secs(30), // 30秒清理一次
        }
    }
}

// 连接元数据
#[derive(Debug, Clone)]
pub struct ConnectionMetadata {
    pub channel: Channel,
    pub created_at: Instant,
    pub last_used: Instant,
    pub use_count: u64,
}

impl ConnectionMetadata {
    pub fn new(channel: Channel) -> Self {
        let now = Instant::now();
        Self {
            channel,
            created_at: now,
            last_used: now,
            use_count: 0,
        }
    }

    pub fn touch(&mut self) {
        self.last_used = Instant::now();
        self.use_count += 1;
    }

    pub fn is_expired(&self, config: &ConnectionPoolConfig) -> bool {
        let now = Instant::now();
        now.duration_since(self.created_at) > config.connection_ttl
            || now.duration_since(self.last_used) > config.idle_timeout
    }
}

pub type ClientPool = Arc<DashMap<String, ConnectionMetadata>>;

#[derive(Debug)]
pub struct GrpcClientManager {
    pub clients: ClientPool,
    pub config: ConnectionPoolConfig,
    pub stats: Arc<DashMap<String, u64>>, // 连接统计
    task_tracker: TaskTracker,
}

impl Default for GrpcClientManager {
    fn default() -> Self {
        Self::new(ConnectionPoolConfig::default())
    }
}

impl GrpcClientManager {
    pub fn new(config: ConnectionPoolConfig) -> Self {
        let manager = Self {
            clients: Arc::new(DashMap::new()),
            config: config.clone(),
            stats: Arc::new(DashMap::new()),
            task_tracker: TaskTracker::new(),
        };

        // 启动清理任务
        manager.start_cleanup_task();

        manager
    }

    pub async fn get_or_create_client(
        &self,
        address: &str,
    ) -> Result<Channel, Box<dyn std::error::Error + Send + Sync>> {
        // 如果达到最大连接数限制，移除最老的连接
        if self.clients.len() >= self.config.max_connections {
            self.evict_oldest_connection().await;
        }

        // 尝试从缓存获取并更新使用时间
        if let Some(mut entry) = self.clients.get_mut(address) {
            if !entry.is_expired(&self.config) {
                entry.touch();
                self.increment_stat("cache_hits");
                return Ok(entry.channel.clone());
            } else {
                // 连接已过期，移除它
                drop(entry);
                self.clients.remove(address);
            }
        }

        self.increment_stat("cache_misses");

        // 创建新的客户端连接
        let uri: Uri = address
            .parse()
            .map_err(|e| format!("Invalid URI {address}: {e}"))?;
        let endpoint = Endpoint::from(uri);

        let channel = endpoint
            .connect()
            .await
            .map_err(|e| format!("Failed to connect to {address}: {e}"))?;

        // 将新连接加入缓存
        let metadata = ConnectionMetadata::new(channel.clone());
        self.clients.insert(address.to_string(), metadata);
        self.increment_stat("connections_created");

        tracing::info!(address = %address, total_clients = self.clients.len(), "Created new gRPC client connection");
        Ok(channel)
    }

    pub async fn remove_client(&self, address: &str) {
        if self.clients.remove(address).is_some() {
            self.increment_stat("connections_removed");
            tracing::info!(address = %address, "Removed gRPC client connection");
        }
    }

    pub async fn clear_all(&self) {
        let count = self.clients.len();
        self.clients.clear();
        self.stats.clear();
        tracing::info!(cleared_count = count, "Cleared gRPC client connections");
    }

    pub fn get_stats(&self) -> std::collections::HashMap<String, u64> {
        self.stats
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect()
    }

    fn increment_stat(&self, key: &str) {
        self.stats
            .entry(key.to_string())
            .and_modify(|v| *v += 1)
            .or_insert(1);
    }

    async fn evict_oldest_connection(&self) {
        let mut oldest_key: Option<String> = None;
        let mut oldest_time = Instant::now();

        for entry in self.clients.iter() {
            if entry.value().created_at < oldest_time {
                oldest_time = entry.value().created_at;
                oldest_key = Some(entry.key().clone());
            }
        }

        if let Some(key) = oldest_key {
            if self.clients.remove(&key).is_some() {
                self.increment_stat("connections_evicted");
                tracing::debug!(evicted_connection = %key, "Evicted oldest connection");
            }
        }
    }

    fn start_cleanup_task(&self) {
        let clients = self.clients.clone();
        let config = self.config.clone();
        let stats = self.stats.clone();

        self.task_tracker.spawn(async move {
            let mut cleanup_interval = interval(config.cleanup_interval);
            cleanup_interval.tick().await; // 跳过第一个tick

            loop {
                cleanup_interval.tick().await;

                let mut expired_keys = Vec::new();
                for entry in clients.iter() {
                    if entry.value().is_expired(&config) {
                        expired_keys.push(entry.key().clone());
                    }
                }

                let expired_count = expired_keys.len();
                for key in expired_keys {
                    clients.remove(&key);
                }

                if expired_count > 0 {
                    stats
                        .entry("connections_expired".to_string())
                        .and_modify(|v| *v += expired_count as u64)
                        .or_insert(expired_count as u64);
                    tracing::debug!(
                        removed_count = expired_count,
                        "Cleanup task removed expired connections"
                    );
                }
            }
        });
    }
}

// 实现 Clone trait 用于 DynamicRouter
impl Clone for GrpcClientManager {
    fn clone(&self) -> Self {
        Self {
            clients: self.clients.clone(),
            config: self.config.clone(),
            stats: self.stats.clone(),
            task_tracker: TaskTracker::new(), // 每个克隆都有自己的任务跟踪器
        }
    }
}
