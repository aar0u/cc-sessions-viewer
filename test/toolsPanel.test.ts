// 工具管理的壳状态。主区那半边在 src/views/（覆盖率排除），判断都在这儿。

import { describe, it, expect, beforeEach } from 'vitest'
import { effectScope, nextTick, ref } from 'vue'
import {
  TOOLS_LIST_MAX_WIDTH,
  TOOLS_LIST_MIN_WIDTH,
  TAB_CAPABILITY,
  tabAutoFocusesSearch,
  tabUsesAgentFilter,
  TAB_LABEL,
  TOOL_TABS,
  activeToolsAgents,
  clearToolsFilter,
  isAgentOn,
  resetToolsPanel,
  switchToolsTab,
  toggleToolsAgent,
  toolsAgents,
  toolsFilterDirty,
  toolsQuery,
  toolsTab,
  clampToolsListWidth,
  selectFirstRow,
  setToolsListWidth,
  shownOfTotal,
  toolsListWidth,
} from '../src/toolsPanel'
import { MCP_STATES } from '../src/toolsMcp'
import { HOOK_STATES } from '../src/toolsHooks'
import { BADGE_ORDER, RISK_ORDER } from '../src/toolsSkills'
import { STEP_KINDS } from '../src/toolsSkillsActions'
import { BUNDLE_CATEGORIES } from '../src/toolsBundle'
import { HIT_SORTS } from '../src/toolsRegistry'
import { ALL_AGENTS, setLang, type Lang } from '../src/settings'
import type {
  FileStatus,
  PreviewErrKind,
  RefHealth,
  RegistryErrKind,
  RiskContext,
  StepNote,
} from '../src/types'
import { t } from '../src/i18n'

/** 这三组联合类型在 types.ts 里没有运行时值，列一遍；漏一个编译就报错。 */
const REF_STATES: RefHealth['state'][] = [
  'realDir',
  'linked',
  'broken',
  'cyclic',
  'managedCopy',
  'copyStale',
  'copyEdited',
  'copyDiverged',
  'copyOrphaned',
  'notADirectory',
]
const RISK_CONTEXTS: RiskContext[] = ['executable', 'comment', 'codeBlock', 'prose']
const FILE_STATUSES: FileStatus[] = ['onlyMain', 'onlyExternal', 'same', 'different']
const STEP_NOTES: StepNote[] = ['deadLink']

beforeEach(() => resetToolsPanel())

/** `McpTransport` / `ConfigScope` / `ConfigOrigin` 的全部取值。类型层的联合类型在运行时
 *  拿不到，只能抄一份 —— 抄漏了下面那条 key 覆盖测试就守不住，所以顺带断言了个数。 */
const MCP_TRANSPORTS = ['stdio', 'http', 'sse', 'ws', 'unknown'] as const
const CONFIG_SCOPES = ['user', 'local', 'project'] as const
const CONFIG_ORIGINS = ['own', 'shared', 'compat'] as const
const MCP_STEP_KINDS = ['add', 'update', 'remove', 'enable', 'disable'] as const
const HOOK_STEP_KINDS = ['add', 'remove'] as const
/** `MemoRowState` 里会画出角标的那几个。`ok` 是常态，不画。 */
const MEMO_ROW_STATES = ['fallback', 'missing', 'unsupported', 'fragment', 'broken'] as const
// 配置集：`MemoTargetKind` / `MemoBlock` / `BundleProblem` 的取值（运行时拿不到类型）。
const MEMO_TARGET_KINDS = ['create', 'overwrite', 'blocked'] as const
const MEMO_BLOCKS = ['unsupported', 'noHome', 'badName'] as const
const BUNDLE_PROBLEMS = ['notJson', 'notBundle', 'tooNew'] as const
// 能被导进来的那三类（skills 只列清单，不落盘）。
const BUNDLE_IMPORTABLE = ['mcp', 'hooks', 'memo'] as const
const MEMO_ROLES = ['own', 'fallback', 'extra'] as const
const HOOK_BLOCK_REASONS = [
  'unsupported',
  'noWritableSource',
  'protected',
  'unknownEvent',
  'notInWritableSource',
  'emptyCommand',
  'alreadyThere',
] as const
/** 接口只回这两种。多词查询会切到语义搜索，两个都得有文案。 */
const SEARCH_TYPES = ['fuzzy', 'semantic'] as const

const REGISTRY_ERR_KINDS: RegistryErrKind[] = ['offline', 'tooShort', 'http', 'badJson']

const PREVIEW_ERR_KINDS: PreviewErrKind[] = [
  'notInstallable',
  'cache',
  'clone',
  'notFound',
  'checkout',
]

const MCP_BLOCK_REASONS = [
  'unsupported',
  'noWritableSource',
  'notInWritableSource',
  'noEnableSwitch',
  'incomplete',
  'remoteReadOnly',
  'headersUnsupported',
] as const

