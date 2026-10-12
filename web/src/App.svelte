<script lang="ts">
  import { onMount } from 'svelte'
  import { ExistsError, createDir, createNote, fileUrl, getTree, subscribe, type Entry } from './lib/api'
  import { pageLink, resolveLink } from './lib/links'
  import Editor from './lib/Editor.svelte'
  import PdfReader from './lib/PdfReader.svelte'
  import Preview from './lib/Preview.svelte'
  import QuickSwitcher from './lib/QuickSwitcher.svelte'
  import PromptDialog from './lib/PromptDialog.svelte'
  import { IMAGE } from './lib/markdown'
  import SearchPanel from './lib/SearchPanel.svelte'
  import Tree from './lib/Tree.svelte'
  import type { Theme } from './lib/theme'

  // Estado da tela, refletido na URL (#note=...&doc=...&page=...) para sobreviver a reloads.
  const initial = new URLSearchParams(location.hash.slice(1))
  let note = $state<string | null>(initial.get('note'))
  let doc = $state<string | null>(initial.get('doc'))
  let page = $state(Number(initial.get('page') || 1))

  let tree = $state<Entry[]>([])
  let filter = $state('')
  let sidebarOpen = $state(localStorage.getItem('notas:sidebar') !== '0')
  let notesOpen = $state(localStorage.getItem('notas:notes') !== '0')
  // Leitor recolhido: o PDF continua carregado (mesma página), só fica escondido.
  let readerOpen = $state(true)
  // Preview do markdown (modo leitura, como o Ctrl+E do Obsidian).
  let preview = $state(false)
  let noteText = $state('')

  // Tamanhos ajustáveis com o mouse (salvos por dispositivo).
  const SIDEBAR_W = 260
  const NOTES_PCT = 42
  let sidebarW = $state(Number(localStorage.getItem('notas:sidebarW')) || SIDEBAR_W)
  let notesPct = $state(Number(localStorage.getItem('notas:notesPct')) || NOTES_PCT)
  let dragging = $state<null | 'sidebar' | 'split'>(null)
  let panesEl: HTMLDivElement
  let theme = $state<Theme>(localStorage.getItem('notas:theme') === 'dark' ? 'dark' : 'light')
  let toast = $state('')
  let editor: ReturnType<typeof Editor> | undefined = $state()
  let fileInput: HTMLInputElement
  let pdf: ReturnType<typeof PdfReader> | undefined = $state()

  // Controlar o PDF sem tirar o foco do editor.
  // Sem PDF aberto, devolvem false e o editor segue com o comportamento padrão.
  function togglePreview() {
    if (!note) return
    preview = !preview
    if (!preview) setTimeout(() => editor?.focus())
  }

  /** URL de um arquivo citado na nota: [[nome]] resolvido no vault, ou caminho relativo à nota. */
  function previewFileUrl(target: string): string {
    const found = resolveLink(tree, target)
    if (found) return fileUrl(found)
    const dir = note?.includes('/') ? note.slice(0, note.lastIndexOf('/') + 1) : ''
    return fileUrl(target.includes('/') ? target : dir + target)
  }

  const pdfKeys = [
    { key: 'Alt-ArrowDown', run: () => (pdf && readerOpen ? (pdf.scroll(1), true) : false) },
    { key: 'Alt-ArrowUp', run: () => (pdf && readerOpen ? (pdf.scroll(-1), true) : false) },
    { key: 'Alt-Shift-ArrowDown', run: () => (pdf && readerOpen ? (pdf.turnPage(1), true) : false) },
    { key: 'Alt-Shift-ArrowUp', run: () => (pdf && readerOpen ? (pdf.turnPage(-1), true) : false) },
  ]

  // PDF escolhido do dispositivo: lido direto pelo navegador, sem upload.
  // Não vai para a URL porque o blob: some ao recarregar.
  let localPdf = $state<{ name: string; size: number; url: string } | null>(null)

  // O que o leitor mostra: um PDF do vault ou um PDF local.
  const reader = $derived(
    doc
      ? { src: fileUrl(doc), id: doc, title: doc, link: doc }
      : localPdf
        ? { src: localPdf.url, id: `local:${localPdf.name}:${localPdf.size}`, title: localPdf.name, link: localPdf.name }
        : null,
  )

  $effect(() => {
    const params = new URLSearchParams()
    if (note) params.set('note', note)
    if (doc) {
      params.set('doc', doc)
      params.set('page', String(page))
    }
    // Trocar de nota/PDF cria uma entrada no histórico (Voltar/Avançar do navegador);
    // mudar só a página do PDF apenas atualiza a entrada atual.
    const current = new URLSearchParams(location.hash.slice(1))
    const same = current.get('note') === params.get('note') && current.get('doc') === params.get('doc')
    if (same) history.replaceState(null, '', `#${params}`)
    else history.pushState(null, '', `#${params}`)
  })

  // Voltar/Avançar do navegador (ou Alt+←/→) restaura nota, PDF e página.
  function onPopState() {
    reveal = null
    const params = new URLSearchParams(location.hash.slice(1))
    note = params.get('note')
    const nextDoc = params.get('doc')
    if (nextDoc) {
      closeLocal()
      page = Number(params.get('page') || 1)
      readerOpen = true
    }
    doc = nextDoc
  }

  // Abertos recentemente, para a troca rápida (Ctrl+O / Ctrl+P).
  let recent = $state<string[]>(loadRecent())
  function loadRecent(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem('notas:recent') || '[]')
      return Array.isArray(v) ? v.filter((p) => typeof p === 'string') : []
    } catch {
      return [] // valor corrompido não pode impedir o app de abrir
    }
  }
  let switcherOpen = $state(false)
  // Busca no conteúdo (Ctrl+Shift+F); guarda o último termo para reabrir com ele.
  let searchOpen = $state(false)
  let searchPanel: ReturnType<typeof SearchPanel> | undefined = $state()
  let lastSearch = $state('')
  let reveal = $state<{ path: string; line: number; n: number } | null>(null)
  function openAtLine(path: string, line: number) {
    preview = false
    const n = (reveal?.n ?? 0) + 1
    openPath(path) // limpa pedidos antigos; o novo vem logo abaixo
    reveal = { path, line, n }
  }

  // Diálogo de texto com o visual do app (no lugar do prompt() do navegador).
  let dialog = $state<{ message: string; value: string; hint: string; resolve: (v: string | null) => void } | null>(null)
  function ask(message: string, value = '', hint = ''): Promise<string | null> {
    return new Promise((resolve) => (dialog = { message, value, hint, resolve }))
  }
  function closeDialog(result: string | null) {
    dialog?.resolve(result)
    dialog = null
    setTimeout(() => editor?.focus())
  }
  function remember(path: string) {
    recent = [path, ...recent.filter((p) => p !== path)].slice(0, 30)
    localStorage.setItem('notas:recent', JSON.stringify(recent))
  }
  $effect(() => localStorage.setItem('notas:sidebar', sidebarOpen ? '1' : '0'))
  $effect(() => localStorage.setItem('notas:notes', notesOpen ? '1' : '0'))

  // Nunca deixa os dois painéis recolhidos ao mesmo tempo.
  function toggleNotes() {
    notesOpen = !notesOpen
    if (!notesOpen && reader) readerOpen = true
  }
  function toggleReader() {
    readerOpen = !readerOpen
    if (!readerOpen) notesOpen = true
  }

  const clamp = (n: number, min: number, max: number) => Math.min(Math.max(n, min), max)

  function startDrag(kind: 'sidebar' | 'split', e: PointerEvent) {
    e.preventDefault()
    dragging = kind
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  }
  function onDrag(e: PointerEvent) {
    if (dragging === 'sidebar') {
      sidebarW = clamp(e.clientX, 160, 520)
    } else if (dragging === 'split' && panesEl) {
      const r = panesEl.getBoundingClientRect()
      notesPct = clamp(((r.right - e.clientX) / r.width) * 100, 20, 80)
    }
  }
  function endDrag() {
    if (!dragging) return
    localStorage.setItem('notas:sidebarW', String(Math.round(sidebarW)))
    localStorage.setItem('notas:notesPct', String(Math.round(notesPct)))
    dragging = null
  }
  function resetSize(kind: 'sidebar' | 'split') {
    if (kind === 'sidebar') sidebarW = SIDEBAR_W
    else notesPct = NOTES_PCT
    dragging = kind
    endDrag()
  }
  $effect(() => {
    document.documentElement.dataset.theme = theme
    localStorage.setItem('notas:theme', theme)
  })

  function filterTree(entries: Entry[], q: string): Entry[] {
    if (!q) return entries
    const out: Entry[] = []
    for (const e of entries) {
      if (e.dir) {
        const children = filterTree(e.children ?? [], q)
        if (children.length) out.push({ ...e, children })
      } else if (e.path.toLowerCase().includes(q)) {
        out.push(e)
      }
    }
    return out
  }
  // A árvore do servidor inclui imagens (para o preview resolver ![[foto.png]]);
  // a barra lateral mostra só notas e livros.
  function onlyDocs(entries: Entry[]): Entry[] {
    return entries
      .filter((e) => e.dir || /\.(md|pdf|epub)$/i.test(e.name))
      .map((e) => (e.dir ? { ...e, children: onlyDocs(e.children ?? []) } : e))
  }
  const visibleTree = $derived(filterTree(onlyDocs(tree), filter.trim().toLowerCase()))

  let toastTimer: ReturnType<typeof setTimeout> | undefined
  function flash(msg: string) {
    toast = msg
    clearTimeout(toastTimer) // um aviso novo não some antes da hora por causa do anterior
    toastTimer = setTimeout(() => (toast = ''), 3000)
  }

  function openPath(path: string, targetPage?: number) {
    // Abrir por qualquer outro caminho cancela o posicionamento pedido pela busca.
    reveal = null
    if (/\.(md|pdf)$/i.test(path)) remember(path)
    if (/\.md$/i.test(path)) {
      note = path
      notesOpen = true
    } else if (/\.pdf$/i.test(path)) {
      // Página do link; sem ela, o leitor volta para a última página lida.
      page = targetPage ?? (path === doc ? page : 1)
      closeLocal()
      doc = path
      readerOpen = true
    } else {
      flash('EPUB ainda não suportado (próximo passo do MVP)')
    }
  }

  function openLocal(e: Event) {
    const input = e.target as HTMLInputElement
    const file = input.files?.[0]
    input.value = '' // permite escolher o mesmo arquivo de novo
    if (!file) return
    closeLocal()
    localPdf = { name: file.name, size: file.size, url: URL.createObjectURL(file) }
    doc = null
    page = 1
    readerOpen = true
  }

  function closeLocal() {
    if (localPdf) URL.revokeObjectURL(localPdf.url)
    localPdf = null
  }

  function closeReader() {
    closeLocal()
    doc = null
  }

  async function openLink(target: string, targetPage?: number) {
    const path = resolveLink(tree, target)
    if (path) return openPath(path, targetPage)
    // Link para o PDF local aberto (gravado só com o nome do arquivo).
    const name = target.split('/').pop()!
    if (localPdf && name === localPdf.name) {
      if (targetPage) page = targetPage
      return
    }
    // Link para nota que ainda não existe: oferece criar (como no Obsidian),
    // na pasta da nota atual quando o link não tem caminho.
    // PDFs, EPUBs e imagens não viram nota; nomes com ponto (ex.: [[2024.10.12]]) sim.
    if (!/\.(pdf|epub)$/i.test(name) && !IMAGE.test(name)) {
      const dir = !target.includes('/') && note?.includes('/') ? note.slice(0, note.lastIndexOf('/') + 1) : ''
      const suggestion = (dir + target).replace(/\.md$/i, '') + '.md'
      const chosen = await ask(`Criar a nota “${name.replace(/\.md$/i, '')}”?`, suggestion, 'O link ainda não aponta para nenhuma nota. Ajuste o caminho se quiser.')
      if (chosen?.trim()) await createAndOpen(chosen)
      return
    }
    flash(`Não encontrado: ${target}${/\.pdf$/i.test(target) ? ' (se for um PDF local, abra-o com 📂)' : ''}`)
  }

  function allPaths(entries: Entry[]): Set<string> {
    const s = new Set<string>()
    const walk = (es: Entry[]) => es.forEach((e) => (e.dir ? walk(e.children ?? []) : s.add(e.path)))
    walk(entries)
    return s
  }

  /** dir: subpasta escolhida na árvore; sem ela, pede o caminho completo. */
  async function newNote(dir?: string) {
    // Com um PDF aberto, sugere uma nota de leitura ligada a ele.
    const base = reader ? reader.link.split('/').pop()!.replace(/\.pdf$/i, '') : 'Nova nota'
    let path: string | undefined
    if (dir) {
      const name = (await ask(`Nova nota em "${dir}":`, base))?.trim().replace(/^\/+/, '')
      if (!name) return
      path = `${dir}/${name}`
    } else {
      const suggestion = reader ? `Book notes/${base}.md` : `Random notes/${base}.md`
      path = (await ask('Nova nota:', suggestion, 'Caminho relativo ao vault; pastas que não existem são criadas.'))?.trim()
    }
    if (path) await createAndOpen(path)
  }

  /**
   * Cria a nota (com título e, se houver PDF aberto, o frontmatter source) e a abre.
   * Se já existir, só abre: o servidor nunca sobrescreve uma nota existente.
   */
  async function createAndOpen(path: string) {
    path = path.trim().replace(/^\/+/, '')
    if (!path.endsWith('.md')) path += '.md'
    if (invalidPath(path)) return flash('Nome inválido: não use "..", "." no início ou barras vazias')
    if (allPaths(tree).has(path)) return openPath(path)
    const title = path.split('/').pop()!.replace(/\.md$/, '')
    const content = reader ? `---\nsource: "[[${reader.link}]]"\n---\n\n# ${title}\n\n` : `# ${title}\n\n`
    try {
      await createNote(path, content)
    } catch (e) {
      // Já existe (Neovim/Obsidian criou antes de a árvore atualizar): abre a existente.
      if (!(e instanceof ExistsError)) return flash((e as Error).message)
    }
    try {
      tree = await getTree()
    } catch (e) {
      flash((e as Error).message)
    }
    openPath(path)
  }

  /** Recusa ., .. e nomes ocultos (o servidor também recusa; aqui a mensagem fica clara). */
  function invalidPath(path: string): boolean {
    return path.split('/').some((part) => part === '' || part.startsWith('.'))
  }

  /** parent: pasta escolhida na árvore; sem ela, cria a partir da raiz do vault. */
  async function newFolder(parent?: string) {
    const name = (await ask(parent ? `Nova subpasta em "${parent}":` : 'Nova pasta:', '', parent ? '' : 'Caminho relativo ao vault.'))
      ?.trim()
      .replace(/^\/+|\/+$/g, '')
    if (!name) return
    if (invalidPath(name)) return flash('Nome inválido: não use "..", "." no início ou barras vazias')
    try {
      const { path } = await createDir(parent ? `${parent}/${name}` : name)
      tree = await getTree()
      flash(`Pasta criada: ${path}`)
    } catch (e) {
      flash((e as Error).message)
    }
  }

  function insertPageLink() {
    if (reader && editor) editor.insert(pageLink(reader.link, page) + ' ')
  }

  // Atalhos na fase de captura: chegam antes do editor, cujo modo vim usa Ctrl+E (rolar)
  // e Ctrl+O (voltar na lista de saltos).
  function onKeyCapture(e: KeyboardEvent) {
    if (dialog) return
    if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === 'f') {
      e.preventDefault()
      e.stopPropagation()
      switcherOpen = false
      // Fechar pelo painel guarda o termo e devolve o foco ao editor.
      if (searchOpen) searchPanel?.close()
      else searchOpen = true
      return
    }
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && /^[op]$/i.test(e.key)) {
      // Com a busca aberta, Ctrl+P sobe na lista (tratado nela) e só Ctrl+O fecha.
      if (switcherOpen && e.key.toLowerCase() === 'p') return
      e.preventDefault()
      e.stopPropagation()
      switcherOpen = !switcherOpen
      return
    }
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'e' && note) {
      e.preventDefault()
      e.stopPropagation()
      togglePreview()
    }
  }

  function onKey(e: KeyboardEvent) {
    // Fora do editor (que já trata e marca defaultPrevented), os mesmos atalhos do PDF.
    if (!e.defaultPrevented && e.altKey && !e.ctrlKey && !e.metaKey && pdf && readerOpen) {
      const dir = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0
      if (dir) {
        e.preventDefault()
        if (e.shiftKey) pdf.turnPage(dir)
        else pdf.scroll(dir)
        return
      }
    }
    const mod = e.ctrlKey || e.metaKey
    if (e.altKey && !mod && e.key === '\\' && reader) {
      e.preventDefault()
      toggleReader()
      return
    }
    if (mod && e.key === '\\') {
      e.preventDefault()
      toggleNotes()
    } else if (mod && e.key === '|') {
      e.preventDefault()
      sidebarOpen = !sidebarOpen
    } else if (mod && e.shiftKey && e.key.toLowerCase() === 'l') {
      e.preventDefault()
      insertPageLink()
    }
  }

  onMount(() => {
    getTree().then((t) => (tree = t))
    let refresh: ReturnType<typeof setTimeout> | undefined
    return subscribe((ev) => {
      if (ev.path === note) editor?.externalChange()
      if (ev.op !== 'write') {
        clearTimeout(refresh)
        refresh = setTimeout(async () => (tree = await getTree()), 300)
      }
    })
  })
