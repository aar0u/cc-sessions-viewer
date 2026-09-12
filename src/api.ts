import { invoke } from '@tauri-apps/api/core'
import type {
  AccountUsage,
  Agent,
  AgentStats,
  ChatImageInput,
  ChatTextElement,
  ClaudeRuntimeInfo,
  CodexRuntimeInfo,
  ChatStartInfo,
  RunningChatInfo,
  ReclaudeInfo,
  SlashCommand,
  ProjectFileEntry,
  ProjectInfo,
  SessionPage,
  Msg,
  StatsRange,
  StatsScope,
  RuntimeDiagnostics,
  ToolSurfaceInfo,
  StorageUsageEntry,
  TrashItem,
  TrayStats,
  SearchHit,
  UsageSummary,
  DiffHunk,
  GitCommit,
  GitFileStatus,
  GitDiffFile,
  GitRepositoryState,
  McpScan,
  McpEdit,
  McpFileStamp,
  McpWriteReport,
  HookScan,
  HookEdit,
  HookWriteReport,
  HookTestResult,
  MemoScan,
  MemoMergeReport,
  MemoDoc,
  MemoDiff,
  MemoRevision,
  PiTreeNode,
} from './types'

export interface BackgroundMedia {
  id: string
  name: string
  path: string
}

export interface BackgroundMediaExport {
  count: number
  directory: string
}

export interface CodexVisibilityOptions {
  includeCodexInternal?: boolean
  includeCodexArchived?: boolean
}

export const listProjects = (
  agent: Agent,
  options: CodexVisibilityOptions = {},
) =>
  invoke<ProjectInfo[]>('list_projects', {
    agent,
    includeCodexInternal: options.includeCodexInternal ?? false,
    includeCodexArchived: options.includeCodexArchived ?? false,
  })

/** 把原生窗口外观（标题栏 / 失焦红绿灯灰圈）钉到 App 主题。null = 跟随系统。 */
export const setTitlebarTheme = (theme: 'dark' | 'light' | null) =>
  invoke<void>('set_titlebar_theme', { theme })

export const windowHideToTray = () => invoke<void>('window_hide_to_tray')
export const windowExitApp = () => invoke<void>('window_exit_app')
export const cleanupRuntimeChildren = () => invoke<void>('cleanup_runtime_children')
/** 刷新 webview 时只清理遗留的内嵌终端，不影响可重连的 GUI Chat。 */
export const cleanupPtyChildren = () => invoke<void>('cleanup_pty_children')
/** 清除实时模型价格缓存，并在后端后台重新拉取最新价格。 */
export const resetPricingCache = () => invoke<void>('reset_pricing_cache')

/** 已导入应用数据目录的图片 / MP4 背景素材。 */
export const backgroundMediaDirectory = () => invoke<string>('background_media_directory')
export const listBackgroundMedia = () => invoke<BackgroundMedia[]>('list_background_media')
export const exportBackgroundMedia = (destinationPath: string) =>
  invoke<BackgroundMediaExport>('export_background_media', { destinationPath })
export const importBackgroundMedia = (sourcePath: string) =>
  invoke<BackgroundMedia>('import_background_media', { sourcePath })
export const deleteBackgroundMedia = (id: string) =>
  invoke<void>('delete_background_media', { id })

export const dataDirectory = () => invoke<string>('data_directory')
export const changeDataDirectory = (path: string) =>
  invoke<string>('change_data_directory', { newPath: path })
export const resetDataDirectory = () => invoke<string>('reset_data_directory')

/** 设置页「存储占用」：逐项大小 + 清理。 */
export const storageUsage = () => invoke<StorageUsageEntry[]>('storage_usage')
export const clearStorage = (key: string) => invoke<number>('clear_storage', { key })

/** 回收站保留期（天）。0 = 永久保留。 */
export const trashRetention = () => invoke<number>('trash_retention')
/** 该不该弹「回收站现在会自动清理」这一次提示；返回会被删掉的条数，0 = 不弹。 */
export const trashRetentionNotice = () => invoke<number>('trash_retention_notice')

/** 用户看过提示了：放行自动清理，并立刻补清一次。 */
export const ackTrashRetention = () => invoke<void>('ack_trash_retention')

export const setTrashRetention = (days: number) =>
  invoke<number>('set_trash_retention', { days })

/** 运行时自检：内存 / 线程 / 各缓存占用。 */
export const runtimeDiagnostics = () => invoke<RuntimeDiagnostics>('runtime_diagnostics')

/**
 * 七家 agent 的工具面快照（能力位 + 配置落点）。工具管理浮层打开时拉一次。
 *
 * `cwd` 给了才能算出项目级的 MCP 来源（`.mcp.json`、grok 的 `.grok/config.toml`、
 * kimi 的 `.kimi-code/mcp.json`）；不给就只报 user 级的。
 */
export const toolSurfaces = (cwd?: string) => invoke<ToolSurfaceInfo[]>('tool_surfaces', { cwd: cwd ?? null })

export const addBookmark = (agent: Agent, path: string) =>
  invoke<void>('add_bookmark', { agent, path })

