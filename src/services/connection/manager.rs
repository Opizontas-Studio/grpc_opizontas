use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::registry::{ConnectionMessage, ForwardRequest, ForwardResponse};
use crate::services::registry::types::ServiceRegistry;

use super::{
    connection::ReverseConnection,
    types::{PendingRequest, ReverseConnectionConfig, StreamingResponseHandler},
};

// 反向连接管理器
#[derive(Debug, Clone)]
pub struct ReverseConnectionManager {
    // 服务名 -> 反向连接映射
    connections_by_service: Arc<DashMap<String, ReverseConnection>>,
    // 连接ID -> 反向连接映射
    connections_by_id: Arc<DashMap<String, ReverseConnection>>,
    // 等待响应的请求
    pending_requests: Arc<RwLock<DashMap<String, PendingRequest>>>,
    // 流式响应处理器
    streaming_handlers: Arc<RwLock<DashMap<String, StreamingResponseHandler>>>,
    // 主服务注册表的引用，用于同步清理
    service_registry: Option<ServiceRegistry>,
    config: ReverseConnectionConfig,
    task_tracker: Arc<TaskTracker>,
}

impl Default for ReverseConnectionManager {
    fn default() -> Self {
        Self::new(ReverseConnectionConfig::default(), None)
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

    pub fn new(config: ReverseConnectionConfig, service_registry: Option<ServiceRegistry>) -> Self {
        let manager = Self {
            connections_by_service: Arc::new(DashMap::new()),
            connections_by_id: Arc::new(DashMap::new()),
            pending_requests: Arc::new(RwLock::new(DashMap::new())),
            streaming_handlers: Arc::new(RwLock::new(DashMap::new())),
            service_registry,
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

                // 同时从主服务注册表中移除该服务
                if let Some(ref service_registry) = self.service_registry {
                    if service_registry.remove(service).is_some() {
                        tracing::info!(
                            service_name = %service,
                            connection_id = %connection_id,
                            "Removed service from main registry due to reverse connection disconnection"
                        );
                    }
                }

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
        // 检查连接ID格式并记录诊断信息
        let is_valid_uuid = Self::is_valid_connection_id(connection_id);
        let is_empty = connection_id.is_empty();
        
        // 首先尝试按连接ID查找
        if let Some(mut connection) = self.connections_by_id.get_mut(connection_id) {
            connection.update_heartbeat();
            
            tracing::debug!(
                connection_id = %connection_id,
                services = ?connection.services,
                "Updated heartbeat for reverse connection"
            );
            
            // 同时更新服务注册表中对应服务的心跳时间戳
            self.update_service_registry_heartbeat(&connection.services).await;
            return;
        }
        
        // 如果按连接ID找不到，尝试按服务名查找（兼容错误的客户端）
        if let Some(connection_ref) = self.connections_by_service.get(connection_id) {
            let actual_connection_id = connection_ref.connection_id.clone();
            drop(connection_ref); // 释放读锁
            
            if let Some(mut connection) = self.connections_by_id.get_mut(&actual_connection_id) {
                connection.update_heartbeat();
                
                tracing::warn!(
                    received_id = %connection_id,
                    actual_connection_id = %actual_connection_id,
                    services = ?connection.services,
                    is_valid_uuid = %is_valid_uuid,
                    "COMPATIBILITY FIX: Updated heartbeat using service name fallback - CLIENT SHOULD USE ACTUAL CONNECTION_ID"
                );
                
                // 同时更新服务注册表中对应服务的心跳时间戳
                self.update_service_registry_heartbeat(&connection.services).await;
                return;
            }
        }
        
        // 提供详细的错误诊断
        if is_empty {
            tracing::error!(
                connection_id = %connection_id,
                "Received heartbeat with EMPTY connection_id - client must provide valid UUID from gateway"
            );
        } else if !is_valid_uuid {
            tracing::error!(
                connection_id = %connection_id,
                is_valid_uuid = %is_valid_uuid,
                "Received heartbeat with INVALID connection_id format - client must use UUID from gateway, not service name"
            );
        } else {
            tracing::warn!(
                connection_id = %connection_id,
                "Received heartbeat for unknown connection_id (valid UUID format but connection not found)"
            );
        }
    }

    // 辅助方法：更新服务注册表中的心跳时间戳
    async fn update_service_registry_heartbeat(&self, services: &[String]) {
        if let Some(ref service_registry) = self.service_registry {
            let now = std::time::SystemTime::now();
            for service_name in services {
                if let Some(mut service_info) = service_registry.get_mut(service_name) {
                    service_info.last_heartbeat = now;
                    tracing::debug!(
                        service_name = %service_name,
                        "Updated service heartbeat in registry"
                    );
                }
            }
        }
    }

    // 获取服务的反向连接
    pub fn get_connection_for_service(&self, service_name: &str) -> Option<ReverseConnection> {
        // 首先尝试精确匹配
        if let Some(conn) = self.connections_by_service
            .get(service_name)
            .filter(|conn| conn.is_active)
        {
            return Some(conn.clone());
        }

        // 如果精确匹配失败，尝试层级服务名称查找
        self.find_connection_by_hierarchical_name(service_name)
    }

    // 通过层级服务名称查找连接（支持 parent.child -> parent 的映射）
    fn find_connection_by_hierarchical_name(&self, service_name: &str) -> Option<ReverseConnection> {
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
            
            if let Some(conn) = self.connections_by_service
                .get(&parent_name)
                .filter(|conn| conn.is_active)
            {
                tracing::info!(
                    requested_service = %service_name,
                    matched_service = %parent_name,
                    "Found connection using hierarchical service name lookup"
                );
                return Some(conn.clone());
            }
        }

        tracing::debug!(
            service_name = %service_name,
            "No connection found for service using hierarchical lookup"
        );
        None
    }

    // 发送请求到微服务并等待响应
    pub async fn send_request(
        &self,
        service_name: &str,
        method_path: &str,
        headers: std::collections::HashMap<String, String>,
        payload: Vec<u8>,
    ) -> Result<ForwardResponse, String> {
        // 生成新的请求ID
        let request_id = Uuid::new_v4().to_string();
        self.send_request_with_id(&request_id, service_name, method_path, headers, payload)
            .await
    }

    // 流式发送请求到微服务并等待响应
    pub async fn send_request_stream<B>(
        &self,
        service_name: &str,
        method_path: &str,
        headers: std::collections::HashMap<String, String>,
        body: B,
    ) -> Result<ForwardResponse, String>
    where
        B: http_body::Body<Data = bytes::Bytes> + Send + 'static,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + std::fmt::Debug,
    {
        use http_body_util::BodyExt;

        // 对于现有的API兼容性，仍然需要收集body
        // 但这里可以添加大小限制来避免无限内存使用
        let collected = body
            .collect()
            .await
            .map_err(|e| format!("Failed to collect request body: {e:?}"))?;
        let payload = collected.to_bytes().to_vec();

        // 如果payload太大，可以在这里添加大小检查
        const MAX_BODY_SIZE: usize = 100 * 1024 * 1024; // 100MB
        if payload.len() > MAX_BODY_SIZE {
            return Err(format!(
                "Request body too large: {} bytes (max: {} bytes)",
                payload.len(),
                MAX_BODY_SIZE
            ));
        }

        self.send_request(service_name, method_path, headers, payload)
            .await
    }

    // 使用指定的请求ID发送请求到微服务并等待响应
    pub async fn send_request_with_id(
        &self,
        request_id: &str,
        service_name: &str,
        method_path: &str,
        headers: std::collections::HashMap<String, String>,
        payload: Vec<u8>,
    ) -> Result<ForwardResponse, String> {
        // 获取连接
        let connection = self
            .get_connection_for_service(service_name)
            .ok_or_else(|| format!("No reverse connection found for service: {service_name}"))?;

        // 使用传入的请求ID
        let request_id = request_id.to_string();

        // 创建响应通道
        let (response_sender, response_receiver) = tokio::sync::oneshot::channel();

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
            streaming_info: Some(crate::registry::StreamingInfo {
                stream_type: crate::registry::streaming_info::StreamType::Unary as i32,
                is_stream_end: true,
                sequence_number: 0,
                chunk_size: 0,
            }),
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
        // 检查是否为流式响应
        if Self::is_streaming_response(&response) {
            self.handle_streaming_response(response).await;
        } else {
            // 处理常规响应
            let pending_requests = self.pending_requests.read().await;
            if let Some((_id, pending)) = pending_requests.remove(&response.request_id) {
                if pending.response_sender.send(response).is_err() {
                    tracing::warn!(request_id = %pending.request_id, "Failed to send response to waiting client");
                }
            } else {
                tracing::warn!(request_id = %response.request_id, "Received response for unknown request");
            }
        }
    }

    // 处理流式响应
    async fn handle_streaming_response(&self, response: ForwardResponse) {
        let stream_info = match response.response_stream_info.as_ref() {
            Some(info) => info,
            None => {
                tracing::error!(request_id = %response.request_id, "Missing stream info in streaming response");
                return;
            }
        };

        let streaming_handlers = self.streaming_handlers.write().await;

        // 检查是否已有处理器
        if !streaming_handlers.contains_key(&response.request_id) {
            // 这是一个新的流式响应，需要从 pending_requests 中获取 sender
            let pending_requests = self.pending_requests.read().await;
            if let Some((_id, pending)) = pending_requests.remove(&response.request_id) {
                let handler = StreamingResponseHandler {
                    request_id: response.request_id.clone(),
                    chunks: std::collections::BTreeMap::new(),
                    next_expected_chunk: 0,
                    is_complete: false,
                    total_size: stream_info.total_size,
                    response_sender: pending.response_sender,
                };
                streaming_handlers.insert(response.request_id.clone(), handler);
            } else {
                tracing::warn!(request_id = %response.request_id, "No pending request found for streaming response");
                return;
            }
        }

        let mut handler = match streaming_handlers.get_mut(&response.request_id) {
            Some(h) => h,
            None => return,
        };

        // 添加数据块
        handler
            .chunks
            .insert(stream_info.chunk_index, response.payload.clone());

        // 检查是否可以组装完整响应
        if stream_info.is_final_chunk {
            handler.is_complete = true;
        }

        // 尝试组装完整的响应
        if handler.is_complete {
            let mut complete_payload = Vec::new();
            for chunk_index in 0..=stream_info.chunk_index {
                if let Some(chunk_data) = handler.chunks.remove(&chunk_index) {
                    complete_payload.extend(chunk_data);
                } else {
                    tracing::error!(request_id = %response.request_id, chunk_index, "Missing chunk in streaming response");
                    return;
                }
            }

            // 创建完整的响应
            let complete_response = ForwardResponse {
                request_id: response.request_id.clone(),
                status_code: response.status_code,
                headers: response.headers,
                payload: complete_payload,
                error_message: response.error_message,
                streaming_info: response.streaming_info,
                response_stream_info: None, // 清除流式信息，因为这是最终的完整响应
            };

            // 发送完整响应
            if let Some((_id, handler)) = streaming_handlers.remove(&response.request_id) {
                if handler.response_sender.send(complete_response).is_err() {
                    tracing::warn!(request_id = %response.request_id, "Failed to send complete streaming response to waiting client");
                }
            }
        }
    }

    // 创建流式响应块
    pub fn create_response_chunk(
        request_id: String,
        chunk_data: Vec<u8>,
        chunk_index: i64,
        is_final: bool,
        total_size: Option<i64>,
    ) -> ForwardResponse {
        let chunk_size = chunk_data.len() as i32;
        ForwardResponse {
            request_id,
            status_code: 200,
            headers: std::collections::HashMap::new(),
            payload: chunk_data,
            error_message: String::new(),
            streaming_info: None,
            response_stream_info: Some(crate::registry::ResponseStreamInfo {
                is_streamed: true,
                chunk_index,
                is_final_chunk: is_final,
                chunk_size,
                total_size,
            }),
        }
    }

    // 检查响应是否为流式响应
    pub fn is_streaming_response(response: &ForwardResponse) -> bool {
        response
            .response_stream_info
            .as_ref()
            .map(|info| info.is_streamed)
            .unwrap_or(false)
    }

    // 检查服务是否支持反向连接
    pub fn has_reverse_connection(&self, service_name: &str) -> bool {
        self.connections_by_service.contains_key(service_name)
    }

    // 获取连接统计信息
    pub fn get_stats(&self) -> super::types::ConnectionStats {
        super::types::ConnectionStats {
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
            tracing::warn!(
                connection_id = %connection_id,
                services_count = services.len(),
                "Removing expired reverse connection"
            );

            connections_by_id.remove(&connection_id);
            for service in &services {
                connections_by_service.remove(service);
                tracing::debug!(
                    service_name = %service,
                    connection_id = %connection_id,
                    "Removed expired reverse connection for service"
                );
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

impl Drop for ReverseConnectionManager {
    fn drop(&mut self) {
        self.task_tracker.close();
    }
}