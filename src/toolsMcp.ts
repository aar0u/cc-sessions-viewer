// 工具管理 · MCP 面板的纯逻辑。
//
// 和 toolsSkills.ts 同一个理由住在这儿：面板本体在 `src/views/`，而那个目录在
// `vitest.config.ts` 是覆盖率排除的。「这条 server 在哪几家生效」「预算条该画多满」
// 这类判断写进 .vue 就再也没人测得到。

import type {
  Agent,
  McpDefAt,
  McpEdit,
  McpEntry,
  McpScan,
  McpServerDef,
  McpServerInput,
  McpStepKind,
  McpVar,
  McpWriteReport,
  McpWriteStep,
} from './types'
import { agentLabel } from './agentMeta'
import { t } from './i18n'
import { countSummary, type PlanView } from './toolsPlan'
import { splitWords } from './shellWords'
import { isAgentOn } from './toolsPanel'
import { shortenPath } from './toolsSkills'

/**
 * 上下文预算的分母。
 *
 * 200k 是本机这几家 agent 的主力模型里最常见的一档。它只是**画条子用的刻度**，不是
 * 谁的真实上限 —— 所以面板上永远写成「约 N / 200k」，而不是「还剩多少」。
 */
export const CONTEXT_BUDGET = 200_000

/** 超过这个比例就变黄：工具定义还没开口说话就吃掉了十分之一的上下文。 */
export const BUDGET_WARN = 0.1
/** 超过这个比例变红。1/4 的上下文用来装工具说明书已经是病态了。 */
export const BUDGET_DANGER = 0.25

export type BudgetLevel = 'ok' | 'warn' | 'danger'

export function budgetLevel(tokens: number): BudgetLevel {
  const ratio = tokens / CONTEXT_BUDGET
  if (ratio >= BUDGET_DANGER) return 'danger'
  if (ratio >= BUDGET_WARN) return 'warn'
  return 'ok'
}

/** 条子画多满（0–1）。超出分母时封顶在 1，否则条子会画出容器。 */
export function budgetRatio(tokens: number): number {
  return Math.max(0, Math.min(1, tokens / CONTEXT_BUDGET))
}

/**
 * 一个 server 的状态。次序即严重程度，列表按它排在名字之前。
 *
 * - `conflict` —— 同名不同命令行，各家跑的不是一个东西
 * - `incomplete` —— 配置残缺（既没 command 也没 url），起不来
 * - `off` —— 配置里有，但没有任何一家在用
 * - `on` —— 正常
 */
export type McpState = 'conflict' | 'incomplete' | 'off' | 'on'

export const MCP_STATES: readonly McpState[] = ['conflict', 'incomplete', 'off', 'on'] as const

export function mcpState(entry: McpEntry): McpState {
  if (entry.conflict) return 'conflict'
  // 只看生效的那几份：被盖住的那份残不残缺不影响实际跑起来的东西。
  if (effectiveDefs(entry).some((d) => d.def.incomplete)) return 'incomplete'
  return entry.agents.length === 0 ? 'off' : 'on'
}

/** 实际生效的那些定义。同一家 agent 至多一条。 */
export function effectiveDefs(entry: McpEntry): McpDefAt[] {
  return entry.defs.filter((d) => d.effective)
}

/**
 * 详情页要展示的那一份定义。
 *
 * 取生效的第一份；一份都没有（全被关掉）时退回第一份配置，**不是返回空** —— 用户点
 * 进来就是想看这条 server 长什么样，给一片空白等于说「它不存在」。
 */
export function primaryDef(entry: McpEntry): McpServerDef | null {
  return (effectiveDefs(entry)[0] ?? entry.defs[0])?.def ?? null
}

/** 命令行的展示形式。远端 server 显示 URL。 */
export function commandLine(def: McpServerDef | McpServerInput): string {
  // 列表行没有「点一下看原文」这个动作，所以这里**永远**用打过码的那份。拿不到
  // 打码版本时宁可整条不显示，也不把原文漏出去。
  if (def.url) return 'urlMasked' in def ? (def.urlMasked ?? MASKED_URL) : def.url
  return [def.command ?? '', ...def.args].filter((p) => p !== '').join(' ')
}

