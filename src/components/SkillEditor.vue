<script setup lang="ts">
// 内置编辑器（方案 3.4 · 阶段 5）：左边一棵文件树，右边改一个文件。
//
// 为什么要有它：面板到这一步已经能看出「哪个 skill 有问题」，但改一个字仍然要跳出去开
// 编辑器、自己找到那条链接指向的实体目录。改动最多的其实就是 SKILL.md 顶部那三行
// frontmatter —— 所以 SKILL.md 上面额外给一个表单，不用在 YAML 里数缩进。
//
// **写的是实体路径，不是链接**：`body` 由调用方传主 body 的真实目录（`detail.primary`），
// 后端还会再把作用域锁死在它里面（`tools/files.rs`）。前端这一层只是省一次往返，不是防线。
import { computed, nextTick, ref, watch } from 'vue'
import CodeEditor from './CodeEditor.vue'
import ConfirmModal from '../modals/ConfirmModal.vue'
import {
  fileIconFor,
  IconChevronRight,
  IconClose,
  IconExternalLink,
  IconFolder,
  IconSave,
} from './icons'
import * as api from '../api'
import { t } from '../i18n'
import { elidePath, formatSize, renderText } from '../format'
import { buildFileTree, flattenTree, treeDepth } from '../fileTree'
import { langOfPath } from '../shikiHighlight'
import { fmEditable, fmValue, setFmValue, stripFrontmatter } from '../skillFrontmatter'
import type { FileRev, SkillFile } from '../types'

const props = defineProps<{
  show: boolean
  /** skill 名，只用来做标题。 */
  name: string
  /** 主 body 的**实体**目录。 */
  body: string
}>()

const emit = defineEmits<{
  (e: 'close'): void
  /** 存过盘了 —— 面板据此重扫（文件大小、风险、frontmatter 都可能变了）。 */
  (e: 'saved'): void
}>()

/** frontmatter 表单管的三个字段。其余的（model / license…）去下面的原文里改。 */
const FM_KEYS = ['name', 'description', 'allowed-tools'] as const

const files = ref<SkillFile[]>([])
const listTruncated = ref(false)
const loadError = ref<string | null>(null)

const openRel = ref<string | null>(null)
const text = ref('')
/** 上一次落盘的内容。`dirty` 就是拿它和 `text` 比。 */
const saved = ref('')
const rev = ref<FileRev | null>(null)
const binary = ref(false)
const clipped = ref(false)
const fileError = ref<string | null>(null)

const busy = ref(false)
const saveError = ref<string | null>(null)
const mode = ref<'edit' | 'preview'>('edit')
/** 想切过去但当前文件还没存的那个。确认框点「放弃」之后才真的切。 */
const leaving = ref<string | null>(null)

const collapsed = ref<Set<string>>(new Set())

const dirty = computed(() => text.value !== saved.value)
/** 二进制和只读到一半的文件不给存 —— `text` 根本不是完整内容。 */
const editable = computed(() => !binary.value && !clipped.value && openRel.value !== null)
const isMarkdown = computed(() => /\.mdx?$/i.test(openRel.value ?? ''))
const lang = computed(() => (openRel.value ? langOfPath(openRel.value) : null))

const tree = computed(() => buildFileTree(files.value))
const rows = computed(() => flattenTree(tree.value, (p) => !collapsed.value.has(p)))

function toggleDir(path: string) {
  const next = new Set(collapsed.value)
  if (!next.delete(path)) next.add(path)
  collapsed.value = next
}

// ---------------------------------------------------------------------------
// 读
// ---------------------------------------------------------------------------

async function loadList() {
  loadError.value = null
  try {
    const res = await api.toolsListSkillFiles(props.body)
    files.value = res.files
    listTruncated.value = res.truncated
  } catch (e) {
    files.value = []
    loadError.value = String(e)
    return
  }
  // 默认开 SKILL.md —— 十次里有九次要改的就是它。
  const first = files.value.find((f) => f.path === 'SKILL.md') ?? files.value[0]
  if (first) await openFile(first.path)
}

async function openFile(rel: string) {
  busy.value = true
  fileError.value = null
  saveError.value = null
  try {
    const f = await api.toolsReadSkillFile(props.body, rel)
    openRel.value = rel
    text.value = f.text
    saved.value = f.text
    rev.value = f.rev
    binary.value = f.binary
    clipped.value = f.truncated
    mode.value = 'edit'
  } catch (e) {
    fileError.value = String(e)
  } finally {
    busy.value = false
  }
}

