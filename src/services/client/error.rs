use tonic::Status;

/// 网关客户端错误类型
#[derive(Debug, thiserror::Error)]
pub enum GatewayClientError {
    #[error("Transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("gRPC error: {0}")]
    Grpc(#[from] Status),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Timeout error")]
    Timeout,
    #[error("Service not found: {0}")]
    ServiceNotFound(String),
}