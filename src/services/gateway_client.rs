use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Channel, Endpoint};
use uuid::Uuid;

use crate::registry::{
    ForwardRequest, StreamingInfo,
    streaming_info::StreamType,
    registry_service_client::RegistryServiceClient,
};
use super::client::{
    GatewayClientConfig, GatewayClientError
};

/// 网关客户端
#[derive(Debug, Clone)]
pub struct GatewayClient {
    pub(crate) config: GatewayClientConfig,
    pub(crate) client: RegistryServiceClient<Channel>,
}

impl GatewayClient {
    /// 创建新的网关客户端
    pub async fn new(config: GatewayClientConfig) -> Result<Self, GatewayClientError> {
        let endpoint = Endpoint::from_shared(config.gateway_address.clone())?
            .connect_timeout(config.connect_timeout)
            .timeout(config.default_timeout);
            
        let channel = endpoint.connect().await?;
        let client = RegistryServiceClient::new(channel);

        Ok(Self { config, client })
    }

    /// 便捷的创建方法，使用默认配置
    pub async fn connect(gateway_address: &str) -> Result<Self, GatewayClientError> {
        let config = GatewayClientConfig {
            gateway_address: gateway_address.to_string(),
            ..Default::default()
        };
        Self::new(config).await
    }

    /// 一元调用：调用指定服务的方法
    pub async fn call_unary<T, R>(
        &mut self,
        service_name: &str,
        method_path: &str,
        request: T,
    ) -> Result<R, GatewayClientError>
    where
        T: prost::Message,
        R: prost::Message + Default,
    {
        let mut headers = HashMap::new();
        headers.insert("x-service-name".to_string(), service_name.to_string());
        
        let payload = self.serialize_message(&request)?;
        
        let forward_request = ForwardRequest {
            request_id: Uuid::new_v4().to_string(),
            method_path: method_path.to_string(),
            headers,
            payload,
            timeout_seconds: self.config.default_timeout.as_secs() as i32,
            streaming_info: Some(StreamingInfo {
                stream_type: StreamType::Unary as i32,
                is_stream_end: true,
                sequence_number: 0,
                chunk_size: 0,
            }),
        };

        // 通过gRPC双向流发送单个请求
        let message = crate::registry::ConnectionMessage {
            message_type: Some(crate::registry::connection_message::MessageType::Request(forward_request)),
        };

        // 创建单次请求流
        let request_stream = tokio_stream::once(message);
        let response = self.client.establish_connection(request_stream).await?;
        let mut inbound = response.into_inner();

        // 等待第一个响应
        while let Some(message_result) = inbound.next().await {
            match message_result {
                Ok(message) => {
                    if let Some(crate::registry::connection_message::MessageType::Response(response)) = message.message_type {
                        return self.deserialize_response(response.payload);
                    }
                }
                Err(e) => return Err(GatewayClientError::Grpc(e)),
            }
        }
        
        Err(GatewayClientError::Timeout)
    }

    /// 客户端流调用
    pub async fn call_client_stream<T, R>(
        &mut self,
        service_name: &str, 
        method_path: &str,
        requests: impl Iterator<Item = T> + Send + 'static,
    ) -> Result<R, GatewayClientError>
    where
        T: prost::Message + Send + 'static,
        R: prost::Message + Default,
    {
        let (request_tx, request_rx) = mpsc::channel(100);
        
        // 发送所有请求
        super::client::streaming::send_client_stream_requests(self, requests, request_tx, service_name, method_path);
        
        // 建立连接并等待响应
        let request_stream = ReceiverStream::new(request_rx).map(|req| {
            crate::registry::ConnectionMessage {
                message_type: Some(crate::registry::connection_message::MessageType::Request(req)),
            }
        });

        let response = self.client.establish_connection(request_stream).await?;
        let mut inbound = response.into_inner();
        
        // 等待响应
        while let Some(message_result) = inbound.next().await {
            let message = message_result?;
            
            if let Some(crate::registry::connection_message::MessageType::Response(response)) = message.message_type {
                return self.deserialize_response(response.payload);
            }
        }
        
        Err(GatewayClientError::Timeout)
    }

    /// 服务端流调用
    pub async fn call_server_stream<T, R>(
        &mut self,
        service_name: &str,
        method_path: &str, 
        request: T,
    ) -> Result<impl Stream<Item = Result<R, GatewayClientError>>, GatewayClientError>
    where
        T: prost::Message + Send + 'static,
        R: prost::Message + Default + Send + 'static,
    {
        let (response_tx, response_rx) = mpsc::channel(100);
        let (request_tx, request_rx) = mpsc::channel(1);
        
        // 发送单个请求
        super::client::streaming::send_server_stream_request(self, request, request_tx, service_name, method_path);
        
        // 建立连接并处理响应流
        let request_stream = ReceiverStream::new(request_rx).map(|req| {
            crate::registry::ConnectionMessage {
                message_type: Some(crate::registry::connection_message::MessageType::Request(req)),
            }
        });

        let response = self.client.establish_connection(request_stream).await?;
        let inbound = response.into_inner();
        
        // 启动响应流处理任务
        super::client::streaming::spawn_server_stream_handler(inbound, response_tx);
        
        Ok(ReceiverStream::new(response_rx))
    }

