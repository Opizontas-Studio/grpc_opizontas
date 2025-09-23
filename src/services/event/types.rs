use std::time::Duration;
use thiserror::Error;
use serde::{Deserialize, Serialize};

/// 事件总线配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventConfig {
    /// 每个事件类型的最大订阅者数量
    pub max_subscribers_per_type: usize,
    /// 广播通道容量
    pub channel_capacity: usize,
    /// 事件历史保留大小 (可选功能)
    pub max_event_history: Option<usize>,
    /// 事件 TTL 秒数 (可选功能)
    pub event_ttl_seconds: Option<u64>,
    /// 是否启用事件统计
    pub enable_metrics: bool,
}

impl EventConfig {
    /// 获取事件 TTL 时长
    pub fn event_ttl(&self) -> Option<Duration> {
        self.event_ttl_seconds.map(Duration::from_secs)
    }
}

impl Default for EventConfig {
    fn default() -> Self {
        Self {
            max_subscribers_per_type: 1000,
            channel_capacity: 1024,
            max_event_history: None,
            event_ttl_seconds: None,
            enable_metrics: true,
        }
    }
}

/// 事件总线错误类型
#[derive(Error, Debug)]
pub enum EventError {
    #[error("No subscribers for event type: {event_type}")]
    NoSubscribers { event_type: String },
    
    #[error("Channel capacity exceeded for event type: {event_type}")]
    ChannelFull { event_type: String },
    
    #[error("Invalid event type: {event_type}")]
    InvalidEventType { event_type: String },
    
    #[error("Subscriber limit exceeded for event type: {event_type}")]
    SubscriberLimitExceeded { event_type: String },
    
    #[error("Broadcast receiver error: {0}")]
    BroadcastError(#[from] tokio::sync::broadcast::error::RecvError),
    
    #[error("Internal error: {0}")]
    Internal(String),
}

/// 事件统计信息
#[derive(Debug, Clone, Default)]
pub struct EventStats {
    /// 活跃的事件类型数量
    pub active_event_types: usize,
    /// 总订阅者数量
    pub total_subscribers: usize,
    /// 已发布的事件总数
    pub events_published: u64,
    /// 已投递的事件总数
    pub events_delivered: u64,
    /// 失败的投递数量
    pub delivery_failures: u64,
}

/// 订阅者信息
#[derive(Debug, Clone)]
pub struct SubscriberInfo {
    /// 订阅者ID
    pub subscriber_id: String,
    /// 订阅的事件类型
    pub event_types: Vec<String>,
    /// 订阅时间
    pub subscribed_at: std::time::SystemTime,
    /// 接收到的事件数量
    pub events_received: u64,
}