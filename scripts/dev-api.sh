#!/usr/bin/env bash
# Servidor Go do `make dev` com recarga automática: recompila e reinicia a cada
# mudança em arquivos .go (o Vite já faz isso sozinho para o front).
set -u
cd "$(dirname "$0")/.."

pid=""
watcher=""
stop() {
  [ -n "$watcher" ] && kill "$watcher" 2>/dev/null
  [ -n "$pid" ] && kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null
  pid="" watcher=""
}
trap 'stop; exit 0' INT TERM HUP

while true; do
  if go build -o bin/notas-dev ./cmd/notas; then
    bin/notas-dev "$@" &
    pid=$!
  else
    echo "dev-api: erro de compilação; aguardando correção…" >&2
  fi
  # Espera a próxima mudança em código Go (ignora o resto do repositório).
  # Em segundo plano + wait: assim um SIGTERM interrompe a espera na hora.
  inotifywait -qq -r -e close_write,create,delete,move --include '\.go$' cmd internal web/embed.go &
  watcher=$!
  wait "$watcher"
  watcher=""
  echo "dev-api: código Go mudou, reiniciando…" >&2
  stop
done
