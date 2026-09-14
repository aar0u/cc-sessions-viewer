// 工具管理 · Skills 写操作的纯逻辑：主 store、收编计划、冲突三选一的状态机、
// 修复计划、写报告的归纳。
//
// 和 `toolsSkills.ts` 分家的理由是「读 / 写」：那边回答「现在是什么样」，这边回答
// 「要把它改成什么样」。两边都不碰 DOM、不碰 Tauri —— 面板在 `src/views/`、对话框在
// `src/modals/`，那两个目录在 `vitest.config.ts` 是覆盖率排除的，判断写进 .vue 就等于
// 没有单测。而这里恰恰是最不该靠肉眼验的部分：一次点错「收编全部」动的是用户
// 全机器的 skill 目录。

import { ref } from 'vue'
import type {
  AdoptConflict,
  AdoptRequest,
  RepairRequest,
  Resolution,
  SkillEntry,
  SkillRef,
  SkillScan,
  SkillUpdateCheck,
  StepKind,
  WriteReport,
} from './types'
import { isUnhealthy } from './toolsSkills'

// ---------------------------------------------------------------------------
// 主 store
// ---------------------------------------------------------------------------

const MAIN_STORE_KEY = 'toolsMainStore:v1'

function loadMainStore(): string | null {
  try {
    return localStorage.getItem(MAIN_STORE_KEY)
  } catch {
    return null
  }
}

/**
 * 收编的去处。后端不落盘（扫描是无状态的），这里记住用户挑的那个。
 *
 * 为什么不让后端存：主 store 只影响「往哪儿搬」，而每条写命令本来就要把它当参数传
 * 过去。后端再存一份就有两个真相，用户在别处挪走目录之后那份记录还是错的。
 */
export const mainStore = ref<string | null>(loadMainStore())

export function setMainStore(path: string | null) {
  mainStore.value = path
  try {
    if (path) localStorage.setItem(MAIN_STORE_KEY, path)
    else localStorage.removeItem(MAIN_STORE_KEY)
  } catch {
    // 无痕模式之类：内存里的值照样能用，只是下次打开要重选。
  }
}

// ---------------------------------------------------------------------------
// 用户自己加的目录
// ---------------------------------------------------------------------------

const EXTRA_STORES_KEY = 'toolsExtraStores:v1'

function loadExtraStores(): string[] {
  try {
    const raw = localStorage.getItem(EXTRA_STORES_KEY)
    const list: unknown = raw ? JSON.parse(raw) : []
    return Array.isArray(list) ? list.filter((p): p is string => typeof p === 'string') : []
  } catch {
    return []
  }
}

/**
 * 「新增主目录」加进来的目录。
 *
 * 内置的候选是本机客观存在的那几个（各家 agent 自己声明的 + 三个第三方管理器），
 * 用户把 skill 放在别处时一个都不合用。这份列表**每次扫描都要带给后端** —— 后端不
 * 扫的目录不能当主 store：收编会把内容搬进去，下一次扫描却看不见，UI 上就是「skill
 * 凭空消失了」。
 */
export const extraStores = ref<string[]>(loadExtraStores())

function saveExtraStores() {
  try {
    localStorage.setItem(EXTRA_STORES_KEY, JSON.stringify(extraStores.value))
  } catch {
    // 无痕模式之类：这一程还能用，下次打开要重新加。
  }
}

/** 加一个目录。已经在列表里就当没发生 —— 重复一条只会让下拉里多一行一样的。 */
export function addExtraStore(path: string) {
  if (!path || extraStores.value.includes(path)) return
  extraStores.value = [...extraStores.value, path]
  saveExtraStores()
}

/** 只从候选列表里去掉，磁盘上那个目录一个字节都不动。 */
export function removeExtraStore(path: string) {
  if (!extraStores.value.includes(path)) return
  extraStores.value = extraStores.value.filter((p) => p !== path)
  saveExtraStores()
  // 删掉的正是当前主 store 时得松手，否则收编会往一个已经不扫的地方搬。
  if (mainStore.value === path) setMainStore(null)
}

/**
 * 没选过就用扫描给的建议。
 *
 * 存下来的那个要过两关，任一不过就退回建议值：
 *
 * - **还在**：用户可能把目录挪走或删了，否则收编会往一个不存在的地方搬。
 * - **够格**：`canBeMain` 早期有个 bug —— 项目目录和 agent 自有目录也进了下拉，
 *   有人可能已经选中并存下来了。光看「还在不在」放不掉这种，它照样存在。
 *   建议值那边已经只在够格的里面挑，所以退回去一定是安全的。
 */
export function effectiveMainStore(scan: SkillScan | null): string | null {
  if (!scan) return mainStore.value
  const ok = scan.stores.some((s) => s.path === mainStore.value && s.exists && s.canBeMain)
  return ok ? mainStore.value : scan.suggestedMain
}

