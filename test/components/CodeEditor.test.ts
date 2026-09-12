import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import CodeEditor from '../../src/components/CodeEditor.vue'

const highlightLines = vi.hoisted(() => vi.fn())
vi.mock('../../src/shikiHighlight', () => ({ highlightLines }))

/**
 * jsdom 没有 `execCommand`，而组件在真实浏览器里就靠它写入（保住原生撤销栈）。
 * 这里补一个最小实现，顺带记下每一次调用 —— 「有没有走 execCommand」本身就是要测的。
 */
function stubExecCommand(ta: HTMLTextAreaElement) {
  const calls: string[] = []
  ;(document as unknown as { execCommand: unknown }).execCommand = (
    cmd: string,
    _ui: boolean,
    text: string,
  ) => {
    calls.push(text)
    const { selectionStart: s, selectionEnd: e } = ta
    ta.value = ta.value.slice(0, s) + text + ta.value.slice(e)
    ta.setSelectionRange(s + text.length, s + text.length)
    ta.dispatchEvent(new Event('input'))
    return true
  }
  return calls
}

function press(ta: HTMLTextAreaElement, key: string, opts: KeyboardEventInit = {}) {
  ta.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...opts }))
}

async function editor(modelValue: string, props: Record<string, unknown> = {}) {
  const wrapper = mount(CodeEditor, { props: { modelValue, ...props }, attachTo: document.body })
  await nextTick()
  const ta = wrapper.get('textarea').element as HTMLTextAreaElement
  return { wrapper, ta }
}

/** 最后一次 `update:modelValue` 的内容。 */
function latest(wrapper: { emitted: (e: string) => unknown[][] | undefined }) {
  const events = wrapper.emitted('update:modelValue')
  return events ? (events[events.length - 1][0] as string) : undefined
}

beforeEach(() => {
  highlightLines.mockReset()
  highlightLines.mockResolvedValue(null)
})

afterEach(() => {
  delete (document as unknown as { execCommand?: unknown }).execCommand
  document.body.innerHTML = ''
})

describe('the two layers', () => {
  it('shows escaped plain text before any highlighting has run', async () => {
    const { wrapper } = await editor('<a> & </a>')
    expect(wrapper.get('.ce-hl').element.innerHTML).toBe('&lt;a&gt; &amp; &lt;/a&gt;\n')
  })

  it('upgrades to tokens once shiki comes back', async () => {
    highlightLines.mockResolvedValue([[{ content: 'const', color: '#abc' }]])
    const { wrapper } = await editor('const', { lang: 'ts' })
    await vi.waitFor(() =>
      expect(wrapper.get('.ce-hl').element.innerHTML).toContain('color:#abc'),
    )
  })

  it('never asks shiki anything when there is no language', async () => {
    await editor('const x = 1')
    expect(highlightLines).not.toHaveBeenCalled()
  })

  it('stays on plain text when shiki declines the language', async () => {
    highlightLines.mockResolvedValue(null)
    const { wrapper } = await editor('zzz', { lang: 'brainfuck' })
    await vi.waitFor(() => expect(highlightLines).toHaveBeenCalled())
    expect(wrapper.get('.ce-hl').element.innerHTML).toBe('zzz\n')
  })

  it('stays on plain text when shiki throws', async () => {
    // 超限 / 语言加载失败都会走到这里，编辑器本身不能跟着挂掉。
    highlightLines.mockRejectedValue(new Error('too big'))
    const { wrapper } = await editor('boom', { lang: 'ts' })
    await vi.waitFor(() => expect(highlightLines).toHaveBeenCalled())
    expect(wrapper.get('.ce-hl').element.innerHTML).toBe('boom\n')
  })

  it('drops a highlight result that came back for text the user has already changed', async () => {
    // 异步跑完时文本已经变了，把旧 token 贴上去就是整屏错位。
    let release: (v: unknown) => void = () => {}
    highlightLines.mockReturnValue(new Promise((r) => (release = r)))
    const { wrapper } = await editor('old', { lang: 'ts' })
    await vi.waitFor(() => expect(highlightLines).toHaveBeenCalled())
    await wrapper.setProps({ modelValue: 'new' })
    release([[{ content: 'old', color: '#f00' }]])
    await nextTick()
    expect(wrapper.get('.ce-hl').element.innerHTML).not.toContain('#f00')
  })
})

