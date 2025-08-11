use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

use super::reverse_connection_manager::ReverseConnectionManager;
use crate::config::Config;
use crate::registry::{
    ConnectionMessage, ConnectionStatus, RegisterRequest, RegisterResponse,
    connection_message::MessageType, connection_status::StatusType,
    registry_service_server::RegistryService,
};

// 服务注册信息
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub address: String,
    pub last_heartbeat: SystemTime,
    pub health_status: ServiceHealthStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServiceHealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

// 定义增强的服务注册表
pub type ServiceRegistry = Arc<DashMap<String, ServiceInfo>>;

// 定义的服务实现
#[derive(Debug)]
pub struct MyRegistryService {
    pub registry: ServiceRegistry,
    pub config: Config,
    pub reverse_connection_manager: Arc<ReverseConnectionManager>,
}

impl MyRegistryService {
    pub fn new(config: Config) -> Self {
        // 创建反向连接管理器配置
        let reverse_config = super::reverse_connection_manager::ReverseConnectionConfig {
            heartbeat_timeout: Duration::from_secs(config.reverse_connection.heartbeat_timeout),
            request_timeout: Duration::from_secs(config.reverse_connection.request_timeout),
            cleanup_interval: Duration::from_secs(config.reverse_connection.cleanup_interval),
            max_pending_requests: config.reverse_connection.max_pending_requests,
        };

        let service = Self {
            registry: Arc::new(DashMap::new()),
            config,
            reverse_connection_manager: Arc::new(ReverseConnectionManager::new(reverse_config)),
        };

        // 启动定期清理任务
        let registry_clone = service.registry.clone();
        let heartbeat_timeout = service.config.heartbeat_timeout();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_timeout);
            loop {
                interval.tick().await;
                tracing::debug!("Executing service expiration check...");
                Self::cleanup_expired_services(&registry_clone, heartbeat_timeout).await;
            }
        });

        service
    }
}

impl MyRegistryService {
    // 清理过期的服务
    async fn cleanup_expired_services(registry: &ServiceRegistry, timeout: Duration) {
        let now = SystemTime::now();
        let mut to_remove = Vec::new();

        // 收集需要删除的服务
        for entry in registry.iter() {
            let service_name = entry.key();
            let service_info = entry.value();

            if let Ok(elapsed) = now.duration_since(service_info.last_heartbeat) {
                if elapsed > timeout {
                    tracing::warn!(service_name = %service_name, elapsed_secs = elapsed.as_secs(), "Service expired, removing from registry");
                    to_remove.push(service_name.clone());
                } else {
                    tracing::debug!(service_name = %service_name, last_heartbeat_secs = elapsed.as_secs(), "Service is healthy");
                }
            }
        }

        if to_remove.is_empty() {
        } else {
            tracing::info!(
                expired_count = to_remove.len(),
                "Cleanup check completed, removing expired services..."
            );
        }

        // 删除过期的服务
        for service_name in to_remove {
            registry.remove(&service_name);
        }
    }

    // 获取所有健康的服务
    pub fn get_healthy_services(&self) -> HashMap<String, String> {
        self.registry
            .iter()
            .filter(|entry| entry.value().health_status == ServiceHealthStatus::Healthy)
            .map(|entry| (entry.key().clone(), entry.value().address.clone()))
            .collect()
    }

    // 获取服务信息
    pub fn get_service_info(&self, service_name: &str) -> Option<ServiceInfo> {
        self.registry
            .get(service_name)
            .map(|entry| entry.value().clone())
    }

    // 手动更新服务健康状态
    pub fn update_service_health(&self, service_name: &str, status: ServiceHealthStatus) -> bool {
        if let Some(mut entry) = self.registry.get_mut(service_name) {
            tracing::info!(service_name = %service_name, new_status = ?status, "Updated health status for service");
            entry.health_status = status;
            true
        } else {
            false
        }
    }

    // 注销服务
    pub fn unregister_service(&self, service_name: &str) -> bool {
        if self.registry.remove(service_name).is_some() {
            tracing::info!(service_name = %service_name, "Unregistered service");
            true
        } else {
            false
        }
    }
}

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

        // 处理入站消息的任务
        tokio::spawn(async move {
            while let Some(message_result) = inbound.next().await {
                match message_result {
                    Ok(message) => {
                        if let Some(message_type) = message.message_type {
                            match message_type {
                                MessageType::Response(response) => {
                                    // 处理来自微服务的响应
                                    reverse_manager.handle_response(response).await;
                                }
                                MessageType::Heartbeat(heartbeat) => {
                                    // 更新心跳
                                    reverse_manager
                                        .update_heartbeat(&heartbeat.connection_id)
                                        .await;
                                }
                                MessageType::Status(status) => {
                                    // 处理状态消息
                                    if status.status == StatusType::Disconnected as i32 {
                                        tracing::info!(
                                            connection_id = %status.connection_id,
                                            "Client requested disconnection"
                                        );
                                        break;
                                    }
                                }
                                _ => {
                                    tracing::warn!("Unexpected message type from client");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Error receiving message from client");
                        break;
                    }
                }
            }

            // 连接结束时清理
            reverse_manager
                .unregister_connection(&connection_id_clone)
                .await;
            tracing::info!(connection_id = %connection_id_clone, "Reverse connection closed");
        });

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
