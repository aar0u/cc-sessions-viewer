import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

const {
  cursorPositionMock,
  eventHandlers,
  invokeMock,
  listenMock,
  monitorFromPointMock,
  outerPositionMock,
  outerSizeMock,
  scaleFactorMock,
  setPositionMock,
  setSizeMock,
  showMock,
} = vi.hoisted(() => ({
  cursorPositionMock: vi.fn(),
  eventHandlers: new Map<string, (event: { payload?: any }) => void | Promise<void>>(),
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
  monitorFromPointMock: vi.fn(),
  outerPositionMock: vi.fn(),
  outerSizeMock: vi.fn(),
  scaleFactorMock: vi.fn(),
  setPositionMock: vi.fn(),
  setSizeMock: vi.fn(),
  showMock: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string) => `asset://${path}`,
  invoke: invokeMock,
}))
vi.mock('@tauri-apps/api/event', () => ({
  emitTo: vi.fn().mockResolvedValue(undefined),
  listen: listenMock,
}))
vi.mock('@tauri-apps/api/window', () => ({
  LogicalSize: class LogicalSize {
    constructor(public width: number, public height: number) {}
  },
  PhysicalPosition: class PhysicalPosition {
    constructor(public x: number, public y: number) {}
  },
  cursorPosition: cursorPositionMock,
  monitorFromPoint: monitorFromPointMock,
  getCurrentWindow: () => ({
    outerPosition: outerPositionMock,
    outerSize: outerSizeMock,
    scaleFactor: scaleFactorMock,
    setPosition: setPositionMock,
    setSize: setSizeMock,
    show: showMock,
  }),
}))

import DesktopPet from '../../src/components/DesktopPet.vue'
import {
  desktopPetCatalog,
  setDesktopPetCharacter,
  setDesktopPetPosition,
  setDesktopPetSize,
} from '../../src/desktopPet'

const petCatalog = {
  pets: [
    {
      key: 'codex:codex',
      id: 'codex',
      displayName: 'Codex',
      description: 'The original Codex companion.',
      spriteVersionNumber: 2,
      spritesheetPath: 'C:/pets/codex.webp',
      source: 'codex',
    },
    {
      key: 'codex:bsod',
      id: 'bsod',
      displayName: 'BSOD',
      description: 'A tiny blue-screen gremlin.',
      spriteVersionNumber: 2,
      spritesheetPath: 'C:/pets/bsod.webp',
      source: 'codex',
    },
  ],
  customDirectory: 'C:/Users/test/.codex/pets',
  codexInstalled: true,
}

let taskSnapshot: Array<{
  agent: 'claude' | 'codex' | 'agy'
  path: string
  state: 'started' | 'blocked' | 'completed' | 'failed'
  title: string
  updatedAt: number
}> = []

beforeEach(() => {
  vi.useRealTimers()
  setDesktopPetCharacter('codex:codex')
  setDesktopPetSize(112)
  setDesktopPetPosition(null)
  desktopPetCatalog.value = null
  eventHandlers.clear()
  taskSnapshot = []
  invokeMock.mockReset().mockImplementation((command: string) => {
    if (command === 'desktop_pet_catalog') return Promise.resolve(petCatalog)
    if (command === 'desktop_pet_tasks') return Promise.resolve(taskSnapshot)
    return Promise.resolve(undefined)
  })
  cursorPositionMock.mockReset().mockResolvedValue({ x: 0, y: 0 })
  outerPositionMock.mockReset().mockResolvedValue({ x: 100, y: 200 })
  outerSizeMock.mockReset().mockResolvedValue({ width: 356, height: 320 })
  scaleFactorMock.mockReset().mockResolvedValue(1)
  setPositionMock.mockReset().mockResolvedValue(undefined)
  setSizeMock.mockReset().mockResolvedValue(undefined)
  showMock.mockReset().mockResolvedValue(undefined)
  monitorFromPointMock.mockReset().mockResolvedValue({
    position: { x: 0, y: 0 },
    size: { width: 1920, height: 1080 },
  })
  listenMock.mockReset().mockImplementation(
    (event: string, handler: (event: { payload?: any }) => void | Promise<void>) => {
      eventHandlers.set(event, handler)
      return Promise.resolve(vi.fn())
    },
  )
})

