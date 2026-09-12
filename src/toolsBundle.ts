// 配置集（方案 阶段 10）的纯逻辑：读包、校验、把包里的条目翻成写请求。
//
// **导入不走一条新的后端命令。** 包里的 MCP / hook / 全局指令分别翻成
// `McpEdit` / `HookEdit` / 一次 `toolsWriteMemo`，再走那三条已经有校验、dry-run、
// 备份和回读的路。在后端再写一个 `tools_import_bundle` 等于把那几道闸重造一遍 ——
// 而重造的那份一定会先漏掉其中一道。
//
// 翻译住在这儿而不是 `.vue` 里，是因为 `src/views/` 和 `src/modals/` 在
// `vitest.config.ts` 里是覆盖率排除的：「一个包该往哪几个文件写」是这一阶段
// 唯一会造成破坏的判断，它必须测得到。

import type {
  Agent,
  Bundle,
  BundleHook,
  BundleInclude,
  BundleMcp,
  BundleMemo,
  BundleSkill,
  HookEdit,
  McpEdit,
  McpScan,
  McpTransport,
  MemoScan,
  McpWriteReport,
  HookWriteReport,
} from './types'
import { ALL_AGENTS } from './settings'
import { t } from './i18n'
import { countSummary, type PlanView } from './toolsPlan'
import { mcpPlanView } from './toolsMcp'
import { hookPlanView } from './toolsHooks'
import { shortenPath } from './toolsSkills'

/** 包的类型标记。和后端 `bundle::BUNDLE_KIND` 必须逐字一致。 */
export const BUNDLE_KIND = 'cc-sessions-viewer/tool-bundle'
export const BUNDLE_VERSION = 1

/** 被抹掉的值长什么样。和后端 `bundle::REDACTED` 必须一致。 */
export const BUNDLE_REDACTED = '<redacted>'

export type BundleCategory = 'mcp' | 'hooks' | 'memo' | 'skills'

/** 次序就是界面上的显示次序。 */
export const BUNDLE_CATEGORIES: BundleCategory[] = ['mcp', 'hooks', 'memo', 'skills']

export const ALL_INCLUDED: BundleInclude = { mcp: true, hooks: true, memo: true, skills: true }

// ---------------------------------------------------------------------------
// 读包
// ---------------------------------------------------------------------------

/**
 * 读不进来的原因。
 *
 * - `notJson` —— 根本不是 JSON，或者顶层不是对象
 * - `notBundle` —— 是 JSON，但没有我们的类型标记
 * - `tooNew` —— 标记对，版本比本机新。**拒绝**，不是"尽力而为地读" —— 新版本里
 *   多出来的字段可能恰恰是限制写入范围的那个，按老规则读等于无视它
 */
export type BundleProblem = 'notJson' | 'notBundle' | 'tooNew'

export interface BundleRead {
  bundle: Bundle | null
  problem: BundleProblem | null
  /** `tooNew` 时告诉用户对面是几版。 */
  version: number | null
}

type Dict = Record<string, unknown>

