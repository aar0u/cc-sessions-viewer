// 渲染尺寸闸门 —— 所有「超过这个大小就别渲染了」的阈值集中在这里。
//
// 为什么需要：会话里的单个块没有上限。`cat` 一个 5 MB 文件的 tool_result、一次
// 全仓 diff、一份巨大的 JSON 响应，都会原样进到渲染管线里：
//   · Shiki 对任意长度调 `codeToHtml`，5 MB 代码能生成百万级 `<span>`；
//   · JSON / diff 的行级染色同理，每行都要建 DOM；
//   · 虚拟列表按**条数**判断（80 条），80 条以内哪怕每条 5 MB 也全量挂 DOM。
// 高亮出来的 DOM 是原文的几十倍内存，而且用户在一个 5 MB 的块里根本看不出配色。
// 超限就退回纯 `<pre>`：内容一个字不少，只是没有配色，并且默认折叠 + 标注体积。
//
// 单位说明：阈值比较的是 JS 字符串的 `.length`（UTF-16 码元数），不是 UTF-8 字节。
// 这里要的是「防爆」量级而不是精确计量，中文内容会比字节数偏小一点，无所谓。

import type { Msg } from './types'

/** Shiki 语法高亮上限。超过就保留纯 `<pre>`（`data-shiki="skip"`）。 */
export const SHIKI_MAX_CHARS = 100 * 1024

/** JSON 美化 / token 上色上限。 */
export const JSON_HIGHLIGHT_MAX_CHARS = 200 * 1024

/** 文本 diff 行级染色上限。 */
export const DIFF_HIGHLIGHT_MAX_CHARS = 300 * 1024

/** 块超过这个体积就默认折叠，并在标题上标注大小。取三个高亮阈值里最小的那个 ——
 *  凡是「大到不给高亮」的块，都值得先折起来。 */
export const OVERSIZE_BLOCK_CHARS = SHIKI_MAX_CHARS

/** 消息列表启用虚拟滚动的条数阈值。 */
export const VIRTUALIZE_MIN_COUNT = 80

/** 消息列表启用虚拟滚动的总字节阈值。少量但超大的消息（20 条 × 500 KB）同样会把
 *  DOM 撑爆，只看条数会漏掉这一类。 */
export const VIRTUALIZE_MIN_CHARS = 2 * 1024 * 1024

/** `renderText` 的 markdown 结果缓存上限（按产出 HTML 的字符数）。原实现按 3000
 *  **条**封顶，一条 5 MB 的块和一条 20 字节的块等价计数，等于没有上限。 */
export const RENDER_TEXT_CACHE_MAX_CHARS = 20 * 1024 * 1024

/** Mermaid 渲染结果缓存上限：条数与总字符数任一超限即按 LRU 淘汰。 */
export const MERMAID_CACHE_MAX_ENTRIES = 50
export const MERMAID_CACHE_MAX_CHARS = 10 * 1024 * 1024

/** 一条消息的近似文本体积 —— 虚拟化阈值用。只累加真正会变成文本 DOM 的字段：
 *  块正文、工具入参、结构化 diff 的行文本。图片（`imageSrc`）不算 —— 它是单个
 *  `<img>` 节点，DOM 成本与 src 字符串长度无关（大图现在也已经是一条缓存文件
 *  路径而不是内联 base64 了，见后端 image_cache.rs）。 */
export function messageTextChars(msg: Pick<Msg, 'blocks'>): number {
  let total = 0
  for (const block of msg.blocks ?? []) {
    total += block.text?.length ?? 0
    total += block.toolInput?.length ?? 0
    for (const hunk of block.diff ?? []) {
      for (const line of hunk.lines) total += line.text.length
    }
  }
  return total
}

/** 整份消息列表的近似文本体积。列表是不可变快照（见 msgSnapshot.ts），所以调用方
 *  可以安全地按数组引用缓存这个结果。
 *
 *  `limit` 是提前收工的门槛：判「够不够大」只需要知道有没有越过某个线，越过就停，
 *  不必把一份 100 MB 的 transcript 从头数到尾。 */
export function totalTextChars(
  msgs: readonly Pick<Msg, 'blocks'>[],
  limit = Number.POSITIVE_INFINITY,
): number {
  let total = 0
  for (const msg of msgs) {
    total += messageTextChars(msg)
    if (total > limit) return total
  }
  return total
}

/** 消息列表是否该走虚拟滚动：条数超阈值**或**总正文超阈值。
 *
 *  条数判断放在前面且直接返回，所以正文扫描只会发生在 80 条以内的小列表上；
 *  再加上 `totalTextChars` 的提前收工，实时对话里每来一条消息重算一次也不会有负担。 */
export function shouldVirtualize(msgs: readonly Pick<Msg, 'blocks'>[]): boolean {
  if (msgs.length > VIRTUALIZE_MIN_COUNT) return true
  return totalTextChars(msgs, VIRTUALIZE_MIN_CHARS) > VIRTUALIZE_MIN_CHARS
}
