// 工具管理 · 全局配置面板的纯逻辑。
//
// 和另外三个面板同一个理由住在这儿：面板在 `src/views/`、弹框在 `src/modals/`，两个目录
// 在 `vitest.config.ts` 都是覆盖率排除的。
//
// 这个面板最容易错的一条判断是**「改这个文件会影响谁」**：opencode 和 grok 在自己的
// `AGENTS.md` 缺席时都回退去读 `~/.claude/CLAUDE.md`，用户以为只在改 Claude。判断写进
// .vue 就再也没人测得到。

import type {
  Agent,
  MemoAgentInfo,
  MemoDiff,
  MemoFile,
  MemoFork,
  MemoImport,
  MemoReader,
  MemoRevision,
  MemoScan,
} from './types'
import { isAgentOn } from './toolsPanel'

/** 左栏的一行：要么是一家 agent，要么是一个被引用进来的片段。 */
export interface MemoRow {
  /** 列表 key。agent 行按 agent 排重（两家回退到同一个文件时不能挤成一行）。 */
  key: string
  kind: 'agent' | 'fragment'
  agent: Agent | null
  /**
   * 点开它编辑哪个文件。null = 点不开（这家没有 home 级约定）。
   *
   * 回退中的那几家指向**实际生效**的那份，不是自己那个还不存在的位置：grok 明明有
   * 一整套全局指令在生效，点开却是个空编辑器，那是在说谎。
   */
  path: string | null
  /** 这家自己的约定路径。回退中时和 `path` 不是同一个。 */
  own: string | null
  /** 显示的文件名 —— 也就是 `path` 的文件名。 */
  name: string
  state: MemoRowState
  bytes: number
}

/**
 * 一行的状态。
 * - `ok` —— 自己的文件在
 * - `fallback` —— 自己那份没有，实际读的是别人的
 * - `missing` —— 自己那份没有，也没有回退（能新建）
 * - `unsupported` —— 这家没有 home 级约定，禁用态
 * - `fragment` —— 被 `@import` 进来的片段
 * - `broken` —— 片段的目标文件不存在
 */
export type MemoRowState = 'ok' | 'fallback' | 'missing' | 'unsupported' | 'fragment' | 'broken'

function fileAt(scan: MemoScan | null, path: string | null): MemoFile | null {
  if (!scan || !path) return null
  return scan.files.find((f) => f.path === path) ?? null
}

function agentRow(scan: MemoScan, info: MemoAgentInfo): MemoRow {
  const state: MemoRowState = !info.supported
    ? 'unsupported'
    : info.exists
      ? 'ok'
      : info.fallenBack
        ? 'fallback'
        : 'missing'
  // 不支持的那几家点不开：给个输入框让用户白写比什么都不给更糟。
  const path = info.supported ? (info.effective ?? info.path) : null
  return {
    key: `agent:${info.agent}`,
    kind: 'agent',
    agent: info.agent,
    path,
    own: info.path,
    name: path ? baseName(path) : '',
    state,
    bytes: fileAt(scan, path)?.bytes ?? 0,
  }
}

/**
 * 左栏列表的**全集**：先七家 agent，再是被引用进来的片段。不看 agent 勾选，也不看
 * 搜索词。
 *
 * 健康条上那个总数要的就是这个。别的三个面板可以直接拿后端 summary 当分母
 * （它们的行就是扫描结果本身），全局指令这边不行 —— 它的行是**位置**不是文件：
 * 七家里有四家还没有自己那份（state=missing），扫描却一份文件都数不到。拿
 * `summary.files` 当分母会得出「9 行 / 8 个文件」这种自相矛盾的东西。
 */
export function allMemoRows(scan: MemoScan | null): MemoRow[] {
  if (!scan) return []
  const rows: MemoRow[] = scan.agents.map((a) => agentRow(scan, a))

  // 片段 = 被 import 进来的文件，且不是任何一家的约定路径（后者已经在上面了）。
  const owned = new Set(scan.agents.map((a) => a.path).filter(Boolean) as string[])
  for (const f of scan.files) {
    if (f.importedBy === null || owned.has(f.path)) continue
    rows.push({
      key: f.path,
      kind: 'fragment',
      agent: null,
      path: f.path,
      own: f.path,
      name: f.name,
      state: f.exists ? 'fragment' : 'broken',
      bytes: f.bytes,
    })
  }
  return rows
}

/**
 * 左栏列表：`allMemoRows` 过一遍 agent 勾选和搜索词。
 *
 * agent 过滤器只作用在上半截 —— 片段不属于任何一家（`~/.claude/RTK.md` 是被
 * `CLAUDE.md` 引用的，而 `CLAUDE.md` 有三家在读）。
 */
