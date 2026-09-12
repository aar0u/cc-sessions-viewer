// 工具管理 · 写操作确认框的通用形状。
//
// MCP 和 Hooks 的计划长得一模一样：若干步「在哪个文件里对什么做什么」，外加一串
// 「这几条做不了，原因是」。第二个面板照着第一个抄的时候就该提出来 —— 再抄一遍
// 就是三份互相偏移的弹框。
//
// 各面板把自己的 report 翻成这个形状（翻译函数住在各自的 `.ts` 里，有测试覆盖），
// 弹框本身不认识任何一种 report。

/** 计划里的一步。 */
export interface PlanRow {
  /** 给 CSS 用的种类标记（`remove` 会被画成红色）。 */
  kind: string
  /** 种类的人话标签，调用方自己 `t()` 好。 */
  kindLabel: string
  /** 「在哪儿」：agent + 文件。 */
  where: string
  /** 「改什么」：命令行 / server 名，一行等宽字。 */
  detail?: string
  /** 这一步的补充说明。 */
  note?: string
  /** 说明是不是警告性质（画成警示色）。 */
  noteWarn?: boolean
  /**
   * 这一步归属的分段（比如「这是在处理 RTK.md」）。
   *
   * 一次动好几个对象时，二十来步平铺开来是一堵墙，看不出哪几步是一伙的。给了这个
   * 字段，弹框会在段与段之间画一条带标题的分割线；**只有一个分段时不画** —— 给一堵
   * 本来就连贯的墙加个标题只是多一行。
   */
  group?: string
}

/** 弹框要展示的一整份计划。 */
export interface PlanView {
  title: string
  /** 标题下那一句概括（「新增 2 · 移除 1」）。 */
  summary: string
  rows: PlanRow[]
  /** 做不了的那几条，每条一句完整的话。 */
  blocked: string[]
  /** 有丢配置的步骤 —— 确认按钮变红。 */
  danger: boolean
  /** 确认按钮上的字。 */
  applyLabel: string
  /** 按钮上方那一句（备份说明 / 不可逆警告）。 */
  footnote: string
}

/** 把一组计数拼成「新增 2 · 移除 1」，零的那几档不出现。 */
export function countSummary(
  counts: Record<string, number>,
  label: (kind: string, n: number) => string,
): string {
  return Object.keys(counts)
    .filter((kind) => counts[kind] > 0)
    .map((kind) => label(kind, counts[kind]))
    .join(' · ')
}
