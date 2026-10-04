// The /intent draft editor: Milkdown (CommonMark + GitHub Markdown) with the
// syntax hidden. Built into src/notes/milkdown/editor.min.js by build.mjs and
// inlined into the draft page's one nonce-tagged script, which drives it
// through `window.OcgenDraft`.
//
// The file stays plain Markdown. What Save writes (`markdown()`):
// - nothing edited: the text the page was opened with, byte for byte;
// - otherwise: the original text of every top-level block that is unchanged,
//   and Milkdown's Markdown only for the blocks that were edited. The result
//   is re-parsed and checked against the editor before it is used; if the
//   check fails, the whole document is serialized by Milkdown instead.
//
// Nothing in a draft runs or loads: raw HTML is an atom shown as text,
// images are shown as their Markdown, links never navigate, and pasted HTML
// keeps only what the schema knows (no scripts, no event attributes).

import {
  Editor,
  defaultValueCtx,
  editorViewCtx,
  editorViewOptionsCtx,
  parserCtx,
  remarkCtx,
  remarkStringifyOptionsCtx,
  rootCtx,
  serializerCtx,
} from '@milkdown/core'
import {
  bulletListSchema,
  commonmark,
  hardbreakSchema,
  imageSchema,
  linkSchema,
  listItemSchema,
  remarkPreserveEmptyLinePlugin,
  sanitizeLinkHref,
  syncHeadingIdPlugin,
} from '@milkdown/preset-commonmark'
import { gfm } from '@milkdown/preset-gfm'
import { history } from '@milkdown/plugin-history'
import { clipboard } from '@milkdown/plugin-clipboard'
import { keymap } from '@milkdown/prose/keymap'
import { liftListItem, sinkListItem } from '@milkdown/prose/schema-list'
import { toggleMark } from '@milkdown/prose/commands'
import { Plugin, PluginKey } from '@milkdown/prose/state'
import { Decoration, DecorationSet } from '@milkdown/prose/view'
import { $prose, $remark, replaceAll } from '@milkdown/utils'
import { defaultHandlers } from 'mdast-util-to-markdown'
import { visit } from 'unist-util-visit'

const VERSION = '__MILKDOWN_VERSION__'

// --------------------------------------------------------------- schema --

const webUrl = (u) => /^https?:\/\//i.test(String(u || '').trim())

// Images are shown as their Markdown and never load. Pasted ones keep only
// web addresses (a pasted data: image would be a huge, useless string).
const imageAsText = imageSchema.extendSchema((prev) => (ctx) => {
  const base = prev(ctx)
  return {
    ...base,
    // Milkdown 7.22.2 passes a missing title as null, which its own schema
    // refuses: `![alt](src)` would fail to load.
    parseMarkdown: {
      match: ({ type }) => type === 'image',
      runner: (state, node, type) => {
        state.addNode(type, { src: node.url || '', alt: node.alt || '', title: node.title || '' })
      },
    },
    parseDOM: [
      {
        tag: 'img[src]',
        getAttrs: (dom) => {
          const src = dom.getAttribute('src') || ''
          if (!webUrl(src)) return false
          return { src, alt: dom.getAttribute('alt') || '', title: dom.getAttribute('title') || '' }
        },
      },
    ],
    toDOM: (node) => [
      'span',
      { 'data-type': 'image', class: 'md-atom md-image', title: node.attrs.src },
      `![${node.attrs.alt}](${node.attrs.src}${node.attrs.title ? ` "${node.attrs.title}"` : ''})`,
    ],
  }
})

// A line break is a line break, as GitHub shows it in an issue. Shift+Enter
// makes a plain newline (not `\` at the end of the line).
const lineBreak = hardbreakSchema.extendSchema((prev) => (ctx) => {
  const base = prev(ctx)
  return {
    ...base,
    attrs: { isInline: { default: true, validate: 'boolean' } },
    parseDOM: [{ tag: 'br', getAttrs: () => ({ isInline: true }) }],
    toDOM: (node) => ['br', { 'data-type': 'hardbreak', 'data-is-inline': String(node.attrs.isInline) }],
  }
})