</script>

<svelte:window onkeydown={onKey} onkeydowncapture={onKeyCapture} onpopstate={onPopState} />

{#if dialog}
  <PromptDialog
    message={dialog.message}
    value={dialog.value}
    hint={dialog.hint}
    onsubmit={(v) => closeDialog(v)}
    oncancel={() => closeDialog(null)}
  />
{/if}

{#if searchOpen}
  <SearchPanel
    bind:this={searchPanel}
    initial={lastSearch}
    onopen={(p, line) => openAtLine(p, line)}
    onclose={(q) => {
      lastSearch = q
      searchOpen = false
      setTimeout(() => editor?.focus())
    }}
  />
{/if}

{#if switcherOpen}
  <QuickSwitcher
    files={[...allPaths(tree)].filter((p) => /\.(md|pdf|epub)$/i.test(p))}
    {recent}
    onopen={(p) => openPath(p)}
    oncreate={(p) => createAndOpen(p)}
    onclose={() => {
      switcherOpen = false
      setTimeout(() => editor?.focus())
    }}
  />
{/if}

<input type="file" accept="application/pdf,.pdf" hidden bind:this={fileInput} onchange={openLocal} />

<div class="app" class:dragging style:--sidebar-w="{sidebarW}px" style:--notes-w="{notesPct}%">
  {#if sidebarOpen}
    <aside class="sidebar">
      <div class="sidebar-head">
        <input placeholder="Filtrar…" title="Filtra a árvore pelo nome · Ctrl+O busca rápida · Ctrl+Shift+F busca no conteúdo" bind:value={filter} />
        <button onclick={() => newNote()} title="Nova nota">+</button>
        <button onclick={() => newFolder()} title="Nova pasta">📁+</button>
        <button onclick={() => fileInput.click()} title="Abrir PDF deste dispositivo">📂</button>
      </div>
      <div class="tree">
        <Tree entries={visibleTree} active={[note, doc]} onopen={(e) => openPath(e.path)} onnew={newNote} onnewdir={newFolder} expandAll={!!filter} />
      </div>
    </aside>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="resizer"
      title="Arraste para redimensionar · duplo clique restaura"
      onpointerdown={(e) => startDrag('sidebar', e)}
      onpointermove={onDrag}
      onpointerup={endDrag}
      onpointercancel={endDrag}
      ondblclick={() => resetSize('sidebar')}
    ></div>
  {/if}

  <main class="workspace">
    <div class="topbar">
      <button onclick={() => (sidebarOpen = !sidebarOpen)} title="Arquivos (Ctrl+Shift+\)">☰</button>
      <button onclick={() => (searchOpen = true)} title="Buscar no conteúdo das notas (Ctrl+Shift+F)">🔍</button>
      <span class="crumb">{note ?? ''}</span>
      {#if note}
        <button onclick={togglePreview} title="Alternar preview / edição (Ctrl+E)" class:active={preview}>
          {preview ? '✎ Editar' : '👁 Preview'}
        </button>
      {/if}
      {#if reader && note && notesOpen}
        <button onclick={insertPageLink} title="Inserir link da página atual (Ctrl+Shift+L)">⇥ p.{page}</button>
      {/if}
      {#if !sidebarOpen}
        <button onclick={() => fileInput.click()} title="Abrir PDF deste dispositivo">📂</button>
      {/if}
      {#if reader}
        <button onclick={toggleReader} title="Recolher/mostrar o PDF sem fechar (Alt+\)">
          {readerOpen ? '⟨ Recolher PDF' : 'PDF ⟩'}
        </button>
        <button onclick={closeReader} title="Fechar leitor">✕</button>
      {/if}
      <button
        onclick={() => (theme = theme === 'light' ? 'dark' : 'light')}
        title={theme === 'light' ? 'Tema escuro' : 'Tema claro (papel)'}>{theme === 'light' ? '☾' : '☀'}</button
      >
      <button onclick={toggleNotes} title="Notas (Ctrl+\)">
        {notesOpen ? 'Recolher notas ⟩' : '⟨ Notas'}
      </button>
    </div>

    <div
      class="panes"
      bind:this={panesEl}
      class:split={!!reader && readerOpen && notesOpen}
      class:notes-hidden={!notesOpen}
    >
      {#if reader}
        <section class="reader-pane" class:hidden={!readerOpen}>
          <PdfReader bind:this={pdf} src={reader.src} id={reader.id} title={reader.title} bind:page />
        </section>
        {#if readerOpen && notesOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="resizer"
            title="Arraste para redimensionar · duplo clique restaura"
            onpointerdown={(e) => startDrag('split', e)}
            onpointermove={onDrag}
            onpointerup={endDrag}
            onpointercancel={endDrag}
            ondblclick={() => resetSize('split')}
          ></div>
        {/if}
      {/if}
      <section class="notes-pane" class:hidden={!notesOpen}>
        {#if note}
          <div class="editor-wrap" class:hidden={preview}>
            <Editor
              bind:this={editor}
              path={note}
              onlink={openLink}
              keys={pdfKeys}
              {theme}
              oncontent={(t) => (noteText = t)}
              linkTargets={() => [...allPaths(tree)]}
              {reveal}
            />
          </div>
          {#if preview}
            <Preview text={noteText} fileUrl={previewFileUrl} exists={(t) => !!resolveLink(tree, t)} onlink={openLink} />
          {/if}
        {:else}
          <div class="empty">
            <p>Abra uma nota na barra lateral ou crie uma com <kbd>+</kbd>. Para ler um PDF deste dispositivo (fora do vault), use <kbd>📂</kbd>.</p>
            <p class="muted">
              <kbd>Ctrl+O</kbd> busca rápida · <kbd>Ctrl+Shift+F</kbd> busca no conteúdo · <kbd>Ctrl+E</kbd> preview ·  <kbd>Ctrl+\</kbd> recolhe as notas · <kbd>Alt+\</kbd> recolhe o PDF · <kbd>Ctrl+Shift+\</kbd> barra lateral · <kbd>Ctrl+Shift+L</kbd> insere link da página ·
              <kbd>Alt+↑/↓</kbd> rola o PDF · <kbd>Alt+Shift+↑/↓</kbd> muda de página · <kbd>Ctrl+clique</kbd> ou <kbd>Ctrl+Enter</kbd> segue um <code>[[link]]</code> · <kbd>:w</kbd> salva
            </p>
          </div>
        {/if}
      </section>
    </div>
  </main>

  {#if toast}<div class="toast" role="status">{toast}</div>{/if}
</div>

<style>
  .app {
    display: flex;
    height: 100vh;
  }
  .sidebar {
    width: var(--sidebar-w);
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background: var(--panel);
  }
  .sidebar-head {
    display: flex;
    gap: 6px;
    padding: 8px;
  }
  .sidebar-head input {
    flex: 1;
    min-width: 0;
  }
  .tree {
    flex: 1;
    overflow: auto;
    padding: 0 4px 12px;
    font-size: 14px;
  }
  .workspace {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
  }
  .crumb {
    flex: 1;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .panes {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .reader-pane {
    flex: 1;
    min-width: 0;
    min-height: 0;
  }
  .notes-pane {
    flex: 1;
    min-width: 0;
    min-height: 0;
  }
  .panes.split .notes-pane {
    flex: 0 0 var(--notes-w);
  }
  .reader-pane.hidden,
  .editor-wrap {
    height: 100%;
  }
  .editor-wrap.hidden,
  .notes-pane.hidden {
    display: none;
  }
  .empty {
    padding: 40px;
    line-height: 1.8;
  }
  .toast {
    position: fixed;
    bottom: 20px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 70;
    max-width: min(560px, 90vw);
    background: var(--panel);
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.2);
    padding: 10px 16px;
    border-radius: 8px;
    font-size: 13px;
  }
  .resizer {
    flex: 0 0 5px;
    cursor: col-resize;
    background: var(--border);
    touch-action: none;
  }
  .resizer:hover,
  .dragging .resizer {
    background: var(--accent);
  }
  .app.dragging {
    cursor: col-resize;
    user-select: none;
  }
  @media (max-width: 800px) {
    .panes.split {
      flex-direction: column;
    }
    .panes.split .notes-pane {
      flex: 1;
    }
    .panes .resizer {
      display: none;
    }
  }
</style>
