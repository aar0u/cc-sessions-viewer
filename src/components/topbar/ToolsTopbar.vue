<script setup lang="ts">
// 工具管理的顶栏：搜索框 + 关闭按钮。
//
// 工具管理不是弹框，是和统计 / 回收站同一档的主区视图，所以搜索框必须落在顶栏中列
// —— 和别的视图同一个位置，用户的眼睛不用换地方找。关闭按钮贴最右（顶栏第三列），
// 那儿平时是空的。
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import {
  submitToolsSearch,
  tabAutoFocusesSearch,
  toolsBundleOpen,
  toolsQuery,
  toolsTab,
  type ToolTab,
} from '../../toolsPanel'
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
/**
 * 回车 = 立刻按当前输入搜。
 *
 * 本地那几个面板用不上（它们输入即过滤），「发现」面板的一次输入是一次 HTTP，
 * 防抖叠到 700 ms，知道要搜什么的人不该干等。顶栏只管说「用户提交了」，
 * **不知道也不需要知道**是谁在听、那边要干什么。
 */
function onSubmit() {
  commitSearch(searchDraft.value)
  submitToolsSearch()
}

/**
 * 进「发现」面板时把光标送进搜索框。
 *
 * 哪个 tab 要这个待遇由 `tabAutoFocusesSearch` 说了算，不写成 `=== 'discover'`：
 * 那条判断有理由（见它的注释），而理由该和判断待在一处，且那儿测得到。
 *
 * `nextTick` 不能省 —— tab 刚换，面板还没渲染完，此刻 focus 会被随后的
 * 渲染/滚动抢走。
 */
async function focusSearchFor(tab: ToolTab) {
  if (!tabAutoFocusesSearch(tab)) return
  await nextTick()
  searchInput.value?.focus()
}

watch(toolsTab, focusSearchFor)

onMounted(() => {
  window.addEventListener('keydown', onFindShortcut)
  // 顶栏和面板是一起挂上来的：如果打开工具管理时停在的就是「发现」，watch 不会触发。
  void focusSearchFor(toolsTab.value)
})
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
        @keydown.enter="onSubmit"
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
