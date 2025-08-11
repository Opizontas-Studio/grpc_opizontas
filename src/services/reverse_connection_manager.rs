use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::registry::{ConnectionMessage, ForwardRequest, ForwardResponse};

// 反向连接元数据
#[derive(Debug, Clone)]
pub struct ReverseConnection {
    pub connection_id: String,
    pub services: Vec<String>,
    pub created_at: Instant,
    pub last_heartbeat: Instant,
    pub is_active: bool,
    // 用于向微服务发送请求的发送端
    pub request_sender: mpsc::UnboundedSender<ConnectionMessage>,
}

impl ReverseConnection {

    pub fn update_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    pub fn is_expired(&self, timeout: Duration) -> bool {
        Instant::now().duration_since(self.last_heartbeat) > timeout
    }
}

// 等待中的请求
#[derive(Debug)]
pub struct PendingRequest {
    pub request_id: String,
    pub created_at: Instant,
    pub response_sender: oneshot::Sender<ForwardResponse>,
}

// 反向连接管理器配置
#[derive(Debug, Clone)]
pub struct ReverseConnectionConfig {
    pub heartbeat_timeout: Duration,
    pub request_timeout: Duration,
    pub cleanup_interval: Duration,
    pub max_pending_requests: usize,
}

impl Default for ReverseConnectionConfig {
    fn default() -> Self {
        Self {
            heartbeat_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(30),
            cleanup_interval: Duration::from_secs(60),
            max_pending_requests: 1000,
        }
    }
}

// 反向连接管理器
#[derive(Debug)]
pub struct ReverseConnectionManager {
    // 服务名 -> 反向连接映射
    connections_by_service: Arc<DashMap<String, ReverseConnection>>,
    // 连接ID -> 反向连接映射
    connections_by_id: Arc<DashMap<String, ReverseConnection>>,
    // 等待响应的请求
    pending_requests: Arc<RwLock<DashMap<String, PendingRequest>>>,
    config: ReverseConnectionConfig,
    task_tracker: Arc<TaskTracker>,
}

impl Default for ReverseConnectionManager {
    fn default() -> Self {
        Self::new(ReverseConnectionConfig::default())
    }
}

impl ReverseConnectionManager {
    pub fn new(config: ReverseConnectionConfig) -> Self {
        let manager = Self {
            connections_by_service: Arc::new(DashMap::new()),
            connections_by_id: Arc::new(DashMap::new()),
            pending_requests: Arc::new(RwLock::new(DashMap::new())),
            config: config.clone(),
            task_tracker: Arc::new(TaskTracker::new()),
        };

        // 启动清理任务
        manager.start_cleanup_task();

        manager
    }

    // 注册新的反向连接
    pub async fn register_connection(
        &self,
        connection_id: String,
        services: Vec<String>,
        request_sender: mpsc::UnboundedSender<ConnectionMessage>,
    ) -> Result<(), String> {
        let now = Instant::now();
        let connection = ReverseConnection {
            connection_id: connection_id.clone(),
            services: services.clone(),
            created_at: now,
            last_heartbeat: now,
            is_active: true,
            request_sender,
        };

        // 按连接ID存储
        self.connections_by_id
            .insert(connection_id.clone(), connection.clone());

        // 按服务名存储
        for service in services {
            tracing::info!(
                service_name = %service,
                connection_id = %connection_id,
                "Registered reverse connection for service"
            );
            self.connections_by_service
                .insert(service, connection.clone());
        }

        Ok(())
    }

    // 注销反向连接
    pub async fn unregister_connection(&self, connection_id: &str) {
        if let Some((_id, connection)) = self.connections_by_id.remove(connection_id) {
            // 从服务映射中移除
            for service in &connection.services {
                self.connections_by_service.remove(service);
                tracing::info!(
                    service_name = %service,
                    connection_id = %connection_id,
                    "Unregistered reverse connection for service"
                );
            }
        }
    }

    // 更新心跳
    pub async fn update_heartbeat(&self, connection_id: &str) {
        if let Some(mut connection) = self.connections_by_id.get_mut(connection_id) {
            connection.update_heartbeat();
        }
    }

    // 获取服务的反向连接
    pub fn get_connection_for_service(&self, service_name: &str) -> Option<ReverseConnection> {
        self.connections_by_service
            .get(service_name)
            .filter(|conn| conn.is_active)
            .map(|conn| conn.clone())
    }