/** 打码版本缺席时的兜底。真出现说明后端漏填了，显示成这样也比漏出原文强。 */
const MASKED_URL = '••••'

/** 详情页那一栏：默认给打过码的，用户点一下才给原文。和环境变量同一个动作。 */
export function urlDisplay(def: McpServerDef, revealed: boolean): string {
  if (!def.url) return ''
  return revealed ? def.url : (def.urlMasked ?? MASKED_URL)
}

/** 这条 URL 里有东西被打码了吗 —— 没有的话详情页就不该做成可点的。 */
export function urlHasSecret(def: McpServerDef): boolean {
  return def.url !== null && def.urlMasked !== null && def.urlMasked !== def.url
}

/**
 * 打码。**只露头尾各 2 个字符**，中间固定 4 个点。
 *
 * 不按长度成比例打点：点的个数会泄漏密钥长度，而长度本身就是有用的攻击信息。太短的
 * 值一个字符都不露 —— 4 个字符的 token 露掉头尾就只剩没露的那两个了。
 */
export function maskValue(value: string): string {
  if (value.length <= 8) return '••••'
  return `${value.slice(0, 2)}••••${value.slice(-2)}`
}

/** 一行环境变量的展示值：像凭据的默认打码，用户点开才给原文。 */
export function varDisplay(v: McpVar, revealed: boolean): string {
  return v.secret && !revealed ? maskValue(v.value) : v.value
}

// ---------------------------------------------------------------------------
// 过滤
// ---------------------------------------------------------------------------

export interface McpFilter {
  /** 只看这一类状态。null = 不按状态过滤。 */
  state: McpState | null
}

/** 点同一个角标第二下就是取消，不需要另给一个「全部」按钮。 */
export function toggleState(filter: McpFilter, state: McpState): McpFilter {
  return { state: filter.state === state ? null : state }
}

/**
 * 列表可见的 server。
 *
 * agent 过滤器按「**在这家生效**」判，不是「这家的配置文件里提到过」：用户勾上 codex
 * 是想看 codex 真正会加载的东西，而不是它某个被项目配置盖掉的历史残留。只在配置里出现
 * 过的那些仍然能通过「关着」那一档找到。
 */
/**
 * 只命中命令行时，那条命令行；否则 `null`。
 *
 * 行里只有名字和「传输 · 工具数 · token」，命令行是藏在详情里的。搜 `npx` 出来一串
 * 名字里没有 `npx` 的行，看上去像没过滤 —— 那就把命中的那条命令行顶到第二行来。
 * 和 skills 面板的 `queryPath()` 同一个理由。
 */
export function queryCommand(entry: McpEntry, query: string): string | null {
  const q = query.trim().toLowerCase()
  if (!q) return null
  if (entry.name.toLowerCase().includes(q)) return null
  const def = primaryDef(entry)
  if (!def) return null
  const line = commandLine(def)
  return line.toLowerCase().includes(q) ? line : null
}

export function visibleServers(scan: McpScan | null, query: string, filter: McpFilter): McpEntry[] {
  if (!scan) return []
  const q = query.trim().toLowerCase()
  return scan.servers.filter((entry) => {
    if (filter.state && mcpState(entry) !== filter.state) return false
    if (!agentVisible(entry)) return false
    if (q === '') return true
    const def = primaryDef(entry)
    return (
      entry.name.toLowerCase().includes(q) ||
      (def ? commandLine(def).toLowerCase().includes(q) : false)
    )
  })
}

function agentVisible(entry: McpEntry): boolean {
  // 一家 agent 都不沾的（全机器没人用）不该被 agent 过滤器筛掉 —— 它正是用户最该
  // 看见的那种：占着配置却没人读。
  const touched = [...entry.agents, ...entry.inactiveAgents]
  if (touched.length === 0) return true
  return touched.some((a) => isAgentOn(a))
}

