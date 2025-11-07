use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::RwLock;

use crate::services::registry::types::{ServiceInstances, ServiceRegistry};
use super::{
    manager::ReverseConnectionManager,
    service_pool::ServicePool,
    types::PendingRequest,
    connection::ReverseConnection,
};

impl ReverseConnectionManager {
    // 启动清理任务
    pub(super) fn start_cleanup_task(&self) {
        let connections_by_service = self.connections_by_service.clone();
        let connections_by_id = self.connections_by_id.clone();
        let pending_requests = self.pending_requests.clone();
        let heartbeat_timeout = self.config.heartbeat_timeout;
        let request_timeout = self.config.request_timeout;
        let cleanup_interval = self.config.cleanup_interval;
        let service_registry = self.service_registry.clone();

        self.task_tracker.spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                interval.tick().await;
                Self::cleanup_expired_connections(
                    &connections_by_service,
                    &connections_by_id,
                    service_registry.clone(),
                    heartbeat_timeout,
                );
                Self::cleanup_expired_requests(&pending_requests, request_timeout).await;
            }
        });
    }

    // 清理过期连接
    fn cleanup_expired_connections(
        connections_by_service: &Arc<DashMap<String, ServicePool>>,
        connections_by_id: &Arc<DashMap<String, ReverseConnection>>,
        service_registry: Option<ServiceRegistry>,
        timeout: Duration,
    ) {
        let mut expired_connections = Vec::new();

        for entry in connections_by_id.iter() {
            let connection = entry.value();

            if connection.is_expired(timeout) {
                expired_connections.push(connection.clone());
            }
        }

        for connection in expired_connections {
            let connection_id = connection.connection_id.clone();
            let services = connection.services.clone();

            tracing::warn!(
                connection_id = %connection_id,
                services_count = services.len(),
                "Removing expired reverse connection"
            );

            connections_by_id.remove(&connection_id);
            for service in &services {
                if let Some(pool_entry) = connections_by_service.get(service) {
                    let pool = pool_entry.clone();
                    drop(pool_entry);

                    if pool.remove_connection(&connection_id).is_some() {
                        tracing::debug!(
                            service_name = %service,
                            connection_id = %connection_id,
                            "Removed expired reverse connection instance from service pool"
                        );
                    }

                    if pool.is_empty() {
                        connections_by_service.remove_if(service, |_, p| p.is_empty());
                        tracing::debug!(
                            service_name = %service,
                            "Service pool empty after removing expired connections, removed mapping"
                        );
                    }
                }

                if let Some(ref registry) = service_registry {
                    if let Some(instances_guard) = registry.get(service) {
                        let instances = instances_guard.clone();
                        drop(instances_guard);

                        if instances.remove(&connection_id).is_some() {
                            tracing::debug!(
                                service_name = %service,
                                connection_id = %connection_id,
                                "Removed expired service instance from registry"
                            );
                        }

                        if instances.is_empty() {
                            registry.remove_if(service, |_, v: &ServiceInstances| v.is_empty());
                        }
                    }
                }
            }
        }
    }

    // 清理过期请求
    async fn cleanup_expired_requests(
        pending_requests: &Arc<RwLock<DashMap<String, PendingRequest>>>,
        timeout: Duration,
    ) {
        let pending_requests_guard = pending_requests.read().await;
        let now = Instant::now();
        let mut expired_requests = Vec::new();

        // 收集过期请求
        for entry in pending_requests_guard.iter() {
            let request_id = entry.key();
            let request = entry.value();

            if now.duration_since(request.created_at) > timeout {
                expired_requests.push(request_id.clone());
            }
        }

        // 移除过期请求
        for request_id in expired_requests {
            if let Some((_id, _request)) = pending_requests_guard.remove(&request_id) {
                tracing::warn!(request_id = %request_id, "Removing expired pending request");
            }
        }
    }
}