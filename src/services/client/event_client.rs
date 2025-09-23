use std::time::{SystemTime, Duration};
use tokio_stream::{Stream, StreamExt};
use uuid::Uuid;

use crate::registry::{
    ConnectionMessage, EventMessage, SubscriptionRequest,
    connection_message::MessageType,
    subscription_request::Action,
};
use super::GatewayClientError;
use crate::services::gateway_client::GatewayClient;

/// 事件客户端 - 基于现有 GatewayClient 扩展事件发布订阅功能
pub struct EventClient {
    gateway_client: GatewayClient,
    connection_id: String,
}

impl EventClient {
    /// 创建新的事件客户端
    pub fn new(gateway_client: GatewayClient, connection_id: String) -> Self {
        Self {
            gateway_client,
            connection_id,
        }
    }

    /// 发布事件到网关
    pub async fn publish_event(
        &self,
        event_type: &str,
        payload: Vec<u8>,
        metadata: Option<std::collections::HashMap<String, String>>,
    ) -> Result<(), GatewayClientError> {
        let event = EventMessage {
            event_id: Uuid::new_v4().to_string(),
            event_type: event_type.to_string(),
            publisher_id: self.connection_id.clone(),
            payload,
            timestamp: SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            metadata: metadata.unwrap_or_default(),
        };

        let message = ConnectionMessage {
            message_type: Some(MessageType::Event(event)),
        };

        self.send_connection_message(message).await
    }

    /// 订阅事件类型
    pub async fn subscribe_events(
        &self,
        event_types: Vec<&str>,
    ) -> Result<(), GatewayClientError> {
        let subscription = SubscriptionRequest {
            action: Action::Subscribe as i32,
            event_types: event_types.iter().map(|s| s.to_string()).collect(),
            subscriber_id: self.connection_id.clone(),
        };

        let message = ConnectionMessage {
            message_type: Some(MessageType::Subscription(subscription)),
        };

        self.send_connection_message(message).await
    }

    /// 取消订阅事件类型
    pub async fn unsubscribe_events(
        &self,
        event_types: Vec<&str>,
    ) -> Result<(), GatewayClientError> {
        let subscription = SubscriptionRequest {
            action: Action::Unsubscribe as i32,
            event_types: event_types.iter().map(|s| s.to_string()).collect(),
            subscriber_id: self.connection_id.clone(),
        };

        let message = ConnectionMessage {
            message_type: Some(MessageType::Subscription(subscription)),
        };

        self.send_connection_message(message).await
    }

    /// 发送连接消息的内部方法
    async fn send_connection_message(
        &self,
        message: ConnectionMessage,
    ) -> Result<(), GatewayClientError> {
        tracing::debug!(
            message_type = ?message.message_type,
            connection_id = %self.connection_id,
            "Sending connection message"
        );
        
        // 创建 GatewayClient 的可变引用来发送消息
        // 注意：这里需要克隆 GatewayClient，因为它实现了 Clone trait
        let mut client = self.gateway_client.clone();
        
        // 调用 GatewayClient 的 send_connection_message 方法，并添加错误处理
        match client.send_connection_message(message).await {
            Ok(()) => {
                tracing::info!(
                    connection_id = %self.connection_id,
                    "Connection message sent successfully"
                );
                Ok(())
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    connection_id = %self.connection_id,
                    "Failed to send connection message"
                );
                Err(e)
            }
        }
    }

    /// 带重试机制的发送连接消息
    async fn send_connection_message_with_retry(
        &self,
        message: ConnectionMessage,
        max_retries: u32,
    ) -> Result<(), GatewayClientError> {
        let mut attempts = 0;
        let mut backoff = Duration::from_millis(100); // 初始退避时间 100ms

        loop {
            match self.send_connection_message(message.clone()).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    attempts += 1;
                    
                    if attempts > max_retries {
                        tracing::error!(
                            error = %e,
                            attempts = attempts,
                            connection_id = %self.connection_id,
                            "Failed to send connection message after max retries"
                        );
                        return Err(e);
                    }

                    tracing::warn!(
                        error = %e,
                        attempt = attempts,
                        backoff_ms = backoff.as_millis(),
                        connection_id = %self.connection_id,
                        "Retrying connection message send"
                    );

                    // 指数退避，最大不超过5秒
                    tokio::time::sleep(backoff).await;
                    backoff = std::cmp::min(backoff * 2, Duration::from_secs(5));
                }
            }
        }
    }

    /// 发布事件（带重试）
    pub async fn publish_event_with_retry(
        &self,
        event_type: &str,
        payload: Vec<u8>,
        metadata: Option<std::collections::HashMap<String, String>>,
        max_retries: u32,
    ) -> Result<(), GatewayClientError> {
        let event = EventMessage {
            event_id: Uuid::new_v4().to_string(),
            event_type: event_type.to_string(),
            publisher_id: self.connection_id.clone(),
            payload,
            timestamp: SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            metadata: metadata.unwrap_or_default(),
        };

        let message = ConnectionMessage {
            message_type: Some(MessageType::Event(event)),
        };

        self.send_connection_message_with_retry(message, max_retries).await
    }
}

/// 事件客户端构建器
pub struct EventClientBuilder {
    gateway_client: Option<GatewayClient>,
    connection_id: Option<String>,
}

impl EventClientBuilder {
    pub fn new() -> Self {
        Self {
            gateway_client: None,
            connection_id: None,
        }
    }

    pub fn with_gateway_client(mut self, client: GatewayClient) -> Self {
        self.gateway_client = Some(client);
        self
    }

    pub fn with_connection_id(mut self, connection_id: String) -> Self {
        self.connection_id = Some(connection_id);
        self
    }

    pub fn build(self) -> Result<EventClient, String> {
        let gateway_client = self.gateway_client
            .ok_or("Gateway client is required")?;
        let connection_id = self.connection_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        Ok(EventClient::new(gateway_client, connection_id))
    }
}

impl Default for EventClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}