<script lang="ts" module>
  export interface EpubLocation {
    id: string // which book this position belongs to
    cfi: string
    fraction: number // 0–1 of the whole book
    chapter: string
  }
</script>

<script lang="ts">
  import ChevronLeft from '@lucide/svelte/icons/chevron-left'
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import { onMount, untrack } from 'svelte'
  import type { Theme } from './theme'

  // EPUB reader with foliate-js (the engine of the Foliate reader), in the same panel as the PDF.

  let {
    src,
    id,
    title,
    theme = 'light',
    target = null,
    location = $bindable(null),
  }: {
    src: string | File // vault URL or local file (no upload)
    id: string // identifies the book to remember the last position
    title: string
    theme?: Theme
    /** Request to go to a position ([[book.epub#cfi=…]] link); n changes on every request.
     *  Only applies if id is this book's (a stale request for another book is ignored). */
    target?: { id: string; cfi: string; n: number } | null
    location?: EpubLocation | null
  } = $props()

  let host: HTMLDivElement
  let view: any = $state(null)
  let toc = $state<{ label: string; href: string; depth: number }[]>([])
  let error = $state('')
  let ready = $state(false)

  const key = () => `quirenote:epub:${id}`
  const NOT_FOUND = 'link position not found in this book'

  /** Does the position exist in this book? (resolveNavigation accepts missing chapters.) */
  function valid(el: any, cfi: string | null | undefined): boolean {
    if (!cfi) return false
    try {
      const r = el.resolveNavigation(cfi)
      return !!r && r.index >= 0 && r.index < el.book.sections.length
    } catch {
      return false
    }
  }

  // Theme colors inside the book (the EPUB runs in its own iframe).
  function css(t: Theme) {
    const [bg, fg, link] = t === 'light' ? ['#f2eadb', '#332e26', '#4b6a88'] : ['#282c34', '#dcdde0', '#61afef']
    return `
      html { color-scheme: ${t}; }
      html, body { background: ${bg} !important; color: ${fg} !important; }
      body * { color: inherit; background-color: transparent; }
      a:link, a:visited { color: ${link} !important; }
      p, li, blockquote, dd { line-height: 1.6; hyphens: auto; }
      pre { white-space: pre-wrap !important; }
      img, svg { max-width: 100%; height: auto; }
    `
  }

  function flattenToc(items: any[] = [], depth = 0, out: { label: string; href: string; depth: number }[] = []) {
    for (const it of items) {
      if (it.href) out.push({ label: String(it.label ?? '').trim(), href: it.href, depth })
      if (it.subitems?.length) flattenToc(it.subitems, depth + 1, out)
    }
    return out
  }

  // User-initiated navigation: the previous error message no longer applies.
  function next() {
    error = ''
    view?.next()
  }
  function prev() {
    error = ''
    view?.prev()
  }

  // Continuous scrolling, like the PDF: the mouse wheel scrolls within a chapter natively.
  // At the end (or start) of a chapter, a bit more wheel in the same direction moves to the
  // next (or previous) one. The threshold keeps trackpad inertia from skipping chapters.
  const EDGE_PUSH = 240
  let push = 0
  let turning = false
  function onWheel(e: WheelEvent) {
    const r = view?.renderer
    if (!r || turning || e.ctrlKey || !e.deltaY) return
    const atEnd = e.deltaY > 0 && r.viewSize - r.end <= 2
    const atStart = e.deltaY < 0 && r.start <= 0
    if (!atEnd && !atStart) {
      push = 0
      return
    }
    push += Math.abs(e.deltaY)
    if (push < EDGE_PUSH) return
    push = 0
    turning = true
    Promise.resolve(atEnd ? view.next() : view.prev()).finally(() => setTimeout(() => (turning = false), 300))
  }

  function onKey(e: KeyboardEvent) {
    if (!view || e.ctrlKey || e.metaKey || e.altKey) return
    if (e.key === 'ArrowRight' || e.key === 'PageDown' || e.key === ' ') next()
    else if (e.key === 'ArrowLeft' || e.key === 'PageUp') prev()
    else return
    e.preventDefault()
  }

  onMount(() => {
    let disposed = false
    let el: any
    ;(async () => {
      try {
        await import('foliate-js/view.js')
        if (disposed) return
        el = document.createElement('foliate-view')
        el.style.cssText = 'display:block;width:100%;height:100%'
        host.append(el)
        await el.open(src)
        if (disposed) return
        const r = el.renderer
        r.setAttribute('flow', 'scrolled')
        r.setAttribute('margin', '36px')
        r.setAttribute('gap', '6%')
        r.setAttribute('max-inline-size', '760px')
        r.setStyles?.(css(theme))
        toc = flattenToc(el.book.toc)
        el.addEventListener('relocate', (e: CustomEvent) => {
          const d = e.detail
          location = { id, cfi: d.cfi, fraction: d.fraction ?? 0, chapter: d.tocItem?.label?.trim?.() ?? '' }
          if (d.cfi) localStorage.setItem(key(), d.cfi)
        })
        // Arrow keys inside the book also turn the page.
        el.addEventListener('load', (e: CustomEvent) => {
          e.detail.doc.addEventListener('keydown', onKey)
          e.detail.doc.addEventListener('wheel', onWheel, { passive: true })
        })
        // External links in the book: only http(s)/mailto and without window.opener (like the preview).
        el.addEventListener('external-link', (e: CustomEvent) => {
          e.preventDefault()
          const href = String(e.detail?.href ?? '')
          if (/^(https?:|mailto:)/i.test(href)) window.open(href, '_blank', 'noopener')
        })
        // Read before the await: a request arriving during init is left to the $effect.
        const t0 = target?.id === id ? target : null
        lastTarget = t0?.n ?? 0
        // Invalid link target: open at the saved position and warn (instead of staying blank).
        const saved = localStorage.getItem(key())
        const start = valid(el, t0?.cfi) ? t0!.cfi : valid(el, saved) ? saved : null
        await el.init({ lastLocation: start, showTextStart: true })
        if (t0 && start !== t0.cfi) error = NOT_FOUND
        view = el
        ready = true
      } catch (e) {
        error = (e as Error).message || 'could not open the EPUB'
      }
    })()
    return () => {
      disposed = true
      el?.close?.()
      el?.remove()
    }
  })

  // Theme changed with the book open.
  $effect(() => {
    const t = theme
    untrack(() => view?.renderer?.setStyles?.(css(t)))
  })

  // A position link clicked with the book already open.
  let lastTarget = 0
  $effect(() => {
    const t = target
    if (t && ready && t.id === id && t.n !== lastTarget) {
      lastTarget = t.n
      untrack(() => {
        error = ''
        if (!valid(view, t.cfi)) error = NOT_FOUND
        else view.goTo(t.cfi).catch(() => (error = NOT_FOUND))
      })
    }
  })

  /** Same interface as the PDF reader (Alt+↑/↓ shortcuts while the editor has focus). */
  export function scroll(dir: 1 | -1) {
    if (dir > 0) next()
    else prev()
  }
  export function turnPage(dir: 1 | -1) {
    scroll(dir)
  }

  function goToc(e: Event) {
    const href = (e.target as HTMLSelectElement).value
    error = ''
    if (href) view?.goTo(href)
  }
