use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::types::{ServiceHealthStatus, ServiceInfo, ServiceInstances, ServiceRegistry};
use crate::config::Config;
use crate::services::connection::{ReverseConnectionConfig, ReverseConnectionManager};

// 定义的服务实现
#[derive(Debug)]
pub struct MyRegistryService {
    pub registry: ServiceRegistry,
    pub config: Config,
    pub reverse_connection_manager: Arc<ReverseConnectionManager>,
}

impl MyRegistryService {
    pub fn new(config: Config) -> Self {
        // 创建反向连接管理器配置
        let reverse_config = ReverseConnectionConfig {
            heartbeat_timeout: Duration::from_secs(config.reverse_connection.heartbeat_timeout),
            request_timeout: Duration::from_secs(config.reverse_connection.request_timeout),
            cleanup_interval: Duration::from_secs(config.reverse_connection.cleanup_interval),
            max_pending_requests: config.reverse_connection.max_pending_requests,
        };

        let registry: ServiceRegistry = Arc::new(DashMap::new());
        let event_config = config.event.clone();

        let service = Self {
            registry: registry.clone(),
            config,
            reverse_connection_manager: Arc::new(ReverseConnectionManager::new(
                reverse_config,
                Some(registry),
                event_config,
            )),
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

    // 处理通过反向连接接收到的服务请求
    pub async fn handle_service_request(
        reverse_manager: Arc<ReverseConnectionManager>,
        request: crate::registry::ForwardRequest,
    ) -> Result<crate::registry::ForwardResponse, String> {
        // 从方法路径中提取服务名
        let service_name = Self::extract_service_name(&request.method_path)?;

        tracing::info!(
            request_id = %request.request_id,
            service_name = %service_name,
            method_path = %request.method_path,
            "Forwarding request to target service"
        );

        // 通过反向连接管理器转发请求
        reverse_manager
            .send_request_with_id(
                &request.request_id,
                &service_name,
                &request.method_path,
                request.headers,
                request.payload,
            )
            .await
    }

    // 从方法路径中提取服务名
    fn extract_service_name(method_path: &str) -> Result<String, String> {
        let parts: Vec<&str> = method_path.trim_start_matches('/').split('/').collect();
        if parts.is_empty() {
            return Err("Invalid method path format".to_string());
        }

        let service_part = parts[0];
        let service_parts: Vec<&str> = service_part.split('.').collect();

        if service_parts.len() < 2 {
            return Err("Invalid service path format".to_string());
        }

        // 返回完整的服务名称（包含包名），而不是只返回第一部分
        // 例如：从 "amwaybot.RecommendationService" 返回 "amwaybot.RecommendationService"
        // 而不是只返回 "amwaybot"
        Ok(service_part.to_string())
    }

    // 清理过期的服务实例
    async fn cleanup_expired_services(registry: &ServiceRegistry, timeout: Duration) {
        let now = SystemTime::now();
        let mut expired_instances = Vec::new();

        for service_entry in registry.iter() {
            let service_name = service_entry.key().clone();
            let instances = service_entry.value().clone();

            for instance_entry in instances.iter() {
                let instance_id = instance_entry.key().clone();
                let service_info = instance_entry.value().clone();

                if let Ok(elapsed) = now.duration_since(service_info.last_heartbeat) {
                    if elapsed > timeout {
                        tracing::warn!(
                            service_name = %service_name,
                            instance_id = %instance_id,
                            elapsed_secs = elapsed.as_secs(),
                            timeout_secs = timeout.as_secs(),
                            "Service instance expired due to heartbeat timeout"
                        );
                        expired_instances.push((service_name.clone(), instance_id));
                    } else {
                        tracing::debug!(
                            service_name = %service_name,
                            instance_id = %instance_id,
                            last_heartbeat_secs = elapsed.as_secs(),
                            timeout_secs = timeout.as_secs(),
                            "Service instance is healthy"
                        );
                    }
                }
            }
        }

        if !expired_instances.is_empty() {
            tracing::info!(
                expired_count = expired_instances.len(),
                "Cleanup check completed, removing expired service instances..."
            );
        }

        for (service_name, instance_id) in expired_instances {
            if let Some(service_entry) = registry.get(&service_name) {
                let instances = service_entry.clone();
                drop(service_entry);

                if instances.remove(&instance_id).is_some() {
                    tracing::info!(
                        service_name = %service_name,
                        instance_id = %instance_id,
                        "Removed expired service instance from registry"
                    );
                }

                if instances.is_empty() {
                    registry.remove_if(&service_name, |_, inner: &ServiceInstances| {
                        inner.is_empty()
                    });
                    tracing::info!(
                        service_name = %service_name,
                        "All instances expired, removed service from registry"
                    );
                }
            }
        }
    }

    // 获取所有健康的服务（返回第一个健康实例的地址）
    pub fn get_healthy_services(&self) -> HashMap<String, String> {
        let mut healthy = HashMap::new();

        for service_entry in self.registry.iter() {
            let service_name = service_entry.key().clone();
            let instances = service_entry.value().clone();

            for instance_entry in instances.iter() {
                if instance_entry.value().health_status == ServiceHealthStatus::Healthy {
                    healthy.insert(service_name.clone(), instance_entry.value().address.clone());
                    break;
                }
            }
        }

        healthy
    }

    // 获取服务信息（返回第一个实例）
    pub fn get_service_info(&self, service_name: &str) -> Option<ServiceInfo> {
        self.registry
            .get(service_name)
            .and_then(|instances| instances.iter().next().map(|entry| entry.value().clone()))
    }

    // 手动更新服务健康状态（应用到所有实例）
    pub fn update_service_health(&self, service_name: &str, status: ServiceHealthStatus) -> bool {
        if let Some(service_entry) = self.registry.get(service_name) {
            let instances = service_entry.clone();
            drop(service_entry);

            let mut updated = false;
            for mut instance in instances.iter_mut() {
                instance.value_mut().health_status = status.clone();
                updated = true;
            }

            if updated {
                tracing::info!(
                    service_name = %service_name,
                    new_status = ?status,
                    "Updated health status for all service instances"
                );
            }
            updated
        } else {
            false
        }
    }

    // 注销服务（移除全部实例）
    pub fn unregister_service(&self, service_name: &str) -> bool {
        if let Some((_name, instances)) = self.registry.remove(service_name) {
            instances.clear();
            tracing::info!(
                service_name = %service_name,
                "Unregistered service and cleared all instances"
            );
            true
        } else {
            false
        }
    }
}
