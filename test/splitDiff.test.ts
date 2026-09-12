import { describe, expect, it } from 'vitest'
import { splitRows } from '../src/splitDiff'
import type { DiffHunk, DiffLine } from '../src/types'

function ctx(oldNo: number, newNo: number, text: string): DiffLine {
  return { kind: 'ctx', oldNo, newNo, text }
}
function del(oldNo: number, text: string): DiffLine {
  return { kind: 'del', oldNo, newNo: null, text }
}
function add(newNo: number, text: string): DiffLine {
  return { kind: 'add', oldNo: null, newNo, text }
}
function hunk(lines: DiffLine[]): DiffHunk {
  return { oldStart: lines[0]?.oldNo ?? 0, newStart: lines[0]?.newNo ?? 0, lines }
}

describe('splitRows', () => {
  it('上下文行两边同时出现，行号各取各的', () => {
    const rows = splitRows([hunk([ctx(3, 5, 'same')])])
    expect(rows).toHaveLength(1)
    expect(rows[0].left).toEqual({ kind: 'ctx', no: 3, text: 'same' })
    expect(rows[0].right).toEqual({ kind: 'ctx', no: 5, text: 'same' })
  })

  it('一删一增配成一行', () => {
    const rows = splitRows([hunk([del(1, 'old'), add(1, 'new')])])
    expect(rows).toHaveLength(1)
    expect(rows[0].left).toEqual({ kind: 'del', no: 1, text: 'old' })
    expect(rows[0].right).toEqual({ kind: 'add', no: 1, text: 'new' })
  })

  it('增的比删的多，多出来的那几行左边补 pad', () => {
    const rows = splitRows([hunk([del(1, 'a'), add(1, 'A'), add(2, 'B'), add(3, 'C')])])
    expect(rows.map((r) => [r.left.kind, r.right.kind])).toEqual([
      ['del', 'add'],
      ['pad', 'add'],
      ['pad', 'add'],
    ])
    expect(rows[1].left).toEqual({ kind: 'pad', no: null, text: '' })
    expect(rows[2].right.text).toBe('C')
  })

  it('删的比增的多，右边补 pad', () => {
    const rows = splitRows([hunk([del(1, 'a'), del(2, 'b'), add(1, 'A')])])
    expect(rows.map((r) => [r.left.kind, r.right.kind])).toEqual([
      ['del', 'add'],
      ['del', 'pad'],
    ])
    expect(rows[1].right.no).toBeNull()
  })

  it('上下文行封住上面那一段，不跨过它配对', () => {
    const rows = splitRows([
      hunk([del(1, 'a'), ctx(2, 1, 'keep'), add(2, 'B')]),
    ])
    expect(rows.map((r) => [r.left.kind, r.right.kind])).toEqual([
      ['del', 'pad'],
      ['ctx', 'ctx'],
      ['pad', 'add'],
    ])
  })

  it('hunk 之间插一条横跨两栏的省略行', () => {
    const rows = splitRows([hunk([ctx(1, 1, 'a')]), hunk([ctx(9, 9, 'b')])])
    expect(rows.map((r) => r.sep)).toEqual([false, true, false])
  })

  it('没有 hunk 就没有行', () => {
    expect(splitRows([])).toEqual([])
  })

  it('每个 pad 是独立对象，不共享', () => {
    const rows = splitRows([hunk([del(1, 'a'), del(2, 'b')])])
    expect(rows[0].right).not.toBe(rows[1].right)
  })
})
