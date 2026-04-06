# Remote Command Project Plan

## 1. Product Goal

This repository now serves one product goal only:

- build a controllable remote terminal system in Rust
- let the server operate clients only through a declared command set
- keep a clean control plane that can later be called by AI

Current phase:

- operator logs into the server console
- operator manually selects commands and dispatches tasks
- client executes and returns structured results

Future phase:

- AI becomes a caller of the same server control plane
- AI should not bypass the command declaration and task model

## 2. Current Mainline

Only these 3 crates are on the product mainline:

- `/Users/wuhao/data/ai/ruchat/ru-app/ru-api-server`
  Role: bootstrap API, WS session handling, task management, admin console, SQLite persistence.
- `/Users/wuhao/data/ai/ruchat/ru-app/ru-api-user`
  Role: bootstrap client, WS client, command registration, local execution, result reporting.
- `/Users/wuhao/data/ai/ruchat/ru-frame/ru-protocol/ru-command-protocol`
  Role: shared HTTP DTOs, MQTT-like WS frame definitions, protobuf payloads, topic conventions.

Everything else should be judged by whether it directly supports this mainline.

## 3. Current Design

Protocol stack:

- HTTP is used only for bootstrap and management routes.
- WebSocket is the only long-lived transport.
- MQTT-like semantics live inside WebSocket frames.
- Protobuf is used for business payloads carried by publish frames.

Execution model:

- client reports supported commands on registration
- server creates tasks only from the reported command list
- server pushes tasks over the active WS session
- client acknowledges, executes, returns result, and supports cancel

Security model today:

- agent bootstrap and WS connect use static token auth
- admin console uses username/password plus in-memory session cookie

## 4. Current Status

Already done:

- server/client/protocol mainline refactor
- SQLite task and agent persistence
- task lifecycle with queued, dispatched, running, succeeded, failed, canceled
- task ack, retry, cancel signaling
- minimal admin console with login, agent list, task submission, task result view

Still missing:

- real integration tests for the end-to-end socket flow
- output truncation and larger-result handling
- durable admin session storage
- stronger auth and audit model
- AI caller layer on top of the current manual console/API

## 5. Keep / Remove Rule

Any module or file kept in this repository should satisfy all of the following:

- it directly supports the remote terminal mainline
- it is referenced by current code or current docs
- it has a concrete owner and future purpose
- it does not describe an unrelated domain

If not, delete it.

## 6. Next Plan

Phase 1: stabilize the current manual control path

- run a real local end-to-end verification
- fix any console / task / result issues found in live flow
- add integration tests around `bootstrap -> connect -> hello -> task -> ack -> result`

Phase 2: harden the control plane

- add output size limits and truncation
- add better audit fields for operator action
- move DB access toward a cleaner async boundary or worker model
- strengthen admin and agent auth

Phase 3: prepare for AI control

- define a server-side AI calling interface that uses the same task model
- keep AI constrained to declared commands, not arbitrary shell strings
- add approval / policy points before high-risk command execution

## 7. Naming Direction

The next structural cleanup should make directory names match the product language:

- `ru-app/ru-api-server` -> `apps/command-server`
- `ru-app/ru-api-user` -> `apps/command-client`
- `ru-frame/ru-protocol/ru-command-protocol` -> `crates/command-protocol`

This is not urgent, but it should happen before the repository grows again.
