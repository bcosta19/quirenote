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
  import { linkCompletion, linkText } from './linkComplete'
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
    onimage,
  }: {
    path: string
    onlink: (target: string, page?: number, cfi?: string) => void
    /** Atalhos extras do app (ex.: rolar o PDF); têm prioridade sobre o vim. */
    keys?: KeyBinding[]
    theme?: Theme
    /** Texto atual do editor (inclui o que ainda não foi salvo); usado pelo preview. */
    oncontent?: (text: string) => void
    /** Arquivos do vault sugeridos ao digitar "[[" (lido na hora, sempre atualizado). */
    linkTargets?: () => string[]
    /** Posiciona o cursor numa linha (resultado de busca); n muda a cada pedido. */
    reveal?: { path: string; line: number; n: number } | null
    /** Grava uma imagem colada/arrastada e devolve o caminho no vault (ou null se falhar). */
    onimage?: (file: File) => Promise<string | null>
  } = $props()

  // Compartimento: troca o tema sem recriar o estado (mantém undo, cursor, modo vim).
  const themeSlot = new Compartment()
  // Somente leitura enquanto a nota é renomeada/movida/excluída (ver setLocked).
  const lockSlot = new Compartment()
  let locked = false

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
    onlink(link.target, link.page, link.cfi)
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
        lockSlot.of(EditorState.readOnly.of(locked)),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({ spellcheck: 'true', lang: 'pt-BR' }),
        // Mantém a quebra de linha original (CRLF de notas vindas do Windows):
        // sem isso, qualquer edição regravaria a nota inteira em LF.
        EditorState.lineSeparator.of(doc.includes('\r\n') ? '\r\n' : '\n'),
        // Prec.highest: senão o modo vim captura as teclas antes.
        Prec.highest(
          keymap.of([
            { key: 'Mod-s', run: () => (save(), true) },
            // Segue o [[link]] sob o cursor; numa linha de tarefa sem link, alterna [ ]/[x].
            {
              key: 'Mod-Enter',
              run: (v) => {
                const head = v.state.selection.main.head
                return followLinkAt(head) || toggleTask(v.state.doc.lineAt(head).number)
              },
            },
            ...keys,
          ]),
        ),
        EditorView.domEventHandlers({
          // Colar print/imagem: grava no vault e insere ![[imagem]], como o Obsidian.
          // Se a área de transferência também tiver texto (ex.: trecho de página), cola o texto.
          paste(e, v) {
            const images = imageFiles(e.clipboardData)
            if (!images.length || e.clipboardData?.getData('text/plain')) return false
            e.preventDefault()
            insertImages(images, v.state.selection.main.head)
            return true
          },
          drop(e, v) {
            const images = imageFiles(e.dataTransfer)
            if (!images.length) return false
            e.preventDefault()
            insertImages(images, v.posAtCoords({ x: e.clientX, y: e.clientY }) ?? v.state.selection.main.head)
            return true
          },
          mousedown(e, v) {
            if (!(e.ctrlKey || e.metaKey)) return false
            const pos = v.posAtCoords({ x: e.clientX, y: e.clientY })
            if (pos == null || !followLinkAt(pos)) return false
            e.preventDefault()
            return true
          },
        }),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) oncontent?.(u.state.sliceDoc())
          if (!u.docChanged || !base) return
          if (u.state.sliceDoc() === base.content) {
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

  function imageFiles(data: DataTransfer | null): File[] {
    return [...(data?.files ?? [])].filter((f) => f.type.startsWith('image/'))
  }

  async function insertImages(files: File[], pos: number) {
    if (!onimage || locked) return
    const notePath = base?.path
    for (const [i, file] of files.entries()) {
      const before = view?.state.doc
      const path = await onimage(file)
      // Ignora se falhou ou se o usuário trocou de nota enquanto a imagem subia.
      if (!path || !view || base?.path !== notePath) return
      // Se o texto mudou durante o upload, a posição guardada pode ter ficado no meio
      // de uma palavra: nesse caso a imagem entra onde o cursor está agora.
      const at = view.state.doc === before ? Math.min(pos, view.state.doc.length) : view.state.selection.main.head
      // Só o nome quando é único no vault, senão o caminho (como o Obsidian); várias imagens, uma por linha.
      const insert = (i > 0 ? '\n' : '') + `![[${linkText(path, linkTargets())}]]`
      view.dispatch({ changes: { from: at, insert }, selection: { anchor: at + insert.length }, userEvent: 'input.paste' })
      pos = at + insert.length
    }
    view?.focus()
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
    const content = view.state.sliceDoc()
    if (content === base.content) return
    status = 'saving'
    try {
      base = await saveNote(base.path, content, base.mtime)
      status = view.state.sliceDoc() === base.content ? 'saved' : 'dirty'
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

  /**
   * Trava a edição durante uma operação no arquivo: algo digitado entre gravar o
   * pendente e a resposta do servidor seria salvo no caminho antigo, recriando-o.
   */
  export function setLocked(value: boolean) {
    locked = value
    view?.dispatch({ effects: lockSlot.reconfigure(EditorState.readOnly.of(value)) })
  }

  /** Grava o que estiver pendente (antes de renomear, mover ou excluir a nota). */
  export async function flush(): Promise<boolean> {
    clearTimeout(timer)
    while (status === 'saving') await new Promise((r) => setTimeout(r, 30))
    if (status === 'dirty' || status === 'error') await save() // só retorna depois de gravar
    return status === 'saved'
  }

  /** Alterna "- [ ]" / "- [x]" na linha (1 = primeira). Devolve false se não for tarefa. */
  export function toggleTask(n: number): boolean {
    if (!view || locked || n < 1 || n > view.state.doc.lines) return false
    const line = view.state.doc.line(n)
    // Aceita tarefas dentro de citações ("> - [ ] x"), como o preview mostra.
    const m = line.text.match(/^(\s*(?:>\s*)*(?:[-*+]|\d+[.)])\s+\[)([ xX])\]/)
    if (!m) return false
    const at = line.from + m[1].length
    view.dispatch({ changes: { from: at, to: at + 1, insert: m[2] === ' ' ? 'x' : ' ' }, userEvent: 'input' })
    return true
  }

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
    view = undefined // chamadas tardias (ex.: setLocked no fim de uma exclusão) viram no-op
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