export function memoRows(scan: MemoScan | null, query: string): MemoRow[] {
  const q = query.trim().toLowerCase()
  const rows = allMemoRows(scan).filter((r) => r.kind !== 'agent' || isAgentOn(r.agent as Agent))
  if (!q) return rows
  // `own` 也算：回退中那几行显示的是别人的路径，只搜 `path` 的话搜 `.grok` 会一无所获。
  return rows.filter((r) =>
    [r.name, r.path ?? '', r.own ?? '', r.agent ?? ''].some((s) => s.toLowerCase().includes(q)),
  )
}

export function baseName(path: string): string {
  return path.split(/[/\\]/).pop() || path
}

/**
 * 「改这个文件会影响谁」—— 除了正在看的这一家。
 *
 * **这是本功能最容易踩的坑。** 只算 `active` 的：`~/.claude/CLAUDE.md` 在 grok 眼里是
 * 回退目标，但 grok 一旦自己建了 `AGENTS.md`，这条链路就不生效了，再提示就是误报。
 */
export function otherReaders(scan: MemoScan | null, path: string | null, self: Agent | null) {
  const f = fileAt(scan, path)
  if (!f) return [] as MemoReader[]
  return f.readers.filter((r) => r.active && r.agent !== self)
}

/** 这个文件所在的那处分叉。没有就是 null。 */
export function forkOf(scan: MemoScan | null, path: string | null): MemoFork | null {
  if (!scan || !path) return null
  return scan.forks.find((k) => k.sides.some((s) => s.path === path)) ?? null
}

/** 分叉的另一边（可能不止一个）。 */
export function forkOthers(fork: MemoFork | null, path: string | null) {
  if (!fork || !path) return []
  return fork.sides.filter((s) => s.path !== path)
}

/** 这个文件里的 `@import` 行。 */
export function importsOf(scan: MemoScan | null, path: string | null): MemoImport[] {
  return fileAt(scan, path)?.imports ?? []
}

/** 打开一个新文件时用的空白 revision —— 「还没读过」。 */
export function blankRevision(): MemoRevision {
  return { exists: false, size: 0, mtimeMs: null }
}

/** 两份指纹是不是同一份文件状态。保存前后各比一次。 */
export function sameRevision(a: MemoRevision, b: MemoRevision): boolean {
  return a.exists === b.exists && a.size === b.size && a.mtimeMs === b.mtimeMs
}

/**
 * 新建时预填的最小模板。
 *
 * 只给一个标题 —— 再多就是替用户决定他的全局指令该写什么。文件名进标题是为了让人一眼
 * 认出这是哪一份（几家都叫 `AGENTS.md`）。
 */
export function template(path: string): string {
  return `# ${baseName(path)}\n\n`
}

/** 字节数写成人能读的。全局指令都是几百字节到几十 KB。 */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

/**
 * 这一行能不能编辑。
 *
 * 不支持的那几家不行；其余都行 —— 包括文件还不存在的，保存即新建。
 */
export function editable(row: MemoRow): boolean {
  return row.path !== null && row.state !== 'unsupported'
}

/** 一家 agent 现在实际读的是哪个文件，以及是不是回退来的。 */
export function effectiveOf(scan: MemoScan | null, agent: Agent | null) {
  if (!scan || !agent) return null
  return scan.agents.find((a) => a.agent === agent) ?? null
}

/**
 * 「让这家自己接管」要写到哪个文件。不在回退中就没有这回事。
 *
 * 接管从**空文件**开始是个陷阱：存下去的那一刻这家就丢掉了本来在生效的一整套指令。
 * 所以调用方拿着这个路径去开编辑器时，正文预填的是现在生效的那份。
 */
export function takeoverPath(row: MemoRow): string | null {
  return row.state === 'fallback' ? row.own : null
}

/**
 * 这个文件在磁盘上是不是已经和我们读到的那份不一样了。
 *
 * `watch.rs` 是单会话 watcher，套不上这些散落在各家 home 下的文件，所以只能在两个时刻
 * 比对：面板打开 / 窗口重新聚焦时重扫一遍，以及保存时后端再比一次。扫不到这个路径
 * （比如它压根不是任何一家的约定文件）时**不报**冲突 —— 没有依据的警告比没有警告糟。
 */
export function externallyChanged(
  scan: MemoScan | null,
  path: string | null,
  loaded: MemoRevision | null,
): boolean {
  const f = fileAt(scan, path)
  if (!f || !loaded) return false
  return !sameRevision(f.revision, loaded)
}

/**
 * 这个比较框该不该给「覆盖」按钮。
 *
 * 两边一样时不给：一次没有差异的覆盖什么也改不了，却照样重写一遍文件、留一份 .bak
 * ——一个点了什么都不会发生的危险按钮比没有按钮糟。
 *
 * 太大没逐行比的那种**照样给**：没比过不等于一样，那会儿这个按钮正是用户唯一的出路。
 */
export function canSyncFork(diff: MemoDiff | null): boolean {
  return diff !== null && !diff.same
}
