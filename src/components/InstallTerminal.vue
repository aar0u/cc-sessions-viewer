<script setup lang="ts">
// 「发现」面板里那个跑安装命令的内嵌终端。
//
// **为什么是终端而不是后台跑一遍。** `npx skills add` 会下载并执行任意 npm 包、
// 没法 dry-run、装完才知道装了什么。所以这儿不代替用户做决定：命令原样打进一个
// **看得见的**终端，回车之后每一行输出都在用户眼前，要停随时 Ctrl-C。
// 装之前该看清楚的（描述 / 文件清单 / 风险点）在左边那一屏已经摊开过了。
//
// 和 `terminals.ts` 那套全局 TUI tabs 是两回事：那套按会话管 tab、跨分屏保活、
// 切走不丢 scrollback。这儿是**一次性**的 —— 关掉就结束，PTY 一起收掉。共用它只会
// 把一个 skill 安装塞进「会话」的模型里，两边都别扭。
//
// 复用的是那边的调色板和 base64 编解码：终端长得不一样会立刻看出来是两套东西。
import { markRaw, onBeforeUnmount, onMounted, ref, shallowRef } from 'vue'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import * as api from '../api'
import { t } from '../i18n'
import { base64ToBytes, bytesToBase64, isDarkActive, terminalColorScheme, xtermTheme } from '../terminals'

const props = defineProps<{
  /** 原样打进去的那一行。 */
  command: string
  /** 起步目录。`npx skills add` 在哪儿跑决定了它装到哪儿。 */
  cwd: string
}>()

const emit = defineEmits<{ (e: 'close'): void; (e: 'ran'): void; (e: 'fail', msg: string): void }>()

const hostEl = ref<HTMLDivElement>()
const term = shallowRef<Terminal | null>(null)
const fit = shallowRef<FitAddon | null>(null)
const ptyId = ref<number | null>(null)
const error = ref<string | null>(null)
let unlistenData: UnlistenFn | null = null
let unlistenExit: UnlistenFn | null = null
let ro: ResizeObserver | null = null
const encoder = new TextEncoder()

function write(text: string) {
  const id = ptyId.value
  if (id === null) return
  void api.ptyWrite(id, bytesToBase64(encoder.encode(text)))
}

function refit() {
  const id = ptyId.value
  if (id === null || !fit.value || !term.value) return
  try {
    fit.value.fit()
    void api.ptyResize(id, term.value.cols, term.value.rows)
  } catch {
    /* 容器这一帧还没有尺寸，下一次 resize 会再来 */
  }
}

onMounted(async () => {
  const host = hostEl.value
  if (!host) return
  const instance = markRaw(
    new Terminal({
      fontSize: 13,
      fontFamily: '"SF Mono", "Menlo", "Consolas", "Liberation Mono", "Courier New", monospace',
      cursorBlink: true,
      convertEol: false,
      allowProposedApi: true,
      scrollback: 5000,
      theme: xtermTheme(isDarkActive()),
    }),
  )
  const addon = markRaw(new FitAddon())
  instance.loadAddon(addon)
  instance.open(host)
  term.value = instance
  fit.value = addon
  try {
    addon.fit()
  } catch {
    /* 容器这一帧还没有尺寸，下面的 ResizeObserver 会再来一次 */
  }

  // 先挂监听再起进程。
  //
  // 反过来写会丢字节：`pty://data` 是广播事件，**订阅之前发出的那些没人收得到**，
  // 而登录 shell 的提示符在 spawn 返回后的几毫秒内就出来了。实测过一次整屏空白 ——
  // 命令其实跑完了（磁盘上装好了），屏幕上一个字没有。
  //
  // 此刻还不知道自己的 id（要等 spawn 返回），所以连 id 一起存着，拿到 id 再筛：
  // 同时可能有别的 PTY（全局 TUI tabs）在发消息，不能一股脑全喂进来。
  let mine: number | null = null
  const backlog: { id: number; base64: string }[] = []
  let typed = false

  function feed(base64: string) {
    instance.write(base64ToBytes(base64))
    // 提示符已经画出来了才打命令。固定延时是不行的 —— 各人的 profile 快慢差很多，
    // 抢在初始化前写进去的字会被 shell 自己冲掉。
    if (!typed) {
      typed = true
      setTimeout(() => {
        if (ptyId.value === null) return
        write(`${props.command}\r`)
        emit('ran')
      }, 120)
    }
  }

  unlistenData = await listen<{ id: number; base64: string }>('pty://data', (e) => {
    if (mine === null) {
      backlog.push(e.payload)
      return
    }
    if (e.payload.id !== mine) return
    feed(e.payload.base64)
  })
  unlistenExit = await listen<{ id: number; code: number }>('pty://exit', (e) => {
    if (e.payload.id === mine) ptyId.value = null
  })

  try {
    mine = await api.ptySpawnShell(props.cwd, instance.cols, instance.rows, terminalColorScheme())
    ptyId.value = mine
    for (const payload of backlog.splice(0)) {
      if (payload.id === mine) feed(payload.base64)
    }
    // 键盘接回去：装到一半要中断、要回答什么，都得能打字。
    instance.onData((data) => write(data))
    instance.focus()
    // 兜底：万一这个 shell 安静得一个字都不出（`PS1=''` 之类），也得把命令打出去。
    setTimeout(() => {
      if (typed || ptyId.value === null) return
      typed = true
      write(`${props.command}\r`)
      emit('ran')
    }, 1500)
  } catch (e) {
    error.value = String(e)
    emit('fail', String(e))
  }

  ro = new ResizeObserver(() => refit())
  ro.observe(host)
})

