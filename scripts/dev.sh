#!/usr/bin/env bash
# Own only the backend started by this development session.
set -euo pipefail
cd "$(dirname "$0")/.."
addr=${QUIRENOTE_ADDR:-127.0.0.1:8080}
args=("$@")
for ((i=0; i<${#args[@]}; i++)); do
  if [[ "${args[i]}" == "-addr" ]]; then addr=${args[i+1]}; fi
done
url="http://$addr"
if curl --connect-timeout 1 --max-time 2 -fs -o /dev/null "$url/api/setup"; then
  echo "Quirenote: $addr is already serving an application; choose another ADDR" >&2
  exit 1
fi
bin/quirenote-dev "$@" &
server_pid=$!
stop() { kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; }
trap stop EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
for _ in {1..50}; do
  kill -0 "$server_pid" 2>/dev/null || exit 1
  if curl --connect-timeout 1 --max-time 2 -fs -o /dev/null "$url/api/setup"; then
    exec_ready=1
    break
  fi
  sleep 0.2
done
[[ "${exec_ready:-0}" == 1 ]] || { echo "Quirenote: backend startup timed out" >&2; exit 1; }
bin/quirenote-gui-dev --server "$url"
