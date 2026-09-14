export type Agent = 'claude' | 'codex' | 'agy' | 'opencode' | 'grok' | 'kimicode' | 'pi'

export interface ProjectInfo {
  dirName: string
  displayPath: string
  sessionCount: number
  lastModified: number
  /** 项目目录当前是否仍存在于磁盘上 */
  exists: boolean
  bookmarked?: boolean
  parentDirName?: string
  worktreeName?: string
}

export interface SessionMeta {
  id: string
  fileName: string
  path: string
  title: string
  cwd?: string
  created?: string
  modified: number
  size: number
  messageCount: number
  /** Pi-only: terminal branches and complete persisted tree entries. */
  piBranchCount?: number
  piEntryCount?: number
  codexAppListRank?: number | null
  codexAppListScanned: number
  codexAppFirstPageSize: number
  codexAppFirstPagePosition: number
  codexInternal: boolean
  codexArchived: boolean
}

export interface PiTreeNode {
  id: string
  parentId?: string | null
  children: string[]
  kind: string
  timestamp?: string | null
  ordinal: number
  terminal: boolean
}

export interface SessionPage {
  total: number
  sessions: SessionMeta[]
}

export type BlockKind = 'text' | 'thinking' | 'tool_use' | 'tool_result' | 'image' | 'file'

export interface DiffLine {
  kind: 'ctx' | 'add' | 'del'
  oldNo: number | null
  newNo: number | null
  text: string
}

export interface DiffHunk {
  oldStart: number
  newStart: number
  lines: DiffLine[]
}

export interface GitCommit {
  hash: string
  author: string
  date: string
  message: string
}

export interface GitFileStatus {
  path: string
  status: string
}

export interface GitRepositoryState {
  branch: string | null
  branches: string[]
  changeCount: number
}

export interface GitDiffFile {
  path: string
  additions: number
  deletions: number
  status: string
}

export interface Block {
  kind: BlockKind
  text?: string
  toolName?: string
  toolInput?: string
  toolId?: string
  isError: boolean
  filePath?: string
  fileChangeType?: 'add' | 'update' | 'delete' | string
  /** file 块：该路径是目录（GUI chat 的「Add folder」附件）。决定 chip 用文件夹图标 +
   *  「打开文件夹」提示，而非文件图标 +「打开文件」。 */
  isDir?: boolean
  diff?: DiffHunk[]
  imageSrc?: string
  /** 历史剪贴板图片路径存在但本地临时文件已被清理。仍按图片布局显示裂图占位。 */
  imageUnavailable?: boolean
  /** Chat 粘贴图片在正文中的 token；历史/本地回显用来保持图片绑定。 */
  inlinePlaceholder?: string
  /** Pi todo tool result's structured current task list. */
  piTodoSummary?: PiTodoSummary
}

export interface PiTodoSummary {
  completed: number
  total: number
  tasks: Array<{
    subject: string
    status: string
  }>
}

export interface Msg {
  uuid?: string
  role: 'user' | 'assistant'
  timestamp?: string
  /** Live Chat 中从本轮用户输入到该条模型/工具消息到达的耗时（仅前端运行时字段）。 */
  executionMs?: number
  model?: string
  sidechain: boolean
  blocks: Block[]
  /** 系统注入的 `type:"user"` 记录归类（compact / meta / task-notification /
   *  system / command-output）。后端 claude 源填充；其它 agent 不填 → undefined。
   *  非空时前端把这条渲染成低调的「系统」块，而非「Me」气泡。 */
  metaKind?: string
}

/** 全局搜索的命中条目（与 Rust 端 SearchHit 同形）。 */
export type SearchField = 'title' | 'id' | 'path' | 'text'

/** 全局搜索的范围：只看会话 ID，或看标题 + 用户消息正文（不含 ID）。 */
export type SearchScope = 'id' | 'keyword'
export interface SearchHit {
  projectKey: string
  projectDisplay: string
  session: SessionMeta
  matchedField: SearchField
  /** 命中片段：title/id/path 等于原值；text 上是带前后文（带省略号）的小段。 */
  snippet: string
  /** 文本命中所在消息的索引（read_session 返回的数组下标）；metadata 命中为 undefined。 */
  matchMsgIndex?: number
  /** 文本命中所在消息的 uuid（若 agent 写了）；前端定位时优先用 uuid 兜底。 */
  matchMsgUuid?: string
  /** Pi-only terminal entry containing the text hit. */
  piLeafId?: string
}

/** 单个会话的 token 用量；与 Rust 端 UsageSummary 同形。
 *  `cacheCreation1hInputTokens` 是 `cacheCreationInputTokens` 的子集（1-hour tier），
 *  cost 公式额外按 1× 5min 价位再算一遍（合计 2×），别在 UI 上把它加进 total。 */
export interface UsageSummary {
  inputTokens: number
  outputTokens: number
  cacheCreationInputTokens: number
  cacheCreation1hInputTokens: number
  cacheReadInputTokens: number
  reasoningOutputTokens: number
  total: number
}

/** 统计 dashboard：单个项目的聚合（与 Rust ProjectStats 同形）。 */
export interface ProjectStats {
  dirName: string
  displayPath: string
  sessionCount: number
  messageCount: number
  callCount: number
  usage: UsageSummary
  costUsd: number
  lastModified: number
}

/** 统计 dashboard：某一天（UTC）的活动量。 */
export interface DailyActivity {
  date: string // YYYY-MM-DD
  sessionCount: number
  messageCount: number
  callCount: number
  tokens: number
  costUsd: number
}

/** Top Sessions 排行里的一条。 */
export interface SessionStat {
  agent: Agent
  sessionId: string
  path: string
  projectDisplay: string
  title: string
  lastModified: number
  callCount: number
  usage: UsageSummary
  costUsd: number
}

/** By Model 排行里的一条。 */
export interface ModelStat {
  model: string
  label: string
  callCount: number
  usage: UsageSummary
  costUsd: number
  /** 价格表未命中的真实 API 调用数；0 也可能是已知免费，不能只看 costUsd 判断。 */
  unpricedCallCount: number
  /** 使用 Grok 官方旗舰价格兜底估算的真实 API 调用数。 */
  estimatedCallCount: number
  /** 0..=1。cache_read / (input + cache_read + cache_creation)。 */
  cacheHitRate: number
}

/** By Tool / By Shell / By MCP 共用 name+count 对。 */
export interface NamedCount {
  name: string
  count: number
}

/** By Activity 一行：分类 key + 调用 / 成本。`key` 对应 stats.activity.* 翻译。 */
export interface ActivityStat {
  key: string
  turnCount: number
  callCount: number
  costUsd: number
}

/** 统计范围筛选 —— 前端 dropdown 切换。 */
export type StatsScope = 'all' | Agent

/** 时间范围筛选。`custom:start:end` 使用本地日期（YYYY-MM-DD），end 按整日包含。 */
export type StatsPresetRange = 'today' | 'days7' | 'days30' | 'month' | 'months3' | 'months6'
export type StatsRange = StatsPresetRange | `custom:${string}:${string}`

