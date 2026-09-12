import { createHighlighterCore, type HighlighterCore } from '@shikijs/core'
import { createJavaScriptRegexEngine } from '@shikijs/engine-javascript'
import { SHIKI_MAX_CHARS } from './renderLimits'

let highlighterPromise: Promise<HighlighterCore> | null = null

const LANG_IMPORTS: Record<string, () => Promise<any>> = {
  javascript: () => import('@shikijs/langs/javascript'),
  typescript: () => import('@shikijs/langs/typescript'),
  jsx: () => import('@shikijs/langs/jsx'),
  tsx: () => import('@shikijs/langs/tsx'),
  python: () => import('@shikijs/langs/python'),
  rust: () => import('@shikijs/langs/rust'),
  go: () => import('@shikijs/langs/go'),
  java: () => import('@shikijs/langs/java'),
  c: () => import('@shikijs/langs/c'),
  cpp: () => import('@shikijs/langs/cpp'),
  html: () => import('@shikijs/langs/html'),
  css: () => import('@shikijs/langs/css'),
  scss: () => import('@shikijs/langs/scss'),
  vue: () => import('@shikijs/langs/vue'),
  svelte: () => import('@shikijs/langs/svelte'),
  json: () => import('@shikijs/langs/json'),
  yaml: () => import('@shikijs/langs/yaml'),
  toml: () => import('@shikijs/langs/toml'),
  xml: () => import('@shikijs/langs/xml'),
  bash: () => import('@shikijs/langs/bash'),
  shell: () => import('@shikijs/langs/shellscript'),
  zsh: () => import('@shikijs/langs/shellscript'),
  powershell: () => import('@shikijs/langs/powershell'),
  sql: () => import('@shikijs/langs/sql'),
  graphql: () => import('@shikijs/langs/graphql'),
  markdown: () => import('@shikijs/langs/markdown'),
  diff: () => import('@shikijs/langs/diff'),
  ruby: () => import('@shikijs/langs/ruby'),
  php: () => import('@shikijs/langs/php'),
  swift: () => import('@shikijs/langs/swift'),
  kotlin: () => import('@shikijs/langs/kotlin'),
  dart: () => import('@shikijs/langs/dart'),
  dockerfile: () => import('@shikijs/langs/dockerfile'),
  lua: () => import('@shikijs/langs/lua'),
  zig: () => import('@shikijs/langs/zig'),
}

const SUPPORTED_LANGS = new Set(Object.keys(LANG_IMPORTS))

/**
 * 围栏信息串（```js / ```ts / ```py …）→ Shiki 规范语言名。CommonMark 允许任意别名，
 * 但 Shiki 语言包名是 javascript / typescript / python，别名对不上 `tryLoadLang` 第一步
 * `SUPPORTED_LANGS.has()` 就 false → 整块 skip、不高亮（`js` 就是这么漏掉的）。
 * 这里把常见别名归一；已是规范名或未知串原样返回（未知的照旧走 SUPPORTED_LANGS 兜底 skip）。
 */
const LANG_ALIASES: Record<string, string> = {
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', node: 'javascript',
  ts: 'typescript', mts: 'typescript', cts: 'typescript',
  py: 'python',
  rb: 'ruby',
  rs: 'rust',
  kt: 'kotlin', kts: 'kotlin',
  golang: 'go',
  sh: 'bash', console: 'bash',
  yml: 'yaml',
  md: 'markdown', mdx: 'markdown',
  htm: 'html',
  'c++': 'cpp',
  gql: 'graphql',
}

export function canonicalLang(lang: string): string {
  return LANG_ALIASES[lang] || lang
}

