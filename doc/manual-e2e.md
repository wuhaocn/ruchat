# Manual E2E Quickstart

This document covers the current target flow:

1. start the server
2. log into the console
3. start one client
4. verify the client reports its command set
5. manually dispatch a command
6. confirm the result is stored and visible

## 1. Build

```bash
cargo build
```

## 2. Port Model

- server uses one fixed listening address, default `127.0.0.1:18080` in local dev
- client does not expose a listening port
- client only makes outbound HTTP and WS connections to the server
- HTTP and WS currently share the same server listener

Current code also supports split listeners:

- `RU_SERVER_HTTP_BIND=127.0.0.1:18080`
- `RU_SERVER_WS_BIND=127.0.0.1:18081`
- `RU_SERVER_PUBLIC_WS_BASE=ws://127.0.0.1:18081/ws/nodes`

When split mode is used:

- browser and client bootstrap still go to HTTP
- actual WebSocket session goes to the WS port
- `RU_SERVER_PUBLIC_WS_BASE` should be set explicitly

Helper script form:

```bash
bash scripts/run-server-dev.sh split
```

## 3. Start Server

Recommended local dev settings:

```bash
RU_SERVER_BIND=127.0.0.1:18080 \
RU_SERVER_DB_PATH=./command-plane.db \
RU_SERVER_PUBLIC_WS_BASE=ws://127.0.0.1:18080/ws/nodes \
RU_SERVER_SHARED_TOKEN=dev-shared-token \
RU_ADMIN_USERNAME=admin \
RU_ADMIN_PASSWORD=admin123 \
cargo run -p command-plane-server
```

Or use the helper script:

```bash
bash scripts/run-server-dev.sh
```

After startup:

- console URL: `http://127.0.0.1:18080/console/login`
- default username: `admin`
- default password: `admin123`

## 4. Prepare Client Config

Use the existing example:

```bash
cp client-config.example.json client-config.json
```

The default sample exposes these commands:

- `hostname`
- `echo`
- `list_tmp`

## 5. Start Client

```bash
cargo run -p command-plane-client -- client-config.json
```

Expected behavior:

- client calls `/api/v1/bootstrap`
- client connects to `/ws/nodes/node-1`
- client reports its command list
- server stores the agent record

## 6. Verify In Console

Open `http://127.0.0.1:18080/console/login`.

After login:

- open `Nodes`
- confirm `node-1` is present
- open the agent detail page
- confirm the command cards are visible

## 7. Dispatch A Command

Recommended first test:

- choose `echo`
- enter extra args as multiple lines, for example:

```text
hello
from
console
```

- optionally set timeout to `30`
- submit the form

## 8. Confirm Result

Open `Tasks` and check:

- task status moves through `queued`, `dispatched`, `running`
- final state becomes `succeeded` or `failed`
- stdout and stderr are visible in the task row details

Recommended second test:

- run `hostname`
- confirm stdout matches the client host

## 9. Cancel Test

If you register a longer-running command later:

- create a task
- hit `Cancel` in the task list
- confirm final state becomes `canceled`

## 10. Failure Checklist

If the agent does not appear:

- confirm server and client use the same token
- confirm `RU_SERVER_PUBLIC_WS_BASE` points to the running server
- confirm `node_id` in config matches the WS path returned by bootstrap

If the task stays queued:

- confirm the client is still connected
- check server logs for session close or auth errors

If login fails:

- confirm `RU_ADMIN_USERNAME` and `RU_ADMIN_PASSWORD`
- restart the server after changing env vars
