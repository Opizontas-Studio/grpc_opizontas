use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::{RwLock, mpsc};
use tokio_util::task::TaskTracker;

use crate::registry::ConnectionMessage;
use crate::services::event::{EventBus, EventConfig};
use crate::services::registry::types::{ServiceInstances, ServiceRegistry};

use super::{
    connection::ReverseConnection,
    service_pool::ServicePool,
    types::{PendingRequest, ReverseConnectionConfig, StreamingResponseHandler},
};

// 反向连接管理器
#[derive(Debug, Clone)]
pub struct ReverseConnectionManager {
    // 服务名 -> 反向连接映射
    pub(crate) connections_by_service: Arc<DashMap<String, ServicePool>>,
    // 连接ID -> 反向连接映射
    pub(crate) connections_by_id: Arc<DashMap<String, ReverseConnection>>,
    // 等待响应的请求
    pub(crate) pending_requests: Arc<RwLock<DashMap<String, PendingRequest>>>,
    // 流式响应处理器
    pub(crate) streaming_handlers: Arc<RwLock<DashMap<String, StreamingResponseHandler>>>,
    // 主服务注册表的引用，用于同步清理
    pub(crate) service_registry: Option<ServiceRegistry>,
    // 事件总线
    pub event_bus: Arc<EventBus>,
    pub(crate) config: ReverseConnectionConfig,
    pub(crate) task_tracker: Arc<TaskTracker>,
}

impl Default for ReverseConnectionManager {
    fn default() -> Self {
        Self::new(
            ReverseConnectionConfig::default(),
            None,
            EventConfig::default(),
        )
    }
}

impl ReverseConnectionManager {
    // 检查是否为有效的UUID格式连接ID
    fn is_valid_connection_id(id: &str) -> bool {
        // UUID格式：xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
        if id.len() != 36 {
            return false;
        }

        let parts: Vec<&str> = id.split('-').collect();
        if parts.len() != 5 {
            return false;
        }

        let expected_lengths = [8, 4, 4, 4, 12];
        for (i, part) in parts.iter().enumerate() {
            if part.len() != expected_lengths[i] {
                return false;
            }
            if !part.chars().all(|c| c.is_ascii_hexdigit()) {
                return false;
            }
        }

        true
    }

    pub fn new(
        config: ReverseConnectionConfig,
        service_registry: Option<ServiceRegistry>,
        event_config: EventConfig,
    ) -> Self {
        let manager = Self {
            connections_by_service: Arc::new(DashMap::new()),
            connections_by_id: Arc::new(DashMap::new()),
            pending_requests: Arc::new(RwLock::new(DashMap::new())),
            streaming_handlers: Arc::new(RwLock::new(DashMap::new())),
            service_registry,
            event_bus: Arc::new(EventBus::new(event_config)),
            config: config.clone(),
            task_tracker: Arc::new(TaskTracker::new()),
        };

        // 启动清理任务
        manager.start_cleanup_task();

        manager
    }

    // 注册新的反向连接 - 自动处理旧连接替换
    pub async fn register_connection(
        &self,
        connection_id: String,
        services: Vec<String>,
        request_sender: mpsc::UnboundedSender<ConnectionMessage>,
    ) -> Result<(), String> {
        let now = Instant::now();
        let new_connection = ReverseConnection {
            connection_id: connection_id.clone(),
            services: services.clone(),
            created_at: now,
            last_heartbeat: now,
            is_active: true,
            request_sender,
        };

        for service in &services {
            let pool = self
                .connections_by_service
                .entry(service.clone())
                .or_insert_with(ServicePool::new);

            match pool.add_connection(new_connection.clone()) {
                Some(_) => {
                    tracing::info!(
                        service_name = %service,
                        connection_id = %connection_id,
                        "Updated existing reverse connection instance in service pool"
                    );
                }
                None => {
                    tracing::info!(
                        service_name = %service,
                        connection_id = %connection_id,
                        pool_size = pool.len(),
                        "Registered reverse connection instance for service"
                    );
                }
            }
        }

        if let Some(old_connection) = self
            .connections_by_id
            .insert(connection_id.clone(), new_connection.clone())
        {
            tracing::info!(
                new_connection_id = %connection_id,
                old_connection_id = %old_connection.connection_id,
                "Replaced existing reverse connection with identical connection_id"
            );
            self.detach_connection(&old_connection);
        }

        Ok(())
    }

