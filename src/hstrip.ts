// 一条「没有滚动条的横向滑动区」。
//
// 两个地方要同一个东西：会话页顶上的 tab 条（`TerminalStrip.vue`），和工具管理页
// 健康条里那排角标（`ToolsSkillsPanel.vue`）。两处的需求一模一样 —— 内容比容器宽
// 时能滑，但**不要**露出那条横向滚动条：它是 16px 高的一根灰杠，压在 26px 的 tab
// 和 22px 的药丸底下，比被它藏起来的那半个 tab 还难看。
//
// 做法是把内容整个放进一条 `.track`，用 `transform: translateX(-scrollX)` 平移，
// 外面那层 `overflow: hidden` 负责裁。滚轮 / 拖空白处直接改 scrollX（关掉过渡，
// 1:1 跟手）；程序化露出某一项时保留过渡，滑过去让人看见发生了什么。
//
// 抽在这儿而不是各写一份：这套东西有四个互相咬合的状态（scrollX、maxScroll、
// panning、两侧还能不能滑），拷第二份就是拷四个状态的同步时机，改一处漏一处。

import { computed, nextTick, onUnmounted, ref, watch, type ComputedRef, type Ref } from 'vue'

/** 还能往左滑多少 —— 内容比容器窄时是 0（也就是「压根不用滑」）。 */
export function maxScrollOf(trackWidth: number, viewportWidth: number): number {
  return Math.max(0, trackWidth - viewportWidth)
}

/** 把滑动位置夹回 `[0, max]`。惯性滚轮和拖拽都会冲出界，越界之后轨道会飘出空白。 */
export function clampScroll(x: number, max: number): number {
  return Math.max(0, Math.min(x, max))
}

/**
 * 滚轮事件取哪个轴。
 *
 * 触控板横扫给的是 `deltaX`，鼠标滚轮只有 `deltaY`，而这条是横向的 —— 两个都得认，
 * 取绝对值大的那个，否则触控板斜着扫一下会被竖向分量反着带走。
 */
export function wheelDelta(deltaX: number, deltaY: number): number {
  return Math.abs(deltaX) > Math.abs(deltaY) ? deltaX : deltaY
}

/**
 * 要把某一项完整露出来，滑动位置得改多少（0 = 它已经整个在视野里，别动）。
 *
 * `margin` 是留边：贴着边缘停下来的那一项看着像还被切着，人还要再滑一下确认。
 */
export function revealDelta(
  elLeft: number,
  elRight: number,
  viewLeft: number,
  viewRight: number,
  margin: number,
): number {
  if (elLeft < viewLeft + margin) return elLeft - (viewLeft + margin)
  if (elRight > viewRight - margin) return elRight - (viewRight - margin)
  return 0
}

export interface HStripOptions {
  /**
   * 按在这些元素上时**不**当成「拖空白处平移」。tab 本体的按下要留给排序拖拽，
   * 两边都响应的话，想拖着换顺序会先把整条轨道带跑。
   */
  grabExclude?: string
  /** 程序化露出某一项时留的边，默认 16px。 */
  revealMargin?: number
}

export interface HStrip {
  /** 跟手期间为 true：模板拿它关掉轨道上的 transition。 */
  panning: Ref<boolean>
  canLeft: ComputedRef<boolean>
  canRight: ComputedRef<boolean>
  trackStyle: ComputedRef<{ transform: string }>
  /** 内容或容器尺寸变了之后重新量一遍（容器自身的变化已经有 ResizeObserver 盯着）。 */
  measure: () => void
  /** 把某个元素完整滑进视野；已经看得见就什么都不做。 */
  revealEl: (el: HTMLElement | null | undefined) => void
  onWheel: (ev: WheelEvent) => void
  onPanPointerDown: (ev: PointerEvent) => void
}

/**
 * 两个容器的 ref 由调用方声明后传进来，不在这儿创建：`<script setup>` 里
 * `ref="xxx"` 只认**直接声明**的那个 ref，从返回值里解构出来的同一个对象编译器
 * 不当它是模板 ref（vue-tsc 会报「声明了没用」，运行期也只是碰巧能赋上）。
 */
