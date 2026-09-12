<script setup lang="ts">
// 侧栏底部常驻的那一行：设置 + 工具管理。
//
// 抽成组件而不是在两处各写一遍：工具管理开着时侧栏整个换成 ToolsNav，而这一行是
// 常驻的，不该跟着项目列表一起消失。写两份的话，「有新版本」那个红点和 release
// 入口迟早只在一边更新 —— 这种东西错了没人会立刻发现。
import { t } from '../i18n'
import { latestVersion, updateAvailable } from '../updateCheck'
import { IconDownload, IconSettings, IconWrench } from './icons'

defineProps<{
  /** 工具管理面板开着没。开着时扳手是「当前位置」，点它退回会话。 */
  toolsActive?: boolean
}>()

const emit = defineEmits<{
  (e: 'open-settings', tab?: 'general' | 'updates'): void
  (e: 'toggle-tools'): void
}>()
</script>

<template>
  <div class="sidebar-footer">
    <button
      class="trash-tab"
      :class="{ 'has-update': updateAvailable }"
      v-tooltip="updateAvailable
        ? t('sidebar.updateAvailable', { v: latestVersion ?? '' })
        : t('sidebar.settings')"
      @click="emit('open-settings')"
    >
      <IconSettings /> {{ t('sidebar.settings') }}
      <!-- 有新版本时，行尾多挂一个「更新」入口按钮：点它直接跳到设置里的「更新」tab
           （不再直接跳 GitHub）。@click.stop 防止冒泡到外层 button 打开通用设置。 -->
      <span
        v-if="updateAvailable"
        class="sidebar-release-btn"
        role="button"
        tabindex="0"
        v-tooltip="t('sidebar.updateAvailable', { v: latestVersion ?? '' })"
        :aria-label="t('sidebar.updateAvailable', { v: latestVersion ?? '' })"
        @click.stop="emit('open-settings', 'updates')"
        @keydown.enter.stop.prevent="emit('open-settings', 'updates')"
        @keydown.space.stop.prevent="emit('open-settings', 'updates')"
      >
        <IconDownload />
      </span>
      <span v-if="updateAvailable" class="update-dot" aria-hidden="true" />
    </button>
    <!-- 工具管理入口。必须是设置按钮的**兄弟节点**，不能塞进去：那里面已经有一个
         嵌在 <button> 里的 <span role="button">（release 入口），不该再叠第二个；
         而且这个入口是常驻的，不像 release 那样只在有新版本时出现。 -->
    <button
      class="sidebar-tools-btn"
      :class="{ active: toolsActive }"
      :aria-pressed="toolsActive === true"
      v-tooltip="toolsActive ? t('tools.close') : t('sidebar.tools')"
      :aria-label="toolsActive ? t('tools.close') : t('sidebar.tools')"
      @click="emit('toggle-tools')"
    >
      <IconWrench />
    </button>
  </div>
</template>
