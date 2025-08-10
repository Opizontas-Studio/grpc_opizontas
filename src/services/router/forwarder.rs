use super::error::RouterError;
use crate::config::Config;
use crate::services::client_manager::GrpcClientManager;
use http_body::Body;
use http_body_util::BodyExt;
use tower::util::ServiceExt;

// 转发 gRPC 请求到目标服务
pub async fn forward_request<B>(
    client_manager: &GrpcClientManager,
    config: &Config,
    req: http::Request<B>,
    target_addr: &str,
) -> Result<
    http::Response<
        http_body_util::combinators::UnsyncBoxBody<
            bytes::Bytes,
            Box<dyn std::error::Error + Send + Sync>,
        >,
    >,
    RouterError,
>
where
    B: Body<Data = bytes::Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>> + std::fmt::Debug,
{
    // 获取或创建客户端连接
    let channel = client_manager
        .get_or_create_client(target_addr)
        .await
        .map_err(|e| RouterError::ForwardingError(format!("Failed to get client: {e}")))?;

    // 直接构建新的请求，不收集请求体
    let (parts, body) = req.into_parts();

    // 构建新的请求
    let mut new_req = http::Request::builder()
        .method(parts.method)
        .uri(parts.uri)
        .version(parts.version);

    // 复制头部
    for (name, value) in parts.headers.iter() {
        new_req = new_req.header(name, value);
    }

    let new_req = new_req
        .body(tonic::body::Body::new(body))
        .map_err(|e| RouterError::ForwardingError(format!("Failed to build request: {e}")))?;

    // 发送请求到目标服务（带超时）
    let response = tokio::time::timeout(config.request_timeout(), channel.clone().oneshot(new_req))
        .await
        .map_err(|_| RouterError::ForwardingError("Request timeout".to_string()))?
        .map_err(|e| RouterError::ForwardingError(format!("Failed to forward request: {e}")))?;

    // 直接转换响应体，不收集响应体
    let (parts, body) = response.into_parts();

    let mut response_builder = http::Response::builder()
        .status(parts.status)
        .version(parts.version);

    // 复制响应头部
    for (name, value) in parts.headers.iter() {
        response_builder = response_builder.header(name, value);
    }

    // 使用 UnsyncBoxBody 来避免 Sync 约束
    let boxed_body = http_body_util::combinators::UnsyncBoxBody::new(
        body.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) }),
    );

    response_builder
        .body(boxed_body)
        .map_err(|e| RouterError::ForwardingError(format!("Failed to build response: {e}")))
}
