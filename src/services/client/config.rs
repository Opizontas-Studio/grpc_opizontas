use std::time::Duration;

/// 网关客户端配置
#[derive(Debug, Clone)]
pub struct GatewayClientConfig {
    /// 网关地址
    pub gateway_address: String,
    /// 默认超时时间
    pub default_timeout: Duration,
    /// 连接超时时间
    pub connect_timeout: Duration,
    /// API 密钥
    pub api_key: String,
}

impl Default for GatewayClientConfig {
    fn default() -> Self {
        Self {
            gateway_address: "http://localhost:50051".to_string(),
            default_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            api_key: String::new(),
        }
    }
}
