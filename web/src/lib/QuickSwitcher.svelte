<script lang="ts">
  import FileIcon from './FileIcon.svelte'
  import { onMount, tick } from 'svelte'
  import { score } from './fuzzy'

  let {
    files,
    recent,
    onopen,
    oncreate,
    onclose,
  }: {
    files: string[] // caminhos de notas e livros do vault
    recent: string[] // abertos recentemente (mais recente primeiro)
    onopen: (path: string) => void
    oncreate: (path: string) => void
    onclose: () => void
  } = $props()

  type Item = { kind: 'open'; path: string } | { kind: 'create'; path: string }

  let query = $state('')
  let selected = $state(0)
  let input: HTMLInputElement
  let list: HTMLUListElement

  const MAX = 50

  const items = $derived.by((): Item[] => {
    const q = query.trim()
    if (!q) {
      const known = new Set(files)
      const rec = recent.filter((p) => known.has(p))
      const rest = files.filter((p) => !rec.includes(p))
      return [...rec, ...rest].slice(0, MAX).map((path) => ({ kind: 'open', path }))
    }
    const found = files
      .map((path) => ({ path, s: score(q, path) }))
      .filter((r): r is { path: string; s: number } => r.s !== null)
      .sort((a, b) => b.s - a.s)
      .slice(0, MAX)
      .map(({ path }): Item => ({ kind: 'open', path }))
    // Sem nota com esse nome exato: oferece criar (como no Obsidian).
    const target = /\.md$/i.test(q) ? q : `${q}.md`
    if (!files.some((f) => f.toLowerCase() === target.toLowerCase())) found.push({ kind: 'create', path: target })
    return found
  })

  $effect(() => {
    query
    selected = 0
  })

  function choose(item: Item | undefined) {
    if (!item) return
    if (item.kind === 'open') onopen(item.path)
    else oncreate(item.path)
    onclose()
  }

  async function move(delta: number) {
    if (!items.length) return
    selected = (selected + delta + items.length) % items.length
    await tick()
    list?.querySelector('.selected')?.scrollIntoView({ block: 'nearest' })
  }

  function onkeydown(e: KeyboardEvent) {
    const ctrl = e.ctrlKey || e.metaKey
    if (e.key === 'ArrowDown' || (ctrl && (e.key === 'n' || e.key === 'j'))) {
      e.preventDefault()
      move(1)
    } else if (e.key === 'ArrowUp' || (ctrl && (e.key === 'p' || e.key === 'k'))) {
      e.preventDefault()
      move(-1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      // Shift+Enter cria uma nota com o texto digitado mesmo que haja resultados.
      // Se a nota já existe, não há item "Criar": abre a existente.
      if (e.shiftKey && query.trim()) choose(items.find((i) => i.kind === 'create') ?? items[0])
      else choose(items[selected])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      onclose()
    }
  }

  const name = (p: string) => p.split('/').pop()!.replace(/\.md$/i, '')
  const dir = (p: string) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '')

  onMount(() => input.focus())
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onclose}>
  <div class="switcher" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Abrir nota" tabindex="-1">
    <input
      bind:this={input}
      bind:value={query}
      {onkeydown}
      placeholder="Buscar ou criar nota…  (↑↓ navega · Enter abre · Shift+Enter cria · Esc fecha)"
    />
    <ul bind:this={list}>
      {#each items as item, i (item.kind + item.path)}
        <li>
          <button class:selected={i === selected} onmousemove={() => (selected = i)} onclick={() => choose(item)}>
            {#if item.kind === 'create'}
              <span class="name">＋ Criar nota “{name(item.path)}”</span>
              <span class="dir">{dir(item.path) || 'raiz do vault'}</span>
            {:else}
              <span class="name"><FileIcon name={item.path} /> {name(item.path)}</span>
              <span class="dir">{dir(item.path)}</span>
            {/if}
          </button>
        </li>
      {:else}
        <li class="empty">Nenhuma nota no vault.</li>
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
    padding-top: 12vh;
  }
  .switcher {
    width: min(640px, 92vw);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.25);
    overflow: hidden;
  }
  input {
    width: 100%;
    border: 0;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    padding: 12px 14px;
    font-size: 15px;
    background: transparent;
    outline: none;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 6px;
    max-height: 50vh;
    overflow: auto;
  }
  li button {
    display: flex;
    width: 100%;
    justify-content: space-between;
    gap: 12px;
    border: 0;
    background: none;
    padding: 6px 10px;
    border-radius: 6px;
    text-align: left;
    font-size: 14px;
  }
  li button.selected {
    background: var(--accent-bg);
    color: var(--accent);
  }
  .name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dir {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 1;
  }
  .empty {
    padding: 10px;
    color: var(--muted);
  }
</style>
