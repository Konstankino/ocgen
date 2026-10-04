// Fidelity check for the bundle: opens tests/fixtures/intent-draft-everything.md
// in the built editor (jsdom) and checks what Save would write.
//
//   cd tools/milkdown && npm test            # checks, exits 1 on a failure
//   cd tools/milkdown && npm test -- --show  # also prints every diff
//
// It reports what Milkdown changes in an edited block, and what it would
// change everywhere if the whole document were rewritten (the fallback).

import { JSDOM } from 'jsdom'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const bundle = readFileSync(join(here, '..', '..', 'src', 'notes', 'milkdown', 'editor.min.js'), 'utf8')
const fixture = readFileSync(join(here, '..', '..', 'tests', 'fixtures', 'intent-draft-everything.md'), 'utf8')
const show = process.argv.includes('--show')

let failures = 0
function check(ok, what, detail) {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`)
  if (!ok) {
    failures++
    if (detail) console.log(indent(detail))
  }
}
const indent = (s) => s.replace(/^/gm, '       ')

async function open(text) {
  const dom = new JSDOM('<!doctype html><div id="words"></div><div id="root"></div>', {
    runScripts: 'outside-only',
    pretendToBeVisual: true,
  })
  const w = dom.window
  // Layout calls jsdom doesn't implement; ProseMirror only needs them to exist.
  w.document.elementFromPoint = () => null
  w.Range.prototype.getClientRects = () => []
  w.Range.prototype.getBoundingClientRect = () => ({ top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 })
  w.eval(bundle)
  const api = await w.OcgenDraft.create({ root: w.document.getElementById('root'), text, label: 'Draft' })
  return { w, api, view: api.view }
}

// The end of the first textblock inside top-level block `i`, or null.
function endOfText(doc, i) {
  let start = 0
  for (let k = 0; k < i; k++) start += doc.child(k).nodeSize
  const block = doc.child(i)
  if (block.isTextblock) {
    // After its last text: a block of raw HTML alone has none.
    let end = null
    block.forEach((c, off) => {
      if (c.isText) end = start + 1 + off + c.nodeSize
    })
    return end ?? (block.type.spec.code ? start + 1 : null)
  }
  let at = null
  block.descendants((n, pos) => {
    if (at != null) return false
    if (n.isTextblock && !n.type.spec.code && n.textContent.length) {
      at = start + 1 + pos + 1 + n.content.size
      return false
    }
    return true
  })
  if (at == null && doc.child(i).type.spec.code) at = start + 1 + doc.child(i).content.size
  return at
}

function edit(view, pos, word = ' EDITED') {
  view.dispatch(view.state.tr.insertText(word, pos))
}

// A small line diff: '-' old, '+' new, ' ' same.
function diff(a, b) {
  const x = a.split('\n')
  const y = b.split('\n')
  const t = Array.from({ length: x.length + 1 }, () => new Uint32Array(y.length + 1))
  for (let i = x.length - 1; i >= 0; i--)
    for (let j = y.length - 1; j >= 0; j--)
      t[i][j] = x[i] === y[j] ? t[i + 1][j + 1] + 1 : Math.max(t[i + 1][j], t[i][j + 1])
  const out = []
  let i = 0
  let j = 0
  while (i < x.length || j < y.length) {
    if (i < x.length && j < y.length && x[i] === y[j]) {
      out.push('  ' + x[i])
      i++
      j++
    } else if (j < y.length && (i === x.length || t[i][j + 1] >= t[i + 1][j])) out.push('+ ' + y[j++])
    else out.push('- ' + x[i++])
  }
  return out.filter((l) => !l.startsWith('  ')).join('\n')
}

// ------------------------------------------------------------------ run --

{
  const { api } = await open(fixture)
  check(api.markdown() === fixture, 'nothing edited: Save writes the file byte for byte')
  check(!api.edited(), 'nothing edited: the editor says so')
  check(api.spans() && api.spans().length === api.view.state.doc.childCount, 'every block of the file is located')
}

{
  // An empty paragraph (Enter at the end of a block) writes nothing.
  const { api, view } = await open(fixture)
  const end = endOfText(view.state.doc, 2)
  view.dispatch(view.state.tr.split(end))
  check(api.markdown() === fixture, 'an empty new paragraph changes nothing in the file')
}

// Edit each block in turn: the file changes inside that block only.
const blockReport = []
{
  const probe = await open(fixture)
  const count = probe.view.state.doc.childCount
  const spans = probe.api.spans()
  for (let i = 0; i < count; i++) {
    const { api, view } = await open(fixture)
    const pos = endOfText(view.state.doc, i)
    const name = view.state.doc.child(i).type.name
    const [s, e] = spans[i]
    const was = fixture.slice(s, e)
    if (pos == null) {
      blockReport.push({ i, name, was, now: null })
      continue
    }
    edit(view, pos)
    const md = api.markdown()
    const head = fixture.slice(0, s)
    const tail = fixture.slice(e)
    const inside = md.startsWith(head) && md.endsWith(tail)
    const now = inside ? md.slice(head.length, md.length - tail.length) : null
    check(inside, `edit in block ${i} (${name}) changes only that block`, inside ? '' : diff(fixture, md))
    blockReport.push({ i, name, was, now })
  }
}

// After an edit elsewhere, everything special survives as written.
{
  const { api, view } = await open(fixture)
  edit(view, endOfText(view.state.doc, 2)) // "Deploys keep the cache warm…"
  const md = api.markdown()
  for (const [what, text] of [
    ['<details><summary>', '<details><summary>Plan (fill in once the intent is agreed)</summary>'],
    ['</details>', '\n</details>\n'],
    ['<kbd>', 'Press <kbd>Ctrl</kbd>+<kbd>S</kbd> to save.<br>'],
    ['<br>', 'save.<br>\nA second line'],
    ['a one-line comment', '## Intent\n<!-- 2-3 lines. What should be true afterwards. -->\n'],
    ['a comment over three lines', '<!--\nA comment over\nthree lines.\n-->'],
    ['task list', '- [ ] @alice — pending\n- [x] @bob-smith — approved on 2026-10-01\n'],
    ['table', '|--------|-----:|:----:|\n| A      | low  | Medium |'],
    ['code block', '```rust\nfn warm(keys: &[Key]) -> Result<()> {'],
    ['@mention in a heading', '## Needs from @alice and @bob-smith'],
    ['#123', 'follows up #123 and owner/repo#45.'],
    ['star bullets', '* A bullet written with a star\n'],
    ['reference link and its definition', 'the [change][pr]\n</details>\n\n[pr]: https://github.com/owner/repo/pull/7\n'],
    ['image as written', '![latency chart](https://example.com/p99.png)'],
    ['autolink and bare URL', '<https://example.com/warm> and https://example.com/bare.'],
  ]) {
    check(md.includes(text), `after an edit elsewhere: ${what} survives as written`, `missing: ${JSON.stringify(text)}`)
  }
  check(!/\\@|\\#/.test(md), 'after an edit elsewhere: no \\@ or \\# escapes')
  check(md === fixture.replace('repo#45.', 'repo#45. EDITED'), 'after an edit elsewhere: the diff is the edit alone', diff(fixture, md))
}

