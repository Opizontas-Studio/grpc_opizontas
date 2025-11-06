pub mod error;
pub mod extractor;
pub mod forwarder;
pub mod response;

pub use error::RouterError;

use super::client_manager::GrpcClientManager;
use super::connection::ReverseConnectionManager;
use crate::config::Config;
use crate::services::registry::{ServiceHealthStatus, ServiceRegistry};
use futures::future::BoxFuture;
use http_body::Body;
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::task::{Context, Poll};
use std::time::Duration;
use tonic::server::NamedService;
use tower::Service;

// 定义动态路由服务
#[derive(Debug, Clone)]
pub struct DynamicRouter {
    pub registry: ServiceRegistry,
    pub client_manager: GrpcClientManager,
    pub config: Config,
    pub reverse_manager: std::sync::Arc<ReverseConnectionManager>,
}

impl DynamicRouter {
    pub fn new(
        registry: ServiceRegistry,
        config: Config,
        reverse_manager: std::sync::Arc<ReverseConnectionManager>,
    ) -> Self {
        // 使用配置创建连接管理器
        let connection_pool_config = crate::services::client_manager::ConnectionPoolConfig {
            max_connections: config.connection_pool.max_connections,
            connection_ttl: Duration::from_secs(config.connection_pool.connection_ttl),
            idle_timeout: Duration::from_secs(config.connection_pool.idle_timeout),
            cleanup_interval: Duration::from_secs(config.connection_pool.cleanup_interval),
        };

        Self {
            registry,
            client_manager: GrpcClientManager::new(connection_pool_config),
            config,
            reverse_manager,
        }
    }

    // 通过反向连接转发请求（流式版本）
    async fn forward_via_reverse_connection<B>(
        reverse_manager: &std::sync::Arc<ReverseConnectionManager>,
        service_name: &str,
        method_path: &str,
        req: http::Request<B>,
    ) -> Result<
        http::Response<
            http_body_util::combinators::UnsyncBoxBody<
                bytes::Bytes,
                Box<dyn std::error::Error + Send + Sync>,
            >,
        >,
        String,
    >
    where
        B: Body<Data = bytes::Bytes> + Send + 'static,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + std::fmt::Debug,
    {
        // 分解请求
        let (parts, body) = req.into_parts();

        // 收集请求头
        let mut headers = HashMap::new();
        for (name, value) in parts.headers.iter() {
            if let Ok(value_str) = value.to_str() {
                headers.insert(name.to_string(), value_str.to_string());
            }
        }

        // 使用流式处理请求体
        let forward_response = reverse_manager
            .send_request_stream(service_name, method_path, headers, body)
            .await?;

        // 构建 HTTP 响应
        let mut response_builder =
            http::Response::builder().status(forward_response.status_code as u16);

        // 检查是否为流式响应
        let is_streaming = ReverseConnectionManager::is_streaming_response(&forward_response);

        // 添加响应头
        for (name, value) in forward_response.headers {
            response_builder = response_builder.header(name, value);
        }

        // 创建响应体（支持流式响应）
        let response_body = if is_streaming {
            // 对于流式响应，直接使用payload，因为它们已经在reverse_connection_manager中被组装了
            http_body_util::combinators::UnsyncBoxBody::new(
                http_body_util::Full::new(bytes::Bytes::from(forward_response.payload))
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) }),
            )
        } else {
            // 常规响应
            http_body_util::combinators::UnsyncBoxBody::new(
                http_body_util::Full::new(bytes::Bytes::from(forward_response.payload))
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) }),
            )
        };

        response_builder
            .body(response_body)
            .map_err(|e| format!("Failed to build response: {e}"))
    }
}

impl<B> Service<http::Request<B>> for DynamicRouter
where
    B: Body<Data = bytes::Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + std::fmt::Debug,
{
    type Response = http::Response<
        http_body_util::combinators::UnsyncBoxBody<
            bytes::Bytes,
            Box<dyn std::error::Error + Send + Sync>,
        >,
    >;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let registry = self.registry.clone();
        let client_manager = self.client_manager.clone();
        let config = self.config.clone();
        let reverse_manager = self.reverse_manager.clone();

        Box::pin(async move {
            let path = req.uri().path().to_string();

            // 解析服务名（改进的错误处理）
            let service_name = match extractor::extract_service_name(&path) {
                Ok(name) => name,
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "Invalid gRPC path");
                    return Ok(response::create_error_response(&e));
                }
            };

            // 检查是否有反向连接可用
            if reverse_manager.has_reverse_connection(&service_name) {
                // 使用反向连接转发请求
                tracing::info!(
                    service_name = %service_name,
                    path = %path,
                    "Using reverse connection for request forwarding"
                );

                match Self::forward_via_reverse_connection(
                    &reverse_manager,
                    &service_name,
                    &path,
                    req,
                )
                .await
                {
                    Ok(response) => {
                        tracing::debug!(
                            service_name = %service_name,
                            path = %path,
                            status = %response.status(),
                            "Request forwarded successfully via reverse connection"
                        );
                        Ok(response)
                    }
                    Err(e) => {
                        tracing::error!(
                            service_name = %service_name,
                            path = %path,
                            error = %e,
                            "Failed to forward request via reverse connection"
                        );
                        Ok(response::create_error_response(
                            &RouterError::ForwardingError(e),
                        ))
                    }
                }
            } else {
                // 使用传统的正向连接转发请求
                // 从注册表查找目标地址（使用 DashMap）
                let target_addr = if let Some(instances_guard) = registry.get(&service_name) {
                    let instances = instances_guard.clone();
                    drop(instances_guard);

                    instances
                        .iter()
                        .find(|instance| {
                            instance.value().health_status == ServiceHealthStatus::Healthy
                        })
                        .map(|instance| instance.value().address.clone())
                } else {
                    None
                };
                tracing::debug!(
                    service_name = %service_name,
                    path = %path,
                    target_addr = ?target_addr,
                    "Using traditional forward connection"
                );

                match target_addr {
                    Some(ref addr) => {
                        tracing::info!(
                            service_name = %service_name,
                            target_addr = %addr,
                            path = %path,
                            "Forwarding request to healthy service instance"
                        );

                        // 转发请求到目标服务
                        match forwarder::forward_request(&client_manager, &config, req, addr).await
                        {
                            Ok(response) => {
                                tracing::debug!(
                                    service_name = %service_name,
                                    target_addr = %addr,
                                    status = %response.status(),
                                    "Request forwarded successfully"
                                );
                                Ok(response)
                            }
                            Err(e) => {
                                tracing::error!(
                                    service_name = %service_name,
                                    target_addr = %addr,
                                    path = %path,
                                    error = %e,
                                    "Failed to forward request to target service"
                                );
                                Ok(response::create_error_response(&e))
                            }
                        }
                    }
                    None => {
                        // 服务未注册
                        tracing::warn!(
                            service_name = %service_name,
                            path = %path,
                            "Service not found or no healthy instances available in registry"
                        );
                        let error = RouterError::ServiceNotFound(format!(
                            "Service '{service_name}' not found in registry"
                        ));
                        Ok(response::create_error_response(&error))
                    }
                }
            }
        })
    }
}

impl NamedService for DynamicRouter {
    const NAME: &'static str = "grpc_opizontas.DynamicRouter";
}
