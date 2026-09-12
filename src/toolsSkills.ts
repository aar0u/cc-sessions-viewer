// 工具管理 · Skills 面板的纯逻辑：过滤、搜索、排序、角标与链路展示。
//
// 为什么单独一个模块：面板住在 `src/views/`、对话框住在 `src/modals/`，这两个目录在
// `vitest.config.ts` 是**覆盖率排除**的（和 App.vue 一样属于有状态的壳）。判定逻辑要是写在 .vue
// 里，这个功能等于一条单测都没有 —— 而「哪条链算断了」「哪个角标该赢」恰恰是最不该
// 靠肉眼验的部分。所以这里只有数据进数据出，不碰 DOM、不碰 Tauri。

import { ref } from 'vue'
import type {
  Agent,
  RefHealth,
  RiskLevel,
  SkillBadge,
  SkillEntry,
  SkillRef,
  SkillScan,
  StoreCandidate,
} from './types'

// ---------------------------------------------------------------------------
// 等级与角标的次序
// ---------------------------------------------------------------------------

/** 由轻到重。取最高等级一律走 `worstRisk`，别在模板里散写比较。 */
export const RISK_ORDER: RiskLevel[] = ['none', 'low', 'medium', 'high', 'critical']

export function riskRank(level: RiskLevel): number {
  return RISK_ORDER.indexOf(level)
}

export function worstRisk(levels: RiskLevel[]): RiskLevel {
  return levels.reduce<RiskLevel>((a, b) => (riskRank(b) > riskRank(a) ? b : a), 'none')
}

/**
 * 由轻到重。列表每行只有一个角标位，多个问题时显示最重的那个 —— 一条既断链又重复的
 * skill，先要解决的是断链。
 */
export const BADGE_ORDER: SkillBadge[] = ['duplicate', 'twoHop', 'copyStale', 'cyclic', 'broken']

export function badgeRank(badge: SkillBadge): number {
  return BADGE_ORDER.indexOf(badge)
}

/** 一组角标里最重的那个；没有问题返回 null。 */
export function worstBadge(badges: SkillBadge[]): SkillBadge | null {
  if (badges.length === 0) return null
  return badges.reduce((a, b) => (badgeRank(b) > badgeRank(a) ? b : a))
}

// ---------------------------------------------------------------------------
// 路径
// ---------------------------------------------------------------------------

/**
 * 把 home 下的绝对路径缩写成 `~/…`。后端一律返回绝对路径，缩写是纯展示。
 *
 * 分隔符两种都认：Windows 上后端回的是 `C:\Users\me\.agents\skills`，只判 `/`
 * 的话每一条路径都缩不掉，整个面板会摊开一屏的 `C:\Users\…`。缩完保留原来的
 * 分隔符 —— 显示的是用户机器上真实的样子，不是我们统一过的样子。
 */
export function shortenPath(path: string, home: string): string {
  if (!home) return path
  const base = home.replace(/[/\\]+$/, '')
  if (path === base) return '~'
  const next = path[base.length]
  if (path.startsWith(base) && (next === '/' || next === '\\')) return '~' + path.slice(base.length)
  return path
}

/** store 的展示名：缩写路径。第三方 store 没有 agent，就靠路径认。 */
export function storeLabel(store: StoreCandidate, home: string): string {
  return shortenPath(store.path, home)
}

/**
 * 这个 skill 的风险结论可信吗。
 *
 * `truncated` 时 `risk` 只是**已扫部分**里的最高值 —— 危险脚本可能就在第 401 个文件里，
 * 或藏在第 9 层目录。UI 必须据此把话说成「至少 X」，不能显示成结论：把部分扫描结果
 * 呈现为「干净」比不扫更糟，因为用户会据此放心。
 */
export function riskIsConclusive(entry: Pick<SkillEntry, 'truncated'>): boolean {
  return !entry.truncated
}

// ---------------------------------------------------------------------------
// 条目
// ---------------------------------------------------------------------------

/**
 * 哪些 agent 能用到这个 skill —— 列表里那排 `C X · · ·` 点。
 *
 * 一条引用可能被好几家读到：grok 开着 `[compat.claude] skills` 就也扫 `~/.claude/skills`。
 * 每条引用只记一家的话，用户会以为放在那儿的 skill 别家用不了。
 */