    // 发送请求到微服务并等待响应
    pub async fn send_request(
        &self,
        service_name: &str,
        method_path: &str,
        headers: std::collections::HashMap<String, String>,
        payload: Vec<u8>,
    ) -> Result<ForwardResponse, String> {
        // 获取连接
        let connection = self
            .get_connection_for_service(service_name)
            .ok_or_else(|| format!("No reverse connection found for service: {service_name}"))?;

        // 生成请求ID
        let request_id = Uuid::new_v4().to_string();

        // 创建响应通道
        let (response_sender, response_receiver) = oneshot::channel();

        // 存储等待中的请求
        {
            let pending_requests = self.pending_requests.read().await;
            if pending_requests.len() >= self.config.max_pending_requests {
                return Err("Too many pending requests".to_string());
            }
            pending_requests.insert(
                request_id.clone(),
                PendingRequest {
                    request_id: request_id.clone(),
                    created_at: Instant::now(),
                    response_sender,
                },
            );
        }

        // 构建转发请求
        let forward_request = ForwardRequest {
            request_id: request_id.clone(),
            method_path: method_path.to_string(),
            headers,
            payload,
            timeout_seconds: self.config.request_timeout.as_secs() as i32,
        };

        let message = ConnectionMessage {
            message_type: Some(crate::registry::connection_message::MessageType::Request(
                forward_request,
            )),
        };

        // 发送请求到微服务
        if connection.request_sender.send(message).is_err() {
            // 移除等待中的请求
            let pending_requests = self.pending_requests.read().await;
            pending_requests.remove(&request_id);
            return Err("Failed to send request to microservice".to_string());
        }

        // 等待响应（带超时）
        match tokio::time::timeout(self.config.request_timeout, response_receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                // 移除等待中的请求
                let pending_requests = self.pending_requests.read().await;
                pending_requests.remove(&request_id);
                Err("Response channel closed".to_string())
            }
            Err(_) => {
                // 移除等待中的请求
                let pending_requests = self.pending_requests.read().await;
                pending_requests.remove(&request_id);
                Err("Request timeout".to_string())
            }
        }
    }

    // 处理来自微服务的响应
    pub async fn handle_response(&self, response: ForwardResponse) {
        let pending_requests = self.pending_requests.read().await;
        if let Some((_id, pending)) = pending_requests.remove(&response.request_id) {
            if pending.response_sender.send(response).is_err() {
                tracing::warn!(request_id = %pending.request_id, "Failed to send response to waiting client");
            }
        } else {
            tracing::warn!(request_id = %response.request_id, "Received response for unknown request");
        }
    }

    // 检查服务是否支持反向连接
    pub fn has_reverse_connection(&self, service_name: &str) -> bool {
        self.connections_by_service.contains_key(service_name)
    }

    // 获取连接统计信息
    pub fn get_stats(&self) -> ConnectionStats {
        ConnectionStats {
            active_connections: self.connections_by_id.len(),
            registered_services: self.connections_by_service.len(),
        }
    }

    // 启动清理任务
    fn start_cleanup_task(&self) {
        let connections_by_service = self.connections_by_service.clone();
        let connections_by_id = self.connections_by_id.clone();
        let pending_requests = self.pending_requests.clone();
        let heartbeat_timeout = self.config.heartbeat_timeout;
        let request_timeout = self.config.request_timeout;
        let cleanup_interval = self.config.cleanup_interval;

        self.task_tracker.spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                interval.tick().await;
                Self::cleanup_expired_connections(
                    &connections_by_service,
                    &connections_by_id,
                    heartbeat_timeout,
                )
                .await;
                Self::cleanup_expired_requests(&pending_requests, request_timeout).await;
            }
        });
    }

    // 清理过期连接
    async fn cleanup_expired_connections(
        connections_by_service: &Arc<DashMap<String, ReverseConnection>>,
        connections_by_id: &Arc<DashMap<String, ReverseConnection>>,
        timeout: Duration,
    ) {
        let mut expired_connections = Vec::new();

        // 收集过期连接
        for entry in connections_by_id.iter() {
            let connection_id = entry.key();
            let connection = entry.value();

            if connection.is_expired(timeout) {
                expired_connections.push((connection_id.clone(), connection.services.clone()));
            }
        }

        // 移除过期连接
        for (connection_id, services) in expired_connections {
            tracing::warn!(connection_id = %connection_id, "Removing expired reverse connection");

            connections_by_id.remove(&connection_id);
            for service in services {
                connections_by_service.remove(&service);
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

// 连接统计信息
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub active_connections: usize,
    pub registered_services: usize,
}

impl Drop for ReverseConnectionManager {
    fn drop(&mut self) {
        self.task_tracker.close();
    }
}