/** 规范语言名 → 展示名（好看的大小写）。缺省回落规范名本身。 */
const LANG_DISPLAY_NAMES: Record<string, string> = {
  javascript: 'JavaScript', typescript: 'TypeScript', jsx: 'JSX', tsx: 'TSX',
  python: 'Python', rust: 'Rust', go: 'Go', java: 'Java', c: 'C', cpp: 'C++',
  html: 'HTML', css: 'CSS', scss: 'SCSS', vue: 'Vue', svelte: 'Svelte',
  json: 'JSON', yaml: 'YAML', toml: 'TOML', xml: 'XML',
  bash: 'Bash', shell: 'Shell', zsh: 'Zsh', powershell: 'PowerShell',
  sql: 'SQL', graphql: 'GraphQL', markdown: 'Markdown', diff: 'Diff',
  ruby: 'Ruby', php: 'PHP', swift: 'Swift', kotlin: 'Kotlin', dart: 'Dart',
  dockerfile: 'Dockerfile', lua: 'Lua', zig: 'Zig',
}

/**
 * 围栏信息串 → 代码块左上角语言标签的展示名。**未知语言（空串 / 不在支持集）返回 null**，
 * 调用方据此「未知不展示」。已知则给规范化后的展示名（如 `js` → "JavaScript"）。
 */
export function langLabel(rawLang: string): string | null {
  const lang = canonicalLang(rawLang.trim().toLowerCase())
  if (!SUPPORTED_LANGS.has(lang)) return null
  return LANG_DISPLAY_NAMES[lang] ?? lang
}

const THEMES = ['github-light', 'github-dark', 'dracula'] as const

function getHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighterCore({
      themes: [
        import('@shikijs/themes/github-light'),
        import('@shikijs/themes/github-dark'),
        import('@shikijs/themes/dracula'),
      ],
      langs: [],
      engine: createJavaScriptRegexEngine(),
    })
  }
  return highlighterPromise
}

function currentTheme(): typeof THEMES[number] {
  const el = document.documentElement
  if (el.classList.contains('theme-dracula')) return 'dracula'
  if (el.classList.contains('theme-dark')) return 'github-dark'
  return 'github-light'
}

async function tryLoadLang(hl: HighlighterCore, lang: string): Promise<boolean> {
  if (!SUPPORTED_LANGS.has(lang)) return false
  if (hl.getLoadedLanguages().includes(lang as any)) return true
  try {
    const mod = await LANG_IMPORTS[lang]()
    await hl.loadLanguage(mod.default ?? mod)
    return true
  } catch {
    return false
  }
}

// Shiki 产出的 `<pre>` 的 `textContent` 逐字等于原始代码（每行一个 `<span class="line">`，
// 行间是真实的换行文本节点）。所以主题切换重画和「复制代码」都直接读 `textContent`
// 即可，不需要再把源码 `encodeURIComponent` 存一份到 `data-source` —— 那份编码后的
// 副本比原文还大（转义字符 ×3），且随节点一直驻留在内存里。
function replaceWithShiki(
  pre: HTMLPreElement,
  html: string,
  lang: string,
  extraClass?: string,
): void {
  const wrapper = document.createElement('div')
  wrapper.innerHTML = html
  const shikiPre = wrapper.querySelector('pre')
  if (!shikiPre) return
  shikiPre.className = (extraClass ? extraClass + ' ' : '') + 'shiki'
  shikiPre.dataset.shiki = 'done'
  shikiPre.dataset.lang = lang
  pre.replaceWith(shikiPre)
}

const EXT_TO_LANG: Record<string, string> = {
  js: 'javascript', mjs: 'javascript', cjs: 'javascript',
  ts: 'typescript', mts: 'typescript', cts: 'typescript',
  jsx: 'jsx', tsx: 'tsx',
  vue: 'vue', svelte: 'svelte',
  py: 'python',
  rs: 'rust',
  go: 'go',
  java: 'java',
  c: 'c', h: 'c',
  cpp: 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp',
  html: 'html', htm: 'html',
  css: 'css', scss: 'scss',
  json: 'json', jsonc: 'json',
  yaml: 'yaml', yml: 'yaml',
  toml: 'toml',
  xml: 'xml', svg: 'xml',
  sh: 'bash', bash: 'bash', zsh: 'zsh',
  sql: 'sql',
  rb: 'ruby',
  php: 'php',
  swift: 'swift',
  kt: 'kotlin', kts: 'kotlin',
  dart: 'dart',
  lua: 'lua',
  zig: 'zig',
  md: 'markdown', mdx: 'markdown',
  graphql: 'graphql', gql: 'graphql',
  dockerfile: 'dockerfile',
}