function isDict(v: unknown): v is Dict {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

function str(v: unknown): string | null {
  return typeof v === 'string' ? v : null
}

function strList(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
}

function num(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

const TRANSPORTS: McpTransport[] = ['stdio', 'http', 'sse', 'ws', 'unknown']

function transport(v: unknown): McpTransport {
  const s = str(v)
  return TRANSPORTS.includes(s as McpTransport) ? (s as McpTransport) : 'unknown'
}

/**
 * 包里的 agent 名字是**别人机器上**写下的，不能直接当 `Agent` 用。
 *
 * `agentLabel()` 是 `AGENT_META[agent].shortLabel` —— 喂一个本机不认识的角色
 * （别人机器上装了 cursor）进去就是在 `undefined` 上取属性，整个弹框白屏。
 * 包里的 `agent` / `parent` 一律先过这一道。
 */
export function isAgent(v: unknown): v is Agent {
  return typeof v === 'string' && (ALL_AGENTS as string[]).includes(v)
}

function readMcp(v: unknown): BundleMcp | null {
  if (!isDict(v)) return null
  const name = str(v.name)
  if (!name) return null
  return {
    name,
    transport: transport(v.transport),
    command: str(v.command),
    args: strList(v.args),
    envKeys: strList(v.envKeys),
    headerKeys: strList(v.headerKeys),
    url: str(v.url),
    cwd: str(v.cwd),
    agents: strList(v.agents),
  }
}

function readHook(v: unknown): BundleHook | null {
  if (!isDict(v)) return null
  const command = str(v.command)
  const events = strList(v.events)
  // 没有命令或者没有事件的那条装不上：`HookEdit` 两个字段都是必填。
  if (!command || events.length === 0) return null
  return {
    command,
    events,
    matcher: str(v.matcher),
    timeout: num(v.timeout),
    agents: strList(v.agents),
  }
}

function readMemo(v: unknown): BundleMemo | null {
  if (!isDict(v)) return null
  const name = str(v.name)
  const text = str(v.text)
  if (!name || text === null) return null
  return { agent: str(v.agent), parent: str(v.parent), name, text }
}

function readSkill(v: unknown): BundleSkill | null {
  if (!isDict(v)) return null
  const name = str(v.name)
  if (!name) return null
  return { name, remote: str(v.remote), agents: strList(v.agents) }
}

function pick<T>(v: unknown, one: (x: unknown) => T | null): T[] {
  return Array.isArray(v) ? v.map(one).filter((x): x is T => x !== null) : []
}

/**
 * 把一段文本读成包。
 *
 * 每个字段都重新核一遍形状，认不出来的条目直接丢掉 —— 这个文件是**别人发过来的**，
 * `as Bundle` 一把梭的结果是一个 `undefined` 一路漂到写路径上。
 */
export function readBundle(text: string): BundleRead {
  let raw: unknown
  try {
    raw = JSON.parse(text)
  } catch {
    return { bundle: null, problem: 'notJson', version: null }
  }
  if (!isDict(raw)) return { bundle: null, problem: 'notJson', version: null }
  if (raw.kind !== BUNDLE_KIND) return { bundle: null, problem: 'notBundle', version: null }

  const version = num(raw.version) ?? 0
  if (version > BUNDLE_VERSION) return { bundle: null, problem: 'tooNew', version }

  return {
    bundle: {
      kind: BUNDLE_KIND,
      version,
      createdAt: num(raw.createdAt) ?? 0,
      app: str(raw.app) ?? '',
      mcp: pick(raw.mcp, readMcp),
      hooks: pick(raw.hooks, readHook),
      memo: pick(raw.memo, readMemo),
      skills: pick(raw.skills, readSkill),
      redacted: strList(raw.redacted),
    },
    problem: null,
    version,
  }
}

export function bundleCounts(b: Bundle): Record<BundleCategory, number> {
  return { mcp: b.mcp.length, hooks: b.hooks.length, memo: b.memo.length, skills: b.skills.length }
}

export function bundleIsEmpty(b: Bundle): boolean {
  return BUNDLE_CATEGORIES.every((c) => bundleCounts(b)[c] === 0)
}

// ---------------------------------------------------------------------------
// 勾选
// ---------------------------------------------------------------------------

/**
 * 条目的标识。
 *
 * 用下标，不用名字：hook 没有名字（同一条命令可以挂在好几个事件上），而全局指令
 * 的 `name` 会重名（本机两份 `RTK.md` 就是）。下标只在"当前这个包"里有意义，
 * 而勾选状态的寿命也只有这一次导入。
 */
export function bundleKey(kind: BundleCategory, index: number): string {
  return `${kind}:${index}`
}

// ---------------------------------------------------------------------------
// 翻译：MCP
// ---------------------------------------------------------------------------

/**
 * 一条 MCP 翻成每个目标 agent 一条 `put`。
 *
 * **env / headers 一个都不写。** 包里本来就只有键名（后端导出时把值全抹了），
 * 那就只有两个选择：写成空串，或者不写。写空串会**盖掉**用户 shell 里本来继承
 * 得到的那份 —— server 拿到一个空 token，报的是认证失败，而配置文件上明明有这个
 * 键，这种坏法查起来最久。所以不写，改成在预览里把缺的键逐条列出来
 * （`missingSecrets`），让人自己补。
 */
export function bundleMcpEdits(
  b: Bundle,
  picked: ReadonlySet<string>,
  agents: Agent[],
): McpEdit[] {
  const out: McpEdit[] = []
  b.mcp.forEach((s, i) => {
    if (!picked.has(bundleKey('mcp', i))) return
    for (const agent of agents) {
      out.push({
        agent,
        name: s.name,
        op: 'put',
        def: {
          transport: s.transport,
          command: s.command,
          args: [...s.args],
          url: s.url,
          env: [],
          headers: [],
          cwd: s.cwd,
        },
      })
    }
  })
  return out
}

/** 装完还要自己补的那些值，`server.env.KEY` 一条一行。 */
export function missingSecrets(b: Bundle, picked: ReadonlySet<string>): string[] {
  const out: string[] = []
  b.mcp.forEach((s, i) => {
    if (!picked.has(bundleKey('mcp', i))) return
    for (const k of s.envKeys) out.push(`${s.name}.env.${k}`)
    for (const k of s.headerKeys) out.push(`${s.name}.headers.${k}`)
  })
  return out
}

/**
 * 这次导入会**清掉**本机哪些已经配着的值。
 *
 * `env` / `headers` 归写入方管，`McpOp.put` 是**整片覆盖**这两个键而不是逐键合并
 * （见 `mcp_write.rs` 的 `JSON_OWNED_KEYS`）：同名 server 一旦被覆盖，它原有的
 * 变量会整片消失 —— 连包里压根没提到的那几个键也一起没。而计划框里的 `before` /
 * `after` 只画命令行（`mcp_write.rs::command_line`），命令行没变的时候那一行看上去
 * 和没动一样。不在这儿点出来，用户是在 server 起不来之后才发现的。
 *
 * 只看**可写的那一份**：导入只往那儿写，被它盖住的别的定义不受影响。空值不算 ——
 * 键在值不在，本来就没东西可丢。
 */
export function clearedValues(
  b: Bundle,
  picked: ReadonlySet<string>,
  agents: Agent[],
  local: McpScan | null,
): string[] {
  if (!local) return []
  const out: string[] = []
  b.mcp.forEach((s, i) => {
    if (!picked.has(bundleKey('mcp', i))) return
    const entry = local.servers.find((e) => e.name === s.name)
    if (!entry) return
    for (const agent of agents) {
      const at = entry.defs.find((d) => d.agent === agent && d.source.writable)
      if (!at) continue
      for (const v of at.def.env) if (v.value !== '') out.push(`${agent}/${s.name}.env.${v.key}`)
      for (const v of at.def.headers) {
        if (v.value !== '') out.push(`${agent}/${s.name}.headers.${v.key}`)
      }
    }
  })
  return out
}

/** 被抹掉过值的 args（`--api-key=<redacted>`）。这些**照原样写进去**，所以要点出来。 */
export function redactedArgs(b: Bundle, picked: ReadonlySet<string>): string[] {
  const out: string[] = []
  b.mcp.forEach((s, i) => {
    if (!picked.has(bundleKey('mcp', i))) return
    for (const a of s.args) if (a.includes(BUNDLE_REDACTED)) out.push(`${s.name}: ${a}`)
  })
  return out
}

// ---------------------------------------------------------------------------
// 翻译：hooks
// ---------------------------------------------------------------------------

/** 一条 hook 挂在 N 个事件上就是 N 条 `HookEdit` —— 后端那一层本来就是按事件记的。 */
export function bundleHookEdits(
  b: Bundle,
  picked: ReadonlySet<string>,
  agents: Agent[],
): HookEdit[] {
  const out: HookEdit[] = []
  b.hooks.forEach((h, i) => {
    if (!picked.has(bundleKey('hooks', i))) return
    for (const agent of agents) {
      for (const event of h.events) {
        out.push({ agent, op: 'add', event, matcher: h.matcher, command: h.command, timeout: h.timeout })
      }
    }
  })
  return out
}

// ---------------------------------------------------------------------------
// 翻译：全局指令
// ---------------------------------------------------------------------------

export type MemoTargetKind = 'create' | 'overwrite' | 'blocked'

/** 放不下的原因。 */
export type MemoBlock =
  /** 这家在本机没有 home 级约定（agy），或者本机根本不认得这个角色。 */
  | 'unsupported'
  /** 片段没记 `parent` —— 不知道该跟谁走。 */
  | 'noParent'
  /** 片段记了 `parent`，但那家在本机落不了地（没装 / 没有 home 级约定 / 不认得）。 */
  | 'noHome'
  /** 名字里有路径分隔符或 `..`。包是别人发来的，这种名字要么是手改坏了要么是恶意的。 */
  | 'badName'

export interface MemoTarget {
  key: string
  name: string
  /** 落在本机的哪个路径。`blocked` 时为 null。 */
  path: string | null
  /** 这是哪家的（片段则是被哪家 `@` 进来的）。 */
  agent: string | null
  fragment: boolean
  text: string
  kind: MemoTargetKind
  reason: MemoBlock | null
  /** `overwrite` 时磁盘上那份的字节数；其余为 0。 */
  bytes: number
}

function dirOf(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return cut < 0 ? '' : path.slice(0, cut)
}

/** 只准是个纯文件名。这个字符串会被拼进写入路径里。 */
function safeName(name: string): boolean {
  return name !== '' && !/[/\\]/.test(name) && name !== '.' && name !== '..'
}

/**
 * 包里的每一份全局指令在**本机**落到哪儿。
 *
 * 按**角色**落，不按包里的路径：导出那台机器的用户名和 home 都不一样，原样拿绝对
 * 路径去写，写出来的是一个谁也不读的文件。
 *
 * 片段（`@` 进来的那些）跟着它的 `parent` 走，落在那家约定文件的**同一个目录**里
 * —— `~/.claude/CLAUDE.md` 里那行 `@RTK.md` 是相对路径，只有落在旁边才接得上。
 */
export function bundleMemoTargets(b: Bundle, scan: MemoScan): MemoTarget[] {
  const home = (role: string | null): string | null => {
    if (!role) return null
    const info = scan.agents.find((a) => a.agent === role)
    return info && info.supported ? info.path : null
  }
  const onDisk = new Map(scan.files.map((f) => [f.path, f]))

  return b.memo.map((m, i): MemoTarget => {
    const fragment = m.agent === null
    const role = m.agent ?? m.parent
    const base = {
      key: bundleKey('memo', i),
      name: m.name,
      agent: role,
      fragment,
      text: m.text,
      bytes: 0,
    }
    if (!safeName(m.name)) {
      return { ...base, path: null, kind: 'blocked', reason: 'badName' }
    }
    const anchor = home(role)
    if (!anchor) {
      // 三种不同的坏法，写三句不同的话：约定文件那家本机不支持、片段压根没记
      // `parent`、片段记了 `parent` 而本机落不了那一家（`cursor` 这种本机不认得的
      // 角色就落在最后一档）。并成一句的话，用户拿到的提示和他手上的包对不上。
      const reason: MemoBlock = !fragment ? 'unsupported' : role ? 'noHome' : 'noParent'
      return { ...base, path: null, kind: 'blocked', reason }
    }
    // 约定文件走**那家自己的**路径（文件名各家不一样：CLAUDE.md / AGENTS.md /
    // MEMORY.md），片段才按名字落在旁边。
    const path = fragment ? `${dirOf(anchor)}/${m.name}` : anchor
    const existing = onDisk.get(path)
    return existing && existing.exists
      ? { ...base, path, kind: 'overwrite', reason: null, bytes: existing.bytes }
      : { ...base, path, kind: 'create', reason: null }
  })
}

/**
 * 这一份全局指令能不能勾。
 *
 * 「装给哪几家」那一排**也管全局指令**。不管的话，用户只勾了 Kimi Code，
 * 计划框里却冒出一行「覆盖 ~/.claude/CLAUDE.md」—— 那一排的字面意思就是全局的，
 * 让它只管 MCP 和 hooks 是在骗人（方案 2.5：同步必须按 agent 勾，不能是广播）。
 *
 * 片段算它 `parent` 那一家：它本来就是跟着那份约定文件走的。
 */
export function memoPickable(target: MemoTarget, agents: Agent[]): boolean {
  if (target.kind === 'blocked') return false
  return isAgent(target.agent) && agents.includes(target.agent)
}

/**
 * 默认勾上哪些。
 *
 * MCP 和 hooks 全勾：它们是**加**一条，本机原有的那份要么同名被覆盖（计划框会把
 * 前后两条命令行并排列出来），要么不受影响。
 *
 * 全局指令只默认勾「新建」那几条。覆盖一份 `CLAUDE.md` 是把用户攒了很久的个人指令
 * 整个换掉，这种事不能靠他注意到某一行没取消勾选。
 */
export function defaultPicks(b: Bundle, targets: MemoTarget[], agents: Agent[]): Set<string> {
  const picked = new Set<string>()
  b.mcp.forEach((_, i) => picked.add(bundleKey('mcp', i)))
  b.hooks.forEach((_, i) => picked.add(bundleKey('hooks', i)))
  for (const t of targets) if (t.kind === 'create' && memoPickable(t, agents)) picked.add(t.key)
  return picked
}

/**
 * 勾上的东西里有没有"必须指定 agent 才写得了"的。
 *
 * 不能拿 `bundleMcpEdits(...).length` 来判 —— 那个函数是**按 agent 展开**的，
 * 一家都没勾时它恒为空，于是"至少勾一家"那句提示永远不会出现，用户看到的是
 * "一步都排不出来"，而那句话没告诉他该做什么。
 */
export function needsAgents(b: Bundle, picked: ReadonlySet<string>): boolean {
  return (
    b.mcp.some((_, i) => picked.has(bundleKey('mcp', i))) ||
    b.hooks.some((_, i) => picked.has(bundleKey('hooks', i)))
  )
}

/**
 * 包里提到过的角色，**原样**，包括本机不认识的那些。
 *
 * `suggestedAgents` 过滤掉了本机没有的（它要产出 `Agent[]`），但界面上那句提示该说
 * 全：导出那台机器上还跑着一个 cursor，这件事本身就是信息 —— 不说的话用户会以为
 * 这个包只覆盖了他看到的这几家。
 */
export function bundleAgentNames(b: Bundle): string[] {
  const seen: string[] = []
  const add = (a: string) => {
    if (a && !seen.includes(a)) seen.push(a)
  }
  for (const s of b.mcp) s.agents.forEach(add)
  for (const h of b.hooks) h.agents.forEach(add)
  for (const s of b.skills) s.agents.forEach(add)
  for (const m of b.memo) {
    const role = m.agent ?? m.parent
    if (role) add(role)
  }
  return seen
}

/**
 * 包里那几家在本机也认得的 —— 目标 agent 的默认勾选。
 *
 * **全局指令那一类也要算**：只看 `agents` 字段的话，一个只带全局指令的包（分享
 * 一套写法给别人，完全是个正常用法）一家都选不出来，界面上那一排全是灰的，而
 * 下面的条目又摆在那儿 —— 看上去像坏了。片段算它 `parent` 那一家。
 */
export function suggestedAgents(b: Bundle): Agent[] {
  const named = new Set<string>()
  for (const s of b.mcp) for (const a of s.agents) named.add(a)
  for (const h of b.hooks) for (const a of h.agents) named.add(a)
  for (const s of b.skills) for (const a of s.agents) named.add(a)
  for (const m of b.memo) {
    const role = m.agent ?? m.parent
    if (role) named.add(role)
  }
  return ALL_AGENTS.filter((a) => named.has(a))
}

// ---------------------------------------------------------------------------
// 导出侧
// ---------------------------------------------------------------------------

/** 落盘的文件名。带日期 —— 一个人多半会存好几份（"上班那套"/"折腾那套"）。 */
export function bundleFileName(at: Date): string {
  const p = (n: number) => String(n).padStart(2, '0')
  return `tools-bundle-${at.getFullYear()}-${p(at.getMonth() + 1)}-${p(at.getDate())}.json`
}

/** 存成人能读的样子：这个文件是拿去发给别人的，对方多半会先打开看一眼。 */
export function bundleText(b: Bundle): string {
  return `${JSON.stringify(b, null, 2)}\n`
}

// ---------------------------------------------------------------------------
// 确认框
// ---------------------------------------------------------------------------

/**
 * 把三份来源不同的计划并成**一个**确认框。
 *
 * 不是弹三次：导入是一次操作，分三次确认的话用户在第二次上点了取消，前面那一批
 * 已经写下去了 —— 他以为自己取消了整件事。并成一份之后按钮只有一个，按下去之前
 * 什么都没发生。
 *
 * MCP 和 hooks 那两段的行**照用各自的翻译函数**（`mcpPlanView` / `hookPlanView`），
 * 这儿只负责拼。全局指令没有 dry-run 报告（写入是一次一个文件的），所以它那几行
 * 在这儿现排。
 */
export function bundlePlanView(
  mcp: McpWriteReport | null,
  hooks: HookWriteReport | null,
  memo: MemoTarget[],
  home: string,
): PlanView {
  const mcpPlan = mcp ? mcpPlanView('', mcp, home) : null
  const hookPlan = hooks ? hookPlanView('', hooks, home) : null
  const writes = memo.filter((m) => m.kind !== 'blocked')
  // 覆盖一份已有的全局指令是这一整套里唯一会**丢东西**的一步。
  const overwrites = writes.filter((m) => m.kind === 'overwrite')

  const counts = {
    mcp: mcpPlan?.rows.length ?? 0,
    hooks: hookPlan?.rows.length ?? 0,
    memo: writes.length,
  }
  const danger = overwrites.length > 0 || !!mcpPlan?.danger || !!hookPlan?.danger
  const rows = [
    ...(mcpPlan?.rows ?? []),
    ...(hookPlan?.rows ?? []),
    ...writes.map((m) => ({
      kind: m.kind === 'overwrite' ? 'remove' : 'add',
      kindLabel: t(`tools.bundle.memo.${m.kind}`),
      where: shortenPath(m.path!, home),
      detail: `${m.text.length} ${t('tools.bundle.chars')}`,
      note: m.kind === 'overwrite' ? t('tools.bundle.memo.overwriteNote', { n: String(m.bytes) }) : undefined,
      noteWarn: m.kind === 'overwrite',
    })),
  ]

  return {
    title: t('tools.bundle.plan.title'),
    summary: countSummary(counts, (kind, n) => t(`tools.bundle.plan.count.${kind}`, { n: String(n) })),
    rows,
    blocked: [
      ...(mcpPlan?.blocked ?? []),
      ...(hookPlan?.blocked ?? []),
      ...memo
        .filter((m) => m.kind === 'blocked')
        .map((m) => `${m.name} —— ${t(`tools.bundle.block.${m.reason}`, { agent: m.agent ?? '' })}`),
    ],
    danger,
    applyLabel: t('tools.bundle.plan.apply', { n: String(rows.length) }),
    footnote: danger ? t('tools.bundle.plan.dangerNote') : t('tools.bundle.plan.backupNote'),
  }
}

/** 只留勾上的那几类。导出侧的四个勾选框在前端生效，不为了改一个勾再扫一遍磁盘。 */
export function filterBundle(b: Bundle, include: BundleInclude): Bundle {
  const mcp = include.mcp ? b.mcp : []
  return {
    ...b,
    mcp,
    hooks: include.hooks ? b.hooks : [],
    memo: include.memo ? b.memo : [],
    skills: include.skills ? b.skills : [],
    // 抹掉的值全来自 MCP。不带 MCP 的包里留着那张单子，读的人会去找一个不存在的 server。
    redacted: mcp.length > 0 ? b.redacted : [],
  }
}
