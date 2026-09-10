<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { convertFileSrc } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  cursorPosition,
  getCurrentWindow,
  LogicalSize,
  monitorFromPoint,
  PhysicalPosition,
} from '@tauri-apps/api/window'
import {
  activeDesktopPet,
  dominantDesktopTaskState,
  desktopPetCharacter,
  desktopPetPosition,
  desktopPetSize,
  fetchDesktopPetTasks,
  focusDesktopPetMain,
  loadDesktopPetCatalog,
  setDesktopPetCharacter,
  setDesktopPetPosition,
  setDesktopPetSize,
  type DesktopPetCharacter,
  type DesktopTask,
  type DesktopTaskState,
} from '../desktopPet'
import DesktopPetFallback from './DesktopPetFallback.vue'
import PetAtlasPlayer, { type PetAnimationState } from './PetAtlasPlayer.vue'

type PointerSample = {
  screenX: number
  screenY: number
}

type DragSession = {
  hasMoved: boolean
  pointerId: number
  startScreenX: number
  startScreenY: number
  screenX: number
  screenY: number
  startWindow: Promise<{ x: number; y: number; scaleFactor: number }>
}

const DRAG_THRESHOLD = 4
const GAZE_HOLD_MS = 1000

const currentWindow = getCurrentWindow()
const characterArea = ref<HTMLElement>()
const hovered = ref(false)
const dragging = ref(false)
const dragState = ref<'running-left' | 'running-right' | null>(null)
const lookFrame = ref<number | null>(null)
const gazeActive = ref(false)
const baseState = ref<PetAnimationState>('waving')
const tasks = ref<DesktopTask[]>([])
const dominantTaskState = computed(() => dominantDesktopTaskState(tasks.value))
const taskPresentation: Record<DesktopTaskState, {
  animation: PetAnimationState
}> = {
  blocked: { animation: 'waiting' },
  failed: { animation: 'failed' },
  completed: { animation: 'review' },
  started: { animation: 'running' },
}
const activityAnimation = computed<PetAnimationState | null>(() => {
  const state = dominantTaskState.value
  return state ? taskPresentation[state].animation : null
})
const spritesheetSrc = computed(() => activeDesktopPet.value
  ? convertFileSrc(activeDesktopPet.value.spritesheetPath)
  : '')
const animationState = computed<PetAnimationState>(() => {
  if (dragState.value) return dragState.value
  if (hovered.value) return 'waving'
  return activityAnimation.value ?? baseState.value
})
const effectiveLookFrame = computed(() => {
  if (
    !gazeActive.value
    || hovered.value
    || dragging.value
    || activeDesktopPet.value?.spriteVersionNumber !== 2
  ) {
    return null
  }
  return lookFrame.value
})
const avatarStyle = computed(() => ({
  width: `${desktopPetSize.value}px`,
  height: `${desktopPetSize.value * 208 / 192}px`,
}))
const windowSize = computed(() => {
  const avatarWidth = desktopPetSize.value + 16
  const avatarHeight = Math.ceil(desktopPetSize.value * 208 / 192) + 16
  return {
    width: Math.ceil(avatarWidth),
    height: Math.ceil(avatarHeight),
  }
})

let dragSession: DragSession | null = null
let pendingWindowPosition: { x: number; y: number } | null = null
let moveFrame = 0
/** 全局指针轮询间隔（ms）。见 onMounted 里的说明。 */
const CURSOR_POLL_MS = 100
let cursorTimer: ReturnType<typeof setInterval> | null = null
let wakeTimer: ReturnType<typeof setTimeout> | null = null
let gazeTimer: ReturnType<typeof setTimeout> | null = null
let lastCursorPosition: { x: number; y: number } | null = null
let cursorPollPending = false
let unlisten: UnlistenFn[] = []