function langFromPath(filePath: string): string | null {
  const name = filePath.split('/').pop() || ''
  if (name.toLowerCase() === 'dockerfile') return 'dockerfile'
  const ext = name.includes('.') ? name.split('.').pop()!.toLowerCase() : ''
  return EXT_TO_LANG[ext] || null
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

function applyTokensToSpans(
  textSpans: NodeListOf<HTMLSpanElement>,
  hl: HighlighterCore,
  lang: string,
  themeName: string,
): void {
  const lines = [...textSpans].map(s => s.textContent ?? '')
  const code = lines.join('\n')
  const result = hl.codeToTokens(code, { lang: lang as any, theme: themeName })
  for (let i = 0; i < textSpans.length && i < result.tokens.length; i++) {
    const tokens = result.tokens[i]
    let html = ''
    for (const t of tokens) {
      const style = t.color ? ` style="color:${t.color}"` : ''
      html += `<span${style}>${escapeHtml(t.content)}</span>`
    }
    textSpans[i].innerHTML = html
  }
}

const DIFF_TARGETS: { selector: string; textSelector: string; getFilePath: (el: HTMLElement) => string }[] = [
  {
    selector: '.diff:not([data-shiki])',
    textSelector: '.diff-text',
    getFilePath: (el) => el.dataset.file || '',
  },
  {
    selector: '.codex-patch-file:not([data-shiki])',
    textSelector: '.codex-patch-text',
    getFilePath: (el) => {
      const link = el.querySelector<HTMLAnchorElement>('.codex-patch-path')
      return link?.dataset.localTarget || link?.textContent || ''
    },
  },
]

async function highlightDiffBlocks(root: HTMLElement, hl: HighlighterCore, themeName: string): Promise<void> {
  for (const target of DIFF_TARGETS) {
    const diffs = root.querySelectorAll<HTMLElement>(target.selector)
    for (const diffEl of diffs) {
      const filePath = target.getFilePath(diffEl)
      const lang = langFromPath(filePath)
      if (!lang) { diffEl.dataset.shiki = 'skip'; continue }
      if (!(await tryLoadLang(hl, lang))) { diffEl.dataset.shiki = 'skip'; continue }

      const textSpans = diffEl.querySelectorAll<HTMLSpanElement>(target.textSelector)
      if (!textSpans.length) { diffEl.dataset.shiki = 'skip'; continue }

      applyTokensToSpans(textSpans, hl, lang, themeName)
      diffEl.dataset.shiki = 'done'
      diffEl.dataset.lang = lang
    }
  }
}

async function rehighlightDiffBlocks(root: HTMLElement, hl: HighlighterCore, themeName: string): Promise<void> {
  for (const target of DIFF_TARGETS) {
    const done = target.selector.replace(':not([data-shiki])', '[data-shiki="done"]')
    const diffs = root.querySelectorAll<HTMLElement>(done)
    for (const diffEl of diffs) {
      const lang = diffEl.dataset.lang || ''
      if (!lang) continue
      const textSpans = diffEl.querySelectorAll<HTMLSpanElement>(target.textSelector)
      if (!textSpans.length) continue
      applyTokensToSpans(textSpans, hl, lang, themeName)
    }
  }
}

/**
 * 内置编辑器专用：把源码切成逐行 token，调用方自己拼 DOM。
 *
 * 不用 `codeToHtml`：那出来的是 shiki 自己的 `<pre>`，带它自己的背景和内边距，而编辑器
 * 的高亮层必须和 textarea **逐像素同款**（同字体、同行高、同 padding、同换行规则）。
 * 拿 token 自己拼，这些就全在我们手里。
 *
 * `null` = 这一份不高亮（语言不认识、没加载成功、或者超过 `SHIKI_MAX_CHARS`），
 * 调用方退化成纯文本 —— shiki 是 tokenizer，大文件每次按键全量跑必卡。
 */
export async function highlightLines(
  code: string,
  lang: string,
): Promise<{ content: string; color?: string }[][] | null> {
  const canonical = canonicalLang(lang)
  if (!canonical || !code || code.length > SHIKI_MAX_CHARS) return null
  const hl = await getHighlighter()
  if (!(await tryLoadLang(hl, canonical))) return null
  const { tokens } = hl.codeToTokens(code, { lang: canonical as any, theme: currentTheme() })
  return tokens.map((line) => line.map((t) => ({ content: t.content, color: t.color })))
}

/** 按文件名猜语言。编辑器打开一个文件时用它决定高亮成什么。 */
export function langOfPath(filePath: string): string | null {
  return langFromPath(filePath)
}

export async function highlightAllCodeBlocks(root: HTMLElement | null): Promise<void> {
  if (!root) return

  const fenced = root.querySelectorAll<HTMLPreElement>('pre.code-block:not([data-shiki])')
  const toolJson = root.querySelectorAll<HTMLPreElement>('pre.lang-json:not([data-shiki])')
  const toolDiff = root.querySelectorAll<HTMLPreElement>('pre.lang-diff:not([data-shiki])')
  const diffBlocks = root.querySelectorAll<HTMLElement>('.diff:not([data-shiki]), .codex-patch-file:not([data-shiki])')

  if (!fenced.length && !toolJson.length && !toolDiff.length && !diffBlocks.length) return
  const hl = await getHighlighter()
  const themeName = currentTheme()

  for (const pre of fenced) {
    const lang = canonicalLang(pre.dataset.lang || '')
    if (!lang) { pre.dataset.shiki = 'skip'; continue }
    const code = pre.querySelector('code')?.textContent ?? ''
    if (!code || code.length > SHIKI_MAX_CHARS) { pre.dataset.shiki = 'skip'; continue }
    if (!(await tryLoadLang(hl, lang))) { pre.dataset.shiki = 'skip'; continue }
    const html = hl.codeToHtml(code, { lang, theme: themeName })
    replaceWithShiki(pre, html, lang, 'code-block')
  }

  for (const pre of toolJson) {
    const code = pre.textContent ?? ''
    if (!code.trim() || code.length > SHIKI_MAX_CHARS) { pre.dataset.shiki = 'skip'; continue }
    if (!(await tryLoadLang(hl, 'json'))) { pre.dataset.shiki = 'skip'; continue }
    const html = hl.codeToHtml(code, { lang: 'json', theme: themeName })
    replaceWithShiki(pre, html, 'json', 'lang-json')
  }

  for (const pre of toolDiff) {
    const code = pre.textContent ?? ''
    if (!code.trim() || code.length > SHIKI_MAX_CHARS) { pre.dataset.shiki = 'skip'; continue }
    if (!(await tryLoadLang(hl, 'diff'))) { pre.dataset.shiki = 'skip'; continue }
    const html = hl.codeToHtml(code, { lang: 'diff', theme: themeName })
    replaceWithShiki(pre, html, 'diff', 'lang-diff')
  }

  if (diffBlocks.length) {
    await highlightDiffBlocks(root, hl, themeName)
  }
}

export async function rehighlightAllCodeBlocks(root: HTMLElement | null): Promise<void> {
  if (!root) return
  const blocks = root.querySelectorAll<HTMLPreElement>('pre[data-shiki="done"]')
  const diffBlocks = root.querySelectorAll<HTMLElement>('.diff[data-shiki="done"], .codex-patch-file[data-shiki="done"]')
  if (!blocks.length && !diffBlocks.length) return
  const hl = await getHighlighter()
  const themeName = currentTheme()

  for (const pre of blocks) {
    const lang = pre.dataset.lang || ''
    const code = pre.textContent ?? ''
    if (!lang || !code) continue
    const origClass = pre.className.replace(/\bshiki\b/, '').trim()
    const html = hl.codeToHtml(code, { lang, theme: themeName })
    replaceWithShiki(pre, html, lang, origClass)
  }

  if (diffBlocks.length) {
    await rehighlightDiffBlocks(root, hl, themeName)
  }
}
