#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-single}"

export RU_SERVER_DB_PATH="${RU_SERVER_DB_PATH:-./command-plane.db}"
export RU_SERVER_SHARED_TOKEN="${RU_SERVER_SHARED_TOKEN:-dev-shared-token}"
export RU_ADMIN_USERNAME="${RU_ADMIN_USERNAME:-admin}"
export RU_ADMIN_PASSWORD="${RU_ADMIN_PASSWORD:-admin123}"

if [[ "${MODE}" == "split" ]]; then
  export RU_SERVER_HTTP_BIND="${RU_SERVER_HTTP_BIND:-127.0.0.1:18080}"
  export RU_SERVER_WS_BIND="${RU_SERVER_WS_BIND:-127.0.0.1:18081}"
  export RU_SERVER_PUBLIC_WS_BASE="${RU_SERVER_PUBLIC_WS_BASE:-ws://127.0.0.1:18081/ws/agents}"
  echo "starting command-plane-server in split mode: http=${RU_SERVER_HTTP_BIND} ws=${RU_SERVER_WS_BIND}"
else
  export RU_SERVER_BIND="${RU_SERVER_BIND:-127.0.0.1:18080}"
  export RU_SERVER_HTTP_BIND="${RU_SERVER_HTTP_BIND:-${RU_SERVER_BIND}}"
  export RU_SERVER_WS_BIND="${RU_SERVER_WS_BIND:-${RU_SERVER_BIND}}"
  export RU_SERVER_PUBLIC_WS_BASE="${RU_SERVER_PUBLIC_WS_BASE:-ws://127.0.0.1:18080/ws/agents}"
  echo "starting command-plane-server in single mode: bind=${RU_SERVER_HTTP_BIND}"
fi

cargo run -p command-plane-server
