import { beforeEach, describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ToolResult from '../../src/components/ToolResult.vue'
import type { Block } from '../../src/types'
import { setLang } from '../../src/settings'
import {
  DIFF_HIGHLIGHT_MAX_CHARS,
  JSON_HIGHLIGHT_MAX_CHARS,
  OVERSIZE_BLOCK_CHARS,
} from '../../src/renderLimits'

beforeEach(() => setLang('en'))

function blk(over: Partial<Block> & { kind: Block['kind'] }): Block {
  return { isError: false, ...over }
}

describe('ToolResult', () => {
  it('labels a plain result and stays collapsed', () => {
    const wrapper = mount(ToolResult, {
      props: { block: blk({ kind: 'tool_result', text: 'output' }) },
    })
    expect(wrapper.find('.label').text()).toBe('Tool result')
    expect(wrapper.find('details').attributes('open')).toBeUndefined()
    expect(wrapper.find('pre').text()).toBe('output')
  })

  it('marks an error result', () => {
    const wrapper = mount(ToolResult, {
      props: { block: blk({ kind: 'tool_result', text: 'bad', isError: true }) },
    })
    expect(wrapper.find('.thinking-label').text()).toBe('Tool result · error')
    expect(wrapper.find('details').classes()).toContain('thinking-block')
    expect(wrapper.find('details').classes()).toContain('tool-result-error')
    expect(wrapper.find('.thinking-icon').exists()).toBe(true)
  })

  it('renders a Bash textual unified diff as an expanded file-change card', () => {
    const wrapper = mount(ToolResult, {
      props: {
        block: blk({
          kind: 'tool_result',
          text: 'diff --git a/lib/routes/example.dart b/lib/routes/example.dart\nindex abc..def 100644\n--- a/lib/routes/example.dart\n+++ b/lib/routes/example.dart\n@@ -1 +1,2 @@\n old\n+new',
        }),
      },
    })

    expect(wrapper.find('.label').text()).toBe('File change · example.dart')
    expect(wrapper.find('details').attributes('open')).toBeDefined()
    expect(wrapper.find('details').classes()).toContain('text-diff-result')
    expect(wrapper.find('.diff-stat').text()).toBe('+1 −0')
    expect(wrapper.find('.diff-add').text()).toBe('+new')
  })

  it('renders a diff result as a codex patch card', () => {
    const wrapper = mount(ToolResult, {
      props: {
        block: blk({
          kind: 'tool_result',
          filePath: '/deep/nested/file.ts',
          fileChangeType: 'delete',
          diff: [
            {
              oldStart: 1,
              newStart: 1,
              lines: [
                { kind: 'add', oldNo: null, newNo: 1, text: 'x' },
                { kind: 'add', oldNo: null, newNo: 2, text: 'y' },
                { kind: 'del', oldNo: 1, newNo: null, text: 'z' },
              ],
            },
          ],
        }),
      },
    })
    expect(wrapper.find('details').exists()).toBe(false)
    expect(wrapper.find('.codex-patch-file').exists()).toBe(true)
    expect(wrapper.find('.codex-patch-path').text()).toBe('/deep/nested/file.ts')
    expect(wrapper.find('.codex-patch-op').text()).toBe('Deleted')
    expect(wrapper.find('.codex-patch-stat').text()).toBe('+2-1')
    expect(wrapper.find('.codex-patch-line.add').exists()).toBe(true)
    expect(wrapper.find('.codex-patch-line.del').exists()).toBe(true)
  })

  it('renders a filePath-only result as an empty codex patch card', () => {
    const wrapper = mount(ToolResult, {
      props: {
        block: blk({
          kind: 'tool_result',
          filePath: '/deep/nested/empty.md',
          fileChangeType: 'delete',
          text: 'File changed.',
        }),
      },
    })
    expect(wrapper.find('details').exists()).toBe(false)
    expect(wrapper.find('.codex-patch-path').text()).toBe('/deep/nested/empty.md')
    expect(wrapper.find('.codex-patch-op').text()).toBe('Deleted')
    expect(wrapper.text()).not.toContain('File changed.')
  })

  it('omits the diff-stat element when there is no diff', () => {
    const wrapper = mount(ToolResult, {
      props: { block: blk({ kind: 'tool_result', text: 'plain' }) },
    })
    expect(wrapper.find('.diff-stat').exists()).toBe(false)
  })

  it('adds the in-user modifier class when inUser is set', () => {
    const wrapper = mount(ToolResult, {
      props: { block: blk({ kind: 'tool_result', text: 'o' }), inUser: true },
    })
    expect(wrapper.find('details').classes()).toContain('in-user')
  })

  // ---- 体积闸门 ----
  // 染色是按行建 DOM 的，一个 5 MB 的 tool 输出能生成百万级 <span>，内存是原文的
  // 几十倍。超限跳过染色、默认折叠、标注体积；内容一个字都不会少。

  function hugeDiff(chars: number): string {
    const head = 'diff --git a/big.txt b/big.txt\n--- a/big.txt\n+++ b/big.txt\n@@ -1 +1,2 @@\n'
    const body = '+line of added text\n'.repeat(Math.ceil(chars / 20))
    return head + body
  }

  it('skips diff colouring past the size gate but keeps the diff identity', () => {
    const text = hugeDiff(DIFF_HIGHLIGHT_MAX_CHARS + 1000)
    expect(text.length).toBeGreaterThan(DIFF_HIGHLIGHT_MAX_CHARS)
    const wrapper = mount(ToolResult, { props: { block: blk({ kind: 'tool_result', text }) } })

    // 仍然按「文件改动」标题和 diff 语义呈现，只是没有行级染色。
    expect(wrapper.find('.label').text()).toBe('File change · big.txt')
    expect(wrapper.find('details').classes()).toContain('text-diff-result')
    expect(wrapper.find('pre.lang-diff').exists()).toBe(false)
    // 用 textContent 而不是 .text()：后者会 trim，而这里要证明的正是"一个字都没少"。
    expect(wrapper.find('pre').element.textContent).toBe(text)
  })

  it('skips JSON colouring past the size gate', () => {
    const text = JSON.stringify({ items: Array.from({ length: 40000 }, (_, i) => `item-${i}`) })
    expect(text.length).toBeGreaterThan(JSON_HIGHLIGHT_MAX_CHARS)
    const wrapper = mount(ToolResult, { props: { block: blk({ kind: 'tool_result', text }) } })

    expect(wrapper.find('pre.lang-json').exists()).toBe(false)
    expect(wrapper.find('pre').element.textContent).toBe(text)
  })

  it('folds an oversize block and labels its size', () => {
    const text = 'x'.repeat(OVERSIZE_BLOCK_CHARS + 1)
    const wrapper = mount(ToolResult, { props: { block: blk({ kind: 'tool_result', text }) } })

    expect(wrapper.find('.oversize-hint').exists()).toBe(true)
    expect(wrapper.find('.oversize-hint').text()).toContain('large')
    expect(wrapper.find('details').attributes('open')).toBeUndefined()
  })

  it('keeps an oversize diff folded even though diffs normally auto-open', () => {
    const wrapper = mount(ToolResult, {
      props: { block: blk({ kind: 'tool_result', text: hugeDiff(OVERSIZE_BLOCK_CHARS + 1000) }) },
    })
    expect(wrapper.find('details').attributes('open')).toBeUndefined()
    expect(wrapper.find('.oversize-hint').exists()).toBe(true)
  })

  it('leaves a normal-sized block untouched', () => {
    const wrapper = mount(ToolResult, {
      props: { block: blk({ kind: 'tool_result', text: 'small output' }) },
    })
    expect(wrapper.find('.oversize-hint').exists()).toBe(false)
  })
})
