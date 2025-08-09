use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tonic::{Request, Response, Status};

use crate::registry::{
    registry_service_server::RegistryService, RegisterRequest, RegisterResponse,
};

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
}

impl Default for MyRegistryService {
    fn default() -> Self {
        let service = Self {
            registry: Arc::new(Mutex::new(HashMap::new())),
        };

        // 启动定期清理任务
        let registry_clone = service.registry.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                println!("🔍 执行服务过期检查...");
                Self::cleanup_expired_services(&registry_clone).await;
            }
        });

        service
    }
}

impl MyRegistryService {
    // 清理过期的服务
    async fn cleanup_expired_services(registry: &ServiceRegistry) {
        let mut registry_map = registry.lock().unwrap();
        let now = SystemTime::now();
        let timeout = Duration::from_secs(60); // 60秒超时

        let mut to_remove = Vec::new();
        for (service_name, service_info) in registry_map.iter() {
            if let Ok(elapsed) = now.duration_since(service_info.last_heartbeat) {
                if elapsed > timeout {
                    println!("❌ Service '{}' expired after {}s, removing from registry", 
                           service_name, elapsed.as_secs());
                    to_remove.push(service_name.clone());
                } else {
                    println!("✅ Service '{}' is healthy (last heartbeat {}s ago)", 
                           service_name, elapsed.as_secs());
                }
            }
        }

        if to_remove.is_empty() {
        } else {
            println!("清理检查完成: 发现{}个过期服务，正在清理...", to_remove.len());
        }

        for service_name in to_remove {
            registry_map.remove(&service_name);
        }
    }

    // 获取所有健康的服务
    pub fn get_healthy_services(&self) -> HashMap<String, String> {
        let registry = self.registry.lock().unwrap();
        registry
            .iter()
            .filter(|(_, info)| info.health_status == ServiceHealthStatus::Healthy)
            .map(|(name, info)| (name.clone(), info.address.clone()))
            .collect()
    }

    // 获取服务信息
    pub fn get_service_info(&self, service_name: &str) -> Option<ServiceInfo> {
        let registry = self.registry.lock().unwrap();
        registry.get(service_name).cloned()
    }

    // 手动更新服务健康状态
    pub fn update_service_health(&self, service_name: &str, status: ServiceHealthStatus) -> bool {
        let mut registry = self.registry.lock().unwrap();
        if let Some(service_info) = registry.get_mut(service_name) {
            println!("Updated health status for service '{}': {:?}", service_name, status);
            service_info.health_status = status;
            true
        } else {
            false
        }
    }

    // 注销服务
    pub fn unregister_service(&self, service_name: &str) -> bool {
        let mut registry = self.registry.lock().unwrap();
        if registry.remove(service_name).is_some() {
            println!("Unregistered service: {}", service_name);
            true
        } else {
            false
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
        let mut registry = self.registry.lock().unwrap();

        let service_info = ServiceInfo {
            address: req.address.clone(),
            last_heartbeat: SystemTime::now(),
            health_status: ServiceHealthStatus::Healthy,
        };

        for service_name in req.services {
            println!(
                "Registering service '{}' at address '{}'",
                &service_name, &req.address
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