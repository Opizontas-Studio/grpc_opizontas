
use super::client_manager::GrpcClientManager;
use super::registry_service::{ServiceRegistry, ServiceHealthStatus};
use crate::config::Config;
use futures::future::BoxFuture;
use http_body::Body;
use http_body_util::BodyExt;
use std::task::{Context, Poll};
use tower::{Service, util::ServiceExt};
use http_body_util::combinators::BoxBody;
use tonic::server::NamedService;
use std::time::Duration;

// 定义路由错误类型
#[derive(Debug)]
pub enum RouterError {
    ServiceNotFound(String),
    ServiceUnavailable(String),
    InvalidPath(String),
    ForwardingError(String),
    LockError(String),
}

impl std::fmt::Display for RouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouterError::ServiceNotFound(msg) => write!(f, "Service not found: {}", msg),
            RouterError::ServiceUnavailable(msg) => write!(f, "Service unavailable: {}", msg),
            RouterError::InvalidPath(msg) => write!(f, "Invalid path: {}", msg),
            RouterError::ForwardingError(msg) => write!(f, "Forwarding error: {}", msg),
            RouterError::LockError(msg) => write!(f, "Lock error: {}", msg),
        }
    }
}

impl std::error::Error for RouterError {}

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

    // 增强的服务名解析，支持多种格式
    fn extract_service_name(path: &str) -> Result<String, RouterError> {
        if path.is_empty() || !path.starts_with('/') {
            return Err(RouterError::InvalidPath("Path must start with '/'".to_string()));
        }
        
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if parts.len() < 2 {
            return Err(RouterError::InvalidPath(
                "Path must have at least service and method parts".to_string()
            ));
        }
        
        let service_path = parts[0];
        
        // 支持多种服务名格式
        let service_name = if service_path.contains('.') {
            // 标准格式: "package.ServiceName" -> "ServiceName"
            service_path.split('.').last().unwrap_or(service_path)
        } else {
            // 简单格式: "ServiceName"
            service_path
        };
        
        if service_name.is_empty() {
            return Err(RouterError::InvalidPath("Empty service name".to_string()));
        }
        
        Ok(service_name.to_string())
    }

    // 转发 gRPC 请求到目标服务
    async fn forward_request<B>(
        &self,
        req: http::Request<B>,
        target_addr: &str,
    ) -> Result<http::Response<BoxBody<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>>, RouterError>
    where
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + std::fmt::Debug,
    {
        // 获取或创建客户端连接
        let channel = self.client_manager.get_or_create_client(target_addr).await
            .map_err(|e| RouterError::ForwardingError(format!("Failed to get client: {}", e)))?;
        
        // 收集请求体
        let (parts, body) = req.into_parts();
        let body_bytes = body.collect().await
            .map_err(|e| RouterError::ForwardingError(format!("Failed to collect request body: {:?}", e)))?
            .to_bytes();

        // 构建新的请求
        let mut new_req = http::Request::builder()
            .method(parts.method)
            .uri(parts.uri)
            .version(parts.version);

        // 复制头部
        for (name, value) in parts.headers.iter() {
            new_req = new_req.header(name, value);
        }

        let new_req = new_req.body(tonic::body::Body::new(
            http_body_util::Full::new(body_bytes)
        )).map_err(|e| RouterError::ForwardingError(format!("Failed to build request: {}", e)))?;

        // 发送请求到目标服务（带超时）
        let response = tokio::time::timeout(
            self.config.request_timeout(),
            channel.clone().oneshot(new_req)
        ).await
            .map_err(|_| RouterError::ForwardingError("Request timeout".to_string()))?
            .map_err(|e| RouterError::ForwardingError(format!("Failed to forward request: {}", e)))?;

        // 转换响应体
        let (parts, body) = response.into_parts();
        let body_bytes = body.collect().await
            .map_err(|e| RouterError::ForwardingError(format!("Failed to collect response body: {:?}", e)))?
            .to_bytes();

        let mut response_builder = http::Response::builder()
            .status(parts.status)
            .version(parts.version);

        // 复制响应头部
        for (name, value) in parts.headers.iter() {
            response_builder = response_builder.header(name, value);
        }

        let boxed_body = http_body_util::Full::new(body_bytes)
            .map_err(|never| -> Box<dyn std::error::Error + Send + Sync> { match never {} })
            .boxed();

        response_builder.body(boxed_body)
            .map_err(|e| RouterError::ForwardingError(format!("Failed to build response: {}", e)))
    }

    // 创建错误响应
    fn create_error_response(
        error: &RouterError,
    ) -> http::Response<BoxBody<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>> {
        let (grpc_status, message) = match error {
            RouterError::ServiceNotFound(msg) => ("5", msg.as_str()), // NOT_FOUND
            RouterError::ServiceUnavailable(msg) => ("14", msg.as_str()), // UNAVAILABLE
            RouterError::InvalidPath(msg) => ("3", msg.as_str()), // INVALID_ARGUMENT
            RouterError::ForwardingError(msg) => ("14", msg.as_str()), // UNAVAILABLE
            RouterError::LockError(msg) => ("2", msg.as_str()), // UNKNOWN
        };
        
        println!("Creating error response: {} - {}", grpc_status, message);
        
        // 使用 Result 处理而不是 unwrap()
        match http::Response::builder()
            .status(200) // HTTP status is always 200 for gRPC
            .header("grpc-status", grpc_status)
            .header("grpc-message", message)
            .header("content-type", "application/grpc")
            .body(
                http_body_util::Empty::new()
                    .map_err(|never| -> Box<dyn std::error::Error + Send + Sync> { match never {} })
                    .boxed(),
            ) {
            Ok(response) => response,
            Err(e) => {
                println!("Failed to create error response: {}", e);
                // 创建一个最小的错误响应
                http::Response::builder()
                    .status(500)
                    .body(
                        http_body_util::Empty::new()
                            .map_err(|never| -> Box<dyn std::error::Error + Send + Sync> { match never {} })
                            .boxed(),
                    )
                    .unwrap_or_default()
            }
        }
    }
}

