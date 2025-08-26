# 使用指南

本文档将指导您如何作为 **服务提供端** 将您的服务接入网关，以及如何作为 **客户端** 通过网关调用这些服务。

## 快速开始

### 1. 启动网关服务器

在开始之前，请确保网关服务器正在运行。在一个终端中启动网关：

```bash
cargo run
```

您应该看到类似以下的输出，表明网关已成功启动并监听在 `0.0.0.0:50051`：

```
INFO grpc_opizontas: Starting gateway server...
INFO grpc_opizontas::server: Security configuration loaded successfully
INFO grpc_opizontas::server: Gateway server listening on 0.0.0.0:50051 with registry service
INFO grpc_opizontas::server: Dynamic routing enabled for all gRPC requests
```

---

## Part 1: 面向服务提供端 (如何注册您的服务)

作为服务开发者，您需要将您的 gRPC 服务注册到网关，以便客户端可以发现和调用它。

### 核心概念

1.  **服务注册**: 在您的服务启动时，您必须向网关发送一个注册请求。这个请求告诉网关您的服务名称、监听地址以及用于认证的 `api_key`。
2.  **服务心跳**: 为了让网关知道您的服务仍然存活，您必须定期（例如每30秒）重新发送注册请求。这本质上是一个心跳机制。如果网关在配置的超时时间（默认为120秒）内没有收到心跳，它将认为您的服务已下线并将其从注册表中移除。
3.  **API Key**: 每次注册或心跳请求都必须包含一个有效的 `api_key`。这个密钥用于认证，确保只有受信任的服务才能注册到网关。

### 步骤1: 定义您的 `.proto` 文件

首先，定义您的 gRPC 服务。例如，一个简单的 `PostService`：

```proto
// proto/post.proto
syntax = "proto3";

package post;

service PostService {
  rpc GetPost(GetPostRequest) returns (GetPostResponse);
}

message GetPostRequest {
  string post_id = 1;
}

message GetPostResponse {
  string title = 1;
  string content = 2;
}
```

### 步骤2: 实现并注册您的服务

接下来，在您的 Rust 微服务中，您需要：
1.  实现 `PostService`。
2.  连接到网关的 `RegistryService`。
3.  在启动时注册服务，并启动一个异步任务来定期发送心跳。

这是一个完整的示例：

```rust
// main.rs of your microservice
use std::time::Duration;
use tonic::{transport::Server, Request, Response, Status};
use tokio::time::interval;

// 假设这些是由 tonic-build 生成的代码
use post::post_service_server::{PostService, PostServiceServer};
use post::{GetPostRequest, GetPostResponse};

// 网关注册服务的客户端
use registry::registry_service_client::RegistryServiceClient;
use registry::RegisterRequest;

// 替换为您自己的模块路径
mod post {
    tonic::include_proto!("post");
}
mod registry {
    tonic::include_proto!("registry");
}

const SERVICE_NAME: &str = "PostService";
const SERVICE_ADDRESS: &str = "http://post-service:50052"; // 您的服务监听地址
const GATEWAY_ADDRESS: &str = "http://gateway:50051"; // 网关地址
const API_KEY: &str = "your-secret-token"; // 必须与网关配置匹配

#[derive(Default)]
pub struct MyPostService {}

#[tonic::async_trait]
impl PostService for MyPostService {
    async fn get_post(&self, request: Request<GetPostRequest>) -> Result<Response<Post>, Status> {
        let post_id = request.into_inner().post_id;
        println!("Received request for post: {}", post_id);
        let reply = Post {
            id: post_id,
            author_id: "user-123".to_string(),
            channel_id: "channel-456".to_string(),
            title: "Hello from PostService!".to_string(),
            content: "This is a sample post.".to_string(),
            created_at: chrono::Utc::now().timestamp(),
            tags: vec!["rust".to_string(), "grpc".to_string()],
            reaction_count: 0,
            reply_count: 0,
            image_url: "".to_string(),
        };
        Ok(Response::new(reply))
    }
}

async fn start_heartbeat() {
    loop {
        let mut interval = interval(Duration::from_secs(30));
        interval.tick().await; // 第一次立即执行

        println!("Connecting to gateway for heartbeat...");
        match RegistryServiceClient::connect(GATEWAY_ADDRESS).await {
            Ok(mut client) => {
                let request = Request::new(RegisterRequest {
                    api_key: API_KEY.to_string(),
                    address: SERVICE_ADDRESS.to_string(),
                    services: vec![SERVICE_NAME.to_string()],
                });
                if let Err(e) = client.register(request).await {
                    eprintln!("Failed to send heartbeat: {}", e);
                } else {
                    println!("Heartbeat sent successfully for {}", SERVICE_NAME);
                }
            }
            Err(e) => {
                eprintln!("Failed to connect to gateway: {}", e);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50052".parse()?;
    let post_service = MyPostService::default();

    // 启动心跳任务
    tokio::spawn(start_heartbeat());

    println!("PostService listening on {}", addr);
    Server::builder()
        .add_service(PostServiceServer::new(post_service))
        .serve(addr)
        .await?;

    Ok(())
}
```