describe('tab', () => {
  it('五个面板，次序固定', () => {
    // discover 夹在 skills 和 hooks 中间：它装出来的东西就落在 Skills 面板里，
    // 两者是同一件事的「找」和「管」两半，隔开只会让人来回跳。
    expect(TOOL_TABS).toEqual(['mcp', 'skills', 'discover', 'hooks', 'memo'])
  })

  it('每个 tab 都有文案，漏一个就会渲染成空白', () => {
    for (const tab of TOOL_TABS) {
      expect(TAB_LABEL[tab]).toMatch(/^tools\.tab\./)
    }
  })

  /**
   * 能力位要么是 `ToolCapabilities` 的合法键，要么**显式**为 null。
   *
   * 原来这条断言写的是 `toBeTruthy()`，那等于规定「每个面板都必须对应某一家 agent
   * 的某个能力」—— discover 搜的是 skills.sh，跟本机装了哪几家毫无关系，硬塞一个
   * 能力位进去就是在声称「某家 agent 不支持搜索」，那是假的。
   *
   * 但也不能干脆不检查：漏写一项会得到 `undefined`，而 `undefined` 和「我想好了，
   * 这个面板跟 agent 无关」长得一样。所以要求 `null` 必须是**写出来**的。
   */
  it('能力位要么是合法字段，要么显式写成 null', () => {
    const fields = ['mcp', 'skills', 'hooks', 'globalMemo']
    for (const tab of TOOL_TABS) {
      expect(tab in TAB_CAPABILITY).toBe(true)
      const cap = TAB_CAPABILITY[tab]
      if (cap !== null) expect(fields).toContain(cap)
    }
    expect(TAB_CAPABILITY.discover).toBeNull()
  })

  /** 只有 discover 不认 agent 勾选 —— 压暗那一排靠的就是这个。 */
  it('只有发现面板不吃 agent 过滤', () => {
    for (const tab of TOOL_TABS) {
      expect(tabUsesAgentFilter(tab)).toBe(tab !== 'discover')
    }
  })

  /**
   * 也只有 discover 抢焦点。另外四个一进去就有内容，抢焦点会打断「先看看有什么」；
   * 发现面板一进去是空的，不输入就永远是空的。
   */
  it('只有发现面板进去就抢搜索框的焦点', () => {
    for (const tab of TOOL_TABS) {
      expect(tabAutoFocusesSearch(tab)).toBe(tab === 'discover')
    }
  })

  it('切 tab 清搜索词，但保留 agent 过滤', () => {
    // 四个面板搜的不是一类东西，带着上一个 tab 的关键词过去多半零结果，
    // 用户会以为新 tab 是空的。而「我关心哪几家」跨 tab 一直成立。
    toolsQuery.value = 'hyperframes'
    toggleToolsAgent('codex')
    switchToolsTab('mcp')
    expect(toolsTab.value).toBe('mcp')
    expect(toolsQuery.value).toBe('')
    expect(isAgentOn('codex')).toBe(true)
    expect(isAgentOn('claude')).toBe(false)
  })

  it('切到当前 tab 不清搜索词', () => {
    toolsQuery.value = 'keep'
    switchToolsTab(toolsTab.value)
    expect(toolsQuery.value).toBe('keep')
  })
})

describe('agent 过滤器', () => {
  it('默认全选', () => {
    expect(toolsAgents.value.size).toBe(0)
    for (const a of ALL_AGENTS) expect(isAgentOn(a)).toBe(true)
    expect(activeToolsAgents()).toEqual(ALL_AGENTS)
  })

  it('从全选点第一下 = 只看这一个', () => {
    toggleToolsAgent('grok')
    expect(activeToolsAgents()).toEqual(['grok'])
  })

  it('再点一下取消，回到全选而不是空面板', () => {
    // 「空集 = 什么都不显示」的话，用户点掉最后一个时面板会突然变空，看上去像坏了。
    toggleToolsAgent('grok')
    toggleToolsAgent('grok')
    expect(toolsAgents.value.size).toBe(0)
    expect(activeToolsAgents()).toEqual(ALL_AGENTS)
  })

  it('可以多选', () => {
    toggleToolsAgent('claude')
    toggleToolsAgent('codex')
    expect(activeToolsAgents()).toEqual(['claude', 'codex'])
  })

  it('生效列表按 ALL_AGENTS 的次序，不是点击次序', () => {
    // 角标那一排在每行都要对齐，次序必须稳定。
    toggleToolsAgent('pi')
    toggleToolsAgent('claude')
    expect(activeToolsAgents()).toEqual(['claude', 'pi'])
  })
})

