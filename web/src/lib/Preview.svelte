<script lang="ts">
  import { createRenderer } from './markdown'

  let {
    text,
    fileUrl,
    exists,
    onlink,
  }: {
    text: string
    fileUrl: (target: string) => string
    exists: (target: string) => boolean
    onlink: (target: string, page?: number) => void
  } = $props()

  const render = $derived(createRenderer({ fileUrl, exists }))
  const html = $derived(render(text))

  function onclick(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest('a')
    if (!a) return
    e.preventDefault()
    const target = a.dataset.target
    const href = a.getAttribute('href') ?? ''
    if (target) {
      onlink(target, a.dataset.page ? Number(a.dataset.page) : undefined)
    } else if (/^(https?:|mailto:)/.test(href)) {
      window.open(href, '_blank', 'noopener')
    } else if (href && !href.startsWith('#')) {
      // Link markdown para outra nota/arquivo do vault: [texto](pasta/nota.md)
      onlink(decodeURI(href))
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="preview" {onclick}>
  <article>{@html html}</article>
</div>

<style>
  .preview {
    height: 100%;
    overflow: auto;
    background: var(--page-bg);
  }
  article {
    max-width: 44em;
    margin: 0 auto;
    padding: 24px 28px 40vh;
    font-family: var(--editor-font);
    font-size: 19px;
    line-height: 1.6;
    color: var(--text);
    overflow-wrap: anywhere;
  }
  article :global(h1),
  article :global(h2),
  article :global(h3),
  article :global(h4) {
    color: var(--heading);
    line-height: 1.25;
    margin: 1.2em 0 0.4em;
  }
  article :global(h1) {
    font-size: 1.8em;
  }
  article :global(h2) {
    font-size: 1.45em;
  }
  article :global(h3) {
    font-size: 1.2em;
  }
  article :global(p) {
    margin: 0 0 0.8em;
  }
  article :global(a) {
    color: var(--link);
    text-decoration: none;
    border-bottom: 1px solid color-mix(in srgb, var(--link) 35%, transparent);
    cursor: pointer;
  }
  /* Link para nota que ainda não existe (clicar oferece criar), como no Obsidian. */
  article :global(a.missing) {
    opacity: 0.6;
    border-bottom-style: dashed;
  }
  article :global(a.embed) {
    font-style: italic;
  }
  article :global(blockquote) {
    margin: 0 0 0.8em;
    padding: 0.1em 1em;
    border-left: 3px solid var(--border);
    color: var(--muted);
    font-style: italic;
  }
  article :global(code) {
    font-family: var(--mono);
    font-size: 0.8em;
    background: var(--code-bg);
    padding: 0.1em 0.3em;
    border-radius: 3px;
  }
  article :global(pre) {
    background: var(--code-bg);
    padding: 0.8em 1em;
    border-radius: 6px;
    overflow: auto;
  }
  article :global(pre code) {
    background: none;
    padding: 0;
  }
  article :global(ul),
  article :global(ol) {
    padding-left: 1.4em;
    margin: 0 0 0.8em;
  }
  article :global(li.task) {
    list-style: none;
    margin-left: -1.2em;
  }
  article :global(li > p) {
    margin: 0;
  }
  article :global(img) {
    max-width: 100%;
    border-radius: 4px;
  }
  article :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 1.5em 0;
  }
  article :global(table) {
    border-collapse: collapse;
    margin: 0 0 0.8em;
    font-size: 0.9em;
  }
  article :global(th),
  article :global(td) {
    border: 1px solid var(--border);
    padding: 0.3em 0.6em;
  }
  article :global(.props) {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px 14px;
    margin: 0 0 1.4em;
    padding: 0.6em 0.9em;
    font-size: 0.8em;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--muted);
  }
  article :global(.props dt) {
    font-family: var(--mono);
    font-size: 0.85em;
    padding-top: 0.2em;
  }
  article :global(.props dd) {
    margin: 0;
  }
</style>
