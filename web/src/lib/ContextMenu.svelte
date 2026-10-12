<script lang="ts">
  import { onMount } from 'svelte'

  export interface MenuItem {
    label: string
    action: () => void
    danger?: boolean
  }

  // Context menu (right click) with the app look.
  let { x, y, items, onclose }: { x: number; y: number; items: MenuItem[]; onclose: () => void } = $props()

  let menu: HTMLDivElement
  let left = $state(0)
  let top = $state(0)

  onMount(() => {
    // Keeps the menu inside the window.
    const r = menu.getBoundingClientRect()
    left = Math.min(x, window.innerWidth - r.width - 4)
    top = Math.min(y, window.innerHeight - r.height - 4)
    menu.querySelector('button')?.focus()
    const close = (e: Event) => {
      if (!menu.contains(e.target as Node)) onclose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation() // Esc only closes the menu; it does not reach vim or the app shortcuts
      onclose()
    }
    // capture: closes before another click acts; contextmenu on another item reopens in the right place.
    window.addEventListener('pointerdown', close, true)
    window.addEventListener('keydown', onKey, true)
    window.addEventListener('blur', onclose)
    return () => {
      window.removeEventListener('pointerdown', close, true)
      window.removeEventListener('keydown', onKey, true)
      window.removeEventListener('blur', onclose)
    }
  })

  function run(item: MenuItem) {
    onclose()
    item.action()
  }
</script>

<div class="menu" role="menu" bind:this={menu} style:left="{left || x}px" style:top="{top || y}px">
  {#each items as item (item.label)}
    <button role="menuitem" class:danger={item.danger} onclick={() => run(item)}>{item.label}</button>
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 80;
    min-width: 200px;
    padding: 4px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.2);
    display: flex;
    flex-direction: column;
  }
  button {
    text-align: left;
    border: 0;
    background: none;
    padding: 6px 10px;
    border-radius: 5px;
  }
  button:hover,
  button:focus-visible {
    background: var(--accent-bg);
    outline: none;
  }
  .danger {
    color: var(--error);
  }
</style>
