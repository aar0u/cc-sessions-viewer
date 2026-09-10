import { beforeEach, describe, expect, it, vi } from 'vitest'
import { setTheme } from '../src/settings'

const mocks = vi.hoisted(() => ({
  initialize: vi.fn(),
  render: vi.fn(),
}))

vi.mock('mermaid', () => ({
  default: {
    initialize: mocks.initialize,
    render: mocks.render,
  },
}))

import { mermaidCacheSize, renderAllMermaid, resetMermaidForTheme } from '../src/mermaid'
import { MERMAID_CACHE_MAX_ENTRIES } from '../src/renderLimits'

function mermaidPlaceholder(source: string): string {
  return `<div class="md-mermaid" data-source="${encodeURIComponent(source)}"></div>`
}

describe('renderAllMermaid', () => {
  beforeEach(() => {
    mocks.initialize.mockReset()
    mocks.render.mockReset()
    mocks.render.mockImplementation(async (_id: string, source: string) => ({
      svg: `<svg viewBox="0 0 120 40"><text>${source}</text></svg>`,
    }))
    setTheme('light')
  })

  it('renders the replacement placeholder when its rich-text lifecycle runs after a tab change', async () => {
    const root = document.createElement('div')
    root.innerHTML = mermaidPlaceholder('flowchart TD\nA-->B')

    const firstRender = renderAllMermaid(root)
    // v-rich-html runs again after Vue replaces the session HTML in this node.
    root.innerHTML = mermaidPlaceholder('flowchart TD\nC-->D')
    const currentRender = renderAllMermaid(root)
    await Promise.all([firstRender, currentRender])

    expect(mocks.render).toHaveBeenCalledTimes(2)
    expect(mocks.render).toHaveBeenLastCalledWith(
      expect.any(String),
      'flowchart TD\nC-->D',
    )
    expect(root.querySelector('.md-mermaid')?.dataset.rendered).toBe('1')
    expect(root.querySelector('svg')).not.toBeNull()
  })

  it('reuses a rendered SVG when virtual scrolling recreates the same message row', async () => {
    const source = 'flowchart TD\nVirtualized-->Cached'
    const root = document.createElement('div')
    root.innerHTML = mermaidPlaceholder(source)

    await renderAllMermaid(root)
    root.innerHTML = mermaidPlaceholder(source)
    await renderAllMermaid(root)

    expect(mocks.render).toHaveBeenCalledTimes(1)
    expect(root.querySelector('.md-mermaid')?.dataset.rendered).toBe('1')
    expect(root.querySelector('svg')).not.toBeNull()
  })
})

// 渲染结果缓存过去只增不减（只在切主题时整体清空）。一份 mermaid SVG 动辄几百 KB，
// 长会话滚一遍就能攒出几百 MB 并永久驻留。
describe('mermaid render cache', () => {
  beforeEach(() => {
    resetMermaidForTheme(null)
    mocks.render.mockReset()
    mocks.render.mockImplementation(async (_id: string, source: string) => ({
      svg: `<svg viewBox="0 0 120 40"><text>${source}</text></svg>`,
    }))
    setTheme('light')
  })

  it('reuses a cached diagram instead of laying it out again', async () => {
    const root = document.createElement('div')
    root.innerHTML = mermaidPlaceholder('flowchart TD\nA-->B')
    await renderAllMermaid(root)

    const second = document.createElement('div')
    second.innerHTML = mermaidPlaceholder('flowchart TD\nA-->B')
    await renderAllMermaid(second)

    expect(mocks.render).toHaveBeenCalledTimes(1)
    expect(mermaidCacheSize().entries).toBe(1)
  })

  it('evicts the oldest diagrams once past the entry limit', async () => {
    for (let i = 0; i < MERMAID_CACHE_MAX_ENTRIES + 5; i++) {
      const root = document.createElement('div')
      root.innerHTML = mermaidPlaceholder(`flowchart TD\nN${i}-->X`)
      await renderAllMermaid(root)
    }
    expect(mermaidCacheSize().entries).toBeLessThanOrEqual(MERMAID_CACHE_MAX_ENTRIES)

    // 最早那张已被淘汰 → 再看到它要重画一次，而不是白白留在内存里。
    const callsBefore = mocks.render.mock.calls.length
    const root = document.createElement('div')
    root.innerHTML = mermaidPlaceholder('flowchart TD\nN0-->X')
    await renderAllMermaid(root)
    expect(mocks.render.mock.calls.length).toBe(callsBefore + 1)
  })

  it('drops the whole cache when the theme changes', async () => {
    const root = document.createElement('div')
    root.innerHTML = mermaidPlaceholder('flowchart TD\nA-->B')
    await renderAllMermaid(root)
    expect(mermaidCacheSize().entries).toBe(1)

    resetMermaidForTheme(null)
    expect(mermaidCacheSize()).toEqual({ entries: 0, chars: 0 })
  })
})