async function factory() {
  const wrapper = mount(DesktopPet, { attachTo: document.body })
  await flushPromises()
  return wrapper
}

async function dispatchPointer(
  element: EventTarget,
  type: string,
  init: MouseEventInit & { pointerId: number; timeMs?: number },
) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, ...init })
  Object.defineProperty(event, 'pointerId', { value: init.pointerId })
  if (init.timeMs != null) Object.defineProperty(event, 'timeStamp', { value: init.timeMs })
  element.dispatchEvent(event)
  await flushPromises()
}

describe('DesktopPet', () => {
  it('shrinks the transparent window to the visible avatar area', async () => {
    setDesktopPetSize(80)
    const wrapper = await factory()

    expect(setSizeMock).toHaveBeenCalledWith(expect.objectContaining({
      width: 96,
      height: 103,
    }))
    wrapper.unmount()
  })

  it('keeps the transparent window at avatar size while activity drives animation', async () => {
    taskSnapshot = [
      { agent: 'codex', path: 'running', state: 'started', title: 'Running task', updatedAt: 40 },
      { agent: 'claude', path: 'ready', state: 'completed', title: 'Ready task', updatedAt: 30 },
      { agent: 'agy', path: 'failed', state: 'failed', title: 'Failed task', updatedAt: 20 },
      { agent: 'claude', path: 'approval', state: 'blocked', title: 'Approval task', updatedAt: 10 },
    ]
    setDesktopPetSize(80)
    const wrapper = await factory()

    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('waiting')
    expect(wrapper.find('.activity-trigger').exists()).toBe(false)
    expect(wrapper.find('.activity-tray').exists()).toBe(false)
    for (const [size] of setSizeMock.mock.calls) {
      expect(size).toEqual(expect.objectContaining({ width: 96, height: 103 }))
    }
    wrapper.unmount()
  })

  it('renders a fallback avatar when no pet spritesheets are available', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'desktop_pet_catalog') {
        return Promise.resolve({
          pets: [],
          customDirectory: 'C:/Users/test/.codex/pets',
          codexInstalled: false,
        })
      }
      if (command === 'desktop_pet_tasks') return Promise.resolve(taskSnapshot)
      return Promise.resolve(undefined)
    })

    const wrapper = await factory()

    expect(wrapper.find('.pet-atlas-sprite').exists()).toBe(false)
    expect(wrapper.get('.desktop-pet-fallback').attributes('data-character')).toBe('codex:codex')
    expect(wrapper.get('.desktop-pet-fallback').attributes('data-state')).toBe('waving')
    expect(showMock).toHaveBeenCalledOnce()
    wrapper.unmount()
  })

  it('renders the Codex avatar without legacy task UI when no activity exists', async () => {
    const wrapper = await factory()

    expect(wrapper.get('.pet-atlas-sprite').attributes('data-character')).toBe('codex:codex')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('waving')
    expect(wrapper.find('.task-panel').exists()).toBe(false)
    expect(wrapper.find('.status-notices').exists()).toBe(false)
    expect(wrapper.find('.activity-trigger').exists()).toBe(false)
    expect(invokeMock).toHaveBeenCalledWith('desktop_pet_tasks')
    expect(showMock).toHaveBeenCalledOnce()
    wrapper.unmount()
  })

  it('restores the highest-priority activity and returns to it after hover', async () => {
    taskSnapshot = [
      { agent: 'codex', path: 'running', state: 'started', title: 'Running task', updatedAt: 40 },
      { agent: 'claude', path: 'ready', state: 'completed', title: 'Ready task', updatedAt: 30 },
      { agent: 'agy', path: 'failed', state: 'failed', title: 'Failed task', updatedAt: 20 },
      { agent: 'claude', path: 'approval', state: 'blocked', title: 'Approval task', updatedAt: 10 },
    ]
    const wrapper = await factory()
    const avatar = wrapper.get('.avatar-button')

    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('waiting')
    expect(wrapper.find('.activity-trigger').exists()).toBe(false)
    await avatar.trigger('pointerenter')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('waving')
    await avatar.trigger('pointerleave')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('waiting')

    wrapper.unmount()
  })

  it('refreshes activity from realtime Hook and acknowledgement events', async () => {
    const wrapper = await factory()
    taskSnapshot = [{
      agent: 'codex',
      path: 'C:/sessions/ready.jsonl',
      state: 'completed',
      title: 'Ready task',
      updatedAt: 50,
    }]

    await eventHandlers.get('terminal-turn://state')?.({ payload: {} })
    await flushPromises()
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('review')
    expect(wrapper.find('.activity-trigger').exists()).toBe(false)

    taskSnapshot = []
    await eventHandlers.get('desktop-pet://activity-acknowledged')?.({ payload: {} })
    await flushPromises()
    expect(wrapper.find('.activity-trigger').exists()).toBe(false)
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('waving')
    wrapper.unmount()
  })

  it('uses a stable waving hover without competing gaze frames', async () => {
    vi.useFakeTimers()
    const wrapper = await factory()
    const avatar = wrapper.get('.avatar-button')

    await vi.advanceTimersByTimeAsync(8000)
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('idle')

    await avatar.trigger('pointerenter')
    await dispatchPointer(avatar.element, 'pointermove', {
      pointerId: 1,
      clientX: 100,
      clientY: 60,
    })
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('waving')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-look-frame')).toBeUndefined()

    await avatar.trigger('pointerleave')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('idle')
    wrapper.unmount()
    vi.useRealTimers()
  })

  it('updates the selected pet and size from the shared preference event', async () => {
    const wrapper = await factory()

    await eventHandlers.get('desktop-pet://preferences')?.({
      payload: { character: 'codex:bsod', size: 176 },
    })
    await flushPromises()

    expect(wrapper.get('.pet-atlas-sprite').attributes('data-character')).toBe('codex:bsod')
    expect(wrapper.get('.pet-atlas-sprite').attributes('aria-label')).toBe('BSOD')
    expect(wrapper.get('.pet-atlas-sprite').attributes('style')).toContain('bsod.webp')
    expect(wrapper.get('.avatar-button').attributes('style')).toContain('width: 176px')
    wrapper.unmount()
  })

  it('temporarily prioritizes cursor gaze over task animation, then restores it', async () => {
    vi.useFakeTimers()
    taskSnapshot = [{
      agent: 'codex',
      path: 'C:/sessions/ready.jsonl',
      state: 'completed',
      title: 'Ready task',
      updatedAt: 50,
    }]
    let cursor = { x: 156, y: 261 }
    cursorPositionMock.mockImplementation(() => Promise.resolve(cursor))
    const wrapper = await factory()
    const area = wrapper.get('.avatar-button').element as HTMLElement
    area.getBoundingClientRect = () => ({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 112,
      bottom: 121.333,
      width: 112,
      height: 121.333,
      toJSON: () => ({}),
    })

    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('review')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-look-frame')).toBeUndefined()

    cursor = { x: 256, y: 261 }
    // 指针轮询间隔（DesktopPet 的 CURSOR_POLL_MS）。
    await vi.advanceTimersByTimeAsync(100)
    await flushPromises()

    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('review')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-look-frame')).toBe('4')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-row')).toBe('9')

    await vi.advanceTimersByTimeAsync(1000)
    await flushPromises()

    expect(wrapper.get('.pet-atlas-sprite').attributes('data-look-frame')).toBeUndefined()
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('review')
    wrapper.unmount()
    vi.useRealTimers()
  })

  it('uses directional running feedback after the drag threshold', async () => {
    const wrapper = await factory()
    const avatar = wrapper.get('.avatar-button')

    await dispatchPointer(avatar.element, 'pointerdown', {
      button: 0,
      pointerId: 7,
      screenX: 100,
      screenY: 100,
    })
    await dispatchPointer(avatar.element, 'pointermove', {
      pointerId: 7,
      screenX: 112,
      screenY: 100,
    })

    expect(avatar.classes()).toContain('dragging')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('running-right')
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-look-frame')).toBeUndefined()

    await dispatchPointer(avatar.element, 'pointermove', {
      pointerId: 7,
      screenX: 100,
      screenY: 100,
    })
    expect(wrapper.get('.pet-atlas-sprite').attributes('data-state')).toBe('running-left')
    wrapper.unmount()
  })

  it('treats release movement beyond the threshold as a drag without an intermediate move event', async () => {
    const wrapper = await factory()
    const avatar = wrapper.get('.avatar-button')

    await dispatchPointer(avatar.element, 'pointerdown', {
      button: 0,
      pointerId: 11,
      screenX: 100,
      screenY: 100,
    })
    await dispatchPointer(window, 'pointerup', {
      pointerId: 11,
      screenX: 110,
      screenY: 100,
    })

    expect(invokeMock).not.toHaveBeenCalledWith('focus_desktop_pet_main')
    expect(setPositionMock).toHaveBeenCalledWith(expect.objectContaining({ x: 110, y: 200 }))
    wrapper.unmount()
  })

  it('treats a click without movement like the Codex mascot button', async () => {
    const wrapper = await factory()
    const avatar = wrapper.get('.avatar-button')

    await dispatchPointer(avatar.element, 'pointerdown', {
      button: 0,
      pointerId: 3,
      screenX: 100,
      screenY: 100,
    })
    await dispatchPointer(avatar.element, 'pointerup', {
      pointerId: 3,
      screenX: 100,
      screenY: 100,
    })

    expect(invokeMock).toHaveBeenCalledWith('focus_desktop_pet_main')
    wrapper.unmount()
  })

  it('stops at the release point and persists without post-release movement', async () => {
    vi.useFakeTimers()
    let windowPosition = { x: 100, y: 200 }
    outerPositionMock.mockImplementation(() => Promise.resolve({ ...windowPosition }))
    setPositionMock.mockImplementation((position: { x: number; y: number }) => {
      windowPosition = { x: position.x, y: position.y }
      return Promise.resolve()
    })
    const wrapper = await factory()
    const avatar = wrapper.get('.avatar-button')

    await dispatchPointer(avatar.element, 'pointerdown', {
      button: 0,
      pointerId: 9,
      screenX: 100,
      screenY: 100,
      timeMs: 0,
    })
    await dispatchPointer(avatar.element, 'pointermove', {
      pointerId: 9,
      screenX: 200,
      screenY: 100,
      timeMs: 100,
    })
    await dispatchPointer(avatar.element, 'pointerup', {
      pointerId: 9,
      screenX: 200,
      screenY: 100,
      timeMs: 100,
    })
    await flushPromises()

    const callsAtRelease = setPositionMock.mock.calls.length
    expect(windowPosition).toEqual({ x: 200, y: 200 })
    await vi.advanceTimersByTimeAsync(1000)
    await flushPromises()

    expect(setPositionMock).toHaveBeenCalledTimes(callsAtRelease)
    expect(windowPosition).toEqual({ x: 200, y: 200 })
    expect(JSON.parse(localStorage.getItem('desktopPetPosition:v1')!)).toEqual(windowPosition)
    wrapper.unmount()
    vi.useRealTimers()
  })
})
