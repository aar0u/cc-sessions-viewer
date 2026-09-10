import { describe, expect, it } from 'vitest'
import { highlightAllCodeBlocks, rehighlightAllCodeBlocks } from '../src/shikiHighlight'
import { SHIKI_MAX_CHARS } from '../src/renderLimits'

function fenced(lang: string, code: string): HTMLElement {
  const root = document.createElement('div')
  const pre = document.createElement('pre')
  pre.className = 'code-block'
  pre.dataset.lang = lang
  const el = document.createElement('code')
  el.textContent = code
  pre.appendChild(el)
  root.appendChild(pre)
  return root
}

describe('highlightAllCodeBlocks', () => {
  it('highlights a normal code block', async () => {
    const root = fenced('typescript', 'const a = 1')
    await highlightAllCodeBlocks(root)
    const pre = root.querySelector('pre')!
    expect(pre.dataset.shiki).toBe('done')
    expect(pre.className).toContain('shiki')
  })

  // 体积闸门：高亮把每个 token 变成一个 <span>，一份 5 MB 的代码能生成百万级节点，
  // 内存是原文的几十倍。超限退回纯 <pre>，内容一个字不少。
  it('skips a block past the size gate and leaves the text intact', async () => {
    const huge = 'const a = 1\n'.repeat(Math.ceil(SHIKI_MAX_CHARS / 12) + 1)
    expect(huge.length).toBeGreaterThan(SHIKI_MAX_CHARS)
    const root = fenced('typescript', huge)
    await highlightAllCodeBlocks(root)
    const pre = root.querySelector('pre')!
    expect(pre.dataset.shiki).toBe('skip')
    expect(pre.className).not.toContain('shiki')
    expect(pre.querySelector('code')?.textContent).toBe(huge)
  })

  // 高亮后的 <pre> 不再另存一份 data-source —— 那份 encodeURIComponent 副本比原文还大，
  // 而且随节点常驻。主题切换重画和「复制代码」都改读 textContent，前提是它逐字等于原文。
  it('does not keep a second copy of the source on the element', async () => {
    const root = fenced('typescript', 'const a = 1')
    await highlightAllCodeBlocks(root)
    expect(root.querySelector('pre')!.dataset.source).toBeUndefined()
  })

  it('round-trips the source through textContent across repeated rehighlights', async () => {
    const cases = [
      'const a = 1',
      'const a = 1\nconst b = 2',
      'const a = 1\n',
      'function f() {\n  return "<b>&amp;</b>"\n}\n\nf()',
      'const s = "中文 🎉"',
      '\tindented\n\t\tdeeper',
    ]
    for (const code of cases) {
      const root = fenced('typescript', code)
      await highlightAllCodeBlocks(root)
      expect(root.querySelector('pre')!.textContent).toBe(code)
      // 主题切换会连着重画好几轮；每一轮都必须还原出同一份源码，不能越切越歪。
      for (let pass = 0; pass < 3; pass++) {
        await rehighlightAllCodeBlocks(root)
        expect(root.querySelector('pre')!.textContent).toBe(code)
      }
    }
  })
})
