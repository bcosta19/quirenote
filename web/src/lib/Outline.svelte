<script lang="ts">
  import { onMount, tick } from 'svelte'
  import { score } from './fuzzy'
  import { headings, type Heading } from './markdown'

  // Sumário da nota (Ctrl+Shift+O): títulos com busca aproximada; Enter leva até o título.
  let { text, onjump, onclose }: { text: string; onjump: (line: number) => void; onclose: () => void } = $props()

  let query = $state('')
  let selected = $state(0)
  let input: HTMLInputElement
  let list: HTMLUListElement

  const all = $derived(headings(text))
  const items = $derived.by(() => {
    if (!query.trim()) return all
    return all
      .map((h) => ({ h, s: score(query, h.title) }))
      .filter((r) => r.s !== null)
      .sort((a, b) => b.s! - a.s!)
      .map((r) => r.h)
  })

  $effect(() => {
    query
    selected = 0
  })

  function choose(h: Heading | undefined) {
    if (!h) return
    onclose()
    onjump(h.line)
  }

  async function move(d: number) {
    if (!items.length) return
    selected = (selected + d + items.length) % items.length
    await tick()
    list?.querySelector('.selected')?.scrollIntoView({ block: 'nearest' })
  }

  function onkeydown(e: KeyboardEvent) {
    e.stopPropagation()
    const ctrl = e.ctrlKey || e.metaKey
    if (e.key === 'ArrowDown' || (ctrl && (e.key === 'j' || e.key === 'n'))) {
      e.preventDefault()
      move(1)
    } else if (e.key === 'ArrowUp' || (ctrl && (e.key === 'k' || e.key === 'p'))) {
      e.preventDefault()
      move(-1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      choose(items[selected])
    } else if (e.key === 'Escape' || (ctrl && e.shiftKey && e.key.toLowerCase() === 'o')) {
      e.preventDefault()
      onclose()
    }
  }

  onMount(() => input.focus())
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onclose}>
  <div class="outline" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Sumário da nota" tabindex="-1">
    <input bind:this={input} bind:value={query} {onkeydown} placeholder="Ir para o título…" />
    <ul bind:this={list}>
      {#each items as h, i (h.line)}
        <li>
          <button
            class:selected={i === selected}
            style:padding-left="{(query.trim() ? 0 : (h.level - 1) * 14) + 10}px"
            onmousemove={() => (selected = i)}
            onclick={() => choose(h)}
          >
            <span class="level">H{h.level}</span>
            {h.title}
          </button>
        </li>
      {:else}
        <li class="empty">{all.length ? 'Nenhum título com esse texto.' : 'Esta nota não tem títulos.'}</li>
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
  .outline {
    width: min(560px, 92vw);
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
    max-height: 55vh;
    overflow: auto;
  }
  li button {
    display: flex;
    gap: 8px;
    align-items: baseline;
    width: 100%;
    border: 0;
    background: none;
    padding: 5px 10px;
    border-radius: 6px;
    text-align: left;
    font-family: var(--editor-font);
    font-size: 16px;
  }
  li button.selected {
    background: var(--accent-bg);
    color: var(--accent);
  }
  .level {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--muted);
  }
  .empty {
    padding: 10px;
    color: var(--muted);
  }
</style>
