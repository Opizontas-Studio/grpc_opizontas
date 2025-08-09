# 项目架构设计

本文档描述了 `grpc_opizontas` 项目的整体代码架构和模块职责。

## 1. 目录结构

```
grpc_opizontas/
├── Cargo.toml
├── build.rs              # 构建脚本，用于编译 .proto 文件
├── proto/
│   └── communication.proto # API 定义
└── src/
    ├── main.rs           # 应用入口，初始化和启动服务器
    ├── server.rs         # gRPC 服务器的配置和启动逻辑
    ├── models.rs         # 定义核心业务数据结构 (例如 Post 结构体)
    ├── utils/
    │   ├── mod.rs        # 声明工具模块
    └── services/
        ├── mod.rs        # 声明服务模块
        ├── post_service.rs     # 实现 PostService gRPC 服务
        └── comm_service.rs # 实现 BotCommunicator gRPC 服务
```

## 2. 模块职责

*   **`main.rs`**: 程序的唯一入口。负责解析配置、初始化日志系统，并调用 `server` 模块来启动服务。
*   **`server.rs`**: 核心服务器模块。负责构建 `tonic` 服务，将所有 gRPC 服务实现（来自 `services` 模块）添加到路由器，并监听网络端口。
*   **`services/`**: 业务逻辑的核心。每个文件对应一个 gRPC `service` 的实现。它们接收请求，调用 `models` 或其他服务来处理数据，并返回响应。
*   **`models.rs`**: 定义应用程序的内部领域模型。这些是纯粹的 Rust 结构体，代表了业务的核心概念（如用户、帖子）。它们与 Protobuf 生成的结构体是解耦的，以提供更好的灵活性。
*   **`utils/`**: 存放项目范围内的通用工具和辅助函数。例如，自定义错误类型、配置加载器或日志初始化帮助程序。这有助于保持其他模块的简洁和专注。

## 3. 数据流图

下图展示了当一个客户端请求到达时，数据如何在系统内部流动：

graph TD
    subgraph "Bot Microservices"
        BotA[Bot A eg, Python]
        BotB[Bot B eg, Go]
    end

    subgraph "gRPC Gateway (grpc_opizontas)"
        Gateway[Tonic Server]
    end

    BotA -- "gRPC Request eg, GetPost" --> Gateway;
    Gateway -- "Forward/Format Request" --> BotB;
    BotB -- "gRPC Response" --> Gateway;
    Gateway -- "Forward Response" --> BotA;
```

**数据流说明:**

1.  **Bot A** (例如，一个命令处理 Bot) 向 **Gateway** (`grpc_opizontas`) 发送一个 `GetPost` 请求。
2.  **Gateway** 接收到请求。它知道帖子数据实际上由 **Bot B** (帖子数据 Bot) 管理。
3.  **Gateway** 作为客户端，向 **Bot B** 发送一个 `GetPost` 请求。
4.  **Bot B** 处理请求（可能会查询自己的数据库），然后将 `Post` 数据返回给 **Gateway**。
5.  **Gateway** 收到来自 **Bot B** 的响应，然后将其原封不动地转发给 **Bot A**。

在这个模型中，`grpc_opizontas` 充当了一个纯粹的、高性能的 gRPC 代理或路由器。