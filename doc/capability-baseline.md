# Capability Baseline

这份文档用于描述项目当前已经具备的功能和能力边界。

目标不是讨论长期理想形态，而是明确当前版本到底能做什么、怎么做、还不能做什么。

## 1. 当前产品目标

当前项目是一套用 Rust 实现的可控远程终端控制面。

当前阶段的主目标是：

- 客户端主动连接服务端
- 客户端上报自己支持执行的命令
- 服务端后台由操作员手工选择命令并下发
- 客户端执行后回传结构化结果
- 服务端持久化保存 agent、任务状态和执行结果

当前项目不是：

- 任意 shell 远控工具
- 交互式终端复刻
- 已经完整接入 AI 的代理系统

AI 在当前阶段只是未来调用方，不是当前主线功能。

## 2. 当前功能列表

按主线能力划分，当前已经具备这些功能：

### 2.1 控制面引导

- 服务端提供 HTTP `bootstrap` 接口
- 客户端使用 `node_id + auth_token` 请求 bootstrap
- 服务端返回该 agent 对应的 WebSocket 地址和心跳周期

这部分负责把“短连接控制面”切换到“长连接数据面”。

### 2.2 长连接会话

- 客户端建立一条 WebSocket 长连接
- WebSocket 内承载 MQTT-like 消息语义
- MQTT-like `PUBLISH.payload` 内继续承载 Protobuf 业务消息
- 服务端为每个在线 agent 维护一条 session

### 2.3 Node 注册

- 客户端上线后发送 `ClientHello`
- 上报 `node_id`
- 上报 `hostname`
- 上报 `platform`
- 上报 `poll_interval_secs`
- 上报支持命令列表

服务端接收后会把这些信息持久化到 SQLite。

### 2.4 任务管理

- 服务端可创建任务
- 服务端可按 agent 查询任务
- 服务端可查询全局任务列表
- 服务端可取消未完成任务
- 服务端支持基础重试和断线回队

### 2.5 本地命令执行

- 客户端按已声明命令执行本地进程
- 支持默认参数
- 可按命令定义控制是否允许额外参数
- 支持采集 stdout/stderr
- 支持回传退出码、耗时、错误信息
- 支持服务端下发取消

### 2.6 后台控制台

- 后台登录
- 查看 agent 列表
- 查看某个 agent 的命令和任务
- 在 agent 工作台页手工下发命令
- 查看执行日志与结果

## 3. 模块职责

当前主线只保留 3 个 crate，职责边界如下。

### 3.1 command-plane-server

路径：

- [apps/command-server](/Users/wuhao/data/ai/ruchat/apps/command-server)

主要职责：

- 提供 HTTP bootstrap 和后台管理接口
- 接收 agent WebSocket 连接
- 维护在线 session
- 创建、派发、取消、回收任务
- 提供后台控制台
- 持久化 agent 和任务数据到 SQLite

内部按职责大致分为：

- [main.rs](/Users/wuhao/data/ai/ruchat/apps/command-server/src/main.rs)
  启动入口和监听模式
- [http.rs](/Users/wuhao/data/ai/ruchat/apps/command-server/src/http.rs)
  HTTP API 和 WS 路由装配
- [session.rs](/Users/wuhao/data/ai/ruchat/apps/command-server/src/session.rs)
  长连接会话状态机
- [db.rs](/Users/wuhao/data/ai/ruchat/apps/command-server/src/db.rs)
  SQLite 持久化和任务状态流转
- [console.rs](/Users/wuhao/data/ai/ruchat/apps/command-server/src/console.rs)
  后台页面
- [app_state.rs](/Users/wuhao/data/ai/ruchat/apps/command-server/src/app_state.rs)
  共享状态、session 信号和 admin session
- [auth.rs](/Users/wuhao/data/ai/ruchat/apps/command-server/src/auth.rs)
  agent token 校验

### 3.2 command-plane-client

路径：

- [apps/command-client](/Users/wuhao/data/ai/ruchat/apps/command-client)

主要职责：

