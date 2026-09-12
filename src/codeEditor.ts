// 内置编辑器的纯逻辑：缩进计算 + 高亮层的 HTML 拼装。
//
// 编辑器本体是 `components/CodeEditor.vue`（透明 textarea 叠在高亮层上）。把这两件事
// 挪出来的理由是它们都**不碰 DOM 却极容易写错**：
//
// - 缩进要返回「选哪一段、插什么、插完光标在哪」，而不是直接改 `textarea.value` ——
//   赋值会清空浏览器原生的撤销栈，用户按一次 ⌘Z 整个文件回到打开时的样子。
//   走 `execCommand('insertText')` 才保得住，而那个 API 只认「当前选区 + 一段文本」。
// - 高亮层和 textarea 必须**逐字符对齐**，差一个转义就整屏错位。

/** 一次缩进用两个空格。skill 里是 md / sh / json / yaml，两格是这几种的通行写法。 */
export const INDENT = '  '

export interface HlToken {
  content: string
  color?: string
}

export function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

/**
 * 高亮层的 HTML。
 *
 * 末尾补一个换行：textarea 的内容以 `\n` 结尾时，它自己会多留一个空行的高度，而
 * `<pre>` 的最后一个换行会被吃掉 —— 两层的总高度就差一行，光标跑到文档末尾时高亮
 * 整体上移一行。
 */
export function tokensToHtml(lines: HlToken[][]): string {
  return (
    lines
      .map((tokens) =>
        tokens
          .map((t) =>
            t.color
              ? `<span style="color:${t.color}">${escapeHtml(t.content)}</span>`
              : escapeHtml(t.content),
          )
          .join(''),
      )
      .join('\n') + '\n'
  )
}

/** 不高亮时的同款结构：同样补末尾换行，换行规则必须和 `tokensToHtml` 一模一样。 */
export function plainToHtml(code: string): string {
  return escapeHtml(code) + '\n'
}

// ---------------------------------------------------------------------------
// 缩进
// ---------------------------------------------------------------------------

/**
 * 一次 Tab / Shift-Tab 要做的事。
 *
 * `from` / `to` 是**插入前要选中**的范围，`text` 是要插进去的内容，`selStart` / `selEnd`
 * 是插完之后的选区。这个形状是为 `execCommand('insertText')` 定的。
 */
export interface IndentEdit {
  from: number
  to: number
  text: string
  selStart: number
  selEnd: number
}

/** 这一行开头有多少能被吃掉的缩进（最多一层）。 */
function outdentWidth(line: string): number {
  if (line.startsWith('\t')) return 1
  let n = 0
  while (n < INDENT.length && line[n] === ' ') n += 1
  return n
}

/** 光标所在行的行首下标。 */
function lineStart(value: string, at: number): number {
  return value.lastIndexOf('\n', at - 1) + 1
}

/**
 * 算一次缩进。`null` = 什么都不做（Shift-Tab 但没有一行有缩进可退）。
 *
 * 三种情况分开：
 *
 * - **Tab，选区没跨行**：就是插一段缩进（选中的内容被替换掉，和输入任何字符一致）。
 * - **Tab，选区跨行**：整块每行前面加一层。这时候用户要的是「缩进这几行」，
 *   而不是「把这几行换成两个空格」。
 * - **Shift-Tab**：一律按整行处理，哪怕没有选区 —— 退的是行首的缩进，和光标在哪无关。
 */
export function indentEdit(
  value: string,
  start: number,
  end: number,
  outdent: boolean,
): IndentEdit | null {
  const selected = value.slice(start, end)
  if (!outdent && !selected.includes('\n')) {
    return {
      from: start,
      to: end,
      text: INDENT,
      selStart: start + INDENT.length,
      selEnd: start + INDENT.length,
    }
  }

  const from = lineStart(value, start)
  // 选区正好停在换行符后面时，最后那一行其实没被选中，别跟着一起缩进。
  const tail = end > start && value[end - 1] === '\n' ? end - 1 : end
  const nl = value.indexOf('\n', tail)
  const to = nl === -1 ? value.length : nl

  const lines = value.slice(from, to).split('\n')
  let firstDelta = 0
  let totalDelta = 0
  const next = lines.map((line, i) => {
    if (outdent) {
      const width = outdentWidth(line)
      if (i === 0) firstDelta = -width
      totalDelta -= width
      return line.slice(width)
    }
    // 空行不缩进：加进去的是一串看不见的尾随空白，保存之后 diff 里全是它们。
    if (line.length === 0) return line
    if (i === 0) firstDelta = INDENT.length
    totalDelta += INDENT.length
    return INDENT + line
  })
  if (totalDelta === 0) return null

  const text = next.join('\n')
  // 选区本来就贴着行首时钉在行首不动 —— 用户要的是「这几行」，缩进进来的那两格也算
  // 这几行的一部分，连按几次 Tab 选区不该越缩越小。其余情况保住光标的相对位置
  // （但不许退到行首之前）。
  const selStart = start === from ? from : Math.max(from, start + firstDelta)
  const selEnd = start === end ? selStart : Math.max(selStart, end + totalDelta)
  return { from, to, text, selStart, selEnd }
}

/**
 * 回车时沿用上一行的缩进。
 *
 * 不做这件事的话，在一段缩进的 yaml / 脚本里每敲一次回车都要手动补齐，而这个编辑器
 * 的定位恰恰是「改个参数」——最常做的动作就是在某一行下面再加一行。
 */
export function newlineIndent(value: string, start: number): string {
  const line = value.slice(lineStart(value, start), start)
  const lead = /^[\t ]*/.exec(line)?.[0] ?? ''
  return '\n' + lead
}