export function agentsOf(entry: SkillEntry): Agent[] {
  const seen: Agent[] = []
  for (const r of entry.refs) {
    for (const a of r.agents) if (!seen.includes(a)) seen.push(a)
  }
  return seen
}

/** 这个 skill 的内容散落在哪几个 store 里。多于一个就是重复的来源。 */
export function bodyStores(entry: SkillEntry): string[] {
  const seen: string[] = []
  for (const b of entry.bodies) {
    const store = b.store ?? b.path
    if (!seen.includes(store)) seen.push(store)
  }
  return seen
}

/**
 * 一个 agent 现在**能不能读到**这条 skill，以及是靠哪条引用读到的。
 *
 * 这里不能只看「这家自己的 skills 目录里有没有」。实测（opencode `debug skill` 的输出、
 * pi 在空目录下的 `/skill:` 补全）：`~/.agents/skills` 是 grok / pi / opencode 三家都会
 * **原生扫描**的公共目录，`~/.claude/skills` 也被 grok 和 opencode 默认读。所以一条只
 * 躺在 `~/.agents/skills` 里的 skill，对这三家来说早就是启用状态了。
 *
 * 按自有目录判的后果是双重的：详情页的开关会和列表行上那排 agent 圆点**互相矛盾**
 * （圆点走的是 `agentsOf`，看的是引用上的 `agents`）；而用户照着那个假的「关」点一下，
 * 我们就会在这家自己的目录里再建一条指向它本来就读得到的内容的链接 —— 正是这个面板
 * 要清理的「绕远路 / 重复」，由面板自己制造出来。
 *
 * 三种状态：
 * - `off`   没有任何健康引用能被它读到。
 * - `own`   靠**它自己的** skills 目录读到的 —— 只有这种能在这里单独停用。
 * - `shared` 靠一个多家共用的目录读到的（`~/.agents/skills`、或 grok/opencode 兼容读的
 *   `~/.claude/skills`）。这种不能在这里关：那个目录不归它管，删掉会连着把别家一起断掉。
 */
export type AgentReach = 'off' | 'own' | 'shared'

export interface ReachInfo {
  state: AgentReach
  /** 让这家读到它的那些引用（健康的）。`off` 时为空。 */
  via: SkillRef[]
  /** 它自己目录里那条健康引用，没有则 null。停用只动这一条。 */
  own: SkillRef | null
}

export function agentReach(
  entry: SkillEntry,
  agent: Agent,
  ownDir: string | null | undefined,
): ReachInfo {
  const via = entry.refs.filter((r) => r.agents.includes(agent) && !isUnhealthy(r))
  const own = (ownDir && via.find((r) => r.store === ownDir)) || null
  if (via.length === 0) return { state: 'off', via, own: null }
  return { state: own ? 'own' : 'shared', via, own }
}

/**
 * 跨 agent 的公共目录（`~/.agents/skills`）上这个 skill 的那条引用。
 *
 * 这个目录不属于任何一家，但 codex / grok / kimi / opencode / pi 都会去扫它（claude 和
 * agy 不读，见 `tools/mod.rs` 的 `exactly_the_right_agents_read_the_cross_agent_hub`）——
 * 所以在里面建**一条**链接，这几家一起生效，不用往各自的目录里分别塞一条。
 * UI 上具体列哪几家一律取扫描结果里这个 store 的 `agents`，不在前端写死。
 * 用户把主 store 挑到别处
 * （`~/.skills-manager/skills` 之类）之后它就空了，这时它更需要一条链接：收编只保证
 * 「内容搬走的地方留个链接」，从没在这儿出现过的内容不会自己长过来。
 *
 * 不健康的（断链 / 成环 / 指向非目录）不算数 —— 那种引用读不到东西。
 */
export function sharedDirRef(entry: SkillEntry, dir: string | null): SkillRef | null {
  if (!dir) return null
  return entry.refs.find((r) => r.store === dir && !isUnhealthy(r)) ?? null
}

/** 这条引用有没有问题。断链、成环、指向非目录都算 —— 三种都是「用不了」。 */
export function isUnhealthy(ref: SkillRef): boolean {
  const s = ref.health.state
  return s === 'broken' || s === 'cyclic' || s === 'notADirectory'
}