export const removeBookmark = (agent: Agent, path: string) =>
  invoke<void>('remove_bookmark', { agent, path })

/** 在 `projectPath` 下新建 git worktree（同名新分支），落到
 *  `<projectPath>/.claude/worktrees/<name>`。返回新 worktree 的绝对路径。 */
export const createWorktree = (projectPath: string, name: string) =>
  invoke<string>('create_worktree', { projectPath, name })

/** 全部删除 `path` 处的 worktree（工作树 + 分支，不可撤销）。
 *  其会话记录需调用方先软删到回收站。 */
export const removeWorktree = (path: string) =>
  invoke<void>('remove_worktree', { path })

export const cleanupWorktreeProjectDirs = (worktreePath: string) =>
  invoke<void>('cleanup_worktree_project_dirs', { worktreePath })

export const listSessions = (
  agent: Agent,
  projectKey: string,
  offset: number,
  limit: number,
  options: CodexVisibilityOptions = {},
) =>
  invoke<SessionPage>('list_sessions', {
    agent,
    projectKey,
    offset,
    limit,
    includeCodexInternal: options.includeCodexInternal ?? false,
    includeCodexArchived: options.includeCodexArchived ?? false,
  })

export const readSession = (agent: Agent, path: string, leafId?: string) =>
  invoke<Msg[]>('read_session', { agent, path, leafId })

export const sessionTree = (agent: Agent, path: string) =>
  invoke<PiTreeNode[]>('session_tree', { agent, path })
export const sessionExportJson = (agent: Agent, path: string, leafId?: string | null) =>
  invoke<string>('session_export_json', { agent, path, leafId })

/** 单个会话的 token 用量。
 *  后端按 (path, mtime) 缓存，重复调用不会重复扫描文件。 */
export const sessionUsage = (agent: Agent, path: string) =>
  invoke<UsageSummary>('session_usage', { agent, path })

export const sessionLastPrompt = (agent: Agent, path: string) =>
  invoke<string | null>('session_last_prompt', { agent, path })

/** 续聊种子：会话最后一条 usage（≈当前上下文规模），区别于 sessionUsage 的累加。 */
export const sessionContextUsage = (agent: Agent, path: string) =>
  invoke<UsageSummary>('session_context_usage', { agent, path })

/** 当前 agent 的统计概览。**兼容入口**，前端 stats 页面默认走 `startAgentStats` 流式
 *  接口；这里保留仅作老回退。 */
export const agentStats = (agent: Agent) =>
  invoke<AgentStats>('agent_stats', { agent })

/** 流式启动一次统计扫描；函数立刻返回。Worker 通过 `stats://progress` / `stats://done` /
 *  `stats://error` 事件 emit 结果，前端用 `useStatsStream` 监听。
 *  `scope`：'all' | 'claude' | 'codex' | `session:<agent>:<absolutePath>`。
 *  `range`：'today' | 'days7' | 'days30' | 'month' | 'months3' | 'months6' |
 *  `custom:YYYY-MM-DD:YYYY-MM-DD`
 *  （session-scope 时被忽略）。 */
export const startAgentStats = (
  scope: StatsScope | string,
  range: StatsRange,
  requestId: number,
) => invoke<void>('start_agent_stats', { scope, range, requestId })

/** 立刻取消任何在跑的统计 worker。bump 后端代际计数器 —— 老的 worker 自己 bail。 */
export const cancelStats = () => invoke<void>('cancel_stats')

/** 单调递增的 stats 请求 id 工厂。每次 startAgentStats 前取一个。 */
let _nextStatsId = 0
export function nextStatsRequestId(): number {
  _nextStatsId += 1
  return _nextStatsId
}

/** 跨当前 agent 的项目 / 会话搜索；空字符串返回空数组。
 *  `requestId` 单调递增；后端在循环中比对，更新换代时立刻 bail —— 真正可中断的搜索。
 *  `projectKey` 可选 —— 给会话列表搜索用：只搜当前项目，省掉全局扫描。
 *  `scope` 可选 —— `'id'` 只匹配会话 ID；`'keyword'` 匹配标题 + 用户消息正文；
 *  缺省时全量匹配。
 *  实际写：每次新调用前先 `cancelSearch()`，让 CPU 让位给打字。 */
export const searchSessions = (
  agent: Agent,
  query: string,
  requestId: number,
  projectKey?: string,
  scope?: 'id' | 'keyword',
) =>
  invoke<SearchHit[]>('search_sessions', {
    agent,
    query,
    requestId,
    projectKey,
    scope,
  })

/** 立刻取消任何正在跑的全局搜索 —— 仅 bump 后端的代际计数器。 */
export const cancelSearch = () => invoke<void>('cancel_search')

/** 单调自增的搜索 request id 工厂。每次 `searchSessions` 调用前取一个。 */
let _nextSearchId = 0
export function nextSearchRequestId(): number {
  _nextSearchId += 1
  return _nextSearchId
}

export const renameSession = (agent: Agent, path: string, name: string) =>
  invoke<void>('rename_session', { agent, path, name })

