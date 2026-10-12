<script lang="ts" module>
  export interface EpubLocation {
    id: string // de qual livro é esta posição
    cfi: string
    fraction: number // 0–1 do livro inteiro
    chapter: string
  }
</script>

<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import type { Theme } from './theme'

  // Leitor de EPUB com foliate-js (o motor do leitor Foliate), no mesmo painel do PDF.

  let {
    src,
    id,
    title,
    theme = 'light',
    target = null,
    location = $bindable(null),
  }: {
    src: string | File // URL do vault ou arquivo local (sem upload)
    id: string // identifica o livro para lembrar a última posição
    title: string
    theme?: Theme
    /** Pedido de ir para uma posição (link [[livro.epub#cfi=…]]); n muda a cada pedido.
     *  Só vale se id for o deste livro (um pedido antigo de outro livro é ignorado). */
    target?: { id: string; cfi: string; n: number } | null
    location?: EpubLocation | null
  } = $props()

  let host: HTMLDivElement
  let view: any = $state(null)
  let toc = $state<{ label: string; href: string; depth: number }[]>([])
  let error = $state('')
  let ready = $state(false)

  const key = () => `notas:epub:${id}`
  const NOT_FOUND = 'posição do link não encontrada neste livro'

  /** A posição existe neste livro? (resolveNavigation aceita capítulos inexistentes.) */
  function valid(el: any, cfi: string | null | undefined): boolean {
    if (!cfi) return false
    try {
      const r = el.resolveNavigation(cfi)
      return !!r && r.index >= 0 && r.index < el.book.sections.length
    } catch {
      return false
    }
  }

  // Cores do tema dentro do livro (o EPUB roda num iframe próprio).
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

  // Navegação pedida pelo usuário: a mensagem de erro anterior deixa de valer.
  function next() {
    error = ''
    view?.next()
  }
  function prev() {
    error = ''
    view?.prev()
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
        r.setAttribute('flow', 'paginated')
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
        // Setas dentro do livro também viram a página.
        el.addEventListener('load', (e: CustomEvent) => e.detail.doc.addEventListener('keydown', onKey))
        // Links externos do livro: só http(s)/mailto e sem window.opener (como no preview).
        el.addEventListener('external-link', (e: CustomEvent) => {
          e.preventDefault()
          const href = String(e.detail?.href ?? '')
          if (/^(https?:|mailto:)/i.test(href)) window.open(href, '_blank', 'noopener')
        })
        // Lido antes do await: um pedido que chegue durante o init fica para o $effect.
        const t0 = target?.id === id ? target : null
        lastTarget = t0?.n ?? 0
        // Alvo do link inválido: abre na posição salva e avisa (em vez de ficar em branco).
        const saved = localStorage.getItem(key())
        const start = valid(el, t0?.cfi) ? t0!.cfi : valid(el, saved) ? saved : null
        await el.init({ lastLocation: start, showTextStart: true })
        if (t0 && start !== t0.cfi) error = NOT_FOUND
        view = el
        ready = true
      } catch (e) {
        error = (e as Error).message || 'não foi possível abrir o EPUB'
      }
    })()
    return () => {
      disposed = true
      el?.close?.()
      el?.remove()
    }
  })

  // Tema trocado com o livro aberto.
  $effect(() => {
    const t = theme
    untrack(() => view?.renderer?.setStyles?.(css(t)))
  })

  // Link para uma posição clicado com o livro já aberto.
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

  /** Mesma interface do leitor de PDF (atalhos Alt+↑/↓ com o foco no editor). */
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
      <select onchange={goToc} aria-label="Capítulos" value="">
        <option value="" disabled>{location?.chapter || 'Capítulos'}</option>
        {#each toc as item (item.href)}
          <option value={item.href}>{'  '.repeat(item.depth)}{item.label}</option>
        {/each}
      </select>
    {/if}
    <button onclick={prev} title="Página anterior (←)">‹</button>
    <span class="muted">{location ? Math.round(location.fraction * 100) + '%' : '…'}</span>
    <button onclick={next} title="Próxima página (→)">›</button>
  </div>
  {#if error}<div class="error">Erro no EPUB: {error}</div>{/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="viewport" bind:this={host} onkeydown={onKey}></div>
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
