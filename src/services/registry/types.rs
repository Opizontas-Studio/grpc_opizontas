use dashmap::DashMap;
use std::sync::Arc;
use std::time::SystemTime;

// 服务注册信息
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub address: String,
    pub last_heartbeat: SystemTime,
    pub health_status: ServiceHealthStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServiceHealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

pub type ServiceInstances = Arc<DashMap<String, ServiceInfo>>;

// 定义增强的服务注册表（服务名 -> 服务实例集合）
pub type ServiceRegistry = Arc<DashMap<String, ServiceInstances>>;