/**
 * 把键盘还给终端。
 *
 * 「命令已发出」那个弹框恰好挡在 CLI 要键盘的那一刻：关掉它之后光标不回来，用户
 * 得再往终端里点一下才能勾选 agent —— 那一下没有任何道理，纯粹是弹框留下的债。
 */
function focus() {
  term.value?.focus()
}

defineExpose({ focus })

onBeforeUnmount(() => {
  ro?.disconnect()
  ro = null
  unlistenData?.()
  unlistenExit?.()
  unlistenData = null
  unlistenExit = null
  // 关掉就是结束：这不是一个会被切走再切回来的 tab，留着一个没人看的子进程只会
  // 在后台接着装。
  if (ptyId.value !== null) void api.ptyKill(ptyId.value)
  term.value?.dispose()
  term.value = null
  fit.value = null
})
</script>

<template>
  <div class="inst-term">
    <p v-if="error" class="tools-placeholder error">{{ error }}</p>
    <div class="inst-term-box">
      <div ref="hostEl" class="inst-term-host" :aria-label="t('tools.discover.terminal')" />
    </div>
  </div>
</template>

<style scoped>
.inst-term {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  margin-top: 8px;
}
/*
 * 边框这一层自己撑开，终端**绝对定位**在里面 —— 和 `style.css` 里 `.terminal-slot`
 * / `.terminal-host` 同一套写法。
 *
 * 换掉过一版「host 直接 flex:1 + padding」的写法：那样 xterm 的 `.xterm-screen`
 * 会比 `.xterm-viewport` 高出一行（实测 675 vs 665），最下面一行被裁掉一半。
 * 根子在 FitAddon 按**容器**算行数，而容器高度又被 flex 反过来受内容影响，两边
 * 各算各的差出一行的余数。脱离文档流之后容器高度先定死，FitAddon 才有唯一解。
 */
.inst-term-box {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  border-radius: 8px;
  border: 1px solid var(--border);
  /* `--bg` 而不是 `--surface-2`：xterm 的调色板背景就是 `#ffffff` / `#0a0a0a`，也就是
     `--bg`。露出来的那几像素必须和终端自己画的背景**同色**，否则边上会有一道浅灰的缝。 */
  background: var(--bg);
}
.inst-term-host {
  position: absolute;
  inset: 8px 10px;
}
/* 两个都要写死。只写 `.xterm` 的话 viewport 仍按自己算出的高度滚，差出的那一行
   就卡在可视区外面 —— 用户看到的是「最后一行显示不全」。 */
.inst-term-host :deep(.xterm),
.inst-term-host :deep(.xterm-viewport) {
  width: 100% !important;
  height: 100% !important;
}
/*
 * 上面那条写死高度之后必然会露馅：整行数乘行高（44 × 15 = 660）填不满被撑到 665 的
 * viewport，底下永远剩几像素。而 `xterm.css` 给 `.xterm-viewport` 的默认底色是**纯黑**
 * —— 浅色主题下那几像素就是一条黑杠。
 *
 * `style.css` 里 `.terminal-host` 早就配了这一条，当时只抄了写死高度那半边。
 */
.inst-term-host :deep(.xterm-viewport) {
  background-color: transparent !important;
}
</style>
