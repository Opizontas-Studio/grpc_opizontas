# 项目概述: grpc_opizontas

## 1. 项目目标

`grpc_opizontas` 是一个为 "旅程ΟΡΙΖΟΝΤΑΣ" Discord 服务器的 Bot 微服务生态系统设计的高性能 gRPC 通信网关。

它的核心目标是提供一个统一、稳定、可扩展的通信中枢，以解耦各个功能独立的 Bot 微服务（例如，使用 Go、Python、JavaScript 等不同语言编写的 Bot），使它们可以高效、安全地进行交互。

## 2. 核心架构

本项目被设计为一个**具备服务发现能力的高性能 gRPC 路由网关**。它不直接处理核心业务逻辑，而是专注于网络通信的中间层。

其主要职责包括：

*   **安全的服务注册**: 提供 `RegistryService`，允许下游 Bot 通过 `api_key` 安全地注册自身。
*   **动态服务发现与路由**: 拦截所有 gRPC 请求，根据服务名在注册表中查找健康的实例，并将请求动态转发。
*   **服务健康检查**: 自动监控已注册服务的心跳，并剔除无响应的实例，保证路由的高可用性。
*   **高性能连接池**: 管理到下游服务的 gRPC 连接，通过复用和智能回收机制减少延迟。

更详细的架构设计、目录结构和数据流图，请参阅 [项目架构设计文档](./architecture.md)。

## 3. API 服务

本项目通过 Protobuf 定义服务，这些服务构成了微服务之间通信的契约。

*   **`RegistryService` (由网关提供)**: 允许后端 Bot 进行服务注册和心跳维持。这是网关的核心管理服务。
*   **`PostService` (下游服务示例)**: 提供帖子的查询功能。
    *   [详细 API 定义](./post/post.md)
*   **`RecommendationService` (下游服务示例)**: 提供对“安利小纸条”的获取、删除和屏蔽功能。
    *   [详细 API 定义](./post/recommendation_api.md)
*   **`BotCommunicator` (待定)**: 负责通用的机器人间通信。

所有 API 定义都遵循 gRPC 的最佳实践，旨在实现跨语言的互操作性。

## 4. 技术选型

*   **语言**: [Rust](https://www.rust-lang.org/) - 以其高性能、内存安全和并发能力而著称。
*   **gRPC 框架**: [Tonic](https://github.com/hyperium/tonic) - 一个纯 Rust 实现的高性能 gRPC 框架。
*   **异步运行时**: [Tokio](https://tokio.rs/) - Rust 生态中最成熟的异步运行时。
*   **并发数据结构**: [DashMap](https://github.com/xacrimon/dashmap) - 用于实现高性能、线程安全的服务注册表。
*   **配置与序列化**: [Serde](https://serde.rs/) 和 [TOML](https://toml.io/en/) - 用于灵活的配置文件处理。
*   **API 定义语言**: [Protocol Buffers (proto3)](https://developers.google.com/protocol-buffers) - 用于定义 gRPC 服务和消息。