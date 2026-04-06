#!/usr/bin/env bash
set -euo pipefail

CONFIG_PATH="${1:-client-config.json}"

cargo run -p command-plane-client -- "${CONFIG_PATH}"