---

## Part 2: 面向客户端 (如何通过网关调用服务)

作为客户端开发者，您不需要知道后端服务的具体地址。您只需要将所有请求发送到网关，并使用 `package.ServiceName/Method` 格式的路径即可。

### 核心概念

1.  **统一入口**: 网关是所有 gRPC 请求的唯一入口。
2.  **动态路由**: 网关会解析请求的 URL 路径（例如 `/post.PostService/GetPost`），提取服务名 `PostService`，然后在内部注册表中查找该服务的地址，最后将请求转发过去。
3.  **位置透明性**: 对客户端而言，后端服务的位置是完全透明的。您只需要与网关交互。

### 示例1: 使用 `grpcurl` (用于测试和调试)

`grpcurl` 是一个强大的命令行工具，非常适合用来测试 gRPC 服务。

```bash
# -plaintext: 因为默认配置下网关未使用TLS
# localhost:50051: 网关的地址
# post.PostService/GetPost: 调用的方法，格式为 package.Service/Method
# -d '{"post_id": "123"}': 请求的数据体 (JSON格式)

grpcurl -plaintext -d '{"post_id": "123"}' localhost:50051 post.PostService/GetPost
```

> **技术细节说明：路径解析**
> 网关的路由逻辑会解析请求路径 `/package.ServiceName/MethodName`，并提取点号 (`.`)之后的 `ServiceName` 作为服务的唯一标识符进行查找。该行为定义在 [`src/services/router/extractor.rs`](src/services/router/extractor.rs)。因此，请确保您的客户端始终使用标准的 `package.ServiceName` 格式。

如果一切正常，您将收到来自 `PostService` 的响应：

```json
{
  "title": "Hello from PostService!",
  "content": "This is a sample post."
}
```

### 示例2: 使用 Rust Tonic 客户端

在您的 Rust 客户端代码中，您只需要连接到网关地址，然后像调用普通 gRPC 服务一样调用目标服务。

```rust
// main.rs of your client application
use tonic::Request;

// 假设这是由 tonic-build 生成的代码
use post::post_service_client::PostServiceClient;
use post::GetPostRequest;

// 替换为您自己的模块路径
mod post {
    tonic::include_proto!("post");
}

const GATEWAY_ADDRESS: &str = "http://localhost:50051"; // 始终连接到网关

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 连接到网关，而不是直接连接到 PostService
    let mut client = PostServiceClient::connect(GATEWAY_ADDRESS).await?;

    // 2. 创建一个发往 PostService 的请求
    let request = Request::new(GetPostRequest {
        post_id: "456".to_string(),
    });

    // 3. 发送请求
    // Tonic 会自动将请求路径设置为 /post.PostService/GetPost
    // 网关会解析此路径并进行转发
    let response = client.get_post(request).await?;

    let post = response.into_inner();
    println!("Received Post: {} - {}", post.title, post.content);

    Ok(())
}
```

## 配置与故障排除

有关详细的配置选项、监控和故障排除指南，请参阅文档的后续部分。