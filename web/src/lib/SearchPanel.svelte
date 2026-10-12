<script lang="ts">
  import { onMount, tick } from 'svelte'
  import { searchNotes, type SearchResult } from './api'

  // Busca no conteúdo de todas as notas (ripgrep no servidor), sem diferenciar acentos.
  let {
    initial = '',
    onopen,
    onclose,
  }: {
    initial?: string
    onopen: (path: string, line: number) => void
    /** Recebe o termo buscado, para reabrir com ele depois. */
    onclose: (query: string) => void
  } = $props()

  // svelte-ignore state_referenced_locally
  let query = $state(initial) // termo inicial (ex.: busca anterior); depois o campo é livre
  let results = $state<SearchResult[]>([])
  let selected = $state(0)
  let status = $state('')
  let input: HTMLInputElement
  let list: HTMLUListElement
  let timer: ReturnType<typeof setTimeout> | undefined
  let inflight: AbortController | undefined
  const LIMIT = 200 // igual a search.MaxTotal no servidor

  async function run(q: string) {
    inflight?.abort()
    if (!q.trim()) {
      results = []
      status = ''
      return
    }
    inflight = new AbortController()
    status = 'buscando…'
    try {
      // O servidor manda um resultado além do limite só para indicar que houve corte.
      const found = await searchNotes(q, inflight.signal)
      const cut = found.length > LIMIT
      results = found.slice(0, LIMIT)
      selected = 0
      status = results.length ? `${results.length}${cut ? '+' : ''} trecho(s)` : 'nada encontrado'
    } catch (e) {
      if ((e as Error).name !== 'AbortError') status = (e as Error).message
    }
  }

  // Busca enquanto digita, esperando uma pausa curta.
  $effect(() => {
    const q = query
    clearTimeout(timer)
    timer = setTimeout(() => run(q), 200)
    return () => clearTimeout(timer)
  })

  /** Fecha guardando o termo (usado também pelo Ctrl+Shift+F do app). */
  export function close() {
    onclose(query)
  }

  function choose(r: SearchResult | undefined) {
    if (!r) return
    onopen(r.path, r.line)
    onclose(query)
  }

  async function move(delta: number) {
    if (!results.length) return
    selected = (selected + delta + results.length) % results.length
    await tick()
    list?.querySelector('.selected')?.scrollIntoView({ block: 'nearest' })
  }

  function onkeydown(e: KeyboardEvent) {
    e.stopPropagation() // atalhos do app não agem enquanto digita aqui
    const ctrl = e.ctrlKey || e.metaKey
    if (e.key === 'ArrowDown' || (ctrl && (e.key === 'j' || e.key === 'n'))) {
      e.preventDefault()
      move(1)
    } else if (e.key === 'ArrowUp' || (ctrl && (e.key === 'k' || e.key === 'p'))) {
      e.preventDefault()
      move(-1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      choose(results[selected])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      onclose(query)
    }
  }

  const name = (p: string) => p.split('/').pop()!.replace(/\.md$/i, '')
  const dir = (p: string) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '')

  onMount(() => {
    input.focus()
    input.select()
    return () => inflight?.abort()
  })
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={() => onclose(query)}>
  <div class="panel" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Buscar nas notas" tabindex="-1">
    <div class="head">
      <input bind:this={input} bind:value={query} {onkeydown} placeholder="Buscar no conteúdo das notas…" />
      <span class="status">{status}</span>
    </div>
    <ul bind:this={list}>
      {#each results as r, i (r.path + ':' + r.line)}
        <li>
          <button class:selected={i === selected} onmousemove={() => (selected = i)} onclick={() => choose(r)}>
            <span class="where"
              ><b>{name(r.path)}</b> <span class="dir">{dir(r.path)}{dir(r.path) ? ' · ' : ''}linha {r.line}</span></span
            >
            <span class="snippet"
              >{#each r.parts as part, j (j)}{#if part.hit}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}</span
            >
          </button>
        </li>
      {/each}
    </ul>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: rgba(0, 0, 0, 0.25);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 10vh;
  }
  .panel {
    width: min(760px, 94vw);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.25);
    overflow: hidden;
  }
  .head {
    display: flex;
    align-items: center;
    border-bottom: 1px solid var(--border);
  }
  input {
    flex: 1;
    border: 0;
    border-radius: 0;
    padding: 12px 14px;
    font-size: 15px;
    background: transparent;
    outline: none;
  }
  .status {
    padding: 0 14px;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 6px;
    max-height: 60vh;
    overflow: auto;
  }
  li button {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    border: 0;
    background: none;
    padding: 6px 10px;
    border-radius: 6px;
    text-align: left;
  }
  li button.selected {
    background: var(--accent-bg);
  }
  .where {
    font-size: 13px;
  }
  .dir {
    color: var(--muted);
    font-size: 12px;
  }
  .snippet {
    font-family: var(--editor-font);
    font-size: 15px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  mark {
    background: var(--accent-bg);
    color: var(--text);
    border-radius: 2px;
    padding: 0 1px;
  }
  li button.selected mark {
    background: var(--warn-bg);
  }
</style>
