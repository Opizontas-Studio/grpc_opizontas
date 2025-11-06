use std::time::Duration;
use tokio::time::timeout;
use tokio_stream::StreamExt;

use grpc_opizontas::registry::{EventMessage, SubscriptionRequest, subscription_request::Action};
use grpc_opizontas::services::event::{EventBus, EventConfig};

#[tokio::test]
async fn test_event_publish_subscribe() {
    // 创建事件总线
    let config = EventConfig {
        max_subscribers_per_type: 10,
        channel_capacity: 100,
        max_event_history: None,
        event_ttl_seconds: None,
        enable_metrics: true,
    };

    let event_bus = EventBus::new(config);

    // 测试事件类型
    let event_type = "test.event";
    let subscriber_id = "test-subscriber";

    // 创建订阅
    let mut event_stream = event_bus
        .subscribe_event_type(event_type, subscriber_id)
        .expect("Failed to subscribe to event type");

    // 创建测试事件
    let test_event = EventMessage {
        event_id: "test-event-1".to_string(),
        event_type: event_type.to_string(),
        publisher_id: "test-publisher".to_string(),
        payload: b"Hello, World!".to_vec(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
        metadata: std::collections::HashMap::new(),
    };

    // 发布事件
    let publish_result = event_bus.publish_event(test_event.clone()).await;
    assert!(publish_result.is_ok());
    assert_eq!(publish_result.unwrap(), 1); // 应该有1个订阅者

    // 接收事件
    let received_event = timeout(Duration::from_secs(1), event_stream.next())
        .await
        .expect("Timeout waiting for event")
        .expect("Stream ended unexpectedly")
        .expect("Event stream error");

    // 验证接收到的事件
    assert_eq!(received_event.event_id, test_event.event_id);
    assert_eq!(received_event.event_type, test_event.event_type);
    assert_eq!(received_event.publisher_id, test_event.publisher_id);
    assert_eq!(received_event.payload, test_event.payload);
}

#[tokio::test]
async fn test_multiple_subscribers() {
    let event_bus = EventBus::new(EventConfig::default());
    let event_type = "multi.test";

    // 创建多个订阅者
    let mut stream1 = event_bus
        .subscribe_event_type(event_type, "subscriber-1")
        .expect("Failed to subscribe subscriber-1");

    let mut stream2 = event_bus
        .subscribe_event_type(event_type, "subscriber-2")
        .expect("Failed to subscribe subscriber-2");

    // 发布事件
    let test_event = EventMessage {
        event_id: "multi-test-1".to_string(),
        event_type: event_type.to_string(),
        publisher_id: "multi-publisher".to_string(),
        payload: b"Multi subscriber test".to_vec(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
        metadata: std::collections::HashMap::new(),
    };

    let publish_result = event_bus.publish_event(test_event.clone()).await;
    assert!(publish_result.is_ok());
    assert_eq!(publish_result.unwrap(), 2); // 应该有2个订阅者

    // 两个订阅者都应该收到事件
    let event1 = timeout(Duration::from_secs(1), stream1.next())
        .await
        .expect("Timeout waiting for event on stream1")
        .expect("Stream1 ended unexpectedly")
        .expect("Stream1 error");

    let event2 = timeout(Duration::from_secs(1), stream2.next())
        .await
        .expect("Timeout waiting for event on stream2")
        .expect("Stream2 ended unexpectedly")
        .expect("Stream2 error");

    assert_eq!(event1.event_id, test_event.event_id);
    assert_eq!(event2.event_id, test_event.event_id);
}

#[tokio::test]
async fn test_subscription_management() {
    let event_bus = EventBus::new(EventConfig::default());
    let subscriber_id = "test-sub-mgmt";

    // 测试订阅请求处理
    let subscription_request = SubscriptionRequest {
        action: Action::Subscribe as i32,
        event_types: vec!["test.sub1".to_string(), "test.sub2".to_string()],
        subscriber_id: subscriber_id.to_string(),
    };

    let result = event_bus
        .handle_subscription_request(subscription_request)
        .await;
    assert!(result.is_ok());

    // 验证订阅者信息
    let event_types = event_bus.get_subscriber_event_types(subscriber_id);
    assert_eq!(event_types.len(), 2);
    assert!(event_types.contains(&"test.sub1".to_string()));
    assert!(event_types.contains(&"test.sub2".to_string()));

    // 测试取消订阅
    let unsubscribe_request = SubscriptionRequest {
        action: Action::Unsubscribe as i32,
        event_types: vec!["test.sub1".to_string()],
        subscriber_id: subscriber_id.to_string(),
    };

    let result = event_bus
        .handle_subscription_request(unsubscribe_request)
        .await;
    assert!(result.is_ok());

    // 验证订阅者信息更新
    let event_types = event_bus.get_subscriber_event_types(subscriber_id);
    assert_eq!(event_types.len(), 1);
    assert!(event_types.contains(&"test.sub2".to_string()));
}

#[tokio::test]
async fn test_event_stats() {
    let event_bus = EventBus::new(EventConfig::default());

    // 创建订阅
    let _stream = event_bus
        .subscribe_event_type("stats.test", "stats-subscriber")
        .expect("Failed to subscribe");

    // 发布事件
    let test_event = EventMessage {
        event_id: "stats-test-1".to_string(),
        event_type: "stats.test".to_string(),
        publisher_id: "stats-publisher".to_string(),
        payload: b"Stats test".to_vec(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
        metadata: std::collections::HashMap::new(),
    };

    let _ = event_bus.publish_event(test_event).await;

    // 检查统计信息
    let stats = event_bus.get_stats();
    assert_eq!(stats.active_event_types, 1);
    assert_eq!(stats.total_subscribers, 1);
    assert_eq!(stats.events_published, 1);
    assert_eq!(stats.events_delivered, 1);
}

#[tokio::test]
async fn test_no_subscribers() {
    let event_bus = EventBus::new(EventConfig::default());

    // 发布事件到没有订阅者的事件类型
    let test_event = EventMessage {
        event_id: "no-sub-test".to_string(),
        event_type: "no.subscribers".to_string(),
        publisher_id: "no-sub-publisher".to_string(),
        payload: b"No subscribers".to_vec(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
        metadata: std::collections::HashMap::new(),
    };

    let result = event_bus.publish_event(test_event).await;
    assert!(result.is_err()); // 应该返回错误，因为没有订阅者
}