// ---------------------------------------------------------------------------
// 收编
// ---------------------------------------------------------------------------

/**
 * 这份内容是不是已经在主 store 里了。
 *
 * 分隔符两种都认（Windows 是 `\`）。只判 `/` 的话，Windows 上**每一条**内容都会被
 * 判成「不在主 store 里」—— 收编会把已经在里面的东西再搬一遍。
 */
export function inMainStore(bodyPath: string, main: string): boolean {
  const base = main.replace(/[/\\]+$/, '')
  if (bodyPath === base) return true
  const next = bodyPath[base.length]
  return bodyPath.startsWith(base) && (next === '/' || next === '\\')
}

/**
 * 一个 skill 里需要收编的实体内容。
 *
 * 已经在主 store 里的那份不算 —— 它就是收编的**去处**。所以一个只有一份内容且已经
 * 归位的 skill 返回空数组，UI 据此把「收编」按钮置灰，而不是让用户点一下什么都没发生。
 */
export function adoptTargets(entry: SkillEntry, main: string): AdoptRequest[] {
  return entry.bodies
    .filter((b) => !inMainStore(b.path, main))
    .map((b) => ({ name: entry.name, body: b.path, resolution: null }))
}

/**
 * 整批收编：**列表里当前这些** skill 的外部实体内容。
 *
 * 吃的是筛完、搜完之后的那份列表，不是整份扫描结果。理由是这个按钮就钉在健康条上，
 * 和角标筛选器并排 —— 筛到「重复 26」只想收编这 26 条，点下去却把全机器 43 条都搬了，
 * 那是屏幕上写的一件事、实际做的另一件事。
 *
 * 想收编全部：把筛选清掉，列表本来就是全部。
 */
export function adoptAllTargets(skills: SkillEntry[], main: string): AdoptRequest[] {
  return skills.flatMap((s) => adoptTargets(s, main))
}

// ---------------------------------------------------------------------------
// 冲突三选一
// ---------------------------------------------------------------------------

export interface ConflictState {
  queue: AdoptConflict[]
  /** 当前处理到第几条。等于 `queue.length` 表示走完了。 */
  index: number
  /** name → 用户的选择。 */
  resolutions: Record<string, Resolution>
  /** 明确跳过的（这一轮不动它）。 */
  skipped: string[]
}

export function startConflicts(conflicts: AdoptConflict[]): ConflictState {
  return { queue: conflicts, index: 0, resolutions: {}, skipped: [] }
}

export function currentConflict(state: ConflictState): AdoptConflict | null {
  return state.queue[state.index] ?? null
}

export function conflictDone(state: ConflictState): boolean {
  return state.index >= state.queue.length
}

/** 记下当前这条的选择并前进。状态整体替换，方便 Vue 的响应式跟上。 */
export function resolveCurrent(state: ConflictState, resolution: Resolution): ConflictState {
  const cur = currentConflict(state)
  if (!cur) return state
  return {
    ...state,
    index: state.index + 1,
    resolutions: { ...state.resolutions, [cur.name]: resolution },
  }
}

export function skipCurrent(state: ConflictState): ConflictState {
  const cur = currentConflict(state)
  if (!cur) return state
  return { ...state, index: state.index + 1, skipped: [...state.skipped, cur.name] }
}

/** 「全部跳过」：剩下的一条不做，已经选过的仍然算数。 */
export function skipAll(state: ConflictState): ConflictState {
  const rest = state.queue.slice(state.index).map((c) => c.name)
  return { ...state, index: state.queue.length, skipped: [...state.skipped, ...rest] }
}

/**
 * 把选择贴回请求列表，准备第二次提交。
 *
 * 跳过的条目**整条剔掉**，而不是带着 `resolution: null` 再发一次 —— 那样后端会把它
 * 当成「还没选」，第二轮又原样报回来一个冲突，用户点「跳过」等于没跳过。
 */
export function requestsWithResolutions(
  requests: AdoptRequest[],
  state: ConflictState,
): AdoptRequest[] {
  return requests
    .filter((r) => !state.skipped.includes(r.name))
    .map((r) => ({ ...r, resolution: state.resolutions[r.name] ?? r.resolution }))
}

// ---------------------------------------------------------------------------
// 修复
// ---------------------------------------------------------------------------

/** 这条引用需要修吗：断了、成环、指到非目录，或者绕了两跳以上。 */
export function needsRepair(ref: SkillRef): boolean {
  return isUnhealthy(ref) || ref.hops.length > 1
}

