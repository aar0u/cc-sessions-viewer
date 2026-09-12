<script setup lang="ts">
// 工具管理的导航栏 —— 面板开着时顶掉项目侧栏，形状和它一模一样：
// 上面一排 agent（那儿平时是 agent 切换器），下面一列入口（那儿平时是项目列表）。
//
// 复用 .sidebar / .sidebar-top / .proj-list / .proj-item 这几个类不是偷懒：宽度跟着
// --sidebar-w 走、拖动分隔条一样有效、hover/active 的底色和圆角也不会和项目列表对不上。
import { computed, onMounted, type Component } from 'vue'
import type { Agent } from '../types'
import { t } from '../i18n'
import * as api from '../api'
import { agentLabel } from '../agentMeta'
import { agentIcons, IconCompass, IconFileDoc, IconLayers, IconRefresh, IconSkill, IconWebhook } from './icons'
import SidebarFooter from './SidebarFooter.vue'
import {
  TAB_LABEL,
  TOOL_TABS,
  type ToolTab,
  clearToolsFilter,
  ensureInstalledAgents,
  isAgentOn,
  panelAgents,
  switchToolsTab,
  tabUsesAgentFilter,
  toggleToolsAgent,
  toolsFilterDirty,
  toolsTab,
} from '../toolsPanel'

const emit = defineEmits<{
  (e: 'open-settings', tab?: 'general' | 'updates'): void
  (e: 'close'): void
}>()

// 五个入口各自的图标。次序由 TOOL_TABS 定，这里只管长相。
const TAB_ICON: Record<ToolTab, Component> = {
  mcp: IconLayers,
  skills: IconSkill,
  discover: IconCompass,
  hooks: IconWebhook,
  memo: IconFileDoc,
}

// 过滤的是「管哪几家的工具」，和侧栏那个「现在看哪家的会话」是两回事，所以这里列
// 不是 visibleAgents —— 会话列表里藏起来的 agent，工具照样要能管；但也不是全部七家，
// 本机没装的（`~/.claude` 之类的配置目录都不存在）列出来只会是个点不出东西的空壳。
const agents = computed(() => panelAgents())
const shownCount = computed(() => agents.value.filter(isAgentOn).length)
const agentFilterOn = computed(() => tabUsesAgentFilter(toolsTab.value))

onMounted(() => void ensureInstalledAgents(() => api.toolSurfaces()))

function agentTip(agent: Agent) {
  return agentLabel(agent, true)
}
</script>

<template>
  <aside class="sidebar tools-nav" :aria-label="t('tools.title')">
    <div class="sidebar-top">
      <!-- agent 过滤器。列表项右侧那排角标也按这个次序，所以两处要用同一份 `agents`。 -->
      <!-- 「发现」面板搜的是 skills.sh，跟本机勾了哪几家没关系 —— 整排压暗不可点。
           不压暗的话用户会以为勾掉几家能筛掉一些结果，点了却毫无反应。 -->
      <div class="tools-agent-row" :class="{ inert: !agentFilterOn }">
        <button
          v-for="a in agents"
          :key="a"
          class="tools-agent"
          :class="{ off: !isAgentOn(a) }"
          :aria-pressed="isAgentOn(a)"
          :disabled="!agentFilterOn"
          v-tooltip="agentFilterOn ? agentTip(a) : t('tools.agentFilterOff')"
          @click="toggleToolsAgent(a)"
        >
          <component :is="agentIcons[a]" />
        </button>
      </div>
      <div class="sidebar-sub">
        <span class="sidebar-sub-label">
          {{ t('tools.agentFilter') }} ·
          {{ agentFilterOn ? t('tools.agentFilterHint', { n: String(shownCount) }) : t('tools.agentFilterOff') }}
        </span>
        <!-- 只在动过过滤器时出现：清掉 agent 勾选和搜索词，但**留在当前面板**。 -->
        <button
          v-if="toolsFilterDirty()"
          type="button"
          class="sidebar-sub-btn"
          v-tooltip="t('tools.filterReset')"
          :aria-label="t('tools.filterReset')"
          @click="clearToolsFilter()"
        >
          <IconRefresh />
        </button>
      </div>
    </div>

    <nav class="proj-list tools-nav-list">
      <button
        v-for="tab in TOOL_TABS"
        :key="tab"
        type="button"
        class="proj-item tools-nav-item"
        :class="{ active: toolsTab === tab }"
        :aria-current="toolsTab === tab ? 'page' : undefined"
        @click="switchToolsTab(tab)"
      >
        <component :is="TAB_ICON[tab]" class="tools-nav-ic" />
        <span class="proj-name">{{ t(TAB_LABEL[tab]) }}</span>
      </button>
    </nav>

    <!-- 底部这一行是常驻的：工具管理顶掉的是项目列表，不是「设置」这个入口。
         扳手在这儿是 active 态 —— 它就是当前位置，点它退回会话。 -->
    <SidebarFooter
      tools-active
      @open-settings="(tab) => emit('open-settings', tab)"
      @toggle-tools="emit('close')"
    />
  </aside>
</template>

<style scoped>
.tools-agent-row {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 2px;
  background: var(--surface-2);
  border-radius: 8px;
}
/* 整排不可用时压暗并吃掉指针事件 —— 光靠 disabled 视觉上看不出来。 */
.tools-agent-row.inert {
  opacity: 0.4;
}
.tools-agent-row.inert .tools-agent {
  cursor: default;
}
.tools-agent {
  flex: 1;
  height: 26px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 6px;
  color: var(--text);
  transition: background 0.12s, opacity 0.12s;
}
.tools-agent:hover {
  background: var(--surface-hover);
}
/* 没选中不是「消失」，是「压暗」—— 用户要能看见自己关掉了哪几家。 */
.tools-agent.off {
  opacity: 0.32;
}
.tools-agent :deep(svg),
.tools-agent :deep(img) {
  width: 15px;
  height: 15px;
}

.tools-nav-item {
  color: var(--text-dim);
}
.tools-nav-ic {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  opacity: 0.85;
}
</style>