describe('重置', () => {
  it('动过过滤器才算 dirty', () => {
    expect(toolsFilterDirty()).toBe(false)
    toolsQuery.value = '  '
    expect(toolsFilterDirty()).toBe(false)
    toolsQuery.value = 'x'
    expect(toolsFilterDirty()).toBe(true)
    toolsQuery.value = ''
    toggleToolsAgent('agy')
    expect(toolsFilterDirty()).toBe(true)
  })

  it('重置过滤器不改当前面板', () => {
    // 用户点的是「重置过滤」，不是「回到 Skills」—— 把他踢出当前面板就成了别的动作。
    switchToolsTab('hooks')
    toolsQuery.value = 'x'
    toggleToolsAgent('agy')
    clearToolsFilter()
    expect(toolsTab.value).toBe('hooks')
    expect(toolsQuery.value).toBe('')
    expect(toolsAgents.value.size).toBe(0)
    expect(toolsFilterDirty()).toBe(false)
  })

  it('关掉面板把壳状态全部归零', () => {
    toolsTab.value = 'hooks'
    toolsQuery.value = 'x'
    toggleToolsAgent('agy')
    resetToolsPanel()
    expect(toolsTab.value).toBe('skills')
    expect(toolsQuery.value).toBe('')
    expect(toolsAgents.value.size).toBe(0)
  })
})

describe('列表栏宽度', () => {
  it('夹在上下限之间', () => {
    expect(clampToolsListWidth(10)).toBe(TOOLS_LIST_MIN_WIDTH)
    expect(clampToolsListWidth(9999)).toBe(TOOLS_LIST_MAX_WIDTH)
    expect(clampToolsListWidth(380)).toBe(380)
  })

  it('窗口窄的时候上限再收，给详情留 420px', () => {
    // 只有静态上限的话，600px 宽的窗口里列表能拉到 560，详情剩 40px —— 而详情里
    // 是路径、链路和逐条风险点，挤没了这个面板就退化成一张「有哪些 skill」的清单。
    const w = window.innerWidth
    Object.defineProperty(window, 'innerWidth', { value: 600, configurable: true })
    expect(clampToolsListWidth(560)).toBe(240)
    Object.defineProperty(window, 'innerWidth', { value: w, configurable: true })
  })

  it('松手才落盘，拖拽过程中不写 localStorage', () => {
    localStorage.removeItem('toolsListWidth:v1')
    setToolsListWidth(400)
    expect(toolsListWidth.value).toBe(400)
    expect(localStorage.getItem('toolsListWidth:v1')).toBeNull()
    setToolsListWidth(400, true)
    expect(localStorage.getItem('toolsListWidth:v1')).toBe('400')
  })
})

