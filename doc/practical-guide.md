# Practical Guide

这是一份当前版本的实用说明，目标是让你快速理解和使用项目，而不是展开长期设计讨论。

## 1. 项目现在是干什么的

当前项目是一套 Rust 实现的可控远程终端系统。

当前阶段的实际使用方式是：

- 客户端主动连接服务端
- 客户端上报自己支持执行的命令
- 操作员登录服务端控制台
- 操作员手工选择命令并下发
- 服务端记录任务状态和执行结果

后续可以在这套控制面之上再接 AI，但 AI 也应该走同一套任务模型，不直接越过控制面做任意 shell。

## 2. 当前主线结构

- `apps/command-server`
  服务端，负责 HTTP bootstrap、WS 会话、后台控制台、任务调度、SQLite 持久化
- `apps/command-client`
  客户端，负责 bootstrap、WS 连接、命令执行、结果回传
- `crates/command-protocol`
  共享协议，负责 HTTP DTO、MQTT-like WS 帧、PB 载荷

## 3. 协议和端口模型

当前协议分层：

- HTTP
  用于 bootstrap 和后台管理接口
- WebSocket
  用于客户端和服务端的长连接承载
- MQTT-like
  运行在 WebSocket 之上的生命周期消息语义
- Protobuf
  运行在 MQTT-like `PUBLISH.payload` 中的业务载荷

当前最小时序：

1. HTTP `bootstrap` 返回 `ws_url`
2. WS 内先做 `CONNECT / CONNACK`
3. 然后做 `SUBSCRIBE / SUBACK`
4. 服务端先发 `SessionInfo`
5. 客户端再发 `ClientHello` 和 `CommandCatalog`
6. 任务优先由服务端 push，下游可用 `TaskPullRequest / TaskPullResponse` 补偿

当前端口模型：

- 客户端不监听端口
- 客户端只主动发起 HTTP / WS 出站连接
- 服务端可以单端口，也可以双端口

单端口：

- HTTP 和 WS 共用一个服务端地址
- 本地开发默认 `127.0.0.1:18080`

双端口：

- HTTP 例如 `127.0.0.1:18080`
- WS 例如 `127.0.0.1:18081`
- bootstrap 通过 `RU_SERVER_PUBLIC_WS_BASE` 返回正确的 WS 地址

## 4. 当前控制台能做什么

当前控制台已经是一个最小可用工作台。

`/console/nodes/:node_id` 页面：

- 左侧是执行日志流
  展示最近任务、状态、耗时、stdout/stderr、取消按钮
- 右侧是对话式操作区
  展示 operator/system 消息流
- 右侧发送命令时支持本地 pending 消息
  先显示“正在提交”，再等待真实任务结果回写

## 5. 最常用启动方式

### 单端口开发

```bash
bash scripts/run-server-dev.sh
bash scripts/run-client-dev.sh client-config.example.json
```

访问：

- 控制台：`http://127.0.0.1:18080/console/login`

默认后台账号：

- 用户名：`admin`
- 密码：`admin123`

### 双端口开发

```bash
bash scripts/run-server-dev.sh split
bash scripts/run-client-dev.sh client-config.example.json
```

默认双端口：

- HTTP：`127.0.0.1:18080`
- WS：`127.0.0.1:18081`

## 6. 一个最小操作流程

1. 启动服务端
2. 启动客户端
3. 打开控制台并登录
4. 进入 `Nodes`
5. 点开某个 node，例如 `node-1`
6. 在右侧选择命令并发送
7. 在左侧查看执行日志和输出

建议第一个测试命令：

- `echo`

建议第二个测试命令：

- `hostname`

## 7. 当前关键配置

服务端常用环境变量：

- `RU_SERVER_BIND`
  单端口模式监听地址
- `RU_SERVER_HTTP_BIND`
  双端口模式 HTTP 监听地址
- `RU_SERVER_WS_BIND`
  双端口模式 WS 监听地址
- `RU_SERVER_PUBLIC_WS_BASE`
  bootstrap 返回给客户端的 WS 基地址
- `RU_SERVER_SHARED_TOKEN`
  node 共享 token
- `RU_ADMIN_USERNAME`
  控制台用户名
- `RU_ADMIN_PASSWORD`
  控制台密码

客户端配置重点：

- `node_id`
- `auth_token`
- `server_url`
- `commands`

## 8. 当前系统的边界

当前已经具备：

- 受控命令下发
- node 注册
- 任务状态跟踪
- 任务取消
- 控制台查看和操作
- SQLite 持久化

当前还没有：

- 正式 AI 调用入口
- 完整审计事件流
- 大输出流式处理
- 更强的鉴权模型，例如 mTLS
- 多实例共享 admin session

当前已经有：

- stdout/stderr 结果长度限制与截断标记
- 控制台对截断结果的提示

## 9. 当前最实用的使用原则

- 不要把它当任意远控 shell
- 只通过客户端声明过的命令执行操作
- 先把命令集合设计清楚，再考虑 AI 调用
- 人工控制流程先跑稳，再增加 AI 层

## 10. 推荐的下一步

如果只按“实用优先”推进，下一步建议顺序是：

1. 补集成测试，固定 `bootstrap -> ws -> task -> result`
2. 增加更明确的审计字段
3. 继续优化控制台的信息层级和任务详情展示
4. 再考虑 AI 调用层