impl<B> Service<http::Request<B>> for DynamicRouter
where
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + std::fmt::Debug,
{
    type Response = http::Response<BoxBody<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>>;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let registry = self.registry.clone();
        let router = self.clone();

        Box::pin(async move {
            let path = req.uri().path();
            
            // 解析服务名（改进的错误处理）
            let service_name = match Self::extract_service_name(path) {
                Ok(name) => name,
                Err(e) => {
                    println!("DynamicRouter: Invalid gRPC path '{}': {}", path, e);
                    return Ok(Self::create_error_response(&e));
                }
            };

            // 从注册表查找目标地址（改进的锁处理）
            let target_addr = match registry.lock() {
                Ok(registry_map) => {
                    registry_map.get(&service_name)
                        .filter(|info| info.health_status == ServiceHealthStatus::Healthy)
                        .map(|info| info.address.clone())
                }
                Err(e) => {
                    let error = RouterError::LockError(format!("Failed to acquire registry lock: {}", e));
                    println!("DynamicRouter: {}", error);
                    return Ok(Self::create_error_response(&error));
                }
            };

            println!(
                "DynamicRouter: Request for service '{}' at path '{}', target: {:?}",
                &service_name, path, target_addr
            );

            match target_addr {
                Some(addr) => {
                    // 转发请求到目标服务
                    match router.forward_request(req, &addr).await {
                        Ok(response) => Ok(response),
                        Err(e) => {
                            println!("Failed to forward request: {}", e);
                            Ok(Self::create_error_response(&e))
                        }
                    }
                }
                None => {
                    // 服务未注册
                    let error = RouterError::ServiceNotFound(
                        format!("Service '{}' not found in registry", service_name)
                    );
                    Ok(Self::create_error_response(&error))
                }
            }
        })
    }
}

impl NamedService for DynamicRouter {
    const NAME: &'static str = "grpc_opizontas.DynamicRouter";
}