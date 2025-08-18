use std::time::{Duration, Instant};
use tokio::sync::oneshot;

use crate::registry::ForwardResponse;

// 等待中的请求
#[derive(Debug)]
pub struct PendingRequest {
    pub request_id: String,
    pub created_at: Instant,
    pub response_sender: oneshot::Sender<ForwardResponse>,
}

// 流式响应处理器
#[derive(Debug)]
pub struct StreamingResponseHandler {
    pub request_id: String,
    pub chunks: std::collections::BTreeMap<i64, Vec<u8>>, // chunk_index -> data
    pub next_expected_chunk: i64,
    pub is_complete: bool,
    pub total_size: Option<i64>,
    pub response_sender: oneshot::Sender<ForwardResponse>,
}

// 反向连接管理器配置
#[derive(Debug, Clone)]
pub struct ReverseConnectionConfig {
    pub heartbeat_timeout: Duration,
    pub request_timeout: Duration,
    pub cleanup_interval: Duration,
    pub max_pending_requests: usize,
}

impl Default for ReverseConnectionConfig {
    fn default() -> Self {
        Self {
            heartbeat_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(30),
            cleanup_interval: Duration::from_secs(60),
            max_pending_requests: 1000,
        }
    }
}

// 连接统计信息
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub active_connections: usize,
    pub registered_services: usize,
}