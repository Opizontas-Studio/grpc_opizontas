
use super::client_manager::GrpcClientManager;
use super::registry_service::{ServiceRegistry, ServiceHealthStatus};
use futures::future::BoxFuture;
use http_body::Body;
use http_body_util::BodyExt;
use std::task::{Context, Poll};
use tower::{Service, util::ServiceExt};
use http_body_util::combinators::BoxBody;

// 定义动态路由服务
#[derive(Debug, Clone)]
pub struct DynamicRouter {
    pub registry: ServiceRegistry,
    pub client_manager: GrpcClientManager,
}

impl DynamicRouter {
    pub fn new(registry: ServiceRegistry) -> Self {
        Self {
            registry,
            client_manager: GrpcClientManager::default(),
        }
    }

    // 从 gRPC 路径解析服务名
    // 例如："/package.ServiceName/MethodName" -> "ServiceName"
    fn extract_service_name(path: &str) -> Option<String> {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if parts.len() >= 2 {
            // 从 "package.ServiceName" 中提取 ServiceName
            if let Some(service_part) = parts[0].split('.').last() {
                return Some(service_part.to_string());
            }
        }
        None
    }

    // 转发 gRPC 请求到目标服务
    async fn forward_request<B>(
        &self,
        req: http::Request<B>,
        target_addr: &str,
    ) -> Result<http::Response<BoxBody<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>>, Box<dyn std::error::Error + Send + Sync>>
    where
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + std::fmt::Debug,
    {
        // 获取或创建客户端连接
        let channel = self.client_manager.get_or_create_client(target_addr).await?;
        
        // 收集请求体
        let (parts, body) = req.into_parts();
        let body_bytes = body.collect().await
            .map_err(|e| format!("Failed to collect request body: {:?}", e))?
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
        ))?;

        // 发送请求到目标服务
        let response = channel
            .clone()
            .oneshot(new_req)
            .await
            .map_err(|e| format!("Failed to forward request: {}", e))?;

        // 转换响应体
        let (parts, body) = response.into_parts();
        let body_bytes = body.collect().await
            .map_err(|e| format!("Failed to collect response body: {:?}", e))?
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

        Ok(response_builder.body(boxed_body)?)
    }

    // 创建错误响应
    fn create_error_response(
        grpc_status: &str,
        message: &str,
    ) -> http::Response<BoxBody<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>> {
        println!("Creating error response: {} - {}", grpc_status, message);
        
        http::Response::builder()
            .status(200) // HTTP status is always 200 for gRPC
            .header("grpc-status", grpc_status)
            .header("grpc-message", message)
            .header("content-type", "application/grpc")
            .body(
                http_body_util::Empty::new()
                    .map_err(|never| -> Box<dyn std::error::Error + Send + Sync> { match never {} })
                    .boxed(),
            )
            .unwrap()
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
            
            // 解析服务名
            let service_name = match Self::extract_service_name(path) {
                Some(name) => name,
                None => {
                    println!("DynamicRouter: Invalid gRPC path: {}", path);
                    return Ok(Self::create_error_response(
                        "5", // NOT_FOUND
                        "Invalid gRPC path format",
                    ));
                }
            };

            // 从注册表查找目标地址
            let target_addr = {
                let registry_map = registry.lock().unwrap();
                registry_map.get(&service_name)
                    .filter(|info| info.health_status == ServiceHealthStatus::Healthy)
                    .map(|info| info.address.clone())
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
                            Ok(Self::create_error_response(
                                "14", // UNAVAILABLE
                                &format!("Service unavailable: {}", e),
                            ))
                        }
                    }
                }
                None => {
                    // 服务未注册
                    Ok(Self::create_error_response(
                        "5", // NOT_FOUND
                        &format!("Service '{}' not found in registry", service_name),
                    ))
                }
            }
        })
    }
}