<script lang="ts" module>
  import { Vim } from '@replit/codemirror-vim'

  // Comandos ex são globais no codemirror-vim; apontam para o editor ativo.
  let activeSave: (() => void) | null = null
  Vim.defineEx('write', 'w', () => activeSave?.())
  // Como num processador de texto: j/k e as setas andam pelas linhas que aparecem
  // na tela, não pelo parágrafo inteiro (equivale a gj/gk do vim).
  for (const mode of ['normal', 'visual']) {
    Vim.map('j', 'gj', mode)
    Vim.map('k', 'gk', mode)
    Vim.map('<Down>', 'gj', mode)
    Vim.map('<Up>', 'gk', mode)
  }
</script>

<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte'
  import { EditorView } from 'codemirror'
  import { Compartment, EditorState, Prec } from '@codemirror/state'
  import { keymap, type KeyBinding } from '@codemirror/view'
  import { markdown } from '@codemirror/lang-markdown'
  import { yamlFrontmatter } from '@codemirror/lang-yaml'
  import { codeBlocks } from './codeblocks'
  import { writing } from './writing'
  import { visualLineNumbers } from './visualLines'
  import { linkCompletion } from './linkComplete'
  import { languages } from '@codemirror/language-data'
  import { editorTheme, type Theme } from './theme'
  import { vim } from '@replit/codemirror-vim'
  import { ConflictError, getNote, saveNote, type Note } from './api'
  import { linkAt } from './links'

  let {
    path,
    onlink,
    keys = [],
    theme = 'light',
    oncontent,
    linkTargets = () => [],
    reveal,
  }: {
    path: string
    onlink: (target: string, page?: number) => void
    /** Atalhos extras do app (ex.: rolar o PDF); têm prioridade sobre o vim. */
    keys?: KeyBinding[]
    theme?: Theme
    /** Texto atual do editor (inclui o que ainda não foi salvo); usado pelo preview. */
    oncontent?: (text: string) => void
    /** Arquivos do vault sugeridos ao digitar "[[" (lido na hora, sempre atualizado). */
    linkTargets?: () => string[]
    /** Posiciona o cursor numa linha (resultado de busca); n muda a cada pedido. */
    reveal?: { path: string; line: number; n: number } | null
  } = $props()

  // Compartimento: troca o tema sem recriar o estado (mantém undo, cursor, modo vim).
  const themeSlot = new Compartment()

  type Status = 'loading' | 'saved' | 'dirty' | 'saving' | 'conflict' | 'error'

  let host: HTMLDivElement
  let view: EditorView | undefined
  // Última versão confirmada no disco (conteúdo + mtime).
  let base: Note | null = null
  let status = $state<Status>('loading')
  let errorMsg = $state('')
  let conflict = $state<Note | null>(null)
  let timer: ReturnType<typeof setTimeout> | undefined

  const AUTOSAVE_MS = 1000

  function followLinkAt(pos: number): boolean {
    if (!view) return false
    const line = view.state.doc.lineAt(pos)
    const link = linkAt(line.text, pos - line.from)
    if (!link) return false
    onlink(link.target, link.page)
    return true
  }

  function newState(doc: string): EditorState {
    return EditorState.create({
      doc,
      extensions: [
        vim(),
        // Frontmatter YAML (--- ... ---) como no Obsidian; sem isso vira título markdown.
        yamlFrontmatter({ content: markdown({ codeLanguages: languages }) }),
        codeBlocks,
        writing,
        visualLineNumbers,
        linkCompletion(() => linkTargets()),
        themeSlot.of(editorTheme(theme)),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({ spellcheck: 'true', lang: 'pt-BR' }),
        // Prec.highest: senão o modo vim captura as teclas antes.
        Prec.highest(
          keymap.of([
            { key: 'Mod-s', run: () => (save(), true) },
            // Segue o [[link]] sob o cursor.
            { key: 'Mod-Enter', run: (v) => followLinkAt(v.state.selection.main.head) },
            ...keys,
          ]),
        ),
        EditorView.domEventHandlers({
          mousedown(e, v) {
            if (!(e.ctrlKey || e.metaKey)) return false
            const pos = v.posAtCoords({ x: e.clientX, y: e.clientY })
            if (pos == null || !followLinkAt(pos)) return false
            e.preventDefault()
            return true
          },
        }),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) oncontent?.(u.state.doc.toString())
          if (!u.docChanged || !base) return
          if (u.state.doc.toString() === base.content) {
            if (status === 'dirty') status = 'saved'
            return
          }
          if (status === 'saved' || status === 'error') status = 'dirty'
          clearTimeout(timer)
          timer = setTimeout(save, AUTOSAVE_MS)
        }),
      ],
    })
  }

  function setDoc(content: string) {
    if (!view) return
    const head = Math.min(view.state.selection.main.head, content.length)
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: content },
      selection: { anchor: head },
    })
  }

  async function save() {
    clearTimeout(timer)
    if (!view || !base || status === 'conflict' || status === 'saving') return
    const content = view.state.doc.toString()
    if (content === base.content) return
    status = 'saving'
    try {
      base = await saveNote(base.path, content, base.mtime)
      status = view.state.doc.toString() === base.content ? 'saved' : 'dirty'
      if (status === 'dirty') timer = setTimeout(save, AUTOSAVE_MS)
    } catch (e) {
      if (e instanceof ConflictError) {
        conflict = e.current
        status = 'conflict'
      } else {
        errorMsg = (e as Error).message
        status = 'error'
      }
    }
  }

  async function load(p: string) {
    if (base && base.path !== p && status === 'dirty') await save()
    status = 'loading'
    conflict = null
    try {
      const note = await getNote(p)
      if (p !== path) return // usuário já trocou de nota
      base = note
      view?.setState(newState(note.content))
      oncontent?.(note.content)
      status = 'saved'
      view?.focus()
      if (reveal?.path === p) revealLine(reveal)
    } catch (e) {
      errorMsg = (e as Error).message
      status = 'error'
    }
  }

  /** Chamado quando o arquivo muda no disco (Neovim, Obsidian, Syncthing). */
  export async function externalChange() {
    if (!view || !base) return
    const note = await getNote(base.path).catch(() => null)
    if (!note || note.mtime === base.mtime) return // nosso próprio salvamento
    if (status === 'saved') {
      base = note
      setDoc(note.content)
    } else {
      conflict = note
      status = 'conflict'
    }
  }

  function useDisk() {
    if (!conflict) return
    base = conflict
    conflict = null
    setDoc(base.content)
    status = 'saved'
  }

  function keepMine() {
    if (!conflict || !base) return
    base = { ...base, mtime: conflict.mtime }
    conflict = null
    status = 'dirty'
    save()
  }

  // Cada pedido de posicionamento vale uma vez: voltar à nota depois não pula de novo.
  let revealed = 0
  function revealLine(r: { line: number; n: number }) {
    if (!view || r.n === revealed) return
    revealed = r.n
    const line = view.state.doc.line(Math.min(Math.max(r.line, 1), view.state.doc.lines))
    view.dispatch({ selection: { anchor: line.from }, effects: EditorView.scrollIntoView(line.from, { y: 'center' }) })
    view.focus()
  }

  // Pedido de posicionar numa nota já aberta (se for outra nota, o load aplica ao carregar).
  $effect(() => {
    const r = reveal
    if (r) untrack(() => base?.path === r.path && status !== 'loading' && revealLine(r))
  })

  export function focus() {
    view?.focus()
  }

  export function insert(text: string) {
    if (!view) return
    view.dispatch(view.state.replaceSelection(text))
    view.focus()
  }

  onMount(() => {
    view = new EditorView({ parent: host, state: newState('') })
    activeSave = save
    const onUnload = (e: BeforeUnloadEvent) => {
      if (status === 'dirty' || status === 'saving') e.preventDefault()
    }
    window.addEventListener('beforeunload', onUnload)
    return () => window.removeEventListener('beforeunload', onUnload)
  })

  $effect(() => {
    const ext = editorTheme(theme)
    untrack(() => view?.dispatch({ effects: themeSlot.reconfigure(ext) }))
  })

  $effect(() => {
    const p = path // única dependência: trocar de nota recarrega o editor
    untrack(() => load(p))
  })

  onDestroy(() => {
    if (status === 'dirty') save()
    if (activeSave === save) activeSave = null
    view?.destroy()
  })

  const labels: Record<Status, string> = {
    loading: 'carregando…',
    saved: 'salvo',
    dirty: 'editando…',
    saving: 'salvando…',
    conflict: 'conflito',
    error: 'erro',
  }
