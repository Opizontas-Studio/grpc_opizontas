use serde::{Deserialize, Serialize};
use std::fs;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub security: SecurityConfig,
    pub router: RouterConfig,
    pub connection_pool: ConnectionPoolConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub heartbeat_timeout: u64,
    pub request_timeout: u64,
    pub retry_attempts: u32,
    pub max_concurrent_requests: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionPoolConfig {
    pub max_connections: usize,
    pub connection_ttl: u64,
    pub idle_timeout: u64,
    pub cleanup_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub address: String,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub tokens: Vec<String>,
}

// 环境变量配置结构
#[derive(Debug, Deserialize)]
struct EnvConfig {
    #[serde(default)]
    grpc_security_tokens: Option<String>,
    #[serde(default)]
    grpc_router_heartbeat_timeout: Option<u64>,
    #[serde(default)]
    grpc_router_request_timeout: Option<u64>,
    #[serde(default)]
    grpc_router_retry_attempts: Option<u32>,
    #[serde(default)]
    grpc_router_max_concurrent_requests: Option<usize>,
    #[serde(default)]
    grpc_pool_max_connections: Option<usize>,
    #[serde(default)]
    grpc_pool_connection_ttl: Option<u64>,
    #[serde(default)]
    grpc_pool_idle_timeout: Option<u64>,
    #[serde(default)]
    grpc_pool_cleanup_interval: Option<u64>,
    #[serde(default)]
    grpc_server_address: Option<String>,
    #[serde(default)]
    grpc_log_level: Option<String>,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        // 加载 .env 文件（如果存在）
        let _ = dotenvy::dotenv();
        
        // 从文件加载基础配置
        let mut config = Self::load_from_file().unwrap_or_else(|_| Self::default());
        
        // 应用环境变量覆盖
        config.apply_env_overrides()?;
        
        Ok(config)
    }
    
    fn load_from_file() -> Result<Self, Box<dyn std::error::Error>> {
        let config_str = fs::read_to_string("config.toml")?;
        let config: Config = toml::from_str(&config_str)?;
        Ok(config)
    }
    
    fn apply_env_overrides(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let env_config: EnvConfig = envy::from_env()?;
        
        // 安全配置覆盖
        if let Some(tokens_str) = env_config.grpc_security_tokens {
            self.security.tokens = tokens_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        
        // 路由配置覆盖
        if let Some(val) = env_config.grpc_router_heartbeat_timeout {
            self.router.heartbeat_timeout = val;
        }
        if let Some(val) = env_config.grpc_router_request_timeout {
            self.router.request_timeout = val;
        }
        if let Some(val) = env_config.grpc_router_retry_attempts {
            self.router.retry_attempts = val;
        }
        if let Some(val) = env_config.grpc_router_max_concurrent_requests {
            self.router.max_concurrent_requests = val;
        }
        
        // 连接池配置覆盖
        if let Some(val) = env_config.grpc_pool_max_connections {
            self.connection_pool.max_connections = val;
        }
        if let Some(val) = env_config.grpc_pool_connection_ttl {
            self.connection_pool.connection_ttl = val;
        }
        if let Some(val) = env_config.grpc_pool_idle_timeout {
            self.connection_pool.idle_timeout = val;
        }
        if let Some(val) = env_config.grpc_pool_cleanup_interval {
            self.connection_pool.cleanup_interval = val;
        }
        
        // 服务器配置覆盖
        if let Some(val) = env_config.grpc_server_address {
            self.server.address = val;
        }
        if let Some(val) = env_config.grpc_log_level {
            self.server.log_level = val;
        }
        
        Ok(())
    }
    
    pub fn validate_token(&self, token: &str) -> bool {
        self.security.tokens.contains(&token.to_string())
    }
    
    // 获取路由配置的便利方法
    pub fn heartbeat_timeout(&self) -> Duration {
        Duration::from_secs(self.router.heartbeat_timeout)
    }
    
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.router.request_timeout)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            security: SecurityConfig {
                tokens: vec![], // 默认无 token，必须通过环境变量设置
            },
            router: RouterConfig {
                heartbeat_timeout: 120,
                request_timeout: 30,
                retry_attempts: 3,
                max_concurrent_requests: 1000,
            },
            connection_pool: ConnectionPoolConfig {
                max_connections: 100,
                connection_ttl: 300,
                idle_timeout: 60,
                cleanup_interval: 30,
            },
            server: ServerConfig {
                address: "0.0.0.0:50051".to_string(),
                log_level: "info".to_string(),
            },
        }
    }
}