/**
 * 一条引用的链路，逐跳一行，供详情页缩进渲染。
 *
 * 实体目录没有跳，返回它自己一行。断链的最后一行 `exists` 为 false —— 断点就在那儿，
 * 光说「断了」用户没法修。
 */
export interface ChainLine {
  /** 缩过的显示用路径（`~/…`）。 */
  path: string
  /**
   * 没缩过的真路径。
   *
   * 「在文件管理器中显示」只认绝对路径 —— 拿 `path` 去 reveal 会打开一个叫 `~` 的
   * 相对目录，或者干脆什么都不发生。两个都留着，别让调用方自己去还原。
   */
  abs: string
  exists: boolean
  depth: number
}

export function chainLines(ref: SkillRef, home: string): ChainLine[] {
  const line = (abs: string, exists: boolean, depth: number): ChainLine => ({
    path: shortenPath(abs, home),
    abs,
    exists,
    depth,
  })
  const head = line(ref.path, true, 0)
  if (ref.hops.length === 0) {
    // 受管副本在磁盘上是目录，但它有个「源」，那个源才是内容。
    if (ref.health.state === 'managedCopy') {
      return [head, line(ref.health.detail, true, 1)]
    }
    return [head]
  }
  return [head, ...ref.hops.map((h, i) => line(h.to, h.exists, i + 1))]
}

/**
 * 这条引用是不是一条**没人读的活链接** —— 除了拆掉它，没有别的出路。
 *
 * 「停用」是**按 agent 关**的：它删掉的是那家自己目录里的那一条。而第三方 store
 * （`~/.skills-manager`、`~/.cc-switch`）里的链接不属于任何一家，那个开关碰不到它。
 * 死链有「修链 · 清理死链」兜着，活链接原来一个入口都没有 —— 收编完成之后，旧 store
 * 里会各剩一条指向新家的链接，谁也不读，谁也删不掉。
 *
 * 有 agent 够得着的**不给**：那种要关就去关那家的开关，在这儿多开一个「删链接」等于
 * 给同一件事两个入口，而其中一个不说清楚会断掉谁。同样看 `reachedBy` —— 一条谁都不
 * 直接读的链接，可能正是别人那条链的中途一跳，拆了它上游整条就断了。
 */
export function removableLink(ref: SkillRef): boolean {
  return ref.health.state === 'linked' && ref.reachedBy.length === 0
}

/** 健康状态的 i18n key + 参数，给 tooltip 用。 */
export function healthTip(health: RefHealth, home: string): { key: string; vars?: Record<string, string> } {
  switch (health.state) {
    case 'realDir':
      return { key: 'tools.skills.health.realDir' }
    case 'linked':
      return { key: 'tools.skills.health.linked' }
    case 'broken':
      return { key: 'tools.skills.health.broken', vars: { target: shortenPath(health.detail, home) } }
    case 'cyclic':
      return { key: 'tools.skills.health.cyclic' }
    case 'managedCopy':
      return { key: 'tools.skills.health.managedCopy', vars: { source: shortenPath(health.detail, home) } }
    case 'copyStale':
      return { key: 'tools.skills.health.copyStale', vars: { source: shortenPath(health.detail, home) } }
    case 'copyEdited':
      return { key: 'tools.skills.health.copyEdited', vars: { source: shortenPath(health.detail, home) } }
    case 'copyDiverged':
      return { key: 'tools.skills.health.copyDiverged', vars: { source: shortenPath(health.detail, home) } }
    case 'copyOrphaned':
      return { key: 'tools.skills.health.copyOrphaned', vars: { source: shortenPath(health.detail, home) } }
    case 'notADirectory':
      return {
        key: 'tools.skills.health.notADirectory',
        vars: { target: shortenPath(health.detail, home) },
      }
  }
}

// ---------------------------------------------------------------------------
// 过滤 / 搜索 / 排序
// ---------------------------------------------------------------------------

export interface SkillFilter {
  /** 匹配名字、描述、以及任一引用/内容所在的路径。 */
  query: string
  /** 只看被某个 agent 引用的。null = 全部。 */
  agent: Agent | null
  /** 只看带某个角标的。null = 全部。 */
  badge: SkillBadge | null
  /** 只看从远端 clone 来的。false = 不过滤。 */
  fromGit: boolean
  /** 只看风险不低于该等级的。'none' = 不过滤。 */
  minRisk: RiskLevel
}