describe('文案', () => {
  // 缺 key 时 `t()` 原样返回 key 本身（见 i18n.test.ts），UI 上就是一行
  // `tools.tab.mcp` 这样的字面量 —— 不报错、不崩，肉眼不盯着看就发现不了。
  const LANGS: Lang[] = ['en', 'zh', 'zh-TW', 'ja']
  const KEYS = [
    'tools.title',
    'tools.close',
    'tools.back',
    'tools.agentFilter',
    'tools.agentFilterHint',
    'tools.agentFilterOff',
    'tools.filterReset',
    'tools.healthPending',
    'tools.listPane',
    'tools.detailPane',
    'tools.shellOnly',
    'tools.noSelection',
    'sidebar.tools',
    'settings.shortcut.tools',
    ...TOOL_TABS.map((tab) => TAB_LABEL[tab]),
    ...TOOL_TABS.map((tab) => `tools.search.${tab}`),
    // 发现面板。四个错误分类和两档排序都是拼接 key，漏一种只会在界面上蹦出 key 字面量。
    'tools.discover.idle',
    'tools.discover.idleHint',
    'tools.discover.tooShort',
    'tools.discover.loading',
    'tools.discover.empty',
    'tools.discover.found',
    'tools.discover.searchTypeTip',
    'tools.discover.installed',
    'tools.discover.installedTip',
    'tools.discover.badge.installed',
    'tools.discover.badge.blocked',
    'tools.discover.blockedTip',
    'tools.discover.blockedNote',
    'tools.discover.installsTip',
    'tools.discover.retry',
    'tools.discover.detailPending',
    'tools.discover.errDetail',
    'tools.discover.openRepo',
    'tools.discover.openPage',
    'tools.discover.copyAndInstall',
    'tools.discover.closeTerminal',
    'tools.discover.terminal',
    'tools.discover.terminalHint',
    'tools.discover.ran',
    'tools.discover.detail.source',
    'tools.discover.detail.repo',
    'tools.discover.detail.id',
    'tools.discover.detail.installs',
    'tools.discover.detail.path',
    'tools.discover.detail.commit',
    'tools.discover.detail.otherPaths',
    'tools.discover.detail.cached',
    'tools.discover.detail.noFrontmatter',
    'tools.discover.detail.truncated',
    'tools.discover.loadingDetail',
    'tools.discover.refreshPreview',
    ...SEARCH_TYPES.map((k) => `tools.discover.searchType.${k}`),
    ...HIT_SORTS.map((s) => `tools.discover.sort.${s}`),
    ...REGISTRY_ERR_KINDS.map((k) => `tools.discover.err.${k}`),
    ...PREVIEW_ERR_KINDS.map((k) => `tools.discover.perr.${k}`),
    // 详情三节和本地 Skills 面板共用组件，所以它用的也是那边的 key ——
    // 漏一个的表现是发现面板里蹦出 `tools.skills.risk.high` 这样的字面量。
    'tools.skills.findings',
    'tools.skills.frontmatter',
    'tools.skills.files',
    'tools.skills.truncatedRisk',
    'tools.skills.downgraded',
    'tools.skills.loadingDetail',
    // Skills 面板。枚举出来的那几组尤其容易漏：模板里写的是 `tools.skills.step.${s.kind}`
    // 这种拼接 key，少一种取值不会有任何编译期提示，只会在计划框里蹦出一行 key 字面量。
    'tools.skills.total',
    'tools.skills.loading',
    'tools.skills.empty',
    'tools.skills.refresh',
    'tools.skills.mainStore',
    'tools.skills.mainStorePick',
    'tools.skills.mainStoreTitle',
    'tools.skills.storeAdd',
    'tools.skills.storeForget',
    'tools.skills.storeNotEligible',
    'tools.skills.reach.alreadyOn',
    'tools.skills.reach.cannotOff',
    'tools.skills.sharedReach',
    'tools.skills.sharedDirTip',
    'tools.skills.sharedDirBody',
    'tools.skills.badge.fromGit',
    'tools.skills.badgeTip.fromGit',
    'tools.skills.fromGitTip',
    'tools.skills.action.update',
    'tools.skills.action.updateTip',
    'tools.skills.update.title',
    'tools.skills.update.from',
    'tools.skills.update.behind',
    'tools.skills.update.current',
    'tools.skills.update.overwrite',
    'tools.skills.update.untracked',
    'tools.skills.update.ok',
    'tools.skills.update.upToDate',
    'tools.skills.update.done',
    'tools.skills.action.edit',
    'tools.skills.action.editTip',
    'tools.skills.editor.files',
    'tools.skills.editor.pickFile',
    'tools.skills.editor.tooManyFiles',
    'tools.skills.editor.edit',
    'tools.skills.editor.preview',
    'tools.skills.editor.save',
    'tools.skills.editor.saveTip',
    'tools.skills.editor.external',
    'tools.skills.editor.externalTip',
    'tools.skills.editor.binary',
    'tools.skills.editor.tooBig',
    'tools.skills.editor.fmMultiline',
    'tools.skills.editor.fmMultilineTip',
    'tools.skills.editor.discardTitle',
    'tools.skills.editor.discardMsg',
    'tools.skills.editor.discardOk',
    'tools.skills.conflict.diffClipped',
    'tools.skills.realDirs',
    'tools.skills.needMainStore',
    'tools.skills.noBody',
    'tools.skills.noSkillsSupport',
    'tools.skills.inMain',
    'tools.skills.thirdParty',
    'tools.skills.missing',
    'tools.skills.truncated',
    'tools.skills.truncatedRisk',
    'tools.skills.fileCount',
    'tools.skills.bodies',
    'tools.skills.refs',
    'tools.skills.findings',
    'tools.skills.files',
    'tools.skills.dirCount',
    'tools.skills.frontmatter',
    'tools.skills.enabledIn',
    'tools.skills.keepBodies',
    'tools.skills.keepBodiesTip',
    'tools.skills.keepAsk.onTitle',
    'tools.skills.keepAsk.onMsg',
    'tools.skills.keepAsk.offTitle',
    'tools.skills.keepAsk.offMsg',
    'tools.skills.keepAsk.ok',
    'tools.skills.downgraded',
    ...BADGE_ORDER.map((b) => `tools.skills.badge.${b}`),
    ...BADGE_ORDER.map((b) => `tools.skills.badgeTip.${b}`),
    ...RISK_ORDER.map((r) => `tools.skills.risk.${r}`),
    ...REF_STATES.map((s) => `tools.skills.state.${s}`),
    ...REF_STATES.map((s) => `tools.skills.health.${s}`),
    ...(['resync', 'resyncTip', 'resynced'] as const).map((k) => `tools.skills.action.${k}`),
    'tools.skills.copyManual',
    ...RISK_CONTEXTS.map((c) => `tools.skills.context.${c}`),
    ...STEP_KINDS.map((k) => `tools.skills.step.${k}`),
    ...STEP_NOTES.map((n) => `tools.skills.note.${n}`),
    ...(
      [
        'adopt',
        'adoptTip',
        'adoptAll',
        'adoptAllTip',
        'repair',
        'repairTip',
        'delete',
        'deleteBody',
        'deleteBodyTip',
        'enable',
        'disable',
      ] as const
    ).map((a) => `tools.skills.action.${a}`),
    ...(
      [
        'apply',
        'applied',
        'nothing',
        'irreversible',
        'backupNote',
        'countMove',
        'countLink',
        'countUnlink',
        'countDelete',
      ] as const
    ).map((p) => `tools.skills.plan.${p}`),
    ...(
      [
        'title',
        'sub',
        'remaining',
        'mainSide',
        'externalSide',
        'keepMain',
        'useExternal',
        'keepBoth',
        'skip',
        'skipAll',
        'apply',
        'sameCount',
        'truncated',
        'diffTooBig',
      ] as const
    ).map((c) => `tools.skills.conflict.${c}`),
    ...FILE_STATUSES.map((s) => `tools.skills.conflict.file.${s}`),

    // MCP 面板。`transport` / `state` / `reach` / `scope` / `origin` 五组都是模板里
    // 拼出来的 key，少一种取值只会在界面上蹦出一行 key 字面量，编译期一点提示都没有。
    'tools.mcp.total',
    'tools.mcp.loading',
    'tools.mcp.empty',
    'tools.mcp.refresh',
    'tools.mcp.toolCount',
    'tools.mcp.tokenApprox',
    'tools.mcp.unmeasured',
    'tools.mcp.conflictNote',
    'tools.mcp.incompleteNote',
    'tools.mcp.shadowedNote',
    'tools.conditional',
    'tools.mcp.showSecret',
    'tools.mcp.hideSecret',
    'tools.mcp.sourceError',
    ...MCP_STATES.map((s) => `tools.mcp.state.${s}`),
    ...MCP_STATES.map((s) => `tools.mcp.stateTip.${s}`),
    ...MCP_TRANSPORTS.map((s) => `tools.mcp.transport.${s}`),
    ...(['title', 'value', 'tip', 'unmeasured'] as const).map((k) => `tools.mcp.budget.${k}`),
    ...(
      ['definition', 'transport', 'command', 'url', 'cwd', 'noCommand', 'reach', 'shadowed'] as const
    ).map((k) => `tools.mcp.detail.${k}`),
    ...(['on', 'off', 'absent', 'onTip', 'offTip', 'absentTip'] as const).map(
      (k) => `tools.mcp.reach.${k}`,
    ),
    ...CONFIG_SCOPES.map((s) => `tools.scope.${s}`),
    ...CONFIG_ORIGINS.map((s) => `tools.origin.${s}`),

    // 共用确认框的固定文案。
    'tools.plan.blockedTitle',

    // MCP 写。`plan.kind` / `plan.count` / `block` 三组也是模板里拼出来的。
    'tools.mcp.syncNote',
    'tools.mcp.syncOnTip',
    'tools.mcp.syncOffTip',
    ...(
      [
        'add',
        'edit',
        'editTip',
        'sync',
        'unsync',
        'enable',
        'disable',
        'toggleTip',
        'remove',
        'removeTip',
      ] as const
    ).map((k) => `tools.mcp.action.${k}`),
    ...MCP_STEP_KINDS.map((k) => `tools.mcp.plan.kind.${k}`),
    ...MCP_STEP_KINDS.map((k) => `tools.mcp.plan.count.${k}`),
    ...(
      [
        'newFile',
        'shadowed',
        'removeNote',
        'backupNote',
        'apply',
        'applied',
        'nothing',
      ] as const
    ).map((k) => `tools.mcp.plan.${k}`),
    ...MCP_BLOCK_REASONS.map((r) => `tools.mcp.block.${r}`),
    ...(
      [
        'addTitle',
        'editTitle',
        'name',
        'namePlaceholder',
        'nameInvalid',
        'nameTaken',
        'command',
        'commandPlaceholder',
        'cwd',
        'cwdPlaceholder',
        'env',
        'addVar',
        'dropVar',
        'varKey',
        'varValue',
        'targets',
        'noTarget',
        'save',
      ] as const
    ).map((k) => `tools.mcp.form.${k}`),

    // Hooks。`state` / `stateTip` / `plan.kind` / `plan.count` / `block` 都是模板里
    // 拼出来的，所以照着联合类型抄一份来查。
    ...(
      [
        'loading',
        'total',
        'defs',
        'defsTip',
        'empty',
        'refresh',
        'events',
        'eventsTip',
        'matcherAll',
        'sourceError',
        'managedNote',
        'disabledTag',
        'timeoutTip',
        'timeoutValue',
        'writeTo',
        'noWritePath',
        'notSupported',
      ] as const
    ).map((k) => `tools.hooks.${k}`),
    ...HOOK_STATES.map((s) => `tools.hooks.state.${s}`),
    ...HOOK_STATES.map((s) => `tools.hooks.stateTip.${s}`),
    ...(['command', 'reach', 'kind', 'missing'] as const).map((k) => `tools.hooks.detail.${k}`),
    ...(
      ['add', 'remove', 'removeTip', 'removeLocked', 'removeOne', 'test', 'testTip', 'testKind', 'reset', 'resetTip'] as const
    ).map((k) => `tools.hooks.action.${k}`),
    ...HOOK_STEP_KINDS.map((k) => `tools.hooks.plan.kind.${k}`),
    ...HOOK_STEP_KINDS.map((k) => `tools.hooks.plan.count.${k}`),
    ...(
      [
        'newFile',
        'removeNote',
        'backupNote',
        'apply',
        'applied',
        'nothing',
        'addTitle',
        'removeTitle',
      ] as const
    ).map((k) => `tools.hooks.plan.${k}`),
    ...HOOK_BLOCK_REASONS.map((r) => `tools.hooks.block.${r}`),
    ...(
      [
        'title',
        'command',
        'commandPlaceholder',
        'commandHint',
        'events',
        'eventsHint',
        'matcher',
        'matcherPlaceholder',
        'matcherHint',
        'timeout',
        'timeoutPlaceholder',
        'timeoutHint',
        'agents',
        'noTarget',
        'save',
        'needCommand',
        'needEvent',
        'needAgent',
        'eventAgents',
      ] as const
    ).map((k) => `tools.hooks.form.${k}`),
    ...(
      [
        'title',
        'note',
        'event',
        'run',
        'running',
        'payload',
        'stdout',
        'stderr',
        'empty',
        'exit',
        'killed',
        'duration',
        'timedOut',
        'blocking',
        'close',
      ] as const
    ).map((k) => `tools.hooks.test.${k}`),

    // 全局配置。`state` / `role` 是模板里拼出来的，照着 `MemoRowState` / `MemoRole`
    // 抄一份来查。
    ...(
      [
        'loading',
        'empty',
        'refresh',
        'total',
        'present',
        'presentTip',
        'broken',
        'brokenTip',
        'forks',
        'forksTip',
        'pickFile',
        'dirty',
        'saved',
        'unsupportedNote',
        'missingNote',
        'fallbackNote',
        'takeoverNote',
        'cascade',
      ] as const
    ).map((k) => `tools.memo.${k}`),
    ...MEMO_ROW_STATES.map((s) => `tools.memo.state.${s}`),
    ...MEMO_ROLES.map((r) => `tools.memo.role.${r}`),
    ...(
      ['save', 'saveTip', 'external', 'externalTip', 'takeover', 'takeoverTip'] as const
    ).map((k) => `tools.memo.action.${k}`),
    'tools.memo.detail.imports',
    ...(
      ['nested', 'brokenTarget', 'unresolved'] as const
    ).map((k) => `tools.memo.imports.${k}`),
    ...(
      [
        'title',
        'compare',
        'diffTitle',
        'sync',
        'syncTip',
        'synced',
        'same',
        'tooBig',
        'clipped',
        'close',
      ] as const
    ).map((k) => `tools.memo.fork.${k}`),
    ...(
      ['msg', 'diff', 'reload', 'reloadTip', 'reloadDirty'] as const
    ).map((k) => `tools.memo.stale.${k}`),
    ...(['title', 'left', 'right'] as const).map((k) => `tools.memo.conflict.${k}`),
    ...(
      ['discardTitle', 'discardMsg', 'discardOk'] as const
    ).map((k) => `tools.memo.${k}`),

    // 配置集。`memo.*` / `block.*` / `problem.*` 是模板里按取值拼出来的，照着
    // `MemoTargetKind` / `MemoBlock` / `BundleProblem` 抄一份来查。
    ...(
      ['open', 'title', 'close', 'count', 'chars', 'loading'] as const
    ).map((k) => `tools.bundle.${k}`),
    ...(['export', 'import'] as const).map((k) => `tools.bundle.mode.${k}`),
    ...(
      ['intro', 'save', 'saved', 'empty'] as const
    ).map((k) => `tools.bundle.export.${k}`),
    ...(['title', 'why', 'none'] as const).map((k) => `tools.bundle.redacted.${k}`),
    ...(['note', 'local'] as const).map((k) => `tools.bundle.skills.${k}`),
    'tools.bundle.item.agents',
    ...(
      ['pick', 'from', 'agents', 'noAgents', 'apply', 'nothing', 'done', 'empty', 'readOnly'] as const
    ).map((k) => `tools.bundle.import.${k}`),
    ...BUNDLE_PROBLEMS.map((k) => `tools.bundle.problem.${k}`),
    ...MEMO_TARGET_KINDS.map((k) => `tools.bundle.memo.${k}`),
    ...(['overwriteNote', 'fragment', 'note'] as const).map((k) => `tools.bundle.memo.${k}`),
    ...MEMO_BLOCKS.map((k) => `tools.bundle.block.${k}`),
    ...(
      ['title', 'apply', 'backupNote', 'dangerNote'] as const
    ).map((k) => `tools.bundle.plan.${k}`),
    ...BUNDLE_IMPORTABLE.map((k) => `tools.bundle.plan.count.${k}`),
    ...(['title', 'why'] as const).map((k) => `tools.bundle.missing.${k}`),
    ...(['title', 'why'] as const).map((k) => `tools.bundle.args.${k}`),
    // 四个类别名直接借 tab 的 —— 弹框里那四行标题走的就是 `TAB_LABEL`。
    ...BUNDLE_CATEGORIES.map((c) => TAB_LABEL[c]),
  ]

  it('抄下来的那几组取值没有漏项', () => {
    // 上面三个常量是从 `types.ts` 的联合类型手抄的（运行时拿不到类型）。抄漏一个，
    // key 覆盖测试就跟着少查一条，而界面上会蹦出一行 key 字面量。个数写死在这儿，
    // 联合类型加成员时这条先红。
    expect(MCP_TRANSPORTS).toHaveLength(5)
    expect(CONFIG_SCOPES).toHaveLength(3)
    expect(CONFIG_ORIGINS).toHaveLength(3)
    expect(MCP_STATES).toHaveLength(4)
    expect(MCP_STEP_KINDS).toHaveLength(5)
    expect(MCP_BLOCK_REASONS).toHaveLength(7)
    expect(HOOK_STATES).toHaveLength(3)
    expect(HOOK_STEP_KINDS).toHaveLength(2)
    expect(HOOK_BLOCK_REASONS).toHaveLength(7)
    expect(MEMO_ROW_STATES).toHaveLength(5)
    expect(MEMO_ROLES).toHaveLength(3)
    expect(MEMO_TARGET_KINDS).toHaveLength(3)
    expect(MEMO_BLOCKS).toHaveLength(3)
    expect(BUNDLE_PROBLEMS).toHaveLength(3)
    // 配置集的四类必须都是真实存在的面板（弹框里那四行标题直接用面板的文案），
    // 但**不是每个面板都有东西可打包** —— discover 搜的是网上有什么，本机没有对应
    // 的配置可导出。所以是子集关系，不是相等。
    expect(BUNDLE_CATEGORIES.every((c) => (TOOL_TABS as readonly string[]).includes(c))).toBe(true)
    expect([...TOOL_TABS].filter((tab) => !BUNDLE_CATEGORIES.includes(tab as never))).toEqual([
      'discover',
    ])
    expect(BUNDLE_IMPORTABLE).toHaveLength(3)
  })

  it('面板用到的每个 key 在四种语言里都有', () => {
    for (const l of LANGS) {
      setLang(l)
      for (const key of KEYS) {
        expect(t(key), `${l} is missing ${key}`).not.toBe(key)
      }
    }
    setLang('en')
  })

  it('带变量的文案真的插了值', () => {
    setLang('en')
    expect(t('tools.agentFilterHint', { n: '3' })).toContain('3')
    expect(t('tools.shellOnly', { tab: 'Skills' })).toContain('Skills')
    setLang('zh')
    expect(t('tools.agentFilterHint', { n: '3' })).toContain('3')
    setLang('en')
  })

  it('写操作文案的变量位在四种语言里都保住了', () => {
    // 计划框里「执行 N 步」的 N、断链提示里的目标路径 —— 少插一个值，用户看到的是
    // 「执行 步」或者「指向 ，那里不存在」，句子还通顺，错得看不出来。
    for (const l of LANGS) {
      setLang(l)
      expect(t('tools.skills.plan.apply', { n: '7' })).toContain('7')
      expect(t('tools.skills.plan.countDelete', { n: '2' })).toContain('2')
      expect(t('tools.skills.health.broken', { target: '~/x/y' })).toContain('~/x/y')
      expect(t('tools.skills.health.managedCopy', { source: '~/a/b' })).toContain('~/a/b')
      expect(t('tools.skills.conflict.title', { name: 'pdf' })).toContain('pdf')
      expect(t('tools.skills.conflict.remaining', { n: '5' })).toContain('5')
      expect(t('tools.skills.bodies', { n: '3' })).toContain('3')
      expect(t('tools.skills.noSkillsSupport', { agent: 'Pi' })).toContain('Pi')
    }
    setLang('en')
  })

  it('降级说明把两个变量都摆进同一句', () => {
    // `{base}` 和 `{context}` 是一句话的两半：「本身是高危，因为出现在代码块里而降级」。
    // 漏掉 context 那半就变成一句自相矛盾的「本身是高危」。
    for (const l of LANGS) {
      setLang(l)
      const line = t('tools.skills.downgraded', {
        base: t('tools.skills.risk.high'),
        context: t('tools.skills.context.codeBlock'),
      })
      expect(line).toContain(t('tools.skills.risk.high'))
      expect(line).toContain(t('tools.skills.context.codeBlock'))
    }
    setLang('en')
  })
})


