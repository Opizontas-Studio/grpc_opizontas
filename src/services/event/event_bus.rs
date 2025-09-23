use std::sync::Arc;
use std::time::SystemTime;

use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tonic::Status;
use uuid::Uuid;

use crate::registry::{EventMessage, SubscriptionRequest};
use super::types::{EventConfig, EventError, EventStats, SubscriberInfo};

/// 基于 Tokio broadcast 的事件总线
#[derive(Debug)]
pub struct EventBus {
    /// 事件类型 -> broadcast 发送器的映射
    channels: Arc<DashMap<String, broadcast::Sender<EventMessage>>>,
    /// 订阅者信息 (订阅者ID -> 订阅信息)
    subscribers: Arc<DashMap<String, SubscriberInfo>>,
    /// 事件统计
    stats: Arc<std::sync::Mutex<EventStats>>,
    /// 配置
    config: EventConfig,
}

impl EventBus {
    /// 创建新的事件总线
    pub fn new(config: EventConfig) -> Self {
        Self {
            channels: Arc::new(DashMap::new()),
            subscribers: Arc::new(DashMap::new()),
            stats: Arc::new(std::sync::Mutex::new(EventStats::default())),
            config,
        }
    }

    /// 发布事件到指定事件类型的所有订阅者
    pub async fn publish_event(&self, mut event: EventMessage) -> Result<usize, EventError> {
        // 验证事件类型
        if event.event_type.is_empty() {
            return Err(EventError::InvalidEventType {
                event_type: event.event_type,
            });
        }

        // 如果事件ID为空，生成一个新的
        if event.event_id.is_empty() {
            event.event_id = Uuid::new_v4().to_string();
        }

        // 如果时间戳为0，设置当前时间
        if event.timestamp == 0 {
            event.timestamp = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
        }

        // 获取或创建该事件类型的广播通道
        let sender = self.get_or_create_channel(&event.event_type);
        
        // 发送事件，返回订阅者数量
        match sender.send(event.clone()) {
            Ok(subscriber_count) => {
                // 更新统计信息
                if self.config.enable_metrics {
                    if let Ok(mut stats) = self.stats.lock() {
                        stats.events_published += 1;
                        stats.events_delivered += subscriber_count as u64;
                    }
                }

                tracing::debug!(
                    event_type = %event.event_type,
                    event_id = %event.event_id,
                    subscriber_count = %subscriber_count,
                    "Published event successfully"
                );

                Ok(subscriber_count)
            }
            Err(_) => {
                // 没有订阅者或通道已关闭
                tracing::debug!(
                    event_type = %event.event_type,
                    event_id = %event.event_id,
                    "No active subscribers for event"
                );

                Err(EventError::NoSubscribers {
                    event_type: event.event_type,
                })
            }
        }
    }

    /// 订阅指定事件类型，返回事件流
    pub fn subscribe_event_type(
        &self,
        event_type: &str,
        subscriber_id: &str,
    ) -> Result<impl Stream<Item = Result<EventMessage, Status>>, EventError> {
        // 检查订阅者限制
        let current_subscriber_count = self.get_subscriber_count_for_type(event_type);
        if current_subscriber_count >= self.config.max_subscribers_per_type {
            return Err(EventError::SubscriberLimitExceeded {
                event_type: event_type.to_string(),
            });
        }

        // 获取或创建广播通道
        let sender = self.get_or_create_channel(event_type);
        let receiver = sender.subscribe();

        // 更新订阅者信息
        self.update_subscriber_info(subscriber_id, event_type);

        // 更新统计信息
        if self.config.enable_metrics {
            if let Ok(mut stats) = self.stats.lock() {
                stats.total_subscribers += 1;
            }
        }

        tracing::info!(
            event_type = %event_type,
            subscriber_id = %subscriber_id,
            "New subscription created"
        );

        // 返回转换后的流
        Ok(BroadcastStream::new(receiver)
            .map(|result| {
                result.map_err(|_err| {
                    // BroadcastStreamRecvError 不提供错误详细信息，使用通用错误
                    Status::internal("Event stream error")
                })
            }))
    }

    /// 处理订阅请求，返回是否成功
    pub async fn handle_subscription_request(
        &self,
        request: SubscriptionRequest,
    ) -> Result<(), EventError> {
        match request.action() {
            crate::registry::subscription_request::Action::Subscribe => {
                // 订阅事件 - 只记录订阅信息，实际的流创建在连接处理时进行
                for event_type in &request.event_types {
                    self.update_subscriber_info(&request.subscriber_id, event_type);
                }

                tracing::info!(
                    subscriber_id = %request.subscriber_id,
                    event_types = ?request.event_types,
                    "Subscribed to events"
                );
            }
            crate::registry::subscription_request::Action::Unsubscribe => {
                // 取消订阅
                self.unsubscribe(&request.subscriber_id, &request.event_types).await;

                tracing::info!(
                    subscriber_id = %request.subscriber_id,
                    event_types = ?request.event_types,
                    "Unsubscribed from events"
                );
            }
        }

        Ok(())
    }