/** 流式统计的完整结果（与 Rust AgentStats 同形）。`scope` 标识维度。 */
export interface AgentStats {
  scope: 'all' | Agent | string
  sessionCount: number
  messageCount: number
  callCount: number
  daysActive: number
  usage: UsageSummary
  costUsd: number
  /** 未命中价格表的真实 API 调用数；costUsd 只包含已知价格部分。 */
  unpricedCallCount: number
  /** 使用 Grok 官方旗舰价格兜底估算的真实 API 调用数。 */
  estimatedCallCount: number
  /** Pi persisted `usage.cost.total` calls. */
  recordedCallCount: number
  /** Calls priced by the strict provider/model catalog fallback. */
  catalogCallCount: number
  cacheHitRate: number
  /** 按 cost_usd 降序的项目列表。 */
  projects: ProjectStats[]
  /** 按日期升序的日活时间轴（稀疏，没活动的天不出现）。 */
  dailyActivity: DailyActivity[]
  /** 按 cost_usd 降序的 Top 10 会话。 */
  topSessions: SessionStat[]
  /** 按 cost_usd 降序的模型排行。 */
  byModel: ModelStat[]
  /** 按调用次数降序的工具排行。 */
  byTool: NamedCount[]
  /** 按调用次数降序的 shell 主命令排行。 */
  byShell: NamedCount[]
  /** 按调用次数降序的 MCP server 排行。 */
  byMcp: NamedCount[]
  /** 按 cost_usd 降序的活动分类排行。 */
  byActivity: ActivityStat[]
}

/** 流式推送的进度负载。`partial` 是到目前为止的累计快照，前端直接替换。 */
export interface StatsProgress {
  requestId: number
  processed: number
  total: number
  partial: AgentStats
}

export interface StatsDone {
  requestId: number
  stats: AgentStats
}

export interface StatsError {
  requestId: number
  error: string
}

// ============================ GUI chat（程序化聊天）============================

/** 一轮问答的运行状态。 */
export type ChatTurnState = 'idle' | 'running'

/** 输入框里的图片附件（粘贴 / 拖拽 / 选择）。`dataUrl` 供预览与本地回显，
 *  `data` 是去掉 `data:` 前缀的纯 base64，发送给后端时用。 */
export interface ChatImageAttachment {
  dataUrl: string
  mediaType: string
  data: string
  /** 文件名（来自文件选择/拖拽；粘贴的截图回退 image.png）。仅前端展示用。 */
  name?: string
  /** 原始磁盘路径（文件选择器 / 拖拽得到）。粘贴板截图无此字段。
   *  Codex 等 OneShot agent 用 `@"path"` 引用本地文件而非传 base64。 */
  sourcePath?: string
  /** 粘贴到正文中的稳定占位符（例如 `[Image #1]`）。普通附件没有此字段。 */
  inlinePlaceholder?: string
}

/** agent_chat_send 透传给后端的图片输入（与 Rust ChatImageInput 同形）。 */
export interface ChatImageInput {
  mediaType: string
  data: string
  /** Claude stdin 用来把图片内容块放回正文语义位置；Codex 会忽略。 */
  placeholder?: string
}

export interface ChatTextElement {
  type: 'mention'
  byteRange: {
    start: number
    end: number
  }
  path: string
  name: string
  placeholder?: string
}

/**
 * 非图片附件（文件 / 文件夹）。由系统选择器选出，发送时以 `@"path"` 追加到 prompt，
 * 让 agent 自己按路径读取。`isDir` 仅影响 chip 图标（文件夹用 folder 图标）。
 */
export interface ChatFileAttachment {
  path: string
  name: string
  isDir: boolean
}

/** GUI chat `@` 文件浮层的一条目录/文件项（与 Rust ProjectFileEntry 同形）。
 *  `relPath` 相对会话 cwd（`/` 分隔）；`name` 是末段名字；`isDir` 决定图标 + 钻取行为。 */
export interface ProjectFileEntry {
  relPath: string
  name: string
  isDir: boolean
  /** 仅目录有意义：是否含可见子项。空目录 = false → 不显示「进入」chevron、禁用下钻。 */
  hasChildren: boolean
}

/** GUI chat `/` 浮层的一条可用项（命令 / 技能，与 Rust SlashCommand 同形）。 */
export interface SlashCommand {
  /** 调用 token（无前导 `/`）：命令命名空间名 / 技能名。 */
  name: string
  /** 展示名：命令 = `/name`；技能 = 美化后的 Title Case。 */
  title: string
  description: string
  /** 分组 + 图标依据。`system` = 前端注入的客户端内置指令（不来自磁盘扫描）。 */
  kind: 'command' | 'skill' | 'system'
  /** 来源类别：user → UI 显示「Personal」；project / plugin → 用 originName；system → 无角标。 */
  origin: 'user' | 'project' | 'plugin' | 'system'
  /** 项目名 / 插件名（user 来源省略）。 */
  originName?: string
  /** 命令 `argument-hint`（如 `[--wait] [--base <ref>]`）：选中后在输入框作为 ghost 参数提示。 */
  argumentHint?: string
}

/** GUI chat 的进程模型：长驻 stdin / Codex app-server 切设置需 restart-with-resume；
 *  one-shot resume 则切设置改下轮 flag 即生效。 */
export type ChatProcessModel = 'longLivedStdin' | 'oneShotResume' | 'codexAppServer'

/** agent_chat_start 的返回（与 Rust ChatStartInfo 同形）。 */
export interface ChatStartInfo {
  chatId: number
  processModel: ChatProcessModel
}

export interface RunningChatInfo {
  chatId: number
  agent: Agent
  projectKey: string
  cwd: string
  sessionId: string | null
  title?: string
  messages?: Msg[]
  turnState?: ChatTurnState
  turnStartedAtMs?: number | null
  permissionMode: string
  model: string | null
  effort: string | null
  processModel: string
}

export interface ReclaudeInfo {
  installed: boolean
  daemonRunning: boolean
  daemonPort: number | null
}

export interface ClaudeRuntimeInfo {
  hasCustomBaseUrl: boolean
  aliasTargets: {
    opus?: string
    sonnet?: string
    haiku?: string
    fable?: string
  }
  /** init 事件回来前对鉴权方式的预判：'none' = 订阅/OAuth；其它 = API key；缺省 = 判不出。 */
  apiKeySource?: string
  /** settings.json 的 `effortLevel`：用户全局 reasoning effort 默认档。CLI 不带 --effort
   *  时即用它 —— effort 选择器在用户未改档前展示这个「真实生效默认」，而非假的 levels[0]。 */
  effortLevel?: string
}

export interface CodexRuntimeInfo {
  /** true = 用户通过第三方 API key / 自定义端点使用 Codex（config.toml 有 model_provider）。 */
  usesApiKey: boolean
  /** ~/.codex/config.toml 顶层 model；自定义 provider 下用于显示/勾选，不强制下发。 */
  model?: string
  /** ~/.codex/config.toml 顶层 model_reasoning_effort；保留给 UI 展示/后续扩展。 */
  effort?: string
}

/** agent-chat://* 事件 payload（与 Rust 端同形）。 */
export interface ChatEventPayload { chatId: number; msg: Msg }
export interface ChatInitPayload { chatId: number; sessionId?: string; apiKeySource?: string }
export interface ChatResultPayload { chatId: number; ok: boolean; usage?: UsageSummary }
export interface ChatStderrPayload { chatId: number; line: string }
export interface ChatExitPayload { chatId: number; code: number }

