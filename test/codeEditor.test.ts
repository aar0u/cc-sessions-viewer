import { describe, it, expect } from 'vitest'
import {
  INDENT,
  escapeHtml,
  tokensToHtml,
  plainToHtml,
  indentEdit,
  newlineIndent,
} from '../src/codeEditor'

/** 把 `indentEdit` 的结果套回原文，得到「按下这一次键之后」的完整文本。 */
function apply(value: string, start: number, end: number, outdent = false) {
  const edit = indentEdit(value, start, end, outdent)
  if (!edit) return null
  return {
    text: value.slice(0, edit.from) + edit.text + value.slice(edit.to),
    selStart: edit.selStart,
    selEnd: edit.selEnd,
  }
}

describe('escaping', () => {
  it('escapes exactly the three characters that would break the markup', () => {
    expect(escapeHtml('a & b < c > d')).toBe('a &amp; b &lt; c &gt; d')
  })

  it('escapes the ampersand first so an entity is not double-written', () => {
    // 顺序错了这里会变成 `&amp;lt;`，屏幕上看到的是 `&lt;` 而不是 `<`。
    expect(escapeHtml('&lt;')).toBe('&amp;lt;')
  })

  it('leaves quotes alone — they live in text nodes, not attributes', () => {
    expect(escapeHtml(`"it's"`)).toBe(`"it's"`)
  })
})

/** 一行的盒子。行号是这个盒子的 `::before` 画的，所以每一行都得是一个真元素。 */
const R = (inner: string) => `<span class="ce-row">${inner}</span>`

describe('the highlight layer', () => {
  it('wraps only the tokens that carry a colour', () => {
    const html = tokensToHtml([[{ content: 'let', color: '#f00' }, { content: ' x' }]])
    expect(html).toBe(R('<span style="color:#f00">let</span> x'))
  })

  it('escapes inside the spans too', () => {
    const html = tokensToHtml([[{ content: '<div>', color: '#0f0' }]])
    expect(html).toBe(R('<span style="color:#0f0">&lt;div&gt;</span>'))
  })

  it('puts every line in its own box, with nothing in between', () => {
    // 行之间夹任何字符都不行：高亮层是 `pre-wrap`，一个换行就是一行空行，从第二行
    // 起整层和 textarea 错开。
    const html = tokensToHtml([[{ content: 'a' }], [{ content: 'b' }]])
    expect(html).toBe(R('a') + R('b'))
  })

  it('renders an empty line as an empty box, not as nothing', () => {
    expect(tokensToHtml([[{ content: 'a' }], [], [{ content: 'b' }]])).toBe(R('a') + R('') + R('b'))
  })

  it('always has at least one line — an empty file still shows line 1', () => {
    expect(tokensToHtml([])).toBe(R(''))
    expect(plainToHtml('')).toBe(R(''))
  })

  it('行数以正文为准，不以 token 行数为准', () => {
    // 结尾那个 `\n` 要不要算最后一个空行，shiki 各版本不一样，而 textarea 一定给它
    // 留一行的高度。少一行就是「光标走到文末，高亮整体上移一行」。
    expect(tokensToHtml([[{ content: 'a' }]], 2)).toBe(R('a') + R(''))
    // 多出来的 token 行也不画：正文只有一行，就只有一行。
    expect(tokensToHtml([[{ content: 'a' }], [{ content: 'b' }]], 1)).toBe(R('a'))
  })

  it('分行规则两条路径一模一样，否则上色跑完那一刻整层会跳行', () => {
    expect(plainToHtml('a\nb')).toBe(R('a') + R('b'))
    expect(plainToHtml('a\nb')).toBe(tokensToHtml([[{ content: 'a' }], [{ content: 'b' }]]))
  })

  it('keeps a text that already ends in a newline distinguishable', () => {
    expect(plainToHtml('a\n')).toBe(R('a') + R(''))
  })
})

describe('Tab', () => {
  it('inserts one level at the cursor', () => {
    const out = apply('const x = 1', 6, 6)
    expect(out?.text).toBe('const ' + INDENT + 'x = 1')
    expect(out?.selStart).toBe(8)
    expect(out?.selEnd).toBe(8)
  })

  it('replaces a selection that stays on one line, like any other keystroke', () => {
    const out = apply('const x = 1', 6, 7)
    expect(out?.text).toBe('const ' + INDENT + ' = 1')
  })

  it('indents every line of a multi-line selection instead of replacing them', () => {
    const out = apply('a\nb\nc', 0, 3)
    expect(out?.text).toBe('  a\n  b\nc')
  })

  it('indents the whole of the first and last line, however partial the selection', () => {
    // 选区从 `a` 的第 2 个字符开始，缩进仍然要落在行首。
    const out = apply('aa\nbb', 1, 4)
    expect(out?.text).toBe('  aa\n  bb')
  })

  it('leaves blank lines blank', () => {
    // 否则保存后的 diff 里全是看不见的尾随空白。
    const out = apply('a\n\nb', 0, 4)
    expect(out?.text).toBe('  a\n\n  b')
  })

  it('does not drag in the line after a selection that stops at the newline', () => {
    const out = apply('a\nb\nc', 0, 4)
    expect(out?.text).toBe('  a\n  b\nc')
  })

  it('keeps the whole block selected afterwards', () => {
    const out = apply('a\nb', 0, 3)
    expect(out?.selStart).toBe(0)
    expect(out?.selEnd).toBe(7)
  })
})

describe('Shift-Tab', () => {
  it('removes one level from the cursor line with no selection at all', () => {
    const out = apply('    x', 5, 5, true)
    expect(out?.text).toBe('  x')
    expect(out?.selStart).toBe(3)
  })

  it('eats a full tab character as one level', () => {
    const out = apply('\tx', 2, 2, true)
    expect(out?.text).toBe('x')
  })

  it('eats a single stray space rather than refusing', () => {
    const out = apply(' x', 2, 2, true)
    expect(out?.text).toBe('x')
  })

  it('does nothing when no line in range has anything to give back', () => {
    expect(indentEdit('x\ny', 0, 3, true)).toBeNull()
  })

  it('outdents a block, skipping the lines that have no indent left', () => {
    const out = apply('  a\nb\n  c', 0, 9, true)
    expect(out?.text).toBe('a\nb\nc')
  })

  it('never pulls the cursor in front of its own line', () => {
    // 光标在 `  x` 的第 1 列，这一行退掉 2 格，光标不能算成 -1。
    const out = apply('a\n  x', 2, 2, true)
    expect(out?.text).toBe('a\nx')
    expect(out?.selStart).toBe(2)
  })
})

describe('Enter', () => {
  it('carries the leading whitespace of the line down', () => {
    expect(newlineIndent('    foo', 7)).toBe('\n    ')
  })

  it('carries tabs as tabs', () => {
    expect(newlineIndent('\t\tfoo', 5)).toBe('\n\t\t')
  })

  it('is a bare newline on an unindented line', () => {
    expect(newlineIndent('foo', 3)).toBe('\n')
  })

  it('reads the indent of the line the cursor is on, not the first line', () => {
    expect(newlineIndent('a:\n  b: 1', 9)).toBe('\n  ')
  })

  it('takes the indent from the text before the cursor, not the whole line', () => {
    // 光标在缩进中间时，后面那半截空白会跟着换行走，不该再复制一遍。
    expect(newlineIndent('    foo', 2)).toBe('\n  ')
  })
})