/** `/fork`：把 `sourceId` 会话克隆成全新独立 transcript（新 session id），打上 `title`，
 *  返回新 session id。`projectKey` = 项目目录名（ChatSession.projectKey）。 */
export const forkSession = (
  agent: Agent,
  projectKey: string,
  sourceId: string,
  title: string,
) => invoke<string>('fork_session', { agent, projectKey, sourceId, title })

/** 复制 Claude transcript 中目标提问之前的部分，返回新的 session id。 */
export const forkSessionBeforeUserTurn = (
  agent: Agent,
  projectKey: string,
  sourceId: string,
  title: string,
  keepUserTurns: number,
) => invoke<string>('fork_session_before_user_turn', {
  agent,
  projectKey,
  sourceId,
  title,
  keepUserTurns,
})

export const codexArchiveSession = (sessionId: string) =>
  invoke<void>('codex_archive_session', { sessionId })

export const softDeleteSession = (
  agent: Agent,
  path: string,
  projectLabel: string,
) => invoke<void>('soft_delete_session', { agent, path, projectLabel })

/** 永久删除一个会话文件（不进回收站、不可恢复）。仅供 worktree「全部删除」使用。 */
export const hardDeleteSession = (agent: Agent, path: string) =>
  invoke<void>('hard_delete_session', { agent, path })

/** btw 侧聊关闭后清理 --fork-session 产生的会话文件。 */
export const purgeBtwSession = (projectKey: string, sessionId: string) =>
  invoke<void>('purge_btw_session', { projectKey, sessionId })

export const listTrash = () => invoke<TrashItem[]>('list_trash')

export const restoreSession = (trashFile: string) =>
  invoke<void>('restore_session', { trashFile })

export const permanentDeleteTrash = (trashFile: string) =>
  invoke<void>('permanent_delete_trash', { trashFile })

export const emptyTrash = () => invoke<void>('empty_trash')

export const revealInFinder = (path: string) =>
  invoke<void>('reveal_in_finder', { path })

/** 打开本地文件；若 path 带 `:line[:column]`，后端会尽量跳到对应位置。 */
export const openLocalPath = (path: string) =>
  invoke<void>('open_local_path', { path })

/** 在系统默认浏览器中打开一个外部链接（仅 http/https）。 */
export const openUrl = (url: string) => invoke<void>('open_url', { url })

/**
 * 用系统默认程序打开聊天里的文件（相对 / 部分路径按会话 cwd 解析）。
 * 传了 `line`（可选 `col`）时，若装了支持跳行的编辑器（VS Code/Cursor/Zed/Sublime/Android
 * Studio 等）则在其中打开并跳到对应行；否则退回默认程序仅打开。
 */
export const openPathExternal = (path: string, cwd?: string, line?: number, col?: number) =>
  invoke<void>('open_path_external', { path, cwd, line, col })

/** 写入用户指定的绝对路径（覆盖同名）。返回最终路径以便后续 reveal。 */
export const writeFile = (path: string, content: string) =>
  invoke<string>('write_file', { path, content })

/** 写入二进制文件（base64 编码）。 */
export const writeBinaryFile = (path: string, base64: string) =>
  invoke<string>('write_binary_file', { path, base64 })

/** Live tail：让后端开始监听一个 JSONL 文件，新增片段会通过 `session:append` 事件
 *  推送过来。同一时刻只有一个 watcher —— 换成别的会话会替换前一个，重复订阅**同一个**
 *  会话是幂等的空操作（不会重建 watcher、也不会再起一条轮询线程）。
 *
 *  `knownCount` 是调用方手上已有的消息条数，后端拿它当「append 从哪里开始切」的基准。
 *  只在**刚刚 readSession 完同一个 path** 时传 —— 传错会导致整段消息被当成新增重复追加。
 *  不传则由后端自己解析一遍文件建立基准（多花一次全量解析）。 */
export const watchSession = (agent: Agent, path: string, knownCount?: number) =>
  invoke<void>('watch_session', { agent, path, knownCount })

/** 关闭 Live tail。可重入 —— 没有活跃 watcher 也不会抛错。 */
export const unwatchSession = () => invoke<void>('unwatch_session')

export const checkWatchedSession = () => invoke<void>('check_watched_session')

export const terminalTurnSignal = (
  agent: Agent,
  path: string,
  state: 'started' | 'completed' | 'blocked' | 'failed',
) => invoke<void>('terminal_turn_signal', { agent, path, state })

export type TurnHookInstallResult = {
  claudeSettingsPath: string
  codexHooksPath: string
  agyHooksPath: string
  grokConfigPath: string
  kimiConfigPath: string
  piExtensionPath: string
  piSettingsPath: string
}

export type TurnHookEventStatus = {
  name: string
  installed: boolean
}

export type TurnHookEntry = {
  event: string
  category: string | null
  matcher: string | null
  hookType: string
  detail: string
  managed: boolean
}

export type TurnHookAgentStatus = {
  installed: boolean
  configPath: string
  events: TurnHookEventStatus[]
  hooks: TurnHookEntry[]
}

