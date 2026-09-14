import { describe, expect, it } from 'vitest'
import { codexUsageWindows, codexWindowLabel } from '../src/codexUsage'
import type { CodexAccountUsage } from '../src/types'

describe('codexUsageWindows', () => {
  it('空快照 → 空数组', () => {
    expect(codexUsageWindows(null)).toEqual([])
    expect(codexUsageWindows(undefined)).toEqual([])
    expect(codexUsageWindows({})).toEqual([])
  })

  it('固定顺序 [短窗口, 长窗口]，百分比取整，窗口长度原样带出', () => {
    const u: CodexAccountUsage = {
      secondary: { usedPercent: 1.6, windowMinutes: 10080, resetsAt: '2026-09-14T06:09:33.000Z' },
      primary: { usedPercent: 3.4, windowMinutes: 300, resetsAt: '2026-09-07T11:09:33.000Z' },
    }
    expect(codexUsageWindows(u)).toEqual([
      { key: 'primary', minutes: 300, percent: 3, resetsAt: '2026-09-07T11:09:33.000Z' },
      { key: 'secondary', minutes: 10080, percent: 2, resetsAt: '2026-09-14T06:09:33.000Z' },
    ])
  })

  it('只有一个窗口时只回该窗口', () => {
    expect(codexUsageWindows({ secondary: { usedPercent: 50, windowMinutes: 10080 } })).toEqual([
      { key: 'secondary', minutes: 10080, percent: 50, resetsAt: undefined },
    ])
  })

  it('usedPercent 缺失按 0 处理；null 窗口跳过', () => {
    const u = {
      primary: { usedPercent: undefined as unknown as number, windowMinutes: 300 },
      secondary: null,
    }
    expect(codexUsageWindows(u as CodexAccountUsage)).toEqual([
      { key: 'primary', minutes: 300, percent: 0, resetsAt: undefined },
    ])
  })
})

describe('codexWindowLabel', () => {
  it('整天优先按天，其次按小时', () => {
    expect(codexWindowLabel(10080)).toBe('7d')
    expect(codexWindowLabel(1440)).toBe('1d')
    expect(codexWindowLabel(300)).toBe('5h')
    expect(codexWindowLabel(60)).toBe('1h')
  })

  it('不整除的时长退回分钟', () => {
    expect(codexWindowLabel(45)).toBe('45m')
    expect(codexWindowLabel(90)).toBe('90m')
  })

  it('未知时长（0 / 负数 / NaN）→ 空串，调用方只显示百分比', () => {
    expect(codexWindowLabel(0)).toBe('')
    expect(codexWindowLabel(-1)).toBe('')
    expect(codexWindowLabel(Number.NaN)).toBe('')
  })
})