/** 点了另一个文件。当前这个改了没存就先问一句。 */
function selectFile(rel: string) {
  if (rel === openRel.value) return
  if (dirty.value) {
    leaving.value = rel
    return
  }
  void openFile(rel)
}

function discardAndGo() {
  const rel = leaving.value
  leaving.value = null
  if (rel) void openFile(rel)
}

// ---------------------------------------------------------------------------
// 写
// ---------------------------------------------------------------------------

async function save() {
  const rel = openRel.value
  const current = rev.value
  if (!rel || !current || !editable.value || !dirty.value || busy.value) return
  busy.value = true
  saveError.value = null
  try {
    // 回传读的时候那份 `rev`：文件在别的编辑器里被改过时后端会拒，不会默默盖掉。
    rev.value = await api.toolsWriteSkillFile(props.body, rel, text.value, current)
    saved.value = text.value
    emit('saved')
  } catch (e) {
    saveError.value = String(e)
  } finally {
    busy.value = false
  }
}

function setFm(key: string, value: string) {
  text.value = setFmValue(text.value, key, value)
}

function fmDisabled(key: string) {
  return !editable.value || !fmEditable(text.value, key)
}

function openExternally() {
  if (!openRel.value) return
  void api.openPathExternal(`${props.body}/${openRel.value}`)
}

// ---------------------------------------------------------------------------
// 开 / 关
// ---------------------------------------------------------------------------

function requestClose() {
  // 改了没存不给直接关 —— 这个框是全屏的，关掉就找不回来了。
  if (dirty.value) {
    leaving.value = ''
    return
  }
  emit('close')
}

function discardAndClose() {
  leaving.value = null
  emit('close')
}

function reset() {
  files.value = []
  listTruncated.value = false
  loadError.value = null
  openRel.value = null
  text.value = ''
  saved.value = ''
  rev.value = null
  binary.value = false
  clipped.value = false
  fileError.value = null
  saveError.value = null
  leaving.value = null
  collapsed.value = new Set()
}

watch(
  () => [props.show, props.body] as const,
  ([show]) => {
    reset()
    if (show && props.body) void nextTick(loadList)
  },
  { immediate: true },
)

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && leaving.value === null) {
    e.stopPropagation()
    requestClose()
  }
}
</script>

