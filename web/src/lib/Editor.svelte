<script lang="ts" module>
  import { Vim } from '@replit/codemirror-vim'

  // Ex commands are global in codemirror-vim; they target the active editor.
  let activeSave: (() => void) | null = null
  Vim.defineEx('write', 'w', () => activeSave?.())
  // Like a word processor: j/k and the arrows move through the lines shown on
  // screen, not the whole paragraph (equivalent to vim's gj/gk).
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
    /** Extra app shortcuts (e.g. scrolling the PDF); they take precedence over vim. */
    keys?: KeyBinding[]
    theme?: Theme
    /** Current editor text (including unsaved changes); used by the preview. */
    oncontent?: (text: string) => void
    /** Vault files suggested when typing "[[" (read on demand, always current). */
    linkTargets?: () => string[]
    /** Moves the cursor to a line (search result); n changes on every request. */
    reveal?: { path: string; line: number; n: number } | null
    /** Stores a pasted/dropped image and returns its vault path (or null on failure). */
    onimage?: (file: File) => Promise<string | null>
  } = $props()

  // Compartment: swaps the theme without recreating the state (keeps undo, cursor, vim mode).
  const themeSlot = new Compartment()
  // Read-only while the note is renamed/moved/deleted (see setLocked).
  const lockSlot = new Compartment()
  let locked = false

  type Status = 'loading' | 'saved' | 'dirty' | 'saving' | 'conflict' | 'error'

  let host: HTMLDivElement
  let view: EditorView | undefined
  // Last version confirmed on disk (content + mtime).
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
        // YAML frontmatter (--- ... ---) like Obsidian; otherwise it becomes a markdown heading.
        yamlFrontmatter({ content: markdown({ codeLanguages: languages }) }),
        codeBlocks,
        writing,
        visualLineNumbers,
        linkCompletion(() => linkTargets()),
        themeSlot.of(editorTheme(theme)),
        lockSlot.of(EditorState.readOnly.of(locked)),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({ spellcheck: 'true', lang: 'pt-BR' }),
        // Keeps the original line endings (CRLF from notes created on Windows):
        // otherwise any edit would rewrite the whole note as LF.
        EditorState.lineSeparator.of(doc.includes('\r\n') ? '\r\n' : '\n'),
        // Prec.highest: otherwise vim mode grabs the keys first.
        Prec.highest(
          keymap.of([
            { key: 'Mod-s', run: () => (save(), true) },
            // Follows the [[link]] under the cursor; on a task line without a link, toggles [ ]/[x].
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
          // Pasting a screenshot/image: stores it in the vault and inserts ![[image]], like Obsidian.
          // If the clipboard also has text (e.g. a snippet from a page), paste the text.
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
      // Ignore if it failed or the user switched notes while the image was uploading.
      if (!path || !view || base?.path !== notePath) return
      // If the text changed during the upload, the saved position may now be in the middle
      // of a word: in that case the image goes where the cursor is now.
      const at = view.state.doc === before ? Math.min(pos, view.state.doc.length) : view.state.selection.main.head
      // Name only when unique in the vault, otherwise the path (like Obsidian); several images, one per line.
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
      if (p !== path) return // the user already switched notes
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

  /** Called when the file changes on disk (Neovim, Obsidian, Syncthing). */
  export async function externalChange() {
    if (!view || !base) return
    const note = await getNote(base.path).catch(() => null)
    if (!note || note.mtime === base.mtime) return // our own save
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

  // Each positioning request applies once: returning to the note later does not jump again.
  let revealed = 0
  function revealLine(r: { line: number; n: number }) {
    if (!view || r.n === revealed) return
    revealed = r.n
    const line = view.state.doc.line(Math.min(Math.max(r.line, 1), view.state.doc.lines))
    view.dispatch({ selection: { anchor: line.from }, effects: EditorView.scrollIntoView(line.from, { y: 'center' }) })
    view.focus()
  }

  // Positioning request for an already open note (for another note, load applies it).
  $effect(() => {
    const r = reveal
    if (r) untrack(() => base?.path === r.path && status !== 'loading' && revealLine(r))
  })

  /**
   * Locks editing during a file operation: anything typed between saving the pending
   * changes and the server response would be saved to the old path, recreating it.
   */
  export function setLocked(value: boolean) {
    locked = value
    view?.dispatch({ effects: lockSlot.reconfigure(EditorState.readOnly.of(value)) })
  }

  /** Saves anything pending (before renaming, moving or deleting the note). */
  export async function flush(): Promise<boolean> {
    clearTimeout(timer)
    while (status === 'saving') await new Promise((r) => setTimeout(r, 30))
    if (status === 'dirty' || status === 'error') await save() // only returns after saving
    return status === 'saved'
  }

  /** Toggles "- [ ]" / "- [x]" on the line (1 = first). Returns false if it is not a task. */
  export function toggleTask(n: number): boolean {
    if (!view || locked || n < 1 || n > view.state.doc.lines) return false
    const line = view.state.doc.line(n)
    // Accepts tasks inside quotes ("> - [ ] x"), as the preview shows them.
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
    const p = path // the only dependency: switching notes reloads the editor
    untrack(() => load(p))
  })

  onDestroy(() => {
    if (status === 'dirty') save()
    if (activeSave === save) activeSave = null
    view?.destroy()
    view = undefined // late calls (e.g. setLocked at the end of a delete) become no-ops
  })

  const labels: Record<Status, string> = {
    loading: 'loading…',
    saved: 'saved',
    dirty: 'editing…',
    saving: 'saving…',
    conflict: 'conflict',
    error: 'error',
  }
</script>

<div class="editor">
  {#if conflict}
    <div class="banner">
      <span>This note changed on disk (Neovim/Obsidian?).</span>
      <button onclick={useDisk}>Use the disk version</button>
      <button onclick={keepMine}>Manter a minha</button>
    </div>
  {/if}
  {#if status === 'error'}
    <div class="banner error">{errorMsg} <button onclick={save}>try again</button></div>
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
  /* [[link]] autocompletion list: the UI font, not the text font. */
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
  /* Centered reading column (~100 characters), like Neovim zen mode. */
  .cm :global(.cm-scroller) {
    justify-content: center;
  }
  .cm :global(.cm-content) {
    flex: 0 1 44em;
    max-width: 44em;
    padding: 16px 0 40vh; /* breathing room at the end: you can write in the middle of the screen */
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
