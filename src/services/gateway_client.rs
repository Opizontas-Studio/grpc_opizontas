use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Channel, Endpoint};
use tonic::Status;
use uuid::Uuid;

use crate::registry::{
    ForwardRequest, StreamingInfo,
    streaming_info::StreamType,
    registry_service_client::RegistryServiceClient,
};

/// 网关客户端错误类型
#[derive(Debug, thiserror::Error)]
pub enum GatewayClientError {
    #[error("Transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("gRPC error: {0}")]
    Grpc(#[from] Status),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Timeout error")]
    Timeout,
    #[error("Service not found: {0}")]
    ServiceNotFound(String),
}

/// 网关客户端配置
#[derive(Debug, Clone)]
pub struct GatewayClientConfig {
    /// 网关地址
    pub gateway_address: String,
    /// 默认超时时间
    pub default_timeout: Duration,
    /// 连接超时时间
    pub connect_timeout: Duration,
    /// API 密钥
    pub api_key: String,
}

impl Default for GatewayClientConfig {
    fn default() -> Self {
        Self {
            gateway_address: "http://localhost:50051".to_string(),
            default_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            api_key: String::new(),
        }
    }
}

/// 网关客户端
#[derive(Debug, Clone)]
pub struct GatewayClient {
    config: GatewayClientConfig,
    client: RegistryServiceClient<Channel>,
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
        self.send_client_stream_requests(requests, request_tx, service_name, method_path);
        
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
        self.send_server_stream_request(request, request_tx, service_name, method_path);
        
        // 建立连接并处理响应流
        let request_stream = ReceiverStream::new(request_rx).map(|req| {
            crate::registry::ConnectionMessage {
                message_type: Some(crate::registry::connection_message::MessageType::Request(req)),
            }
        });

        let response = self.client.establish_connection(request_stream).await?;
        let inbound = response.into_inner();
        
        // 启动响应流处理任务
        self.spawn_server_stream_handler::<R>(inbound, response_tx);
        
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
        self.spawn_request_sender(requests, request_tx, service_name, method_path).await;
        
        // 启动响应处理任务
        self.spawn_response_handler::<R>(inbound, response_tx).await;
        
        Ok(ReceiverStream::new(response_rx))
    }

    /// 启动请求发送任务
    async fn spawn_request_sender<T>(
        &self,
        requests: impl Stream<Item = T> + Send + Unpin + 'static,
        request_tx: mpsc::Sender<ForwardRequest>,
        service_name: &str,
        method_path: &str,
    ) where
        T: prost::Message + Send + 'static,
    {
        let service_name = service_name.to_string();
        let method_path = method_path.to_string();
        let timeout = self.config.default_timeout.as_secs() as i32;
        
        tokio::spawn(async move {
            let mut sequence_number = 0i64;
            let mut requests = requests;
            
            while let Some(request_item) = requests.next().await {
                let payload = match Self::serialize_message_static(&request_item) {
                    Ok(p) => p,
                    Err(_) => break,
                };
                
                let mut headers = HashMap::new();
                headers.insert("x-service-name".to_string(), service_name.clone());
                
                let forward_request = ForwardRequest {
                    request_id: Uuid::new_v4().to_string(),
                    method_path: method_path.clone(),
                    headers,
                    payload,
                    timeout_seconds: timeout,
                    streaming_info: Some(StreamingInfo {
                        stream_type: StreamType::BidirectionalStreaming as i32,
                        is_stream_end: false,
                        sequence_number,
                        chunk_size: 0,
                    }),
                };
                
                sequence_number += 1;
                if request_tx.send(forward_request).await.is_err() {
                    break;
                }
            }
            
            // 发送结束标记
            let mut end_headers = HashMap::new();
            end_headers.insert("x-service-name".to_string(), service_name);
            
            let end_request = ForwardRequest {
                request_id: Uuid::new_v4().to_string(),
                method_path,
                headers: end_headers,
                payload: Vec::new(),
                timeout_seconds: timeout,
                streaming_info: Some(StreamingInfo {
                    stream_type: StreamType::BidirectionalStreaming as i32,
                    is_stream_end: true,
                    sequence_number,
                    chunk_size: 0,
                }),
            };
            
            let _ = request_tx.send(end_request).await;
        });
    }

    /// 启动响应处理任务
    async fn spawn_response_handler<R>(
        &self,
        mut inbound: impl Stream<Item = Result<crate::registry::ConnectionMessage, Status>> + Send + Unpin + 'static,
        response_tx: mpsc::Sender<Result<R, GatewayClientError>>,
    ) where
        R: prost::Message + Default + Send + 'static,
    {
        tokio::spawn(async move {
            while let Some(message_result) = inbound.next().await {
                match message_result {
                    Ok(message) => {
                        if let Some(crate::registry::connection_message::MessageType::Response(response)) = message.message_type {
                            let result = Self::deserialize_response_static::<R>(response.payload);
                            
                            if response_tx.send(result).await.is_err() {
                                break;
                            }
                            
                            // 检查流结束
                            if let Some(streaming_info) = response.streaming_info
                                && streaming_info.is_stream_end {
                                    break;
                                }
                        }
                    }
                    Err(e) => {
                        let _ = response_tx.send(Err(GatewayClientError::Grpc(e))).await;
                        break;
                    }
                }
            }
        });
    }

