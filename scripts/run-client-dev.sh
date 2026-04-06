#!/usr/bin/env bash
set -euo pipefail

CONFIG_PATH="${1:-client-config.json}"

cargo run -p ru-command-client -- "${CONFIG_PATH}"