export type TurnHookStatus = {
  enabled: boolean
  claude: TurnHookAgentStatus
  codex: TurnHookAgentStatus
  agy: TurnHookAgentStatus
  grok: TurnHookAgentStatus
  kimicode: TurnHookAgentStatus
  pi: TurnHookAgentStatus
}

export const installTurnHooks = () => invoke<TurnHookInstallResult>('install_turn_hooks')
export const uninstallTurnHooks = () => invoke<TurnHookInstallResult>('uninstall_turn_hooks')
export const turnHookStatus = () => invoke<TurnHookStatus>('turn_hook_status')
export const claudeRuntimeInfo = () => invoke<ClaudeRuntimeInfo>('claude_runtime_info')
export const codexRuntimeInfo = () => invoke<CodexRuntimeInfo>('codex_runtime_info')

export const resumeSession = (
  agent: Agent,
  sessionId: string,
  cwd: string,
  path: string,
  extraArgs?: string,
  terminalApp?: string,
) => invoke<void>('resume_session', { agent, sessionId, cwd, path, extraArgs: extraArgs || '', terminalApp: terminalApp || 'terminal' })

/** 在终端里为某个项目目录开一个全新会话（不带 --resume）。 */
export const newSession = (agent: Agent, cwd: string, extraArgs?: string, terminalApp?: string) =>
  invoke<void>('new_session', { agent, cwd, extraArgs: extraArgs || '', terminalApp: terminalApp || 'terminal' })

/** 检测 macOS 上已安装的外部终端应用（iTerm2 / Ghostty / cmux）。 */
export const detectTerminals = () => invoke<string[]>('detect_terminals')

// ---------- 内嵌 TUI（在窗口里直接跑 resume CLI，配合 xterm.js）----------

/** 拉起一个 PTY 跑 `<shell> -l -c "cd <cwd> && <agent resume CLI>"`，返回 PTY id。
 *  后续通过 `pty://data` 事件接收输出，`ptyWrite` 喂键盘输入，`ptyResize` 跟窗口大小。 */
export const ptySpawn = (
  agent: Agent,
  sessionId: string,
  cwd: string,
  path: string,
  cols: number,
  rows: number,
  extraArgs?: string,
  colorScheme?: 'light' | 'dark',
  useReclaude?: boolean,
) => invoke<number>('pty_spawn', {
  agent,
  sessionId,
  cwd,
  path,
  cols,
  rows,
  extraArgs: extraArgs || '',
  colorScheme: colorScheme || 'light',
  useReclaude,
})

/** 启动一个新会话的 PTY（不带 --resume）。 */
export const ptySpawnNew = (
  agent: Agent,
  cwd: string,
  cols: number,
  rows: number,
  extraArgs?: string,
  colorScheme?: 'light' | 'dark',
  useReclaude?: boolean,
) =>
  invoke<number>('pty_spawn_new', {
    agent,
    cwd,
    cols,
    rows,
    extraArgs: extraArgs || '',
    colorScheme: colorScheme || 'light',
    useReclaude,
  })

/** 启动一个纯 shell PTY（不跑任何 agent CLI）。 */
export const ptySpawnShell = (
  cwd: string,
  cols: number,
  rows: number,
  colorScheme?: 'light' | 'dark',
) =>
  invoke<number>('pty_spawn_shell', {
    cwd,
    cols,
    rows,
    colorScheme: colorScheme || 'light',
  })

/** 把用户的按键 base64 后写进 PTY stdin。 */
export const ptyWrite = (id: number, base64: string) =>
  invoke<void>('pty_write', { id, data: base64 })

/** 容器尺寸变了同步给 PTY，子进程会收到 SIGWINCH 重新布局。 */
export const ptyResize = (id: number, cols: number, rows: number) =>
  invoke<void>('pty_resize', { id, cols, rows })

/** 强杀子进程并清理 PTY；幂等，已死的 id 也安全。 */
export const ptyKill = (id: number) => invoke<void>('pty_kill', { id })

// ---------- GUI chat（程序化聊天：管道子进程跑 stream-json）----------

/** 启动一个 GUI chat 子进程，返回 { chatId, processModel }。`sessionId` 给出时续聊既有
 *  会话；`permissionMode` 走后端允许列表（default | acceptEdits | plan | bypassPermissions
 *  | ask | approve | fullAccess | custom），缺省由 agent 决定。`model` / `effort`
 *  缺省走 CLI 自身默认。`processModel` 让前端决定切
 *  设置走 restart-with-resume（长驻/app-server）还是下轮 flag（one-shot）。后续通过
 *  `agent-chat://event|init|result|delta|exit|stderr` 事件接收。 */
export const agentChatStart = (
  agent: Agent,
  projectKey: string,
  cwd: string,
  sessionId?: string,
  permissionMode?: string,
  model?: string,
  effort?: string,
  fork?: boolean,
  useReclaude?: boolean,
  preloadMessages?: Msg[],
  title?: string,
  /** Codex side conversation：临时 thread，不写入 session history。 */
  ephemeral?: boolean,
) =>
  invoke<ChatStartInfo>('agent_chat_start', {
    agent,
    projectKey,
    cwd,
    sessionId,
    permissionMode,
    model,
    effort,
    fork,
    useReclaude,
    preloadMessages,
    title,
    ...(ephemeral === undefined ? {} : { ephemeral }),
  })

