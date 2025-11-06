use super::GatewayClientError;
use crate::registry::{ForwardRequest, StreamingInfo, streaming_info::StreamType};
use crate::services::gateway_client::GatewayClient;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt};
use tonic::Status;
use uuid::Uuid;

/// 启动请求发送任务
pub(crate) async fn spawn_request_sender<T>(
    client: &GatewayClient,
    requests: impl Stream<Item = T> + Send + Unpin + 'static,
    request_tx: mpsc::Sender<ForwardRequest>,
    service_name: &str,
    method_path: &str,
) where
    T: prost::Message + Send + 'static,
{
    let service_name = service_name.to_string();
    let method_path = method_path.to_string();
    let timeout = client.config.default_timeout.as_secs() as i32;

    tokio::spawn(async move {
        let mut sequence_number = 0i64;
        let mut requests = requests;

        while let Some(request_item) = requests.next().await {
            let payload = match super::generic::serialize_message_static(&request_item) {
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
pub(crate) async fn spawn_response_handler<R>(
    mut inbound: impl Stream<Item = Result<crate::registry::ConnectionMessage, Status>>
    + Send
    + Unpin
    + 'static,
    response_tx: mpsc::Sender<Result<R, GatewayClientError>>,
) where
    R: prost::Message + Default + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(message_result) = inbound.next().await {
            match message_result {
                Ok(message) => {
                    if let Some(crate::registry::connection_message::MessageType::Response(
                        response,
                    )) = message.message_type
                    {
                        let result =
                            super::generic::deserialize_response_static::<R>(response.payload);

                        if response_tx.send(result).await.is_err() {
                            break;
                        }

                        // 检查流结束
                        if let Some(streaming_info) = response.streaming_info
                            && streaming_info.is_stream_end
                        {
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
pub(crate) fn send_client_stream_requests<T>(
    client: &GatewayClient,
    requests: impl Iterator<Item = T> + Send + 'static,
    request_tx: mpsc::Sender<ForwardRequest>,
    service_name: &str,
    method_path: &str,
) where
    T: prost::Message + Send + 'static,
{
    let service_name = service_name.to_string();
    let method_path = method_path.to_string();
    let timeout = client.config.default_timeout.as_secs() as i32;

    tokio::spawn(async move {
        let mut sequence_number = 0i64;

        // 发送所有请求
        for request_item in requests {
            let Ok(payload) = super::generic::serialize_message_static(&request_item) else {
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
pub(crate) fn send_server_stream_request<T>(
    client: &GatewayClient,
    request: T,
    request_tx: mpsc::Sender<ForwardRequest>,
    service_name: &str,
    method_path: &str,
) where
    T: prost::Message + Send + 'static,
{
    let service_name = service_name.to_string();
    let method_path = method_path.to_string();
    let timeout = client.config.default_timeout.as_secs() as i32;

    tokio::spawn(async move {
        let Ok(payload) = super::generic::serialize_message_static(&request) else {
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
pub(crate) fn spawn_server_stream_handler<R>(
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

            let Some(crate::registry::connection_message::MessageType::Response(response)) =
                message.message_type
            else {
                continue;
            };

            let result = super::generic::deserialize_response_static::<R>(response.payload);

            if response_tx.send(result).await.is_err() {
                break;
            }

            // 检查流结束标记
            if let Some(streaming_info) = response.streaming_info
                && streaming_info.is_stream_end
            {
                break;
            }
        }
    });
}
