# 使用指南

## 快速开始

### 1. 启动网关服务器

在一个终端中启动网关：

```bash
cargo run
```

您应该看到类似输出：

```
Starting gateway server...
Gateway server listening on 0.0.0.0:50051 with registry service
Dynamic routing will be integrated via custom router middleware
```

### 2. 运行演示客户端

在另一个终端中运行演示程序：

```bash
cargo run --bin usage_demo
```

这将演示如何：
- 注册多个服务到网关
- 模拟服务心跳更新
- 观察服务注册表的行为

### 3. 运行简单测试

或者运行简单的测试客户端：

```bash
cargo run --bin test_registry
```

## 实际使用场景

### 场景1: 微服务注册

当您的微服务启动时，它应该向网关注册自己：

```rust
use tonic::Request;

// 在您的微服务代码中
let mut client = RegistryServiceClient::connect("http://gateway:50051").await?;

let request = Request::new(RegisterRequest {
    address: "http://my-service:50052".to_string(),
    services: vec!["MyService".to_string()],
});

let response = client.register(request).await?;
```

### 场景2: 定期心跳

定期重新注册以保持服务活跃：

```rust
let mut interval = tokio::time::interval(Duration::from_secs(30));
loop {
    interval.tick().await;
    
    // 重新注册服务
    let request = Request::new(RegisterRequest {
        address: "http://my-service:50052".to_string(),
        services: vec!["MyService".to_string()],
    });
    
    if let Err(e) = client.register(request).await {
        eprintln!("心跳注册失败: {}", e);
    }
}
```

### 场景3: 客户端发现服务

客户端可以通过服务名向网关发送请求：

```bash
# 向网关发送 gRPC 请求，路径格式为 /package.ServiceName/Method
grpcurl -plaintext localhost:50051 package.MyService/GetData
```

## 配置选项

### 服务器配置

在 `src/server.rs` 中修改：

```rust
let addr = "0.0.0.0:50051".parse()?; // 修改监听地址
```

### 服务超时配置

在 `src/services/registry_service.rs` 中修改：

```rust
let timeout = Duration::from_secs(60); // 修改服务超时时间
```

### 清理间隔配置

```rust
let mut interval = tokio::time::interval(Duration::from_secs(30)); // 修改清理间隔
```

## 监控和调试

### 查看服务注册日志

网关会输出详细的服务注册和清理日志：

```
Registering service 'UserService' at address 'http://user-service:50052'
Service 'OldService' expired after 65s, removing from registry
```

### 服务状态检查

您可以通过修改 `registry_service.rs` 添加查询接口：

```rust
// 添加到 proto 文件和服务实现中
rpc ListServices(Empty) returns (ServiceListResponse);
```

## 扩展功能

### 添加健康检查

修改 `client_manager.rs` 添加主动健康检查：

```rust
pub async fn health_check(&self, address: &str) -> bool {
    // 实现健康检查逻辑
}
```

### 添加负载均衡

如果同一服务有多个实例，可以在 `router_service.rs` 中添加负载均衡逻辑。

### 添加指标监控

集成 Prometheus 指标：

```toml
# 在 Cargo.toml 中添加
prometheus = "0.13"
```

## 故障排除

### 常见问题

1. **连接被拒绝**
   - 确保网关服务器正在运行
   - 检查端口是否被占用

2. **服务注册失败**
   - 检查网络连通性
   - 确认 protobuf 定义一致

3. **服务被意外清理**
   - 增加心跳频率
   - 调整超时时间

### 调试技巧

1. 启用详细日志
2. 使用 `grpcurl` 测试连接
3. 检查服务器控制台输出

## 下一步

1. 实现完整的动态路由集成
2. 添加服务健康检查
3. 实现负载均衡策略
4. 添加监控和指标
5. 支持服务版本管理