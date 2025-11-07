use std::collections::HashMap;
use std::time::Instant;

use uuid::Uuid;

use crate::registry::{
    ConnectionMessage, ForwardRequest, ForwardResponse, StreamingInfo,
    connection_message::MessageType,
};
use super::{
    manager::ReverseConnectionManager,
    types::{PendingRequest, StreamingResponseHandler},
};

impl ReverseConnectionManager {
    // 发送请求到微服务并等待响应
    pub async fn send_request(
        &self,
        service_name: &str,
        method_path: &str,
        headers: HashMap<String, String>,
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
        headers: HashMap<String, String>,
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
        headers: HashMap<String, String>,
        payload: Vec<u8>,
    ) -> Result<ForwardResponse, String> {
        // 获取连接
        let connection = self
            .get_connection_for_service(service_name)
            .ok_or_else(|| {
                tracing::error!(
                    service_name = %service_name,
                    method_path = %method_path,
                    request_id = %request_id,
                    "No reverse connection found for service"
                );
                format!("No reverse connection found for service: {service_name}")
            })?;

        // 使用传入的请求ID
        let request_id = request_id.to_string();
        let payload_size = payload.len();

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
            streaming_info: Some(StreamingInfo {
                stream_type: crate::registry::streaming_info::StreamType::Unary as i32,
                is_stream_end: true,
                sequence_number: 0,
                chunk_size: 0,
            }),
        };

        let message = ConnectionMessage {
            message_type: Some(MessageType::Request(forward_request)),
        };

        tracing::debug!(
            service_name = %service_name,
            method_path = %method_path,
            request_id = %request_id,
            connection_id = %connection.connection_id,
            payload_size = payload_size,
            "Sending request via reverse connection"
        );

        // 发送请求到微服务
        if connection.request_sender.send(message).is_err() {
            // 移除等待中的请求
            let pending_requests = self.pending_requests.read().await;
            pending_requests.remove(&request_id);

            tracing::error!(
                service_name = %service_name,
                method_path = %method_path,
                request_id = %request_id,
                connection_id = %connection.connection_id,
                "Failed to send request to microservice - connection channel closed"
            );

            return Err("Failed to send request to microservice".to_string());
        }

        // 等待响应（带超时）
        match tokio::time::timeout(self.config.request_timeout, response_receiver).await {
            Ok(Ok(response)) => {
                tracing::debug!(
                    service_name = %service_name,
                    method_path = %method_path,
                    request_id = %request_id,
                    status_code = response.status_code,
                    response_size = response.payload.len(),
                    "Received response via reverse connection"
                );
                Ok(response)
            }
            Ok(Err(_)) => {
                // 移除等待中的请求
                let pending_requests = self.pending_requests.read().await;
                pending_requests.remove(&request_id);

                tracing::error!(
                    service_name = %service_name,
                    method_path = %method_path,
                    request_id = %request_id,
                    "Response channel closed - microservice disconnected unexpectedly"
                );

                Err("Response channel closed".to_string())
            }
            Err(_) => {
                // 移除等待中的请求
                let pending_requests = self.pending_requests.read().await;
                pending_requests.remove(&request_id);

                tracing::error!(
                    service_name = %service_name,
                    method_path = %method_path,
                    request_id = %request_id,
                    timeout_ms = self.config.request_timeout.as_millis(),
                    "Request timeout - microservice did not respond in time"
                );

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
            payload: chunk_data,
            response_stream_info: Some(crate::registry::ResponseStreamInfo {
                is_streamed: true,
                chunk_index,
                is_final_chunk: is_final,
                chunk_size,
                total_size,
            }),
            ..Default::default()
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

    // 处理事件消息
    pub async fn handle_event_message(
        &self,
        event: crate::registry::EventMessage,
    ) -> Result<usize, String> {
        match self.event_bus.publish_event(event).await {
            Ok(subscriber_count) => Ok(subscriber_count),
            Err(err) => {
                tracing::warn!(error = %err, "Failed to publish event");
                Err(format!("Failed to publish event: {}", err))
            }
        }
    }

    // 处理订阅请求
    pub async fn handle_subscription_request(
        &self,
        subscription: crate::registry::SubscriptionRequest,
    ) -> Result<(), String> {
        match self
            .event_bus
            .handle_subscription_request(subscription)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                tracing::warn!(error = %err, "Failed to handle subscription request");
                Err(format!("Failed to handle subscription: {}", err))
            }
        }
    }

    // 为连接创建事件流
    pub fn create_event_stream_for_connection(
        &self,
        connection_id: &str,
        event_type: &str,
    ) -> Result<
        impl tokio_stream::Stream<Item = Result<crate::registry::EventMessage, tonic::Status>>,
        String,
    > {
        match self
            .event_bus
            .subscribe_event_type(event_type, connection_id)
        {
            Ok(stream) => Ok(stream),
            Err(err) => {
                tracing::warn!(
                    connection_id = %connection_id,
                    event_type = %event_type,
                    error = %err,
                    "Failed to create event stream"
                );
                Err(format!("Failed to create event stream: {}", err))
            }
        }
    }

    // 清理连接的所有订阅
    pub async fn cleanup_connection_subscriptions(&self, connection_id: &str) {
        self.event_bus.remove_subscriber(connection_id).await;
        tracing::debug!(
            connection_id = %connection_id,
            "Cleaned up event subscriptions for connection"
        );
    }
}