/** 各状态各有几条。健康条上的数字。 */
export function stateCounts(scan: McpScan | null): Record<McpState, number> {
  const out: Record<McpState, number> = { conflict: 0, incomplete: 0, off: 0, on: 0 }
  for (const entry of scan?.servers ?? []) out[mcpState(entry)] += 1
  return out
}

// ---------------------------------------------------------------------------
// 覆盖关系
// ---------------------------------------------------------------------------

/** 一家 agent 对某条 server 的态度。 */
export type McpReach =
  /** 生效中。 */
  | { state: 'on'; def: McpDefAt }
  /** 配置里有，但显式关掉了。 */
  | { state: 'off'; def: McpDefAt }
  /** 这家压根没配。 */
  | { state: 'absent' }

/**
 * 这家 agent 会不会加载这条 server。
 *
 * 只看**生效的那一份**。同一家里被盖住的那些不改变结论 —— 它们解释的是「为什么改了
 * 没反应」，那是另一个问题，见 [`shadowedDefs`]。
 */
export function agentReach(entry: McpEntry, agent: Agent): McpReach {
  const def = entry.defs.find((d) => d.agent === agent && d.effective)
  if (!def) return { state: 'absent' }
  return { state: def.def.enabled ? 'on' : 'off', def }
}

/**
 * 这家 agent 里被盖住的那些定义，按优先级从高到低。
 *
 * 「同一条 server 配了两遍」本身很常见（user 级配过、项目里又配了一遍），但用户改了
 * 不生效的那份却什么都没发生时，这是唯一能解释原因的地方。
 */
export function shadowedDefs(entry: McpEntry, agent: Agent): McpDefAt[] {
  return entry.defs
    .filter((d) => d.agent === agent && !d.effective)
    .sort((a, b) => b.source.precedence - a.source.precedence)
}

/** 这条 server 被哪些文件定义过，去重后按优先级从高到低。 */
export function definingPaths(entry: McpEntry): string[] {
  const out: string[] = []
  for (const d of [...entry.defs].sort((a, b) => b.source.precedence - a.source.precedence)) {
    if (!out.includes(d.source.path)) out.push(d.source.path)
  }
  return out
}

// ---------------------------------------------------------------------------
// 写：按 agent 同步 / 增删改
// ---------------------------------------------------------------------------

/**
 * 这条 server 在这家 agent 的**可写文件**里的那一份。
 *
 * 和 [`agentReach`] 是两个问题：reach 问的是「跑不跑」（可能来自项目配置、也可能来自
 * 别家的兼容来源），这里问的是「我们动得了的那个文件里有没有它」。勾选框要按后者画 ——
 * 把一条来自项目 `.mcp.json` 的定义画成「已勾选」，用户取消勾选却什么都不会发生。
 */
export function defAtWritable(entry: McpEntry, scan: McpScan, agent: Agent): McpDefAt | null {
  const path = writePath(scan, agent)
  if (!path) return null
  return entry.defs.find((d) => d.agent === agent && d.source.path === path) ?? null
}

/** 这家 agent 的可写文件。null = 这家没法改。 */
export function writePath(scan: McpScan | null, agent: Agent): string | null {
  return scan?.agents.find((a) => a.agent === agent)?.writePath ?? null
}

/** 勾选框的三态。`locked` = 这家没有可写来源，勾不动。 */
export type SyncState = 'checked' | 'unchecked' | 'locked'

export function syncState(entry: McpEntry, scan: McpScan | null, agent: Agent): SyncState {
  if (!scan || !writePath(scan, agent)) return 'locked'
  const at = defAtWritable(entry, scan, agent)
  return at && at.def.enabled ? 'checked' : 'unchecked'
}

