
use super::registry_service::ServiceRegistry;
use futures::future::BoxFuture;
use http_body::Body;
use http_body_util::BodyExt;
use std::task::{Context, Poll};
use tower::Service;
use http_body_util::combinators::BoxBody;

// 定义动态路由服务
#[derive(Debug, Clone)]
pub struct DynamicRouter {
    pub registry: ServiceRegistry,
}



impl<B> Service<http::Request<B>> for DynamicRouter
where
    B: Body + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = http::Response<BoxBody<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>>;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let registry = self.registry.clone();

        Box::pin(async move {
            // 从请求 URI 中解析服务名
            let path = req.uri().path();
            let service_name = path.split('/').nth(1).unwrap_or("").to_string();

            // 在 await 之前释放 MutexGuard
            let target_addr = {
                let registry_map = registry.lock().unwrap();
                registry_map.get(&service_name).cloned()
            };

            println!(
                "DynamicRouter: Request for service '{}', found target: {:?}",
                &service_name, target_addr
            );

            // TODO: 实现真正的 gRPC 客户端转发
            // 临时返回一个 unimplemented 状态
            let response = http::Response::builder()
                .status(200) // gRPC status is in trailers
                .header("grpc-status", "12") // 12 = UNIMPLEMENTED
                .header("content-type", "application/grpc")
                .body(http_body_util::Empty::new().map_err(|never| -> Box<dyn std::error::Error + Send + Sync> { match never {} }).boxed())
                .unwrap();

            Ok(response)
        })
    }
}