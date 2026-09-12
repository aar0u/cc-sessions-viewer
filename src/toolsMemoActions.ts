// 工具管理 · 全局配置「合并重复」的纯逻辑：主 store、计划翻译、按钮该不该亮。
//
// 和 `toolsMemo.ts` 分家的理由和 Skills 那边一样，是「读 / 写」：那边回答「现在是
// 什么样」，这边回答「要把它改成什么样」。面板在 `src/views/`、弹框在 `src/modals/`，
// 两个目录在 `vitest.config.ts` 都是覆盖率排除的 —— 而这里恰恰最不该靠肉眼验：
// 一次点错动的是用户七家 agent 都在读的那几个指令文件。

import { ref } from 'vue'
import type { MemoDup, MemoMergeReport, MemoScan } from './types'
import { t } from './i18n'
import { shortenPath } from './toolsSkills'
import { countSummary, type PlanView } from './toolsPlan'

// ---------------------------------------------------------------------------
// 主 store
// ---------------------------------------------------------------------------

const MEMO_STORE_KEY = 'toolsMemoStore:v1'

function loadMemoStore(): string | null {
  try {
    return localStorage.getItem(MEMO_STORE_KEY)
  } catch {
    return null
  }
}

/**
 * 合并的去处。用户没改过就是 `null`，走 [`memoStorePath`] 的默认值。
 *
 * 和 Skills 的主 store 一样不让后端存：它只影响「往哪儿搬」，而命令本来就要把它当
 * 参数传过去；后端再存一份就有两个真相，用户在别处挪走目录之后那份记录还是错的。
 */
export const memoStore = ref<string | null>(loadMemoStore())

export function setMemoStore(path: string | null) {
  memoStore.value = path
  try {
    if (path) localStorage.setItem(MEMO_STORE_KEY, path)
    else localStorage.removeItem(MEMO_STORE_KEY)
  } catch {
    // 无痕模式之类：内存里的值照样能用，只是下次打开要重选。
  }
}

/**
 * 实际要用的主 store 目录。
 *
 * 默认 `~/.agents/memo` —— 和 Skills 的主 store（`~/.agents/skills`）同一个父目录，
 * 而那个目录本来就是「跨 agent 共享的东西放这儿」的意思。**不默认塞进某一家的 home**：
 * 放进 `~/.claude/` 的话，卸载 Claude Code 会把另外六家的全局指令一起带走。
 */
export function memoStorePath(home: string): string {
  const picked = memoStore.value?.trim()
  if (picked) return picked
  return `${home.replace(/\/+$/, '')}/.agents/memo`
}

// ---------------------------------------------------------------------------
// 哪些能合
// ---------------------------------------------------------------------------

/** 这个路径所在的那一组重复。没有就是 null。 */
export function dupOf(scan: MemoScan | null, path: string | null): MemoDup | null {
  if (!scan || !path) return null
  return scan.dups.find((d) => d.paths.includes(path)) ?? null
}

/**
 * 一组重复里除了自己以外的那几个位置。
 *
 * 详情页那句话要说「它在另外 2 个地方还各存着一份」，说的就是这些。
 */
export function dupOthers(dup: MemoDup | null, path: string | null): string[] {
  if (!dup || !path) return []
  return dup.paths.filter((p) => p !== path)
}

/**
 * 这一组还值不值得合并。
 *
 * 实体少于两份就没得合了 —— 后端也会拒，但按钮先灰掉，省得用户点一下换来一句拒绝。
 * 注意**不是**看 `paths`：三个位置里两个已经是链接时，`paths` 还是 3，但真正重复的
 * 内容只剩一份。
 */
export function mergeable(dup: MemoDup | null): boolean {
  return (dup?.bodies.length ?? 0) >= 2
}

/** 全机器还能合的那几组。健康条上「合并重复」按钮交给后端的就是这串名字。 */
export function mergeableDups(scan: MemoScan | null): MemoDup[] {
  return (scan?.dups ?? []).filter(mergeable)
}

// ---------------------------------------------------------------------------
// 计划翻译
// ---------------------------------------------------------------------------

/**
 * 把后端的合并报告翻成通用计划弹框要的形状。
 *
 * 弹框（`ToolsPlanModal`）不认识任何一种 report，各面板自己翻 —— 这样翻译逻辑落在
 * `.ts` 里能被测到，而 `.vue` 只管画。
 */
export function memoMergePlan(
  report: MemoMergeReport,
  home: string,
  names: string[],
): PlanView {
  const counts: Record<string, number> = {}
  for (const s of report.steps) counts[s.kind] = (counts[s.kind] ?? 0) + 1

  const rows = report.steps.map((s) => ({
    kind: s.kind === 'drop' || s.kind === 'unlink' ? 'remove' : s.kind,
    kindLabel: t(`tools.memo.merge.step.${s.kind}`),
    where: shortenPath(s.path, home),
    detail: s.target ? shortenPath(s.target, home) : undefined,
    // 删掉的那几份内容不是「没了」—— 同样的字节已经在主 store 里躺着。这句话
    // 不说清楚，用户看到「删除 2」就不敢点了。
    note: s.kind === 'drop' ? t('tools.memo.merge.dropNote') : undefined,
    // 「合并全部重复」一次能出二十来步，弹框按它在文件之间画分割线。
    group: s.group,
  }))

  return {
    title: t('tools.memo.merge.title', { n: String(names.length) }),
    summary: countSummary(counts, (kind, n) =>
      t(`tools.memo.merge.count.${kind}`, { n: String(n) }),
    ),
    rows,
    blocked: report.blocked,
    // 确认按钮不变红：删掉的每一份都和主 store 里那份逐字节相同，而且删之前还会
    // 再复核一次。真正会丢东西的情况在后端就被挡成 blocked 了。
    danger: false,
    applyLabel: t('tools.memo.merge.apply'),
    footnote: t('tools.memo.merge.footnote'),
  }
}

/**
 * 应用完之后给用户的一句话。
 *
 * 报告里可能**又成功又有做不了的**（一组合上了、另一组被占位文件挡住），所以不能
 * 只看有没有 blocked 就判成功或失败。
 */
export function mergeOutcome(report: MemoMergeReport): { msg: string; error: boolean } {
  const done = report.steps.filter((s) => s.done).length
  if (done === 0) {
    return {
      msg: report.blocked[0] ?? t('tools.memo.merge.nothing'),
      error: true,
    }
  }
  if (report.blocked.length > 0) {
    return {
      msg: t('tools.memo.merge.partial', {
        n: String(done),
        why: report.blocked[0],
      }),
      error: true,
    }
  }
  return { msg: t('tools.memo.merge.done', { n: String(done) }), error: false }
}