- 读取本地 agent 配置
- 通过 HTTP bootstrap 获取 WS 地址
- 建立并维护 WS 会话
- 上报命令清单和主机信息
- 接收任务并执行本地进程
- 回传 ack、结果和取消反馈

内部按职责大致分为：

- [main.rs](/Users/wuhao/data/ai/ruchat/apps/command-client/src/main.rs)
  启动入口和重连循环
- [config.rs](/Users/wuhao/data/ai/ruchat/apps/command-client/src/config.rs)
  本地配置和命令声明
- [bootstrap.rs](/Users/wuhao/data/ai/ruchat/apps/command-client/src/bootstrap.rs)
  HTTP bootstrap 请求
- [ws.rs](/Users/wuhao/data/ai/ruchat/apps/command-client/src/ws.rs)
  WS 会话和协议处理
- [executor.rs](/Users/wuhao/data/ai/ruchat/apps/command-client/src/executor.rs)
  本地进程执行和输出采集

### 3.3 command-plane-protocol

路径：

- [crates/command-protocol](/Users/wuhao/data/ai/ruchat/crates/command-protocol)

主要职责：

- 定义 HTTP bootstrap DTO
- 定义 MQTT-like WS 帧
- 定义 Protobuf 业务载荷
- 定义 topic 约定
- 定义 agent、任务、结果的共享领域模型

内部按职责大致分为：

- [http.rs](/Users/wuhao/data/ai/ruchat/crates/command-protocol/src/http.rs)
  HTTP DTO
- [mqtt.rs](/Users/wuhao/data/ai/ruchat/crates/command-protocol/src/mqtt.rs)
  WS 内部帧语义
- [pb.rs](/Users/wuhao/data/ai/ruchat/crates/command-protocol/src/pb.rs)
  业务消息
- [topic.rs](/Users/wuhao/data/ai/ruchat/crates/command-protocol/src/topic.rs)
  topic 命名约定
- [domain.rs](/Users/wuhao/data/ai/ruchat/crates/command-protocol/src/domain.rs)
  共享业务模型

## 4. 长连接通道能力

这是当前系统最重要的能力。

当前通道模型是：

`HTTP bootstrap -> WebSocket -> MQTT-like 帧 -> Protobuf 业务消息`

### 4.1 连接建立流程

最小流程如下：

1. 客户端访问 `/api/v1/bootstrap`
2. 服务端校验 agent token
3. 服务端返回 `ws_url`
4. 客户端连接 `ws_url`
5. 客户端第一帧发送 MQTT-like `CONNECT`
6. 服务端校验 `client_id / path_node_id / auth_token`
7. 服务端返回 `CONNACK`
8. 客户端订阅任务和控制 topic
9. 客户端发送 `ClientHello`
10. 服务端完成 agent 注册并尝试派发任务

### 4.2 通道内消息语义

当前 WS 内的消息语义包括：

- `CONNECT`
- `CONNACK`
- `SUBSCRIBE`
- `SUBACK`
- `PUBLISH`
- `PINGREQ`
- `PINGRESP`

这不是独立 MQTT broker，而是项目内部定义的一层轻量消息协议。

### 4.3 Topic 约定

当前已固定的 topic 如下：

- `nodes/{node_id}/hello`
- `nodes/{node_id}/task`
- `nodes/{node_id}/ack`
- `nodes/{node_id}/result`
- `nodes/{node_id}/control`

它们各自的职责是：

- `hello`
  客户端注册和命令声明
- `task`
  服务端下发任务
- `ack`
  客户端确认已接收任务
- `result`
  客户端回传执行结果
- `control`
  服务端发送取消和错误控制消息

### 4.4 会话内调度能力

服务端维护在线 session 表，并支持通过内存信号驱动会话：

- 有新任务时通知 agent session 尝试派发
- 任务被取消时通知对应 session 下发取消信号

这使得任务不是靠轮询拉取，而是通过当前长连接直接推送。

### 4.5 心跳与存活能力

当前双端都支持心跳：