/**
 * 点一下勾选框要做的那件事。
 *
 * 取消勾选 = **从这家的可写文件里移除**，不是写一个 `enabled: false`：七家里只有
 * codex / opencode 的格式确认认这个键，给 claude 写一个它不读的开关，面板显示
 * 「已停用」而 server 照跑，比没有这个功能坏得多。停用是详情页上单独的按钮，
 * 后端会把不支持的那几家报进 `blocked`。
 *
 * 从「关着」变成「勾上」走 `put` 而不是 `enable`：`put` 重写整条，顺带把
 * `enabled = false` 清掉，一条路径覆盖两种格式。
 */
export function syncEdit(entry: McpEntry, scan: McpScan, agent: Agent): McpEdit | null {
  const state = syncState(entry, scan, agent)
  if (state === 'locked') return null
  if (state === 'checked') return { agent, name: entry.name, op: 'drop', def: null }
  const source = primaryDef(entry)
  if (!source) return null
  return { agent, name: entry.name, op: 'put', def: defToInput(source) }
}

/** 把一份读到的定义变成一份可写的输入。派生字段（`incomplete` / `enabled`）不带过去。 */
export function defToInput(def: McpServerDef): McpServerInput {
  return {
    transport: def.transport,
    command: def.command,
    args: [...def.args],
    url: def.url,
    env: def.env.map((v) => ({ key: v.key, value: v.value })),
    headers: def.headers.map((v) => ({ key: v.key, value: v.value })),
    cwd: def.cwd,
  }
}

/** 从**所有**可写文件里删掉它。动不了的那些（项目配置、兼容来源）由后端报进 blocked。 */
export function removeEdits(entry: McpEntry, scan: McpScan): McpEdit[] {
  return touchedAgents(entry)
    .filter((a) => defAtWritable(entry, scan, a))
    .map((agent) => ({ agent, name: entry.name, op: 'drop' as const, def: null }))
}

/** 把它在所有可写文件里的 `enabled` 位翻成 `on`。 */
export function enableEdits(entry: McpEntry, scan: McpScan, on: boolean): McpEdit[] {
  return touchedAgents(entry)
    .filter((a) => defAtWritable(entry, scan, a))
    .map((agent) => ({
      agent,
      name: entry.name,
      op: on ? ('enable' as const) : ('disable' as const),
      def: null,
    }))
}

/** 把一份定义同时写给这几家。 */
export function putEdits(name: string, def: McpServerInput, agents: Agent[]): McpEdit[] {
  return agents.map((agent) => ({ agent, name, op: 'put' as const, def }))
}

/**
 * 表单保存时的一批改动：勾上的写过去，**原来勾着、这次取消了的移除**。
 *
 * 少了后半截的话，编辑框里取消一个勾等于什么都没做 —— 用户看到的是「取消了还在」。
 * 新增（`entry` 为 null）时没有「原来」，只有写。
 */
export function formEdits(
  name: string,
  def: McpServerInput,
  picked: Agent[],
  entry: McpEntry | null,
  scan: McpScan | null,
): McpEdit[] {
  const puts = putEdits(name, def, picked)
  if (!entry || !scan) return puts
  const drops = touchedAgents(entry)
    .filter((a) => !picked.includes(a) && syncState(entry, scan, a) === 'checked')
    .map((agent) => ({ agent, name, op: 'drop' as const, def: null }))
  return [...puts, ...drops]
}

/** 沾过这条 server 的 agent，开着的和关着的都算。 */
export function touchedAgents(entry: McpEntry): Agent[] {
  return [...entry.agents, ...entry.inactiveAgents]
}

/**
 * 命令行拆成命令 + 参数。
 *
 * 分词在 `shellWords.ts`，和 Hooks 面板共用 —— 两边各抄一份的结果必然是两种拆法。
 */
export function parseCommandLine(line: string): { command: string | null; args: string[] } {
  const [command, ...args] = splitWords(line)
  return { command: command ?? null, args }
}

