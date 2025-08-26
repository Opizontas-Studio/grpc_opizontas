use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::services::connection::{ReverseConnectionManager, ReverseConnectionConfig};
use super::types::{ServiceHealthStatus, ServiceInfo, ServiceRegistry};
use crate::config::Config;

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

        let registry = Arc::new(dashmap::DashMap::new());
        
        let service = Self {
            registry: registry.clone(),
            config,
            reverse_connection_manager: Arc::new(ReverseConnectionManager::new(reverse_config, Some(registry))),
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

    // 清理过期的服务
    async fn cleanup_expired_services(registry: &ServiceRegistry, timeout: Duration) {
        let now = SystemTime::now();
        let mut to_remove = Vec::new();

        // 收集需要删除的服务
        for entry in registry.iter() {
            let service_name = entry.key();
            let service_info = entry.value();

            if let Ok(elapsed) = now.duration_since(service_info.last_heartbeat) {
                if elapsed > timeout {
                    tracing::warn!(
                        service_name = %service_name, 
                        elapsed_secs = elapsed.as_secs(),
                        timeout_secs = timeout.as_secs(),
                        "Service expired due to heartbeat timeout, removing from registry"
                    );
                    to_remove.push(service_name.clone());
                } else {
                    tracing::debug!(
                        service_name = %service_name, 
                        last_heartbeat_secs = elapsed.as_secs(),
                        timeout_secs = timeout.as_secs(),
                        "Service is healthy"
                    );
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

        // 删除过期的服务
        for service_name in to_remove {
            if registry.remove(&service_name).is_some() {
                tracing::info!(
                    service_name = %service_name,
                    "Successfully removed expired service from registry"
                );
            }
        }
    }

    // 获取所有健康的服务
    pub fn get_healthy_services(&self) -> HashMap<String, String> {
        self.registry
            .iter()
            .filter(|entry| entry.value().health_status == ServiceHealthStatus::Healthy)
            .map(|entry| (entry.key().clone(), entry.value().address.clone()))
            .collect()
    }

    // 获取服务信息
    pub fn get_service_info(&self, service_name: &str) -> Option<ServiceInfo> {
        self.registry
            .get(service_name)
            .map(|entry| entry.value().clone())
    }

    // 手动更新服务健康状态
    pub fn update_service_health(&self, service_name: &str, status: ServiceHealthStatus) -> bool {
        if let Some(mut entry) = self.registry.get_mut(service_name) {
            tracing::info!(service_name = %service_name, new_status = ?status, "Updated health status for service");
            entry.health_status = status;
            true
        } else {
            false
        }
    }

    // 注销服务
    pub fn unregister_service(&self, service_name: &str) -> bool {
        if self.registry.remove(service_name).is_some() {
            tracing::info!(service_name = %service_name, "Unregistered service");
            true
        } else {
            false
        }
    }
}