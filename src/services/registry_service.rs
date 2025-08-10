use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tonic::{Request, Response, Status};

use crate::config::Config;
use crate::registry::{
    RegisterRequest, RegisterResponse, registry_service_server::RegistryService,
};

// 服务注册表错误类型
#[derive(Debug)]
pub enum RegistryError {
    LockError(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::LockError(msg) => write!(f, "Registry lock error: {msg}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<RegistryError> for Status {
    fn from(error: RegistryError) -> Self {
        match error {
            RegistryError::LockError(msg) => {
                tracing::error!("Registry lock error: {}", msg);
                Status::internal(format!("Internal service error: {msg}"))
            }
        }
    }
}

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

// 定义增强的服务注册表
pub type ServiceRegistry = Arc<Mutex<HashMap<String, ServiceInfo>>>;

// 定义的服务实现
#[derive(Debug)]
pub struct MyRegistryService {
    pub registry: ServiceRegistry,
    pub config: Config,
}

impl MyRegistryService {
    pub fn new(config: Config) -> Self {
        let service = Self {
            registry: Arc::new(Mutex::new(HashMap::new())),
            config,
        };

        // 启动定期清理任务
        let registry_clone = service.registry.clone();
        let heartbeat_timeout = service.config.heartbeat_timeout();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_timeout);
            loop {
                interval.tick().await;
                tracing::debug!("Executing service expiration check...");
                Self::cleanup_expired_services(&registry_clone, heartbeat_timeout).await;
            }
        });

        service
    }
}

impl MyRegistryService {
    // 清理过期的服务
    async fn cleanup_expired_services(registry: &ServiceRegistry, timeout: Duration) {
        let mut registry_map = match registry.lock() {
            Ok(map) => map,
            Err(e) => {
                tracing::error!("Failed to acquire registry lock for cleanup: {}", e);
                return;
            }
        };
        let now = SystemTime::now();

        let mut to_remove = Vec::new();
        for (service_name, service_info) in registry_map.iter() {
            if let Ok(elapsed) = now.duration_since(service_info.last_heartbeat) {
                if elapsed > timeout {
                    tracing::warn!(service_name = %service_name, elapsed_secs = elapsed.as_secs(), "Service expired, removing from registry");
                    to_remove.push(service_name.clone());
                } else {
                    tracing::debug!(service_name = %service_name, last_heartbeat_secs = elapsed.as_secs(), "Service is healthy");
                }
            }
        }

        if to_remove.is_empty() {
        } else {
            tracing::info!(
                expired_count = to_remove.len(),
                "Cleanup check completed, removing expired services..."
            );
        }

        for service_name in to_remove {
            registry_map.remove(&service_name);
        }
    }

    // 获取所有健康的服务
    pub fn get_healthy_services(&self) -> Result<HashMap<String, String>, RegistryError> {
        let registry = match self.registry.lock() {
            Ok(registry) => registry,
            Err(e) => {
                let error_msg = format!(
                    "Failed to acquire registry lock in get_healthy_services: {e}"
                );
                tracing::error!("{}", error_msg);
                return Err(RegistryError::LockError(error_msg));
            }
        };
        Ok(registry
            .iter()
            .filter(|(_, info)| info.health_status == ServiceHealthStatus::Healthy)
            .map(|(name, info)| (name.clone(), info.address.clone()))
            .collect())
    }

    // 获取服务信息
    pub fn get_service_info(
        &self,
        service_name: &str,
    ) -> Result<Option<ServiceInfo>, RegistryError> {
        let registry = match self.registry.lock() {
            Ok(registry) => registry,
            Err(e) => {
                let error_msg =
                    format!("Failed to acquire registry lock in get_service_info: {e}");
                tracing::error!("{}", error_msg);
                return Err(RegistryError::LockError(error_msg));
            }
        };
        Ok(registry.get(service_name).cloned())
    }

    // 手动更新服务健康状态
    pub fn update_service_health(
        &self,
        service_name: &str,
        status: ServiceHealthStatus,
    ) -> Result<bool, RegistryError> {
        let mut registry = match self.registry.lock() {
            Ok(registry) => registry,
            Err(e) => {
                let error_msg = format!(
                    "Failed to acquire registry lock in update_service_health: {e}"
                );
                tracing::error!("{}", error_msg);
                return Err(RegistryError::LockError(error_msg));
            }
        };
        if let Some(service_info) = registry.get_mut(service_name) {
            tracing::info!(service_name = %service_name, new_status = ?status, "Updated health status for service");
            service_info.health_status = status;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // 注销服务
    pub fn unregister_service(&self, service_name: &str) -> Result<bool, RegistryError> {
        let mut registry = match self.registry.lock() {
            Ok(registry) => registry,
            Err(e) => {
                let error_msg = format!(
                    "Failed to acquire registry lock in unregister_service: {e}"
                );
                tracing::error!("{}", error_msg);
                return Err(RegistryError::LockError(error_msg));
            }
        };
        if registry.remove(service_name).is_some() {
            tracing::info!(service_name = %service_name, "Unregistered service");
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// 为结构体实现 gRPC 服务 trait
#[tonic::async_trait]
impl RegistryService for MyRegistryService {
    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();

        // 验证 Token
        if !self.config.validate_token(&req.api_key) {
            return Err(Status::unauthenticated("Invalid token"));
        }

        let mut registry = match self.registry.lock() {
            Ok(registry) => registry,
            Err(e) => {
                let error_msg = format!("Failed to acquire registry lock in register: {e}");
                tracing::error!("{}", error_msg);
                return Err(Status::internal(format!(
                    "Internal service error: {error_msg}"
                )));
            }
        };

        let service_info = ServiceInfo {
            address: req.address.clone(),
            last_heartbeat: SystemTime::now(),
            health_status: ServiceHealthStatus::Healthy,
        };

        for service_name in req.services {
            tracing::info!(
                service_name = %service_name,
                address = %req.address,
                "Registering service"
            );
            registry.insert(service_name, service_info.clone());
        }

        let reply = RegisterResponse {
            success: true,
            message: "Registration successful".into(),
        };

        Ok(Response::new(reply))
    }
}
