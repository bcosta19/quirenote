import MarkdownIt, { type MarkdownIt as MD, type StateCore, type StateInline } from 'markdown-it'
import DOMPurify from 'dompurify'

// Renderização do preview, com as extensões do Obsidian que mais aparecem nas notas:
// [[links]], ![[imagens]], listas de tarefas e frontmatter.

export interface RenderContext {
  /** Resolve um alvo do vault ([[alvo]] ou caminho relativo) para uma URL de arquivo. */
  fileUrl: (target: string) => string
}

const IMAGE = /\.(png|jpe?g|gif|webp|svg|avif|bmp)$/i
const escapeHtml = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!)

function wikilinks(md: MD, ctx: RenderContext) {
  md.inline.ruler.before('link', 'wikilink', (state: StateInline, silent: boolean) => {
    const { src, pos } = state
    const embed = src.startsWith('![[', pos)
    const start = pos + (embed ? 3 : 2)
    if (!embed && !src.startsWith('[[', pos)) return false
    const end = src.indexOf(']]', start)
    if (end < 0) return false
    if (!silent) {
      const raw = src.slice(start, end)
      const [ref, alias] = raw.split('|')
      const target = ref.split('#')[0].trim()
      const page = ref.match(/#page=(\d+)/)?.[1]
      if (embed && IMAGE.test(target)) {
        // ![[foto.png|300]]: o número após | é a largura, como no Obsidian.
        const width = /^\d+$/.test(alias ?? '') ? ` width="${alias}"` : ''
        const t = state.push('html_inline', '', 0)
        t.content = `<img src="${escapeHtml(ctx.fileUrl(target))}" alt="${escapeHtml(target)}"${width}>`
      } else {
        const open = state.push('link_open', 'a', 1)
        open.attrs = [
          ['href', '#'],
          ['class', embed ? 'wikilink embed' : 'wikilink'],
          ['data-target', target],
        ]
        if (page) open.attrs.push(['data-page', page])
        const text = state.push('text', '', 0)
        text.content = (alias ?? (page ? `${target} · p.${page}` : ref)).trim()
        state.push('link_close', 'a', -1)
      }
    }
    state.pos = end + 2
    return true
  })
}

// "- [ ] tarefa" / "- [x] feita" viram caixas de seleção (só leitura).
function taskLists(md: MD) {
  md.core.ruler.push('task_lists', (state: StateCore) => {
    const tokens = state.tokens
    for (let i = 2; i < tokens.length; i++) {
      const inline = tokens[i]
      if (inline.type !== 'inline' || tokens[i - 1].type !== 'paragraph_open' || tokens[i - 2].type !== 'list_item_open')
        continue
      const m = inline.content.match(/^\[([ xX])\]\s+/)
      const first = inline.children?.[0]
      if (!m || !first || first.type !== 'text') continue
      first.content = first.content.slice(m[0].length)
      const box = new state.Token('html_inline', '', 0)
      box.content = `<input type="checkbox" disabled${m[1] === ' ' ? '' : ' checked'}> `
      inline.children!.unshift(box)
      tokens[i - 2].attrJoin('class', 'task')
    }
  })
}

function relativeImages(md: MD, ctx: RenderContext) {
  const image = md.renderer.rules.image!
  md.renderer.rules.image = (tokens, idx, options, env, self) => {
    const src = String(tokens[idx].attrGet('src') ?? '')
    if (src && !/^(https?:|data:|blob:)/.test(src)) tokens[idx].attrSet('src', ctx.fileUrl(decodeURI(src)))
    return image(tokens, idx, options, env, self)
  }
}

/** Frontmatter simples (chave: valor) exibido como as "propriedades" do Obsidian. */
function splitFrontmatter(text: string): { props: [string, string][]; body: string } {
  const m = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/)
  if (!m) return { props: [], body: text }
  const props: [string, string][] = []
  for (const line of m[1].split(/\r?\n/)) {
    const kv = line.match(/^([\w-]+):\s*(.*)$/)
    if (kv) props.push([kv[1], kv[2].replace(/^["']|["']$/g, '')])
  }
  return { props, body: text.slice(m[0].length) }
}

export function createRenderer(ctx: RenderContext) {
  const md = new MarkdownIt({ html: true, linkify: true, breaks: true })
  md.use(wikilinks, ctx).use(taskLists).use(relativeImages, ctx)

  return (text: string): string => {
    const { props, body } = splitFrontmatter(text)
    let html = ''
    if (props.length) {
      html += '<dl class="props">'
      for (const [k, v] of props) html += `<dt>${escapeHtml(k)}</dt><dd>${md.renderInline(v)}</dd>`
      html += '</dl>'
    }
    html += md.render(body)
    // As notas podem ter HTML; sanitiza para nada executar script na página.
    return DOMPurify.sanitize(html)
  }
}
