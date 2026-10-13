#!/usr/bin/env bash
# Runs the contract suite against ONE server binary and leaves the recording in
# /tmp/qbench/contract-NAME/{run,setup}.json
#   contract/run.sh NAME BINARY PORT
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
source "$HERE/../lib.sh"
NAME=$1 BIN=$2 PORT=$3
W=$QB_ROOT/contract-$NAME
qb_assert_tmp "$W"
rm -rf "$W"; mkdir -p "$W/vault" "$W/home"
python3 -I "$HERE/contract.py" seed "$W/vault"

echo "== $NAME: main suite"
qb_start_server "$NAME" "$BIN" "$PORT" "$W/vault" "$W/home" "$W/server.log"
trap 'qb_stop_server $QB_PID' EXIT
python3 -I "$HERE/contract.py" run --url "http://127.0.0.1:$PORT" --vault "$W/vault" --out "$W/run.json" ${ONLY:+--only "$ONLY"} || true
qb_stop_server "$QB_PID"; trap - EXIT

echo "== $NAME: first-run setup suite"
mkdir -p "$W/home2/.config"
qb_start_server "$NAME-setup" "$BIN" "$PORT" "" "$W/home2" "$W/server-setup.log"
trap 'qb_stop_server $QB_PID' EXIT
python3 -I "$HERE/contract.py" setup --url "http://127.0.0.1:$PORT" --home "$W/home2" --config-dir "$W/home2/.config" --out "$W/setup.json" || true
qb_stop_server "$QB_PID"; trap - EXIT