export const agentChatListRunning = () =>
  invoke<RunningChatInfo[]>('agent_chat_list_running')

export const agentChatSetTitle = (id: number, title: string) =>
  invoke<void>('agent_chat_set_title', { id, title })

/** 向某个 chat 子进程发送一条用户消息（含可选图片附件 + 本轮 model/effort/权限）。
 *  one-shot agent 据此每轮切换；长驻/app-server agent 的切换走 restart。 */
export const agentChatSend = (
  id: number,
  text: string,
  images?: ChatImageInput[],
  model?: string,
  effort?: string,
  permissionMode?: string,
  textElements?: ChatTextElement[],
) =>
  invoke<void>('agent_chat_send', {
    id,
    text,
    images: images ?? [],
    model,
    effort,
    permissionMode,
    textElements: textElements ?? [],
  })

/** 把一条 follow-up 追加到当前运行的 Codex turn；成功时不会创建新 turn 或中断当前执行。 */
export const agentChatSteer = (
  id: number,
  text: string,
  textElements: ChatTextElement[] = [],
) =>
  invoke<void>('agent_chat_steer', {
    id,
    text,
    textElements,
  })

/** 从最近一次被取消的 Codex turn 之前创建持久化分支，返回新 thread id。 */
export const agentChatForkBeforeLastTurn = (id: number) =>
  invoke<string>('agent_chat_fork_before_last_turn', { id })

/** 读取本地图片文件为 base64（系统选择器只给路径，这里取字节做缩略图 + 视觉块）。 */
export const readFileBase64 = (path: string) =>
  invoke<ChatImageInput>('read_file_base64', { path })

export const saveClipboardImage = (data: string, mediaType: string) =>
  invoke<string>('save_clipboard_image', { data, mediaType })

/** Read and normalize a macOS NSPasteboard image (including screenshot TIFFs). */
export const saveMacosClipboardImage = () =>
  invoke<string | null>('save_macos_clipboard_image')

/** Read text from the native macOS pasteboard without WebView clipboard permissions. */
export const readMacosClipboardText = () =>
  invoke<string | null>('read_macos_clipboard_text')

/** 判断本地路径是否为目录（拖拽到输入框的附件可能是文件或文件夹，据此选图标 + 提示）。 */
export const pathIsDir = (path: string) => invoke<boolean>('path_is_dir', { path })

/** 会话 cwd 所在仓库的当前 git 分支名；无仓库 / 读不到时为 null（chat 头部展示用）。 */
export const gitCurrentBranch = (cwd: string) =>
  invoke<string | null>('git_current_branch', { cwd })

/** 分支选择器用的仓库状态：当前分支、本地可切换分支和未提交文件数。 */
export const gitRepositoryState = (cwd: string) =>
  invoke<GitRepositoryState>('git_repository_state', { cwd })

/** 切换到已存在的本地分支；后端会拒绝未提交改动，避免误带 working changes。 */
export const gitSwitchBranch = (cwd: string, branch: string) =>
  invoke<GitRepositoryState>('git_switch_branch', { cwd, branch })

/** 删除已合并的非当前本地分支；后端采用非强制 git branch -d。 */
export const gitDeleteBranch = (cwd: string, branch: string) =>
  invoke<GitRepositoryState>('git_delete_branch', { cwd, branch })

/** 基于当前 HEAD 创建一个本地分支，不会切换当前工作目录。 */
export const gitCreateBranch = (cwd: string, branch: string) =>
  invoke<GitRepositoryState>('git_create_branch', { cwd, branch })

/** cwd 是否是一个 git 仓库；前端据此决定是否显示 Git Changes 入口。 */
export const gitHasRepo = (cwd: string) => invoke<boolean>('git_has_repo', { cwd })

/** commit 列表（hash / author / date / message），按最近优先。 */
export const gitLog = (cwd: string, limit?: number) =>
  invoke<GitCommit[]>('git_log', { cwd, limit })

/** 未提交的 working changes 文件列表。 */
export const gitStatus = (cwd: string) => invoke<GitFileStatus[]>('git_status', { cwd })

/** 某个 ref（`"working"` 或 commit hash）的变更文件列表 + 增删行数统计。 */
export const gitDiffFiles = (cwd: string, gitRef: string) =>
  invoke<GitDiffFile[]>('git_diff_files', { cwd, gitRef })

/** 某个 ref 下单个文件的 unified diff，已解析成 DiffHunk[]（复用 DiffBlock.vue）。 */
export const gitDiffFile = (cwd: string, gitRef: string, path: string) =>
  invoke<DiffHunk[]>('git_diff_file', { cwd, gitRef, path })

/** 粘贴板图片无磁盘路径，存到临时目录供 Codex 等 agent 通过 @"path" 引用。 */
export const saveTempImage = (base64: string, mediaType: string) =>
  invoke<string>('save_temp_image', { base64, mediaType })

