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
            RouterError::ServiceNotFound(msg) => write!(f, "Service not found: {msg}"),
            RouterError::ServiceUnavailable(msg) => write!(f, "Service unavailable: {msg}"),
            RouterError::InvalidPath(msg) => write!(f, "Invalid path: {msg}"),
            RouterError::ForwardingError(msg) => write!(f, "Forwarding error: {msg}"),
            RouterError::LockError(msg) => write!(f, "Lock error: {msg}"),
        }
    }
}

impl std::error::Error for RouterError {}
