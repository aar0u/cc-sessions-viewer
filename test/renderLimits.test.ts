import { describe, expect, it } from 'vitest'
import {
  OVERSIZE_BLOCK_CHARS,
  SHIKI_MAX_CHARS,
  VIRTUALIZE_MIN_CHARS,
  VIRTUALIZE_MIN_COUNT,
  messageTextChars,
  shouldVirtualize,
  totalTextChars,
} from '../src/renderLimits'
import type { Msg } from '../src/types'

function textMsg(text: string): Msg {
  return { role: 'assistant', sidechain: false, blocks: [{ kind: 'text', text, isError: false }] }
}

describe('messageTextChars', () => {
  it('adds up block text, tool input and diff line text', () => {
    const msg: Msg = {
      role: 'assistant',
      sidechain: false,
      blocks: [
        { kind: 'text', text: 'abcde', isError: false },
        { kind: 'tool_use', toolInput: '1234', isError: false },
        {
          kind: 'tool_result',
          isError: false,
          diff: [
            {
              oldStart: 1,
              oldLines: 1,
              newStart: 1,
              newLines: 1,
              lines: [
                { kind: 'add', oldNo: null, newNo: 1, text: 'xyz' },
                { kind: 'del', oldNo: 1, newNo: null, text: 'w' },
              ],
            },
          ],
        },
      ],
    }
    expect(messageTextChars(msg)).toBe(5 + 4 + 3 + 1)
  })

  // 图片是单个 <img>，DOM 成本与 base64 串长无关，不该把虚拟化阈值顶穿。
  it('ignores inline image payloads', () => {
    const msg: Msg = {
      role: 'user',
      sidechain: false,
      blocks: [{ kind: 'image', imageSrc: 'data:image/png;base64,' + 'A'.repeat(5000), isError: false }],
    }
    expect(messageTextChars(msg)).toBe(0)
  })
})

describe('totalTextChars', () => {
  it('sums every message', () => {
    expect(totalTextChars([textMsg('abc'), textMsg('de')])).toBe(5)
  })

  // 提前收工：判「够不够大」不需要把一份 100 MB 的 transcript 数到底。
  it('stops counting once it passes the limit', () => {
    const msgs = [textMsg('a'.repeat(10)), textMsg('b'.repeat(10)), textMsg('c'.repeat(10))]
    expect(totalTextChars(msgs, 5)).toBe(10)
  })
})

describe('shouldVirtualize', () => {
  it('is off for a short, small list', () => {
    expect(shouldVirtualize([textMsg('hi'), textMsg('there')])).toBe(false)
  })

  it('turns on past the message count threshold', () => {
    const msgs = Array.from({ length: VIRTUALIZE_MIN_COUNT + 1 }, () => textMsg('x'))
    expect(shouldVirtualize(msgs)).toBe(true)
  })

  // 少量但超大的消息（20 条 × 500 KB）条数远没到线，DOM 却一样会被撑爆。
  it('turns on for few but enormous messages', () => {
    const each = Math.ceil(VIRTUALIZE_MIN_CHARS / 10) + 1
    const msgs = Array.from({ length: 11 }, () => textMsg('x'.repeat(each)))
    expect(msgs.length).toBeLessThan(VIRTUALIZE_MIN_COUNT)
    expect(shouldVirtualize(msgs)).toBe(true)
  })
})

describe('thresholds', () => {
  it('folds anything too big to highlight', () => {
    expect(OVERSIZE_BLOCK_CHARS).toBeLessThanOrEqual(SHIKI_MAX_CHARS)
  })
})