    /// 获取订阅者的事件类型列表
    pub fn get_subscriber_event_types(&self, subscriber_id: &str) -> Vec<String> {
        if let Some(subscriber_info) = self.subscribers.get(subscriber_id) {
            subscriber_info.event_types.clone()
        } else {
            Vec::new()
        }
    }

    /// 取消订阅
    pub async fn unsubscribe(&self, subscriber_id: &str, event_types: &[String]) {
        // 更新订阅者信息
        if let Some(mut subscriber_info) = self.subscribers.get_mut(subscriber_id) {
            subscriber_info.event_types.retain(|et| !event_types.contains(et));
            
            if subscriber_info.event_types.is_empty() {
                drop(subscriber_info);
                self.subscribers.remove(subscriber_id);
            }
        }

        // 更新统计信息
        if self.config.enable_metrics {
            if let Ok(mut stats) = self.stats.lock() {
                stats.total_subscribers = stats.total_subscribers.saturating_sub(event_types.len());
            }
        }
    }

    /// 移除订阅者的所有订阅
    pub async fn remove_subscriber(&self, subscriber_id: &str) {
        if let Some((_, subscriber_info)) = self.subscribers.remove(subscriber_id) {
            let event_count = subscriber_info.event_types.len();
            
            // 更新统计信息
            if self.config.enable_metrics {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.total_subscribers = stats.total_subscribers.saturating_sub(event_count);
                }
            }

            tracing::info!(
                subscriber_id = %subscriber_id,
                unsubscribed_events = %event_count,
                "Removed all subscriptions for subscriber"
            );
        }
    }

    /// 获取事件统计信息
    pub fn get_stats(&self) -> EventStats {
        let base_stats = self.stats.lock().unwrap().clone();
        
        EventStats {
            active_event_types: self.channels.len(),
            total_subscribers: self.subscribers.len(),
            ..base_stats
        }
    }

    /// 获取所有订阅者信息
    pub fn get_subscribers(&self) -> Vec<SubscriberInfo> {
        self.subscribers
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// 获取指定事件类型的订阅者数量
    fn get_subscriber_count_for_type(&self, event_type: &str) -> usize {
        self.subscribers
            .iter()
            .filter(|entry| entry.value().event_types.contains(&event_type.to_string()))
            .count()
    }

    /// 获取或创建事件类型的广播通道
    fn get_or_create_channel(&self, event_type: &str) -> broadcast::Sender<EventMessage> {
        if let Some(sender) = self.channels.get(event_type) {
            sender.clone()
        } else {
            let (sender, _) = broadcast::channel(self.config.channel_capacity);
            self.channels.insert(event_type.to_string(), sender.clone());
            
            // 更新统计信息
            if self.config.enable_metrics {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.active_event_types = self.channels.len();
                }
            }
            
            tracing::debug!(
                event_type = %event_type,
                capacity = %self.config.channel_capacity,
                "Created new broadcast channel for event type"
            );
            
            sender
        }
    }

    /// 更新订阅者信息
    fn update_subscriber_info(&self, subscriber_id: &str, event_type: &str) {
        self.subscribers
            .entry(subscriber_id.to_string())
            .and_modify(|info| {
                if !info.event_types.contains(&event_type.to_string()) {
                    info.event_types.push(event_type.to_string());
                }
            })
            .or_insert_with(|| SubscriberInfo {
                subscriber_id: subscriber_id.to_string(),
                event_types: vec![event_type.to_string()],
                subscribed_at: SystemTime::now(),
                events_received: 0,
            });
    }

    /// 清理不活跃的通道
    pub async fn cleanup_inactive_channels(&self) {
        let mut to_remove = Vec::new();
        
        for entry in self.channels.iter() {
            let event_type = entry.key();
            let sender = entry.value();
            
            // 如果没有接收者，标记为待删除
            if sender.receiver_count() == 0 {
                to_remove.push(event_type.clone());
            }
        }
        
        for event_type in to_remove {
            self.channels.remove(&event_type);
            tracing::debug!(
                event_type = %event_type,
                "Cleaned up inactive broadcast channel"
            );
        }
    }
}

impl Clone for EventBus {
    fn clone(&self) -> Self {
        Self {
            channels: self.channels.clone(),
            subscribers: self.subscribers.clone(),
            stats: self.stats.clone(),
            config: self.config.clone(),
        }
    }
}