// What the draft wrote that mdast forgets, read from the source so an edited
// block is written back the same way: a list's bullet (`*` or `-`), and a
// bare URL (not `<url>`). Runs after Milkdown's own remark plugins.
const sourceMarks = $remark('ocgenSourceMarks', () => () => (tree, file) => {
  const src = String(file.value)
  const at = (node) => (node && node.position ? src.charAt(node.position.start.offset) : '')
  visit(tree, (node) => {
    if (node.type === 'list' && !node.ordered) {
      const c = at(node.children && node.children[0])
      if ('-*+'.includes(c) && c) node.ocgenBullet = c
    } else if (node.type === 'link') {
      const c = at(node)
      if (c && c !== '<' && c !== '[') node.ocgenLiteral = true
    }
  })
})

const bulletMark = bulletListSchema.extendSchema((prev) => (ctx) => {
  const base = prev(ctx)
  return {
    ...base,
    attrs: { ...base.attrs, marker: { default: null, validate: 'string|null' } },
    parseMarkdown: {
      match: ({ type, ordered }) => type === 'list' && !ordered,
      runner: (state, node, type) => {
        state.openNode(type, { spread: node.spread ?? false, marker: node.ocgenBullet || null })
        state.next(node.children).closeNode()
      },
    },
    toMarkdown: {
      match: (node) => node.type.name === 'bullet_list',
      runner: (state, node) => {
        const props = { ordered: false, spread: node.attrs.spread }
        if (node.attrs.marker) props.ocgenBullet = node.attrs.marker
        state.openNode('list', undefined, props).next(node.content).closeNode()
      },
    },
  }
})

// remark-stringify handlers that write those back.
const handlers = {
  list: (node, parent, state, info) => {
    const was = state.options.bullet
    if (!node.ordered && node.ocgenBullet) state.options.bullet = node.ocgenBullet
    try {
      return defaultHandlers.list(node, parent, state, info)
    } finally {
      state.options.bullet = was
    }
  },
  link: (node, parent, state, info) => {
    const only = node.children && node.children.length === 1 && node.children[0]
    if (node.ocgenLiteral && only && only.type === 'text' && !node.title) {
      const t = only.value
      if (node.url === t || node.url === 'mailto:' + t || node.url === 'http://' + t) return t
    }
    return defaultHandlers.link(node, parent, state, info)
  },
}

// Pasted links keep only safe targets; the draft's own links stay as written.
const safeLink = linkSchema.extendSchema((prev) => (ctx) => {
  const base = prev(ctx)
  return {
    ...base,
    attrs: { ...base.attrs, literal: { default: false, validate: 'boolean' } },
    parseMarkdown: {
      match: (node) => node.type === 'link',
      runner: (state, node, markType) => {
        state.openMark(markType, { href: node.url, title: node.title ?? null, literal: Boolean(node.ocgenLiteral) })
        state.next(node.children)
        state.closeMark(markType)
      },
    },
    toMarkdown: {
      match: (mark) => mark.type.name === 'link',
      runner: (state, mark) => {
        const props = { title: mark.attrs.title, url: mark.attrs.href }
        if (mark.attrs.literal) props.ocgenLiteral = true
        state.withMark(mark, 'link', undefined, props)
      },
    },
    parseDOM: [
      {
        tag: 'a[href]',
        getAttrs: (dom) => {
          const href = sanitizeLinkHref(dom.getAttribute('href'))
          if (!href) return false
          return { href, title: dom.getAttribute('title') }
        },
      },
    ],
  }
})

// Heading ids are useless here and would mark untouched headings as edited.
const presetCommonmark = commonmark.filter(
  (p) => p !== remarkPreserveEmptyLinePlugin.plugin && p !== remarkPreserveEmptyLinePlugin.options &&
    ![].concat(syncHeadingIdPlugin).includes(p)
)

// ------------------------------------------------------------- plugins --