/** GUI chat 输入框 `@` 文件浮层：列出会话 cwd 下的目录/文件（相对路径）。
 *  `query` 空 → 顶层直接子项；裸查询 → 全工作区模糊搜索；带 `/` → 目录逐级浏览。 */
export const listProjectFiles = (cwd: string, query: string, limit = 200) =>
  invoke<ProjectFileEntry[]>('list_project_files', { cwd, query, limit })

/** 结束一个 chat 子进程（kill + 回收）。幂等。 */
export const agentChatStop = (id: number) => invoke<void>('agent_chat_stop', { id })
/** 仅中断当前一轮生成；Claude 长驻 chat 会话继续保活。 */
export const agentChatInterrupt = (id: number) => invoke<void>('agent_chat_interrupt', { id })

/** 回写一次交互式工具权限决定（应答 `agent-chat://permission`）。`decision` 由前端按
 *  CLI 控制协议构造：允许 = `{behavior:'allow',updatedInput,[updatedPermissions]}`；
 *  拒绝 = `{behavior:'deny',message,interrupt}`。仅 Claude（长驻 stdin）支持。 */
export const agentChatRespondPermission = (
  id: number,
  requestId: string,
  decision: unknown,
) => invoke<void>('agent_chat_respond_permission', { id, requestId, decision })

/** 回写一次结构化提问（AskUserQuestion）的答案决定（应答 `agent-chat://question`）。 */
export const agentChatRespondQuestion = (
  id: number,
  requestId: string,
  decision: unknown,
) => invoke<void>('agent_chat_respond_question', { id, requestId, decision })

/** 拉 GUI chat `/` 浮层的动态指令（磁盘上的自定义命令 / user-invocable skills）。 */
export const agentChatSlashCommands = (agent: Agent, cwd: string) =>
  invoke<SlashCommand[]>('agent_chat_slash_commands', { agent, cwd })

export const reclaudeInfo = () => invoke<ReclaudeInfo>('reclaude_info')

export const trayQuickStats = () => invoke<TrayStats>('tray_quick_stats')

/** Keep native tray statistics aligned with the visible agent setting. */
export const setTrayEnabledAgents = (agents: Agent[]) =>
  invoke<void>('set_tray_enabled_agents', { agents })

/** 账号额度（5 小时 / 周 / 各模型分项）—— 走 OAuth 用量接口，每窗口含精确利用率 + 重置时间。 */
export const accountUsage = (force = false) => invoke<AccountUsage>('account_usage', { force })

export interface UpdateInfo {
  current: string
  latest: string
  hasUpdate: boolean
  /** GitHub release page URL — present when a remote release was found. */
  htmlUrl?: string
}
export const appVersion = () => invoke<string>('app_version')

// 仓库地址直接写死 —— 与 src/App.vue 里 REPO_URL 同源。GitHub /releases/latest 已经
// 过滤掉 draft / prerelease，所以拿到的就是当前稳定版。Tauri WKWebView 自带 fetch，
// 没有 CSP 限制（tauri.conf.json csp=null），不需要在 Rust 侧加 HTTP client 依赖。
const GITHUB_LATEST_RELEASE_URL =
  'https://api.github.com/repos/jerrywu001/cc-sessions-viewer/releases/latest'
const RELEASE_PAGE_URL =
  'https://github.com/jerrywu001/cc-sessions-viewer/releases/latest'

interface GitHubRelease {
  tag_name?: string
  html_url?: string
}

function compareVer(a: string, b: string): number {
  const pa = a.replace(/^v/i, '').split(/[.-]/).map((x) => parseInt(x, 10) || 0)
  const pb = b.replace(/^v/i, '').split(/[.-]/).map((x) => parseInt(x, 10) || 0)
  const n = Math.max(pa.length, pb.length)
  for (let i = 0; i < n; i++) {
    const da = pa[i] ?? 0
    const db = pb[i] ?? 0
    if (da !== db) return da - db
  }
  return 0
}

export async function checkUpdate(): Promise<UpdateInfo> {
  const current = await appVersion()
  const res = await fetch(GITHUB_LATEST_RELEASE_URL)
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const release = await res.json() as GitHubRelease
  const latest = release.tag_name?.replace(/^v/i, '')
  if (!latest) return { current, latest: current, hasUpdate: false }
  return {
    current,
    latest,
    hasUpdate: compareVer(latest, current) > 0,
    htmlUrl: release.html_url ?? RELEASE_PAGE_URL,
  }
}

// ---- CLI 环境检测 ----

import type { CliVersionInfo, CliDiagnosisResult, CliUpgradeResult } from './types'

export const checkCliVersions = () =>
  invoke<CliVersionInfo[]>('check_cli_versions')

export const checkCliVersion = (cliName: string) =>
  invoke<CliVersionInfo>('check_cli_version', { cliName })

export const installCli = (cliName: string) =>
  invoke<CliUpgradeResult>('install_cli', { cliName })

export const upgradeCli = (cliName: string) =>
  invoke<CliUpgradeResult>('upgrade_cli', { cliName })