    fn detach_connection(&self, connection: &ReverseConnection) {
        for service in &connection.services {
            if let Some(pool_entry) = self.connections_by_service.get(service) {
                let pool = pool_entry.clone();
                drop(pool_entry);

                if pool.remove_connection(&connection.connection_id).is_some() {
                    tracing::info!(
                        service_name = %service,
                        connection_id = %connection.connection_id,
                        "Detached reverse connection instance from service pool"
                    );
                }

                if pool.is_empty() {
                    self.connections_by_service
                        .remove_if(service, |_, p| p.is_empty());
                    tracing::debug!(
                        service_name = %service,
                        connection_id = %connection.connection_id,
                        "Service pool empty after detaching connection, removed mapping"
                    );
                }
            }

            self.remove_service_registry_instance(service, &connection.connection_id);
        }
    }

    fn remove_service_registry_instance(&self, service_name: &str, instance_id: &str) {
        if let Some(ref service_registry) = self.service_registry {
            if let Some(instances_guard) = service_registry.get(service_name) {
                let instances = instances_guard.clone();
                drop(instances_guard);

                if instances.remove(instance_id).is_some() {
                    tracing::info!(
                        service_name = %service_name,
                        instance_id = %instance_id,
                        "Removed service instance from registry"
                    );
                }

                if instances.is_empty() {
                    service_registry
                        .remove_if(service_name, |_, v: &ServiceInstances| v.is_empty());
                    tracing::debug!(
                        service_name = %service_name,
                        "Service registry entry empty, removed service"
                    );
                }
            }
        }
    }

    // 注销反向连接
    pub async fn unregister_connection(&self, connection_id: &str) {
        if let Some((_id, connection)) = self.connections_by_id.remove(connection_id) {
            self.detach_connection(&connection);
            tracing::info!(
                connection_id = %connection_id,
                services = ?connection.services,
                "Unregistered reverse connection and cleaned up service mappings"
            );
        }
    }

