# API 消费者指南：如何通过网关调用服务

本指南面向希望通过 gRPC 网关调用后端微服务的客户端开发者（API 消费者）。

## 核心概念：透明代理

对于 API 消费者而言，gRPC 网关扮演着一个**透明代理**的角色。您不需要关心后端的服务是如何被发现或连接的。您只需要像调用一个普通的 gRPC 服务一样，将请求发送到网关即可。

**关键点：**

-   **单一入口**: 所有的 gRPC 调用都指向网关的地址。
-   **标准 gRPC**: 您使用标准的 gRPC 客户端，无需任何自定义逻辑。
-   **服务路由**: 网关会根据您调用的 gRPC 方法的完整路径（例如 `/post.PostService/GetPost`）来自动将请求转发到正确的后端微服务。
-   **内部复杂性被隐藏**: 后端服务的注册、心跳、反向连接等所有复杂机制对您都是不可见的。

## 如何调用服务

假设您希望调用一个名为 `post.PostService` 的服务，该服务有一个 `GetPost` 方法，并且已经通过反向连接注册到了网关。

### 步骤

1.  **获取 `.proto` 文件**: 您需要目标服务（例如 `post.proto`）的 `.proto` 文件来生成客户端代码。

2.  **生成客户端代码**: 使用 `protoc` 为您选择的语言生成 gRPC 客户端存根。

    例如，对于 Go：
    ```bash
    protoc --go_out=. --go_opt=paths=source_relative \
           --go-grpc_out=. --go-grpc_opt=paths=source_relative \
           proto/post.proto
    ```

3.  **创建 gRPC 客户端**: 在您的代码中，创建一个 gRPC 连接，但**目标地址是网关的地址**，而不是后端服务的直接地址。

    ```go
    // Go 示例
    import (
        "google.golang.org/grpc"
        pb "path/to/your/gen/post/proto" // 引入生成的 Post 服务代码
    )

    // 连接到网关，而不是后端服务
    gatewayAddr := "gateway.example.com:443"
    conn, err := grpc.Dial(gatewayAddr, grpc.WithInsecure()) // 在生产中使用 TLS
    if err != nil {
        log.Fatalf("无法连接到网关: %v", err)
    }
    defer conn.Close()

    // 创建的是 Post 服务的客户端，但它通过网关的连接进行通信
    postClient := pb.NewPostServiceClient(conn)
    ```

4.  **像往常一样调用 RPC**: 使用您创建的客户端直接调用目标方法。网关将负责其余的工作。

    ```go
    // Go 示例
    req := &pb.GetPostRequest{PostId: "123"}
    res, err := postClient.GetPost(context.Background(), req)
    if err != nil {
        log.Fatalf("调用 GetPost 失败: %v", err)
    }

    log.Printf("成功获取文章: %s", res.GetTitle())
    ```

## 总结

作为 API 消费者，您与网关的交互非常简单。您只需要：

-   知道网关的地址。
-   拥有您想调用的服务的 `.proto` 定义。
-   使用标准的 gRPC 客户端，将网关作为目标地址。

网关的路由和反向连接机制对您是完全透明的，让您可以像调用单个巨型服务一样调用一组分布式微服务。