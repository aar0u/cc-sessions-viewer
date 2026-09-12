<script setup lang="ts">
// 零依赖的代码编辑器：透明 textarea 叠在高亮层上。
//
// 不上 Monaco / CodeMirror 是硬约束（方案 3.4）：那两个压缩后都是 MB 级，为一个二级
// 功能把包体翻倍不划算，而高亮、体积上限、语言识别仓库里本来就全有。
//
// **两层必须逐像素同款**：同字体、同 `font-size` / `line-height` / `padding` /
// `white-space` / `tab-size` / `word-break`。差一项光标就和高亮错位，而且是那种
// 「短文本看不出来、写到第三屏才发现」的错位。所以这些值都写在 `.ce-layer` 这一个
// 类里，两层共用，不给任何一层单独覆盖的机会。
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { plainToHtml, tokensToHtml, indentEdit, newlineIndent, type HlToken } from '../codeEditor'
import { highlightLines } from '../shikiHighlight'
import { theme } from '../settings'

const props = defineProps<{
  modelValue: string
  /** shiki 的语言名。null = 不高亮（未知后缀、或者调用方就不想高亮）。 */
  lang?: string | null
  readonly?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void
  (e: 'save'): void
}>()

const taEl = ref<HTMLTextAreaElement>()
const hlEl = ref<HTMLPreElement>()

/** 高亮层的 HTML。还没跑出 token（或不高亮）时是转义过的纯文本 —— 先对齐，再上色。 */
const html = ref(plainToHtml(props.modelValue))

// 防抖：shiki 是 tokenizer，每敲一个字符全量重跑，中等大小的文件就开始掉帧。
// 120ms 是「停手了」的体感门槛；这中间显示的是上一轮的上色，文字位置是当前的
// （纯文本层已经同步更新），所以不会出现「字和颜色对不上」的中间态。
const HIGHLIGHT_DEBOUNCE = 120
let timer = 0

function paintPlain() {
  html.value = plainToHtml(props.modelValue)
}

async function paintHighlighted() {
  const code = props.modelValue
  const lang = props.lang
  if (!lang) return
  let lines: HlToken[][] | null = null
  try {
    lines = await highlightLines(code, lang)
  } catch {
    lines = null
  }
  // 跑 shiki 是异步的，回来时用户可能已经又敲了几个字或者换了文件 —— 那一轮的上色
  // 贴到新文本上就是整屏错位，宁可这一轮不上色。
  if (code !== props.modelValue || lang !== props.lang) return
  if (lines) html.value = tokensToHtml(lines)
}

function schedule() {
  // 先把纯文本层同步上去：位置必须永远是对的，颜色可以慢一步。
  paintPlain()
  window.clearTimeout(timer)
  timer = window.setTimeout(() => {
    timer = 0
    void paintHighlighted()
  }, HIGHLIGHT_DEBOUNCE)
}

watch(() => [props.modelValue, props.lang], schedule)
// 主题换了颜色就全变了，但文字没动，不用等防抖。
watch(theme, () => void paintHighlighted())
onMounted(() => {
  paintPlain()
  void paintHighlighted()
})

// ---------------------------------------------------------------------------
// 输入
// ---------------------------------------------------------------------------

function onInput(e: Event) {
  emit('update:modelValue', (e.target as HTMLTextAreaElement).value)
}

/**
 * 用 `execCommand('insertText')` 写入，**不是**赋值 `textarea.value`。
 *
 * 赋值会把浏览器原生的撤销栈整个清掉：用户按一次 ⌘Z，文件直接回到打开时的样子，
 * 中间几十次编辑一起没。`insertText` 走的是和真实输入同一条路径，undo / redo 照常。
 *
 * 回退到赋值只为一件事：jsdom 没有 `execCommand`（单测里），那儿不需要撤销栈。
 */