</script>

<div class="editor">
  {#if conflict}
    <div class="banner">
      <span>Esta nota mudou no disco (Neovim/Obsidian?).</span>
      <button onclick={useDisk}>Usar a do disco</button>
      <button onclick={keepMine}>Manter a minha</button>
    </div>
  {/if}
  {#if status === 'error'}
    <div class="banner error">{errorMsg} <button onclick={save}>tentar de novo</button></div>
  {/if}
  <div class="cm" bind:this={host}></div>
  <div class="status" class:warn={status === 'conflict' || status === 'error'}>{labels[status]}</div>
</div>

<style>
  .editor {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .cm {
    flex: 1;
    min-height: 0;
  }
  .cm :global(.cm-editor) {
    height: 100%;
    font-size: 19px;
  }
  .cm :global(.cm-scroller) {
    font-family: var(--editor-font);
    line-height: 1.6;
  }
  .cm :global(.cm-gutters) {
    font-family: var(--mono);
    font-size: 11px;
  }
  .cm :global(.cm-visualLineNumbers .cm-gutterElement) {
    padding: 0 10px 0 6px;
    text-align: right;
  }
  /* Lista do autocompletar de [[links]]: fonte da interface, não a do texto. */
  .cm :global(.cm-tooltip-autocomplete > ul) {
    font-family: system-ui, sans-serif;
    font-size: 14px;
    max-height: 18em;
  }
  .cm :global(.cm-tooltip-autocomplete > ul > li) {
    padding: 3px 10px;
  }
  .cm :global(.cm-completionDetail) {
    margin-left: 1em;
    font-style: normal;
    font-size: 12px;
    color: var(--muted);
  }
  .cm :global(.cm-codeblock) {
    font-family: var(--mono);
    font-size: 0.8em;
    background: var(--code-bg);
  }
  /* Coluna de leitura centralizada (~100 caracteres), como o zen mode do Neovim. */
  .cm :global(.cm-scroller) {
    justify-content: center;
  }
  .cm :global(.cm-content) {
    flex: 0 1 44em;
    max-width: 44em;
    padding: 16px 0 40vh; /* respiro no fim: dá para escrever no meio da tela */
  }
  .banner {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 6px 10px;
    background: var(--warn-bg);
    font-size: 13px;
  }
  .banner button {
    white-space: nowrap;
  }
  .banner.error {
    background: var(--error-bg);
  }
  .status {
    position: absolute;
    right: 10px;
    bottom: 6px;
    font-size: 11px;
    color: var(--muted);
    pointer-events: none;
    z-index: 5;
  }
  .status.warn {
    color: var(--warn);
  }
</style>