async function syncWindowSize() {
  const next = windowSize.value
  try {
    const [position, size, scaleFactor] = await Promise.all([
      currentWindow.outerPosition(),
      currentWindow.outerSize(),
      currentWindow.scaleFactor(),
    ])
    await currentWindow.setSize(new LogicalSize(next.width, next.height))
    const nextSize = await currentWindow.outerSize().catch(() => ({
      width: Math.round(next.width * scaleFactor),
      height: Math.round(next.height * scaleFactor),
    }))
    await currentWindow.setPosition(new PhysicalPosition(
      Math.round(position.x + size.width - nextSize.width),
      Math.round(position.y + size.height - nextSize.height),
    ))
  } catch {
    await currentWindow.setSize(new LogicalSize(next.width, next.height)).catch(() => {})
  }
}

async function refreshTasks() {
  try {
    tasks.value = await fetchDesktopPetTasks()
  } catch (error) {
    console.warn('[desktop-pet] failed to load task activity:', error)
  }
}

function pointerSample(event: PointerEvent): PointerSample {
  return { screenX: event.screenX, screenY: event.screenY }
}

function hasDragMovement(session: DragSession, sample: PointerSample) {
  return session.hasMoved
    || Math.abs(sample.screenX - session.startScreenX) >= DRAG_THRESHOLD
    || Math.abs(sample.screenY - session.startScreenY) >= DRAG_THRESHOLD
}

function cancelWindowMotion() {
  if (moveFrame) cancelAnimationFrame(moveFrame)
  moveFrame = 0
  pendingWindowPosition = null
}

function queueWindowPosition(position: { x: number; y: number }) {
  pendingWindowPosition = position
  if (moveFrame) return
  moveFrame = requestAnimationFrame(() => {
    moveFrame = 0
    const next = pendingWindowPosition
    pendingWindowPosition = null
    if (next) {
      void currentWindow.setPosition(new PhysicalPosition(Math.round(next.x), Math.round(next.y)))
    }
  })
}

async function persistCurrentPosition(fallback?: { x: number; y: number }) {
  try {
    const position = fallback ?? await currentWindow.outerPosition()
    setDesktopPetPosition({ x: Math.round(position.x), y: Math.round(position.y) })
  } catch {
    // Position persistence is best-effort when the window is closing.
  }
}

async function finishDragMotion(
  session: DragSession,
  release: PointerSample,
) {
  const start = await session.startWindow
  if (moveFrame) cancelAnimationFrame(moveFrame)
  moveFrame = 0
  pendingWindowPosition = null
  const position = {
    x: start.x + (release.screenX - session.startScreenX) * start.scaleFactor,
    y: start.y + (release.screenY - session.startScreenY) * start.scaleFactor,
  }
  await currentWindow.setPosition(new PhysicalPosition(Math.round(position.x), Math.round(position.y)))
  await persistCurrentPosition(position)
}

function beginDrag(event: PointerEvent) {
  if (
    event.button !== 0
    || event.ctrlKey
    || !(event.target instanceof Element)
    || event.target.closest('.no-drag')
  ) return

  event.preventDefault()
  cancelWindowMotion()
  event.currentTarget instanceof Element
    && event.currentTarget.setPointerCapture?.(event.pointerId)
  const sample = pointerSample(event)
  dragSession = {
    hasMoved: false,
    pointerId: event.pointerId,
    startScreenX: sample.screenX,
    startScreenY: sample.screenY,
    screenX: sample.screenX,
    screenY: sample.screenY,
    startWindow: Promise.all([
      currentWindow.outerPosition(),
      currentWindow.scaleFactor(),
    ]).then(([position, scaleFactor]) => ({ ...position, scaleFactor })),
  }
  dragState.value = null
}

function moveDrag(event: PointerEvent) {
  const session = dragSession
  if (!session || session.pointerId !== event.pointerId) return
  event.stopPropagation()
  const sample = pointerSample(event)
  const deltaX = sample.screenX - session.screenX
  const deltaY = sample.screenY - session.screenY
  if (Math.abs(deltaX) < DRAG_THRESHOLD && Math.abs(deltaY) < DRAG_THRESHOLD) return

  event.preventDefault()
  session.hasMoved = true
  session.screenX = sample.screenX
  session.screenY = sample.screenY
  dragging.value = true
  if (deltaX >= DRAG_THRESHOLD) dragState.value = 'running-right'
  else if (deltaX <= -DRAG_THRESHOLD) dragState.value = 'running-left'

  const totalX = sample.screenX - session.startScreenX
  const totalY = sample.screenY - session.startScreenY
  void session.startWindow.then((start) => {
    if (dragSession !== session) return
    queueWindowPosition({
      x: start.x + totalX * start.scaleFactor,
      y: start.y + totalY * start.scaleFactor,
    })
  })
}

