# command-plane

一个用 Rust 实现的 AI 可控远程终端系统。

当前阶段先做人控闭环：

- 客户端启动后连接服务端并上报支持的命令。
- 服务端后台登录后查看客户端和命令列表。
- 操作员手工选择命令下发执行。
- 服务端保存任务状态和结果。

后续再在服务端之上增加 AI 调用层，把“AI -> 服务端控制面 -> 客户端受控执行”这条链路接起来。

当前仓库主线只包含 3 个 crate：

- `command-plane-server`：服务端，负责 HTTP bootstrap、WebSocket 会话、任务管理、后台控制台和 SQLite 持久化。
- `command-plane-client`：部署在目标机器上的客户端，先通过 HTTP 获取地址，再通过 WebSocket 建立单一会话。
- `command-plane-protocol`：客户端与服务端共享的数据协议，HTTP 控制面用 JSON，WebSocket 内部是 MQTT-like 语义 + Protobuf 载荷。

手工联调可直接参考 [doc/manual-e2e.md](/Users/wuhao/data/ai/ruchat/doc/manual-e2e.md)，日常使用说明可参考 [doc/practical-guide.md](/Users/wuhao/data/ai/ruchat/doc/practical-guide.md)，也可以使用 [scripts/run-server-dev.sh](/Users/wuhao/data/ai/ruchat/scripts/run-server-dev.sh) 和 [scripts/run-client-dev.sh](/Users/wuhao/data/ai/ruchat/scripts/run-client-dev.sh)。

端口模型当前是：

- 服务端使用固定监听端口，本地开发默认 `18080`
- 客户端不监听端口，只主动发起 HTTP 和 WS 连接
- HTTP 和 WS 当前复用同一个服务端监听端口
- 也支持拆分成双监听：`RU_SERVER_HTTP_BIND` 和 `RU_SERVER_WS_BIND`

## 设计说明

这套实现不是“服务端直接远程 shell 到客户端”，而是一个受控任务系统：

- 客户端先通过 HTTP `bootstrap` 接口获取 `ws://` 地址。
- 客户端随后通过 WebSocket 建立单一连接。
- WebSocket 连接内部承载一层 MQTT-like 语义：`CONNECT / SUBSCRIBE / PUBLISH / PING`。
- 业务载荷继续使用 Protobuf，放在 `PUBLISH.payload` 里。
- 服务端只能从客户端已声明的命令中选一个创建任务。
- 服务端在长连接上直接下发任务，客户端执行完成后再通过同一个连接回传结果。

这样做的好处是：

- 服务端无法随意执行客户端未声明的命令。
- 避免 `sh -c` / `cmd /C` 这类字符串拼接带来的注入风险。
- 控制面和传输面分离，后续更容易接入 AI 调度层、鉴权和审计日志。

当前产品边界：

- 当前阶段是“人工在后台下发命令”。
- AI 目前还不是直接接入点，只是后续扩展方向。
- 当前目标是把“可控远程终端”的底层控制面和执行链路做稳。

## Workspace

```text
apps/
  command-server/   # command-plane-server
  command-client/   # command-plane-client
crates/
  command-protocol/ # command-plane-protocol
```

当前不在这条主线上的历史实验代码和无关文档已清理，后续新增内容也应直接服务这 3 个 crate。

## 启动服务端

```bash
RU_SERVER_BIND=0.0.0.0:18080 \
RU_SERVER_DB_PATH=./command-plane.db \
RU_SERVER_PUBLIC_WS_BASE=ws://127.0.0.1:18080/ws/agents \
RU_SERVER_SHARED_TOKEN=dev-shared-token \
cargo run -p command-plane-server
```

默认监听 `0.0.0.0:18080`，默认 SQLite 文件是当前目录下的 `command-plane.db`。

可选环境变量：

- `RU_SERVER_PUBLIC_WS_BASE`：bootstrap 返回给客户端的 WebSocket 基地址，例如 `ws://127.0.0.1:18080/ws/agents`
- `RU_SERVER_HTTP_BIND`：可选，单独指定 HTTP 控制面监听地址
- `RU_SERVER_WS_BIND`：可选，单独指定 WS 监听地址
- `RU_SERVER_HEARTBEAT_SECS`：应用层心跳秒数，默认 `15`
- `RU_ADMIN_USERNAME`：后台控制台用户名，默认 `admin`
- `RU_ADMIN_PASSWORD`：后台控制台密码，默认 `admin123`
- `RU_ADMIN_SESSION_TTL_SECS`：后台登录会话有效期，默认 `28800`
- `RU_SERVER_SHARED_TOKEN`：所有 agent 共用的静态 token，适合单环境 PoC
- `RU_SERVER_AGENT_TOKENS_JSON`：按 agent_id 指定 token 的 JSON，例如 `{"node-1":"token-a","node-2":"token-b"}`