describe('typing', () => {
  it('emits what the user typed', async () => {
    const { wrapper, ta } = await editor('a')
    ta.value = 'ab'
    await wrapper.get('textarea').trigger('input')
    expect(latest(wrapper)).toBe('ab')
  })
})

describe('Tab', () => {
  it('writes through execCommand so the native undo stack survives', async () => {
    // 直接赋值 `textarea.value` 会把撤销栈清空，用户一次 ⌘Z 就退回打开文件时的样子。
    const { ta } = await editor('x')
    const calls = stubExecCommand(ta)
    ta.setSelectionRange(0, 0)
    press(ta, 'Tab')
    expect(calls).toEqual(['  '])
    expect(ta.value).toBe('  x')
  })

  it('still works where execCommand does not exist', async () => {
    const { wrapper, ta } = await editor('x')
    ta.setSelectionRange(0, 0)
    press(ta, 'Tab')
    expect(ta.value).toBe('  x')
    expect(latest(wrapper)).toBe('  x')
  })

  it('indents a whole selected block', async () => {
    const { ta } = await editor('a\nb')
    stubExecCommand(ta)
    ta.setSelectionRange(0, 3)
    press(ta, 'Tab')
    expect(ta.value).toBe('  a\n  b')
  })

  it('outdents on Shift-Tab', async () => {
    const { ta } = await editor('    x')
    stubExecCommand(ta)
    ta.setSelectionRange(5, 5)
    press(ta, 'Tab', { shiftKey: true })
    expect(ta.value).toBe('  x')
  })

  it('leaves the text alone when there is nothing left to outdent', async () => {
    const { ta } = await editor('x')
    stubExecCommand(ta)
    ta.setSelectionRange(1, 1)
    press(ta, 'Tab', { shiftKey: true })
    expect(ta.value).toBe('x')
  })

  it('swallows the key so focus does not jump to the next control', async () => {
    const { ta } = await editor('x')
    stubExecCommand(ta)
    const e = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true })
    ta.dispatchEvent(e)
    expect(e.defaultPrevented).toBe(true)
  })

  it('does nothing at all when read-only', async () => {
    const { ta } = await editor('x', { readonly: true })
    stubExecCommand(ta)
    ta.setSelectionRange(0, 0)
    const e = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true })
    ta.dispatchEvent(e)
    expect(ta.value).toBe('x')
    expect(e.defaultPrevented).toBe(false)
  })
})

describe('Enter', () => {
  it('carries the current indent down to the new line', async () => {
    const { ta } = await editor('  a')
    stubExecCommand(ta)
    ta.setSelectionRange(3, 3)
    press(ta, 'Enter')
    expect(ta.value).toBe('  a\n  ')
  })

  it('leaves an unindented line to the browser', async () => {
    // 没有缩进要续时不接管，少一次 DOM 往返，也少一条撤销记录。
    const { ta } = await editor('a')
    stubExecCommand(ta)
    ta.setSelectionRange(1, 1)
    const e = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
    ta.dispatchEvent(e)
    expect(e.defaultPrevented).toBe(false)
  })
})

describe('save', () => {
  it('emits save on the platform shortcut', async () => {
    const { wrapper, ta } = await editor('x')
    press(ta, 's', { metaKey: true })
    press(ta, 'S', { ctrlKey: true })
    expect(wrapper.emitted('save')).toHaveLength(2)
  })

  it('does not emit save when read-only', async () => {
    const { wrapper, ta } = await editor('x', { readonly: true })
    press(ta, 's', { metaKey: true })
    expect(wrapper.emitted('save')).toBeUndefined()
  })
})

describe('scrolling', () => {
  it('drags the highlight layer along with the textarea', async () => {
    const { wrapper, ta } = await editor('a\n'.repeat(200))
    ta.scrollTop = 120
    ta.scrollLeft = 40
    await wrapper.get('textarea').trigger('scroll')
    const hl = wrapper.get('.ce-hl').element
    expect(hl.scrollTop).toBe(120)
    expect(hl.scrollLeft).toBe(40)
  })
})

describe('an empty file', () => {
  it('un-hides the textarea text so the caret has something to sit in', async () => {
    const { wrapper } = await editor('')
    expect(wrapper.get('textarea').classes()).toContain('empty')
    await wrapper.setProps({ modelValue: 'a' })
    expect(wrapper.get('textarea').classes()).not.toContain('empty')
  })
})