function insert(text: string, from: number, to: number, selStart: number, selEnd: number) {
  const ta = taEl.value
  if (!ta) return
  ta.setSelectionRange(from, to)
  const native = typeof document.execCommand === 'function'
  if (!native || !document.execCommand('insertText', false, text)) {
    ta.setRangeText(text, from, to, 'end')
    emit('update:modelValue', ta.value)
  }
  ta.setSelectionRange(selStart, selEnd)
  // `insertText` 会自己派 input 事件（v-model 因此已经更新）；`setRangeText` 不会，
  // 所以上面那条分支手动 emit 过了。
}

function onKeydown(e: KeyboardEvent) {
  const ta = taEl.value
  if (!ta || props.readonly) return

  const mod = e.metaKey || e.ctrlKey
  if (mod && (e.key === 's' || e.key === 'S')) {
    e.preventDefault()
    emit('save')
    return
  }

  if (e.key === 'Tab') {
    // 拦掉默认的焦点跳转 —— 在一个代码编辑器里按 Tab 跳到下一个按钮，没人想要。
    e.preventDefault()
    const edit = indentEdit(ta.value, ta.selectionStart, ta.selectionEnd, e.shiftKey)
    if (edit) insert(edit.text, edit.from, edit.to, edit.selStart, edit.selEnd)
    return
  }

  if (e.key === 'Enter' && !mod && !e.altKey) {
    const text = newlineIndent(ta.value, ta.selectionStart)
    // 只有真有缩进要续时才接管，否则交给浏览器自己插换行（少一次 DOM 往返）。
    if (text.length > 1) {
      e.preventDefault()
      const at = ta.selectionStart
      const end = ta.selectionEnd
      insert(text, at, end, at + text.length, at + text.length)
    }
  }
}

/** 两层同步滚动。高亮层自己不滚（`overflow: hidden`），位置完全跟着 textarea。 */
function onScroll() {
  const ta = taEl.value
  const hl = hlEl.value
  if (!ta || !hl) return
  hl.scrollTop = ta.scrollTop
  hl.scrollLeft = ta.scrollLeft
}

// 换掉 innerHTML 有可能把高亮层的滚动位置带回 0（新内容比旧的短时一定会），
// 而 textarea 没动 —— 在长文件的末尾打字时这就是整屏错位。重贴完对一次。
watch(html, () => void nextTick(onScroll))

const empty = computed(() => props.modelValue.length === 0)

defineExpose({ focus: () => taEl.value?.focus() })
</script>

<template>
  <div class="ce" :class="{ readonly }">
    <pre ref="hlEl" class="ce-layer ce-hl" aria-hidden="true" v-html="html" />
    <textarea
      ref="taEl"
      class="ce-layer ce-ta"
      :class="{ empty }"
      :value="modelValue"
      :readonly="readonly"
      spellcheck="false"
      autocomplete="off"
      autocapitalize="off"
      autocorrect="off"
      wrap="soft"
      @input="onInput"
      @keydown="onKeydown"
      @scroll="onScroll"
    />
  </div>
</template>

<style scoped>
.ce {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  border-radius: 8px;
  background: var(--surface-2);
}

/* 两层共用的盒子。**任何一条都不能只改一层** —— 差一项就是整屏错位。 */
.ce-layer {
  position: absolute;
  inset: 0;
  margin: 0;
  padding: 10px 12px;
  border: 0;
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 12.5px;
  line-height: 1.65;
  letter-spacing: 0;
  tab-size: 2;
  white-space: pre-wrap;
  word-break: break-word;
  overflow-wrap: break-word;
}

.ce-hl {
  overflow: hidden;
  color: var(--text);
  pointer-events: none;
}

.ce-ta {
  overflow: auto;
  resize: none;
  background: transparent;
  /* 字本身透明，只留光标 —— 看到的字全部来自底下那层。 */
  color: transparent;
  caret-color: var(--text);
}
.ce-ta:focus {
  outline: none;
}
/* 空文件时底下那层什么都没有，把字显出来，免得看上去像坏了。 */
.ce-ta.empty {
  color: var(--text);
}
/* 选区必须半透明 —— textarea 的字是透明的，实心底色会把底下那层的字盖掉。 */
.ce-ta::selection {
  background: color-mix(in srgb, var(--accent) 22%, transparent);
}

.ce.readonly .ce-ta {
  caret-color: transparent;
}
</style>
