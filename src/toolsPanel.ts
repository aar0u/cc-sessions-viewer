// 工具管理的壳状态：当前 tab、agent 过滤器、搜索词、列表栏宽度。
//
// 和 toolsSkills.ts 同样的理由住在这儿：主区那半边在 `src/views/`，而那个目录在
// `vitest.config.ts:31` 是覆盖率排除的。「哪些 agent 被勾上了」「切 tab 要不要清搜索」
// 这类判断写进 .vue 就再也没人测得到。

import { ref, watch } from 'vue'
import type { Agent, ToolCapabilities } from './types'
import { ALL_AGENTS } from './settings'

/**
 * 五个面板。次序就是 tab 的显示次序。
 *
 * `discover` 紧挨着 `skills`：它装出来的东西就落在 Skills 面板里，两者是同一件事的
 * 「找」和「管」两半。
 */
export const TOOL_TABS = ['mcp', 'skills', 'discover', 'hooks', 'memo'] as const
export type ToolTab = (typeof TOOL_TABS)[number]

/** tab → i18n key。 */
export const TAB_LABEL: Record<ToolTab, string> = {
  mcp: 'tools.tab.mcp',
  skills: 'tools.tab.skills',
  discover: 'tools.tab.discover',
  hooks: 'tools.tab.hooks',
  memo: 'tools.tab.memo',
}

/**
 * tab → 它对应的能力位。
 *
 * 后端 `ToolSurface::capabilities()` 是从各家的路径方法**派生**出来的（阶段 1），
 * 所以这里只是把 tab 映到字段名，不重复声明「谁支持什么」—— 那是会漂的。
 *
 * `discover` 是 `null`：它搜的是 skills.sh，**跟本机装了哪几家 agent 没有关系**。
 * 硬塞一个能力位进去就等于声称「某家 agent 不支持搜索」，那是假的。
 */
export const TAB_CAPABILITY: Record<ToolTab, keyof ToolCapabilities | null> = {
  mcp: 'mcp',
  skills: 'skills',
  discover: null,
  hooks: 'hooks',
  memo: 'globalMemo',
}

/**
 * 这个 tab 认不认左侧那排 agent 勾选。
 *
 * 只有 `discover` 不认 —— 它列的是网上有什么，不是本机哪几家装了什么。不压暗的话
 * 用户会以为「勾掉几家能筛掉一些结果」，点了却毫无反应。
 */
export function tabUsesAgentFilter(tab: ToolTab): boolean {
  return TAB_CAPABILITY[tab] !== null
}

/**
 * 切到这个 tab 要不要把光标送进搜索框。
 *
 * 只有 `discover` 要。另外四个面板一进去就有内容 —— 本机装了什么直接列出来，
 * 搜索框是**筛掉一些**用的，抢焦点会打断「先看看有什么」这个动作。
 * 发现面板一进去是空的，不输入就永远是空的，搜索框是它唯一的入口。
 */
export function tabAutoFocusesSearch(tab: ToolTab): boolean {
  return tab === 'discover'
}

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

export const toolsTab = ref<ToolTab>('skills')
/** 被勾上的 agent。空集 = 全选（见 `isAgentOn`）。 */
export const toolsAgents = ref<Set<Agent>>(new Set())
export const toolsQuery = ref('')

/**
 * 用户在搜索框里按过几次回车。
 *
 * 存在的理由只有一个：「发现」面板的输入是一次 HTTP，在顶栏那 200 ms 之上还叠了
 * 500 ms 防抖；知道要搜什么的人不该干等。按一下回车这个数 +1，面板 watch 到就立刻发。
 *
 * 做成计数器而不是布尔：连按两次回车（第一次超时了想重试）必须是两个事件，
 * 布尔翻不动第二下。顶栏因此**不需要知道**有谁在监听、更不需要知道那边要发网络请求。
 */
export const toolsSearchSubmit = ref(0)

export function submitToolsSearch() {
  toolsSearchSubmit.value += 1
}

/**
 * 配置集弹框开着没有。
 *
 * 住在这儿而不是某个面板里：它跨四个 tab（一个包同时带 MCP / hooks / 全局指令 /
 * skill 清单），入口在顶栏，而顶栏和主区是 `App.vue` 下的两个兄弟节点 —— 没有
 * 一个共同的组件能存这个状态。
 */
export const toolsBundleOpen = ref(false)

/**
 * 这个 agent 现在显不显示。
 *
 * 空集当全选：用户一个个点掉到最后一个时，如果「空集 = 什么都不显示」，面板会突然变空
 * 而看上去像坏了。空集当全选则退化成「没有过滤」，和刚打开时一致。
 */