<template>
  <Transition name="fade">
    <div v-if="show" class="app-overlay" @keydown="onKeydown">
      <div class="skill-editor" role="dialog" aria-modal="true" :aria-label="name">
        <header class="skill-editor-head">
          <h3>{{ name }}</h3>
          <span class="skill-editor-path">{{ elidePath(body, 3) }}</span>
          <button
            type="button"
            class="modal-close"
            v-tooltip="t('common.close')"
            :aria-label="t('common.close')"
            @click="requestClose"
          >
            <IconClose />
          </button>
        </header>

        <div class="skill-editor-body">
          <!-- 文件树 -->
          <nav class="skill-editor-tree" :aria-label="t('tools.skills.editor.files')">
            <p v-if="loadError" class="skill-editor-msg error">{{ loadError }}</p>
            <div
              v-for="node in rows"
              :key="node.path"
              class="skill-editor-file"
              :class="{ dir: node.children.length > 0, on: node.item?.path === openRel }"
              :style="{ paddingLeft: 8 + treeDepth(node.path) * 12 + 'px' }"
              @click="node.item ? selectFile(node.item.path) : toggleDir(node.path)"
            >
              <!-- 每一行都留出折叠三角那一格（文件行是个空位），不然同一层的文件名和
                   目录名会差开一个图标的宽度，看上去像缩进错了。 -->
              <IconChevronRight
                v-if="node.children.length"
                class="skill-editor-arrow"
                :class="{ open: !collapsed.has(node.path) }"
                aria-hidden="true"
              />
              <span v-else class="skill-editor-arrow" aria-hidden="true" />
              <component
                :is="node.children.length ? IconFolder : fileIconFor(node.name)"
                class="skill-editor-file-ic"
              />
              <span class="skill-editor-file-name">{{ node.name }}</span>
              <span v-if="node.item" class="skill-editor-size">{{ formatSize(node.item.bytes) }}</span>
            </div>
            <p v-if="listTruncated" class="skill-editor-msg">{{ t('tools.skills.editor.tooManyFiles') }}</p>
          </nav>

          <!-- 当前文件 -->
          <section class="skill-editor-pane">
            <div class="skill-editor-bar">
              <span class="skill-editor-open">
                {{ openRel ?? t('tools.skills.editor.pickFile') }}
                <span v-if="dirty" class="skill-editor-dot" aria-hidden="true" />
              </span>

              <div v-if="isMarkdown" class="skill-editor-modes">
                <button
                  type="button"
                  :class="{ on: mode === 'edit' }"
                  @click="mode = 'edit'"
                >{{ t('tools.skills.editor.edit') }}</button>
                <button
                  type="button"
                  :class="{ on: mode === 'preview' }"
                  @click="mode = 'preview'"
                >{{ t('tools.skills.editor.preview') }}</button>
              </div>

              <button
                type="button"
                class="skill-editor-btn"
                :disabled="!openRel"
                v-tooltip="t('tools.skills.editor.externalTip')"
                @click="openExternally"
              >
                <IconExternalLink />
                {{ t('tools.skills.editor.external') }}
              </button>
              <button
                type="button"
                class="skill-editor-btn primary"
                :class="{ running: busy }"
                :disabled="!editable || !dirty || busy"
                v-tooltip="t('tools.skills.editor.saveTip')"
                @click="save"
              >
                <span v-if="busy" class="chip-spinner" aria-hidden="true" />
                <IconSave v-else />
                {{ t('tools.skills.editor.save') }}
              </button>
            </div>

            <p v-if="fileError" class="skill-editor-msg error">{{ fileError }}</p>
            <p v-else-if="saveError" class="skill-editor-msg error">{{ saveError }}</p>

            <p v-if="binary" class="skill-editor-msg">{{ t('tools.skills.editor.binary') }}</p>
            <p v-else-if="clipped" class="skill-editor-msg">{{ t('tools.skills.editor.tooBig') }}</p>

            <!-- SKILL.md 顶上那三行。改这三个字段是这个编辑器最常做的事，
                 不该让人在 YAML 里数缩进、猜要不要加引号。 -->
            <div v-if="openRel === 'SKILL.md' && mode === 'edit' && editable" class="skill-editor-fm">
              <label v-for="k in FM_KEYS" :key="k" class="skill-editor-fm-row">
                <span class="skill-editor-fm-key">{{ k }}</span>
                <input
                  type="text"
                  :value="fmValue(text, k) ?? ''"
                  :disabled="fmDisabled(k)"
                  :placeholder="fmDisabled(k) ? t('tools.skills.editor.fmMultiline') : ''"
                  v-tooltip="fmDisabled(k) ? t('tools.skills.editor.fmMultilineTip') : ''"
                  @input="setFm(k, ($event.target as HTMLInputElement).value)"
                />
              </label>
            </div>

            <CodeEditor
              v-if="openRel && !binary && mode === 'edit'"
              :key="openRel"
              v-model="text"
              :lang="lang"
              :readonly="clipped"
              @save="save"
            />
            <div
              v-else-if="openRel && mode === 'preview'"
              class="skill-editor-preview md"
              v-html="renderText(stripFrontmatter(text))"
            />
          </section>
        </div>
      </div>
    </div>
  </Transition>

  <!-- 改了没存还要走。`leaving` 是空串表示要关整个框，否则是要切过去的那个文件。 -->
  <ConfirmModal
    :show="leaving !== null"
    :title="t('tools.skills.editor.discardTitle')"
    :message="t('tools.skills.editor.discardMsg', { file: openRel ?? '' })"
    :ok-text="t('tools.skills.editor.discardOk')"
    danger
    @confirm="leaving === '' ? discardAndClose() : discardAndGo()"
    @cancel="leaving = null"
  />
</template>

<style scoped>
.skill-editor {
  display: flex;
  flex-direction: column;
  width: min(1080px, calc(100vw - 64px));
  height: min(760px, calc(100vh - 80px));
  background: color-mix(in srgb, var(--surface) 92%, transparent);
  border-radius: 14px;
  /* 外轮廓交给阴影 —— `--shadow-lg` 第一层本身就是 0 0 0 1px 描边，再加 border 会在
     壁纸模式下亮成一道硬边（同 .modal 那一轮的结论）。 */
  box-shadow: var(--shadow-lg);
  overflow: hidden;
}

