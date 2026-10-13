#!/usr/bin/env bash
# Runs the browser test (real frontend + system Chromium) against one backend binary.
#   e2e/run.sh NAME BINARY PORT
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source "$HERE/../lib.sh"
NAME=$1 BIN=$2 PORT=$3
W=$QB_ROOT/e2e-$NAME
qb_assert_tmp "$W"
rm -rf "$W"; mkdir -p "$W/vault" "$W/home"
python3 -I "$HERE/../contract/contract.py" seed "$W/vault"
qb_start_server "$NAME" "$BIN" "$PORT" "$W/vault" "$W/home" "$W/server.log"
trap 'qb_stop_server $QB_PID' EXIT
PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=${PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH:-/usr/bin/chromium} \
  node "$HERE/frontend.mjs" --url "http://127.0.0.1:$PORT" --vault "$W/vault" --shots "$W/shots"