/** 空白的一份定义，给「添加」用。 */
export function blankInput(): McpServerInput {
  return {
    transport: 'stdio',
    command: null,
    args: [],
    url: null,
    env: [],
    headers: [],
    cwd: null,
  }
}

/** server 名字得是各家都收得下的。空格和点在 TOML 的裸键里就是两段路径。 */
export function validName(name: string): boolean {
  return /^[A-Za-z0-9_-]+$/.test(name)
}

/** 计划里各类步骤各有几步。确认框标题下那一句。 */
export function stepCounts(report: McpWriteReport): Record<McpStepKind, number> {
  const out: Record<McpStepKind, number> = {
    add: 0,
    update: 0,
    remove: 0,
    enable: 0,
    disable: 0,
  }
  for (const s of report.steps) out[s.kind] += 1
  return out
}

/** 计划里有没有「移除」这种丢配置的步骤 —— 确认按钮据此变红。 */
export function isDestructive(report: McpWriteReport): boolean {
  return report.steps.some((s) => s.kind === 'remove')
}

/**
 * 把一份 dry-run 报告翻成确认框能画的形状。
 *
 * 翻译住在这儿而不是 `.vue` 里，是因为「移除这一步有没有被画成红的」「被盖住的那条
 * 有没有把盖它的文件说出来」正是最该被测到的部分 —— 弹框本身在 `src/modals/`，
 * 覆盖率排除。
 */
export function mcpPlanView(title: string, report: McpWriteReport, home: string): PlanView {
  const counts = stepCounts(report)
  const danger = isDestructive(report)
  return {
    title,
    summary: countSummary(counts, (kind, n) => t(`tools.mcp.plan.count.${kind}`, { n: String(n) })),
    rows: report.steps.map((s) => ({
      kind: s.kind,
      kindLabel: t(`tools.mcp.plan.kind.${s.kind}`),
      where: `${agentLabel(s.agent)} · ${shortenPath(s.path, home)}`,
      detail: s.after ?? s.before ?? s.name,
      note: stepNote(s, home),
      noteWarn: s.note === 'shadowed',
    })),
    blocked: report.blocked.map(
      (b) =>
        `${agentLabel(b.agent)} · ${b.name} —— ` +
        t(`tools.mcp.block.${b.reason}`, { path: b.path ? shortenPath(b.path, home) : '' }),
    ),
    danger,
    applyLabel: t('tools.mcp.plan.apply', { n: String(report.steps.length) }),
    footnote: danger ? t('tools.mcp.plan.removeNote') : t('tools.mcp.plan.backupNote'),
  }
}

/**
 * 真跑完之后那句话。
 *
 * 三种结局得分开说，说混了就是骗人：全做完 / 什么都没做（确认期间文件被改了）/
 * **做了一半**。最后那种最要紧 —— 七家配置在七个文件里，没有哪个机制能把它们一起
 * 提交，只说一句「失败了」用户根本不知道自己现在有几份配置已经改了。
 */
export function mcpApplyOutcome(
  report: McpWriteReport,
  home: string,
): { msg: string; error: boolean } {
  if (!report.failed) {
    return { msg: t('tools.mcp.plan.applied', { n: String(report.steps.length) }), error: false }
  }
  const path = shortenPath(report.failed.path, home)
  if (report.failed.kind === 'stale') {
    return { msg: t('tools.mcp.fail.stale', { path }), error: true }
  }
  const detail = report.failed.detail ?? ''
  const done = report.steps.filter((s) => s.done).length
  return {
    msg:
      done > 0
        ? t('tools.mcp.fail.writePartial', { path, detail, n: String(done) })
        : t('tools.mcp.fail.write', { path, detail }),
    error: true,
  }
}

function stepNote(s: McpWriteStep, home: string): string | undefined {
  if (s.note === 'newFile') return t('tools.mcp.plan.newFile')
  if (s.note === 'shadowed') {
    return t('tools.mcp.plan.shadowed', { path: shortenPath(s.shadowedBy ?? '', home) })
  }
  return undefined
}