export const emptyFilter = (): SkillFilter => ({
  query: '',
  agent: null,
  badge: null,
  fromGit: false,
  minRisk: 'none',
})

export function matchesQuery(entry: SkillEntry, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (!q) return true
  if (entry.name.toLowerCase().includes(q)) return true
  if (entry.description?.toLowerCase().includes(q)) return true
  // 路径也参与搜索：用户常常是「装在 cc-switch 里的那批」这样找的。
  return (
    entry.refs.some((r) => r.path.toLowerCase().includes(q)) ||
    entry.bodies.some((b) => b.path.toLowerCase().includes(q))
  )
}

/**
 * 命中落在哪儿 —— 数字越小越靠前。没搜索时恒为 0（排序退回原样）。
 *
 * 搜索能命中**描述**，而描述那一行是省略号截断的：搜 `hyperframes`，本机 12 条结果里
 * 有 6 条名字里根本没这个词（`lottie` / `tailwind` / `three` / `waapi` …，它们的描述里
 * 写着 "adapter patterns for HyperFrames"）。列表看上去像压根没过滤。
 */
export function queryRank(entry: SkillEntry, query: string): number {
  const q = query.trim().toLowerCase()
  if (!q) return 0
  const name = entry.name.toLowerCase()
  if (name === q) return 0
  if (name.startsWith(q)) return 1
  if (name.includes(q)) return 2
  if (entry.description?.toLowerCase().includes(q)) return 3
  return 4
}

/**
 * 只命中路径时，命中的那条路径；否则 `null`。
 *
 * 行的第二行平时显示描述。搜 `cc-switch` 这种只落在路径上的词，名字和描述里都没有
 * 用户刚打的那个词，整行看上去和搜索毫无关系 —— 那就把命中的那条路径顶上来。
 */
export function queryPath(entry: SkillEntry, query: string): string | null {
  const q = query.trim().toLowerCase()
  if (!q) return null
  if (entry.name.toLowerCase().includes(q)) return null
  if (entry.description?.toLowerCase().includes(q)) return null
  return (
    entry.bodies.find((b) => b.path.toLowerCase().includes(q))?.path ??
    entry.refs.find((r) => r.path.toLowerCase().includes(q))?.path ??
    null
  )
}

export function filterSkills(skills: SkillEntry[], filter: SkillFilter): SkillEntry[] {
  const floor = riskRank(filter.minRisk)
  return skills.filter((s) => {
    if (!matchesQuery(s, filter.query)) return false
    if (filter.agent && !agentsOf(s).includes(filter.agent)) return false
    if (filter.badge && !s.badges.includes(filter.badge)) return false
    if (filter.fromGit && !s.git) return false
    if (floor > 0 && riskRank(s.risk) < floor) return false
    return true
  })
}

/**
 * agent 过滤器（面板左边那排图标，多选）。**空数组 = 不过滤**。
 *
 * 不能传「当前生效的全部 agent」进来当等价写法：只存在于第三方 store（`~/.agents`、
 * `~/.cc-switch`）里、还没被任何 agent 引用的 skill，`agents` 是空的 —— 拿七家去交集
 * 会把它们全过滤掉，而那恰恰是最需要被看见的一类（没人在用，但占着一份内容）。
 */
export function matchesAgents(entry: SkillEntry, agents: Agent[]): boolean {
  if (agents.length === 0) return true
  return agentsOf(entry).some((a) => agents.includes(a))
}

/** 列表最终显示的那一批：过滤 + agent 过滤 + 排序，一次算完。 */
export function visibleSkills(
  skills: SkillEntry[],
  filter: SkillFilter,
  agents: Agent[],
  pinned: string[] = [],
  sort: SkillSort = 'newest',
): SkillEntry[] {
  return sortSkills(
    filterSkills(skills, filter).filter((s) => matchesAgents(s, agents)),
    pinned,
    filter.query,
    sort,
  )
}