/** token 级流式增量（Claude stream_event / Codex app-server delta）。 */
export interface ChatDelta {
  index: number
  /** 'start' | 'delta' | 'stop' —— 内容块生命周期。 */
  phase: string
  /** 块类型 text | thinking | tool_use（start 必有；delta 也带，前端兜底建块）。 */
  kind?: string
  /** 仅 delta：本次追加的文本片段。 */
  text?: string
}
export interface ChatDeltaPayload { chatId: number; delta: ChatDelta }

/** 交互式工具权限请求（Claude 控制协议 `can_use_tool`，与 Rust ChatPermissionRequest 同形）。
 *  `input` 是工具参数原文（Bash 的 `command`、文件工具的 `file_path` 等）；
 *  `permissionSuggestions` 是「始终允许」的规则建议（`addRules`，含 destination）。 */
export interface ChatPermissionRequest {
  requestId: string
  toolName: string
  input: unknown
  description?: string
  permissionSuggestions?: unknown
}
export interface ChatPermissionPayload { chatId: number; request: ChatPermissionRequest }

/** AskUserQuestion 的单个选项。`preview` 是可选的等宽预览内容（mock / 代码 / 配置），
 *  仅单选题用得上 —— 渲染成左列选项、右栏预览的并排布局。 */
export interface ChatQuestionOption {
  label: string
  description?: string
  preview?: string
}
/** AskUserQuestion 的单条提问。`multiSelect` 为真时允许多选（答案逗号拼接）。 */
export interface ChatQuestionItem {
  question: string
  header?: string
  multiSelect?: boolean
  allowOther?: boolean
  options: ChatQuestionOption[]
}
/** 模型向用户提的结构化选择题（Claude `AskUserQuestion`，与 Rust ChatQuestionRequest 同形）。
 *  与工具权限同走 `can_use_tool` 控制协议，回写时把 `questions` 原样带回 `updatedInput`。 */
export interface ChatQuestionRequest {
  requestId: string
  questions: ChatQuestionItem[]
  /** Kimi 的后台 AskUserQuestion：即时回执不是最终答案。 */
  background?: boolean
  keepAfterTurn?: boolean
  localCodexPlanPrompt?: boolean
}
export interface ChatQuestionPayload { chatId: number; request: ChatQuestionRequest }

/** 单个额度窗口（与 Rust usage_api::UsageWindow 同形）。来自 OAuth 用量接口。 */
export interface UsageWindow {
  /** 利用率百分比 0–100。 */
  utilization: number
  /** ISO8601 重置时间（用 `new Date()` 解析）。 */
  resetsAt?: string
}
/** 账号额度快照（与 Rust usage_api::AccountUsage 同形）。 */
export interface AccountUsage {
  fiveHour?: UsageWindow | null
  sevenDay?: UsageWindow | null
  sevenDayOpus?: UsageWindow | null
  sevenDaySonnet?: UsageWindow | null
}

/** Codex 单个额度窗口（与 Rust codex_usage::CodexUsageWindow 同形）。 */
export interface CodexUsageWindow {
  /** 已用百分比 0–100。 */
  usedPercent: number
  /** 窗口长度（分钟）：300 = 5 小时，10080 = 7 天。标签据此选。 */
  windowMinutes: number
  /** ISO8601 重置时间（后端已把 app-server 的 unix 秒转成字符串）。 */
  resetsAt?: string
}
/** Codex 账号额度快照（与 Rust codex_usage::CodexAccountUsage 同形）。 */
export interface CodexAccountUsage {
  primary?: CodexUsageWindow | null
  secondary?: CodexUsageWindow | null
}

/** 设置页「存储占用」的一行。`key` 同时是 `clearStorage` 的入参。 */
export interface StorageUsageEntry {
  key: string
  path: string
  bytes: number
  /** false = 用户资产（背景素材等），只报大小不给清理按钮。 */
  clearable: boolean
}

/** 设置页「运行诊断」：内存 / 线程 / 各缓存与目录占用。 */
export interface RuntimeDiagnostics {
  mainRssBytes: number
  webviewRssBytes: number
  threads: number
  userTextCacheBytes: number
  usageCacheEntries: number
  scanCacheEntries: number
  watchMapEntries: number
  activeChats: number
  desktopTasks: number
  imageCacheBytes: number
  attachmentsBytes: number
  trashBytes: number
}

export interface TrashItem {
  trashFile: string
  agent: Agent
  projectLabel: string
  originalPath: string
  /** 回收站里可读取的 transcript 路径；Grok 指向会话目录内的 updates.jsonl。 */
  trashPath: string
  deletedAt: number
  title: string
  size: number
}

export interface TrayAgentSummary {
  agent: string
  todayTokens: number
  todayCost: number
  todayUnpricedCalls: number
  todayEstimatedCalls: number
  weekTokens: number
  weekCost: number
  weekUnpricedCalls: number
  weekEstimatedCalls: number
  monthTokens: number
  monthCost: number
  monthUnpricedCalls: number
  monthEstimatedCalls: number
  sessionCount: number
}

export interface TrayStats {
  agents: TrayAgentSummary[]
  totalTodayTokens: number
  totalTodayCost: number
  totalTodayUnpricedCalls: number
  totalTodayEstimatedCalls: number
  totalWeekTokens: number
  totalWeekCost: number
  totalWeekUnpricedCalls: number
  totalWeekEstimatedCalls: number
  totalMonthTokens: number
  totalMonthCost: number
  totalMonthUnpricedCalls: number
  totalMonthEstimatedCalls: number
}

// ---- CLI 环境检测 ----

export interface CliVersionInfo {
  cli: 'claude' | 'codex' | 'agy' | 'opencode' | 'grok' | 'kimi' | 'pi'
  npmPackage: string
  currentVersion: string | null
  latestVersion: string | null
  upgradable: boolean
  installed: boolean
  error: string | null
  health: CliHealthStatus | null
}

export interface CliHealthStatus {
  healthy: boolean
  summary: string | null
}

export interface CliInstallation {
  path: string
  version: string | null
  isDefault: boolean
  packageManager: string
  resolvedPath: string | null
}

export interface CliDiagnosisResult {
  cli: 'claude' | 'codex' | 'agy' | 'opencode' | 'grok' | 'kimi' | 'pi'
  binaryName: string
  installations: CliInstallation[]
  hasConflict: boolean
  error: string | null
}

export interface CliUpgradeResult {
  cli: 'claude' | 'codex' | 'agy' | 'opencode' | 'grok' | 'kimi' | 'pi'
  success: boolean
  newVersion: string | null
  error: string | null
}

/**
 * 一个 agent 的「工具面」：四类工具各自支不支持、配置落在哪个文件。
 *
 * 这些都由后端 `tools::ToolSurface` 给出，前端不另存一份 —— 它们是外部世界的事实
 * （某家把 MCP 从 `config.toml` 挪到 `mcp.json` 就变了），存两份必然漂移。
 */
export interface ToolCapabilities {
  mcp: boolean
  skills: boolean
  hooks: boolean
  globalMemo: boolean
}

/**
 * 一个 MCP 配置来源的作用域。项目级的只有传了 cwd 才算得出来。
 *
 * `local` 是 Claude 特有的一档：它和 `user` 同住 `~/.claude.json`，但只对某个项目
 * 生效且优先级高于 `project`。所以那个文件会以两条来源出现，前端按 `path` 分组即可。
 */
export type ConfigScope = 'user' | 'local' | 'project'

/**
 * 这个文件是谁的配置：
 * - `own` —— agent 自有格式
 * - `shared` —— 跨 agent 通用的 `.mcp.json`
 * - `compat` —— 为兼容而扫描的别人家的（grok 默认读 `~/.claude.json` 和 Cursor 的）
 */
