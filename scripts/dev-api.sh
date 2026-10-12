#!/usr/bin/env bash
# Servidor Go do `make dev` com recarga automática: recompila e reinicia a cada
# mudança em arquivos .go (o Vite já faz isso sozinho para o front).
set -u
cd "$(dirname "$0")/.."

pid=""
watcher=""
crashes=0 # quedas seguidas logo ao subir (porta ocupada, vault inválido…)
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
    started=$SECONDS
  else
    echo "dev-api: erro de compilação; aguardando correção…" >&2
  fi
  # Espera a próxima mudança em código Go (ignora o resto do repositório).
  # Em segundo plano + wait: assim um SIGTERM interrompe a espera na hora.
  inotifywait -qq -r -e close_write,create,delete,move --include '\.go$' cmd internal web/embed.go &
  watcher=$!
  if [ -n "$pid" ] && [ "$crashes" -lt 3 ]; then
    # Acorda com o que vier primeiro: mudança no código ou o servidor saindo sozinho.
    wait -n "$watcher" "$pid"
    if kill -0 "$watcher" 2>/dev/null; then
      # Caiu em menos de 5 s: conta como falha ao subir; depois de 3, para de insistir.
      if [ $((SECONDS - started)) -lt 5 ]; then crashes=$((crashes + 1)); else crashes=0; fi
      if [ "$crashes" -ge 3 ]; then
        echo "dev-api: o servidor caiu 3 vezes ao subir; aguardando mudança no código…" >&2
        pid=""
        wait "$watcher"
      else
        echo "dev-api: o servidor saiu; reiniciando em 1 s…" >&2
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
  echo "dev-api: código Go mudou, reiniciando…" >&2
  stop
done