export const upgradeAllClis = () =>
  invoke<CliUpgradeResult[]>('upgrade_all_clis')

export const diagnoseCli = (cliName: string) =>
  invoke<CliDiagnosisResult>('diagnose_cli', { cliName })

import type {
  AdoptRequest,
  Bundle,
  BundleInclude,
  DeleteOptions,
  FileRev,
  RepairRequest,
  SkillDetail,
  SkillFileList,
  SkillFileText,
  SkillScan,
  SkillUpdateCheck,
  WriteReport,
} from './types'

/**
 * 工具管理 · Skills 全盘扫描（只读）。
 *
 * `cwd` 是当前项目目录 —— 不给就只扫 user 级，项目里的 `.claude/skills/` 等一概看不到。
 */
export const toolsScanSkills = (cwd?: string, extra: string[] = []) =>
  invoke<SkillScan>('tools_scan_skills', { cwd: cwd ?? null, extra })

/** 单个 skill 的详情：完整链路、主 body 的文件清单与风险明细。 */
export const toolsSkillDetail = (name: string, cwd?: string, extra: string[] = []) =>
  invoke<SkillDetail>('tools_skill_detail', { name, cwd: cwd ?? null, extra })

/**
 * 收编：把散落的实体目录搬进主 store，原位留链。
 *
 * `dryRun` 先拿计划给确认框，用户点了再用**同样的参数**真跑一遍。同名冲突不会静默
 * 跳过 —— 那些条目一步都不做，原样回到 `conflicts` 里等用户三选一。
 */
export const toolsAdoptSkills = (items: AdoptRequest[], mainStore: string, dryRun: boolean) =>
  invoke<WriteReport>('tools_adopt_skills', { items, mainStore, dryRun })

/** 启停：在某个 agent 的 skills 目录里建 / 拆链接。永远不碰实体内容。 */
export const toolsToggleSkill = (
  name: string,
  store: string,
  body: string | null,
  on: boolean,
  dryRun: boolean,
) => invoke<WriteReport>('tools_toggle_skill', { name, store, body, on, dryRun })

/** 删除：按反向索引全量解链，再删实体目录。 */
export const toolsDeleteSkill = (
  name: string,
  opts: DeleteOptions,
  cwd: string | undefined,
  extra: string[],
  dryRun: boolean,
) => invoke<WriteReport>('tools_delete_skill', { name, opts, cwd: cwd ?? null, extra, dryRun })

/** 扫全机器的 MCP 配置。只读，不启动任何 server。 */
export const toolsScanMcp = (cwd?: string) =>
  invoke<McpScan>('tools_scan_mcp', { cwd: cwd ?? null })

/**
 * 改 MCP 配置。**一律先 `dryRun: true` 跑一遍给用户看计划**，确认了再跑一次
 * `dryRun: false` —— 改的是用户全机器的 agent 配置文件。
 */
/**
 * `stamps` 是 dry-run 报告里那一份，点确认时原样回传。每个要碰的文件都得在里面、
 * 且指纹对得上，后端才写 —— 否则整批拒绝，一个文件都不动。dry-run 时传空。
 */
export const toolsApplyMcp = (
  edits: McpEdit[],
  cwd: string | undefined,
  dryRun: boolean,
  stamps: McpFileStamp[],
) => invoke<McpWriteReport>('tools_apply_mcp', { edits, cwd: cwd ?? null, dryRun, stamps })

/** 扫全机器的 hook 配置。同 MCP：读不出来的文件把错带回来，不静默跳过。 */
export const toolsScanHooks = (cwd?: string) =>
  invoke<HookScan>('tools_scan_hooks', { cwd: cwd ?? null })

/** 改 hook 配置。同样一律先 dry-run 出计划。 */
export const toolsApplyHooks = (edits: HookEdit[], cwd: string | undefined, dryRun: boolean) =>
  invoke<HookWriteReport>('tools_apply_hooks', { edits, cwd: cwd ?? null, dryRun })

/**
 * 拿一份假事件把命令**真跑一遍**，把 stdin 喂进去的 JSON 连同 stdout/stderr 一起带回来。
 *
 * 这是写 hook 唯一靠谱的验证方式：装上去之后它只在真实回合里触发，出了错也只是
 * 「agent 那边好像卡了一下」。
 */
export const toolsTestHook = (command: string, event: string, cwd?: string) =>
  invoke<HookTestResult>('tools_test_hook', { command, event, cwd: cwd ?? null })

/** 扫全机器的全局指令文件：路径、`@import`、生效链路、分叉。 */
export const toolsScanMemo = () => invoke<MemoScan>('tools_scan_memo')

/** 读一个全局指令文件。不存在不是错误 —— 回来的是空正文加 `exists: false`。 */
export const toolsReadMemo = (path: string) => invoke<MemoDoc>('tools_read_memo', { path })

/**
 * 写回一个全局指令文件。
 *
 * `expected` 是**打开时**拿到的那份指纹。对不上后端直接拒绝 —— 这些文件用户随时会在别的
 * 编辑器里改，拿旧内容盖掉是这个面板最坏的失败方式。
 */
