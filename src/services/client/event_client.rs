use std::time::SystemTime;
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
        // 这里需要实现发送逻辑，可能需要扩展 GatewayClient
        // 暂时返回 Ok，具体实现取决于 GatewayClient 的架构
        tracing::debug!(
            message_type = ?message.message_type,
            connection_id = %self.connection_id,
            "Sending connection message"
        );
        
        // TODO: 实现实际的消息发送逻辑
        // 这可能需要在 GatewayClient 中添加 send_connection_message 方法
        // 或者直接使用双向流发送消息
        
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_client_builder() {
        let builder = EventClientBuilder::new()
            .with_connection_id("test-connection".to_string());
        
        // 由于需要 GatewayClient，这个测试只验证构建器逻辑
        assert!(builder.gateway_client.is_none());
        assert_eq!(builder.connection_id.as_ref().unwrap(), "test-connection");
    }

    #[test]
    fn test_event_message_creation() {
        let event = EventMessage {
            event_id: "test-id".to_string(),
            event_type: "test.event".to_string(),
            publisher_id: "test-publisher".to_string(),
            payload: b"test payload".to_vec(),
            timestamp: 1234567890,
            metadata: std::collections::HashMap::new(),
        };

        assert_eq!(event.event_type, "test.event");
        assert_eq!(event.publisher_id, "test-publisher");
        assert_eq!(event.payload, b"test payload");
    }
}