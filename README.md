# gRPC Opizontas - 动态路由网关

一个高性能的 gRPC 网关服务，支持动态服务发现和请求路由。

## 功能特性

### ✅ 已实现功能

1. **服务注册与发现**
   - gRPC 服务注册 API
   - 服务健康状态管理
   - 自动过期服务清理（60秒超时）
   - 线程安全的服务注册表

2. **动态路由核心**
   - gRPC 客户端连接池管理
   - 自动创建和缓存客户端连接
   - 基于服务名的路径解析
   - 完整的请求/响应转发逻辑

3. **错误处理**
   - gRPC 状态码标准支持
   - 服务不可用时的优雅降级
   - 详细的错误日志记录

### 🚧 待完善功能

1. **完整的路由集成**
   - 需要与 Tonic 服务器深度集成或使用反向代理
   - 支持未知服务的动态转发

2. **负载均衡**
   - 多实例服务的负载均衡策略
   - 健康检查和故障转移

3. **监控与观测**
   - 请求指标和性能监控
   - 分布式追踪支持

## 快速开始

### 启动网关服务器

```bash
cargo run
```

服务器将在 `0.0.0.0:50051` 启动，提供以下服务：

- **RegistryService**: 服务注册和管理

### 注册服务示例

使用 gRPC 客户端向网关注册一个服务：

```rust
let request = RegisterRequest {
    address: "http://bot-service:50052".to_string(),
    services: vec!["PostService".to_string(), "UserService".to_string()],
};
```

### 测试注册功能

运行测试示例：

```bash
# 启动网关（在另一个终端）
cargo run

# 运行测试客户端
cargo run --example test_registry
```

## 架构设计

```
客户端请求 -> gRPC 网关 -> 动态路由 -> 目标微服务
                ↓
          服务注册表
```

### 核心模块

- **`server.rs`**: 主服务器启动和配置
- **`services/registry_service.rs`**: 服务注册表实现
- **`services/router_service.rs`**: 动态路由逻辑
- **`services/client_manager.rs`**: gRPC 客户端连接池

### 服务发现流程

1. 微服务启动时向网关注册自身
2. 网关维护服务名到地址的映射
3. 客户端请求通过服务名路由到对应服务
4. 网关自动管理连接池和健康状态

## 配置选项

- **监听地址**: `0.0.0.0:50051` (在 `server.rs` 中配置)
- **服务超时**: `60秒` (在 `registry_service.rs` 中配置)
- **清理间隔**: `30秒` (在 `registry_service.rs` 中配置)

## 开发说明

本项目基于以下技术栈：

- **Tonic**: gRPC 框架
- **Tokio**: 异步运行时
- **Tower**: 服务抽象和中间件
- **http-body-util**: HTTP 体处理工具

### 编译和运行

```bash
# 开发版本
cargo run

# 生产版本
cargo build --release
./target/release/grpc_opizontas
```

### 代码结构

```
src/
├── main.rs              # 应用入口
├── server.rs            # 服务器配置
└── services/
    ├── mod.rs           # 模块声明
    ├── registry_service.rs    # 服务注册实现
    ├── router_service.rs      # 动态路由实现
    └── client_manager.rs      # 客户端连接池
```

## 贡献

欢迎提交 Issue 和 Pull Request 来完善这个项目！