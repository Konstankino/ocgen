// Builds the Milkdown bundle ocgen's /intent draft editor embeds:
//   src/notes/milkdown/editor.min.js   the minified bundle (inlined into the page)
//   src/notes/milkdown/LICENSES.txt    the license of every package in it
//
//   cd tools/milkdown && npm ci && npm run build && npm test
//
// The Cargo build never runs this: the bundle is checked in, and
// `include_str!` embeds it in the binary.

import { build } from 'esbuild'
import { readFileSync, writeFileSync, mkdirSync, existsSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const out = join(here, '..', '..', 'src', 'notes', 'milkdown')
const pkg = JSON.parse(readFileSync(join(here, 'package.json'), 'utf8'))
const milkdown = pkg.dependencies['@milkdown/core']

const result = await build({
  entryPoints: [join(here, 'src', 'editor.js')],
  bundle: true,
  minify: true,
  format: 'iife',
  globalName: 'OcgenDraft',
  target: ['es2020', 'chrome100', 'firefox100', 'safari15'],
  platform: 'browser',
  legalComments: 'none',
  charset: 'utf8',
  define: { __MILKDOWN_VERSION__: JSON.stringify(milkdown), 'process.env.NODE_ENV': '"production"' },
  metafile: true,
  outfile: join(out, 'editor.min.js'),
  write: false,
})

let js = result.outputFiles[0].text
// The bundle goes inside an inline <script>: nothing in it may end the
// element or open an HTML comment there.
js = js.replace(/<\/(script)/gi, '<\\/$1').replace(/<!--/g, '<\\!--')
js = js.replace('"__MILKDOWN_VERSION__"', JSON.stringify(milkdown))
for (const [bad, why] of [
  [/<\/script/i, 'ends the inline script'],
  [/<!--/, 'opens an HTML comment'],
  [/\beval\s*\(/, 'needs unsafe-eval'],
  [/new\s+Function\s*\(/, 'needs unsafe-eval'],
  [/\bimport\s*\(/, 'loads code at run time'],
  [/https?:\/\/[^\s"'`]*\.(?:js|css|woff2?)\b/, 'names a remote script, style or font'],
]) {
  if (bad.test(js)) throw new Error(`the bundle ${why}: ${js.match(bad)[0]}`)
}

// Which packages went in, how big each one is in the bundle.
const sizes = new Map()
for (const [file, info] of Object.entries(Object.values(result.metafile.outputs)[0].inputs)) {
  const m = file.match(/node_modules\/((?:@[^/]+\/)?[^/]+)\//)
  const name = m ? m[1] : '(ocgen) ' + file.replace(/^.*tools\/milkdown\//, '')
  sizes.set(name, (sizes.get(name) || 0) + info.bytesInOutput)
}

const packages = [...sizes.keys()].filter((n) => !n.startsWith('(ocgen)')).sort()
const meta = packages.map((name) => {
  const dir = join(here, 'node_modules', name)
  const p = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
  const lic = readdirSync(dir).find((f) => /^(licen[sc]e|copying)(\.|$)/i.test(f))
  const text = lic ? readFileSync(join(dir, lic), 'utf8').trim() : ''
  const holder = (text.match(/^\s*(?:\(c\)\s*|copyright\b.*)$/im) || [''])[0].trim()
  return { name, version: p.version, license: p.license || '?', text, holder }
})
const unlicensed = meta.filter((m) => !m.text)
if (unlicensed.length) throw new Error('no license file: ' + unlicensed.map((m) => m.name).join(', '))
const notMit = meta.filter((m) => m.license !== 'MIT')

const header = [
  `/*! ocgen /intent draft editor — Milkdown ${milkdown} (CommonMark + GFM, history, clipboard).`,
  ` * Built by tools/milkdown: cd tools/milkdown && npm ci && npm run build`,
  ` * Bundled packages and their licenses (full texts: src/notes/milkdown/LICENSES.txt):`,
  ...meta.map((m) => ` *   ${m.name}@${m.version} ${m.license}${m.holder ? ' — ' + m.holder : ''}`),
  ` * MIT: Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated`,
  ` * documentation files (the "Software"), to deal in the Software without restriction, including without limitation the`,
  ` * rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit`,
  ` * persons to whom the Software is furnished to do so, subject to the following conditions: The above copyright notice`,
  ` * and this permission notice shall be included in all copies or substantial portions of the Software. THE SOFTWARE IS`,
  ` * PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF`,
  ` * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT`,
  ` * HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,`,
  ` * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.`,
  ...(notMit.length ? [` * Not MIT (see LICENSES.txt): ${notMit.map((m) => `${m.name} (${m.license})`).join(', ')}`] : []),
  ` */`,
].join('\n')

if (!existsSync(out)) mkdirSync(out, { recursive: true })
writeFileSync(join(out, 'editor.min.js'), header + '\n' + js.trimEnd() + '\n')

const licenses = [
  `Third-party software in src/notes/milkdown/editor.min.js`,
  `Built from tools/milkdown (Milkdown ${milkdown}); regenerate with: cd tools/milkdown && npm ci && npm run build`,
  '',
  ...meta.flatMap((m) => ['='.repeat(78), `${m.name}@${m.version} — ${m.license}`, '-'.repeat(78), m.text, '']),
].join('\n')
writeFileSync(join(out, 'LICENSES.txt'), licenses.trimEnd() + '\n')

const total = Buffer.byteLength(header + '\n' + js)
const kb = (n) => (n / 1024).toFixed(1).padStart(7) + ' KiB'
console.log(`editor.min.js: ${kb(total)} (Milkdown ${milkdown}, ${packages.length} packages)`)
for (const [name, n] of [...sizes].sort((a, b) => b[1] - a[1]).slice(0, 25)) console.log(`  ${kb(n)}  ${name}`)
