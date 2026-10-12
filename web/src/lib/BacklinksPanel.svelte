<script lang="ts">
  import CornerUpLeft from '@lucide/svelte/icons/corner-up-left'
  import { onMount, tick } from 'svelte'
  import type { SearchResult } from './api'

  // Notas que apontam para a nota aberta (backlinks), com o trecho de cada link.
  let {
    note,
    results,
    onopen,
    onclose,
  }: {
    note: string
    results: SearchResult[]
    onopen: (path: string, line: number) => void
    onclose: () => void
  } = $props()

  let selected = $state(0)
  let list: HTMLUListElement
  let panel: HTMLDivElement

  function choose(r: SearchResult | undefined) {
    if (!r) return
    onclose()
    onopen(r.path, r.line)
  }

  async function move(d: number) {
    if (!results.length) return
    selected = (selected + d + results.length) % results.length
    await tick()
    list?.querySelector('.selected')?.scrollIntoView({ block: 'nearest' })
  }

  function onkeydown(e: KeyboardEvent) {
    e.stopPropagation()
    const ctrl = e.ctrlKey || e.metaKey
    if (e.key === 'ArrowDown' || e.key === 'j' || (ctrl && e.key === 'n')) {
      e.preventDefault()
      move(1)
    } else if (e.key === 'ArrowUp' || e.key === 'k' || (ctrl && e.key === 'p')) {
      e.preventDefault()
      move(-1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      choose(results[selected])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      onclose()
    }
  }

  const name = (p: string) => p.split('/').pop()!.replace(/\.md$/i, '')
  const dir = (p: string) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '')

  onMount(() => panel.focus())
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onclose}>
  <div
    class="panel"
    bind:this={panel}
    onclick={(e) => e.stopPropagation()}
    {onkeydown}
    role="dialog"
    aria-label="Notas que apontam para esta"
    tabindex="-1"
  >
    <div class="head"><CornerUpLeft size={16} /> Notas que apontam para <b>{name(note)}</b> <span class="muted">({results.length})</span></div>
    <ul bind:this={list}>
      {#each results as r, i (r.path + ':' + r.line)}
        <li>
          <button class:selected={i === selected} onmousemove={() => (selected = i)} onclick={() => choose(r)}>
            <span class="where"
              ><b>{name(r.path)}</b> <span class="muted">{dir(r.path)}{dir(r.path) ? ' · ' : ''}linha {r.line}</span></span
            >
            <span class="snippet"
              >{#each r.parts as part, j (j)}{#if part.hit}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}</span
            >
          </button>
        </li>
      {:else}
        <li class="empty">Nenhuma nota aponta para esta ainda.</li>
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
    outline: none;
  }
  .head {
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
    font-size: 14px;
  }
  .muted {
    color: var(--muted);
    font-size: 12px;
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
  }
  li button.selected mark {
    background: var(--warn-bg);
  }
  .empty {
    padding: 10px;
    color: var(--muted);
  }
</style>
