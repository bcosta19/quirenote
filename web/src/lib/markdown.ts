import MarkdownIt, { type MarkdownIt as MD, type StateCore, type StateInline } from 'markdown-it'
import DOMPurify from 'dompurify'

// Renderização do preview, com as extensões do Obsidian que mais aparecem nas notas:
// [[links]], ![[imagens]], listas de tarefas e frontmatter.

export interface RenderContext {
  /** Resolve um alvo do vault ([[alvo]] ou caminho relativo) para uma URL de arquivo. */
  fileUrl: (target: string) => string
  /** Se o alvo de um [[link]] existe no vault (links inexistentes ficam esmaecidos). */
  exists?: (target: string) => boolean
}

export const IMAGE = /\.(png|jpe?g|gif|webp|svg|avif|bmp)$/i
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
          // [[#seção]] (alvo vazio) aponta para a própria nota: nunca é "inexistente".
          ['class', ['wikilink', embed && 'embed', target && ctx.exists && !ctx.exists(target) && 'missing'].filter(Boolean).join(' ')],
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

// "- [ ] tarefa" / "- [x] feita" viram caixas de seleção. data-line é a linha da
// tarefa na nota (1 = primeira), para o clique no preview alternar a linha certa.
function taskLists(md: MD) {
  md.core.ruler.push('task_lists', (state: StateCore) => {
    const offset = Number(state.env?.lineOffset ?? 0)
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
      const line = (tokens[i - 2].map?.[0] ?? -1) + offset + 1
      box.content = `<input type="checkbox" class="task-box" data-line="${line}"${m[1] === ' ' ? '' : ' checked'}> `
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
function splitFrontmatter(text: string): { props: [string, string][]; body: string; lines: number } {
  const m = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/)
  if (!m) return { props: [], body: text, lines: 0 }
  const props: [string, string][] = []
  for (const line of m[1].split(/\r?\n/)) {
    const kv = line.match(/^([\w-]+):\s*(.*)$/)
    if (kv) props.push([kv[1], kv[2].replace(/^["']|["']$/g, '')])
  }
  // lines: quantas linhas o frontmatter ocupa (o corpo começa depois delas).
  return { props, body: text.slice(m[0].length), lines: m[0].split('\n').length - 1 }
}

export interface Heading {
  level: number
  title: string
  line: number // 1 = primeira linha da nota
}

/** Títulos da nota pelo próprio parser (ATX, setext, recuados; ignora código e frontmatter). */
export function headings(text: string): Heading[] {
  const { body, lines } = splitFrontmatter(text)
  const tokens = new MarkdownIt().parse(body, {})
  const out: Heading[] = []
  tokens.forEach((t, i) => {
    if (t.type === 'heading_open' && t.map) {
      out.push({ level: Number(t.tag.slice(1)), title: tokens[i + 1]?.content ?? '', line: t.map[0] + lines + 1 })
    }
  })
  return out
}

export function createRenderer(ctx: RenderContext) {
  const md = new MarkdownIt({ html: true, linkify: true, breaks: true })
  md.use(wikilinks, ctx).use(taskLists).use(relativeImages, ctx)

  return (text: string): string => {
    const { props, body, lines } = splitFrontmatter(text)
    let html = ''
    if (props.length) {
      html += '<dl class="props">'
      for (const [k, v] of props) html += `<dt>${escapeHtml(k)}</dt><dd>${md.renderInline(v)}</dd>`
      html += '</dl>'
    }
    html += md.render(body, { lineOffset: lines })
    // As notas podem ter HTML; sanitiza para nada executar script na página.
    return DOMPurify.sanitize(html)
  }
}
