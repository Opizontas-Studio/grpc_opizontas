use std::time::SystemTime;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

use super::service::MyRegistryService;
use super::types::{ServiceHealthStatus, ServiceInfo};
use crate::registry::{
    ConnectionMessage, ConnectionStatus, RegisterRequest, RegisterResponse,
    connection_message::MessageType, connection_status::StatusType,
    registry_service_server::RegistryService,
};

// 为结构体实现 gRPC 服务 trait
#[tonic::async_trait]
impl RegistryService for MyRegistryService {
    type EstablishConnectionStream = ReceiverStream<Result<ConnectionMessage, Status>>;
    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();

        // 验证 Token
        if !self.config.validate_token(&req.api_key) {
            return Err(Status::unauthenticated("Invalid token"));
        }

        let service_info = ServiceInfo {
            address: req.address.clone(),
            last_heartbeat: SystemTime::now(),
            health_status: ServiceHealthStatus::Healthy,
        };

        for service_name in req.services {
            tracing::info!(
                service_name = %service_name,
                address = %req.address,
                "Registering service"
            );
            self.registry.insert(service_name, service_info.clone());
        }

        let reply = RegisterResponse {
            success: true,
            message: "Registration successful".into(),
        };

        Ok(Response::new(reply))
    }

    // 建立反向连接的双向流
    async fn establish_connection(
        &self,
        request: Request<Streaming<ConnectionMessage>>,
    ) -> Result<Response<ReceiverStream<Result<ConnectionMessage, Status>>>, Status> {
        let mut inbound = request.into_inner();
        let (outbound_tx, outbound_rx) = mpsc::channel(100);

        // 等待第一个消息，应该是连接注册
        let first_message = match inbound.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => {
                tracing::error!(error = %e, "Error receiving first connection message");
                return Err(Status::internal("Failed to receive connection message"));
            }
            None => {
                tracing::error!("Connection stream closed immediately");
                return Err(Status::aborted("Connection stream closed"));
            }
        };

        // 处理连接注册
        let (connection_id, services) = match first_message.message_type {
            Some(MessageType::Register(register)) => {
                // 验证 Token
                if !self.config.validate_token(&register.api_key) {
                    return Err(Status::unauthenticated("Invalid token"));
                }

                let connection_id = if register.connection_id.is_empty() {
                    Uuid::new_v4().to_string()
                } else {
                    register.connection_id
                };

                tracing::info!(
                    connection_id = %connection_id,
                    services = ?register.services,
                    "Establishing reverse connection"
                );

                (connection_id, register.services)
            }
            _ => {
                return Err(Status::invalid_argument(
                    "First message must be a connection register",
                ));
            }
        };

        // 创建请求发送通道
        let (request_tx, mut request_rx) = mpsc::unbounded_channel();

        // 注册反向连接
        if let Err(e) = self
            .reverse_connection_manager
            .register_connection(connection_id.clone(), services.clone(), request_tx)
            .await
        {
            tracing::error!(error = %e, "Failed to register reverse connection");
            return Err(Status::internal("Failed to register connection"));
        }

        // 发送连接确认
        let status_msg = ConnectionMessage {
            message_type: Some(MessageType::Status(ConnectionStatus {
                connection_id: connection_id.clone(),
                status: StatusType::Connected as i32,
                message: "Connection established".to_string(),
            })),
        };

        if (outbound_tx.send(Ok(status_msg)).await).is_err() {
            tracing::error!("Failed to send connection confirmation");
            return Err(Status::internal("Failed to send confirmation"));
        }

        let reverse_manager = self.reverse_connection_manager.clone();
        let connection_id_clone = connection_id.clone();
        let outbound_tx_for_inbound = outbound_tx.clone();

        // 处理入站消息的任务
        Self::spawn_inbound_message_handler(
            inbound,
            (*reverse_manager).clone(),
            connection_id_clone,
            outbound_tx_for_inbound,
        );

        // 处理出站消息的任务
        let outbound_tx_clone = outbound_tx.clone();
        tokio::spawn(async move {
            while let Some(message) = request_rx.recv().await {
                if (outbound_tx_clone.send(Ok(message)).await).is_err() {
                    tracing::error!("Failed to send message to client, connection may be closed");
                    break;
                }
            }
        });

        let outbound_stream = ReceiverStream::new(outbound_rx);
        Ok(Response::new(outbound_stream))
    }
}

