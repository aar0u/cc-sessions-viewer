// hover 跟随浮块的位置作废。
//
// 这条修的是一个**看上去完全不像浮块干的**的 bug：列表被过滤短了之后整个空白。
// 浮块是 `position: absolute` + `top: var(--spot-y)` + 真实高度，会算进滚动容器的
// scrollHeight；`--spot-y` 停在上一批某一行的偏移量上，scrollHeight 就还是上一批
// 那么长，浏览器于是不收回 `scrollTop`，剩下的几行留在视口上方。

import { describe, it, expect } from 'vitest'
import { resetSpotlight, revealSelected } from '../src/listScroll'

function make() {
  const list = document.createElement('div')
  const spot = document.createElement('div')
  list.className = 'scroll-area has-spot'
  spot.className = 'list-spotlight'
  spot.style.setProperty('--spot-y', '860px')
  spot.style.setProperty('--spot-h', '65px')
  list.appendChild(spot)
  return { list, spot }
}

describe('resetSpotlight', () => {
  it('把位置和高度都收回原点', () => {
    const { list, spot } = make()
    resetSpotlight(list, spot)
    expect(spot.style.getPropertyValue('--spot-y')).toBe('0px')
    expect(spot.style.getPropertyValue('--spot-h')).toBe('0px')
  })

  /** 高度不一起收的话，scrollHeight 仍被撑着一行的高度 —— 那正是要修的东西。 */
  it('高度也收 —— 只挪位置不够', () => {
    const { list, spot } = make()
    resetSpotlight(list, spot)
    expect(spot.style.getPropertyValue('--spot-h')).not.toBe('65px')
  })

  it('顺手把 has-spot 摘掉，下一次 mouseover 再重新亮起来', () => {
    const { list, spot } = make()
    resetSpotlight(list, spot)
    expect(list.classList.contains('has-spot')).toBe(false)
  })

  /** 此刻浮块是透明的，滑给谁看都看不见，还要多跑一次 34ms 的 top 过渡 —— 过渡
      期间 scrollHeight 是个中间值，等于 bug 少一半但还在。 */
  it('直接跳回去，不走那段 top 过渡', () => {
    const { list, spot } = make()
    resetSpotlight(list, spot)
    expect(spot.classList.contains('no-slide')).toBe(true)
  })

  /** 组件还没挂上 / 已经卸载时这两个 ref 都是 undefined，不能炸。 */
  it('两个元素都还没有时什么都不做', () => {
    expect(() => resetSpotlight(undefined, undefined)).not.toThrow()
  })

  it('只有容器没有浮块时也不炸，该摘的类照样摘', () => {
    const { list } = make()
    expect(() => resetSpotlight(list, undefined)).not.toThrow()
    expect(list.classList.contains('has-spot')).toBe(false)
  })
})


// ---------------------------------------------------------------------------

/**
 * 先用「来自 github」筛到 2 条、点中其中一条，再把筛选关掉 —— 列表回到 50 条，而
 * 选中那条按排序落在中段，屏幕上一眼看不见它。右边详情还画着它，左边却找不着。
 *
 * jsdom 没有布局，`getBoundingClientRect` 恒为 0，所以这里把它按到位再断言。
 */
describe('revealSelected', () => {
  function box(el: HTMLElement, top: number, bottom: number) {
    el.getBoundingClientRect = () => ({ top, bottom, height: bottom - top, left: 0, right: 0, width: 0, x: 0, y: top, toJSON: () => ({}) }) as DOMRect
  }

  function make(rowTop: number, rowBottom: number) {
    const list = document.createElement('div')
    const row = document.createElement('button')
    row.className = 'tools-row active'
    list.appendChild(row)
    box(list, 100, 800)
    box(row, rowTop, rowBottom)
    let scrolled: ScrollIntoViewOptions | undefined
    row.scrollIntoView = ((o?: ScrollIntoViewOptions) => {
      scrolled = o
    }) as HTMLElement['scrollIntoView']
    return { list, row, scrolledTo: () => scrolled }
  }

  it('选中项在屏幕外就滚过去', () => {
    const { list, scrolledTo } = make(2000, 2065)
    revealSelected(list, '.tools-row.active')
    expect(scrolledTo()).toEqual({ block: 'center' })
  })

  it('在上方滚出去了也滚回来', () => {
    const { list, scrolledTo } = make(-645, -580)
    revealSelected(list, '.tools-row.active')
    expect(scrolledTo()).toEqual({ block: 'center' })
  })

  /** 不加这一条的话，选中第二行时也会被滚成居中 —— 用户没要求任何事，列表自己动了。 */
  it('已经看得见就一动不动', () => {
    const { list, scrolledTo } = make(150, 215)
    revealSelected(list, '.tools-row.active')
    expect(scrolledTo()).toBeUndefined()
  })

  /** 贴着边缘、露出来一半的那种：只露一半就是没看全，该滚。 */
  it('只露出一半也算看不见', () => {
    const { list, scrolledTo } = make(770, 835)
    revealSelected(list, '.tools-row.active')
    expect(scrolledTo()).toEqual({ block: 'center' })
  })

  it('一条都没选中就什么都不做', () => {
    const list = document.createElement('div')
    box(list, 100, 800)
    expect(() => revealSelected(list, '.tools-row.active')).not.toThrow()
  })

  it('容器还没挂上时不炸', () => {
    expect(() => revealSelected(undefined, '.tools-row.active')).not.toThrow()
  })
})
