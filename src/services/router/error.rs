use thiserror::Error;

// 定义路由错误类型
#[derive(Error, Debug)]
pub enum RouterError {
    #[error("Service not found: {0}")]
    ServiceNotFound(String),
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("Forwarding error: {0}")]
    ForwardingError(String),
}