export type ConfigOrigin = 'own' | 'shared' | 'compat'

/**
 * 一个 MCP 配置文件的格式。**格式跟着文件走，不跟着 agent 走** —— `.mcp.json` 一个
 * 文件 claude / grok / kimi / pi 四家都读，而 codex 和 grok 的 user 级配置又都是 TOML。
 */
export type McpFormat = 'jsonServers' | 'jsonProjectServers' | 'tomlServers' | 'opencodeJson'

export interface McpSource {
  path: string
  scope: ConfigScope
  origin: ConfigOrigin
  format: McpFormat
  /** 工具管理会不会往这里写。目前只写 agent 自有的 user 级文件。 */
  writable: boolean
  exists: boolean
  /**
   * 合并时的优先级，**数字大的覆盖小的**。同名 server 出现在多个文件里时靠它决定
   * 谁生效。数组已按它从高到低排好，但渲染覆盖关系时请用这个字段而不是数组下标 ——
   * 存在并列（grok 的两个 Cursor 来源）。
   */
  precedence: number
  /**
   * 这个来源是不是**有条件**生效。true 表示它受某个后端判定不了的开关影响
   * （grok 的项目级 `.mcp.json` 取决于 Claude import marker，而那个 marker 在哪儿
   * 没有公开说明），UI 该标成「可能未生效」，不要直接画进覆盖链。
   */
  conditional: boolean
}

export interface ToolSurfaceInfo {
  agent: Agent
  /** 本机装没装 —— 全局配置目录（`~/.claude`、`$CODEX_HOME` …）在不在。 */
  installed: boolean
  capabilities: ToolCapabilities
  /** 会被写入的那个 user 级文件，也就是 `mcpSources` 里 `writable` 的那一条。 */
  mcpConfigPath: string | null
  /** 这个 agent 实际会读到的全部 MCP 来源，按优先级从高到低。 */
  mcpSources: McpSource[]
  skillsDir: string | null
  hooksConfigPath: string | null
  memoPath: string | null
  /** 约定路径之外还会被读进来的指令文件（目前只有 opencode 的 `instructions`）。 */
  memoExtraSources: string[]
  /** 约定路径缺席时实际生效的文件（opencode / grok 会回退到 `~/.claude/CLAUDE.md`）。 */
  memoFallback: string | null
}

// ---------------------------------------------------------------------------
// 工具管理 · MCP（只读扫描）
// 后端 `src-tauri/src/tools/mcp.rs`
// ---------------------------------------------------------------------------

/** 传输方式。`unknown` = 配置里写了个我们不认识的值，不是「没写」。 */
export type McpTransport = 'stdio' | 'http' | 'sse' | 'ws' | 'unknown'

/** 一条环境变量 / HTTP header。 */
export interface McpVar {
  key: string
  value: string
  /** 键名看着像凭据。**为 true 时 UI 必须默认打码**，要看得点一下。 */
  secret: boolean
}

/** 一个 server 在某一个文件里的定义。 */
export interface McpServerDef {
  name: string
  transport: McpTransport
  command: string | null
  args: string[]
  url: string | null
  /**
   * `url` 里能确认是凭据的那几处打了码的样子。**列表和详情页显示这一份。**
   *
   * 托管 MCP 的接入地址常常自带密钥（`https://user:tok@host/…`、`?api_key=…`）。
   * 原文留在 `url` 里给指纹和编辑用，但不默认往屏幕上放。
   */
  urlMasked: string | null
  env: McpVar[]
  headers: McpVar[]
  cwd: string | null
  /** 显式关掉的（`enabled: false` / `disabled: true`）。 */
  enabled: boolean
  /** 既没有 command 也没有 url —— 这条根本起不来，别画成正常的。 */
  incomplete: boolean
}

export interface McpDefAt {
  agent: Agent
  source: McpSource
  def: McpServerDef
  /** 这一份是不是这家 agent 实际生效的那份（同名时只有优先级最高的算数）。 */
  effective: boolean
}

/** 一个 server（按名字归并）在全机器的样子。 */
export interface McpEntry {
  name: string
  defs: McpDefAt[]
  /** 真的会加载它的 agent。列表右侧那排角标就是它。 */
  agents: Agent[]
  /** 配置里有它、但被关掉或被盖住的 agent。 */
  inactiveAgents: Agent[]
  /** 同名但命令行不止一种 —— 各家跑的根本不是同一个东西。 */
  conflict: boolean
  /** 生效的定义里出现过的指纹，去重。冲突时拿它列「有哪几种」。 */
  fingerprints: string[]
  /** 缓存里量到的工具数。`null` = 没量过，**不是 0**。 */
  tools: number | null
  /** 工具定义的 token 估算（4 字符 ≈ 1 token）。 */
  tokens: number | null
}

export interface McpSummary {
  servers: number
  /** 至少在一家 agent 里生效的。 */
  active: number
  conflicts: number
  /** 生效集合的合计；量不到的条目不计入。 */
  tools: number
  tokens: number
  /** 有工具数缓存的条目数 —— 上面两个数字覆盖了多少条。 */
  measured: number
}

export interface McpSourceInfo extends McpSource {
  servers: number
  /** 解析失败的原因。有值就说明这个文件整片没读进来。 */
  error: string | null
}

export interface McpAgentInfo {
  agent: Agent
  installed: boolean
  supported: boolean
  /** 可写的那一条，没有就是这家没法改。 */
  writePath: string | null
  sources: McpSourceInfo[]
}

export interface McpScan {
  home: string
  servers: McpEntry[]
  agents: McpAgentInfo[]
  summary: McpSummary
}

// ---------------------------------------------------------------------------
// 工具管理 · MCP（写：按 agent 同步 / 增删改 / 启停）
// 后端 `src-tauri/src/tools/mcp_write.rs`
// ---------------------------------------------------------------------------

/** 一条写请求要做的事。 */
export type McpOp = 'put' | 'drop' | 'enable' | 'disable'

export interface KeyValue {
  key: string
  value: string
}

/**
 * 用户填的一份定义。
 *
 * 不带 `incomplete` / `enabled` 这类**结论**：那些由后端按形状重算，前端说了不算。
 */
export interface McpServerInput {
  transport: McpTransport
  command: string | null
  args: string[]
  url: string | null
  env: KeyValue[]
  headers: KeyValue[]
  cwd: string | null
}

export interface McpEdit {
  agent: Agent
  name: string
  op: McpOp
  /** `put` 要写的定义；其余操作用不上。 */
  def: McpServerInput | null
}

export type McpStepKind = 'add' | 'update' | 'remove' | 'enable' | 'disable'

/**
 * - `newFile` —— 目标文件还不存在，会连它一起建出来
 * - `shadowed` —— 写进去了，但同一家有优先级更高的来源也定义了它，跑的仍是那一份
 */
export type McpStepNote = 'newFile' | 'shadowed'

export interface McpWriteStep {
  agent: Agent
  name: string
  kind: McpStepKind
  path: string
  /** 改之前那份的命令行（新增时为 null）。 */
  before: string | null
  /** 改之后的命令行（移除时为 null）。 */
  after: string | null
  note: McpStepNote | null
  /** `shadowed` 时是哪个文件盖住了它。 */
  shadowedBy: string | null
  /** dry-run 恒为 false；真跑时表示这一步做成了。 */
  done: boolean
}

