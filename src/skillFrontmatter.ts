// SKILL.md 顶部 frontmatter 的**编辑**侧。读取侧在后端（`tools/skills.rs::parse_frontmatter`），
// 这里只做表单要用的那一小块：定位一行、换掉它的值、把改动写回原文。
//
// 故意比后端那个读取器窄得多：后端要把见到的任何写法都读出来（块标量、缩进列表、行内
// 列表），这里只认**单行标量**，其余一律拒绝改。理由是这个函数会把用户的 SKILL.md 写回
// 磁盘 —— 读错了只是显示不准，写错了是把人家的文件改坏。认不出的字段在表单里置灰，
// 让用户去下面的原文里改，比猜一个 YAML 序列化规则安全得多。
//
// 同样不引 YAML 库：真要序列化任意 YAML 才需要它，而这里改的是 `name` / `description` /
// `allowed-tools` 三个单行字段。

/** frontmatter 里的一行 `key: value`。 */
export interface FmLine {
  key: string
  /** 去掉引号之后的值。 */
  value: string
  /** 在整篇文本里的行号（0 起）。 */
  line: number
}

export interface FmBlock {
  /** 起始 `---` 的行号。 */
  open: number
  /** 结束 `---` 的行号。 */
  close: number
  /** 只有单行标量进这里；块标量、缩进列表都不收。 */
  lines: FmLine[]
}

/**
 * YAML 的引号标量，去壳。和后端 `unquote` 同一套规则。
 *
 * 两种引号的转义规则不一样，别混：双引号里是反斜杠（`\\"`、`\\\\`），单引号里是把引号
 * 写两遍（`''`）。这里必须和 `serialize` 严格对称 —— 表单里读出来的值改都没改就写回去，
 * 文件不能变样。
 */
function unquote(raw: string): string {
  if (raw.length < 2) return raw
  const q = raw[0]
  if (!raw.endsWith(q)) return raw
  const inner = raw.slice(1, -1)
  if (q === '"') return inner.replace(/\\(["\\])/g, '$1')
  if (q === "'") return inner.replace(/''/g, "'")
  return raw
}

/**
 * 找出 frontmatter 块。没有就是 `null` —— 不假装有。
 *
 * 要求第一行就是 `---`（YAML 的规矩，前面有空行就不算 frontmatter 了），到下一条
 * 单独成行的 `---` 为止。
 */
export function findFrontmatter(text: string): FmBlock | null {
  const all = text.split('\n')
  if (all.length === 0 || all[0].trim() !== '---') return null
  const close = all.findIndex((l, i) => i > 0 && l.trim() === '---')
  if (close === -1) return null

  const lines: FmLine[] = []
  for (let i = 1; i < close; i += 1) {
    const raw = all[i]
    // 缩进行属于上一个 key（列表项 / 块标量的内容），不是自己一条。
    if (raw.startsWith(' ') || raw.startsWith('\t')) continue
    const trimmed = raw.trim()
    if (!trimmed || trimmed.startsWith('#')) continue
    const at = trimmed.indexOf(':')
    if (at === -1) continue
    const rest = trimmed.slice(at + 1).trim()
    // 空值 = 后面跟着缩进的列表；`>` / `|` = 块标量。两种都是多行的，不收。
    if (!rest || rest === '>' || rest === '|' || rest === '>-' || rest === '|-') continue
    lines.push({ key: trimmed.slice(0, at).trim(), value: unquote(rest), line: i })
  }
  return { open: 0, close, lines }
}

/** 这个字段现在的值。`null` = 没有，或者它是块标量 / 多行列表（表单不碰）。 */
export function fmValue(text: string, key: string): string | null {
  const block = findFrontmatter(text)
  return block?.lines.find((l) => l.key === key)?.value ?? null
}

/**
 * 这个字段能不能在表单里改。
 *
 * 不存在也算能改（会新插一行）；只有「存在但不是单行标量」才不能 —— 那种要么是块标量
 * 要么是缩进列表，改一行救不回来，得去原文里动。
 */
export function fmEditable(text: string, key: string): boolean {
  const block = findFrontmatter(text)
  if (!block) return true
  if (block.lines.some((l) => l.key === key)) return true
  // 块里出现过这个 key 但没进 `lines`，说明它是多行的。
  const raw = text.split('\n')
  for (let i = block.open + 1; i < block.close; i += 1) {
    const line = raw[i]
    if (line.startsWith(' ') || line.startsWith('\t')) continue
    if (line.trim().split(':')[0].trim() === key) return false
  }
  return true
}

/**
 * 值要不要加引号。
 *
 * 宁可多加：加了引号的 YAML 永远读得回来，不加的可能被读成别的类型（`1.0` 变数字、
 * `yes` 变布尔、`a: b` 变嵌套 map）。
 */
function needsQuotes(value: string): boolean {
  if (value === '') return true
  if (value !== value.trim()) return true
  if (/^[-?:,[\]{}#&*!|>'"%@`]/.test(value)) return true
  if (value.includes(': ') || value.endsWith(':') || value.includes(' #')) return true
  return /^(true|false|null|yes|no|on|off|~|-?\d+(\.\d+)?)$/i.test(value)
}

function serialize(value: string): string {
  if (!needsQuotes(value)) return value
  return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
}

/**
 * 把一个字段写回原文，**别的什么都不动**。
 *
 * 三种落点：
 *
 * - 已有这一行 → 只换这一行，缩进、注释、字段顺序全部原样。
 * - 有 frontmatter 但没这个字段 → 插在结束 `---` 前面。
 * - 压根没有 frontmatter → 在文件最前面补一个块。
 *
 * 值里带换行，或者这个字段是块标量 / 多行列表时原样返回 —— 这两种都不是「换一行」能
 * 表达的改动，调用方应该先用 `fmEditable` 把表单置灰。
 */
export function setFmValue(text: string, key: string, value: string): string {
  if (value.includes('\n')) return text
  const block = findFrontmatter(text)
  const all = text.split('\n')

  if (!block) {
    return `---\n${key}: ${serialize(value)}\n---\n\n${text}`
  }

  const hit = block.lines.find((l) => l.key === key)
  if (hit) {
    all[hit.line] = `${key}: ${serialize(value)}`
    return all.join('\n')
  }
  if (!fmEditable(text, key)) return text

  all.splice(block.close, 0, `${key}: ${serialize(value)}`)
  return all.join('\n')
}

/**
 * 去掉 frontmatter，只留正文。预览用。
 *
 * 不去的话 `renderText()` 会把那两条 `---` 当成水平线，中间三行当成普通段落 —— 一段
 * 谁都不会那样读的文本。而这三个字段上面的表单已经好好摆着了。
 */
export function stripFrontmatter(text: string): string {
  const block = findFrontmatter(text)
  if (!block) return text
  return text.split('\n').slice(block.close + 1).join('\n').replace(/^\n+/, '')
}
