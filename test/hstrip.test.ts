// 横向滑动区的算术部分。
//
// 这四个函数以前散在 `TerminalStrip.vue` 里，没有一行测试 —— 而它们全是边界题：
// 夹不住就飘出空白，轴选错了触控板会反向带走，露出算多了会把上一项顶出屏幕。
// 抽进 `hstrip.ts` 之后工具管理页的角标条也吃这一份，改一处两处一起对。

import { describe, it, expect } from 'vitest'
import { maxScrollOf, clampScroll, wheelDelta, revealDelta } from '../src/hstrip'

describe('maxScrollOf', () => {
  it('内容比容器宽，能滑的就是多出来那截', () => {
    expect(maxScrollOf(800, 500)).toBe(300)
  })

  /** 装得下还允许滑的话，轨道会被拖出一片空白，右边的内容凭空消失。 */
  it('装得下就一点都不能滑', () => {
    expect(maxScrollOf(300, 500)).toBe(0)
    expect(maxScrollOf(500, 500)).toBe(0)
  })
})

describe('clampScroll', () => {
  it('区间内原样放行', () => {
    expect(clampScroll(120, 300)).toBe(120)
  })

  /** 惯性滚轮和拖拽都会冲出界，越界之后露出来的是轨道外的空白。 */
  it('两头都夹回去', () => {
    expect(clampScroll(-40, 300)).toBe(0)
    expect(clampScroll(9999, 300)).toBe(300)
  })

  it('压根不能滑时只能停在 0', () => {
    expect(clampScroll(50, 0)).toBe(0)
  })
})

describe('wheelDelta', () => {
  /** 鼠标滚轮只有 deltaY，而这条是横向的 —— 不认 Y 就等于鼠标用户滑不动。 */
  it('只有竖向分量时用竖向', () => {
    expect(wheelDelta(0, 40)).toBe(40)
  })

  it('触控板横扫时用横向', () => {
    expect(wheelDelta(-60, 5)).toBe(-60)
  })

  /**
   * 取绝对值大的那个，而不是「有 X 就用 X」：触控板斜着扫一下两个分量都非 0，
   * 按符号相反的那个走，手往左推屏幕往右跑。
   */
  it('斜着扫按绝对值大的那个走', () => {
    expect(wheelDelta(-3, 55)).toBe(55)
    expect(wheelDelta(-55, 3)).toBe(-55)
  })

  it('两个都是 0 就是 0（调用方据此不拦事件）', () => {
    expect(wheelDelta(0, 0)).toBe(0)
  })
})

describe('revealDelta', () => {
  const VIEW_L = 100
  const VIEW_R = 500
  const MARGIN = 16

  /** 看得见就不动 —— 不加这条，点中间那一项列表也会自己滑一下。 */
  it('已经整个在视野里就不动', () => {
    expect(revealDelta(200, 300, VIEW_L, VIEW_R, MARGIN)).toBe(0)
  })

  it('左边被切了就往左滑出来', () => {
    expect(revealDelta(90, 180, VIEW_L, VIEW_R, MARGIN)).toBe(90 - (VIEW_L + MARGIN))
  })

  it('右边被切了就往右滑出来', () => {
    expect(revealDelta(430, 540, VIEW_L, VIEW_R, MARGIN)).toBe(540 - (VIEW_R - MARGIN))
  })

  /** 留边是为了别让它贴着边缘停下 —— 贴边看着还像被切着。 */
  it('贴着边缘也算没露出来', () => {
    expect(revealDelta(VIEW_L + 4, 200, VIEW_L, VIEW_R, MARGIN)).toBe(-12)
    expect(revealDelta(400, VIEW_R - 4, VIEW_L, VIEW_R, MARGIN)).toBe(12)
  })

  /** 比视野还宽的一项两头都够不着：先保左边，右边留给用户自己滑。 */
  it('比视野还宽时先对齐左边', () => {
    expect(revealDelta(50, 900, VIEW_L, VIEW_R, MARGIN)).toBe(50 - (VIEW_L + MARGIN))
  })
})
