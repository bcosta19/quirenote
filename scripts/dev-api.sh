#!/usr/bin/env bash
# Go server for `make dev` with auto-reload: rebuilds and restarts on every
# change to .go files (Vite already does this on its own for the frontend).
set -u
cd "$(dirname "$0")/.."

pid=""
watcher=""
crashes=0 # consecutive crashes right at startup (port in use, invalid vault…)
stop() {
  [ -n "$watcher" ] && kill "$watcher" 2>/dev/null
  [ -n "$pid" ] && kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null
  pid="" watcher=""
}
trap 'stop; exit 0' INT TERM HUP

while true; do
  if go build -o bin/quirenote-dev ./cmd/quirenote; then
    bin/quirenote-dev "$@" &
    pid=$!
    started=$SECONDS
  else
    echo "dev-api: build error; waiting for a fix…" >&2
  fi
  # Wait for the next Go code change (ignores the rest of the repository).
  # In the background + wait: this way a SIGTERM interrupts the wait right away.
  inotifywait -qq -r -e close_write,create,delete,move --include '\.go$' cmd internal web/embed.go &
  watcher=$!
  if [ -n "$pid" ] && [ "$crashes" -lt 3 ]; then
    # Wake up on whichever comes first: a code change or the server exiting on its own.
    wait -n "$watcher" "$pid"
    if kill -0 "$watcher" 2>/dev/null; then
      # Died in under 5 s: counts as a startup failure; after 3, stop retrying.
      if [ $((SECONDS - started)) -lt 5 ]; then crashes=$((crashes + 1)); else crashes=0; fi
      if [ "$crashes" -ge 3 ]; then
        echo "dev-api: the server crashed 3 times at startup; waiting for a code change…" >&2
        pid=""
        wait "$watcher"
      else
        echo "dev-api: the server exited; restarting in 1 s…" >&2
        kill "$watcher" 2>/dev/null
        watcher="" pid=""
        sleep 1
        continue
      fi
    fi
  else
    wait "$watcher"
  fi
  watcher=""
  crashes=0
  echo "dev-api: Go code changed, restarting…"
 >&2
  stop
done
