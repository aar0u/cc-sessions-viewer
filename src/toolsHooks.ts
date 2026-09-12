// 工具管理 · Hooks 面板的纯逻辑。
//
// 和 toolsMcp.ts 同一个理由住在这儿：面板本体在 `src/views/`、弹框在 `src/modals/`，
// 两个目录在 `vitest.config.ts` 都是覆盖率排除的。「这条是不是本 app 装的、能不能删」
// 「删它要动哪几个文件」这类判断写进 .vue 就再也没人测得到 —— 而第一条判断错了的
// 后果是用户删掉回合信号、GUI 聊天从此收不到回合结束。
//
// 和 MCP 最大的不同：**hook 是叠加语义**。同一条命令挂在 5 家 agent × 6 个事件上是
// 常态，所以后端按命令指纹归并成一行，这儿的函数几乎都在回答「这一行摊开是哪些落点」。

import type {
  Agent,
  HookAt,
  HookEdit,
  HookEntry,
  HookScan,
  HookWriteReport,
  HookWriteStep,
} from './types'
import { agentLabel } from './agentMeta'
import { splitWords } from './shellWords'
import { t } from './i18n'
import { countSummary, type PlanView } from './toolsPlan'
import { isAgentOn } from './toolsPanel'
import { shortenPath } from './toolsSkills'

/**
 * 列表行的状态。
 *
 * - `managed` —— 本 app 自己装的回合信号，受保护
 * - `off` —— 落点全被显式关掉了（目前只有 codex 的 TOML 有这个开关）
 * - `on` —— 会跑
 */
export const HOOK_STATES = ['on', 'off', 'managed'] as const
export type HookState = (typeof HOOK_STATES)[number]

export function hookState(entry: HookEntry): HookState {
  if (entry.managed) return 'managed'
  return entry.enabled ? 'on' : 'off'
}

export interface HookFilter {
  state: HookState | null
  /** 只看挂在这个事件上的。健康条右边那排事件角标点出来的。 */
  event: string | null
}

export function blankFilter(): HookFilter {
  return { state: null, event: null }
}

/** 点一下角标：选中就取消，没选中就换成它。 */
export function toggleState(filter: HookFilter, state: HookState): HookFilter {
  return { ...filter, state: filter.state === state ? null : state }
}

export function toggleEvent(filter: HookFilter, event: string): HookFilter {
  return { ...filter, event: filter.event === event ? null : event }
}

/** 各状态各几行。健康条上那排角标的数字。 */
export function stateCounts(hooks: HookEntry[]): Record<HookState, number> {
  const out: Record<HookState, number> = { on: 0, off: 0, managed: 0 }
  for (const h of hooks) out[hookState(h)] += 1
  return out
}

/**
 * 过滤后的列表。
 *
 * agent 过滤器只看**落点**：一条命令只要有一个落点在被勾上的 agent 里就留着 ——
 * 按「全部落点都在」过滤会让回合信号（横跨五家）在只勾一家时消失。
 */
export function visibleHooks(scan: HookScan | null, query: string, filter: HookFilter) {
  if (!scan) return []
  const q = query.trim().toLowerCase()
  return scan.hooks.filter((h) => {
    if (!h.agents.some(isAgentOn)) return false
    if (filter.state && hookState(h) !== filter.state) return false
    if (filter.event && !h.events.includes(filter.event)) return false
    if (!q) return true
    return (
      h.command.toLowerCase().includes(q) ||
      h.events.some((e) => e.toLowerCase().includes(q)) ||
      h.agents.some((a) => a.toLowerCase().includes(q))
    )
  })
}

/**
 * 永远不是「这条 hook 在干嘛」的答案的词：shell 的测试、控制和内建。
 *
 * 本机实测里有五条 hook 长成 `[ -n "$X" ] && … && cmux hooks codex stop || echo '{}'`。
 * 直接取第一个词的结果是列表里并排五行 `[`，谁也认不出谁。
 */
const SHELL_NOISE = new Set([
  '[', '[[', ']', ']]', 'test', 'command', 'env', 'exec', 'then', 'else', 'elif',
  'fi', 'if', 'do', 'done', 'true', 'false', 'echo', ':',
])

/** 这些是「拿什么去跑」，不是「跑的是什么」—— 真正的答案在它们的参数里。 */
const INTERPRETERS = new Set([
  'node', 'bun', 'deno', 'python', 'python3', 'ruby', 'perl', 'php',
  'bash', 'sh', 'zsh', 'osascript', 'npx', 'uvx', 'pnpx',
])

const SCRIPT_EXT = /\.(sh|bash|zsh|js|cjs|mjs|ts|mts|cts|py|rb|pl|php)$/i
/** 子命令长这样：全小写的一个词，不带路径也不带扩展名（`hooks` / `codex` / `stop`）。 */
const SUBCOMMAND = /^[a-z][a-z0-9-]*$/