/**
 * 做不了的原因。**每一条都要在 UI 上说出来** —— 静默跳过就是「显示成功但什么都
 * 没做」。
 */
export type McpBlockReason =
  | 'unsupported'
  | 'noWritableSource'
  /** 要动的那份在别的文件里（项目配置 / 别家的兼容来源），改可写文件也动不到。 */
  | 'notInWritableSource'
  /** 这个格式没有确认过的停用开关，只能移除。 */
  | 'noEnableSwitch'
  | 'incomplete'
  /** 远端 server 暂不写：各家的 URL 键不一样（agy 用 `httpUrl`）。 */
  | 'remoteReadOnly'
  | 'headersUnsupported'

export interface McpBlocked {
  agent: Agent
  name: string
  reason: McpBlockReason
  path: string | null
}

/** 一个目标文件在排计划那一刻的样子。前端只负责原样带回去，不解读 `stamp`。 */
export interface McpFileStamp {
  path: string
  stamp: string
}

/** `stale` = 确认期间这个文件被别处改过了；`write` = 写的时候出错了。 */
export type McpFailKind = 'stale' | 'write'

export interface McpFailure {
  kind: McpFailKind
  path: string
  /** `write` 时的底层报错原文。 */
  detail: string | null
}

export interface McpWriteReport {
  dryRun: boolean
  steps: McpWriteStep[]
  blocked: McpBlocked[]
  /** 这份计划要碰的每个文件的当时样子。点确认时原样回传，对不上后端会拒。 */
  stamps: McpFileStamp[]
  /**
   * 真跑中途停下来的原因。
   *
   * **`steps` 里 `done` 为真的那几步已经落盘了。** 只说一句「失败了」的话，用户不
   * 知道自己现在有几份配置已经被改了。
   */
  failed: McpFailure | null
}

// ---------------------------------------------------------------------------
// 工具管理 · Skills（只读扫描）
// 后端 `src-tauri/src/tools/skills.rs` / `tools/risk.rs`
// ---------------------------------------------------------------------------

/** 风险等级。顺序有意义，取最高用 `RISK_ORDER`。 */
export type RiskLevel = 'none' | 'low' | 'medium' | 'high' | 'critical'

/**
 * 命中点在什么上下文里，决定了降几级。
 * SKILL.md 正文里拿 `rm -rf /` 当反面例子太常见，不降权的话说明文档全是 Critical。
 */
export type RiskContext = 'executable' | 'comment' | 'codeBlock' | 'prose'

export interface RiskFinding {
  rule: string
  /** 规则本身的等级，**降级前**。UI 要能解释「为什么它不是 Critical」。 */
  baseLevel: RiskLevel
  /** 降级后的实际等级，角标用这个。 */
  level: RiskLevel
  context: RiskContext
  /** 相对 skill 目录的路径。 */
  file: string
  line: number
  excerpt: string
}

/**
 * 扫描时能判定的条目状态。
 * 和后端 `link.rs` 的 `LinkHealth` 不是一回事 —— 那个要先知道「应该指向哪」，
 * 而扫描发生在用户指定主 store 之前。
 */
export type RefHealth =
  | { state: 'realDir' }
  | { state: 'linked' }
  /** `detail` 是链上第一个不存在的路径。 */
  | { state: 'broken'; detail: string }
  | { state: 'cyclic' }
  /** 受管副本，且和源一致。`detail` 是副本记录的源。 */
  | { state: 'managedCopy'; detail: string }
  /**
   * 受管副本，但**源**已经变了 —— 这个 agent 读到的是旧版本。`detail` 是源。
   *
   * 少了这一态就是 Skills-Manager 的那个坑：降级成拷贝之后源改了，界面一路显示健康。
   */
  | { state: 'copyStale'; detail: string }
  /** 受管副本，**副本**被就地改过。重拷会抹掉这些修改，所以不给一键。 */
  | { state: 'copyEdited'; detail: string }
  /** 受管副本，源和副本**都**变了。自动挑一边必然吃掉另一边。 */
  | { state: 'copyDiverged'; detail: string }
  /** 受管副本，但它记录的源已经没了。 */
  | { state: 'copyOrphaned'; detail: string }
  /**
   * 链接解析到了东西，但那不是目录 —— 没有任何内容可用。
   * 只判断「存在」会把它报成健康，用户看到一个「好端端却用不了」的 skill。
   */
  | { state: 'notADirectory'; detail: string }

export interface LinkHop {
  from: string
  to: string
  exists: boolean
}

export interface SkillRef {
  path: string
  store: string
  /**
   * 会读到**这条引用所在 store** 的所有 agent。同一个目录常常不止一家读（grok 的 compat）。
   *
   * 「这家 agent 从哪个入口够到这个 skill」问的是它（见 `agentReach`）。
   * 「这份内容还有没有人在读」问的是 `reachedBy`。
   */
  agents: Agent[]
  /**
   * 谁真的能读到这条引用背后的内容 —— 直接读，或者顺着别人的链走到这儿。
   *
   * 实体目录尤其需要它：`~/.skills-manager/skills/X` 没有任何 agent 直接读
   * `.skills-manager`，`agents` 是空的，但两条链的终点都在它身上。按 `agents` 判
   * 「没人读它、可以删」会让用户亲手打断那两条链。
   */
  reachedBy: Agent[]
  health: RefHealth
  /** 完整链路，实体目录为空数组。两跳链必须整条画出来。 */
  hops: LinkHop[]
  resolved: string | null
}

export interface SkillBody {
  path: string
  store: string | null
  files: number
  bytes: number
  /** 毫秒。 */
  modified: number | null
  risk: RiskLevel
  /**
   * 这份内容**没被完整扫完**（文件数/深度触顶，或有文件太大、读不了、是二进制）。
   * `risk` 因此只是下限：危险脚本可能就在没扫到的那部分里。UI 必须说「至少」而不是「就是」。
   */
  truncated: boolean
}

export type SkillBadge = 'duplicate' | 'twoHop' | 'cyclic' | 'broken' | 'copyStale'

export interface SkillEntry {
  name: string
  refs: SkillRef[]
  /** 多于一份就是「同名重复」—— 改哪份生效全看链接指向谁。 */
  bodies: SkillBody[]
  badges: SkillBadge[]
  risk: RiskLevel
  /** 任一 body 没扫完。`risk` 只是下限，不是结论。 */
  truncated: boolean
  description: string | null
  /** 主 body 是个带 remote 的 clone 时给出它的来源；否则 null。 */
  git: SkillGit | null
}

export interface StoreCandidate {
  path: string
  /** 会读这个目录的**所有** agent。第三方 store（`~/.agents` 等）为空数组。 */
  agents: Agent[]
  scope: ConfigScope
  origin: ConfigOrigin
  exists: boolean
  total: number
  links: number
  /** 只有实体目录才是内容，主 store 该从这一栏多的里面挑。 */
  realDirs: number
  broken: number
  /**
   * 够不够格当主 store。判定在后端（`can_be_main`）：只有用户级的跨 agent 共享目录
   * 够格 —— 项目目录跟着仓库走，agent 自有目录是链接落脚的地方，两类都不能装内容。
   */
  canBeMain: boolean
}