启动后可直接访问 `http://127.0.0.1:18080/console/login` 进入后台控制台。当前阶段建议直接通过控制台人工下发命令，不再把管理接口当主要操作入口。

如果拆分 HTTP / WS 端口，建议这样启动：

```bash
RU_SERVER_HTTP_BIND=0.0.0.0:18080 \
RU_SERVER_WS_BIND=0.0.0.0:18081 \
RU_SERVER_PUBLIC_WS_BASE=ws://127.0.0.1:18081/ws/agents \
RU_SERVER_SHARED_TOKEN=dev-shared-token \
cargo run -p command-plane-server
```

注意：

- 客户端 `server_url` 仍然指向 HTTP 地址，例如 `http://127.0.0.1:18080`
- bootstrap 返回的 `ws_url` 会来自 `RU_SERVER_PUBLIC_WS_BASE`
- 当 `HTTP` 和 `WS` 分端口时，应显式配置 `RU_SERVER_PUBLIC_WS_BASE`

开发环境也可以直接运行：

```bash
bash scripts/run-server-dev.sh
bash scripts/run-client-dev.sh client-config.example.json
```

双端口开发模式也可以直接运行：

```bash
bash scripts/run-server-dev.sh split
bash scripts/run-client-dev.sh client-config.example.json
```

## 启动客户端

仓库里已经带了一个示例文件 [client-config.example.json](/Users/wuhao/data/ai/ruchat/client-config.example.json)，也可以自己准备 `client-config.json`：

```json
{
  "agent_id": "node-1",
  "auth_token": "dev-shared-token",
  "server_url": "http://127.0.0.1:18080",
  "poll_interval_secs": 3,
  "request_timeout_secs": 10,
  "commands": [
    {
      "name": "hostname",
      "description": "打印当前主机名",
      "program": "hostname",
      "default_args": [],
      "allow_extra_args": false
    },
    {
      "name": "echo",
      "description": "回显文本",
      "program": "echo",
      "default_args": [],
      "allow_extra_args": true
    },
    {
      "name": "list_tmp",
      "description": "列出 /tmp 目录",
      "program": "ls",
      "default_args": [
        "-la",
        "/tmp"
      ],
      "allow_extra_args": false
    }
  ]
}
```

然后运行：

```bash
cargo run -p command-plane-client -- client-config.json
```

客户端会先带 token 访问 `http://.../api/v1/bootstrap`，然后在 WebSocket `CONNECT` 帧里再次带上同一个 token。

## MQTT 说明

- 这里的 MQTT 不是独立 broker/TCP 通道，而是 WebSocket 之上的一层消息语义。
- 客户端进入 WS 后先发送带鉴权信息的 `CONNECT`，再 `SUBSCRIBE` 自己的 topic，然后通过 `PUBLISH` 收发业务消息。
- 心跳也走这层语义，连接两端都可以发 `PINGREQ`，对端返回 `PINGRESP`。
- 当前内置的 topic 约定是 `agents/{agent_id}/hello`、`agents/{agent_id}/task`、`agents/{agent_id}/ack`、`agents/{agent_id}/result`、`agents/{agent_id}/control`。
- `PUBLISH.payload` 中承载的是 Protobuf 业务消息。

## 当前最小能力

- 客户端上线后会注册自己和支持命令。
- 后台登录后可以查看在线客户端和命令列表。
- 后台可以选择已声明命令创建任务。
- 客户端执行后会回传 stdout/stderr、耗时和退出码。
- 服务端会保存任务状态，支持取消和基础重试。

## 主要接口

### 1. Bootstrap 下发地址

```bash
curl -X POST http://127.0.0.1:18080/api/v1/bootstrap \
  -H 'content-type: application/json' \
  -d '{
    "agent_id":"node-1",
    "auth_token":"dev-shared-token"
  }'
```