/**
 * 列表次序。
 *
 * `newest` 是默认：这一列每天要回答的是「我刚才装的那个在哪」。`oldest` 给的是另一个
 * 问题 ——「哪些是很久没碰过的」，那通常就是该清掉的一批。`name` 是在心里已经有名字、
 * 只是想按字母扫过去的时候用的。
 */
export type SkillSort = 'newest' | 'oldest' | 'name'

export const SKILL_SORTS: readonly SkillSort[] = ['newest', 'oldest', 'name'] as const

/**
 * 这条 skill 最近一次被动过是什么时候（毫秒）。
 *
 * 同名重复时取**最新**那份：用户问的是「这个名字最近动过没有」，而不是「最老的那份
 * 多老」。一份时间都读不到返回 0 —— 那是「没有内容可读」，排序里另有安排（见 `sortSkills`）。
 */
export function newestBody(entry: SkillEntry): number {
  let newest = 0
  for (const body of entry.bodies) {
    if (body.modified != null && body.modified > newest) newest = body.modified
  }
  return newest
}

/**
 * 最近动过的排前面。
 *
 * 早先是「有问题的排前面」，理由是机器上没有任何东西告诉你哪些坏了。那个理由还在，
 * 但答案已经搬到面板顶上那条健康条了 —— 重复 28 / 绕远路 12 是可点的筛子，问「哪些
 * 坏了」一下就到。而列表本身每天要回答的是另一个问题：**我刚才装的那个在哪**。按
 * 健康度排会把它扔进 49 条的中段，得一行行找。
 *
 * 时间取各份 body 里最新的那个 mtime。**一个时间戳都读不到的排在最前**，不是最后：
 * 那种条目多半压根没有实体目录（链接断了、指到别处去了），它不是「很旧」而是「不在了」，
 * 正是这个面板要喊的那一种。它们内部仍按角标和风险排。
 *
 * 用户能在健康条上换 `sort`（时间倒序 / 时间正序 / 按名称）。换成 `name` 时整段时间
 * 判断直接跳过，落回角标 → 风险 → 名字那一套。
 *
 * `pinned` 里的排在最前，**压过上面全部规则**（换哪一档都一样）：自动排序猜的是
 * 「你大概最该先看哪个」，置顶是用户自己说的「我就要看这个」，后者永远该赢。置顶
 * 之间保持置顶顺序不变（先置顶的在上），不跟着时间重排 —— 那一列的次序是用户自己
 * 攒出来的。
 */
export function sortSkills(
  skills: SkillEntry[],
  pinned: string[] = [],
  query = '',
  sort: SkillSort = 'newest',
): SkillEntry[] {
  const pinRank = (name: string) => {
    const i = pinned.indexOf(name)
    return i === -1 ? Number.MAX_SAFE_INTEGER : i
  }
  return [...skills].sort((a, b) => {
    const pa = pinRank(a.name)
    const pb = pinRank(b.name)
    if (pa !== pb) return pa - pb
    // 搜索时，命中位置压过健康度。默认那套「坏得最厉害的排最前」是给**浏览**用的，
    // 搜索是**找一个具体的东西**：搜 `hyperframes`，同名那条自己一个角标都没有，
    // 按健康度会被 11 条「重复」压到列表最底下 —— 用户打完字在第一屏看不到它。
    // 没搜索时 `queryRank` 恒为 0，这一档整个透明。
    const qa = queryRank(a, query)
    const qb = queryRank(b, query)
    if (qa !== qb) return qa - qb
    // 按名称就是**纯字母序**，角标和风险一概不插队。用户选这一档是因为心里已经有
    // 名字、只想扫过去找到它；再让「坏的排前面」插一手，找的那个还是不在该在的位置。
    if (sort === 'name') return a.name.localeCompare(b.name)
    {
      const ta = newestBody(a)
      const tb = newestBody(b)
      // 读不到时间的先走 —— **两档时间序都这样**：「没有时间」不是「很旧」，
      // 按正序把它排到最前、按倒序把它排到最后，两次得到的都不是用户问的东西。
      if ((ta === 0) !== (tb === 0)) return ta === 0 ? -1 : 1
      if (ta !== tb) return sort === 'newest' ? tb - ta : ta - tb
    }
    const ba = worstBadge(a.badges)
    const bb = worstBadge(b.badges)
    const ra = ba ? badgeRank(ba) : -1
    const rb = bb ? badgeRank(bb) : -1
    if (ra !== rb) return rb - ra
    const risk = riskRank(b.risk) - riskRank(a.risk)
    if (risk !== 0) return risk
    return a.name.localeCompare(b.name)
  })
}