// ---------------------------------------------------------------------------

/**
 * 打开面板就选中第一条。
 *
 * 四个面板都是主从两栏、扫描是异步的 —— 不挑一条的话，开面板看到的是左边一列东西、
 * 右边一句「没选中任何东西」。
 */
describe('selectFirstRow', () => {
  /** 在一个独立的 scope 里跑，测完就把 watcher 收掉。 */
  function run<T>(
    rows: { value: T[] },
    picked: { value: T | null },
    pickable?: (row: T) => boolean,
  ) {
    const scope = effectScope()
    scope.run(() =>
      selectFirstRow(
        () => rows.value,
        () => picked.value !== null,
        (r) => {
          picked.value = r
        },
        pickable,
      ),
    )
    return () => scope.stop()
  }

  it('列表还空着就先不选，等它第一次有东西', async () => {
    const rows = ref<string[]>([])
    const picked = ref<string | null>(null)
    const stop = run(rows, picked)
    expect(picked.value).toBeNull()

    rows.value = ['a', 'b']
    await nextTick()
    expect(picked.value).toBe('a')
    stop()
  })

  /** 扫描比渲染还快（换个 tab 再回来就是）—— 挂上的这一刻列表已经有东西了。 */
  it('挂上时列表已经有东西就当场选', () => {
    const rows = ref(['a', 'b'])
    const picked = ref<string | null>(null)
    const stop = run(rows, picked)
    expect(picked.value).toBe('a')
    stop()
  })

  it('用户已经选了就不插手', async () => {
    const rows = ref<string[]>([])
    const picked = ref<string | null>('b')
    const stop = run(rows, picked)
    rows.value = ['a', 'b']
    await nextTick()
    expect(picked.value).toBe('b')
    stop()
  })

  /**
   * 这条是这个函数最要紧的一条。MCP 和 Hooks 的 `select()` 是**开关**：再点一下当前
   * 那条就取消选中。每次「没选中」都补一条的话，用户就永远取消不掉了。
   */
  it('只做一次 —— 用户取消选中之后不再补', async () => {
    const rows = ref(['a', 'b'])
    const picked = ref<string | null>(null)
    const stop = run(rows, picked)
    expect(picked.value).toBe('a')

    picked.value = null
    rows.value = ['a', 'b', 'c']
    await nextTick()
    expect(picked.value).toBeNull()
    stop()
  })

  /**
   * 全局指令面板用这个：它的行点开会**读文件**，而「自己那份还不存在」的行点开是
   * 预填一份模板 —— 自动点等于开面板就凭空造出一个未保存的改动。
   */
  it('跳过不能自动选的那几行', async () => {
    const rows = ref<string[]>([])
    const picked = ref<string | null>(null)
    const stop = run(rows, picked, (r) => r !== 'missing')
    rows.value = ['missing', 'ok']
    await nextTick()
    expect(picked.value).toBe('ok')
    stop()
  })

  it('一行都不能自动选就一条都不选', async () => {
    const rows = ref<string[]>([])
    const picked = ref<string | null>(null)
    const stop = run(rows, picked, () => false)
    rows.value = ['missing', 'broken']
    await nextTick()
    expect(picked.value).toBeNull()
    stop()
  })

  /** 带着上一个 tab 的搜索词进来、当场零结果 —— 清掉搜索词之后还得补上。 */
  it('先是零结果，等列表真的有东西时再选', async () => {
    const all = ['a', 'b']
    const query = ref('zzz')
    const rows = ref<string[]>([])
    const picked = ref<string | null>(null)
    const stop = run(rows, picked)
    rows.value = all.filter((r) => r.includes(query.value))
    await nextTick()
    expect(picked.value).toBeNull()

    query.value = ''
    rows.value = all.filter((r) => r.includes(query.value))
    await nextTick()
    expect(picked.value).toBe('a')
    stop()
  })
})

describe('健康条上的总数', () => {
  it('没筛就只写一个数', () => {
    expect(shownOfTotal(50, 50)).toBe('50')
  })

  it('筛过就写成 shown/total —— 「50 个 skill」旁边只有 13 行是在骗人', () => {
    expect(shownOfTotal(13, 50)).toBe('13/50')
  })

  it('一条都没剩下也照样写出来，不然就只剩一个空列表让人猜', () => {
    expect(shownOfTotal(0, 50)).toBe('0/50')
  })

  it('什么都没扫到时不出现 0/0', () => {
    expect(shownOfTotal(0, 0)).toBe('0')
  })
})
