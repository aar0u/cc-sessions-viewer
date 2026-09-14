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
//
// 行号不是第三层：它是高亮层里每一行那个块盒子的 `::before`（CSS 计数器）。单开一条
// 行号栏就得自己算「这一行折了几折」，而这一层本来就是按逻辑行分块的 —— 号挂在盒子上，
// 折行自然只在第一折出号。
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
  if (lines) html.value = tokensToHtml(lines, code.split('\n').length)
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

/** 两层同步滚动。高亮层不接受输入，滚动位置完全跟着 textarea 走。 */
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
/* 有边框、有行号栏、聚焦有光圈 —— 一片只有浅底色的方块看上去就是段静态文案，
   没人知道它能打字。 */
.ce {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface-2);
  transition: border-color 0.12s;
}
/* 聚焦只把边框加深一档。小输入框那种 3px 光圈套在一整块编辑区上就是一圈粗粗的彩边，
   而这块地方本来就占满右半屏，不靠光圈也知道光标在哪。 */
.ce:focus-within {
  border-color: var(--border-strong);
}
/* 行号栏的底色。画在 `.ce` 自己身上而不是高亮层里 —— 它不跟着横向滚。 */
.ce::before {
  content: '';
  position: absolute;
  top: 0;
  bottom: 0;
  left: 0;
  width: 50px;
  background: var(--surface);
  border-right: 1px solid var(--border);
  pointer-events: none;
}
.ce.readonly::before {
  background: var(--surface-2);
}

/* 两层共用的盒子。**任何一条都不能只改一层** —— 差一项就是整屏错位。 */
.ce-layer {
  position: absolute;
  inset: 0;
  margin: 0;
  /* 左边这一大截是行号栏。**两层都得留**：只给高亮层留的话 textarea 的折行宽度不一样，
     写到折行的那一刻整段错开。 */
  padding: 10px 12px 10px 60px;
  border: 0;
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 12.5px;
  /* **必须是整数 px，不能写 1.65 这种比例。**
     12.5 × 1.65 = 20.625px —— `<pre>` 老老实实按 20.625 排每一行，而 `<textarea>`
     的行盒会被取整到 21px（WebKit 对文本控件的处理）。每行差 0.375px 看不出来，
     累起来就是灾难：本机实测这份 77 行的 SKILL.md，高亮层 scrollHeight 1861、
     textarea 1820，差 41px（正好 66 个未折行的行 × 0.625）。
     后果有两个：
     1. 滚动位置由 textarea 定，它到底了高亮层还差 41px —— 最后两行永远露不出来，
        看上去就是「编辑器底部被框截掉了」。
     2. 光标和字从上往下越走越偏，中段就能看出来错半行。
     21px 时两层完全一致（实测 scrollHeight 都是 1910，各 90 个行盒）。 */
  line-height: 21px;
  letter-spacing: 0;
  tab-size: 2;
  white-space: pre-wrap;
  word-break: break-word;
  overflow-wrap: break-word;
}

.ce-hl {
  overflow: hidden;
  /* 滚动条那 5px 两层都得留出来。
     全局 `::-webkit-scrollbar` 是 5px 宽的**实体**滚动条（不是覆盖式），textarea 一
     溢出就被它吃掉 5px 内容宽度，而高亮层 `overflow: hidden` 不会 —— 两层的折行宽度
     于是差 5px，长行的折行点对不上。本机实测同一个 SKILL.md：高亮层 scrollHeight
     1861、textarea 1820，差 41px（两行）。
     后果有两个，第二个更要命：
     1. 滚到底还有两行高亮文字露不出来 —— 滚动位置由 textarea 定，它到头了，高亮层
        还差 41px。看上去就是「编辑器底部被框截掉了」。
     2. 折行之后光标和字对不齐，越往下越歪。
     `overflow-y: scroll` 而不是 `auto`：`auto` 只在内容溢出时才留槽，短文件又会反过来
     差 5px。两层都恒定留槽，任何长度都对齐。 */
  overflow-y: scroll;
  color: var(--text);
  pointer-events: none;
  counter-reset: ce-line;
}
/* 高亮层的滚动条只是用来占位的，别真画出来 —— 它和 textarea 那条完全重合。 */
.ce-hl::-webkit-scrollbar-thumb {
  background: transparent;
}

/* 一行一个块。空行也得占一行的高 —— 空盒子高度是 0，下面所有行就往上串一行。
   **必须 `:deep()`**：行盒子是 `v-html` 贴进去的，身上没有 scoped 的 `data-v-` 属性，
   普通选择器一条都命中不了（表现是行号整片不显示）。 */
:deep(.ce-row) {
  display: block;
  position: relative;
  min-height: 1lh;
  counter-increment: ce-line;
}
:deep(.ce-row)::before {
  content: counter(ce-line);
  position: absolute;
  /* 号靠右排在 8..44，分隔线在 50，正文从 60 起 —— 号和正文之间留 16px。挨着正文
     的行号会被当成正文的一部分读（尤其正文本身也是数字开头的列表）。 */
  left: -52px;
  width: 36px;
  text-align: right;
  color: var(--text-mute);
  opacity: 0.75;
  user-select: none;
  font-variant-numeric: tabular-nums;
}

.ce-ta {
  overflow: auto;
  /* 和高亮层一样恒定留槽（见 `.ce-hl`）。`auto` 会让短文件比长文件宽 5px。 */
  overflow-y: scroll;
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