/**
 * 标志、变量、重定向、数字 —— 一律不是答案。
 *
 * 例外是 `$CLAUDE_PROJECT_DIR/scripts/x.sh` 这种**以变量开头的路径**：整条丢掉的话
 * 标题只剩下 `bash`。带 `/` 的就当路径留着，光秃秃一个 `$VAR` 才是噪声。
 */
function isNoise(word: string): boolean {
  if (word === '' || SHELL_NOISE.has(word)) return true
  if (word.startsWith('$')) return !word.includes('/')
  return /^[-&|;><!(){}#\d]/.test(word)
}

/**
 * 列表行的标题。
 *
 * 整条命令常常一屏长，列表里放不下；但**不编名字** —— 只从命令里挑出最能说明「跑的是
 * 什么」的那一段：优先脚本文件名（`node …/ask-bridge.js` 里有意思的是 ask-bridge.js，
 * 不是 node），没有脚本就取第一个像程序名的词，再带上它后面的子命令
 * （`cmux hooks codex stop`）—— 不带的话同一个程序的五条 hook 会并排五行一模一样。
 *
 * 挑不出来就原样返回。详情里给的永远是完整命令。
 */
export function commandHead(command: string): string {
  // `command -v cmux >/dev/null && cmux hooks …` 是「装没装 X」最常见的写法，去掉
  // 噪声之后 X 会连着出现两次。不并起来的话标题就是 `cmux cmux hooks codex`。
  const words = splitWords(command)
    .filter((w) => !isNoise(w))
    .filter((w, i, all) => w !== all[i - 1])
  const script = words.find((w) => SCRIPT_EXT.test(w))
  if (script) return basename(script)
  const at = words.findIndex((w) => !INTERPRETERS.has(w))
  if (at < 0) return words[0] ?? command.trim()
  // 程序名后面连着的子命令一起带上，到第一个不像子命令的词为止。
  const tail: string[] = []
  for (const w of words.slice(at + 1, at + 4)) {
    if (!SUBCOMMAND.test(w)) break
    tail.push(w)
  }
  return [basename(words[at]), ...tail].join(' ')
}

function basename(path: string): string {
  return path.split(/[/\\]/).pop() || path
}

/** 这一行在这家 agent 上挂了哪些事件，去重后按出现顺序。 */
export function agentEvents(entry: HookEntry, agent: Agent): string[] {
  const out: string[] = []
  for (const at of entry.hooks) {
    if (at.agent === agent && !out.includes(at.def.event)) out.push(at.def.event)
  }
  return out
}

/** 这一行的全部落点，按 agent 分组。详情里那张表照它渲染。 */
export function byAgent(entry: HookEntry): { agent: Agent; hooks: HookAt[] }[] {
  const out: { agent: Agent; hooks: HookAt[] }[] = []
  for (const at of entry.hooks) {
    const row = out.find((r) => r.agent === at.agent)
    if (row) row.hooks.push(at)
    else out.push({ agent: at.agent, hooks: [at] })
  }
  return out
}

/** 这家 agent 的可写文件。没有就是「我们写不进去」。 */
export function writePath(scan: HookScan | null, agent: Agent): string | null {
  return scan?.agents.find((a) => a.agent === agent)?.writePath ?? null
}

/** 这家 agent 认不认这个事件。「添加」表单靠它禁用勾选框。 */
export function agentSupports(scan: HookScan | null, agent: Agent, event: string): boolean {
  return scan?.agents.find((a) => a.agent === agent)?.events.includes(event) ?? false
}

/** 这家 agent 有没有 hook 这个机制。没有和「有但我们写不进去」是两回事。 */
export function agentSupported(scan: HookScan | null, agent: Agent): boolean {
  return scan?.agents.find((a) => a.agent === agent)?.supported ?? false
}

/** 我们能往这家写吗 —— 支持 hook、装了、且有可写文件。 */
export function canWrite(scan: HookScan | null, agent: Agent): boolean {
  const info = scan?.agents.find((a) => a.agent === agent)
  return Boolean(info?.supported && info.writePath)
}

/**
 * 删这一行要下的改动：**只动我们能写的那个文件里的落点**。
 *
 * 项目配置和只读来源里的同名落点下不了手，后端会报成 `notInWritableSource`；这里提前
 * 滤掉，免得确认框里列一堆注定失败的条目。受保护的那条一条都不下 —— 后端还会再拦
 * 一次，两道都要在。
 */
export function removeEdits(entry: HookEntry): HookEdit[] {
  if (entry.managed) return []
  return entry.hooks
    .filter((at) => at.source.writable)
    .map((at) => ({
      agent: at.agent,
      op: 'remove' as const,
      event: at.def.event,
      matcher: at.def.matcher,
      command: at.def.command,
      timeout: null,
    }))
}

/**
 * 这一行删得动吗。
 *
 * 只在项目配置 / 只读来源里的那些一条改动都下不出来 —— 不提前判一下的话，按钮点下去
 * 是**彻底的没反应**（`propose` 收到空数组就直接返回，连个说法都没有）。
 */
export function canRemove(entry: HookEntry): boolean {
  return !entry.managed && entry.hooks.some((at) => at.source.writable)
}

/** 这一行落在哪几个文件里，去重。「为什么删不动」那句话要把它们说出来。 */
export function sourcePaths(entry: HookEntry): string[] {
  return [...new Set(entry.hooks.map((at) => at.source.path))]
}

/** 从这一家的可写文件里摘掉这一行。详情表里每行那个「移除」。 */
export function removeAgentEdits(entry: HookEntry, agent: Agent): HookEdit[] {
  return removeEdits(entry).filter((e) => e.agent === agent)
}

/** 「添加」表单提交时的一批改动：同一条命令写给勾上的每一家、每一个事件。 */
export function addEdits(
  command: string,
  events: string[],
  matcher: string | null,
  timeout: number | null,
  agents: Agent[],
): HookEdit[] {
  const out: HookEdit[] = []
  for (const agent of agents) {
    for (const event of events) {
      out.push({ agent, op: 'add', event, matcher, command, timeout })
    }
  }
  return out
}

/**
 * 这一行能不能试跑。
 *
 * 只有 `command` 型的能：`prompt` / `url` 型的那串字是喂给模型、或者拿去发请求的，
 * 过一遍 shell 既跑不出想要的结果，还可能把里面的 `|` `>` 当成真的重定向。
 */
export function canTest(entry: HookEntry): boolean {
  return entry.hooks.length > 0 && entry.hooks.every((at) => at.def.kind === 'command')
}

/** 这一行里出现过的非 `command` 型别，给「为什么不给试跑」那句话用。 */
export function hookKinds(entry: HookEntry): string[] {
  return [...new Set(entry.hooks.map((at) => at.def.kind))].filter((k) => k !== 'command')
}

/** 命令得有内容。空命令写进去就是每个回合跑一次空壳。 */
export function validCommand(command: string): boolean {
  return command.trim().length > 0
}

/** 试跑用哪个事件：优先这一行自己挂着的第一个，没有就退回传进来的默认。 */
export function testEvent(entry: HookEntry, fallback: string): string {
  return entry.events[0] ?? fallback
}

/** 各类步骤各有几步。 */
export function stepCounts(report: HookWriteReport): Record<'add' | 'remove', number> {
  const out = { add: 0, remove: 0 }
  for (const s of report.steps) out[s.kind] += 1
  return out
}

/** 有移除就算破坏性。 */
export function isDestructive(report: HookWriteReport): boolean {
  return report.steps.some((s) => s.kind === 'remove')
}

/** 读不出来的配置文件。静悄悄少几条是这个面板最坏的失败方式。 */
export function sourceErrors(scan: HookScan | null) {
  if (!scan) return []
  return scan.agents.flatMap((a) =>
    a.sources
      .filter((s) => s.error)
      .map((s) => ({ agent: a.agent, path: s.path, error: s.error as string })),
  )
}

/**
 * 把一份 dry-run 报告翻成确认框能画的形状。
 *
 * 和 MCP 那个是两份而不是一份：步骤的字段不一样（这边有 event / matcher，没有命令行
 * 前后对比），原因枚举也不一样。共用的是**画法**（`ToolsPlanModal`），不是翻译。
 */
export function hookPlanView(title: string, report: HookWriteReport, home: string): PlanView {
  const danger = isDestructive(report)
  return {
    title,
    summary: countSummary(stepCounts(report), (kind, n) =>
      t(`tools.hooks.plan.count.${kind}`, { n: String(n) }),
    ),
    rows: report.steps.map((s) => ({
      kind: s.kind,
      kindLabel: t(`tools.hooks.plan.kind.${s.kind}`),
      where: `${agentLabel(s.agent)} · ${shortenPath(s.path, home)}`,
      detail: eventLabel(s),
      note: s.newFile ? t('tools.hooks.plan.newFile') : undefined,
      noteWarn: false,
    })),
    blocked: report.blocked.map(
      (b) =>
        `${agentLabel(b.agent)} · ${b.event} —— ` +
        t(`tools.hooks.block.${b.reason}`, { path: b.path ? shortenPath(b.path, home) : '' }),
    ),
    danger,
    applyLabel: t('tools.hooks.plan.apply', { n: String(report.steps.length) }),
    footnote: danger ? t('tools.hooks.plan.removeNote') : t('tools.hooks.plan.backupNote'),
  }
}

/** 计划里那一行的「改什么」：事件，带上匹配器 —— 同一个事件配两个匹配器是两条。 */
function eventLabel(s: HookWriteStep): string {
  return s.matcher ? `${s.event} · ${s.matcher}` : s.event
}