export interface ScanSummary {
  total: number
  broken: number
  twoHop: number
  duplicate: number
  cyclic: number
  /** 受管副本和源对不上的有几个。非 Windows 上恒为 0（那儿根本不会降级到复制）。 */
  copyStale: number
  /** 从远端 clone 来的有几个 —— 只有这些还能拉到新版本。 */
  fromGit: number
}

export interface SkillScan {
  /** 用户 home，用来把绝对路径缩写成 `~/…`。 */
  home: string
  /** 一个候选都挑不出来时的兜底主 store（`~/.agents/skills`）。 */
  defaultMain: string
  stores: StoreCandidate[]
  /** 建议的主 store：够格的候选里实体目录最多的那个；一个都没有就是 `defaultMain`。 */
  suggestedMain: string
  skills: SkillEntry[]
  summary: ScanSummary
}

export interface FrontmatterField {
  key: string
  value: string
}

export interface SkillFrontmatter {
  name: string | null
  description: string | null
  allowedTools: string[]
  /** 认不出来的字段原样保留，不猜也不丢。 */
  extra: FrontmatterField[]
}

export interface SkillFile {
  /** 相对 skill 目录。 */
  path: string
  bytes: number
}

/** 一个文件的版本标识。保存时原样回传，后端用它挡「外部改过了还盲写」。 */
export interface FileRev {
  exists: boolean
  bytes: number
  modifiedMs: number | null
}

/** 编辑器读到的一个文件。后端 `tools/files.rs`。 */
export interface SkillFileText {
  rel: string
  /** `binary` 或 `truncated` 时这只是**部分内容**，不能拿去保存。 */
  text: string
  bytes: number
  /** 含 NUL 字节，不给编辑。 */
  binary: boolean
  /** 超过后端的单文件上限，只读到开头一段。 */
  truncated: boolean
  rev: FileRev
}

export interface SkillFileList {
  files: SkillFile[]
  /** 文件太多没列完。 */
  truncated: boolean
}

export interface SkillDetail {
  name: string
  refs: SkillRef[]
  bodies: SkillBody[]
  /** 被引用最多的那份 body；文件清单和 frontmatter 都取自它。 */
  primary: string | null
  frontmatter: SkillFrontmatter | null
  files: SkillFile[]
  findings: RiskFinding[]
  risk: RiskLevel
  /** 主 body 没扫完，`risk` / `files` 都不完整。 */
  truncated: boolean
  /** 主 body 是个能从远端更新的 clone 时给出它的 remote；否则 null。 */
  git: SkillGit | null
}

/** 一份从远端 clone 下来的内容。后端 `tools/skills_git.rs`。 */
export interface SkillGit {
  /** origin 的 URL，原样显示 —— 弹框里要说清楚「从哪儿拉」。 */
  remote: string
  branch: string
}

/** `git fetch` 之后的对比结果，只用来填那个二次确认框。 */
export interface SkillUpdateCheck {
  remote: string
  branch: string
  /** 本地 HEAD 的短 sha。 */
  local: string
  /** 远端最新的短 sha。 */
  latest: string
  /** 落后几个提交。0 表示已经是最新的。 */
  behind: number
  /** 会被 `reset --hard` 冲掉的、改过的已跟踪文件。 */
  changed: string[]
  /** 没被 git 跟踪的文件个数 —— 这些不会被动。 */
  untracked: number
}

// ---------------------------------------------------------------------------
// 工具管理 · Skills（写：收编 / 启停 / 删除 / 修复）
// 后端 `src-tauri/src/tools/skills_write.rs`
// ---------------------------------------------------------------------------

/** 一步写操作的种类。`deleteDir` 不可逆，后端保证它永远排在最后。 */
export type StepKind = 'ensureDir' | 'move' | 'backup' | 'link' | 'unlink' | 'deleteDir'

/** 步骤旁边那一句补充说明。走 code 是为了让四种语言各自出文案。 */
export type StepNote = 'deadLink'

export interface WriteStep {
  kind: StepKind
  path: string
  /** 链接指向谁 / 移动到哪。 */
  target: string | null
  note: StepNote | null
  /** dry-run 恒为 false；真跑时表示这一步做成了。 */
  done: boolean
}

export interface WriteReport {
  dryRun: boolean
  steps: WriteStep[]
  /** 需要三选一的同名冲突。**带冲突的条目一步都没做**，不是静默跳过。 */
  conflicts: AdoptConflict[]
}

/** 同名冲突的三选一。内容一致时后端不会问，直接按 `keepMain` 合并。 */
export type Resolution =
  | { kind: 'keepMain' }
  | { kind: 'useExternal' }
  /** `value` 是外部那份进主 store 时用的新名字。 */
  | { kind: 'keepBoth'; value: string }

export interface AdoptRequest {
  name: string
  /** 要收编的实体目录。 */
  body: string
  /** 冲突的处置；null = 还没选，后端会把它报回冲突列表。 */
  resolution: Resolution | null
}

export interface ConflictSide {
  path: string
  files: number
  bytes: number
  /** 毫秒。 */
  modified: number | null
  /** 这一侧没看全，所以**不能**判定两边一致。 */
  truncated: boolean
}

export type FileStatus = 'onlyMain' | 'onlyExternal' | 'same' | 'different'

export interface FileDiff {
  path: string
  status: FileStatus
  mainBytes: number | null
  externalBytes: number | null
}

export interface LineDiff {
  plus: number
  minus: number
  /** 行数超限没逐行比。0 **不表示**没差异。 */
  truncated: boolean
  /** 逐行差异（带上下文），直接喂 `DiffBlock.vue`。空 = 两边一模一样。 */
  hunks: DiffHunk[]
  /** `hunks` 被砍短了，后面还有没显示的。 */
  clipped: boolean
}

export interface AdoptConflict {
  name: string
  main: ConflictSide
  external: ConflictSide
  files: FileDiff[]
  skillMd: LineDiff | null
  /** 「都留着」时建议的新名字，已避开主 store 里已有的。 */
  suggestedRename: string
  /**
   * A 那一侧是不是**已经**在主 store 里。
   *
   * 一次收编里同名的第二份，比的是这一批的第一份 —— 它此刻还在别的 store 里，只是
   * 这一批结束之后会成为主 store 那份。false 时冲突框换 A 的标签。
   */
  mainInStore: boolean
}

export interface DeleteOptions {
  /** 只解链，保留实体目录。 */
  keepBodies: boolean
}

/** 改指向某个实体目录（两跳压一跳 / 死链接回），或直接删掉死链。 */
export type RepairAction = { kind: 'relink'; value: string } | { kind: 'unlink' }

export interface RepairRequest {
  path: string
  action: RepairAction
}

// ---------------------------------------------------------------------------
// 工具管理 · Hooks（扫描 + 写）
// 后端 `src-tauri/src/tools/hooks.rs` / `tools/hooks_write.rs`
// ---------------------------------------------------------------------------

/**
 * 一个 hook 配置文件的格式。和 `McpFormat` 一样**跟着文件走，不跟着 agent 走** ——
 * codex 一家就同时有 `hooks.json`（groupedJson）和 `config.toml`（tomlGrouped）。
 */
export type HookFormat = 'groupedJson' | 'tomlGrouped' | 'tomlList' | 'agyJson'

/**
 * 一个 agent 实际会读到的一个 hook 配置文件。
 *
 * **没有 `precedence`**：hook 是叠加语义不是覆盖语义 —— user 级配一条、项目里再配一条，
 * 两条都会跑。照搬 MCP 那套优先级会画出一个不存在的「被覆盖」关系。
 */
