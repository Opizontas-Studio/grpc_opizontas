use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::registry::ConnectionMessage;

// 反向连接元数据
#[derive(Debug, Clone)]
pub struct ReverseConnection {
    pub connection_id: String,
    pub services: Vec<String>,
    pub created_at: Instant,
    pub last_heartbeat: Instant,
    pub is_active: bool,
    // 用于向微服务发送请求的发送端
    pub request_sender: mpsc::UnboundedSender<ConnectionMessage>,
}

impl ReverseConnection {
    pub fn update_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    pub fn is_expired(&self, timeout: Duration) -> bool {
        Instant::now().duration_since(self.last_heartbeat) > timeout
    }
}
