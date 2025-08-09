# Recommendation Service API 定义

本文档定义了用于管理“安利小纸条”的 gRPC 服务。该服务主要负责获取、删除和屏蔽小纸条，不负责创建。

## 1. 服务定义 (Service)

### `RecommendationService`

```protobuf
service RecommendationService {
  // 根据 ID 获取一个安利小纸条
  rpc GetRecommendation (GetRecommendationRequest) returns (RecommendationSlip);

  // 删除一个小纸条
  rpc DeleteRecommendation (DeleteRecommendationRequest) returns (google.protobuf.Empty);

  // 屏蔽或取消屏蔽一个小纸条
  rpc BlockRecommendation (BlockRecommendationRequest) returns (RecommendationSlip);
}
```

## 2. 消息定义 (Messages)

### `RecommendationSlip`

“安利小纸条”的核心数据结构。

```protobuf
message RecommendationSlip {
  string id = 1;         // 安利小纸条的唯一 ID
  string author_id = 2;  // 作者的 Discord 用户 ID
  string author_nickname = 3; // 作者当时的昵称
  string content = 4;    // 安利内容
  string post_url = 5;   // 指向相关帖子的 URL
  int32 upvotes = 6;     // 获得的“点赞”数
  int32 questions = 7;   // 获得的“疑问”数
  int32 downvotes = 8;   // 获得的“点踩”数
  int64 created_at = 9;  // 创建时间戳 (Unix timestamp)
  string reviewer_id = 10; // 审定该小纸条的管理员 ID
  bool is_blocked = 11;  // 标记该小纸条是否已被屏蔽
}
```

### `GetRecommendationRequest`

```protobuf
message GetRecommendationRequest {
  string id = 1;
}
```

### `DeleteRecommendationRequest`

```protobuf
message DeleteRecommendationRequest {
  string id = 1;
}
```

### `BlockRecommendationRequest`

```protobuf
message BlockRecommendationRequest {
  string id = 1;
  bool block = 2; // true 为屏蔽, false 为取消屏蔽
}
```

**注意**: `DeleteRecommendation` 方法返回 `google.protobuf.Empty`，这是 gRPC 中表示“无内容返回”的标准方式。为了使用它，我们需要在 `.proto` 文件的开头导入 `google/protobuf/empty.proto`。