export interface HookSource {
  path: string
  scope: ConfigScope
  origin: ConfigOrigin
  format: HookFormat
  writable: boolean
  exists: boolean
  /** 受某个后端判定不了的开关影响（codex 的项目配置要过信任闸）。 */
  conditional: boolean
}

/** 一条 hook 在某一个文件里的定义。 */
export interface HookDef {
  event: string
  /** 匹配器（工具名 / 通知类型）。null = 这个事件全都匹配。 */
  matcher: string | null
  /** agy 比别家多的那一层：它的根上是一个个**有名字的** hook。别家恒为 null。 */
  group: string | null
  /** `command` / `prompt` / …。认不出来的原样保留。 */
  kind: string
  command: string
  /** 秒。**各家单位不统一，后端原样报不换算** —— 猜单位比不显示更糟。 */
  timeout: number | null
  enabled: boolean
}

/** 一条定义连同它来自哪儿。 */
export interface HookAt {
  agent: Agent
  source: HookSource
  def: HookDef
}

/** 按命令归并之后的一条 hook。列表一行就是一条。 */
export interface HookEntry {
  /** 命令指纹（去空白后的命令），同时当列表的 key。 */
  fingerprint: string
  /** 原样的命令。列表标题就是它 —— 不从命令里猜名字。 */
  command: string
  hooks: HookAt[]
  agents: Agent[]
  /** 它挂在哪些事件上，去重后按首次出现顺序。 */
  events: string[]
  /** 本 app 自己装的回合信号：**不可删不可改**。 */
  managed: boolean
  /** 至少有一条没被关掉。 */
  enabled: boolean
}

/** 一个事件在各家的支持情况。「添加 hook」的事件列表照它渲染。 */
export interface HookEventInfo {
  name: string
  /** 支持它的 agent。 */
  agents: Agent[]
  /** 已经配了几条。 */
  configured: number
}

export interface HookSourceInfo extends HookSource {
  hooks: number
  error: string | null
}

export interface HookAgentInfo {
  agent: Agent
  installed: boolean
  supported: boolean
  writePath: string | null
  events: string[]
  sources: HookSourceInfo[]
}

export interface HookSummary {
  /** 列表上的行数（归并之后）。 */
  hooks: number
  /**
   * 配置文件里的条数（归并之前）。两个数差很多是正常的 —— 一条命令常常挂在好几家的
   * 好几个事件上。
   */
  defs: number
  managed: number
  disabled: number
  /** 配了 hook 的事件数。 */
  events: number
}

export interface HookScan {
  home: string
  hooks: HookEntry[]
  agents: HookAgentInfo[]
  /** 全部已实证事件的并集，附各家支持情况。 */
  events: HookEventInfo[]
  summary: HookSummary
}

export type HookOp = 'add' | 'remove'

export interface HookEdit {
  agent: Agent
  op: HookOp
  event: string
  matcher: string | null
  command: string
  /** 秒。原样写，不替用户换算。 */
  timeout: number | null
}

export type HookStepKind = 'add' | 'remove'

export interface HookWriteStep {
  agent: Agent
  kind: HookStepKind
  path: string
  event: string
  matcher: string | null
  command: string
  /** 目标文件还不存在，会连它一起建出来。 */
  newFile: boolean
  /** dry-run 恒为 false；真跑时表示这一步做成了。 */
  done: boolean
}

/** 做不了的原因。**每一条都要在 UI 上说出来** —— 静默跳过就是「显示成功但没做」。 */
export type HookBlockReason =
  | 'unsupported'
  | 'noWritableSource'
  /** 回合信号，不许在这儿动 —— 删掉 GUI 聊天就收不到回合结束事件。 */
  | 'protected'
  /** 这家不认这个事件：写得进去，但永远不会触发。 */
  | 'unknownEvent'
  | 'notInWritableSource'
  | 'emptyCommand'
  /** 可写文件里已经有一条一模一样的。再加一条就是同一个 hook 挂两遍，每次触发两次。 */
  | 'alreadyThere'

export interface HookBlocked {
  agent: Agent
  event: string
  reason: HookBlockReason
  path: string | null
}

export interface HookWriteReport {
  dryRun: boolean
  steps: HookWriteStep[]
  blocked: HookBlocked[]
}

/** 试跑一条 hook 的结果。 */
export interface HookTestResult {
  /** 实际喂给它的那份 JSON。**一定要给用户看** —— 写不对多半是字段长得和想的不一样。 */
  payload: string
  stdout: string
  stderr: string
  /** 进程退出码。被信号打断（含超时杀掉）时为 null。 */
  exitCode: number | null
  durationMs: number
  /** 撞上墙钟上限被杀掉的。 */
  timedOut: boolean
  /** 退出码 2：各家都拿它当「拦住这一步」。 */
  blocking: boolean
}

// ---------------------------------------------------------------------------
// 工具管理 · 全局配置（后端 `src-tauri/src/tools/memo.rs`）
// ---------------------------------------------------------------------------

/**
 * 一个文件在某个 agent 眼里的身份。
 * - `own` —— 这家自己的约定路径
 * - `fallback` —— 自己那份缺席时才会读到的
 * - `extra` —— 配置里显式追加的（opencode 的 `instructions`）
 */
export type MemoRole = 'own' | 'fallback' | 'extra'

export interface MemoReader {
  agent: Agent
  role: MemoRole
  /** 这条链路**现在**是不是真的生效。回退目标在自家文件存在时就不生效。 */
  active: boolean
}

/** 一行 `@…`。 */
export interface MemoImport {
  /** 原样那一行。 */
  raw: string
  line: number
  /** 解析出来的绝对路径。解析不出来为 null。 */
  path: string | null
  exists: boolean
  bytes: number
  /** 这一层引用的文件自己还有几行 `@…`。**不展开**，只报数。 */
  nested: number
}

/**
 * 文件指纹，给「保存时外部改动检测」用。
 *
 * 只到毫秒 —— 纳秒精度在前后端之间来回一趟必然掉精度，反过来让每次保存都误判成冲突。
 */
export interface MemoRevision {
  exists: boolean
  size: number
  mtimeMs: number | null
}

export interface MemoFile {
  path: string
  /** 文件名。分叉检测按它分组。 */
  name: string
  exists: boolean
  bytes: number
  revision: MemoRevision
  /** 谁会读到它。空数组是可能的：被 import 进来的片段靠引用它的那个文件生效。 */
  readers: MemoReader[]
  /** 被谁 import 进来的。顶层文件为 null。 */
  importedBy: string | null
  /**
   * 它是条链接时指向哪儿（原样的 target，不解析到底）。实体文件为 null。
   *
   * 合并之后 `~/.claude/RTK.md` 这类位置就是链接了 —— UI 得能把「这儿有一份内容」
   * 和「这儿只是指过去」分开画，否则合并完看上去和没合一样。
   */
  link: string | null
  imports: MemoImport[]
  /** 读不了（权限 / 太大 / 不是普通文件）。 */
  error: string | null
}

export interface MemoAgentInfo {
  agent: Agent
  installed: boolean
  /** 有没有 home 级约定。false 的那几家在 UI 上是禁用态。 */
  supported: boolean
  path: string | null
  exists: boolean
  fallback: string | null
  /** 实际生效的那个文件。自己那份在就是自己的，不在就是回退目标，都没有为 null。 */
  effective: string | null
  fallenBack: boolean
  extra: string[]
}