/**
 * 修复计划：能接回主 body 的就改指向，接不回的（目标已经没了）就清掉死链。
 *
 * `target` 是要接到的实体目录。给了就一律 relink —— 包括两跳链：它们「能用」，但
 * 中间那一跳一旦断掉整条就废，压成一跳才是把成因除掉。`target` 为 null 时只能清死链，
 * 活着的链一条不动（那是「停用」，不是「修复」）。
 */
export function repairPlan(entry: SkillEntry, target: string | null): RepairRequest[] {
  const out: RepairRequest[] = []
  for (const ref of entry.refs) {
    if (!needsRepair(ref)) continue
    // 实体目录不是链接，修不了也不用修。
    if (ref.health.state === 'realDir') continue
    if (ref.resolved === null) {
      // 死链：接得回就接，接不回就清掉。
      out.push({
        path: ref.path,
        action: target ? { kind: 'relink', value: target } : { kind: 'unlink' },
      })
      continue
    }
    if (target) out.push({ path: ref.path, action: { kind: 'relink', value: target } })
  }
  return out
}

/** 修复要接到哪份内容：优先主 store 里的，其次任意一份还在的。 */
export function repairTarget(entry: SkillEntry, main: string | null): string | null {
  if (main) {
    const inMain = entry.bodies.find((b) => inMainStore(b.path, main))
    if (inMain) return inMain.path
  }
  return entry.bodies[0]?.path ?? null
}

// ---------------------------------------------------------------------------
// 从远端更新
// ---------------------------------------------------------------------------

/** 没什么可做：已经是最新的，本地也没有改动。这时不该弹一个只能点「取消」的框。 */
export function updateIsNoop(check: SkillUpdateCheck): boolean {
  return check.behind === 0 && check.changed.length === 0
}

/** 列几个被改过的文件名，多了就省略。全列出来会把弹框撑成一屏。 */
const NAMED_FILES = 3

export function changedSummary(check: SkillUpdateCheck): string {
  const head = check.changed.slice(0, NAMED_FILES).join(', ')
  return check.changed.length > NAMED_FILES ? `${head} …` : head
}

/**
 * 二次确认框的正文，逐行拼。
 *
 * 拆成几条 i18n key 而不是一整段：四种情况（落后 / 已最新 × 改过 / 没改过）真要写成
 * 一条文案就是四条长句，翻译四份还得各自保持一致。而**「改过的会被强制覆盖」这一句
 * 必须只在真的会覆盖时出现** —— 没改过也吓一句，用户下次就不看了。
 */
export function updateMessageParts(
  check: SkillUpdateCheck,
): { key: string; vars?: Record<string, string> }[] {
  const out: { key: string; vars?: Record<string, string> }[] = [
    { key: 'tools.skills.update.from', vars: { remote: check.remote, branch: check.branch } },
  ]
  out.push(
    check.behind > 0
      ? {
          key: 'tools.skills.update.behind',
          vars: { n: String(check.behind), local: check.local, latest: check.latest },
        }
      : { key: 'tools.skills.update.current', vars: { local: check.local } },
  )
  if (check.changed.length > 0) {
    out.push({
      key: 'tools.skills.update.overwrite',
      vars: { n: String(check.changed.length), files: changedSummary(check) },
    })
  }
  // 未跟踪的文件不会被动，但不说清楚用户会以为自己加的笔记也没了。
  if (check.untracked > 0) {
    out.push({ key: 'tools.skills.update.untracked', vars: { n: String(check.untracked) } })
  }
  return out
}

// ---------------------------------------------------------------------------
// 报告
// ---------------------------------------------------------------------------

export const STEP_KINDS: StepKind[] = ['ensureDir', 'move', 'backup', 'link', 'unlink', 'deleteDir']

/** 每类各几步 —— 确认框的标题用它说人话（「解除 4 条链接、删除 1 个目录」）。 */
export function stepCounts(report: WriteReport): Record<StepKind, number> {
  const out = {
    ensureDir: 0,
    move: 0,
    backup: 0,
    link: 0,
    unlink: 0,
    deleteDir: 0,
  } as Record<StepKind, number>
  for (const s of report.steps) out[s.kind] += 1
  return out
}

/**
 * 这个计划会不会毁掉东西 —— 确认框据此变成危险样式。
 *
 * 备份步骤不算：它只是挪开，最后那条 `deleteDir` 删的才是真东西。但收编里那条
 * `deleteDir` 删的是备份出来的副本，内容在主 store 里还在…… 所以这里只问「计划里
 * 有没有删目录」，宁可多问一次，也不要把一次真删悄悄放过去。
 */
export function isDestructive(report: WriteReport): boolean {
  return report.steps.some((s) => s.kind === 'deleteDir')
}

/** 真跑完之后有没有哪一步没做成。后端出错会走 reject，这里兜的是部分提交的情况。 */
export function incompleteSteps(report: WriteReport): number {
  if (report.dryRun) return 0
  return report.steps.filter((s) => !s.done).length
}
