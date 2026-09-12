import { beforeEach, describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import MemoDiffModal from '../../src/modals/MemoDiffModal.vue'
import { vTooltip } from '../../src/tooltip'
import { setLang } from '../../src/settings'
import type { MemoDiff } from '../../src/types'

beforeEach(() => setLang('en'))

const diff = (over: Partial<MemoDiff> = {}): MemoDiff => ({
  left: '/a/RTK.md',
  right: '/b/RTK.md',
  hunks: [
    {
      oldStart: 1,
      newStart: 1,
      lines: [
        { kind: 'del', oldNo: 1, newNo: null, text: 'old line' },
        { kind: 'add', oldNo: null, newNo: 1, text: 'new line' },
        { kind: 'add', oldNo: null, newNo: 2, text: 'extra' },
        { kind: 'ctx', oldNo: 2, newNo: 3, text: 'shared' },
      ],
    },
  ],
  same: false,
  truncated: false,
  clipped: false,
  ...over,
})

type Props = InstanceType<typeof MemoDiffModal>['$props']
const factory = (props: Partial<Props> = {}) =>
  mount(MemoDiffModal, {
    props: {
      show: true,
      title: 'Both copies of RTK.md',
      diff: diff(),
      leftLabel: '~/.grok/RTK.md',
      rightLabel: '~/.codex/RTK.md',
      ...props,
    } as Props,
    global: { directives: { tooltip: vTooltip } },
  })

describe('MemoDiffModal — split layout', () => {
  it('puts each side under its own column header', () => {
    const w = factory()
    const heads = w.findAll('.memo-split-head').map((h) => h.text())
    expect(heads).toEqual(['− ~/.grok/RTK.md', '+ ~/.codex/RTK.md'])
  })

  it('pairs a deleted line with its replacement on one row', () => {
    const w = factory()
    const cells = w.findAll('.memo-split-cell')
    expect(cells[0].classes()).toContain('del')
    expect(cells[0].text()).toContain('old line')
    expect(cells[1].classes()).toContain('add')
    expect(cells[1].text()).toContain('new line')
  })

  it('pads the left side where the right has an extra line', () => {
    const w = factory()
    const cells = w.findAll('.memo-split-cell')
    expect(cells[2].classes()).toContain('pad')
    expect(cells[2].text()).toBe('')
    expect(cells[3].text()).toContain('extra')
  })

  it('shows a context line on both sides with its own line number', () => {
    const w = factory()
    const cells = w.findAll('.memo-split-cell')
    expect(cells[4].find('.memo-split-no').text()).toBe('2')
    expect(cells[5].find('.memo-split-no').text()).toBe('3')
  })

  it('says so instead of drawing an empty box when the two are identical', () => {
    const w = factory({ diff: diff({ same: true, hunks: [] }) })
    expect(w.find('.memo-split-msg').text()).toContain('identical')
    expect(w.findAll('.memo-split-cell')).toHaveLength(0)
    // 标题还在 —— 「哪两份一样」得说清楚。
    expect(w.findAll('.memo-split-head')).toHaveLength(2)
  })
})

describe('MemoDiffModal — actions', () => {
  // 左右顺序跟着上面两栏走：左边那个按钮是「以左边为准」。
  it('offers both directions, the left-sourced one first', () => {
    const w = factory({ primaryLabel: 'left wins', secondaryLabel: 'right wins' })
    const labels = w.findAll('.modal-actions .btn').map((b) => b.text())
    expect(labels).toEqual(['Close', 'left wins', 'right wins'])
  })

  it('emits the matching event for each direction', async () => {
    const w = factory({ primaryLabel: 'left wins', secondaryLabel: 'right wins' })
    const [, primary, secondary] = w.findAll('.modal-actions .btn')
    await secondary.trigger('click')
    await primary.trigger('click')
    expect(w.emitted('secondary')).toHaveLength(1)
    expect(w.emitted('primary')).toHaveLength(1)
  })

  it('leaves only Close when neither direction is offered', () => {
    const w = factory()
    expect(w.findAll('.modal-actions .btn')).toHaveLength(1)
  })

  it('spins on the button that was clicked, not the other one', async () => {
    const w = factory({ primaryLabel: 'left wins', secondaryLabel: 'right wins' })
    await w.findAll('.modal-actions .btn')[2].trigger('click')
    await w.setProps({ busy: true })
    const [, primary, secondary] = w.findAll('.modal-actions .btn')
    expect(secondary.find('.chip-spinner').exists()).toBe(true)
    expect(primary.find('.chip-spinner').exists()).toBe(false)
  })

  it('forgets which button was clicked once the work is done', async () => {
    const w = factory({ primaryLabel: 'left wins', secondaryLabel: 'right wins' })
    await w.findAll('.modal-actions .btn')[2].trigger('click')
    await w.setProps({ busy: true })
    await w.setProps({ busy: false })
    await w.setProps({ busy: true })
    const [, primary, secondary] = w.findAll('.modal-actions .btn')
    expect(secondary.find('.chip-spinner').exists()).toBe(false)
    expect(primary.find('.chip-spinner').exists()).toBe(true)
  })
})
