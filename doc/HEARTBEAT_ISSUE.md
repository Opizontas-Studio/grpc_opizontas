# 心跳包连接ID使用说明

## 问题描述

在使用 grpc_opizontas 网关服务时，发现微服务客户端在发送心跳包时使用了错误的 `connection_id`，导致心跳包无法正确更新连接状态，最终造成服务被误判为过期并从注册表中移除。

## 错误示例

从日志中可以看到以下错误模式：

```log
2025-09-19T02:16:53.212186Z DEBUG grpc_opizontas::services::registry::grpc_impl: Received heartbeat message heartbeat_connection_id=amwaybot
2025-09-19T02:16:53.212238Z  WARN grpc_opizontas::services::connection::manager: Received heartbeat for unknown connection_id connection_id=amwaybot

2025-09-19T02:17:02.341148Z DEBUG grpc_opizontas::services::registry::grpc_impl: Received heartbeat message heartbeat_connection_id=
2025-09-19T02:17:02.341184Z  WARN grpc_opizontas::services::connection::manager: Received heartbeat for unknown connection_id connection_id=
```

## 错误原因

1. **使用服务名作为连接ID**：如 `connection_id=amwaybot`，这是服务名而不是连接ID
2. **使用空字符串**：如 `connection_id=`，完全没有提供连接ID

## 正确做法

### 1. 连接ID的获取

在建立反向连接时，网关会返回一个 UUID 格式的连接ID，例如：
```
connection_id=4d4e7acc-d3cd-4c3d-9d98-d06b3144f891
connection_id=a9661610-d5ae-4fe8-bbfa-6f98d09cdccf
```

### 2. 心跳包发送

客户端在发送心跳包时，必须使用从网关获得的真实连接ID：

```protobuf
message Heartbeat {
    string connection_id = 1;  // 必须使用网关返回的UUID格式连接ID
    int64 timestamp = 2;
}
```

### 3. 连接建立流程

1. **连接注册**：客户端向网关发送连接注册请求
2. **获取连接ID**：网关返回 UUID 格式的 `connection_id`
3. **保存连接ID**：客户端保存此连接ID用于后续心跳包
4. **发送心跳**：使用保存的连接ID发送心跳包

## 常见错误及修复

### 错误1：使用服务名作为连接ID
```go
// ❌ 错误
heartbeat := &pb.Heartbeat{
    ConnectionId: "amwaybot",  // 这是服务名，不是连接ID
}
```

```go
// ✅ 正确
heartbeat := &pb.Heartbeat{
    ConnectionId: connectionIdFromGateway,  // 使用从网关获得的UUID
}
```

### 错误2：使用空连接ID
```go
// ❌ 错误
heartbeat := &pb.Heartbeat{
    ConnectionId: "",  // 空连接ID
}
```

```go
// ✅ 正确
heartbeat := &pb.Heartbeat{
    ConnectionId: connectionIdFromGateway,  // 使用从网关获得的UUID
}
```

## 检查方法

### 客户端检查
在发送心跳包前，验证连接ID格式：
```go
func isValidConnectionId(connectionId string) bool {
    // UUID格式检查：xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
    uuidRegex := regexp.MustCompile(`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`)
    return uuidRegex.MatchString(connectionId)
}
```

### 日志监控
监控以下警告日志：
```log
WARN grpc_opizontas::services::connection::manager: Received heartbeat for unknown connection_id
```

如果出现此日志，说明客户端使用了错误的连接ID。

## 影响

使用错误的连接ID会导致：
1. 心跳包无法更新连接状态
2. 连接被误判为过期
3. 服务从注册表中被移除
4. 客户端请求失败

## 总结

**重要**：始终使用从网关获得的 UUID 格式连接ID发送心跳包，切勿使用服务名或空字符串。这是确保服务正常运行的关键要求。