    /// 发送客户端流请求
    fn send_client_stream_requests<T>(
        &self,
        requests: impl Iterator<Item = T> + Send + 'static,
        request_tx: mpsc::Sender<ForwardRequest>,
        service_name: &str,
        method_path: &str,
    ) where
        T: prost::Message + Send + 'static,
    {
        let service_name = service_name.to_string();
        let method_path = method_path.to_string();
        let timeout = self.config.default_timeout.as_secs() as i32;
        
        tokio::spawn(async move {
            let mut sequence_number = 0i64;
            
            // 发送所有请求
            for request_item in requests {
                let Ok(payload) = Self::serialize_message_static(&request_item) else {
                    break;
                };
                
                let mut headers = HashMap::new();
                headers.insert("x-service-name".to_string(), service_name.clone());
                
                let forward_request = ForwardRequest {
                    request_id: Uuid::new_v4().to_string(),
                    method_path: method_path.clone(),
                    headers,
                    payload,
                    timeout_seconds: timeout,
                    streaming_info: Some(StreamingInfo {
                        stream_type: StreamType::ClientStreaming as i32,
                        is_stream_end: false,
                        sequence_number,
                        chunk_size: 0,
                    }),
                };
                
                sequence_number += 1;
                if request_tx.send(forward_request).await.is_err() {
                    break;
                }
            }
            
            // 发送结束标记
            let mut end_headers = HashMap::new();
            end_headers.insert("x-service-name".to_string(), service_name);
            
            let end_request = ForwardRequest {
                request_id: Uuid::new_v4().to_string(),
                method_path,
                headers: end_headers,
                payload: Vec::new(),
                timeout_seconds: timeout,
                streaming_info: Some(StreamingInfo {
                    stream_type: StreamType::ClientStreaming as i32,
                    is_stream_end: true,
                    sequence_number,
                    chunk_size: 0,
                }),
            };
            
            let _ = request_tx.send(end_request).await;
        });
    }

    /// 发送服务端流请求
    fn send_server_stream_request<T>(
        &self,
        request: T,
        request_tx: mpsc::Sender<ForwardRequest>,
        service_name: &str,
        method_path: &str,
    ) where
        T: prost::Message + Send + 'static,
    {
        let service_name = service_name.to_string();
        let method_path = method_path.to_string();
        let timeout = self.config.default_timeout.as_secs() as i32;
        
        tokio::spawn(async move {
            let Ok(payload) = Self::serialize_message_static(&request) else {
                return;
            };
            
            let mut headers = HashMap::new();
            headers.insert("x-service-name".to_string(), service_name);
            
            let forward_request = ForwardRequest {
                request_id: Uuid::new_v4().to_string(),
                method_path,
                headers,
                payload,
                timeout_seconds: timeout,
                streaming_info: Some(StreamingInfo {
                    stream_type: StreamType::ServerStreaming as i32,
                    is_stream_end: false,
                    sequence_number: 0,
                    chunk_size: 0,
                }),
            };
            
            let _ = request_tx.send(forward_request).await;
        });
    }

    /// 启动服务端流响应处理任务
    fn spawn_server_stream_handler<R>(
        &self,
        mut inbound: tonic::Streaming<crate::registry::ConnectionMessage>,
        response_tx: mpsc::Sender<Result<R, GatewayClientError>>,
    ) where
        R: prost::Message + Default + Send + 'static,
    {
        tokio::spawn(async move {
            while let Some(message_result) = inbound.next().await {
                let message = match message_result {
                    Ok(msg) => msg,
                    Err(e) => {
                        let _ = response_tx.send(Err(GatewayClientError::Grpc(e))).await;
                        break;
                    }
                };
                
                let Some(crate::registry::connection_message::MessageType::Response(response)) = message.message_type else {
                    continue;
                };
                
                let result = Self::deserialize_response_static::<R>(response.payload);
                
                if response_tx.send(result).await.is_err() {
                    break;
                }
                
                // 检查流结束标记
                if let Some(streaming_info) = response.streaming_info
                    && streaming_info.is_stream_end {
                    break;
                }
            }
        });
    }

    /// 静态方法：序列化消息
    fn serialize_message_static<T: prost::Message>(message: &T) -> Result<Vec<u8>, GatewayClientError> {
        let mut buf = Vec::new();
        message.encode(&mut buf)
            .map_err(|e| GatewayClientError::Serialization(e.to_string()))?;
        Ok(buf)
    }

    /// 静态方法：反序列化响应
    fn deserialize_response_static<R: prost::Message + Default>(response_bytes: Vec<u8>) -> Result<R, GatewayClientError> {
        R::decode(&response_bytes[..])
            .map_err(|e| GatewayClientError::Serialization(e.to_string()))
    }

    /// 序列化消息
    fn serialize_message<T: prost::Message>(&self, message: &T) -> Result<Vec<u8>, GatewayClientError> {
        let mut buf = Vec::new();
        message.encode(&mut buf)
            .map_err(|e| GatewayClientError::Serialization(e.to_string()))?;
        Ok(buf)
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
}

/// 辅助宏：简化服务调用
#[macro_export]
macro_rules! gateway_call {
    ($client:expr, $service:expr, $method:expr, $request:expr) => {
        $client.call_unary($service, $method, $request).await
    };
}