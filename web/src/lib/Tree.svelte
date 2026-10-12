<script lang="ts">
  import FileIcon from './FileIcon.svelte'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import FilePlus from '@lucide/svelte/icons/file-plus'
  import FolderPlus from '@lucide/svelte/icons/folder-plus'
  import { untrack } from 'svelte'
  import type { Entry } from './api'
  import Tree from './Tree.svelte'

  let {
    entries,
    active,
    onopen,
    onnew,
    onnewdir,
    oncontext,
    expandAll = false,
    depth = 0,
  }: {
    entries: Entry[]
    active: (string | null)[]
    onopen: (e: Entry) => void
    onnew: (dir: string) => void
    onnewdir: (parent: string) => void
    /** Clique direito num item (menu de renomear/mover/excluir). */
    oncontext: (e: Entry, ev: MouseEvent) => void
    expandAll?: boolean
    depth?: number
  } = $props()

  let open = $state<Record<string, boolean>>({})

  // Revela a nota/PDF aberto: expande as pastas do caminho só quando ele muda
  // (inclusive depois de renomear/mover). Recarregar a árvore a cada autosave não
  // reabre uma pasta que foi recolhida à mão.
  let revealed = ''
  $effect(() => {
    const paths = active.filter((a): a is string => !!a)
    const key = paths.join('\n')
    const list = entries // dependência: reavalia quando a árvore chega/atualiza
    untrack(() => {
      if (key === revealed || !list.length) return
      // Caminhos que passam por uma pasta deste nível (prefixo comum dos itens).
      const prefix = list[0].path.slice(0, list[0].path.lastIndexOf('/') + 1)
      const pending = paths.filter((a) => a.startsWith(prefix) && a.slice(prefix.length).includes('/'))
      const under = (e: Entry) => e.dir && pending.some((a) => a.startsWith(e.path + '/'))
      for (const e of list) if (under(e)) open[e.path] = true
      // Só dá por revelado quando a árvore já tem as pastas do caminho (ela pode chegar
      // depois, ex.: no carregamento inicial ou logo após renomear uma pasta).
      if (pending.every((a) => list.some((e) => e.dir && a.startsWith(e.path + '/')))) revealed = key
    })
  })
</script>

<ul>
  {#each entries as e (e.path)}
    <li>
      {#if e.dir}
        <div class="row">
          <button
            class="item dir"
            style:padding-left="{depth * 12 + 8}px"
            onclick={() => (open[e.path] = !open[e.path])}
            oncontextmenu={(ev) => {
              ev.preventDefault()
              oncontext(e, ev)
            }}
          >
            {#if open[e.path] || expandAll}<ChevronDown size={14} />{:else}<ChevronRight size={14} />{/if}<span class="label">{e.name}</span>
          </button>
          <button
            class="new"
            title="Nova nota em {e.path}"
            onclick={() => {
              open[e.path] = true
              onnew(e.path)
            }} aria-label="Nova nota"><FilePlus size={14} /></button
          >
          <button
            class="new"
            title="Nova subpasta em {e.path}"
            onclick={() => {
              open[e.path] = true
              onnewdir(e.path)
            }} aria-label="Nova subpasta"><FolderPlus size={14} /></button
          >
        </div>
        {#if open[e.path] || expandAll}
          <Tree entries={e.children ?? []} {active} {onopen} {onnew} {onnewdir} {oncontext} {expandAll} depth={depth + 1} />
        {/if}
      {:else}
        <button
          class="item"
          class:active={active.includes(e.path)}
          style:padding-left="{depth * 12 + 8}px"
          title={e.path}
          onclick={() => onopen(e)}
          oncontextmenu={(ev) => {
            ev.preventDefault()
            oncontext(e, ev)
          }}
        >
          <FileIcon name={e.name} /><span class="label">{e.name.replace(/\.md$/, '')}</span>
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
    display: flex;
    align-items: center;
    gap: 5px;
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
  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
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