/** 面板顶部那条健康条：每个角标各有多少条。 */
export function badgeCounts(skills: SkillEntry[]): Record<SkillBadge, number> {
  const out: Record<SkillBadge, number> = {
    duplicate: 0,
    twoHop: 0,
    copyStale: 0,
    cyclic: 0,
    broken: 0,
  }
  for (const s of skills) for (const b of s.badges) out[b] += 1
  return out
}

/**
 * 主 store 下拉里的可选项。
 *
 * 两道关，缺一条就会放进去不该放的东西：
 *
 * 1. `canBeMain` —— 后端判的「用户级跨 agent 共享目录」。项目目录
 *    （`<repo>/.agents/skills`）和 agent 自有目录（`~/.gemini/config/skills`）都在
 *    扫描结果里，但前者换个项目就没了、后者是链接落脚的地方，都不能装内容。
 * 2. `exists` —— 唯一的例外是兜底目录：本机一个 store 都没有时它还不存在，但必须
 *    列出来，否则新机器上下拉是空的，用户没有任何办法开始。第一次收编会建它。
 *
 * **不再要求 `realDirs > 0`**：一个空的 `~/.agents/skills` 是完全合法的起点。
 */
export function mainStoreOptions(scan: SkillScan): StoreCandidate[] {
  return scan.stores
    .filter((s) => s.canBeMain && (s.exists || s.path === scan.defaultMain))
    .sort((a, b) => b.realDirs - a.realDirs || a.path.localeCompare(b.path))
}

// ---------------------------------------------------------------------------
// 面板状态（浮层壳在阶段 3 接上来）
// ---------------------------------------------------------------------------

export const skillFilter = ref<SkillFilter>(emptyFilter())

export function resetSkillFilter() {
  skillFilter.value = emptyFilter()
}

// ---------------------------------------------------------------------------
// 置顶
// ---------------------------------------------------------------------------

const PIN_KEY = 'toolsSkillPins:v1'

/**
 * 按**名字**记，不按路径：同一个名字的重复条目在列表里本来就合成一行（一条 skill
 * 散在三个 store 里也还是一条 skill），按路径记的话收编搬完家置顶就丢了。
 */
function loadPins(): string[] {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(PIN_KEY) ?? '[]')
    return Array.isArray(raw) ? raw.filter((v): v is string => typeof v === 'string') : []
  } catch {
    return []
  }
}

export const pinnedSkills = ref<string[]>(loadPins())

const SORT_KEY = 'toolsSkillSort:v1'

function loadSort(): SkillSort {
  try {
    const raw = localStorage.getItem(SORT_KEY)
    return SKILL_SORTS.includes(raw as SkillSort) ? (raw as SkillSort) : 'newest'
  } catch {
    return 'newest'
  }
}

/**
 * 当前次序。存下来 —— 这是「我习惯怎么看这一列」，不是一次性的动作，每开一次面板
 * 重新选一遍是纯粹的重复劳动。
 */
export const skillSort = ref<SkillSort>(loadSort())

export function setSkillSort(sort: SkillSort) {
  skillSort.value = sort
  try {
    localStorage.setItem(SORT_KEY, sort)
  } catch {
    // 隐私模式下 localStorage 会抛。次序没记住不影响这个面板能用。
  }
}

export function isSkillPinned(name: string): boolean {
  return pinnedSkills.value.includes(name)
}

/** 置顶 / 取消置顶。新置顶的排在已置顶那批的**后面**，置顶列的次序才不会跳。 */
export function toggleSkillPin(name: string) {
  pinnedSkills.value = isSkillPinned(name)
    ? pinnedSkills.value.filter((n) => n !== name)
    : [...pinnedSkills.value, name]
  try {
    localStorage.setItem(PIN_KEY, JSON.stringify(pinnedSkills.value))
  } catch {
    // 隐私模式下 localStorage 会抛。置顶丢了不影响这个面板能用。
  }
}