export function isAgentOn(agent: Agent): boolean {
  return toolsAgents.value.size === 0 || toolsAgents.value.has(agent)
}

/** 勾选 / 取消一个 agent。整体替换 Set 以触发响应式更新。 */
export function toggleToolsAgent(agent: Agent) {
  const next = new Set(toolsAgents.value)
  // 从「全选」状态点第一下，意思是「只看这一个」，不是「除了它都要」。
  if (next.size === 0) {
    next.add(agent)
  } else if (next.has(agent)) {
    next.delete(agent)
  } else {
    next.add(agent)
  }
  toolsAgents.value = next
}

/** 实际生效的 agent 列表（供后续阶段的面板过滤用）。 */
export function activeToolsAgents(): Agent[] {
  return ALL_AGENTS.filter(isAgentOn)
}

/** 过滤器有没有被动过 —— UI 据此决定要不要显示「重置」。 */
export function toolsFilterDirty(): boolean {
  return toolsAgents.value.size > 0 || toolsQuery.value.trim() !== ''
}

/**
 * 只清过滤条件（agent + 搜索词），**不动 tab**。
 *
 * 和 `resetToolsPanel` 的区别就在这儿：那个是「关掉面板」的归零，这个是用户在面板里
 * 点「重置」—— 他想回到「这个 tab 的全部内容」，不是被踢回 Skills。
 */
export function clearToolsFilter() {
  toolsAgents.value = new Set()
  toolsQuery.value = ''
}

/**
 * 切 tab 时清掉搜索词。
 *
 * 四个面板搜的东西完全不是一类（server 名 / skill 名 / hook 事件 / 文件路径），
 * 带着上一个 tab 的关键词过去多半是零结果，用户会以为新 tab 是空的。agent 过滤器
 * 相反 —— 它问的是「我关心哪几家」，跨 tab 一直成立，所以保留。
 */
export function switchToolsTab(tab: ToolTab) {
  if (toolsTab.value === tab) return
  toolsTab.value = tab
  toolsQuery.value = ''
}

/** 关掉面板时把壳状态归零。 */
export function resetToolsPanel() {
  toolsTab.value = 'skills'
  toolsBundleOpen.value = false
  toolsSearchSubmit.value = 0
  clearToolsFilter()
}

// ---------------------------------------------------------------------------
// 打开就选中第一条
// ---------------------------------------------------------------------------

/**
 * 面板挂上之后，等列表第一次有东西，选中第一条。
 *
 * 四个面板都是主从两栏，而扫描是异步的 —— 不挑一条的话，开面板看到的是左边一列
 * 东西、右边一句「没选中任何东西」，用户得先点一下才算真的打开。
 *
 * **只做一次。** MCP 和 Hooks 的 `select()` 是**开关**：再点一下当前那条就取消选中。
 * 如果每次「没选中」都补一条，用户就永远取消不掉了。所以选上、或者用户自己先选了，
 * 这个 watcher 就撤掉。
 *
 * 看的是**过滤后**的列表：屏幕上排第一的那条才是用户以为会被选中的那条。带着上一次
 * 的搜索词进来、当场零结果的话就先不选，等他清掉搜索词、列表第一次有东西时再补。
 *
 * `pickable` 给全局指令那个面板用：它的行点开会**读文件**，而「自己那份还不存在」
 * 的行点开是预填一份模板 —— 那等于开面板就凭空造出一个未保存的改动，下一次切行
 * 就会被拦下来问「要不要丢弃」。
 */
export function selectFirstRow<T>(
  rows: () => T[],
  hasSelection: () => boolean,
  pick: (row: T) => void,
  pickable: (row: T) => boolean = () => true,
): void {
  let done = false
  const step = (list: T[]) => {
    if (done) return
    if (hasSelection()) {
      done = true
      stop()
      return
    }
    const first = list.find(pickable)
    if (!first) return
    done = true
    stop()
    pick(first)
  }
  const stop = watch(rows, step)
  // 挂上的这一刻列表可能已经有东西了（换个 tab 再回来、或者扫描比渲染还快）。
  // `watch` 不带 `immediate` —— 带了的话 `stop` 还在 TDZ 里，撤不掉自己。
  step(rows())
}

// ---------------------------------------------------------------------------
// 健康条上的总数
// ---------------------------------------------------------------------------

/**
 * 健康条左端那个数字。
 *
 * 四个面板原来一律印**扫描到的总数**，而列表是筛过的 —— 搜索词、状态角标、agent
 * 勾选，任何一个生效，「50 个 skill」旁边就只剩十几行，数字和眼睛看到的对不上，
 * 看上去像列表漏了东西。
 *
 * 筛掉过东西就写成 `13/50`，没筛就还是 `50`。这样既不需要每个面板各自去判断
 * 「现在到底算不算筛过」（搜索词、角标、agent 三套状态形状各不相同），也不会在
 * 没筛的时候多出一个 `50/50` 来碍眼 —— 拿两个数一比就够了。
 */