export const toolsWriteMemo = (path: string, text: string, expected: MemoRevision) =>
  invoke<MemoDoc>('tools_write_memo', { path, text, expected })

/** 两份同名文件的逐行差异。只看，不合并。 */
export const toolsDiffMemo = (left: string, right: string) =>
  invoke<MemoDiff>('tools_diff_memo', { left, right })

/**
 * 保存被拒之后那一问：「外面到底改了什么」。
 *
 * 两边都只在内存里 —— 一边是打开时读到的原文，一边是刚重新读回来的，磁盘上没有第二个
 * 路径可以传给 `toolsDiffMemo`。后端是同一个 `diff_text`。
 */
export const toolsDiffMemoText = (
  left: string,
  right: string,
  leftLabel: string,
  rightLabel: string,
) => invoke<MemoDiff>('tools_diff_memo_text', { left, right, leftLabel, rightLabel })

/**
 * 合并若干组「同名同内容」的重复：搬一份进主 store，原位全换成链接。
 *
 * `names` 是文件名（`RTK.md` 这种），一次可以给多组 —— 健康条上那个「合并全部重复」
 * 就是把所有组一起交过来，共用一张计划、一次确认。先 `dryRun: true` 拿计划给用户看。
 */
export const toolsMergeMemo = (names: string[], store: string, dryRun: boolean) =>
  invoke<MemoMergeReport>('tools_merge_memo', { names, store, dryRun })

/**
 * 把一份受管副本按它的源重拷一遍。
 *
 * 没有 dry-run：这一步只做一件事，而且做什么完全由那份副本自己的状态决定。会吃掉数据
 * 的那几种（副本被就地改过 / 两边都变了）后端直接拒，不靠前端记得禁按钮。
 */
export const toolsResyncCopy = (path: string) => invoke<void>('tools_resync_copy', { path })

/** 拆掉一条没人读的活链接。死链走 `tools_repair_links`，那条只碰解析不到东西的。 */
export const toolsUnlinkRef = (path: string, dryRun: boolean) =>
  invoke<WriteReport>('tools_unlink_ref', { path, dryRun })

/** 只删一份内容：先解掉落在它身上的每一条链接，再删目录。 */
export const toolsDeleteBody = (
  name: string,
  body: string,
  cwd: string | undefined,
  extra: string[],
  dryRun: boolean,
) => invoke<WriteReport>('tools_delete_body', { name, body, cwd: cwd ?? null, extra, dryRun })

/** 修复：批量改指向 / 清死链。 */
export const toolsRepairLinks = (items: RepairRequest[], dryRun: boolean) =>
  invoke<WriteReport>('tools_repair_links', { items, dryRun })

/** 更新前的对比：fetch 一次，算出落后几个提交、有哪些本地改动会被冲掉。 */
export const toolsCheckSkillUpdate = (body: string) =>
  invoke<SkillUpdateCheck>('tools_check_skill_update', { body })

/** 强制更新到远端最新（`reset --hard`）。返回新的短 sha。 */
export const toolsUpdateSkill = (body: string) => invoke<string>('tools_update_skill', { body })

// ---------------------------------------------------------------------------
// 内置编辑器
//
// `body` 是 skill 的实体目录，后端拿它当作用域的根；`rel` 一律是相对它的路径，
// 越界由后端再挡一次（前端那道只是省一次往返，不是防线）。
// ---------------------------------------------------------------------------

/** 列出一个 skill 目录里的所有文件。 */
export const toolsListSkillFiles = (body: string) =>
  invoke<SkillFileList>('tools_list_skill_files', { body })

export const toolsReadSkillFile = (body: string, rel: string) =>
  invoke<SkillFileText>('tools_read_skill_file', { body, rel })

/**
 * 写回一个文件。`rev` 必须是**读的时候拿到的那份** —— 文件在别处被改过时后端会拒，
 * 不会把用户在另一个编辑器里的改动默默盖掉。返回写完之后的新 `rev`。
 */
export const toolsWriteSkillFile = (body: string, rel: string, text: string, rev: FileRev) =>
  invoke<FileRev>('tools_write_skill_file', { body, rel, text, rev })

// ---------------------------------------------------------------------------
// 配置集
// ---------------------------------------------------------------------------

/**
 * 把当前这台机器上的一套配置打成一个包。
 *
 * **只有导出走后端。** 导入那一半是把包里的条目翻成 `McpEdit` / `HookEdit` /
 * 一次 `toolsWriteMemo`，再走上面那三条已经有校验、dry-run、备份和回读的路 ——
 * 在后端再写一个 `tools_import_bundle`，等于把那几道闸重造一遍。
 */
export const toolsExportBundle = (cwd: string | undefined, include: BundleInclude) =>
  invoke<Bundle>('tools_export_bundle', { cwd: cwd ?? null, include })

/**
 * 把用户挑中的那个文件读成文本。只读原文 —— 认不认、哪几条能装，全在
 * `toolsBundle.ts` 里判。
 */
export const toolsReadBundle = (path: string) => invoke<string>('tools_read_bundle', { path })