export function useHStrip(
  viewportRef: Ref<HTMLElement | undefined>,
  trackRef: Ref<HTMLElement | undefined>,
  options: HStripOptions = {},
): HStrip {
  const scrollX = ref(0)
  const maxScroll = ref(0)
  // panning=true 时关掉 transition，让滚轮 / 拖拽 1:1 跟手；程序化滑动时为 false 走动画。
  const panning = ref(false)
  const canLeft = computed(() => scrollX.value > 0.5)
  const canRight = computed(() => scrollX.value < maxScroll.value - 0.5)
  const trackStyle = computed(() => ({ transform: `translateX(${-scrollX.value}px)` }))

  function measure() {
    const vp = viewportRef.value
    const tr = trackRef.value
    maxScroll.value = vp && tr ? maxScrollOf(tr.scrollWidth, vp.clientWidth) : 0
    if (scrollX.value > maxScroll.value) scrollX.value = maxScroll.value
  }
  function setScroll(x: number) {
    scrollX.value = clampScroll(x, maxScroll.value)
  }

  // 滚轮 / 触控板 → 横向平移（取代原生横向滚动）
  let wheelIdleTimer = 0
  function onWheel(ev: WheelEvent) {
    if (maxScroll.value <= 0) return
    const delta = wheelDelta(ev.deltaX, ev.deltaY)
    if (!delta) return
    ev.preventDefault()
    panning.value = true
    setScroll(scrollX.value + delta)
    window.clearTimeout(wheelIdleTimer)
    wheelIdleTimer = window.setTimeout(() => (panning.value = false), 140)
  }

  // 拖拽空白处 → 平移
  let pan: { startX: number; startScroll: number } | null = null
  function onPanPointerDown(ev: PointerEvent) {
    if (ev.button !== 0 || maxScroll.value <= 0) return
    const target = ev.target as HTMLElement | null
    if (options.grabExclude && target?.closest(options.grabExclude)) return
    pan = { startX: ev.clientX, startScroll: scrollX.value }
    panning.value = true
    window.addEventListener('pointermove', onPanPointerMove)
    window.addEventListener('pointerup', onPanPointerUp)
    window.addEventListener('pointercancel', onPanPointerUp)
  }
  function onPanPointerMove(ev: PointerEvent) {
    if (!pan) return
    setScroll(pan.startScroll - (ev.clientX - pan.startX))
  }
  function onPanPointerUp() {
    pan = null
    panning.value = false
    window.removeEventListener('pointermove', onPanPointerMove)
    window.removeEventListener('pointerup', onPanPointerUp)
    window.removeEventListener('pointercancel', onPanPointerUp)
  }

  function revealEl(el: HTMLElement | null | undefined) {
    measure()
    const vp = viewportRef.value
    if (!vp || !el || maxScroll.value <= 0) return
    const at = el.getBoundingClientRect()
    const box = vp.getBoundingClientRect()
    const dx = revealDelta(at.left, at.right, box.left, box.right, options.revealMargin ?? 16)
    if (dx === 0) return
    panning.value = false // 程序化滑动：保留 transition 动画
    setScroll(scrollX.value + dx)
  }

  let ro: ResizeObserver | null = null
  watch(
    viewportRef,
    (el) => {
      ro?.disconnect()
      ro = null
      if (!el || typeof ResizeObserver === 'undefined') return
      ro = new ResizeObserver(() => measure())
      ro.observe(el)
      // 轨道也要盯：容器没变但内容变宽（多了一个 tab / 一个角标）时，只盯容器量不出来。
      nextTick(() => {
        if (trackRef.value && ro) ro.observe(trackRef.value)
        measure()
      })
    },
    { immediate: true },
  )

  onUnmounted(() => {
    ro?.disconnect()
    window.clearTimeout(wheelIdleTimer)
    window.removeEventListener('pointermove', onPanPointerMove)
    window.removeEventListener('pointerup', onPanPointerUp)
    window.removeEventListener('pointercancel', onPanPointerUp)
  })

  return {
    panning,
    canLeft,
    canRight,
    trackStyle,
    measure,
    revealEl,
    onWheel,
    onPanPointerDown,
  }
}