- 服务端定期向客户端发送 `PINGREQ`
- 客户端收到后返回 `PINGRESP`
- 客户端也会周期性发送 `PINGREQ`

这保证会话有基础存活检查能力。

### 4.6 断线处理能力

当前会话断开时，服务端会：

- 移除在线 session
- 将该 agent 未完成任务重新入队
- 给任务增加重试原因

这保证客户端异常断线后，任务不会永久卡死在 `dispatched` 或 `running`。

### 4.7 当前通道能力边界

当前长连接能力已经足够支撑“受控任务系统”，但还不是完整终端协议。

当前尚不具备：

- 多路并发任务复用同一 agent session
- 实时 stdout/stderr 流式推送
- 交互式 stdin 流
- PTY / TTY 终端会话
- 终端尺寸同步
- 文件传输通道
- 更强的 TLS / mTLS 信任链

所以它当前更准确地说是“任务型命令执行通道”，不是“交互型终端通道”。

### 4.8 当前长连接通信规范

当前长连接规范可按下面理解。

连接前提：

- 客户端必须先通过 HTTP bootstrap 获取 `ws_url`
- 客户端不得直接猜测或拼接 WS 地址作为正式接入方式
- `node_id` 和 `auth_token` 在 bootstrap 与 WS `CONNECT` 阶段都要保持一致

连接建立顺序：

1. 客户端连接 `ws_url`
2. 第一帧必须是二进制 `CONNECT`
3. 服务端校验 `path_node_id == CONNECT.client_id`
4. 服务端校验 `auth_token`
5. 服务端返回 `CONNACK`
6. 客户端发送 `SUBSCRIBE`
7. 客户端发送 `hello` topic 的 `ClientHello`

如果第 2 步不是 `CONNECT`，或鉴权失败，服务端会拒绝连接。

当前 MQTT-like 帧要求：

- WebSocket 业务帧使用二进制消息
- 外层统一编码为 `PbMqttFrame`
- `PUBLISH.payload` 内层统一编码为 `PbNodePayloadEnvelope`
- 当前不使用 QoS、retain、broker session 等完整 MQTT 语义

当前 topic 和载荷要求：

- `nodes/{node_id}/hello`
  只允许 `ClientHello`
- `nodes/{node_id}/task`
  只允许 `TaskAssignment`
- `nodes/{node_id}/ack`
  只允许 `TaskAck`
- `nodes/{node_id}/result`
  只允许 `TaskResult`
- `nodes/{node_id}/control`
  允许 `TaskCancel` 和 `Error`

任务收发顺序要求：

1. 服务端创建任务
2. 服务端在 session 内将任务置为 `dispatched`
3. 服务端通过 `task` topic 下发 `TaskAssignment`
4. 客户端收到后先发送 `TaskAck`
5. 服务端将任务置为 `running`
6. 客户端执行完成后发送 `TaskResult`
7. 服务端将任务更新为 `succeeded` 或 `failed`

取消与超时要求：

- 服务端取消任务时，通过 `control` topic 下发 `TaskCancel`
- 客户端收到后应终止当前本地子进程
- 服务端在旧任务退出或晚到结果被忽略前，不应继续向同一 agent 并发派发下一条任务
- 服务端任务超时后，会先回退任务到可重试状态，再向客户端发送取消信号

心跳要求：

- 双端都可以发送 `PINGREQ`
- 对端收到后应返回 `PINGRESP`
- 心跳用于连接存活检查，不承载业务状态

结果接受规则：

- 只有 `dispatched` 和 `running` 状态的任务结果会被正常接受
- `queued`、`succeeded`、`failed`、`canceled` 的晚到结果应被忽略

当前并发规则：

- 单个 agent session 同一时刻只允许一个 in-flight task
- 当前协议实现默认单任务串行执行
- 若未来支持多任务并发，需要扩展任务窗口和结果归并规则

## 5. 执行能力

执行能力是当前系统第二个核心能力。

### 5.1 执行模型

当前执行模型不是下发 shell 字符串，而是：

