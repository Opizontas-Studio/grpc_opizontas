# 项目概述: grpc_opizontas

## 1. 项目目标

`grpc_opizontas` 是一个为 "旅程ΟΡΙΖΟΝΤΑΣ" Discord 服务器的 Bot 微服务生态系统设计的高性能 gRPC 通信网关。

它的核心目标是提供一个统一、稳定、可扩展的通信中枢，以解耦各个功能独立的 Bot 微服务（例如，使用 Go、Python、JavaScript 等不同语言编写的 Bot），使它们可以高效、安全地进行交互。

## 2. 核心架构

本项目被设计为一个**智能路由和格式化网关**（Smart Gateway & Router）。它不直接处理核心业务逻辑或持久化数据存储。

其主要职责包括：

*   **接收 gRPC 请求**: 作为所有 Bot 微服务通信的唯一入口点。
*   **服务发现与路由**: 根据请求的服务名称，将其转发到正确的下游 Bot 微服务。
*   **协议转换 (可选)**: 在未来可以扩展以支持协议转换，例如将 gRPC 请求转换为 RESTful API 调用。

更详细的架构设计、目录结构和数据流图，请参阅 [项目架构设计文档](./architecture.md)。

## 3. API 服务

本项目通过 Protobuf 定义了一系列 gRPC 服务。这些服务构成了微服务之间通信的契约。

*   **PostService**: 提供帖子的查询功能。
    *   [详细 API 定义](./post/init.md)
*   **RecommendationService**: 提供对“安利小纸条”的获取、删除和屏蔽功能。
    *   [详细 API 定义](./recommendation_api.md)
*   **BotCommunicator (待定)**: 负责通用的机器人间通信。

所有 API 定义都遵循 gRPC 的最佳实践，旨在实现跨语言的互操作性。

## 4. 技术选型

*   **语言**: [Rust](https://www.rust-lang.org/) - 以其高性能、内存安全和并发能力而著称。
*   **gRPC 框架**: [Tonic](https://github.com/hyperium/tonic) - 一个纯 Rust 实现的高性能 gRPC 框架。
*   **异步运行时**: [Tokio](https://tokio.rs/) - Rust 生态中最成熟的异步运行时。
*   **API 定义语言**: [Protocol Buffers (proto3)](https://developers.google.com/protocol-buffers) - 用于定义 gRPC 服务和消息。