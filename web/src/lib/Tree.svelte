<script lang="ts">
  import type { Entry } from './api'
  import Tree from './Tree.svelte'

  let {
    entries,
    active,
    onopen,
    onnew,
    onnewdir,
    expandAll = false,
    depth = 0,
  }: {
    entries: Entry[]
    active: (string | null)[]
    onopen: (e: Entry) => void
    onnew: (dir: string) => void
    onnewdir: (parent: string) => void
    expandAll?: boolean
    depth?: number
  } = $props()

  let open = $state<Record<string, boolean>>({})
  const icon = (name: string) => (/\.pdf$/i.test(name) ? '📕' : /\.epub$/i.test(name) ? '📗' : '📝')
</script>

<ul>
  {#each entries as e (e.path)}
    <li>
      {#if e.dir}
        <div class="row">
          <button class="item dir" style:padding-left="{depth * 12 + 8}px" onclick={() => (open[e.path] = !open[e.path])}>
            {open[e.path] || expandAll ? '▾' : '▸'} {e.name}
          </button>
          <button
            class="new"
            title="Nova nota em {e.path}"
            onclick={() => {
              open[e.path] = true
              onnew(e.path)
            }}>+</button
          >
          <button
            class="new"
            title="Nova subpasta em {e.path}"
            onclick={() => {
              open[e.path] = true
              onnewdir(e.path)
            }}>📁+</button
          >
        </div>
        {#if open[e.path] || expandAll}
          <Tree entries={e.children ?? []} {active} {onopen} {onnew} {onnewdir} {expandAll} depth={depth + 1} />
        {/if}
      {:else}
        <button
          class="item"
          class:active={active.includes(e.path)}
          style:padding-left="{depth * 12 + 8}px"
          title={e.path}
          onclick={() => onopen(e)}
        >
          {icon(e.name)} {e.name.replace(/\.md$/, '')}
        </button>
      {/if}
    </li>
  {/each}
</ul>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .item {
    display: block;
    width: 100%;
    text-align: left;
    background: none;
    border: 0;
    color: inherit;
    padding: 3px 8px;
    font: inherit;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: pointer;
    border-radius: 4px;
  }
  .item:hover {
    background: var(--hover);
  }
  .dir {
    color: var(--muted);
  }
  .row {
    display: flex;
    align-items: center;
  }
  .row .item {
    flex: 1;
    min-width: 0;
  }
  .new {
    visibility: hidden;
    flex-shrink: 0;
    padding: 0 6px;
    margin-right: 2px;
    line-height: 1.4;
  }
  .row:hover .new,
  .new:focus-visible {
    visibility: visible;
  }
  /* Sem hover (celular/tablet): botão sempre visível. */
  @media (hover: none) {
    .new {
      visibility: visible;
    }
  }
  .active {
    background: var(--accent-bg);
    color: var(--accent);
  }
</style>