// Task items: a click on the box toggles it.
const taskToggle = $prose(
  () =>
    new Plugin({
      key: new PluginKey('ocgen-task-toggle'),
      props: {
        handleDOMEvents: {
          mousedown: (view, event) => {
            const li = event.target instanceof Element && event.target.closest('li[data-item-type="task"]')
            if (!li || !view.dom.contains(li)) return false
            const box = li.getBoundingClientRect()
            const x = event.clientX - box.left
            if (x > 0) return false // only the box, drawn left of the item
            const pos = view.posAtDOM(li, 0) - 1
            const node = view.state.doc.nodeAt(pos)
            if (!node || node.attrs.checked == null) return false
            event.preventDefault()
            view.dispatch(view.state.tr.setNodeMarkup(pos, undefined, { ...node.attrs, checked: !node.attrs.checked }))
            return true
          },
          // Links in the editor are text to edit, never a way out of the page.
          click: (view, event) => {
            if (event.target instanceof Element && event.target.closest('a')) event.preventDefault()
            return false
          },
        },
      },
    })
)

// Tab and Shift+Tab move list items in and out; elsewhere Tab leaves the editor.
const listKeys = $prose((ctx) => {
  const item = listItemSchema.type(ctx)
  return keymap({ Tab: sinkListItem(item), 'Shift-Tab': liftListItem(item) })
})

// Mod+K: link the selection to an address (or unlink it).
const linkKey = $prose((ctx) => {
  const link = linkSchema.type(ctx)
  return keymap({
    'Mod-k': (state, dispatch) => {
      const { from, to, empty } = state.selection
      if (empty) return false
      if (state.doc.rangeHasMark(from, to, link)) return toggleMark(link)(state, dispatch)
      const href = window.prompt(document.getElementById('words')?.dataset.linkPrompt || 'Link address')
      if (!href || !sanitizeLinkHref(href)) return true
      return toggleMark(link, { href: href.trim() })(state, dispatch)
    },
  })
})

// Focus mode: the top-level block holding the cursor is marked, and so are the
// one before it and the one after it; CSS dims the rest.
const focusKey = new PluginKey('ocgen-focus')
const focusBlock = $prose(
  () =>
    new Plugin({
      key: focusKey,
      props: {
        decorations: (state) => {
          const { doc, selection } = state
          if (!doc.childCount) return null
          const at = Math.min(selection.$from.index(0), doc.childCount - 1)
          const marks = []
          doc.forEach((node, offset, i) => {
            if (Math.abs(i - at) <= 1)
              marks.push(Decoration.node(offset, offset + node.nodeSize, { class: i === at ? 'is-current' : 'is-near' }))
          })
          return DecorationSet.create(doc, marks)
        },
      },
    })
)

// --------------------------------------------------- lines wrapped by hand --