.skill-editor-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 14px 16px 12px;
  border-bottom: 1px solid var(--border);
}
.skill-editor-head h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  flex-shrink: 0;
}
.skill-editor-path {
  flex: 1;
  min-width: 0;
  font-size: 11.5px;
  color: var(--text-mute);
  /* 中间省略（`elidePath`），不是 `direction: rtl` —— 那个会把开头的 `/` 甩到末尾，
     `/Users/x/…` 显示成 `Users/x/…/`，看着像条相对路径。 */
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.skill-editor-body {
  flex: 1;
  min-height: 0;
  display: flex;
}

.skill-editor-tree {
  width: 230px;
  flex-shrink: 0;
  overflow: auto;
  padding: 8px 0;
  border-right: 1px solid var(--border);
}
.skill-editor-file {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px 3px 8px;
  font-size: 12px;
  cursor: pointer;
  user-select: none;
}
.skill-editor-file:hover {
  background: var(--surface-hover);
}
.skill-editor-file.on {
  background: var(--surface-hover);
  font-weight: 600;
}
.skill-editor-file-ic {
  width: 15px;
  height: 15px;
  flex-shrink: 0;
  color: var(--text-mute);
}
/* 目录比文件重一档：一棵树里先看见的应该是"这儿还有一层"。 */
.skill-editor-file.dir .skill-editor-file-ic {
  color: var(--text-dim);
}
.skill-editor-file.on .skill-editor-file-ic {
  color: var(--text);
}
.skill-editor-file-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.skill-editor-pane {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 12px 12px;
}

.skill-editor-bar {
  display: flex;
  align-items: center;
  gap: 8px;
}
.skill-editor-open {
  flex: 1;
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 没存的小圆点。比在标题里加个 `*` 好认，也不会把路径顶得换行。 */
.skill-editor-dot {
  width: 6px;
  height: 6px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--brand);
}

.skill-editor-modes {
  display: inline-flex;
  flex-shrink: 0;
  border: 1px solid var(--border);
  border-radius: 7px;
  overflow: hidden;
}
.skill-editor-modes button {
  padding: 3px 10px;
  border: 0;
  background: transparent;
  color: var(--text-dim);
  font-size: 11.5px;
  cursor: pointer;
}
.skill-editor-modes button.on {
  background: var(--accent);
  color: var(--surface);
}

.skill-editor-fm {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex-shrink: 0;
}
.skill-editor-fm-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.skill-editor-fm-row .skill-editor-fm-key {
  width: 92px;
  flex-shrink: 0;
}
.skill-editor-fm-row input {
  flex: 1;
  min-width: 0;
  padding: 4px 8px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface-2);
  color: var(--text);
  font-size: 12px;
}
.skill-editor-fm-row input:focus {
  outline: none;
  border-color: var(--accent);
}
.skill-editor-fm-row input:disabled {
  color: var(--text-mute);
  cursor: not-allowed;
}

.skill-editor-preview {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px 12px;
  border-radius: 8px;
  background: var(--surface-2);
  font-size: 13px;
  line-height: 1.7;
}

/* 这几个类名 ToolsSkillsPanel 里也有，但那边是 scoped 的，隔着组件不会生效。
   与其借一个借不到的名字，不如用自己的名字写自己的一份。 */
.skill-editor-btn {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  gap: 5px;
  padding: 4px 10px;
  border-radius: 7px;
  border: 1px solid var(--border);
  background: transparent;
  color: var(--text-dim);
  font-size: 12px;
  white-space: nowrap;
  cursor: pointer;
  transition: background 0.12s, color 0.12s, opacity 0.12s;
}
.skill-editor-btn:hover:not(:disabled) {
  background: var(--surface-hover);
  color: var(--text);
}
.skill-editor-btn:disabled {
  opacity: 0.4;
  cursor: default;
}
.skill-editor-btn.running:disabled {
  opacity: 1;
}
.skill-editor-btn.primary:not(:disabled) {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--surface);
}
.skill-editor-btn.primary:hover:not(:disabled) {
  opacity: 0.88;
  background: var(--accent);
  color: var(--surface);
}
.skill-editor-btn :deep(svg) {
  width: 13px;
  height: 13px;
}

.skill-editor-msg {
  margin: 0;
  padding: 8px 10px;
  font-size: 11.5px;
  color: var(--text-mute);
  line-height: 1.6;
}
.skill-editor-msg.error {
  color: var(--danger);
  white-space: pre-wrap;
}

.skill-editor-size {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-mute);
}

/* 以前这儿是个 `▸` 字符，11px 的字形在 12px 的格子里几乎看不见（而仓库的规矩是
   图标一律用 `icons.ts` 里的 inline SVG）。换成 lucide 的 chevron，描边的粗细和
   旁边那排文件图标是同一套。文件行复用同一个类当空位，所以没有 `width` 就不行。 */
.skill-editor-arrow {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  color: var(--text-mute);
  transition: transform 0.12s ease;
}
.skill-editor-arrow.open {
  transform: rotate(90deg);
}

.skill-editor-fm-key {
  color: var(--text-mute);
  font-size: 11.5px;
}
</style>
