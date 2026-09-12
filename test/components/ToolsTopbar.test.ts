import { beforeEach, describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ToolsTopbar from '../../src/components/topbar/ToolsTopbar.vue'
import { vTooltip } from '../../src/tooltip'
import { setLang } from '../../src/settings'
import { resetToolsPanel, switchToolsTab, toolsQuery } from '../../src/toolsPanel'

beforeEach(() => {
  setLang('en')
  resetToolsPanel()
})

const factory = () =>
  mount(ToolsTopbar, { global: { directives: { tooltip: vTooltip } } })

describe('ToolsTopbar', () => {
  it('搜索框落在和别的视图同一个位置（.ct-search）', () => {
    expect(factory().find('.ct-search .ct-search-input').exists()).toBe(true)
  })

  it('placeholder 跟着当前面板走 —— 四个面板搜的不是一类东西', async () => {
    const w = factory()
    expect(w.find('.ct-search-input').attributes('placeholder')).toBe('Search skills…')
    switchToolsTab('mcp')
    await w.vm.$nextTick()
    expect(w.find('.ct-search-input').attributes('placeholder')).toBe('Search servers…')
  })

  it('打字防抖写进共享 ref，清空按钮立刻生效', async () => {
    const w = factory()
    const input = w.find('.ct-search-input')
    await input.setValue('hyper')
    // 防抖没到点之前共享 ref 还是空的，但输入框已经显示了。
    expect((input.element as HTMLInputElement).value).toBe('hyper')
    await new Promise((r) => setTimeout(r, 260))
    expect(toolsQuery.value).toBe('hyper')
    await w.find('.ct-search .ct-btn').trigger('click')
    expect(toolsQuery.value).toBe('')
  })

  it('切面板清掉的搜索词也要从输入框里消失', async () => {
    const w = factory()
    await w.find('.ct-search-input').setValue('hyper')
    await new Promise((r) => setTimeout(r, 260))
    switchToolsTab('hooks')
    await w.vm.$nextTick()
    expect((w.find('.ct-search-input').element as HTMLInputElement).value).toBe('')
  })

  it('关闭按钮 emit close，由 App 决定收哪一层', async () => {
    const w = factory()
    await w.find('.tools-topbar-close').trigger('click')
    expect(w.emitted('close')).toHaveLength(1)
  })
})