// Edits inside the blocks that hold mentions, issue numbers and HTML.
{
  const cases = [
    ['task list', 'pending', ['- [ ] @alice — pending EDITED', '- [x] @bob-smith', '- [ ] No reply yet from @carol_dev']],
    ['the paragraph with #123', 'stay fast.', ['follows up #123 and owner/repo#45.', '[ADR-0007](docs/adr/ADR-0007-cache.md)']],
    ['the paragraph with <kbd> and <br>', 'to save.', ['<kbd>Ctrl</kbd>+<kbd>S</kbd>', '<br>', '~~an old idea~~']],
    ['the heading with @mentions', '@bob-smith', ['## Needs from @alice and @bob-smith EDITED']],
    ['the list inside <details>', 'tests touched', ['<details><summary>Plan', '- **Changes:** templates, code, and tests touched EDITED', '</details>', '[pr]: https://github.com/owner/repo/pull/7']],
  ]
  for (const [what, after, want] of cases) {
    const { api, view } = await open(fixture)
    let pos = null
    view.state.doc.descendants((n, p) => {
      if (pos == null && n.isText && n.text.includes(after)) pos = p + n.text.indexOf(after) + after.length
    })
    edit(view, pos)
    const md = api.markdown()
    const missing = want.filter((t) => !md.includes(t))
    check(!missing.length && !/\\@|\\#/.test(md), `edit in ${what}: mentions, #123 and HTML stay as written`, missing.join('\n') + '\n' + diff(fixture, md))
    if (show) console.log(indent(diff(fixture, md)))
  }
}

// Blocks added, removed, retyped; boxes ticked; an empty file.
{
  const at = (view, i) => {
    let p = 0
    for (let k = 0; k < i; k++) p += view.state.doc.child(k).nodeSize
    return p
  }
  let { api, view } = await open(fixture)
  const quote = '> Quoted from the incident review: cold caches cost 40 s of p99.\n\n'
  let p = at(view, 8)
  view.dispatch(view.state.tr.delete(p, p + view.state.doc.child(8).nodeSize))
  check(api.markdown() === fixture.replace(quote, ''), 'a deleted block leaves the rest as written', diff(fixture, api.markdown()))
  ;({ api, view } = await open(fixture))
  view.dispatch(view.state.tr.insert(at(view, 3), view.state.schema.nodes.paragraph.create(null, view.state.schema.text('A new paragraph.'))))
  check(api.markdown() === fixture.replace('**Blocks prod?**', 'A new paragraph.\n\n**Blocks prod?**'), 'a new block goes in with blank lines around it', diff(fixture, api.markdown()))
  ;({ api, view } = await open(fixture))
  view.dispatch(view.state.tr.setBlockType(at(view, 2) + 1, at(view, 2) + 1, view.state.schema.nodes.heading, { level: 3 }))
  const retyped = api.markdown()
  check(retyped.startsWith('## Intent\n<!-- 2-3 lines. What should be true afterwards. -->\n### Deploys keep') && retyped.endsWith(fixture.slice(fixture.indexOf('\n\n**Blocks prod?**'))),
    'a paragraph turned into a heading keeps the spacing around it', diff(fixture, retyped))
  ;({ api, view } = await open(fixture))
  view.state.doc.descendants((n, pos) => {
    if (n.type.name === 'list_item' && n.attrs.checked === false && n.textContent.startsWith('@alice'))
      view.dispatch(view.state.tr.setNodeMarkup(pos, undefined, { ...n.attrs, checked: true }))
  })
  check(api.markdown() === fixture.replace('- [ ] @alice — pending', '- [x] @alice — pending'), 'ticking a box changes that box only', diff(fixture, api.markdown()))
  ;({ api, view } = await open(''))
  view.dispatch(view.state.tr.insertText('Hello @alice, see #12.', 1))
  check(api.markdown() === 'Hello @alice, see #12.\n', 'an empty file takes what is typed', JSON.stringify(api.markdown()))
}

// Lines wrapped by hand: the same lines as `intent::hard_wraps` finds (see
// tests/intent_wraps.rs), and joining them changes nothing else.
{
  const wrapped = readFileSync(join(here, '..', '..', 'tests', 'fixtures', 'intent-draft-wrapped.md'), 'utf8')
  const { api } = await open(fixture)
  const lines = api.wraps(wrapped)
  check(lines.join(',') === '3,4,5,8,9,10,11,19,23', 'wraps: the lines src/intent.rs finds', lines.join(','))
  check(api.wraps(fixture).length === 0, 'wraps: none in the everything fixture (meant breaks stay)', api.wraps(fixture).join(','))
  const joined = [
    ['plan, confidence\nor idle', 'plan, confidence or idle'],
    ['the gates are\nactive', 'the gates are active'],
    ['Evidence and full\nfindings', 'Evidence and full findings'],
    ['(F4); the\napproval', '(F4); the approval'],
    ['(F8);\n`CLAUDE.md`', '(F8); `CLAUDE.md`'],
    ['`.git/config`,\n`WebSearch`', '`.git/config`, `WebSearch`'],
    ['no-op idle\ngate*', 'no-op idle gate*'],
    ['history; the\n  only', 'history; the only'],
    ['is wrapped\n> by hand', 'is wrapped by hand'],
  ].reduce((t, [a, b]) => t.replace(a, b), wrapped)
  const got = api.unwrap(wrapped)
  check(got === joined, 'unwrap: joins only the wrapped lines, byte for byte otherwise', diff(joined, got))
  check(api.wraps(got).length === 0, 'unwrap: nothing left to join')
}

// Focus mode marks the block with the cursor, and one before and one after.
{
  const { view } = await open(fixture)
  const TextSelection = view.state.selection.constructor // the editor starts with a text cursor
  const pos = endOfText(view.state.doc, 5)
  view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, pos)))
  const kids = [...view.dom.children]
  const marked = kids.map((el, i) => (el.classList.contains('is-current') ? `${i}:current` : el.classList.contains('is-near') ? `${i}:near` : null)).filter(Boolean)
  check(marked.join(' ') === '4:near 5:current 6:near', 'focus mode: the current block and one on each side', marked.join(' '))
}

