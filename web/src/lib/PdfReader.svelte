<script lang="ts">
  import Minus from '@lucide/svelte/icons/minus'
  import Plus from '@lucide/svelte/icons/plus'
  import MoveHorizontal from '@lucide/svelte/icons/move-horizontal'
  import { onMount, untrack } from 'svelte'
  import { loadViewer, pdfjsLib } from './pdf'

  let {
    src,
    id,
    title,
    page = $bindable(1),
  }: {
    src: string // PDF URL: /api/files/... (vault) or blob: (local file)
    id: string // identifies the PDF to remember the last page read
    title: string
    page?: number
  } = $props()

  let container: HTMLDivElement
  let viewer: any = $state(null)
  let total = $state(0)
  let error = $state('')
  let loadingTask: any = null

  const pageKey = (p: string) => `quirenote:page:${p}`

  async function open(url: string) {
    if (!viewer) return
    error = ''
    total = 0
    loadingTask?.destroy()
    loadingTask = pdfjsLib.getDocument({ url })
    try {
      const doc = await loadingTask.promise
      if (url !== src) return
      viewer.setDocument(doc)
      total = doc.numPages
    } catch (e) {
      if ((e as Error).name !== 'AbortException') error = (e as Error).message
    }
  }

  onMount(() => {
    let ro: ResizeObserver | undefined
    loadViewer().then(({ EventBus, PDFViewer, PDFLinkService }) => {
      const eventBus = new EventBus()
      const linkService = new PDFLinkService({ eventBus })
      const v = new PDFViewer({ container, eventBus, linkService })
      linkService.setViewer(v)

      eventBus.on('pagesinit', () => {
        v.currentScaleValue = 'page-width'
        // Page requested by the link, otherwise the last one read in this PDF.
        const saved = Number(localStorage.getItem(pageKey(id)) || 1)
        const target = page > 1 ? page : saved
        if (target > 1) v.currentPageNumber = target
      })
      eventBus.on('pagechanging', (e: { pageNumber: number }) => {
        page = e.pageNumber
        localStorage.setItem(pageKey(id), String(e.pageNumber))
      })

      // Refits to the width when the panel is resized. Debounced, so the PDF is not
      // re-rendered on every pixel while the divider is dragged.
      let hiddenPage = 0
      let timer: ReturnType<typeof setTimeout> | undefined
      ro = new ResizeObserver(() => {
        if (container.clientWidth === 0) {
          // Collapsed: keep the page to come back to exactly the same one.
          hiddenPage ||= page
          return
        }
        const restore = hiddenPage
        hiddenPage = 0
        clearTimeout(timer)
        timer = setTimeout(
          () => {
            if (v.currentScaleValue === 'page-width') v.currentScaleValue = 'page-width'
            if (restore && v.pagesCount) v.currentPageNumber = restore
          },
          restore ? 0 : 120,
        )
      })
      ro.observe(container)
      viewer = v
    }).catch(() => {
      // E.g. the Vite dev server updated its dependencies and this tab is now stale.
      error = 'could not load the reader — reload the page (Ctrl+Shift+R)'
    })
    return () => {
      ro?.disconnect()
      loadingTask?.destroy()
    }
  })

  $effect(() => {
    const url = src
    if (viewer) untrack(() => open(url))
  })

  // A [[book.pdf#page=N]] link clicked with the PDF already open.
  $effect(() => {
    const target = page
    if (viewer && total && target !== viewer.currentPageNumber) {
      viewer.currentPageNumber = Math.min(Math.max(target, 1), total)
    }
  })

  const SCROLL_STEP = 60

  /** Scrolls the PDF (called by the shortcuts while the editor has focus). */
  export function scroll(dir: 1 | -1) {
    container?.scrollBy({ top: dir * SCROLL_STEP })
  }

  export function turnPage(dir: 1 | -1) {
    if (total) page = Math.min(Math.max(page + dir, 1), total)
  }

  function zoom(factor: number) {
    if (viewer) viewer.currentScale = viewer.currentScale * factor
  }

  function goTo(e: Event) {
    const n = Number((e.target as HTMLInputElement).value)
    if (n >= 1 && n <= total) page = n
  }
</script>

<div class="reader">
  <div class="toolbar">
    <span class="title" {title}>{title.split('/').pop()}</span>
    <input type="number" min="1" max={total} value={page} onchange={goTo} aria-label="Page" />
    <span class="muted">/ {total || '…'}</span>
    <button onclick={() => zoom(1 / 1.15)} title="Zoom out" aria-label="Zoom out"><Minus size={15} /></button>
    <button onclick={() => zoom(1.15)} title="Zoom in" aria-label="Zoom in"><Plus size={15} /></button>
    <button onclick={() => viewer && (viewer.currentScaleValue = 'page-width')} title="Fit to width" aria-label="Fit to width"><MoveHorizontal size={15} /></button>
  </div>
  {#if error}<div class="error">Could not open the PDF: {error}</div>{/if}
  <div class="viewport">
    <div class="pdf-container" bind:this={container}>
      <div class="pdfViewer"></div>
    </div>
  </div>
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
  input {
    width: 4em;
  }
  .viewport {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  /* PDFViewer requires a container with position: absolute. */
  .pdf-container {
    position: absolute;
    inset: 0;
    overflow: auto;
    background: var(--reader-bg);
  }
  .error {
    padding: 10px;
    color: var(--error);
  }
</style>
