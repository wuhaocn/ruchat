# Long Connection Spec

这是一份当前实现对应的最小长连接通信规范。目标不是描述未来理想形态，而是固定现在客户端和服务端必须共同遵守的协议关系。

## 1. 分层关系

固定分层如下：

1. HTTP
2. WebSocket
3. MQTT-like frame
4. Protobuf business payload

含义如下：

- HTTP 只负责 bootstrap 和管理接口，不承载长连接业务消息
- WebSocket 只负责二进制长连接承载
- MQTT-like frame 负责连接生命周期和 topic 语义
- Protobuf 负责真正的业务消息

当前明确不支持：

- HTTP 直接下发业务消息
- MQTT 作为独立 TCP broker 通道
- WS 和 MQTT 作为并列传输层

## 2. HTTP Bootstrap

客户端先请求：

- `POST /api/v1/bootstrap`

请求体最小字段：

- `node_id`
- `auth_token`

返回体关键字段：

- `node_id`
- `ws_url`
- `heartbeat_interval_secs`
- `protocol_version`
- `transport_stack`
- `capabilities`

服务端通过这个接口把客户端引导到最终的 WebSocket 地址。后续会话生命周期不再走 HTTP。

## 3. WebSocket 承载

客户端连接：

- `GET /ws/nodes/{node_id}`

要求：

- WebSocket 只承载二进制消息
- 二进制消息内容必须是 `PbMqttFrame`
- `node_id` 同时出现在 HTTP bootstrap 请求、WS 路径、MQTT `CONNECT.client_id`、业务 `ClientHello.node_id` 中

## 4. MQTT-like Frame

传输层 schema 见 [transport.proto](/Users/wuhao/data/ai/ruchat/crates/command-protocol/proto/transport.proto)。

当前支持的 frame 类型只有：

- `CONNECT`
- `CONNACK`
- `SUBSCRIBE`
- `SUBACK`
- `PUBLISH`
- `PINGREQ`
- `PINGRESP`

当前最小会话顺序：

1. client -> server: `CONNECT`
2. server -> client: `CONNACK`
3. client -> server: `SUBSCRIBE`
4. server -> client: `SUBACK`
5. server -> client: `PUBLISH(SessionInfo)`
6. client -> server: `PUBLISH(ClientHello)`
7. client -> server: `PUBLISH(CommandCatalog)`
8. server/client: 后续通过 `PUBLISH` 交换任务与结果

约束：

- 未完成 `CONNECT / CONNACK` 前不能进入业务阶段
- 未完成 `SUBSCRIBE / SUBACK` 前服务端不应向该 node 推送业务消息
- 心跳通过 `PINGREQ / PINGRESP` 完成

## 5. Topic 约定

topic 定义见 [topic.rs](/Users/wuhao/data/ai/ruchat/crates/command-protocol/src/topic.rs)。

固定 topic：

- `nodes/{node_id}/hello`
- `nodes/{node_id}/task`
- `nodes/{node_id}/ack`
- `nodes/{node_id}/result`
- `nodes/{node_id}/control`

各 topic 当前用途：

- `hello`
  承载 `ClientHello`、`CommandCatalog`
- `task`
  承载 `TaskAssignment`
- `ack`
  承载 `TaskAck`
- `result`
  承载 `TaskResult`
- `control`
  承载 `SessionInfo`、`TaskPullRequest`、`TaskPullResponse`、`TaskCancel`、`Error`

## 6. 业务消息

业务层 schema 见 [control.proto](/Users/wuhao/data/ai/ruchat/crates/command-protocol/proto/control.proto)。

当前主要消息：

- `ClientHello`
  注册 node 基本信息
- `CommandCatalog`
  单独上报支持的命令集合
- `SessionInfo`
  服务端声明当前会话信息和协议能力
- `TaskAssignment`
  服务端推送待执行任务
- `TaskAck`
  客户端确认收到任务并开始执行
- `TaskResult`
  客户端回传执行结果
- `TaskPullRequest`
  客户端主动补偿拉取任务
- `TaskPullResponse`
  服务端返回拉取结果
- `TaskCancel`
  服务端请求取消任务
- `Error`
  服务端返回协议或业务错误

当前明确约束：

- `ClientHello` 不携带命令列表
- 命令列表必须通过 `CommandCatalog` 单独上报
- 业务消息都必须包在 `PbNodePayloadEnvelope` 中

## 7. 最小业务生命周期

### 7.1 注册阶段

1. 客户端完成 bootstrap
2. 客户端建立 WS 会话并完成 MQTT-like 握手
3. 服务端先发送 `SessionInfo`
4. 客户端发送 `ClientHello`
5. 客户端发送 `CommandCatalog`
6. 服务端把 node 信息和命令集合写入 SQLite

### 7.2 推送执行阶段

1. 操作员在控制台创建任务
2. 服务端向 `nodes/{node_id}/task` 发布 `TaskAssignment`
3. 客户端收到任务后向 `nodes/{node_id}/ack` 发布 `TaskAck`
4. 客户端本地执行命令
5. 客户端向 `nodes/{node_id}/result` 发布 `TaskResult`
6. 服务端更新任务状态和结果

### 7.3 补偿拉取阶段

1. 客户端向 `nodes/{node_id}/control` 发布 `TaskPullRequest`
2. 服务端向 `nodes/{node_id}/control` 返回 `TaskPullResponse`
3. 客户端按返回任务继续执行

### 7.4 取消阶段

1. 操作员取消 `queued / dispatched / running` 任务
2. 若任务已发往客户端，服务端向 `nodes/{node_id}/control` 发布 `TaskCancel`
3. 客户端尝试终止本地子进程
4. 客户端最终仍通过 `TaskResult` 或取消后的状态回传收口

## 8. 当前实现约束

- 单 node 当前按 `single_inflight` 能力运行
- `TaskPullRequest.node_id` 必须和当前认证后的 WS 会话绑定 node 一致
- `TaskPullRequest.limit > 1` 当前也只会返回最多 1 条任务
- `TaskPullRequest.limit = 0` 返回空结果
- HTTP 管理接口当前仍是 JSON，不走 PB
- stdout/stderr 当前是任务完成后整块回传，不是流式输出

## 9. 错误与观测

当前服务端会记录会话事件：

- `connect_rejected`
- `auth_failed`
- `session_opened`
- `node_registered`
- `session_closed`

可通过以下入口查看：

- `GET /api/v1/session-events`
- `GET /api/v1/nodes/{node_id}/session-events`
- `/console/session-events`

## 10. 兼容性原则

后续修改协议时，优先遵守下面几条：

- 先分清是 HTTP 层、传输层还是业务层变化
- 不要把业务字段直接塞回 MQTT-like frame
- 不要把命令目录重新塞回 `ClientHello`
- 新增业务消息时优先扩展 `PbNodePayloadEnvelope`
- 新增生命周期语义时优先扩展 `transport.proto`