export interface MemoForkSide {
  path: string
  bytes: number
}

/** 一处分叉：同名、内容不一样。 */
export interface MemoFork {
  name: string
  sides: MemoForkSide[]
}

/**
 * 一处重复：同名、内容**一模一样**、而且磁盘上真的是好几份。
 *
 * 和 `MemoFork` 是同一枚硬币的两面 —— 分叉是「同名但内容分家了」（只提示），
 * 重复是「同名而且还没分家」（可以合并掉）。已经合并过的不算重复：几条链接指向
 * 同一个物理文件时内容当然还是全相同，但那正是终态。
 */
export interface MemoDup {
  name: string
  /** 每份都一样大（内容相同），所以只有一个数。 */
  bytes: number
  /** 涉及的位置，含已经是链接的那些。 */
  paths: string[]
  /** 其中还是实体文件的那几个 —— 合并真正要动的就是它们。 */
  bodies: string[]
}

export interface MemoSummary {
  files: number
  present: number
  /** 断掉的 `@import`。 */
  broken: number
  forks: number
  /** 有几组同名同内容的重复。 */
  dups: number
}

/** 合并计划里一步的种类。后端 `tools/memo_merge.rs`。 */
export type MemoStepKind = 'ensureDir' | 'move' | 'link' | 'unlink' | 'drop'

export interface MemoStep {
  kind: MemoStepKind
  path: string
  /** `move` / `link` 的另一头。 */
  target: string | null
  done: boolean
  /**
   * 这一步属于哪一组重复（文件名）。
   *
   * 「合并全部重复」一次能出二十来步，平铺开来是一堵墙 —— 计划弹框按它在组之间
   * 画分割线。
   */
  group: string
}

export interface MemoMergeReport {
  dryRun: boolean
  steps: MemoStep[]
  /** 做不了的那几组，每条一句完整的话。带原因的那几组一步都没做。 */
  blocked: string[]
}

export interface MemoScan {
  home: string
  agents: MemoAgentInfo[]
  files: MemoFile[]
  forks: MemoFork[]
  dups: MemoDup[]
  summary: MemoSummary
}

export interface MemoDoc {
  path: string
  text: string
  revision: MemoRevision
}

export interface MemoDiff {
  left: string
  right: string
  hunks: DiffHunk[]
  /** 两边逐字节一样。 */
  same: boolean
  /** 太大，没逐行比。 */
  truncated: boolean
  clipped: boolean
}

// ---------------------------------------------------------------------------
// 配置集（方案 阶段 10）
// 后端 `src-tauri/src/tools/bundle.rs`
// ---------------------------------------------------------------------------

export interface BundleInclude {
  mcp: boolean
  hooks: boolean
  memo: boolean
  skills: boolean
}

export interface BundleMcp {
  name: string
  transport: McpTransport
  command: string | null
  args: string[]
  /** **只有键名**，值一个都不带。导入端自己补。 */
  envKeys: string[]
  headerKeys: string[]
  url: string | null
  /** 导出那台机器上的工作目录。导入端多半要改。 */
  cwd: string | null
  agents: string[]
}

export interface BundleHook {
  command: string
  events: string[]
  matcher: string | null
  timeout: number | null
  agents: string[]
}

/** 一份全局指令。按**角色**记，不按路径 —— 导入端的用户名都不一样。 */
export interface BundleMemo {
  /** 这是哪家的约定文件。`null` = 被 `@` 引用进来的片段。 */
  agent: string | null
  /** 片段是被哪家 `@` 进来的 —— 导入端照它决定往哪个目录写。顶层文件为 null。 */
  parent: string | null
  name: string
  text: string
}

/** 一条 skill **清单**，不含内容。别人的 skill 就是别人写的代码，不一键铺开。 */
export interface BundleSkill {
  name: string
  remote: string | null
  agents: string[]
}

export interface Bundle {
  kind: string
  version: number
  createdAt: number
  app: string
  mcp: BundleMcp[]
  hooks: BundleHook[]
  memo: BundleMemo[]
  skills: BundleSkill[]
  /** 哪些位置的值被抹掉了，逐条列出来（`github.env.GITHUB_TOKEN` 这样）。 */
  redacted: string[]
}

// ---------------------------------------------------------------------------
// 发现面板（skills.sh）
// ---------------------------------------------------------------------------

/**
 * 一条搜索结果。
 *
 * 接口只给五个字段，**没有描述** —— 所以列表第二行放的是 `source` 而不是描述。
 * 这是数据决定的，不是设计偏好（方案文档 1.1）。
 */
export interface RegistryHit {
  /** `emilkowalski/skills`（GitHub 仓库）或 `code.deepline.com`（厂商自托管）。 */
  source: string
  /** 目录名、安装名。**永远用它**，`name` 只用来显示（两者可能不一样）。 */
  skillId: string
  name: string
  installs: number
  /** `source` 是 `owner/repo` 形态才装得了；域名源只能在浏览器里打开。 */
  installable: boolean
}

export interface RegistrySearch {
  /** 接口回显的查询词。判断响应过不过期看它，不看我们发出去的那个。 */
  query: string
  /** `fuzzy` / `semantic`。多词会切到语义搜索，原样显示出来省得用户猜。 */
  searchType: string
  hits: RegistryHit[]
}

/** `offline` 断网 · `tooShort` 查询词太短 · `http` 对方 4xx/5xx · `badJson` 形状不认识。 */
export type RegistryErrKind = 'offline' | 'tooShort' | 'http' | 'badJson'

export interface RegistryError {
  kind: RegistryErrKind
  /** 底层原文。界面默认只显示按 `kind` 翻出来的那句话，这个留给「展开详情」。 */
  detail: string
}

/**
 * 装之前能看清楚的全部东西。
 *
 * `frontmatter` / `files` / `findings` / `risk` / `truncated` **全是本地 Skills 详情
 * 用的那几个类型** —— 后端那边也是同一份 `describe_body`。两处对「一个 skill 目录
 * 长什么样」的定义必须是同一个，否则装之前那一屏等于没答。
 */
export interface RegistryPreview {
  source: string
  skillId: string
  /** 仓库内路径，例如 `skills/prototype`。布局千奇百怪，所以这条要显示出来。 */
  repoPath: string
  /** 同名目录出现在多处时没被选中的那些。只有一条命中时是空的。 */
  otherPaths: string[]
  /** HEAD 的短 sha，只用来显示。 */
  commit: string
  /** 这个子目录的 tree sha。阶段 D 的「有没有更新」比的是它，不是 `commit`。 */
  treeSha: string
  frontmatter: SkillFrontmatter | null
  files: SkillFile[]
  findings: RiskFinding[]
  risk: RiskLevel
  /** 文件没列全（数量 / 深度触顶，或有文件扫不动）。 */
  truncated: boolean
  /** 这一次省掉了 clone。耗时差两个数量级。 */
  cached: boolean
}

/**
 * `notInstallable` 域名源 · `cache` 缓存目录不可用 · `clone` 拉不下来（断网 / 仓库
 * 不存在 / 私有）· `notFound` 仓库里没有这个 skill · `checkout` 找到了却检出不了。
 */
export type PreviewErrKind = 'notInstallable' | 'cache' | 'clone' | 'notFound' | 'checkout'

export interface PreviewError {
  kind: PreviewErrKind
  detail: string
}
