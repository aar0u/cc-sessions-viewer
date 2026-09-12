<script setup lang="ts">
// 工具管理主区 —— 四个面板的容器。
//
// 形态依据方案文档 3.1 加上用户的当面纠正：它不是弹框。设置弹窗已经 9 个 tab 了，
// 工具管理的信息密度（agent × 四类工具 × 状态）塞不进 880×640；而它又是跨项目跨
// agent 的全局操作，所以走统计 / 回收站那一档 —— 顶掉整个主区，导航在侧栏位置，
// 搜索和关闭在顶栏位置，关掉回到原来的会话，一格分屏都不会动。
//
// 每个面板自带健康条和主从两栏（Skills 的健康条要显示断链数，MCP 的要显示覆盖关系，
// 共用一条反而谁都塞不下）。还没实现的 tab 落到下面那个占位骨架。
import { computed, ref } from 'vue'
import { t } from '../i18n'
import { TAB_LABEL, toolsBundleOpen, toolsTab } from '../toolsPanel'
import ToolsSkillsPanel from './ToolsSkillsPanel.vue'
import ToolsMcpPanel from './ToolsMcpPanel.vue'
import ToolsHooksPanel from './ToolsHooksPanel.vue'
import ToolsMemoPanel from './ToolsMemoPanel.vue'
import ToolsBundleModal from '../modals/ToolsBundleModal.vue'

defineProps<{ cwd?: string }>()
const emit = defineEmits<{
  (e: 'notify', msg: string, error?: boolean): void
  (e: 'open-settings'): void
}>()

const tabLabel = computed(() => t(TAB_LABEL[toolsTab.value]))

/**
 * 导入完之后把当前面板整个重挂一次。
 *
 * 四个面板各自在 `onMounted` 里扫一次，没有一条"从外面让它重扫"的路。与其给四个
 * 面板各开一个 expose，不如换掉 `key` —— 丢掉的只是列表选中项，而刚导完配置的人
 * 想看的正是一份新列表。
 */
const reloadKey = ref(0)
</script>

<template>
  <ToolsSkillsPanel
    v-if="toolsTab === 'skills'"
    :key="reloadKey"
    :cwd="cwd"
    @notify="(msg, error) => emit('notify', msg, error)"
  />

  <ToolsMcpPanel
    v-else-if="toolsTab === 'mcp'"
    :key="reloadKey"
    :cwd="cwd"
    @notify="(msg, error) => emit('notify', msg, error)"
  />

  <!-- 回合信号在这儿只能看不能删，删它的入口一直在设置页，所以要能把人送过去。 -->
  <ToolsHooksPanel
    v-else-if="toolsTab === 'hooks'"
    :key="reloadKey"
    :cwd="cwd"
    @notify="(msg, error) => emit('notify', msg, error)"
    @open-settings="emit('open-settings')"
  />

  <!-- 全局配置只有 home 级那一份，不跟着项目走，所以不接 `cwd`。 -->
  <ToolsMemoPanel
    v-else-if="toolsTab === 'memo'"
    :key="reloadKey"
    @notify="(msg, error) => emit('notify', msg, error)"
  />

  <template v-else>
    <!-- 健康条。各面板接上来之前先占着位置。 -->
    <div class="list-head tools-health" role="status">
      <span class="tools-health-empty">{{ t('tools.healthPending') }}</span>
    </div>

    <div class="tools-body">
      <section class="tools-list" :aria-label="t('tools.listPane')">
        <p class="tools-placeholder">{{ t('tools.shellOnly', { tab: tabLabel }) }}</p>
      </section>
      <section class="tools-detail" :aria-label="t('tools.detailPane')">
        <p class="tools-placeholder">{{ t('tools.noSelection') }}</p>
      </section>
    </div>
  </template>

  <!-- 配置集跨四个 tab（一个包同时带 MCP / hooks / 全局指令 / skill 清单），所以挂在
       容器上而不在某一个面板里。它在 `v-if` 链**之外** —— 链里插一个兄弟节点会把
       `v-else` 和它前面那条 `v-else-if` 断开。 -->
  <ToolsBundleModal
    :show="toolsBundleOpen"
    :cwd="cwd"
    @close="toolsBundleOpen = false"
    @notify="(msg, error) => emit('notify', msg, error)"
    @imported="reloadKey += 1"
  />
</template>

<style scoped>
/* 骨架（`.tools-health` / `.tools-list` / `.tools-detail` / `.tools-placeholder`）在
   style.css，四个面板共用。这儿只写占位骨架自己多出来的一条：它没有可拖的分隔条，
   所以那道竖线得自己画。 */
.tools-list {
  border-right: 1px solid var(--border);
}
</style>
