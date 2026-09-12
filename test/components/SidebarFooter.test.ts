// 侧栏底部常驻的那一行。抽出来就是为了工具管理开着时它还在 —— 所以这里盯的是
// 「两种形态渲染的是同一行」，以及扳手在工具态下发的是关闭而不是再打开一次。

import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import SidebarFooter from '../../src/components/SidebarFooter.vue'
import { vTooltip } from '../../src/tooltip'
import { latestVersion, updateAvailable } from '../../src/updateCheck'

const factory = (props: Partial<InstanceType<typeof SidebarFooter>['$props']> = {}) =>
  mount(SidebarFooter, {
    props,
    global: { directives: { tooltip: vTooltip } },
  })

describe('SidebarFooter', () => {
  it('设置和工具管理都在，工具管理是兄弟节点不是嵌套按钮', () => {
    const wrapper = factory()
    expect(wrapper.find('.trash-tab').exists()).toBe(true)
    // 嵌套 <button> 是非法 HTML，而设置那颗里面已经有一个 role=button 的 release 入口。
    expect(wrapper.find('.trash-tab .sidebar-tools-btn').exists()).toBe(false)
    expect(wrapper.find('.sidebar-footer > .sidebar-tools-btn').exists()).toBe(true)
  })

  it('点设置发 open-settings，点扳手发 toggle-tools', async () => {
    const wrapper = factory()
    await wrapper.find('.trash-tab').trigger('click')
    await wrapper.find('.sidebar-tools-btn').trigger('click')
    expect(wrapper.emitted('open-settings')).toHaveLength(1)
    expect(wrapper.emitted('toggle-tools')).toHaveLength(1)
  })

  it('工具管理开着时扳手是 active 态', () => {
    expect(factory().find('.sidebar-tools-btn').classes()).not.toContain('active')
    expect(factory({ toolsActive: true }).find('.sidebar-tools-btn').classes()).toContain('active')
  })

  it('有新版本时才出现 release 入口和红点', async () => {
    expect(factory().find('.sidebar-release-btn').exists()).toBe(false)

    updateAvailable.value = true
    latestVersion.value = '9.9.9'
    try {
      const wrapper = factory()
      expect(wrapper.find('.sidebar-release-btn').exists()).toBe(true)
      expect(wrapper.find('.update-dot').exists()).toBe(true)
      // release 入口点的是「更新」tab，不是通用设置 —— 且不能冒泡到外层。
      await wrapper.find('.sidebar-release-btn').trigger('click')
      expect(wrapper.emitted('open-settings')).toEqual([['updates']])
    } finally {
      updateAvailable.value = false
      latestVersion.value = null
    }
  })
})