- 客户端本地预配置命令
- 服务端从命令声明中选择一个命令
- 服务端补充参数和超时
- 客户端按配置启动本地进程

这是一种白名单执行模型。

### 5.2 命令声明能力

客户端每个命令当前可声明这些字段：

- `name`
- `description`
- `program`
- `default_args`
- `allow_extra_args`

这意味着客户端可以把“对外暴露的命令名”和“本地真实程序路径及默认参数”解耦。

### 5.3 受控执行能力

当前执行有几个关键限制，这是合理的：

- 服务端只能选择客户端已上报的命令
- 如果命令不允许额外参数，则服务端传参会被拒绝
- 客户端直接 `Command::new(program)` 执行，不走 shell 包装

这样做的价值是：

- 避免任意字符串拼接执行
- 降低命令注入风险
- 让后续 AI 也只能在声明命令集合内活动

### 5.4 任务生命周期

当前任务状态包括：

- `queued`
- `dispatched`
- `running`
- `succeeded`
- `failed`
- `canceled`

当前还能记录这些关键时间点：

- `created_at_unix_secs`
- `dispatched_at_unix_secs`
- `acked_at_unix_secs`
- `started_at_unix_secs`
- `finished_at_unix_secs`
- `canceled_at_unix_secs`

### 5.5 结果回传能力

客户端任务完成后当前回传：

- `success`
- `exit_code`
- `stdout`
- `stderr`
- `duration_ms`
- `error`

服务端将其持久化到 SQLite，并在控制台中展示。

### 5.6 取消能力

当前取消流程为：

1. 操作员在服务端取消任务
2. 服务端将任务状态改为 `canceled`
3. 若该任务正在在线 session 中运行，则服务端通过 `control` topic 下发 `TaskCancel`
4. 客户端收到后对本地子进程执行 kill
5. 服务端会等待该旧任务退出或晚到结果被忽略后，再继续派发下一条任务

这已经具备最小可用取消能力。

超时处理当前也遵循同样原则：

- 服务端先把任务回退到可重试状态
- 同时向客户端发送取消信号
- 不会在旧进程还未退出时立刻继续向该 agent 并发派发新任务

### 5.7 当前执行能力边界

当前执行能力仍然有这些限制：

- 同一客户端同一时刻只执行一个任务
- 输出是任务结束后整块回传，不是流式输出
- 没有输出长度限制和截断策略
- 没有资源隔离，例如 cgroup、namespace、sandbox
- 没有审批策略和高危命令分级
- 没有交互式 shell / REPL 会话

因此当前更适合：

- 离散命令执行
- 运维型检查命令
- 明确边界的受控动作

不适合：

- 长时间高吞吐日志流
- 交互式运维终端
- 任意用户输入直接穿透执行

## 6. 数据与控制面能力

除了通道和执行，当前系统还具备基础控制面能力：

- agent 与命令注册持久化
- 任务创建、查看、取消
- 后台账号密码登录
- 基于 cookie 的 admin session
- SQLite 持久化

当前这部分更偏单机 PoC 和本地开发形态，已经能支撑当前阶段，但还没有进入正式多实例控制平面形态。

## 7. 当前最重要的实现边界

如果从“现阶段能否用于人工控制闭环”来看，答案是可以。

如果从“是否已经是成熟远控平台”来看，答案是否。

当前最重要的边界有 3 个：

- 它是任务系统，不是交互式终端系统
- 它是受控命令执行，不是任意 shell 执行
- 它是单机 SQLite + 内存 session 形态，不是分布式控制平台

## 8. 后续优先级

如果继续按“实用优先”推进，建议优先顺序如下：

1. 修正取消和超时后的会话状态机
2. 增加 stdout/stderr 长度限制和截断策略
3. 增加端到端集成测试，固定 `bootstrap -> ws -> hello -> task -> ack -> result`
4. 增加更明确的审计字段
5. 再决定是否要做流式输出或真正交互式终端

当前不建议立即扩展到复杂 AI 编排。

更合理的顺序是：

- 先把受控命令控制面做稳
- 再把 AI 接到这套稳定控制面之上
