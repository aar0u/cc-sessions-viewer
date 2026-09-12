// 并排（split）diff —— 把合并式的 hunk 摊成左右两栏的行对。
//
// 差异本身还是后端算的那一份（`memo::line_hunks`），这里只重排不重算：聊天里的
// `DiffBlock` 把同一份 hunk 按合并式渲染，全局配置的「两份文件对照」把它摊成两栏。
// 前端再写一个行差异算法必然给出第二种 hunk，那才是真正的分叉。
//
// 配对规则：一段连续的 `del` 和紧跟的一段 `add` 按下标一一对上，多出来的那几行对面
// 补空格（`pad`）。上下文行两边同时出现，行号各取各的。
import type { DiffHunk, DiffLine } from './types'

/** `pad` 是「这边本来就没有这一行」，不是空行 —— 空行是有行号的 `ctx`。 */
export type SplitKind = 'ctx' | 'add' | 'del' | 'pad'

export interface SplitCell {
  kind: SplitKind
  /** 行号。`pad` 没有。 */
  no: number | null
  text: string
}

export interface SplitRow {
  /** hunk 之间跳过的那一段，渲染成一条横跨两栏的省略行。 */
  sep: boolean
  left: SplitCell
  right: SplitCell
}

function pad(): SplitCell {
  return { kind: 'pad', no: null, text: '' }
}

function cell(ln: DiffLine, side: 'left' | 'right'): SplitCell {
  return {
    kind: ln.kind,
    no: side === 'left' ? ln.oldNo : ln.newNo,
    text: ln.text,
  }
}

/** 合并式 hunk → 并排行对。 */
export function splitRows(hunks: DiffHunk[]): SplitRow[] {
  const rows: SplitRow[] = []
  hunks.forEach((h, hi) => {
    if (hi > 0) rows.push({ sep: true, left: pad(), right: pad() })
    let dels: DiffLine[] = []
    let adds: DiffLine[] = []
    const flush = () => {
      const n = Math.max(dels.length, adds.length)
      for (let i = 0; i < n; i++) {
        rows.push({
          sep: false,
          left: dels[i] ? cell(dels[i], 'left') : pad(),
          right: adds[i] ? cell(adds[i], 'right') : pad(),
        })
      }
      dels = []
      adds = []
    }
    for (const ln of h.lines) {
      if (ln.kind === 'del') dels.push(ln)
      else if (ln.kind === 'add') adds.push(ln)
      else {
        // 上下文行封住上面那一段改动 —— 跨过它配对就把两处不相干的改动对到一起了。
        flush()
        rows.push({ sep: false, left: cell(ln, 'left'), right: cell(ln, 'right') })
      }
    }
    flush()
  })
  return rows
}
