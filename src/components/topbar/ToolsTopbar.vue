<script setup lang="ts">
// 工具管理的顶栏：搜索框 + 关闭按钮。
//
// 工具管理不是弹框，是和统计 / 回收站同一档的主区视图，所以搜索框必须落在顶栏中列
// —— 和别的视图同一个位置，用户的眼睛不用换地方找。关闭按钮贴最右（顶栏第三列），
// 那儿平时是空的。
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { t } from '../../i18n'
import { toolsBundleOpen, toolsQuery, toolsTab } from '../../toolsPanel'
import { useDebouncedSearch } from '../../useDebouncedSearch'
import { IconArchive, IconClose, IconSearch } from '../icons'

const emit = defineEmits<{ (e: 'close'): void }>()

// 搜索防抖 + IME 组合保护：见 useDebouncedSearch 的注释。
// 切 tab 会把 toolsQuery 清空，watch(target) 会把这里的 draft 一并拉回来。
const {
  draft: searchDraft,
  commit: commitSearch,
  onInput: onSearchInput,
  onCompositionStart: onSearchCompStart,
  onCompositionEnd: onSearchCompEnd,
} = useDebouncedSearch(toolsQuery, 200)
const hasQuery = computed(() => searchDraft.value.length > 0)
// 四个面板搜的不是一类东西，placeholder 跟着 tab 走。
const placeholder = computed(() => t(`tools.search.${toolsTab.value}`))

// ⌘F / Ctrl+F：面板开着时接管系统 Find，聚焦搜索框并全选（和 TrashTopbar 同款）。
// 只检测当前平台对应的修饰键，避免 macOS 上 Ctrl+F（光标右移）被误抢。
const searchInput = ref<HTMLInputElement>()
const isMac = /Mac/i.test(navigator.platform)
function onFindShortcut(e: KeyboardEvent) {
  if (e.key !== 'f' && e.key !== 'F') return
  const want = isMac ? e.metaKey : e.ctrlKey
  const other = isMac ? e.ctrlKey : e.metaKey
  if (!want || other || e.shiftKey || e.altKey) return
  e.preventDefault()
  searchInput.value?.focus()
  searchInput.value?.select()
}
onMounted(() => window.addEventListener('keydown', onFindShortcut))
onUnmounted(() => window.removeEventListener('keydown', onFindShortcut))
</script>

<template>
  <div class="chat-topbar tools-topbar">
    <div class="ct-search" :class="{ active: hasQuery }">
      <span class="ct-search-ic"><IconSearch /></span>
      <input
        ref="searchInput"
        :value="searchDraft"
        type="text"
        class="ct-search-input"
        :placeholder="placeholder"
        :aria-label="placeholder"
        spellcheck="false"
        autocomplete="off"
        @input="onSearchInput"
        @compositionstart="onSearchCompStart"
        @compositionend="onSearchCompEnd"
      />
      <button
        v-if="hasQuery"
        class="ct-btn"
        v-tooltip="t('chat.tb.search.clear')"
        @click="commitSearch('')"
      >
        <IconClose />
      </button>
    </div>
    <!-- 配置集跨四个 tab（一个包同时带 MCP / hooks / 全局指令 / skill 清单），
         所以入口在顶栏而不在某一个面板里。 -->
    <button
      class="ct-btn tools-topbar-bundle"
      :class="{ active: toolsBundleOpen }"
      v-tooltip="t('tools.bundle.open')"
      :aria-label="t('tools.bundle.open')"
      @click="toolsBundleOpen = !toolsBundleOpen"
    >
      <IconArchive />
    </button>
    <button
      class="ct-btn tools-topbar-close"
      v-tooltip="t('tools.close')"
      :aria-label="t('tools.close')"
      @click="emit('close')"
    >
      <IconClose />
    </button>
  </div>
</template>

<style scoped>
/* .topbar-drag 是三列网格：[上下文标题 | 搜索 | 右侧空位]。别的顶栏只占中列，
   工具管理要多占一列，好让关闭按钮贴到窗口最右边。 */
.tools-topbar {
  grid-column: 2 / 4;
}
.tools-topbar-bundle {
  margin-left: auto;
  flex-shrink: 0;
}
.tools-topbar-close {
  flex-shrink: 0;
}
</style>