// Whether the break between source lines `prev` and `next` falls mid-sentence:
// `prev` ends in `,` or `;`, or `next` goes on in lowercase (not a URL). The
// same rule as `intent::hard_wraps` in src/intent.rs, which ocgen applies after
// every write: GitHub shows each newline in an issue as a line break.
function midSentence(prev, next) {
  if (/[,;]$/.test(prev.replace(/[\s*_~`]+$/, ''))) return true
  const start = next.replace(/^[\s>]+/, '')
  if (/^(https?:\/\/|www\.)/i.test(start)) return false
  const c = start.replace(/^[*_~`[("'“‘]+/, '').charAt(0)
  return c !== '' && c === c.toLowerCase() && c !== c.toUpperCase()
}

// The soft line breaks of `md` that fall mid-sentence: the line each ends, and
// the source to replace with one space to join it (the newline, spaces before
// it, and the next line's indent or `>`). Soft breaks live in text nodes; hard
// breaks, code and raw HTML are other nodes.
function wrapsIn(remark, md) {
  const out = []
  visit(remark.parse(md), 'text', (node) => {
    const p = node.position
    if (!p) return
    for (let i = p.start.offset; i < p.end.offset; i++) {
      if (md.charCodeAt(i) !== 10) continue
      const lineStart = md.lastIndexOf('\n', i - 1) + 1
      const nextEnd = md.indexOf('\n', i + 1) < 0 ? md.length : md.indexOf('\n', i + 1)
      const next = md.slice(i + 1, nextEnd)
      if (!midSentence(md.slice(lineStart, i), next)) continue
      let from = i
      while (from > lineStart && (md[from - 1] === ' ' || md[from - 1] === '\t')) from--
      out.push({ line: md.slice(0, i).split('\n').length, from, to: i + 1 + /^[ \t>]*/.exec(next)[0].length })
    }
  })
  return out
}

// ----------------------------------------------------------- markdown --

// An empty paragraph writes nothing, so it is no block of the file.
const isBlank = (n) => n.type.name === 'paragraph' && !n.textContent.trim() &&
  !n.content.content.some((c) => !c.isText)

const blocksOf = (doc) => {
  const out = []
  doc.forEach((n) => {
    if (!isBlank(n)) out.push(n)
  })
  return out
}

// The longest common subsequence of two block lists, by equal content: for
// each new block, the index of the unchanged old block it is, or -1.
function match(olds, news) {
  const n = olds.length
  const m = news.length
  const t = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1))
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      t[i][j] = olds[i].eq(news[j]) ? t[i + 1][j + 1] + 1 : Math.max(t[i + 1][j], t[i][j + 1])
  const same = new Array(m).fill(-1)
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (olds[i].eq(news[j])) {
      same[j] = i
      i++
      j++
    } else if (t[i + 1][j] >= t[i][j + 1]) i++
    else j++
  }
  return same
}

export async function create({ root, text, label, onChange }) {
  // Tells the page about edits — not about a text it loaded itself.
  let notify = () => {}
  let loading = false
  const changes = $prose(
    () =>
      new Plugin({
        key: new PluginKey('ocgen-changes'),
        view: () => ({
          update: (view, prev) => {
            if (!loading && !view.state.doc.eq(prev.doc)) notify()
          },
        }),
      })
  )

  const editor = Editor.make()
    .config((ctx) => {
      ctx.set(rootCtx, root)
      ctx.set(defaultValueCtx, text)
      // How edited blocks are written: the way GitHub's own editor and
      // /intent write them.
      ctx.update(remarkStringifyOptionsCtx, (o) => ({
        ...o,
        bullet: '-',
        bulletOrdered: '.',
        emphasis: '_',
        strong: '*',
        fence: '`',
        fences: true,
        rule: '-',
        listItemIndent: 'one',
        handlers: { ...o.handlers, ...handlers },
      }))
      ctx.update(editorViewOptionsCtx, (o) => ({
        ...o,
        attributes: {
          class: 'zen-editor',
          spellcheck: 'true',
          role: 'textbox',
          'aria-multiline': 'true',
          'aria-label': label || 'Draft',
        },
      }))
    })
    .use(presetCommonmark)
    .use(gfm)
    .use(history)
    .use(clipboard)
    .use([imageAsText, lineBreak, safeLink, bulletMark, sourceMarks].flat())
    .use([taskToggle, listKeys, linkKey, focusBlock, changes])

  await editor.create()
  const ctx = editor.ctx
  const view = ctx.get(editorViewCtx)
  const parse = ctx.get(parserCtx)
  const serialize = ctx.get(serializerCtx)
  const remark = ctx.get(remarkCtx)

  // `base`: the text the page has, the editor's document for it, and where
  // each of its blocks starts and ends in the text (null when the blocks
  // can't be told apart, e.g. an empty file).
  let base = null
  let cache = { doc: null, md: '' }
  function index(t) {
    const doc = view.state.doc
    let spans = null
    try {
      const tree = remark.runSync(remark.parse(t), t)
      const kids = tree.children
      const blocks = blocksOf(doc)
      if (kids.length === blocks.length && kids.every((k) => k.position)) {
        spans = kids.map((k) => [k.position.start.offset, k.position.end.offset])
      }
    } catch {
      spans = null
    }
    base = { text: t, doc, blocks: blocksOf(doc), spans }
    cache = { doc, md: t }
  }
  index(text)

  // Each edited block's Markdown, kept only as long as the block itself.
  let single = new WeakMap()
  function blockMarkdown(node) {
    let md = single.get(node)
    if (md == null) {
      const doc = view.state.schema.topNodeType.create(null, node)
      md = serialize(doc).replace(/\n+$/, '')
      single.set(node, md)
    }
    return md
  }

  // The new blocks with the old text of unchanged ones. `keep`: whether an
  // edited block keeps the spacing of the block it replaced.
  function splice(news, same, keep) {
    const { text: t, spans, blocks: olds } = base
    // The old block each new one stands in for: itself if unchanged, or the
    // one at the same place between two unchanged blocks.
    const slot = same.slice()
    let lastOld = -1
    for (let j = 0; j < news.length; ) {
      if (same[j] >= 0) {
        lastOld = same[j]
        j++
        continue
      }
      let k = j
      while (k < news.length && same[k] < 0) k++
      const nextOld = k < news.length ? same[k] : olds.length
      if (keep && nextOld - lastOld - 1 === k - j) for (let x = j; x < k; x++) slot[x] = lastOld + 1 + (x - j)
      j = k
    }
    let out = slot[0] === 0 ? t.slice(0, spans[0][0]) : ''
    news.forEach((node, j) => {
      if (j > 0) {
        const a = slot[j - 1]
        const b = slot[j]
        out += a >= 0 && b === a + 1 ? t.slice(spans[a][1], spans[b][0]) : '\n\n'
      }
      out += same[j] >= 0 ? t.slice(spans[same[j]][0], spans[same[j]][1]) : blockMarkdown(node)
    })
    const last = slot[news.length - 1]
    out += last === olds.length - 1 ? t.slice(spans[last][1]) : '\n'
    return out
  }

  // Whether `md` reads back as the editor's blocks: unchanged blocks as they
  // are, edited ones as Milkdown's own Markdown for them reads back alone
  // (which may be more than one block: a heading can't hold a line break).
  function holds(md, news, same) {
    let back
    try {
      back = blocksOf(parse(md))
    } catch {
      return false
    }
    let k = 0
    for (let j = 0; j < news.length; j++) {
      const want = same[j] >= 0 ? [news[j]] : blocksOf(parse(blockMarkdown(news[j])))
      for (const node of want) if (k >= back.length || !back[k++].eq(node)) return false
    }
    return k === back.length
  }

  function markdown() {
    const doc = view.state.doc
    if (doc === cache.doc) return cache.md
    let md = null
    const news = blocksOf(doc)
    if (doc.eq(base.doc)) md = base.text
    else if (base.spans && base.blocks.length && news.length) {
      const same = match(base.blocks, news)
      for (const keep of [true, false]) {
        const out = splice(news, same, keep)
        if (holds(out, news, same)) {
          md = out
          break
        }
      }
    }
    if (md == null) md = serialize(doc)
    cache = { doc, md }
    return md
  }

  notify = () => onChange && onChange()

  return {
    version: VERSION,
    view,
    markdown,
    // Whether the document differs from the text the editor was given.
    edited: () => !view.state.doc.eq(base.doc),
    // Show `t` (a newer file, say): a fresh document and undo history.
    load(t) {
      loading = true
      try {
        editor.action(replaceAll(t, true))
      } finally {
        loading = false
      }
      single = new WeakMap()
      index(t)
    },
    focus: () => view.focus(),
    destroy: () => editor.destroy(),
    // The lines of `md` wrapped by hand mid-sentence, and `md` with them joined
    // (nothing else in it changes).
    wraps: (md) => wrapsIn(remark, md).map((w) => w.line),
    unwrap: (md) =>
      wrapsIn(remark, md)
        .reverse()
        .reduce((t, w) => t.slice(0, w.from) + ' ' + t.slice(w.to), md),
    // For tools/milkdown/fidelity.mjs: where the blocks of the original text
    // are, and the whole document as Milkdown alone would write it.
    spans: () => base.spans,
    rewrite: () => serialize(view.state.doc),
  }
}

export const version = VERSION
