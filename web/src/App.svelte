<script lang="ts">
  import FilePlus from '@lucide/svelte/icons/file-plus'
  import FolderPlus from '@lucide/svelte/icons/folder-plus'
  import FolderOpen from '@lucide/svelte/icons/folder-open'
  import PanelLeft from '@lucide/svelte/icons/panel-left'
  import Search from '@lucide/svelte/icons/search'
  import CornerUpLeft from '@lucide/svelte/icons/corner-up-left'
  import Pencil from '@lucide/svelte/icons/pencil'
  import Eye from '@lucide/svelte/icons/eye'
  import Link from '@lucide/svelte/icons/link'
  import PanelLeftClose from '@lucide/svelte/icons/panel-left-close'
  import PanelLeftOpen from '@lucide/svelte/icons/panel-left-open'
  import PanelRightClose from '@lucide/svelte/icons/panel-right-close'
  import PanelRightOpen from '@lucide/svelte/icons/panel-right-open'
  import X from '@lucide/svelte/icons/x'
  import Moon from '@lucide/svelte/icons/moon'
  import Sun from '@lucide/svelte/icons/sun'
  import { onMount, untrack } from 'svelte'
  import {
    ExistsError,
    createDir,
    createNote,
    fileUrl,
    getBacklinks,
    getTree,
    moveItem,
    previewRelink,
    relinkNotes,
    subscribe,
    trashItem,
    uploadImage,
    type Entry,
    type SearchResult,
  } from './lib/api'
  import { cfiLink, pageLink, resolveLink } from './lib/links'
  import Editor from './lib/Editor.svelte'
  import PdfReader from './lib/PdfReader.svelte'
  import EpubReader, { type EpubLocation } from './lib/EpubReader.svelte'
  import Preview from './lib/Preview.svelte'
  import QuickSwitcher from './lib/QuickSwitcher.svelte'
  import PromptDialog from './lib/PromptDialog.svelte'
  import { IMAGE } from './lib/markdown'
  import SearchPanel from './lib/SearchPanel.svelte'
  import Outline from './lib/Outline.svelte'
  import BacklinksPanel from './lib/BacklinksPanel.svelte'
  import ContextMenu, { type MenuItem } from './lib/ContextMenu.svelte'
  import Tree from './lib/Tree.svelte'
  import type { Theme } from './lib/theme'

  // Screen state, mirrored in the URL (#note=...&doc=...&page=...) to survive reloads.
  const initial = new URLSearchParams(location.hash.slice(1))
  let note = $state<string | null>(initial.get('note'))
  let doc = $state<string | null>(initial.get('doc'))
  let page = $state(Number(initial.get('page') || 1))

  let tree = $state<Entry[]>([])
  let filter = $state('')
  let sidebarOpen = $state(localStorage.getItem('quirenote:sidebar') !== '0')
  let notesOpen = $state(localStorage.getItem('quirenote:notes') !== '0')
  // Collapsed reader: the PDF stays loaded (same page), it is just hidden.
  let readerOpen = $state(true)
  // Markdown preview (reading mode, like Obsidian's Ctrl+E).
  let preview = $state(false)
  let noteText = $state('')

  // Sizes adjustable with the mouse (saved per device).
  const SIDEBAR_W = 260
  const NOTES_PCT = 42
  let sidebarW = $state(Number(localStorage.getItem('quirenote:sidebarW')) || SIDEBAR_W)
  let notesPct = $state(Number(localStorage.getItem('quirenote:notesPct')) || NOTES_PCT)
  let dragging = $state<null | 'sidebar' | 'split'>(null)
  let panesEl: HTMLDivElement
  let theme = $state<Theme>(localStorage.getItem('quirenote:theme') === 'dark' ? 'dark' : 'light')
  let toast = $state('')
  let editor: ReturnType<typeof Editor> | undefined = $state()
  let fileInput: HTMLInputElement
  let pdf: ReturnType<typeof PdfReader> | undefined = $state()
  let epub: ReturnType<typeof EpubReader> | undefined = $state()
  // EPUB: current position (for links) and a request to go to a position (#cfi= link).
  let epubLoc = $state<EpubLocation | null>(null)
  let epubTarget = $state<{ id: string; cfi: string; n: number } | null>(null)
  /** Asks the reader of book id to go to cfi (without cfi, cancels pending requests). */
  function goEpub(id?: string, cfi?: string) {
    epubTarget = id && cfi ? { id, cfi, n: (epubTarget?.n ?? 0) + 1 } : null
  }
  /** Switched documents: the previous book's position/request no longer apply. */
  function forgetEpub() {
    epubTarget = null
    epubLoc = null
  }

  // Control the PDF without taking focus from the editor.
  // Without an open PDF, they return false and the editor keeps its default behavior.
  function togglePreview() {
    if (!note) return
    preview = !preview
    if (!preview) setTimeout(() => editor?.focus())
  }

  /** URL of a file referenced in the note: [[name]] resolved in the vault, or a path relative to the note. */
  function previewFileUrl(target: string): string {
    const found = resolveLink(tree, target)
    if (found) return fileUrl(found)
    const dir = note?.includes('/') ? note.slice(0, note.lastIndexOf('/') + 1) : ''
    return fileUrl(target.includes('/') ? target : dir + target)
  }

  const pdfKeys = [
    { key: 'Alt-ArrowDown', run: () => (activeReader && readerOpen ? (activeReader.scroll(1), true) : false) },
    { key: 'Alt-ArrowUp', run: () => (activeReader && readerOpen ? (activeReader.scroll(-1), true) : false) },
    { key: 'Alt-Shift-ArrowDown', run: () => (activeReader && readerOpen ? (activeReader.turnPage(1), true) : false) },
    { key: 'Alt-Shift-ArrowUp', run: () => (activeReader && readerOpen ? (activeReader.turnPage(-1), true) : false) },
  ]

  // PDF picked from the device: read directly by the browser, no upload.
  // Not stored in the URL because the blob: is gone after a reload.
  let localPdf = $state<{ name: string; size: number; url: string; file: File } | null>(null)

  // What the reader shows: a vault PDF or a local PDF.
  const kindOf = (name: string) => (/\.epub$/i.test(name) ? ('epub' as const) : ('pdf' as const))
  const reader = $derived(
    doc
      ? { src: fileUrl(doc), file: null, id: doc, title: doc, link: doc, kind: kindOf(doc) }
      : localPdf
        ? {
            src: localPdf.url,
            file: localPdf.file, // a local EPUB is read straight from the file (no upload)
            id: `local:${localPdf.name}:${localPdf.size}`,
            title: localPdf.name,
            link: localPdf.name,
            kind: kindOf(localPdf.name),
          }
        : null,
  )
  /** Visible reader (PDF or EPUB), for the Alt+↑/↓ shortcuts while the editor has focus. */
  const activeReader = $derived(reader?.kind === 'epub' ? epub : pdf)

  $effect(() => {
    const params = new URLSearchParams()
    if (note) params.set('note', note)
    if (doc) {
      params.set('doc', doc)
      params.set('page', String(page))
    }
    // Switching note/PDF creates a history entry (browser Back/Forward);
    // changing only the PDF page just updates the current entry.
    const current = new URLSearchParams(location.hash.slice(1))
    const same = current.get('note') === params.get('note') && current.get('doc') === params.get('doc')
    if (same) history.replaceState(null, '', `#${params}`)
    else history.pushState(null, '', `#${params}`)
  })

  // Browser Back/Forward (or Alt+←/→) restores the note, PDF and page.
  function onPopState() {
    reveal = null
    const params = new URLSearchParams(location.hash.slice(1))
    note = params.get('note')
    const nextDoc = params.get('doc')
    if (nextDoc !== doc) forgetEpub()
    if (nextDoc) {
      closeLocal()
      page = Number(params.get('page') || 1)
      readerOpen = true
    }
    doc = nextDoc
  }

  // Recently opened, for the quick switcher (Ctrl+O / Ctrl+P).
  let recent = $state<string[]>(loadRecent())
  function loadRecent(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem('quirenote:recent') || '[]')
      return Array.isArray(v) ? v.filter((p) => typeof p === 'string') : []
    } catch {
      return [] // a corrupted value must not keep the app from starting
    }
  }
  let switcherOpen = $state(false)
  // Full-text search (Ctrl+Shift+F); keeps the last term to reopen with it.
  let searchOpen = $state(false)
  let outlineOpen = $state(false) // note outline (Ctrl+Shift+O)

  // Backlinks of the open note: reloaded when switching notes and when the vault changes.
  let backlinks = $state<SearchResult[] | null>(null)
  let backlinksOpen = $state(false)
  let backlinksTimer: ReturnType<typeof setTimeout> | undefined
  let backlinksReq: AbortController | undefined
  function loadBacklinks(delay = 0) {
    clearTimeout(backlinksTimer)
    backlinksTimer = setTimeout(async () => {
      backlinksReq?.abort()
      const current = note
      if (!current) return (backlinks = null)
      backlinksReq = new AbortController()
      try {
        const res = await getBacklinks(current, backlinksReq.signal)
        if (current === note) backlinks = res
      } catch {
        // Transient failure (e.g. slow rg): keep this note's previous count.
      }
    }, delay)
  }
  $effect(() => {
    note // dependency
    untrack(() => {
      backlinks = null
      backlinksOpen = false // the panel belonged to the previous note; it does not reopen by itself
      loadBacklinks()
    })
  })
  let searchPanel: ReturnType<typeof SearchPanel> | undefined = $state()
  let lastSearch = $state('')
  let reveal = $state<{ path: string; line: number; n: number } | null>(null)
  function openAtLine(path: string, line: number) {
    preview = false
    const n = (reveal?.n ?? 0) + 1
    openPath(path) // limpa pedidos antigos; o novo vem logo abaixo
    reveal = { path, line, n }
  }

  // Text dialog with the app look (instead of the browser's prompt()).
  type Dialog = {
    message: string
    value: string
    hint: string
    input: boolean
    okLabel: string
    danger: boolean
    resolve: (v: string | null) => void
  }
  let dialog = $state<Dialog | null>(null)
  function ask(message: string, value = '', hint = ''): Promise<string | null> {
    return new Promise((resolve) => (dialog = { message, value, hint, input: true, okLabel: 'OK', danger: false, resolve }))
  }
  async function confirmAction(message: string, hint: string, okLabel: string, danger = true): Promise<boolean> {
    const r = await new Promise<string | null>(
      (resolve) => (dialog = { message, value: '', hint, input: false, okLabel, danger, resolve }),
    )
    return r !== null
  }

  // Tree context menu: rename/move, trash, create inside folders.
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null)
  function openMenu(e: Entry, ev: MouseEvent) {
    const items: MenuItem[] = e.dir
      ? [
          { label: 'New note here', action: () => newNote(e.path) },
          { label: 'New subfolder', action: () => newFolder(e.path) },
        ]
      : [{ label: 'Open', action: () => openPath(e.path) }]
    items.push(
      { label: 'Rename / move…', action: () => renameEntry(e) },
      { label: 'Move to trash', action: () => trashEntry(e), danger: true },
    )
    menu = { x: ev.clientX, y: ev.clientY, items }
  }

  /** p is the item itself or (for a folder) something inside it. */
  const inside = (p: string | null, e: Entry): p is string => !!p && (p === e.path || (e.dir && p.startsWith(e.path + '/')))
  const remap = (p: string, from: string, to: string) => to + p.slice(from.length)

  /**
   * Runs an operation on item e. If the open note is affected, it locks the editor,
   * saves pending changes first (otherwise autosave would recreate the old file) and unlocks at the end.
   */
  async function withSettled(e: Entry, op: () => Promise<void>) {
    const ed = inside(note, e) ? editor : undefined
    ed?.setLocked(true)
    try {
      if (ed && !(await ed.flush())) {
        flash('Resolve the conflict or error in the open note before moving or deleting.')
        return
      }
      await op()
    } finally {
      ed?.setLocked(false)
    }
  }

  async function pasteImage(file: File): Promise<string | null> {
    if (!note) return null
    try {
      const path = await uploadImage(note, file)
      await refreshTree() // the preview resolves ![[image]] through the tree
      return path
    } catch (err) {
      flash((err as Error).message)
      return null
    }
  }

  async function refreshTree() {
    try {
      tree = await getTree()
    } catch (err) {
      flash((err as Error).message)
    }
  }

  async function renameEntry(e: Entry) {
    let to = (await ask(e.dir ? 'Rename / move folder:' : 'Rename / move:', e.path, 'Change the name or the path; missing folders are created.'))
      ?.trim()
      .replace(/^\/+|\/+$/g, '')
    if (!to || to === e.path) return
    // No extension in the new name: keep the original one (a note stays .md, a PDF stays .pdf).
    const ext = e.dir ? '' : (e.name.match(/\.[^.]+$/)?.[0] ?? '')
    if (ext && !/\.[^./]+$/.test(to.split('/').pop()!)) to += ext
    if (invalidPath(to)) return flash('Invalid name: do not use "..", a leading ".", empty path segments or # ^ [ ] |')
    const target = to
    let moved = ''
    await withSettled(e, async () => {
      try {
        const { path } = await moveItem(e.path, target)
        if (inside(note, e)) note = remap(note, e.path, path)
        if (inside(doc, e)) doc = remap(doc, e.path, path)
        recent = recent.map((p) => (inside(p, e) ? remap(p, e.path, path) : p))
        localStorage.setItem('quirenote:recent', JSON.stringify(recent))
        flash(`Moved to ${path}`)
        if (!e.dir) moved = path
      } catch (err) {
        flash(err instanceof ExistsError ? `Something already exists at ${target}` : (err as Error).message)
      }
    })
    await refreshTree()
    if (moved) await offerRelink(e.path, moved)
  }

  /** After renaming a file, offers to update the [[links]] of the notes that referenced it. */
  async function offerRelink(from: string, to: string) {
    // A note became another type (or vice versa): the link form changes; left to the user.
    if (/\.md$/i.test(from) !== /\.md$/i.test(to)) return
    let plan: { files: string[]; ambiguous: boolean }
    try {
      plan = await previewRelink(from, to) // exact, unlimited list of what would change
    } catch {
      return // no ripgrep or the search failed: the rename already worked, just do not offer
    }
    const ambiguousNote = plan.ambiguous
      ? ' Another file with the old name still exists, so name-only links stay as they are.'
      : ''
    if (!plan.files.length) {
      if (plan.ambiguous) flash(`Name-only links were not changed: another “${from.split('/').pop()}” still exists.`)
      return
    }
    const n = plan.files.length
    const ok = await confirmAction(
      `Update the links in ${n} ${n === 1 ? 'note' : 'notes'}?`,
      `${plan.files.slice(0, 5).join(', ')}${n > 5 ? ` and ${n - 5} more` : ''} still ${n === 1 ? 'points' : 'point'} to the old name ` +
        `(links inside code blocks change too).${ambiguousNote}`,
      'Update links',
      false,
    )
    if (!ok) return
    // The open note may be one of the referencing notes: lock the editor and save pending changes first
    // (the server skips notes changed during the operation instead of overwriting them).
    const ed = note && plan.files.includes(note) ? editor : undefined
    ed?.setLocked(true)
    try {
      if (ed && !(await ed.flush())) return flash('Resolve the conflict or error in the open note before updating the links.')
      const { updated, skipped } = await relinkNotes(from, to)
      flash(
        `${updated.length} ${updated.length === 1 ? 'note updated' : 'notes updated'}` +
          (skipped.length ? `; skipped (changed meanwhile): ${skipped.join(', ')}` : ''),
      )
    } catch (err) {
      flash((err as Error).message)
    } finally {
      ed?.setLocked(false)
    }
  }

  async function trashEntry(e: Entry) {
    const ok = await confirmAction(
      `Move “${e.name.replace(/\.md$/i, '')}” to the trash?`,
      `${e.dir ? 'The whole folder goes' : 'It goes'} to .trash in the vault (the Obsidian trash); you can restore it from there.`,
      'Move to trash',
    )
    if (!ok) return
    await withSettled(e, async () => {
      try {
        await trashItem(e.path)
        if (inside(note, e)) note = null
        if (inside(doc, e)) closeReader()
        recent = recent.filter((p) => !inside(p, e))
        localStorage.setItem('quirenote:recent', JSON.stringify(recent))
        flash(`“${e.name.replace(/\.md$/i, '')}” moved to the trash`)
      } catch (err) {
        flash((err as Error).message)
      }
    })
    await refreshTree()
  }
  function closeDialog(result: string | null) {
    dialog?.resolve(result)
    dialog = null
    setTimeout(() => editor?.focus())
  }
  function remember(path: string) {
    recent = [path, ...recent.filter((p) => p !== path)].slice(0, 30)
    localStorage.setItem('quirenote:recent', JSON.stringify(recent))
  }
  $effect(() => localStorage.setItem('quirenote:sidebar', sidebarOpen ? '1' : '0'))
  $effect(() => localStorage.setItem('quirenote:notes', notesOpen ? '1' : '0'))

  // Never leaves both panels collapsed at the same time.
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
    localStorage.setItem('quirenote:sidebarW', String(Math.round(sidebarW)))
    localStorage.setItem('quirenote:notesPct', String(Math.round(notesPct)))
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
    localStorage.setItem('quirenote:theme', theme)
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
  // The server tree includes images (so the preview can resolve ![[photo.png]]);
  // the sidebar only shows notes and books.
  function onlyDocs(entries: Entry[]): Entry[] {
    return entries
      .filter((e) => e.dir || /\.(md|pdf|epub)$/i.test(e.name))
      .map((e) => (e.dir ? { ...e, children: onlyDocs(e.children ?? []) } : e))
  }
  const visibleTree = $derived(filterTree(onlyDocs(tree), filter.trim().toLowerCase()))

  let toastTimer: ReturnType<typeof setTimeout> | undefined
  function flash(msg: string) {
    toast = msg
    clearTimeout(toastTimer) // a new toast does not disappear early because of the previous one
    toastTimer = setTimeout(() => (toast = ''), 3000)
  }

  function openPath(path: string, targetPage?: number, targetCfi?: string) {
    // Opening by any other path cancels the positioning requested by the search.
    reveal = null
    if (/\.(md|pdf|epub)$/i.test(path)) remember(path)
    if (/\.md$/i.test(path)) {
      note = path
      notesOpen = true
    } else if (/\.pdf$/i.test(path)) {
      // The link page; without it, the reader goes back to the last page read.
      page = targetPage ?? (path === doc ? page : 1)
      if (path !== doc) forgetEpub()
      closeLocal()
      doc = path
      readerOpen = true
    } else if (/\.epub$/i.test(path)) {
      closeLocal()
      if (path !== doc) forgetEpub()
      doc = path
      readerOpen = true
      goEpub(path, targetCfi)
    } else {
      flash('Format not supported by the reader')
    }
  }

  function openLocal(e: Event) {
    const input = e.target as HTMLInputElement
    const file = input.files?.[0]
    input.value = '' // allows picking the same file again
    if (!file) return
    closeLocal()
    localPdf = { name: file.name, size: file.size, url: URL.createObjectURL(file), file }
    doc = null
    page = 1
    forgetEpub()
    readerOpen = true
  }

  function closeLocal() {
    if (localPdf) URL.revokeObjectURL(localPdf.url)
    localPdf = null
  }

  function closeReader() {
    closeLocal()
    forgetEpub()
    doc = null
  }

  async function openLink(target: string, targetPage?: number, targetCfi?: string) {
    const path = resolveLink(tree, target)
    if (path) return openPath(path, targetPage, targetCfi)
    // Link to the open local PDF/EPUB (stored with the file name only).
    const name = target.split('/').pop()!
    if (localPdf && name === localPdf.name) {
      if (targetPage) page = targetPage
      if (targetCfi && reader) goEpub(reader.id, targetCfi)
      return
    }
    // Link to a note that does not exist yet: offer to create it (like Obsidian),
    // in the current note's folder when the link has no path.
    // PDFs, EPUBs and images do not become notes; names with dots (e.g. [[2024.10.12]]) do.
    if (!/\.(pdf|epub)$/i.test(name) && !IMAGE.test(name)) {
      const dir = !target.includes('/') && note?.includes('/') ? note.slice(0, note.lastIndexOf('/') + 1) : ''
      const suggestion = (dir + target).replace(/\.md$/i, '') + '.md'
      const chosen = await ask(`Create the note “${name.replace(/\.md$/i, '')}”?`, suggestion, 'The link does not point to any note yet. Adjust the path if you like.')
      if (chosen?.trim()) await createAndOpen(chosen)
      return
    }
    flash(`Not found: ${target}${/\.(pdf|epub)$/i.test(target) ? ' (if it is a local file, open it with the open file button)' : ''}`)
  }

  function allPaths(entries: Entry[]): Set<string> {
    const s = new Set<string>()
    const walk = (es: Entry[]) => es.forEach((e) => (e.dir ? walk(e.children ?? []) : s.add(e.path)))
    walk(entries)
    return s
  }

  /** dir: subfolder chosen in the tree; without it, asks for the full path. */
  async function newNote(dir?: string) {
    // With a PDF open, suggests a reading note linked to it.
    const base = reader ? reader.link.split('/').pop()!.replace(/\.(pdf|epub)$/i, '') : 'New note'
    let path: string | undefined
    if (dir) {
      const name = (await ask(`New note in "${dir}":`, base))?.trim().replace(/^\/+/, '')
      if (!name) return
      path = `${dir}/${name}`
    } else {
      const suggestion = reader ? `Book notes/${base}.md` : `Random notes/${base}.md`
      path = (await ask('New note:', suggestion, 'Path relative to the vault; missing folders are created.'))?.trim()
    }
    if (path) await createAndOpen(path)
  }

  /**
   * Creates the note (with a title and, if a PDF is open, the source frontmatter) and opens it.
   * If it already exists, it just opens it: the server never overwrites an existing note.
   */
  async function createAndOpen(path: string) {
    path = path.trim().replace(/^\/+/, '')
    if (!path.endsWith('.md')) path += '.md'
    if (invalidPath(path)) return flash('Invalid name: do not use "..", a leading ".", empty path segments or # ^ [ ] |')
    if (allPaths(tree).has(path)) return openPath(path)
    const title = path.split('/').pop()!.replace(/\.md$/, '')
    const content = reader ? `---\nsource: "[[${reader.link}]]"\n---\n\n# ${title}\n\n` : `# ${title}\n\n`
    try {
      await createNote(path, content)
    } catch (e) {
      // Already exists (Neovim/Obsidian created it before the tree updated): open the existing one.
      if (!(e instanceof ExistsError)) return flash((e as Error).message)
    }
    try {
      tree = await getTree()
    } catch (e) {
      flash((e as Error).message)
    }
    openPath(path)
  }

  /**
   * Rejects ., .., hidden names and # ^ [ ] | (Obsidian rejects them: they would break
   * links). The server rejects them too; here the message is clearer.
   */
  function invalidPath(path: string): boolean {
    // # ^ [ ] | only in the new name (last segment): old folders with them stay usable.
    return path.split('/').some((part) => part === '' || part.startsWith('.')) || /[#^[\]|]/.test(path.split('/').pop()!)
  }

  /** parent: folder chosen in the tree; without it, creates from the vault root. */
  async function newFolder(parent?: string) {
    const name = (await ask(parent ? `New subfolder in "${parent}":` : 'New folder:', '', parent ? '' : 'Path relative to the vault.'))
      ?.trim()
      .replace(/^\/+|\/+$/g, '')
    if (!name) return
    if (invalidPath(name)) return flash('Invalid name: do not use "..", a leading ".", empty path segments or # ^ [ ] |')
    try {
      const { path } = await createDir(parent ? `${parent}/${name}` : name)
      tree = await getTree()
      flash(`Folder created: ${path}`)
    } catch (e) {
      flash((e as Error).message)
    }
  }

  function insertPageLink() {
    if (!reader || !editor) return
    if (reader.kind === 'epub') {
      // Only this book's position (right after switching, the previous one may still be in epubLoc).
      if (epubLoc?.cfi && epubLoc.id === reader.id) editor.insert(cfiLink(reader.link, epubLoc.cfi) + ' ')
    } else editor.insert(pageLink(reader.link, page) + ' ')
  }

  // Shortcuts in the capture phase: they arrive before the editor, whose vim mode uses Ctrl+E (scroll)
  // and Ctrl+O (jump list back).
  function onKeyCapture(e: KeyboardEvent) {
    if (dialog) return
    if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === 'o' && note) {
      e.preventDefault()
      e.stopPropagation()
      switcherOpen = searchOpen = false
      outlineOpen = !outlineOpen
      return
    }
    if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === 'f') {
      e.preventDefault()
      e.stopPropagation()
      switcherOpen = false
      // Closing through the panel keeps the term and returns focus to the editor.
      if (searchOpen) searchPanel?.close()
      else searchOpen = true
      return
    }
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && /^[op]$/i.test(e.key)) {
      // With the search open, Ctrl+P moves up the list (handled there) and only Ctrl+O closes it.
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
    // Outside the editor (which handles them and sets defaultPrevented), the same PDF shortcuts.
    if (!e.defaultPrevented && e.altKey && !e.ctrlKey && !e.metaKey && activeReader && readerOpen) {
      const dir = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0
      if (dir) {
        e.preventDefault()
        if (e.shiftKey) activeReader.turnPage(dir)
        else activeReader.scroll(dir)
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
      // Another note changed: a link to the open note may have appeared or disappeared.
      // (The note's own autosave does not change its backlinks.)
      if (/\.md$/i.test(ev.path) && ev.path !== note) loadBacklinks(800)
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
    input={dialog.input}
    okLabel={dialog.okLabel}
    danger={dialog.danger}
    onsubmit={(v) => closeDialog(v)}
    oncancel={() => closeDialog(null)}
  />
{/if}

{#if menu}
  <ContextMenu x={menu.x} y={menu.y} items={menu.items} onclose={() => (menu = null)} />
{/if}

{#if backlinksOpen && note && backlinks}
  <BacklinksPanel
    {note}
    results={backlinks}
    onopen={(p, line) => openAtLine(p, line)}
    onclose={() => {
      backlinksOpen = false
      setTimeout(() => editor?.focus())
    }}
  />
{/if}

{#if outlineOpen && note}
  <Outline
    text={noteText}
    onjump={(line) => note && openAtLine(note, line)}
    onclose={() => {
      outlineOpen = false
      setTimeout(() => editor?.focus())
    }}
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

<input type="file" accept="application/pdf,.pdf,application/epub+zip,.epub" hidden bind:this={fileInput} onchange={openLocal} />

<div class="app" class:dragging style:--sidebar-w="{sidebarW}px" style:--notes-w="{notesPct}%">
  {#if sidebarOpen}
    <aside class="sidebar">
      <div class="sidebar-head">
        <input placeholder="Filter…" title="Filter the tree by name · Ctrl+O quick switcher · Ctrl+Shift+F full-text search" bind:value={filter} />
        <button onclick={() => newNote()} title="New note" aria-label="New note"><FilePlus size={16} /></button>
        <button onclick={() => newFolder()} title="New folder" aria-label="New folder"><FolderPlus size={16} /></button>
        <button onclick={() => fileInput.click()} title="Open a PDF or EPUB from this device" aria-label="Open PDF or EPUB"><FolderOpen size={16} /></button>
      </div>
      <div class="tree">
        <Tree entries={visibleTree} active={[note, doc]} onopen={(e) => openPath(e.path)} onnew={newNote} onnewdir={newFolder} oncontext={openMenu} expandAll={!!filter} />
      </div>
    </aside>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="resizer"
      title="Drag to resize · double-click to reset"
      onpointerdown={(e) => startDrag('sidebar', e)}
      onpointermove={onDrag}
      onpointerup={endDrag}
      onpointercancel={endDrag}
      ondblclick={() => resetSize('sidebar')}
    ></div>
  {/if}

  <main class="workspace">
    <div class="topbar">
      <button onclick={() => (sidebarOpen = !sidebarOpen)} title="Files (Ctrl+Shift+\)" aria-label="Files"><PanelLeft size={16} /></button>
      <button onclick={() => (searchOpen = true)} title="Search note contents (Ctrl+Shift+F)" aria-label="Search"><Search size={16} /></button>
      <span class="crumb">{note ?? ''}</span>
      {#if note && backlinks}
        <button onclick={() => (backlinksOpen = true)} title="Notes linking here (backlinks)" disabled={!backlinks.length}>
          <CornerUpLeft size={16} /> {new Set(backlinks.map((b) => b.path)).size}
        </button>
      {/if}
      {#if note}
        <button onclick={togglePreview} title="Toggle preview / editing (Ctrl+E)" class:active={preview}>
          {#if preview}<Pencil size={16} /> Edit{:else}<Eye size={16} /> Preview{/if}
        </button>
      {/if}
      {#if reader && note && notesOpen}
        <button onclick={insertPageLink} title="Insert a link to the current position (Ctrl+Shift+L)">
          <Link size={16} /> {reader.kind === 'epub'
            ? epubLoc?.id === reader.id
              ? Math.round(epubLoc.fraction * 100) + '%'
              : '…'
            : 'p.' + page}
        </button>
      {/if}
      {#if !sidebarOpen}
        <button onclick={() => fileInput.click()} title="Open a PDF or EPUB from this device" aria-label="Open PDF or EPUB"><FolderOpen size={16} /></button>
      {/if}
      {#if reader}
        <button onclick={toggleReader} title="Collapse/show the reader without closing it (Alt+\)">
          {#if readerOpen}<PanelLeftClose size={16} /> Collapse reader{:else}Reader <PanelLeftOpen size={16} />{/if}
        </button>
        <button onclick={closeReader} title="Close reader" aria-label="Close reader"><X size={16} /></button>
      {/if}
      <button
        onclick={() => (theme = theme === 'light' ? 'dark' : 'light')}
        title={theme === 'light' ? 'Dark theme' : 'Light theme (paper)'}
        aria-label="Toggle theme">{#if theme === 'light'}<Moon size={16} />{:else}<Sun size={16} />{/if}</button
      >
      <button onclick={toggleNotes} title="Notes (Ctrl+\)">
        {#if notesOpen}Collapse notes <PanelRightClose size={16} />{:else}<PanelRightOpen size={16} /> Notes{/if}
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
          {#if reader.kind === 'epub'}
            {#key reader.id}
              <EpubReader
                bind:this={epub}
                src={reader.file ?? reader.src}
                id={reader.id}
                title={reader.title}
                {theme}
                target={epubTarget}
                bind:location={epubLoc}
              />
            {/key}
          {:else}
            <PdfReader bind:this={pdf} src={reader.src} id={reader.id} title={reader.title} bind:page />
          {/if}
        </section>
        {#if readerOpen && notesOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="resizer"
            title="Drag to resize · double-click to reset"
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
              onimage={pasteImage}
            />
          </div>
          {#if preview}
            <Preview
              text={noteText}
              fileUrl={previewFileUrl}
              exists={(t) => !!resolveLink(tree, t)}
              onlink={openLink}
              ontoggle={(line) => editor?.toggleTask(line) || flash('Could not toggle this task from the preview; edit it in the editor.')}
            />
          {/if}
        {:else}
          <div class="empty">
            <p>Open a note from the sidebar or create one with <kbd>+</kbd>. To read a PDF or EPUB from this device (outside the vault), use the open folder button next to <kbd>+</kbd>.</p>
            <p class="muted">
              <kbd>Ctrl+O</kbd> quick switcher · <kbd>Ctrl+Shift+F</kbd> full-text search · <kbd>Ctrl+Shift+O</kbd> outline · <kbd>Ctrl+E</kbd> preview ·  <kbd>Ctrl+\</kbd> collapse notes · <kbd>Alt+\</kbd> collapse PDF · <kbd>Ctrl+Shift+\</kbd> sidebar · <kbd>Ctrl+Shift+L</kbd> insert page link ·
              <kbd>Alt+↑/↓</kbd> scroll the PDF · <kbd>Alt+Shift+↑/↓</kbd> change page · <kbd>Ctrl+click</kbd> or <kbd>Ctrl+Enter</kbd> follows a <code>[[link]]</code> · <kbd>:w</kbd> save
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
