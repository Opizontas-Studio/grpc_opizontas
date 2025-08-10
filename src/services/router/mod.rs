pub mod error;
pub mod extractor;
pub mod forwarder;
pub mod response;

pub use error::RouterError;

use super::client_manager::GrpcClientManager;
use super::registry_service::{ServiceHealthStatus, ServiceRegistry};
use crate::config::Config;
use futures::future::BoxFuture;
use http_body::Body;
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
}

impl DynamicRouter {
    pub fn new(registry: ServiceRegistry, config: Config) -> Self {
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
        }
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

        Box::pin(async move {
            let path = req.uri().path();

            // 解析服务名（改进的错误处理）
            let service_name = match extractor::extract_service_name(path) {
                Ok(name) => name,
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "Invalid gRPC path");
                    return Ok(response::create_error_response(&e));
                }
            };

            // 从注册表查找目标地址（使用 DashMap）
            let target_addr = registry
                .get(&service_name)
                .filter(|entry| entry.value().health_status == ServiceHealthStatus::Healthy)
                .map(|entry| entry.value().address.clone());
            tracing::debug!(
                service_name = %service_name,
                path = %path,
                target_addr = ?target_addr,
                "Forwarding request"
            );

            // 转发请求
            match target_addr {
                Some(addr) => {
                    // 转发请求到目标服务
                    match forwarder::forward_request(
                        &client_manager,
                        &config,
                        req,
                        &addr,
                    )
                    .await
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
        })
    }
}

impl NamedService for DynamicRouter {
    const NAME: &'static str = "grpc_opizontas.DynamicRouter";
}
