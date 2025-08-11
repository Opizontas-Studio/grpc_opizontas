pub mod error;
pub mod extractor;
pub mod forwarder;
pub mod response;

pub use error::RouterError;

use super::client_manager::GrpcClientManager;
use super::registry_service::{ServiceHealthStatus, ServiceRegistry};
use super::reverse_connection_manager::ReverseConnectionManager;
use crate::config::Config;
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

    // 通过反向连接转发请求
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

        // 收集请求体
        let body_bytes = match body.collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(e) => {
                return Err(format!("Failed to collect request body: {e:?}"));
            }
        };

        // 通过反向连接发送请求
        let forward_response = reverse_manager
            .send_request(service_name, method_path, headers, body_bytes)
            .await?;

        // 构建 HTTP 响应
        let mut response_builder =
            http::Response::builder().status(forward_response.status_code as u16);

        // 添加响应头
        for (name, value) in forward_response.headers {
            response_builder = response_builder.header(name, value);
        }

        // 创建响应体
        let response_body = http_body_util::combinators::UnsyncBoxBody::new(
            http_body_util::Full::new(bytes::Bytes::from(forward_response.payload))
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) }),
        );

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
                tracing::debug!(service_name = %service_name, "Using reverse connection for request");

                match Self::forward_via_reverse_connection(
                    &reverse_manager,
                    &service_name,
                    &path,
                    req,
                )
                .await
                {
                    Ok(response) => Ok(response),
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to forward via reverse connection");
                        Ok(response::create_error_response(
                            &RouterError::ForwardingError(e),
                        ))
                    }
                }
            } else {
                // 使用传统的正向连接转发请求
                // 从注册表查找目标地址（使用 DashMap）
                let target_addr = registry
                    .get(&service_name)
                    .filter(|entry| entry.value().health_status == ServiceHealthStatus::Healthy)
                    .map(|entry| entry.value().address.clone());
                tracing::debug!(
                    service_name = %service_name,
                    path = %path,
                    target_addr = ?target_addr,
                    "Using traditional forward connection"
                );

                match target_addr {
                    Some(addr) => {
                        // 转发请求到目标服务
                        match forwarder::forward_request(&client_manager, &config, req, &addr).await
                        {
                            Ok(response) => Ok(response),
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to forward request");
                                Ok(response::create_error_response(&e))
                            }
                        }
                    }
                    None => {
                        // 服务未注册
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