function releasePointer(event: PointerEvent) {
  const element = event.currentTarget instanceof HTMLElement
    ? event.currentTarget
    : event.target instanceof HTMLElement ? event.target : null
  if (element?.hasPointerCapture?.(event.pointerId)) {
    element.releasePointerCapture?.(event.pointerId)
  }
}

function endDrag(event: PointerEvent) {
  const session = dragSession
  if (!session || session.pointerId !== event.pointerId) return
  dragSession = null
  releasePointer(event)
  const release = pointerSample(event)

  dragging.value = false
  dragState.value = null
  if (!hasDragMovement(session, release)) {
    void focusDesktopPetMain()
    return
  }
  event.preventDefault()
  void finishDragMotion(session, release)
}

function cancelDrag(event: PointerEvent) {
  const session = dragSession
  if (!session || session.pointerId !== event.pointerId) return
  dragSession = null
  releasePointer(event)
  dragging.value = false
  dragState.value = null
  const release = { screenX: session.screenX, screenY: session.screenY }
  if (session.hasMoved) void finishDragMotion(session, release)
  else void persistCurrentPosition()
}

function updateLookDirection(deltaX: number, deltaY: number, deadZone: number) {
  if (Math.hypot(deltaX, deltaY) <= deadZone) {
    lookFrame.value = null
    return
  }
  const angle = Math.atan2(deltaX, -deltaY)
  lookFrame.value = (Math.round(angle / (Math.PI / 8)) + 16) % 16
}

function activateGaze() {
  gazeActive.value = true
  if (gazeTimer) clearTimeout(gazeTimer)
  gazeTimer = setTimeout(() => {
    gazeActive.value = false
    gazeTimer = null
  }, GAZE_HOLD_MS)
}

function updateLocalLook(event: PointerEvent) {
  const bounds = characterArea.value?.getBoundingClientRect()
  if (!bounds) return
  if (!dragging.value) activateGaze()
  updateLookDirection(
    event.clientX - (bounds.left + bounds.width / 2),
    event.clientY - (bounds.top + bounds.height / 2),
    1,
  )
}

async function pollGlobalCursor() {
  if (cursorPollPending || !characterArea.value || dragging.value) return
  // 页面被隐藏 / 桌宠不可见时不轮询：看不见的视线跟随没有意义，白烧 IPC。
  if (document.hidden) return
  cursorPollPending = true
  try {
    // 先只问指针位置。原实现每一拍都并发三个 IPC（指针 + 窗口位置 + 缩放），
    // 20 拍/秒 = 60 次 IPC/秒，一直跑。指针没动时后两个纯属浪费 —— 而指针不动
    // 恰恰是绝大多数时间的状态。
    const pointer = await cursorPosition()
    const moved =
      !lastCursorPosition ||
      pointer.x !== lastCursorPosition.x ||
      pointer.y !== lastCursorPosition.y
    if (!moved) return
    if (lastCursorPosition) activateGaze()
    lastCursorPosition = { x: pointer.x, y: pointer.y }

    const [windowPosition, scaleFactor] = await Promise.all([
      currentWindow.outerPosition(),
      currentWindow.scaleFactor(),
    ])
    const bounds = characterArea.value.getBoundingClientRect()
    const centerX = windowPosition.x + (bounds.left + bounds.width / 2) * scaleFactor
    const centerY = windowPosition.y + (bounds.top + bounds.height / 2) * scaleFactor
    updateLookDirection(pointer.x - centerX, pointer.y - centerY, scaleFactor)
  } catch {
    // Local pointer events remain available in tests and unsupported environments.
  } finally {
    cursorPollPending = false
  }
}