    /// 双向流调用
    pub async fn call_bidirectional_stream<T, R>(
        &mut self,
        service_name: &str,
        method_path: &str,
        requests: impl Stream<Item = T> + Send + Unpin + 'static,
    ) -> Result<impl Stream<Item = Result<R, GatewayClientError>>, GatewayClientError>
    where
        T: prost::Message + Send + 'static,
        R: prost::Message + Default + Send + 'static,
    {
        self.create_bidirectional_stream(service_name, method_path, requests).await
    }

    /// 创建双向流连接的辅助方法
    async fn create_bidirectional_stream<T, R>(
        &mut self,
        service_name: &str,
        method_path: &str,
        requests: impl Stream<Item = T> + Send + Unpin + 'static,
    ) -> Result<impl Stream<Item = Result<R, GatewayClientError>>, GatewayClientError>
    where
        T: prost::Message + Send + 'static,
        R: prost::Message + Default + Send + 'static,
    {
        let (response_tx, response_rx) = mpsc::channel(100);
        let (request_tx, request_rx) = mpsc::channel(100);
        
        // 创建请求流
        let request_stream = ReceiverStream::new(request_rx);
        let conn_message_stream = request_stream.map(|req| {
            crate::registry::ConnectionMessage {
                message_type: Some(crate::registry::connection_message::MessageType::Request(req)),
            }
        });

        // 建立连接
        let response = self.client.establish_connection(conn_message_stream).await?;
        let inbound = response.into_inner();
        
        // 启动请求发送任务
        super::client::streaming::spawn_request_sender(self, requests, request_tx, service_name, method_path).await;
        
        // 启动响应处理任务
        super::client::streaming::spawn_response_handler(inbound, response_tx).await;
        
        Ok(ReceiverStream::new(response_rx))
    }

    /// 序列化消息（优化内存使用）
    fn serialize_message<T: prost::Message>(&self, message: &T) -> Result<Vec<u8>, GatewayClientError> {
        // 估算消息大小以减少重新分配
        let estimated_size = message.encoded_len();
        let mut buf = bytes::BytesMut::with_capacity(estimated_size);
        
        message.encode(&mut buf)
            .map_err(|e| GatewayClientError::Serialization(e.to_string()))?;
        
        // 转换为Vec<u8>以保持API兼容性
        Ok(buf.to_vec())
    }

    /// 反序列化响应
    fn deserialize_response<R: prost::Message + Default>(
        &self,
        response_bytes: Vec<u8>,
    ) -> Result<R, GatewayClientError> {
        R::decode(&response_bytes[..])
            .map_err(|e| GatewayClientError::Serialization(e.to_string()))
    }

    /// 获取健康的服务列表
    pub async fn list_healthy_services(&mut self) -> Result<HashMap<String, String>, GatewayClientError> {
        // 这里可以实现一个获取服务列表的方法
        // 需要扩展 proto 定义以支持服务发现
        todo!("Service discovery not yet implemented")
    }

    /// 检查服务是否可用
    pub async fn is_service_available(&mut self, service_name: &str) -> Result<bool, GatewayClientError> {
        let services = self.list_healthy_services().await?;
        Ok(services.contains_key(service_name))
    }

    /// 发送连接消息 - 为事件系统提供支持
    pub async fn send_connection_message(
        &mut self,
        message: crate::registry::ConnectionMessage,
    ) -> Result<(), GatewayClientError> {
        // 创建单次消息流
        let request_stream = tokio_stream::once(message);
        
        // 建立连接并发送消息
        let response = self.client.establish_connection(request_stream).await?;
        let mut inbound = response.into_inner();
        
        // 对于事件消息，我们通常不需要等待响应，但要确保消息发送成功
        // 这里简单检查连接是否建立成功
        if let Some(result) = inbound.next().await {
            match result {
                Ok(_) => {
                    tracing::debug!("Connection message sent successfully");
                    Ok(())
                },
                Err(e) => {
                    tracing::error!(error = %e, "Failed to send connection message");
                    Err(GatewayClientError::Grpc(e))
                }
            }
        } else {
            // 如果没有收到任何响应，说明消息可能已经发送成功
            // 这在事件发布场景中是正常的
            tracing::debug!("Connection message sent, no response expected");
            Ok(())
        }
    }
}

/// 辅助宏：简化服务调用
#[macro_export]
macro_rules! gateway_call {
    ($client:expr, $service:expr, $method:expr, $request:expr) => {
        $client.call_unary($service, $method, $request).await
    };
}