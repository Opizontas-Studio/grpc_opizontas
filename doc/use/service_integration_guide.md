# 通用服务集成指南

本指南为希望与我们的 gRPC 网关集成的所有微服务提供通用的技术说明。它不针对任何特定的编程语言，而是侧重于协议、工作流程和最佳实践。

## 核心概念：反向连接

为了实现服务发现和请求路由，网关采用了“反向连接”模型。与传统的客户端-服务器模型（客户端发起请求，服务器响应）不同，您的微服务将作为客户端，主动与网关建立一个长期的、双向的 gRPC 流。

**工作流程如下：**

1.  **您的服务连接到网关**：您的服务向网关的 `registry.RegistryService` 发起一个 `EstablishConnection` 的 gRPC 流式调用。
2.  **服务注册**：在此流上，您的服务发送的第一条消息必须是 `ConnectionRegister`。此消息包含了您的 API 密钥和您希望注册的服务名称列表。
3.  **获取连接 ID**：网关验证您的身份后，会通过流返回一个 `ConnectionStatus` 消息。此消息包含一个唯一的 `connection_id`。**这是后续所有通信的关键标识符。**
4.  **保持连接**：您的服务必须定期（例如每 30 秒）通过流发送 `Heartbeat` 消息。心跳消息必须包含从网关获取的 `connection_id`。如果网关在指定时间内未收到心跳，它将认为连接已断开并移除服务注册信息。
5.  **接收和处理请求**：当外部客户端请求您的某个服务时，网关会将请求包装成 `ForwardRequest` 消息，并通过已建立的流发送给您的服务。
6.  **返回响应**：您的服务处理请求后，将结果包装在 `ForwardResponse` 消息中，并通过同一个流发回给网关。`ForwardResponse` 必须包含原始请求的 `request_id`，以便网关将响应正确地路由回外部客户端。

## 协议详情 (`registry.proto`)

所有通信都围绕 `ConnectionMessage` 进行，它是一个 `oneof` 结构，可以包含不同类型的消息。

### 关键消息类型

-   `ConnectionRegister` (客户端 -> 网关):
    -   **用途**: 在流建立后发送的第一条消息，用于注册服务。
    -   **字段**: `api_key` (用于认证), `services` (gRPC 服务名称数组), `connection_id` (可选，如果为空，网关会生成一个新的)。

-   `ConnectionStatus` (网关 -> 客户端):
    -   **用途**: 通知客户端连接状态，特别是用于传递 `connection_id`。
    -   **字段**: `connection_id`, `status` (CONNECTED, DISCONNECTED, ERROR), `message`。

-   `Heartbeat` (客户端 -> 网关):
    -   **用途**: 维持连接的活动状态。
    -   **字段**: `timestamp`, `connection_id` (**必须**使用网关分配的 ID)。

-   `ForwardRequest` (网关 -> 客户端):
    -   **用途**: 封装来自外部客户端的实际请求。
    -   **字段**: `request_id`, `method_path` (例如 `/post.PostService/GetPost`), `headers`, `payload` (原始请求的 protobuf 字节)。

-   `ForwardResponse` (客户端 -> 网关):
    -   **用途**: 封装对 `ForwardRequest` 的响应。
    -   **字段**: `request_id` (**必须**与请求的 ID 匹配), `status_code`, `payload` (响应的 protobuf 字节), `error_message` (如果处理失败)。

## 实现步骤

1.  **生成 gRPC 代码**: 使用您选择的语言的 `protoc` 插件，从 `proto/registry.proto` 文件生成客户端存根 (stub)。

2.  **建立 TLS 连接**: 使用 gRPC 客户端库连接到网关的 `RegistryService`。生产环境**必须**使用 TLS。您需要网关的 CA 证书来建立安全连接。

3.  **调用 `EstablishConnection`**: 获取一个双向流对象。

4.  **发送注册消息**: 立即构造并发送一个 `ConnectionMessage`，其类型为 `ConnectionRegister`。

5.  **启动接收循环**: 在一个独立的线程或 goroutine 中，开始从流中接收消息。
    -   **处理第一条消息**: 期望收到 `ConnectionStatus` 消息。保存 `connection_id`。
    -   **处理后续消息**: 监听 `ForwardRequest` 消息。每收到一个请求，就派发给相应的服务逻辑进行处理。

6.  **启动心跳循环**: 在另一个独立的线程或 goroutine 中，根据预设的间隔（例如 30 秒）定期发送 `Heartbeat` 消息。

7.  **实现请求处理逻辑**:
    -   当您的接收循环收到 `ForwardRequest` 时，解析其 `method_path` 和 `payload`。
    -   调用您本地的服务实现来处理该请求。
    -   将处理结果（或错误）封装成 `ForwardResponse`。
    -   通过流将 `ForwardResponse` 发送回网关。

## 错误处理和重连

-   如果在流上发生任何 I/O 错误（例如，`stream.Recv()` 或 `stream.Send()` 返回错误），应视为连接已断开。
-   实现一个退避重连策略（例如，指数退避），在连接断开后自动尝试重新执行上述所有步骤（从建立连接开始）。
-   每次重新连接都会获得一个新的 `connection_id`。