    // 更新心跳
    pub async fn update_heartbeat(&self, connection_id: &str) {
        // 检查连接ID格式并记录诊断信息
        let is_valid_uuid = Self::is_valid_connection_id(connection_id);
        let is_empty = connection_id.is_empty();

        // 首先尝试按连接ID查找
        if let Some(mut connection) = self.connections_by_id.get_mut(connection_id) {
            let now = std::time::Instant::now();
            let old_heartbeat = connection.last_heartbeat;
            connection.update_heartbeat();
            let services = connection.services.clone();

            // 同时更新 connections_by_service 中的副本
            for service_name in &services {
                if let Some(pool) = self.connections_by_service.get(service_name) {
                    pool.update_connection(connection_id, |conn| conn.last_heartbeat = now);
                }
            }

            tracing::debug!(
                connection_id = %connection_id,
                services = ?services,
                old_heartbeat_elapsed_ms = %old_heartbeat.elapsed().as_millis(),
                new_heartbeat_set = %now.elapsed().as_millis(),
                "Updated heartbeat for reverse connection in both mappings"
            );

            // 同时更新服务注册表中对应服务的心跳时间戳
            self.update_service_registry_heartbeat(connection_id, &services)
                .await;
            return;
        }

        // 如果按连接ID找不到，尝试按服务名查找（兼容错误的客户端）
        if let Some(pool_ref) = self.connections_by_service.get(connection_id) {
            let pool = pool_ref.clone();
            drop(pool_ref);

            if let Some(actual_connection) = pool.next_connection(self.config.heartbeat_timeout) {
                let actual_connection_id = actual_connection.connection_id.clone();

                if let Some(mut connection) = self.connections_by_id.get_mut(&actual_connection_id)
                {
                    let now = std::time::Instant::now();
                    let old_heartbeat = connection.last_heartbeat;
                    connection.update_heartbeat();
                    let services = connection.services.clone();

                    // 同时更新 connections_by_service 中的副本
                    for service_name in &services {
                        if let Some(service_pool) = self.connections_by_service.get(service_name) {
                            service_pool.update_connection(&actual_connection_id, |conn| {
                                conn.last_heartbeat = now;
                            });
                        }
                    }

                    tracing::error!(
                        received_id = %connection_id,
                        actual_connection_id = %actual_connection_id,
                        services = ?services,
                        is_valid_uuid = %is_valid_uuid,
                        old_heartbeat_elapsed_ms = %old_heartbeat.elapsed().as_millis(),
                        new_heartbeat_set = %now.elapsed().as_millis(),
                        "CLIENT ERROR: Using service name as heartbeat ID! Client must use connection_id: '{}' for heartbeat, not service name: '{}'. Heartbeat updated in both mappings.",
                        actual_connection_id,
                        connection_id
                    );

                    // 同时更新服务注册表中对应服务的心跳时间戳
                    self.update_service_registry_heartbeat(&actual_connection_id, &services)
                        .await;
                    return;
                }
            }
        }

        // 提供详细的错误诊断和客户端指导
        if is_empty {
            tracing::error!(
                connection_id = %connection_id,
                "CLIENT ERROR: Empty connection_id in heartbeat! \n\
                 SOLUTION: Client must save and use the connection_id returned by EstablishReverseConnection"
            );
        } else if !is_valid_uuid {
            tracing::error!(
                connection_id = %connection_id,
                is_valid_uuid = %is_valid_uuid,
                "CLIENT ERROR: Invalid connection_id format in heartbeat! \n\
                 RECEIVED: '{}' (appears to be service name) \n\
                 SOLUTION: Use the UUID connection_id returned by EstablishReverseConnection, not service name",
                connection_id
            );
        } else {
            tracing::warn!(
                connection_id = %connection_id,
                "CONNECTION NOT FOUND: Valid UUID format but connection expired or not found \n\
                 SOLUTION: Client should re-establish connection and use new connection_id"
            );
        }
    }

    // 辅助方法：更新服务注册表中的心跳时间戳
    async fn update_service_registry_heartbeat(&self, connection_id: &str, services: &[String]) {
        if let Some(ref service_registry) = self.service_registry {
            let now = SystemTime::now();
            for service_name in services {
                if let Some(instances_guard) = service_registry.get(service_name) {
                    let instances = instances_guard.clone();
                    drop(instances_guard);

                    if let Some(mut instance) = instances.get_mut(connection_id) {
                        instance.value_mut().last_heartbeat = now;
                        tracing::debug!(
                            service_name = %service_name,
                            connection_id = %connection_id,
                            "Updated service instance heartbeat in registry"
                        );
                    }
                }
            }
        }
    }

    // 获取服务的反向连接 - 增强状态验证和诊断
    pub fn get_connection_for_service(&self, service_name: &str) -> Option<ReverseConnection> {
        if let Some(pool_ref) = self.connections_by_service.get(service_name) {
            let pool = pool_ref.clone();
            drop(pool_ref);

            if let Some(conn) = pool.next_connection(self.config.heartbeat_timeout) {
                let last_heartbeat_ago = conn.last_heartbeat.elapsed();

                tracing::debug!(
                    service_name = %service_name,
                    connection_id = %conn.connection_id,
                    last_heartbeat_ago_ms = %last_heartbeat_ago.as_millis(),
                    heartbeat_timeout_ms = %self.config.heartbeat_timeout.as_millis(),
                    pool_size = %pool.len(),
                    "Selected reverse connection instance for service"
                );

                return Some(conn);
            } else {
                tracing::warn!(
                    service_name = %service_name,
                    "No active reverse connections available in service pool"
                );
                self.connections_by_service
                    .remove_if(service_name, |_, p| p.is_empty());
            }
        } else {
            tracing::debug!(
                service_name = %service_name,
                "No direct service mapping found, trying hierarchical lookup"
            );
        }

        let result = self.find_connection_by_hierarchical_name(service_name);

        if result.is_none() {
            self.cleanup_orphaned_service_registry_entry(service_name);
        }

        result
    }

