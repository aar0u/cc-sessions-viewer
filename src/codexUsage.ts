// Codex 账号额度（5 小时 / 周）的前端取数与展示模型。数据源是后端 `codex_account_usage`
// 命令 —— 它借 `codex app-server` 的 `account/rateLimits/read` 读官方订阅的额度窗口。
//
// **和 Claude 那套（src/usage.ts）刻意分开两份**：数据源、取数代价、窗口语义都不一样
// （这边每次取数要起一个 ~2s 的短命 codex 进程，那边是 curl；这边窗口长度由服务端给，
// 不是写死的 5h/周）。共用一份再按 agent 分支必然互相污染。
//
// 刷新策略与 Claude 侧同形（事件驱动 + 慢轮询兜底）：
//   · 事件驱动：每轮对话结束（onResult）→ bumpCodexUsage()，强制拉新（绕过后端 20s 缓存）。
//   · 慢轮询：进入 Codex live chat 时订阅（立即拉一次 + 之后每 60s），兜住在本 app 之外
//     消耗的用量（终端里的 codex / 别的机器）。
// 多个订阅者共享同一个定时器（引用计数）。某次拉取失败不清空已有快照。
//
// 即时呈现：最近一次成功的快照写进 localStorage，模块加载时回种 —— 进入 live chat 的瞬间
// 徽标就有值，不必干等那 ~2s 的子进程返回，后台再静默 revalidate。
import { ref } from 'vue'
import { codexAccountUsage } from './api'
import type { CodexAccountUsage } from './types'

/** 轮询间隔：与 Claude 侧一致。每次取数要起一个短命子进程，不能更快。 */
const POLL_MS = 60_000

/** 最近一次成功快照的 localStorage 键（即时回种用）。 */
const CACHE_KEY = 'csv:codex-usage:v1'

/** 两次「真正取数」之间的最小间隔。事件驱动的强制刷新若距上次不足这个间隔就跳过，
 *  防止密集对话把子进程叠起来。略小于后端 20s 缓存。 */
const MIN_FETCH_GAP_MS = 15_000

/** 倒计时心跳间隔（纯本地、不打接口）。 */
const TICK_MS = 30_000

function loadCachedUsage(): CodexAccountUsage | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY)
    return raw ? (JSON.parse(raw) as CodexAccountUsage) : null
  } catch {
    return null
  }
}
function saveCachedUsage(u: CodexAccountUsage | null): void {
  try {
    // null = 额度窗口不适用（切了第三方 key / 退登）。连带把回种用的快照抹掉，
    // 否则下次开屏又会先画出上一个账号的百分比。
    if (u) localStorage.setItem(CACHE_KEY, JSON.stringify(u))
    else localStorage.removeItem(CACHE_KEY)
  } catch {
    /* 配额/隐私模式失败无妨，纯加速用 */
  }
}

/** 当前额度快照（响应式，供底栏徽标读取）。 */
export const codexUsage = ref<CodexAccountUsage | null>(loadCachedUsage())
/** 最近一次「取数失败」的错误（成功后清空）。注意不含「额度窗口不适用」——
 *  那不是失败，走的是快照置 null 那条路。 */
export const codexUsageError = ref<string | null>(null)

/** 当前时间（ms），每 TICK_MS 跳一次；重置倒计时据此响应式重算。 */
export const codexNowMs = ref(Date.now())

let timer: ReturnType<typeof setInterval> | undefined
let tickTimer: ReturnType<typeof setInterval> | undefined
let subscribers = 0
let inFlight = false
let lastFetchAt = 0

async function refresh(force = false): Promise<void> {
  if (inFlight) return
  inFlight = true
  lastFetchAt = Date.now()
  try {
    // u 为 null = 额度窗口不适用（第三方 API key / provider / 已退登）→ 徽标直接消失。
    // 这跟下面 catch 的「这次没取到」是两回事，后者才保留旧值。
    const u = await codexAccountUsage(force)
    codexUsage.value = u
    saveCachedUsage(u)
    codexUsageError.value = null
  } catch (e) {
    // 失败不清空快照 —— 徽标保留「最近一次成功」的百分比，不闪空。
    codexUsageError.value = String(e)
  } finally {
    inFlight = false
  }
}

/**
 * 事件驱动刷新：一轮 Codex 对话结束后调用。强制拉新（绕过后端缓存）拿刚被这轮消耗掉的额度。
 * 仅在有徽标正在展示（订阅中）时才取数；再叠一道 MIN_FETCH_GAP_MS 节流防止密集连发。
 */
export function bumpCodexUsage(): void {
  if (subscribers === 0) return
  if (Date.now() - lastFetchAt < MIN_FETCH_GAP_MS) return
  void refresh(true)
}

/** 订阅额度轮询（第一个订阅者立即拉一次并启动定时器 + 倒计时心跳）。配 stopCodexUsagePolling 用。 */
export function startCodexUsagePolling(): void {
  subscribers += 1
  if (subscribers === 1) {
    void refresh()
    timer = setInterval(() => void refresh(), POLL_MS)
    codexNowMs.value = Date.now()
    tickTimer = setInterval(() => {
      codexNowMs.value = Date.now()
    }, TICK_MS)
  }
}

/** 退订（最后一个订阅者离开时停掉定时器 + 心跳）。 */
export function stopCodexUsagePolling(): void {
  subscribers = Math.max(0, subscribers - 1)
  if (subscribers === 0) {
    if (timer) {
      clearInterval(timer)
      timer = undefined
    }
    if (tickTimer) {
      clearInterval(tickTimer)
      tickTimer = undefined
    }
  }
}

export interface CodexUsageWindowView {
  /** 固定顺序里的位置：primary = 短窗口，secondary = 长窗口。 */
  key: 'primary' | 'secondary'
  /** 窗口长度（分钟），标签据此选。 */
  minutes: number
  /** 取整后的利用率百分比 0–100。 */
  percent: number
  /** ISO8601 重置时间（可能缺失）。 */
  resetsAt?: string
}

/** 把额度快照整理成固定顺序 [短窗口, 长窗口] 的展示列表；窗口不存在则跳过。 */
export function codexUsageWindows(
  u: CodexAccountUsage | null | undefined,
): CodexUsageWindowView[] {
  if (!u) return []
  const out: CodexUsageWindowView[] = []
  if (u.primary) {
    out.push({
      key: 'primary',
      minutes: u.primary.windowMinutes,
      percent: Math.round(u.primary.usedPercent ?? 0),
      resetsAt: u.primary.resetsAt ?? undefined,
    })
  }
  if (u.secondary) {
    out.push({
      key: 'secondary',
      minutes: u.secondary.windowMinutes,
      percent: Math.round(u.secondary.usedPercent ?? 0),
      resetsAt: u.secondary.resetsAt ?? undefined,
    })
  }
  return out
}

/**
 * 窗口长度 → 紧凑标签。Codex 的窗口长度由服务端给（plus/pro 是 300 / 10080 分钟），
 * 但别的套餐可能不同，所以不写死两个标签，而是按分钟算：`5h` / `7d` / `45m`。
 * 未知（0 / 负数）→ 空串，调用方据此省略标签只显示百分比。
 */
export function codexWindowLabel(minutes: number): string {
  if (!Number.isFinite(minutes) || minutes <= 0) return ''
  if (minutes % 1440 === 0) return `${minutes / 1440}d`
  if (minutes % 60 === 0) return `${minutes / 60}h`
  return `${Math.round(minutes)}m`
}