返回值示例：

```json
{
  "agent_id": "node-1",
  "ws_url": "ws://127.0.0.1:18080/ws/agents/node-1",
  "heartbeat_interval_secs": 15
}
```

### 2. 查看已注册客户端

```bash
curl http://127.0.0.1:18080/api/v1/agents
```

注意：`/api/v1/agents`、`/api/v1/tasks` 这类管理接口现在要求先登录后台控制台并带上 `ru_admin_session` cookie；`/health`、`/api/v1/bootstrap`、`/ws/agents/:agent_id` 不受 admin 登录保护。

客户端真正注册、订阅、任务接收和结果回传都发生在 WebSocket 二进制消息中。传输层帧定义见 [mqtt.rs](/Users/wuhao/data/ai/ruchat/crates/command-protocol/src/mqtt.rs) 里的 `PbMqttFrame`，业务载荷定义见 [pb.rs](/Users/wuhao/data/ai/ruchat/crates/command-protocol/src/pb.rs) 里的 `PbAgentPayloadEnvelope` / `PbClientHello` / `PbTaskAssignment` / `PbTaskResult`。

### 3. 创建执行任务

```bash
curl -X POST http://127.0.0.1:18080/api/v1/tasks \
  -H 'content-type: application/json' \
  -d '{
    "agent_id":"node-1",
    "command_name":"echo",
    "args":["hello","world"],
    "timeout_secs":30
  }'
```

任务创建后，如果对应客户端在线，服务端会通过当前 WebSocket 会话里的 `PUBLISH(topic=agents/{agent_id}/task)` 立即推送任务。客户端收到后会先回 `agents/{agent_id}/ack`，服务端再把任务状态切到 `running`。

### 4. 取消未开始任务

```bash
curl -X POST http://127.0.0.1:18080/api/v1/tasks/1/cancel \
  -H 'content-type: application/json' \
  -d '{
    "reason":"operator canceled"
  }'
```

当前取消支持 `queued`、`dispatched` 和 `running`。当任务已经在客户端执行时，服务端会通过 `agents/{agent_id}/control` 下发 `TaskCancel`，客户端收到后会尝试终止本地子进程。

### 5. 查看任务执行结果

```bash
curl http://127.0.0.1:18080/api/v1/tasks
curl http://127.0.0.1:18080/api/v1/tasks/1
```

## 最小操作流程

1. 启动服务端，打开 `http://127.0.0.1:18080/console/login`，用 admin 账号登录。
2. 启动客户端，客户端会先 bootstrap，再通过 WS 注册自己和支持的命令。
3. 在控制台 `Agents` 页面查看客户端和其支持命令。
4. 进入某个 agent 详情页，选择已上报命令并创建任务。
5. 在 `Tasks` 页面观察状态从 `queued/dispatched/running` 进入 `succeeded/failed/canceled`，同时查看 stdout/stderr。

## 当前限制

- 服务端虽然已持久化到 SQLite，但当前仍是单进程串行访问模型，适合轻量控制面。
- 当前只有静态 token 鉴权，没有签名、证书和更细粒度授权，不适合直接暴露到公网。
- 当前后台控制台是服务端渲染的最小实现，适合验证闭环，不适合直接作为正式运维台。
- 当前 MQTT-like 语义只实现了 agent 场景需要的最小子集，不是完整 MQTT 协议实现。
- WebSocket 内的业务载荷已切成 Protobuf，但 HTTP 控制面仍是 JSON。
- 客户端执行命令时会一次性收集输出，超大输出暂未做流式限制。
- 当前 timeout 仍然是服务端基于下发租约做自动重试，还没有升级成“先 cancel 再确认退出”的两阶段超时回收。
- 当前还没有真正的 AI 调用入口，AI 只是在产品定位里属于下一层控制者。

## 后续建议

- 先补真实联调和集成测试，确保“登录后台 -> 看到 agent -> 下发命令 -> 收到结果”稳定可复现。
- 为任务增加输出截断、任务详情页和更清楚的审计字段。
- 增加签名、轮换和 mTLS，替换当前静态 token 模式。
- 将 SQLite 数据访问从同步调用改成独立 worker 或连接池，降低阻塞影响。
- 在服务端控制面稳定后，再增加 AI 调用层，由 AI 通过受控接口选择 agent 和命令，而不是直接越过控制面。
