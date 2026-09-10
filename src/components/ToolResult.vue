<script setup lang="ts">
import { computed } from 'vue'
import type { Block } from '../types'
import { t } from '../i18n'
import CollapsibleBox from './CollapsibleBox.vue'
import { IconChevronRight, IconInfo } from './icons'
import { highlightJsonInPlace, looksLikeJson } from '../jsonHighlight'
import { highlightDiff, looksLikeDiff } from '../diffHighlight'
import { renderCodexFileChangeHtml } from '../codexApplyPatch'
import { formatSize } from '../format'
import {
  DIFF_HIGHLIGHT_MAX_CHARS,
  JSON_HIGHLIGHT_MAX_CHARS,
  OVERSIZE_BLOCK_CHARS,
} from '../renderLimits'

const props = withDefaults(defineProps<{ block: Block; inUser?: boolean; persistOpen?: boolean; cwd?: string }>(), {
  persistOpen: undefined,
})
const emit = defineEmits<{ toggle: [open: boolean] }>()

// 结果文本的渲染优先级：
//   1. structured diff（block.diff，有 hunks）→ DiffBlock（保留交互）
//   2. 文本形态的 unified diff（Bash 跑 git diff / 工具吐 patch）→ 行级染色
//   3. JSON（含 Read .json 文件的 cat-n 行号格式）→ token 上色
//   4. 其它 → 原样 <pre>
// 判断顺序很重要：JSON 文件的 diff 既像 diff 又像 JSON，应该按 diff 渲染。
//
// 每一级都有体积闸门（见 renderLimits.ts）：超限就跳过染色退回纯 <pre>。染色是按行
// 建 DOM 的，一个 5 MB 的 tool 输出能生成百万级 <span>，内存是原文的几十倍，而在
// 那个体积下配色对阅读也没有任何帮助。内容本身一个字都不会少。
const rawText = computed(() => props.block.text ?? '')

/** 文本形态的 diff —— 与是否染色无关。标题、自动展开、增删统计都看它，
 *  所以超限跳过染色时这个判断必须保持不变。 */
const isTextDiff = computed(() => looksLikeDiff(rawText.value))

const diffHtml = computed(() => {
  if (!isTextDiff.value) return null
  if (rawText.value.length > DIFF_HIGHLIGHT_MAX_CHARS) return null
  return highlightDiff(rawText.value)
})
const jsonHtml = computed(() => {
  // diff 优先：JSON 文件的 diff 两种判断都为真，必须按 diff 渲染。
  if (isTextDiff.value) return null
  const txt = rawText.value
  if (txt.length > JSON_HIGHLIGHT_MAX_CHARS) return null
  if (!looksLikeJson(txt)) return null
  return highlightJsonInPlace(txt)
})

/** 超大块：默认折叠，并在标题上标注体积，免得用户对着一个转圈的窗口猜发生了什么。 */
const oversizeLabel = computed(() =>
  rawText.value.length > OVERSIZE_BLOCK_CHARS ? formatSize(rawText.value.length) : '',
)

function baseName(p?: string): string {
  if (!p) return ''
  const parts = p.split('/').filter(Boolean)
  return parts.length ? parts[parts.length - 1] : p
}

/** 从 `git diff` 的文件头中拿一个可读的目标路径。文本 diff 可能包含多文件，这里只取
 * 第一项作为卡片标题，完整内容和统计仍保留在展开区。 */
function firstTextDiffPath(text: string): string | undefined {
  const gitHeader = /^diff --git a\/(.+?) b\/(.+)$/m.exec(text)
  if (gitHeader?.[2]) return gitHeader[2]
  const newFileHeader = /^\+\+\+ (?:b\/)?(.+)$/m.exec(text)
  return newFileHeader?.[1] && newFileHeader[1] !== '/dev/null' ? newFileHeader[1] : undefined
}

const textDiffPath = computed(() => firstTextDiffPath(props.block.text ?? ''))

const label = computed(() => {
  if (props.block.diff || props.block.filePath || isTextDiff.value) {
    return t('tool.resultDiff', { file: baseName(props.block.filePath ?? textDiffPath.value) })
  }
  return props.block.isError ? t('tool.resultError') : t('tool.result')
})

const diffStat = computed(() => {
  let add = 0
  let del = 0
  if (props.block.diff) {
    for (const h of props.block.diff)
      for (const l of h.lines) {
        if (l.kind === 'add') add++
        else if (l.kind === 'del') del++
      }
  } else if (isTextDiff.value) {
    for (const line of (props.block.text ?? '').split('\n')) {
      if (line.startsWith('+') && !line.startsWith('+++')) add++
      else if (line.startsWith('-') && !line.startsWith('---')) del++
    }
  }
  return add || del ? `+${add} −${del}` : ''
})

const hasRenderableText = computed(() => {
  if (props.block.diff || props.block.filePath) return true
  return !!(props.block.text ?? '').trim()
})

const shouldAutoOpen = computed(
  () => !oversizeLabel.value && (!!props.block.diff || !!props.block.filePath || isTextDiff.value),
)
const fileChangeHtml = computed(() => {
  if (!props.block.filePath) return null
  return renderCodexFileChangeHtml(
    props.block.diff,
    props.block.filePath,
    props.block.fileChangeType,
    props.cwd,
  )
})
</script>

<template>
  <div
    v-if="fileChangeHtml"
    class="tool-result-file-change"
    v-html="fileChangeHtml"
  />
  <details
    v-else-if="hasRenderableText"
    :class="{
      'block-card': !block.isError,
      'thinking-block': block.isError,
      'tool-result-error': block.isError,
      'in-user': inUser,
      'auto-open': shouldAutoOpen,
      'text-diff-result': isTextDiff,
    }"
    :open="persistOpen ?? shouldAutoOpen"
    @toggle="emit('toggle', ($event.target as HTMLDetailsElement).open)"
  >
    <summary :class="block.isError ? 'thinking-summary' : 'block-summary'">
      <template v-if="block.isError">
        <IconInfo class="thinking-icon" aria-hidden="true" />
        <span class="thinking-label">{{ label }}</span>
        <span v-if="oversizeLabel" class="oversize-hint">{{
          t('tool.largeContent', { size: oversizeLabel })
        }}</span>
        <span class="thinking-chev"><IconChevronRight /></span>
      </template>
      <template v-else>
        <span class="chev"><IconChevronRight /></span>
        <span class="label">{{ label }}</span>
        <span v-if="diffStat" class="diff-stat">{{ diffStat }}</span>
        <span v-if="oversizeLabel" class="oversize-hint">{{
          t('tool.largeContent', { size: oversizeLabel })
        }}</span>
      </template>
    </summary>
    <div :class="block.isError ? 'thinking-content tool-result-error-content' : 'block-body'">
      <CollapsibleBox :max-height="400">
        <pre v-if="diffHtml" class="lang-diff" v-html="diffHtml" />
        <pre v-else-if="jsonHtml" class="lang-json" v-html="jsonHtml" />
        <pre v-else>{{ block.text }}</pre>
      </CollapsibleBox>
    </div>
  </details>
</template>