</script>

<div class="reader">
  <div class="toolbar">
    <span class="title" {title}>{title.split('/').pop()}</span>
    {#if toc.length}
      <select onchange={goToc} aria-label="Chapters" value="">
        <option value="" disabled>{location?.chapter || 'Chapters'}</option>
        {#each toc as item (item.href)}
          <option value={item.href}>{'  '.repeat(item.depth)}{item.label}</option>
        {/each}
      </select>
    {/if}
    <button onclick={prev} title="Previous page (←)" aria-label="Previous page"><ChevronLeft size={16} /></button>
    <span class="muted">{location ? Math.round(location.fraction * 100) + '%' : '…'}</span>
    <button onclick={next} title="Next page (→)" aria-label="Next page"><ChevronRight size={16} /></button>
  </div>
  {#if error}<div class="error">EPUB error: {error}</div>{/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="viewport" bind:this={host} onkeydown={onKey} onwheel={onWheel}></div>
</div>

<style>
  .reader {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
  }
  .title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  select {
    max-width: 40%;
    font: inherit;
    font-size: 12px;
    color: inherit;
    background: var(--hover);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 4px;
  }
  .viewport {
    flex: 1;
    min-height: 0;
    background: var(--page-bg);
  }
  .error {
    padding: 10px;
    color: var(--error);
  }
</style>
