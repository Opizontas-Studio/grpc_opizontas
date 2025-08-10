# 使用指南

## 快速开始

### 1. 启动网关服务器

在一个终端中启动网关：

```bash
cargo run
```
您应该看到类似输出，具体取决于您的日志级别配置：

```
INFO grpc_opizontas: Starting gateway server...
INFO grpc_opizontas::server: Security configuration loaded successfully
INFO grpc_opizontas::server: Gateway server listening on 0.0.0.0:50051 with registry service
INFO grpc_opizontas::server: Dynamic routing enabled for all gRPC requests
```
```

## 实际使用场景

### 场景1: 微服务注册

当您的微服务启动时，它应该向网关注册自己：

```rust
use tonic::Request;

// 在您的微服务代码中
let mut client = RegistryServiceClient::connect("http://gateway:50051").await?;

// 重新注册服务
let request = Request::new(RegisterRequest {
    api_key: "your-secret-token".to_string(), // 每次心跳都需要提供
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
        api_key: "your-secret-token".to_string(), // 必须提供有效的 API Key
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

项目采用分层配置系统，**您不应该直接修改源代码来更改配置**。

配置优先级从低到高为：**默认值 -> `config.toml` -> 环境变量**。

### 1. 使用 `config.toml` 文件

在项目根目录创建或修改 `config.toml` 文件来覆盖默认值。这是一个完整的配置示例：

```toml
# config.toml

[server]
address = "0.0.0.0:50051"
log_level = "info"

[security]
# 在这里设置的 Token 会被环境变量覆盖
tokens = ["default-token-from-file"]

[router]
heartbeat_timeout = 120  # 服务心跳超时时间 (秒)
request_timeout = 30     # 转发请求的超时时间 (秒)
retry_attempts = 3
max_concurrent_requests = 1000

[connection_pool]
max_connections = 100    # 连接池最大连接数
connection_ttl = 300     # 连接最大生命周期 (秒)
idle_timeout = 60        # 连接最大空闲时间 (秒)
cleanup_interval = 30    # 连接池清理任务的运行间隔 (秒)
```

### 2. 使用环境变量 (推荐用于生产环境)

您可以通过设置环境变量来覆盖所有其他配置。这对于部署和管理密钥尤其有用。

*   `GRPC_SERVER_ADDRESS`: "0.0.0.0:8080"
*   `GRPC_LOG_LEVEL`: "debug"
*   `GRPC_SECURITY_TOKENS`: "secret-token-1,secret-token-2" (多个 Token 用逗号分隔)
*   `GRPC_ROUTER_HEARTBEAT_TIMEOUT`: 60
*   `GRPC_ROUTER_REQUEST_TIMEOUT`: 15
*   `GRPC_POOL_MAX_CONNECTIONS`: 200

## 监控和调试

### 查看服务注册日志

通过 `tracing` 框架，网关会输出详细的结构化日志：

```
INFO grpc_opizontas::services::registry_service: Registering service {service_name="PostService" address="http://post-service:50051"}
WARN grpc_opizontas::services::registry_service: Service expired, removing from registry {service_name="OldService" elapsed_secs=121}
```

### 调试技巧

1.  **启用详细日志**: 设置 `RUST_LOG=debug` 或在 `config.toml` 中设置 `log_level = "debug"` 来查看详细的路由和连接池信息。
2.  **使用 `grpcurl`**: 这是测试 gRPC 服务的强大工具，可以用来直接调用注册服务或通过网关调用后端服务。
3.  **检查服务器控制台输出**: 关注 `WARN` 和 `ERROR` 级别的日志，它们通常能直接指出问题所在。

## 故障排除

### 常见问题

1.  **连接被拒绝**:
    *   确保网关服务器正在运行。
    *   检查 `config.toml` 或 `GRPC_SERVER_ADDRESS` 环境变量中的地址和端口是否正确。

2.  **服务注册失败 (Unauthenticated)**:
    *   **检查 `api_key`**: 确保客户端请求中的 `api_key` 与服务器配置 (`security.tokens` 或 `GRPC_SECURITY_TOKENS`) 中的某个 Token 一致。这是最常见的注册失败原因。
    *   检查网络连通性。

3.  **服务被意外清理**:
    *   客户端发送心跳（重新注册）的频率是否低于 `router.heartbeat_timeout` 的值。
    *   考虑适当增加 `heartbeat_timeout` 的值。

## 下一步

项目已经实现了一个健壮的基础版本，未来的方向可以包括：

1.  **实现负载均衡策略**: 当一个服务有多个实例时，在 `services/router/forwarder.rs` 中实现轮询、最少连接或基于延迟的负载均衡。
2.  **完善指标监控**: 使用 `tokio-metrics` 或集成 `Prometheus`，暴露更详细的指标，如请求延迟、连接池状态、活动连接数等。
3.  **服务版本管理**: 允许服务注册时提供版本号，并支持按版本进行路由。
4.  **分布式配置**: 集成 `etcd` 或 `Consul` 作为配置中心和更强大的服务发现后端。