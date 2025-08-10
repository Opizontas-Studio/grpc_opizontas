use super::error::RouterError;
use http_body_util::{BodyExt, Empty};

// 创建错误响应
pub fn create_error_response(
    error: &RouterError,
) -> http::Response<
    http_body_util::combinators::UnsyncBoxBody<
        bytes::Bytes,
        Box<dyn std::error::Error + Send + Sync>,
    >,
> {
    let (grpc_status, message) = match error {
        RouterError::ServiceNotFound(msg) => ("5", msg.as_str()), // NOT_FOUND
        RouterError::ServiceUnavailable(msg) => ("14", msg.as_str()), // UNAVAILABLE
        RouterError::InvalidPath(msg) => ("3", msg.as_str()),     // INVALID_ARGUMENT
        RouterError::ForwardingError(msg) => ("14", msg.as_str()), // UNAVAILABLE
        RouterError::LockError(msg) => ("2", msg.as_str()),       // UNKNOWN
    };

    tracing::error!(status = ?grpc_status, message = %message, "Creating error response");

    // 使用 Result 处理而不是 unwrap()
    match http::Response::builder()
        .status(200) // HTTP status is always 200 for gRPC
        .header("grpc-status", grpc_status)
        .header("grpc-message", message)
        .header("content-type", "application/grpc")
        .body(http_body_util::combinators::UnsyncBoxBody::new(
            Empty::new().map_err(|never| -> Box<dyn std::error::Error + Send + Sync> {
                match never {}
            }),
        )) {
        Ok(response) => response,
        Err(e) => {
            tracing::error!("Failed to create error response: {}", e);
            // 创建一个最小的错误响应
            http::Response::builder()
                .status(500)
                .body(http_body_util::combinators::UnsyncBoxBody::new(
                    Empty::new().map_err(
                        |never| -> Box<dyn std::error::Error + Send + Sync> { match never {} },
                    ),
                ))
                .unwrap_or_default()
        }
    }
}