// A newer file replaces the document; the new text is the new baseline.
{
  const { api } = await open(fixture)
  const newer = fixture.replace('## Intent', '## Intent (revised)')
  api.load(newer)
  check(api.markdown() === newer && !api.edited(), 'load(): a newer file is shown and saved back unchanged')
}

// Nothing in the editor's DOM loads or runs anything.
{
  const evil = '<script>alert(1)</script>\n\n<img src=x onerror=alert(2)>\n\nInline <b onclick="x()">bold</b> ![p](https://evil.example/p.png) [j](javascript:alert(3))\n'
  const { w, view } = await open(evil)
  const html = view.dom.innerHTML
  // ProseMirror's own cursor helpers are <img class="ProseMirror-separator">
  // with no source; nothing else may be an element that loads or runs.
  check(!w.document.querySelector('#root script, #root img[src], #root img:not(.ProseMirror-separator), #root [onerror], #root [onclick], #root b'),
    'raw HTML and images are text in the editor, never live elements', html)
  check(![...w.document.querySelectorAll('#root a')].some((a) => /^javascript:/i.test(a.getAttribute('href') || '')), 'javascript: links get no href')
}

// ----------------------------------------------------------- the report --

console.log('\nWhat Milkdown writes for a block that was edited (the edit is " EDITED"):')
for (const b of blockReport) {
  if (b.now == null) {
    console.log(`  block ${b.i} (${b.name}): no text to edit — it is an atom (raw HTML), kept as written`)
    continue
  }
  const d = diff(b.was, b.now.replace(/ EDITED/g, ''))
  console.log(`  block ${b.i} (${b.name}): ${d ? 'rewritten' : 'only the edit'}`)
  if (d) console.log(indent(d))
}

{
  const { api, view } = await open(fixture)
  edit(view, endOfText(view.state.doc, 2))
  const whole = api.rewrite().replace(' EDITED', '')
  console.log('\nThe fallback — the whole file rewritten by Milkdown — would change:')
  console.log(indent(diff(fixture, whole) || '(nothing)'))
}

console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed')
process.exit(failures ? 1 : 0)
