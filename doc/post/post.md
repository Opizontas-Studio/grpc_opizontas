# Post Service API 定义

本文档定义了用于访问和查询帖子数据的 gRPC 服务。

## 1. 服务定义 (Service)

### `PostService`

该服务提供了与帖子数据交互的接口。

```protobuf
service PostService {
  // 根据 ID 获取单个帖子的详细信息
  rpc GetPost (GetPostRequest) returns (Post);

  // 根据查询条件获取帖子列表
  rpc QueryPosts (QueryPostsRequest) returns (QueryPostsResponse);
}
```

## 2. 消息定义 (Messages)

### `Post`

帖子的核心数据结构。

```protobuf
message Post {
  string id = 1;         // 帖子的唯一 ID
  string author_id = 2;  // 作者的 Discord 用户 ID
  string channel_id = 3; // 帖子所在的频道 ID
  string title = 4;      // 帖子标题
  string content = 5;    // 帖子内容 (Markdown 或纯文本)
  int64 created_at = 6;  // 创建时间戳 (Unix timestamp)
  repeated string tags = 7; // 标签，用于分类和搜索
  int32 reaction_count = 8; // 帖子的总反应数
  int32 reply_count = 9;    // 帖子的回复消息数
  string image_url = 10;  // 帖子封面图片的 URL
}
```

### `GetPostRequest`

获取单个帖子的请求体。

```protobuf
message GetPostRequest {
  string id = 1; // 要获取的帖子的 ID
  optional string channel_id = 2; // (可选) 帖子所在的频道 ID，用于校验和路由
}
```

### `QueryPostsRequest`

查询帖子的请求体，支持灵活的组合查询和分页。

```protobuf
message QueryPostsRequest {
  optional string author_id = 1;  // 按作者查询
  optional string channel_id = 2; // 按频道查询
  repeated string tags = 3;       // 按标签查询 (查询包含所有指定标签的帖子)
  int32 page_size = 4;            // 分页大小
  int32 page_number = 5;          // 页码
}
```

### `QueryPostsResponse`

查询帖子的响应体。

```protobuf
message QueryPostsResponse {
  repeated Post posts = 1; // 查询到的帖子列表
  int32 total_count = 2;   // 符合条件的总帖子数
}