async function restoreWindowPosition() {
  const saved = desktopPetPosition.value
  if (!saved) return
  const monitor = await monitorFromPoint(saved.x + 1, saved.y + 1)
  if (monitor) await currentWindow.setPosition(new PhysicalPosition(saved.x, saved.y))
}

onMounted(async () => {
  await loadDesktopPetCatalog().catch((error) => {
    console.warn('[desktop-pet] failed to load pet catalog:', error)
  })
  await restoreWindowPosition().catch(() => {})

  unlisten = await Promise.all([
    listen<{ character?: DesktopPetCharacter; size?: number }>(
      'desktop-pet://preferences',
      async (event) => {
        if (event.payload?.character) setDesktopPetCharacter(event.payload.character)
        if (event.payload?.size != null) setDesktopPetSize(event.payload.size)
        await loadDesktopPetCatalog().catch(() => {})
      },
    ),
    listen('terminal-turn://state', () => refreshTasks()),
    listen('desktop-pet://activity-acknowledged', () => refreshTasks()),
  ])
  await refreshTasks()
  await syncWindowSize()
  await currentWindow.show().catch(() => {})
  // 100ms（10 拍/秒）足够让视线跟随看起来是连续的；50ms 那一档带来的平滑度肉眼
  // 分辨不出，代价却是常驻的双倍 IPC。
  cursorTimer = setInterval(pollGlobalCursor, CURSOR_POLL_MS)
  wakeTimer = setTimeout(() => { baseState.value = 'idle' }, 8000)
  window.addEventListener('pointerup', endDrag)
  window.addEventListener('pointercancel', cancelDrag)
  void pollGlobalCursor()
})

onUnmounted(() => {
  cancelWindowMotion()
  if (cursorTimer) clearInterval(cursorTimer)
  if (wakeTimer) clearTimeout(wakeTimer)
  if (gazeTimer) clearTimeout(gazeTimer)
  window.removeEventListener('pointerup', endDrag)
  window.removeEventListener('pointercancel', cancelDrag)
  for (const stop of unlisten) stop()
})

watch(
  () => desktopPetSize.value,
  () => { void syncWindowSize() },
  { immediate: true },
)
</script>

<template>
  <main class="desktop-avatar-overlay" @pointermove="updateLocalLook">
    <div
      ref="characterArea"
      class="avatar-button"
      :class="{ dragging }"
      :style="avatarStyle"
      role="img"
      :aria-label="activeDesktopPet?.displayName || 'Codex pet'"
      @pointerenter="hovered = true"
      @pointerleave="hovered = false"
      @pointerdown="beginDrag"
      @pointermove="moveDrag"
      @pointerup="endDrag"
      @pointercancel="cancelDrag"
      @lostpointercapture="cancelDrag"
    >
      <PetAtlasPlayer
        v-if="spritesheetSrc"
        class="avatar-sprite"
        :src="spritesheetSrc"
        :state="animationState"
        :look-frame="effectiveLookFrame"
        :sprite-version-number="activeDesktopPet?.spriteVersionNumber"
        :label="activeDesktopPet?.displayName"
        :data-character="desktopPetCharacter"
      />
      <DesktopPetFallback
        v-else
        class="avatar-sprite"
        :state="animationState"
        :label="activeDesktopPet?.displayName || 'Codex pet'"
        :data-character="desktopPetCharacter"
      />
    </div>
  </main>
</template>

<style scoped>
:global(html),
:global(body),
:global(#app) {
  width: 100%;
  height: 100%;
  margin: 0;
  overflow: hidden;
  background: transparent !important;
}

.desktop-avatar-overlay {
  position: relative;
  width: 100%;
  height: 100%;
  overflow: hidden;
  user-select: none;
}

.avatar-button {
  position: absolute;
  right: 8px;
  bottom: 8px;
  cursor: grab;
  touch-action: none;
}

.avatar-button.dragging {
  cursor: grabbing;
}

.avatar-sprite {
  width: 100%;
  pointer-events: none;
}

</style>