export function shownOfTotal(shown: number, total: number): string {
  return shown === total ? String(total) : `${shown}/${total}`
}

// ---------------------------------------------------------------------------
// 列表栏宽度
// ---------------------------------------------------------------------------

const LIST_WIDTH_KEY = 'toolsListWidth:v1'
export const TOOLS_LIST_MIN_WIDTH = 240
export const TOOLS_LIST_MAX_WIDTH = 560

/**
 * 夹到 [min, max]，再额外给详情栏留 420px。
 *
 * 光有上限不够：窄窗口下 560px 的列表能把详情挤成一条缝，而详情里是路径、链路和
 * 逐条风险点 —— 挤没了这个面板就只剩「有哪些 skill」，正是它想超越的那种列表。
 */
export function clampToolsListWidth(width: number): number {
  const viewportMax = Math.max(TOOLS_LIST_MIN_WIDTH, window.innerWidth - 420)
  return Math.round(
    Math.min(Math.max(width, TOOLS_LIST_MIN_WIDTH), TOOLS_LIST_MAX_WIDTH, viewportMax),
  )
}

function loadToolsListWidth(): number {
  const raw = Number(localStorage.getItem(LIST_WIDTH_KEY))
  return clampToolsListWidth(Number.isFinite(raw) && raw > 0 ? raw : 320)
}

export const toolsListWidth = ref(loadToolsListWidth())

/** 拖拽中不落盘，松手才写 —— 每帧一次 localStorage 写入毫无意义。 */
export function setToolsListWidth(width: number, persist = false) {
  toolsListWidth.value = clampToolsListWidth(width)
  if (persist) localStorage.setItem(LIST_WIDTH_KEY, String(toolsListWidth.value))
}

let resizeStartX = 0
let resizeStartWidth = 0

/**
 * 分隔条的拖拽。逻辑和 `App.vue` 的侧栏 resizer 同形，但住在这儿：
 * 四个面板各自渲染自己的 `.tools-body`，宽度得是它们共用的一份状态。
 */
export function startToolsListResize(e: PointerEvent) {
  e.preventDefault()
  resizeStartX = e.clientX
  resizeStartWidth = toolsListWidth.value
  document.body.classList.add('is-sidebar-resizing')
  window.addEventListener('pointermove', onToolsListResizeMove)
  window.addEventListener('pointerup', endToolsListResize, { once: true })
  window.addEventListener('pointercancel', endToolsListResize, { once: true })
}

function onToolsListResizeMove(e: PointerEvent) {
  setToolsListWidth(resizeStartWidth + e.clientX - resizeStartX)
}

function endToolsListResize() {
  document.body.classList.remove('is-sidebar-resizing')
  setToolsListWidth(toolsListWidth.value, true)
  window.removeEventListener('pointermove', onToolsListResizeMove)
  window.removeEventListener('pointerup', endToolsListResize)
  window.removeEventListener('pointercancel', endToolsListResize)
}

// ---------------------------------------------------------------------------
// 本机装了哪几家
// ---------------------------------------------------------------------------

/**
 * 装了的 agent。空数组表示还没拉到。
 *
 * 这是工具管理和会话列表最大的一处不同：会话那边跟着设置里勾的可见 agent 走，
 * 这边是**全机器的全景**，不受设置控制 —— 所以只能按「装没装」筛。七家全列出来的话，
 * 其中五家点进去永远是空的，用户会以为面板坏了。
 */
export const installedAgents = ref<Agent[]>([])

let installedLoaded = false

/** 打开面板时拉一次。装没装不随 cwd 变，所以只拉一次就够。 */
export async function ensureInstalledAgents(load: () => Promise<{ agent: Agent; installed: boolean }[]>) {
  if (installedLoaded) return
  installedLoaded = true
  try {
    installedAgents.value = (await load()).filter((s) => s.installed).map((s) => s.agent)
  } catch {
    // 拉不到就退回「全都显示」—— 少显示一家等于把用户装了的东西藏了，比多显示糟。
    installedAgents.value = [...ALL_AGENTS]
    installedLoaded = false
  }
}

/** 面板上要列出来的 agent。还没拉到时先给全量，避免开面板那一下闪一次空列表。 */
export function panelAgents(): Agent[] {
  return installedAgents.value.length > 0 ? installedAgents.value : ALL_AGENTS
}