impl MyRegistryService {
    fn spawn_inbound_message_handler(
        mut inbound: Streaming<ConnectionMessage>,
        reverse_manager: crate::services::connection::ReverseConnectionManager,
        connection_id: String,
        outbound_tx: mpsc::Sender<Result<ConnectionMessage, Status>>,
    ) {
        tokio::spawn(async move {
            while let Some(message_result) = inbound.next().await {
                match message_result {
                    Ok(message) => {
                        if let Some(message_type) = message.message_type {
                            let should_break = Self::handle_message_type(
                                message_type,
                                &reverse_manager,
                                &outbound_tx,
                            ).await;
                            if should_break {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Error receiving message from client");
                        break;
                    }
                }
            }

            reverse_manager.unregister_connection(&connection_id).await;
            tracing::info!(connection_id = %connection_id, "Reverse connection closed");
        });
    }

    async fn handle_message_type(
        message_type: MessageType,
        reverse_manager: &crate::services::connection::ReverseConnectionManager,
        outbound_tx: &mpsc::Sender<Result<ConnectionMessage, Status>>,
    ) -> bool {
        match message_type {
            MessageType::Response(response) => {
                reverse_manager.handle_response(response).await;
                false
            }
            MessageType::Heartbeat(heartbeat) => {
                reverse_manager.update_heartbeat(&heartbeat.connection_id).await;
                false
            }
            MessageType::Status(status) => {
                if status.status == StatusType::Disconnected as i32 {
                    tracing::info!(
                        connection_id = %status.connection_id,
                        "Client requested disconnection"
                    );
                    return true;
                }
                false
            }
            MessageType::Request(request) => {
                Self::handle_service_to_service_request(
                    request,
                    (*reverse_manager).clone(),
                    outbound_tx.clone(),
                ).await;
                false
            }
            MessageType::Register(_) => {
                tracing::warn!("Unexpected register message in established connection");
                false
            }
        }
    }

    async fn handle_service_to_service_request(
        request: crate::registry::ForwardRequest,
        reverse_manager: crate::services::connection::ReverseConnectionManager,
        outbound_tx: mpsc::Sender<Result<ConnectionMessage, Status>>,
    ) {
        tracing::debug!(
            request_id = %request.request_id,
            method_path = %request.method_path,
            "Processing service-to-service request via reverse connection"
        );

        let request_id = request.request_id.clone();
        let streaming_info = request.streaming_info;

        tokio::spawn(async move {
            let result = MyRegistryService::handle_service_request(std::sync::Arc::new(reverse_manager), request).await;
            Self::send_response_or_error(result, request_id, streaming_info, outbound_tx).await;
        });
    }

    async fn send_response_or_error(
        result: Result<crate::registry::ForwardResponse, String>,
        request_id: String,
        streaming_info: Option<crate::registry::StreamingInfo>,
        outbound_tx: mpsc::Sender<Result<ConnectionMessage, Status>>,
    ) {
        match result {
            Ok(response) => {
                let response_msg = ConnectionMessage {
                    message_type: Some(MessageType::Response(response)),
                };
                if let Err(e) = outbound_tx.send(Ok(response_msg)).await {
                    tracing::error!(error = %e, "Failed to send response back to client");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to handle service request");
                let error_response = crate::registry::ForwardResponse {
                    request_id,
                    status_code: 500,
                    headers: std::collections::HashMap::new(),
                    payload: Vec::new(),
                    error_message: e,
                    streaming_info,
                    response_stream_info: None,
                };
                let error_msg = ConnectionMessage {
                    message_type: Some(MessageType::Response(error_response)),
                };
                if let Err(e) = outbound_tx.send(Ok(error_msg)).await {
                    tracing::error!(error = %e, "Failed to send error response");
                }
            }
        }
    }
}