#!/usr/bin/env bash
# Shared helpers for the contract and benchmark scripts.
#
# SAFETY: every server started here is confined to /tmp/qbench: an explicit -vault (or none, for
# the first-run test) and a throw-away HOME / XDG_CONFIG_HOME. The real vault and
# ~/.config/quirenote are never touched, and only ports 9100-9199 are used.

QB_ROOT=/tmp/qbench

qb_assert_tmp() {
  case "$1" in
    "$QB_ROOT"/*) ;;
    *) echo "refusing to use $1: outside $QB_ROOT" >&2; exit 1 ;;
  esac
}

# qb_start_server NAME BINARY PORT VAULT_OR_EMPTY HOME_DIR LOGFILE
# Sets QB_PID. Waits for the first HTTP answer and checks the startup log names the temp vault.
qb_start_server() {
  local name=$1 bin=$2 port=$3 vault=$4 home=$5 log=$6
  qb_assert_tmp "$home"; qb_assert_tmp "$log"
  [ -n "$vault" ] && qb_assert_tmp "$vault"
  if (( port < 9100 || port > 9199 )); then echo "port $port outside 9100-9199" >&2; exit 1; fi
  mkdir -p "$home/.config"
  local args=(-addr "127.0.0.1:$port")
  [ -n "$vault" ] && args+=(-vault "$vault")
  env -u QUIRENOTE_VAULT -u NOTAS_VAULT -u QUIRENOTE_ADDR -u NOTAS_ADDR \
    HOME="$home" XDG_CONFIG_HOME="$home/.config" \
    "$bin" "${args[@]}" >"$log" 2>&1 &
  QB_PID=$!
  local i
  for i in $(seq 1 100); do
    if curl -s -o /dev/null "http://127.0.0.1:$port/api/setup"; then break; fi
    sleep 0.05
  done
  if [ -n "$vault" ]; then
    if ! grep -q "vault=$vault" "$log"; then
      echo "startup log does not show the temporary vault $vault:" >&2; cat "$log" >&2
      kill "$QB_PID" 2>/dev/null; exit 1
    fi
  fi
  echo "[$name] pid $QB_PID port $port: $(grep -m1 -E 'vault=|no vault' "$log")"
}

qb_stop_server() {
  local pid=$1
  kill "$pid" 2>/dev/null
  local i
  for i in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.05; done
  kill -9 "$pid" 2>/dev/null
}
