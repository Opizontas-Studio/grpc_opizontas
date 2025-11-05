# Go 客户端连接网关指南

本指南解释了 Go 微服务如何连接到网关、注册自身并通过反向连接处理请求。

## 概述

网关采用反向连接模型。这意味着微服务（在此上下文中为“客户端”）会主动与网关建立一个持久的双向 gRPC 流。然后，网关使用此流将传入的用户请求转发到相应的微服务。

该过程包括以下关键步骤：
1.  **建立连接**: Go 服务连接到网关的 `RegistryService` 并调用 `EstablishConnection` 流式 RPC。
2.  **注册服务**: 在流上发送的第一条消息必须是 `ConnectionRegister` 消息，它告诉网关此连接将处理哪些服务。
3.  **接收连接 ID**: 网关以 `ConnectionStatus` 消息作为响应，其中包含一个唯一的 `connection_id`。此 ID **至关重要**，必须用于所有后续通信，尤其是心跳。
4.  **心跳**: 服务必须定期向网关发送 `Heartbeat` 消息以保持连接活动。
5.  **处理请求**: 网关向服务发送 `ForwardRequest` 消息。服务处理请求并返回 `ForwardResponse`。

## 先决条件

在开始之前，您需要：
1.  网关的地址（例如，`gateway.example.com:443`）。
2.  用于身份验证的有效 API 密钥。
3.  从 `.proto` 文件生成的 Go 代码。您可以使用 `protoc` 生成它：

```bash
protoc --go_out=. --go_opt=paths=source_relative \
       --go-grpc_out=. --go-grpc_opt=paths=source_relative \
       proto/registry.proto
```

## 步骤 1: 建立连接

首先，创建一个 gRPC 客户端并连接到网关。如果网关使用 TLS，您需要配置 `grpc.WithTransportCredentials`。

```go
import (
	"context"
	"log"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
	pb "path/to/your/gen/proto" // 替换为您的生成代码路径
)

func connectToGateway(gatewayAddr, apiKey string, serviceNames []string) {
	// 配置 TLS。对于生产环境，请使用正确的 CA。
	// 对于开发环境，您可以使用不安全的凭据或自签名证书。
	creds, err := credentials.NewClientTLSFromFile("path/to/ca.crt", "")
	if err != nil {
		log.Fatalf("无法加载 TLS 凭据: %v", err)
	}

	conn, err := grpc.Dial(
		gatewayAddr,
		grpc.WithTransportCredentials(creds),
		grpc.WithBlock(),
	)
	if err != nil {
		log.Fatalf("连接失败: %v", err)
	}
	defer conn.Close()

	client := pb.NewRegistryServiceClient(conn)

	// ... 建立流并处理通信
}
```

## 步骤 2: 注册和处理流

连接后，调用 `EstablishConnection` 创建双向流。您发送的第一条消息必须是 `ConnectionRegister` 消息。

```go
func establishAndManageStream(client pb.RegistryServiceClient, apiKey string, serviceNames []string) {
	stream, err := client.EstablishConnection(context.Background())
	if err != nil {
		log.Fatalf("建立连接失败: %v", err)
	}

	// 1. 发送初始注册消息
	registerMsg := &pb.ConnectionMessage{
		MessageType: &pb.ConnectionMessage_Register{
			Register: &pb.ConnectionRegister{
				ApiKey:   apiKey,
				Services: serviceNames,
			},
		},
	}
	if err := stream.Send(registerMsg); err != nil {
		log.Fatalf("发送注册消息失败: %v", err)
	}

	log.Println("成功发送注册消息。等待确认...")

	// ... 处理传入消息和心跳
}
```

## 步骤 3: 接收连接 ID 和处理消息

发送注册后，您必须侦听传入的消息。来自网关的第一条消息将是 `ConnectionStatus` 消息，其中包含您的唯一 `connection_id`。**您必须保存此 ID。**

```go
func handleIncomingMessages(stream pb.RegistryService_EstablishConnectionClient) {
    var connectionID string

    for {
        msg, err := stream.Recv()
        if err != nil {
            log.Fatalf("接收消息时出错: %v", err)
            return // 或处理重新连接逻辑
        }

        switch m := msg.MessageType.(type) {
        case *pb.ConnectionMessage_Status:
            // 这应该是您收到的第一条消息
            if m.Status.Status == pb.ConnectionStatus_CONNECTED {
                connectionID = m.Status.ConnectionId
                log.Printf("连接已建立，ID: %s", connectionID)
                // 在单独的 goroutine 中启动心跳进程
                go sendHeartbeats(stream, connectionID)
            }
        case *pb.ConnectionMessage_Request:
            // 处理来自网关的传入请求
            log.Printf("收到请求 %s，方法为 %s", m.Request.RequestId, m.Request.MethodPath)
            go handleForwardRequest(stream, m.Request)
        default:
            log.Printf("收到未处理的消息类型: %T", m)
        }
    }
}
```

## 步骤 4: 发送心跳

服务器期望定期心跳以保持连接活动。请使用您收到的 `connection_id`。**不要使用服务名称或任何其他标识符。**

```go
func sendHeartbeats(stream pb.RegistryService_EstablishConnectionClient, connID string) {
	ticker := time.NewTicker(30 * time.Second) // 根据需要调整间隔
	defer ticker.Stop()

	for range ticker.C {
		heartbeatMsg := &pb.ConnectionMessage{
			MessageType: &pb.ConnectionMessage_Heartbeat{
				Heartbeat: &pb.Heartbeat{
					Timestamp:    time.Now().Unix(),
					ConnectionId: connID, // 重要：使用连接 ID
				},
			},
		}
		if err := stream.Send(heartbeatMsg); err != nil {
			log.Printf("发送心跳失败: %v", err)
			return
		}
		log.Println("已发送心跳。")
	}
}
```

## 步骤 5: 处理转发的请求

当网关转发请求时，您将收到一条 `ForwardRequest` 消息。您的服务应处理它并发送回 `ForwardResponse`。

```go
func handleForwardRequest(stream pb.RegistryService_EstablishConnectionClient, req *pb.ForwardRequest) {
	// 示例：将有效负载回显回去
	// 在实际应用中，您将在此处调用您的实际 gRPC 服务实现。
	log.Printf("正在处理请求 %s...", req.RequestId)

	// 模拟工作
	time.Sleep(100 * time.Millisecond)

	responsePayload := []byte("Echo: ")
	responsePayload = append(responsePayload, req.Payload...)

	responseMsg := &pb.ConnectionMessage{
		MessageType: &pb.ConnectionMessage_Response{
			Response: &pb.ForwardResponse{
				RequestId:   req.RequestId,
				StatusCode:  200, // 或适当的 HTTP/gRPC 状态
				Payload:     responsePayload,
			},
		},
	}

	if err := stream.Send(responseMsg); err != nil {
		log.Printf("为请求 %s 发送响应失败: %v", req.RequestId, err)
	}
	log.Printf("已为请求 %s 发送响应", req.RequestId)
}
```
