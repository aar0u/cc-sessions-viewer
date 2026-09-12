// 确认框的通用形状。这儿只有一个纯函数，但它决定了标题下那一句长什么样。

import { describe, it, expect } from 'vitest'
import { countSummary } from '../src/toolsPlan'

describe('计数概括', () => {
  const label = (kind: string, n: number) => `${kind} ${n}`

  it('零的那几档不出现 —— 「新增 0」比不写还糟', () => {
    expect(countSummary({ add: 2, remove: 0, update: 1 }, label)).toBe('add 2 · update 1')
  })

  it('全是零时给一句空的，调用方自己决定怎么兜', () => {
    expect(countSummary({ add: 0, remove: 0 }, label)).toBe('')
  })
})