    // 检查是否存在可用的反向连接
    pub fn has_reverse_connection(&self, service_name: &str) -> bool {
        if self.has_direct_reverse_connection(service_name) {
            return true;
        }

        if service_name.contains('.') {
            return self.has_hierarchical_reverse_connection(service_name);
        }

        false
    }

    fn has_direct_reverse_connection(&self, service_name: &str) -> bool {
        if let Some(pool_ref) = self.connections_by_service.get(service_name) {
            let pool = pool_ref.clone();
            drop(pool_ref);

            if pool
                .next_connection(self.config.heartbeat_timeout)
                .is_some()
            {
                return true;
            }

            self.connections_by_service
                .remove_if(service_name, |_, p| p.is_empty());
        }

        false
    }

    fn has_hierarchical_reverse_connection(&self, service_name: &str) -> bool {
        let parts: Vec<&str> = service_name.split('.').collect();
        if parts.len() < 2 {
            return false;
        }

        for i in (1..parts.len()).rev() {
            let parent_name = parts[..i].join(".");
            if let Some(pool_ref) = self.connections_by_service.get(&parent_name) {
                let pool = pool_ref.clone();
                drop(pool_ref);

                if pool
                    .next_connection(self.config.heartbeat_timeout)
                    .is_some()
                {
                    tracing::debug!(
                        requested_service = %service_name,
                        matched_service = %parent_name,
                        "Found reverse connection during availability check via hierarchical lookup"
                    );
                    return true;
                }

                self.connections_by_service
                    .remove_if(&parent_name, |_, p| p.is_empty());
            }
        }

        false
    }

    // 清理孤立的服务注册表条目（没有对应反向连接的服务）
    fn cleanup_orphaned_service_registry_entry(&self, service_name: &str) {
        if let Some(ref service_registry) = self.service_registry {
            if service_registry
                .remove_if(service_name, |_, instances: &ServiceInstances| {
                    instances.is_empty()
                })
                .is_some()
            {
                tracing::warn!(
                    service_name = %service_name,
                    "CONSISTENCY FIX: Removed orphaned service from registry (no valid reverse connections remaining)"
                );
            }
        }
    }

    // 通过层级服务名称查找连接（支持 parent.child -> parent 的映射）
    fn find_connection_by_hierarchical_name(
        &self,
        service_name: &str,
    ) -> Option<ReverseConnection> {
        // 将服务名按 '.' 分割，从最长的父级开始尝试
        let parts: Vec<&str> = service_name.split('.').collect();

        // 从完整名称开始，逐步减少层级，直到找到匹配的服务
        for i in (1..parts.len()).rev() {
            let parent_name = parts[0..i].join(".");

            tracing::debug!(
                requested_service = %service_name,
                trying_parent = %parent_name,
                "Attempting hierarchical service name lookup"
            );

            if let Some(pool_ref) = self.connections_by_service.get(&parent_name) {
                let pool = pool_ref.clone();
                drop(pool_ref);

                if let Some(conn) = pool.next_connection(self.config.heartbeat_timeout) {
                    tracing::info!(
                        requested_service = %service_name,
                        matched_service = %parent_name,
                        "Found connection using hierarchical service name lookup"
                    );
                    return Some(conn);
                } else {
                    self.connections_by_service
                        .remove_if(&parent_name, |_, p| p.is_empty());
                }
            }
        }

        tracing::debug!(
            service_name = %service_name,
            "No connection found for service using hierarchical lookup"
        );
        None
    }


}

impl Drop for ReverseConnectionManager {
    fn drop(&mut self) {
        self.task_tracker.close();
    }
}
