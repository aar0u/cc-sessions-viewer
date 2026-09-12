import { beforeEach, describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ToolsNav from '../../src/components/ToolsNav.vue'
import { vTooltip } from '../../src/tooltip'
import { ALL_AGENTS, setLang } from '../../src/settings'
import {
  TOOL_TABS,
  resetToolsPanel,
  toolsQuery,
  toolsTab,
} from '../../src/toolsPanel'

// 工具管理开着时这个组件顶掉项目侧栏，所以它必须长得像侧栏：同样的 .sidebar 宽度、
// 上面一排 agent、下面一列入口。判断逻辑都在 toolsPanel.ts，这里只钉住渲染契约。

beforeEach(() => {
  setLang('en')
  resetToolsPanel()
})

const factory = () =>
  mount(ToolsNav, { global: { directives: { tooltip: vTooltip } } })

describe('ToolsNav', () => {
  it('复用侧栏的类名，好让宽度和拖动分隔条继续生效', () => {
    const w = factory()
    expect(w.find('aside.sidebar').exists()).toBe(true)
    expect(w.find('.sidebar-top').exists()).toBe(true)
    expect(w.find('.proj-list').exists()).toBe(true)
  })

  it('四个入口各一行，次序跟 TOOL_TABS 走', () => {
    const items = factory().findAll('.tools-nav-item')
    expect(items).toHaveLength(TOOL_TABS.length)
    expect(items.map((i) => i.text())).toEqual([
      'MCP',
      'Skills',
      'Hooks',
      'Global config',
    ])
  })

  it('当前面板那一行是 active', () => {
    const w = factory()
    const items = w.findAll('.tools-nav-item')
    // 默认停在 Skills（TOOL_TABS 里的第 2 个）。
    expect(items[1].classes()).toContain('active')
    expect(items[0].classes()).not.toContain('active')
  })

  it('点一行切面板，并把上一个面板的搜索词清掉', async () => {
    const w = factory()
    toolsQuery.value = 'hyperframes'
    await w.findAll('.tools-nav-item')[0].trigger('click')
    expect(toolsTab.value).toBe('mcp')
    expect(toolsQuery.value).toBe('')
  })

  it('agent 过滤器列全部 agent —— 会话列表里藏起来的，工具照样要能管', () => {
    expect(factory().findAll('.tools-agent')).toHaveLength(ALL_AGENTS.length)
  })

  it('没勾中的 agent 压暗而不是消失，用户要看得见自己关了哪几家', async () => {
    const w = factory()
    const agents = w.findAll('.tools-agent')
    expect(agents.every((a) => !a.classes().includes('off'))).toBe(true)
    await agents[0].trigger('click')
    expect(w.findAll('.tools-agent')[0].classes()).not.toContain('off')
    expect(w.findAll('.tools-agent')[1].classes()).toContain('off')
    expect(w.find('.sidebar-sub').text()).toContain('1 shown')
  })

  it('重置按钮只在动过过滤器之后出现，点完留在当前面板', async () => {
    const w = factory()
    toolsTab.value = 'hooks'
    expect(w.find('.sidebar-sub .sidebar-sub-btn').exists()).toBe(false)
    await w.findAll('.tools-agent')[0].trigger('click')
    const reset = w.find('.sidebar-sub .sidebar-sub-btn')
    expect(reset.exists()).toBe(true)
    await reset.trigger('click')
    expect(w.findAll('.tools-agent').every((a) => !a.classes().includes('off'))).toBe(true)
    expect(toolsTab.value).toBe('hooks')
  })

  it('底部那一行照常在 —— 工具管理顶掉的是项目列表，不是「设置」这个入口', async () => {
    const w = factory()
    expect(w.find('.sidebar-footer .trash-tab').exists()).toBe(true)
    // 扳手在这儿是「当前位置」，点它退回会话而不是再打开一次。
    const wrench = w.find('.sidebar-tools-btn')
    expect(wrench.classes()).toContain('active')
    await wrench.trigger('click')
    expect(w.emitted('close')).toHaveLength(1)
  })
})
