// 工具管理 · MCP 纯逻辑。
//
// 装置数据照着本机实测的形状造：`~/.claude.json` 里三条（其中一条只有 env、起不来），
// 项目 `.mcp.json` 里同名但参数不同的一条（真冲突），codex 的 TOML 里一条被
// `enabled = false` 关掉的，以及 grok 兼容读 Claude 那份。

import { describe, it, expect, beforeEach } from 'vitest'
import type {
  Agent,
  McpDefAt,
  McpEntry,
  McpScan,
  McpServerDef,
  McpSource,
  McpWriteReport,
} from '../src/types'
import { t } from '../src/i18n'
import { setLang } from '../src/settings'
import { toolsAgents } from '../src/toolsPanel'
import {
  BUDGET_DANGER,
  BUDGET_WARN,
  CONTEXT_BUDGET,
  MCP_STATES,
  agentReach,
  blankInput,
  budgetLevel,
  budgetRatio,
  commandLine,
  defToInput,
  definingPaths,
  effectiveDefs,
  enableEdits,
  formEdits,
  isDestructive,
  queryCommand,
  maskValue,
  mcpApplyOutcome,
  mcpPlanView,
  mcpState,
  parseCommandLine,
  primaryDef,
  putEdits,
  removeEdits,
  shadowedDefs,
  stateCounts,
  stepCounts,
  syncEdit,
  syncState,
  toggleState,
  urlDisplay,
  urlHasSecret,
  validName,
  varDisplay,
  visibleServers,
} from '../src/toolsMcp'

const HOME = '/Users/x'
const CLAUDE_JSON = `${HOME}/.claude.json`
const PROJECT_MCP = `${HOME}/repo/.mcp.json`
const CODEX_TOML = `${HOME}/.codex/config.toml`

function source(path: string, over: Partial<McpSource> = {}): McpSource {
  return {
    path,
    scope: 'user',
    origin: 'own',
    format: path.endsWith('.toml') ? 'tomlServers' : 'jsonServers',
    writable: false,
    exists: true,
    precedence: 100,
    conditional: false,
    ...over,
  }
}

function def(over: Partial<McpServerDef> = {}): McpServerDef {
  return {
    name: 'x',
    transport: 'stdio',
    command: 'npx',
    args: ['foo'],
    url: null,
    urlMasked: null,
    env: [],
    headers: [],
    cwd: null,
    enabled: true,
    incomplete: false,
    ...over,
  }
}

function at(agent: Agent, src: McpSource, d: McpServerDef, effective = true): McpDefAt {
  return { agent, source: src, def: d, effective }
}

function entry(name: string, defs: McpDefAt[], over: Partial<McpEntry> = {}): McpEntry {
  const agents: Agent[] = []
  const inactive: Agent[] = []
  for (const d of defs) {
    const list = d.effective && d.def.enabled ? agents : inactive
    if (!list.includes(d.agent)) list.push(d.agent)
  }
  return {
    name,
    defs,
    agents,
    inactiveAgents: inactive.filter((a) => !agents.includes(a)),
    conflict: false,
    fingerprints: [],
    tools: null,
    tokens: null,
    ...over,
  }
}

/** `chrome-devtools`：项目里那份生效、user 级那份被盖住，两份参数不同 → 冲突。 */
const chrome = entry(
  'chrome-devtools',
  [
    at(
      'claude',
      source(PROJECT_MCP, { scope: 'project', origin: 'shared', precedence: 200 }),
      def({ name: 'chrome-devtools', args: ['-y', 'chrome-devtools-mcp@latest'] }),
    ),
    at(
      'claude',
      source(CLAUDE_JSON),
      def({ name: 'chrome-devtools', args: ['chrome-devtools-mcp@latest'] }),
      false,
    ),
    at(
      'grok',
      source(CLAUDE_JSON, { origin: 'compat', precedence: 300 }),
      def({ name: 'chrome-devtools', args: ['chrome-devtools-mcp@latest'] }),
    ),
  ],
  { conflict: true, fingerprints: ['a', 'b'], tools: 29, tokens: 5726 },
)

/** 只有 env、既没 command 也没 url —— 本机 `~/.claude.json` 里真有这么一条。 */
const tapd = entry('tapd', [
  at(
    'claude',
    source(CLAUDE_JSON),
    def({ name: 'tapd', command: null, args: [], incomplete: true, transport: 'unknown', env: [
      { key: 'TAPD_ACCESS_TOKEN', value: '9ae39deca75e3bae60', secret: true },
      { key: 'TAPD_BASE', value: 'https://tapd.example', secret: false },
    ] }),
  ),
])

/** codex 的 `enabled = false`：配置里有，没人在跑。 */
const computerUse = entry('computer-use', [
  at(
    'codex',
    source(CODEX_TOML),
    def({ name: 'computer-use', command: '/Applications/x', args: ['mcp'], enabled: false }),
  ),
])

const scan: McpScan = {
  home: HOME,
  servers: [chrome, computerUse, tapd],
  agents: [],
  summary: { servers: 3, active: 1, conflicts: 1, tools: 29, tokens: 5726, measured: 1 },
}

beforeEach(() => {
  toolsAgents.value = new Set()
})

describe('状态判定', () => {
  it('同名不同命令行优先报冲突 —— 各家跑的根本不是一个东西', () => {
    expect(mcpState(chrome)).toBe('conflict')
  })

  it('既没 command 也没 url 的报「配置残缺」，不报成正常', () => {
    // 画成正常的就是告诉用户「它在跑」，而它根本起不来。
    expect(mcpState(tapd)).toBe('incomplete')
  })

  it('没有任何一家在用的报「关着」', () => {
    expect(mcpState(computerUse)).toBe('off')
  })

  it('状态表覆盖了所有状态，漏一个健康条上就少一格', () => {
    const seen = new Set(scan.servers.map(mcpState))
    for (const s of seen) expect(MCP_STATES).toContain(s)
    expect(MCP_STATES).toHaveLength(4)
  })

  it('被盖住的那份残缺不影响结论 —— 跑起来的不是它', () => {
    const shadowedBroken = entry('x', [
      at('claude', source(PROJECT_MCP, { precedence: 200 }), def()),
      at('claude', source(CLAUDE_JSON), def({ incomplete: true }), false),
    ])
    expect(mcpState(shadowedBroken)).toBe('on')
  })
})

describe('覆盖关系', () => {
  it('生效的那份说了算，被盖住的那份不改变结论', () => {
    const reach = agentReach(chrome, 'claude')
    expect(reach.state).toBe('on')
    expect(reach.state === 'on' && reach.def.source.path).toBe(PROJECT_MCP)
  })

  it('显式关掉的报 off，不是 absent —— 两者在 UI 上是两回事', () => {
    expect(agentReach(computerUse, 'codex').state).toBe('off')
    expect(agentReach(computerUse, 'claude').state).toBe('absent')
  })

  it('被盖住的定义单独列出来，用户才知道为什么改了没反应', () => {
    const shadowed = shadowedDefs(chrome, 'claude')
    expect(shadowed).toHaveLength(1)
    expect(shadowed[0].source.path).toBe(CLAUDE_JSON)
    expect(shadowedDefs(chrome, 'grok')).toHaveLength(0)
  })

  it('定义它的文件按优先级从高到低列出来，同一个文件只算一次', () => {
    // grok 和 claude 都读 `~/.claude.json`，列两遍就是在说有两个文件。
    expect(definingPaths(chrome)).toEqual([CLAUDE_JSON, PROJECT_MCP])
  })

  it('生效的定义每家至多一条', () => {
    const byAgent = new Map<string, number>()
    for (const d of effectiveDefs(chrome)) {
      byAgent.set(d.agent, (byAgent.get(d.agent) ?? 0) + 1)
    }
    for (const [agent, n] of byAgent) expect(n, agent).toBe(1)
  })
})

describe('展示', () => {
  it('命令行把命令和参数拼起来，远端显示 URL', () => {
    expect(commandLine(def({ command: 'npx', args: ['-y', 'foo'] }))).toBe('npx -y foo')
    const plain = 'https://e.com/mcp'
    expect(commandLine(def({ command: null, args: [], url: plain, urlMasked: plain }))).toBe(plain)
  })

  /**
   * 列表行上**没有**「点一下看原文」这个动作，所以那儿永远只能是打过码的那份。
   * 后端没给打码版本时宁可整条不显示 —— 那是后端漏填了，不是可以顺手漏出原文的理由。
   */
  it('列表行上的远端地址只用打过码的那份', () => {
    const url = 'https://bob:s3cr3t@e.com/mcp'
    expect(commandLine(def({ command: null, url, urlMasked: 'https://bob:••••@e.com/mcp' }))).toBe(
      'https://bob:••••@e.com/mcp',
    )
    expect(commandLine(def({ command: null, url, urlMasked: null }))).toBe('••••')
  })

  it('详情页那条地址点开才给原文', () => {
    const url = 'https://e.com/mcp?api_key=abc123'
    const masked = 'https://e.com/mcp?api_key=••••'
    const withSecret = def({ command: null, url, urlMasked: masked })
    expect(urlDisplay(withSecret, false)).toBe(masked)
    expect(urlDisplay(withSecret, true)).toBe(url)
    expect(urlHasSecret(withSecret)).toBe(true)
  })

  /** 没东西可打码的地址就是一段普通文本 —— 做成可点的只会让人以为后面还藏着什么。 */
  it('地址里没凭据时不做成可点的', () => {
    const plain = 'https://e.com/mcp'
    expect(urlHasSecret(def({ command: null, url: plain, urlMasked: plain }))).toBe(false)
    expect(urlHasSecret(def({ command: 'npx' }))).toBe(false)
  })

  it('全被关掉时详情页仍拿得到一份定义 —— 不能给一片空白', () => {
    expect(primaryDef(computerUse)?.name).toBe('computer-use')
    expect(primaryDef(entry('empty', []))).toBeNull()
  })

  it('凭据默认打码，点开才给原文', () => {
    const [token, base] = tapd.defs[0].def.env
    expect(varDisplay(token, false)).toBe('9a••••60')
    expect(varDisplay(token, true)).toBe('9ae39deca75e3bae60')
    // 不像凭据的照常显示，不然满屏点点点谁也看不出配了什么。
    expect(varDisplay(base, false)).toBe('https://tapd.example')
  })

  it('打码的点数固定，不泄漏值有多长', () => {
    expect(maskValue('a'.repeat(20))).toBe(maskValue('a'.repeat(60)).replace(/a/g, 'a'))
    expect(maskValue('a'.repeat(20)).length).toBe(maskValue('b'.repeat(99)).length)
    // 太短的一个字符都不露：露掉头尾就只剩没露的那两个了。
    expect(maskValue('12345678')).toBe('••••')
  })
})

describe('预算条', () => {
  it('比例封顶在 1，否则条子会画出容器', () => {
    expect(budgetRatio(CONTEXT_BUDGET * 2)).toBe(1)
    expect(budgetRatio(-5)).toBe(0)
    expect(budgetRatio(CONTEXT_BUDGET / 2)).toBeCloseTo(0.5)
  })

  it('过了阈值就变色', () => {
    expect(budgetLevel(0)).toBe('ok')
    expect(budgetLevel(CONTEXT_BUDGET * BUDGET_WARN)).toBe('warn')
    expect(budgetLevel(CONTEXT_BUDGET * BUDGET_DANGER)).toBe('danger')
    expect(budgetLevel(CONTEXT_BUDGET * (BUDGET_WARN - 0.001))).toBe('ok')
  })
})

describe('过滤', () => {
  it('按名字和命令行搜', () => {
    const all = { state: null }
    expect(visibleServers(scan, 'chrome', all).map((s) => s.name)).toEqual(['chrome-devtools'])
    // 命令行里搜得到：用户常记得装的是哪个包，不记得给它起了什么名字。
    expect(visibleServers(scan, '/Applications', all).map((s) => s.name)).toEqual(['computer-use'])
    expect(visibleServers(scan, '', all)).toHaveLength(3)
  })

  it('按状态过滤，点第二下取消', () => {
    expect(visibleServers(scan, '', { state: 'off' }).map((s) => s.name)).toEqual(['computer-use'])
    expect(toggleState({ state: null }, 'off')).toEqual({ state: 'off' })
    expect(toggleState({ state: 'off' }, 'off')).toEqual({ state: null })
  })

  it('agent 过滤器按「这家沾过它」判，关着的也算', () => {
    // codex 那条是关着的。勾上 codex 却看不见它，用户就没法把它打开。
    toolsAgents.value = new Set<Agent>(['codex'])
    expect(visibleServers(scan, '', { state: null }).map((s) => s.name)).toEqual(['computer-use'])
    toolsAgents.value = new Set<Agent>(['grok'])
    expect(visibleServers(scan, '', { state: null }).map((s) => s.name)).toEqual(['chrome-devtools'])
  })

  it('一家 agent 都不沾的不会被 agent 过滤器筛掉', () => {
    // 占着配置却没人读的那种，正是用户最该看见的。
    const orphan = entry('orphan', [])
    const withOrphan: McpScan = { ...scan, servers: [...scan.servers, orphan] }
    toolsAgents.value = new Set<Agent>(['pi'])
    expect(visibleServers(withOrphan, '', { state: null }).map((s) => s.name)).toEqual(['orphan'])
  })

  it('计数和列表是同一份判定', () => {
    const counts = stateCounts(scan)
    expect(counts).toEqual({ conflict: 1, incomplete: 1, off: 1, on: 0 })
    for (const state of MCP_STATES) {
      expect(visibleServers(scan, '', { state })).toHaveLength(counts[state])
    }
  })

  it('还没扫出来时是空列表，不是崩', () => {
    expect(visibleServers(null, '', { state: null })).toEqual([])
    expect(stateCounts(null)).toEqual({ conflict: 0, incomplete: 0, off: 0, on: 0 })
  })

  // 行里只有名字和「传输 · 工具数 · token」，命令行藏在详情里。只命中命令行的那几行
  // 在屏幕上没有任何东西能解释它为什么在结果里 —— 那就把命中的那条顶到第二行来。
  it('只命中命令行时，把那条命令行交出来', () => {
    const hit = visibleServers(scan, '/Applications', { state: null })[0]
    expect(queryCommand(hit, '/Applications')).toContain('/Applications')
  })

  it('名字已经命中就不顶命令行 —— 那一行自己会解释自己', () => {
    const hit = visibleServers(scan, 'chrome', { state: null })[0]
    expect(queryCommand(hit, 'chrome')).toBeNull()
  })

  it('没搜索就没有这回事', () => {
    expect(queryCommand(scan.servers[0], '')).toBeNull()
  })
})

// ---------------------------------------------------------------------------
// 写
// ---------------------------------------------------------------------------

const CLAUDE_WRITE = CLAUDE_JSON
const CODEX_WRITE = CODEX_TOML

const writableScan: McpScan = {
  ...scan,
  agents: [
    {
      agent: 'claude',
      installed: true,
      supported: true,
      writePath: CLAUDE_WRITE,
      sources: [],
    },
    { agent: 'codex', installed: true, supported: true, writePath: CODEX_WRITE, sources: [] },
    // 装了但没有可写来源的一家：勾选框必须勾不动，而不是勾上之后什么都没发生。
    { agent: 'grok', installed: true, supported: true, writePath: null, sources: [] },
  ],
}

describe('按 agent 同步', () => {
  it('勾选框按「可写文件里有没有」画，不按「跑不跑」', () => {
    // chrome-devtools 在 claude 那儿生效的是项目 `.mcp.json` 那份，而可写文件
    // `~/.claude.json` 里也有一份（被盖住的）。勾选框说的是后者。
    expect(syncState(chrome, writableScan, 'claude')).toBe('checked')
    // grok 跑着它，但 grok 的那份在 `~/.claude.json`（兼容来源，不是 grok 的可写文件）。
    expect(syncState(chrome, writableScan, 'grok')).toBe('locked')
    expect(syncState(chrome, writableScan, 'codex')).toBe('unchecked')
  })

  it('取消勾选是移除，不是写一个没人读的 enabled:false', () => {
    const edit = syncEdit(chrome, writableScan, 'claude')
    expect(edit).toEqual({ agent: 'claude', name: 'chrome-devtools', op: 'drop', def: null })
  })

  it('勾上就把生效的那份定义写过去', () => {
    const edit = syncEdit(chrome, writableScan, 'codex')
    expect(edit?.op).toBe('put')
    // 抄的是**生效的那份**（项目 `.mcp.json` 里带 --autoConnect 的），不是随便一份。
    expect(edit?.def?.args).toEqual(['-y', 'chrome-devtools-mcp@latest'])
  })

  it('关着的那条勾上走覆盖 —— 一条路径顺带把 enabled=false 清掉', () => {
    const withWrite = entry('computer-use', [
      at('codex', source(CODEX_WRITE), def({ name: 'computer-use', enabled: false })),
    ])
    expect(syncState(withWrite, writableScan, 'codex')).toBe('unchecked')
    expect(syncEdit(withWrite, writableScan, 'codex')?.op).toBe('put')
  })

  it('没有可写来源的那家勾不动', () => {
    expect(syncEdit(chrome, writableScan, 'grok')).toBeNull()
    expect(syncState(chrome, writableScan, 'pi')).toBe('locked')
  })

  it('删除只点名可写文件里真有它的那几家', () => {
    // grok 也在跑 chrome-devtools，但那份在 `~/.claude.json`，不归 grok 管。
    expect(removeEdits(chrome, writableScan)).toEqual([
      { agent: 'claude', name: 'chrome-devtools', op: 'drop', def: null },
    ])
  })

  it('停用同样只点名可写文件里的那几家', () => {
    expect(enableEdits(chrome, writableScan, false)).toEqual([
      { agent: 'claude', name: 'chrome-devtools', op: 'disable', def: null },
    ])
    expect(enableEdits(chrome, writableScan, true)[0].op).toBe('enable')
  })

  it('派生字段不往回传 —— 那些后端自己重算', () => {
    const input = defToInput(tapd.defs[0].def)
    expect(input).not.toHaveProperty('incomplete')
    expect(input).not.toHaveProperty('enabled')
    expect(input).not.toHaveProperty('name')
    expect(input.env).toEqual([
      { key: 'TAPD_ACCESS_TOKEN', value: '9ae39deca75e3bae60' },
      { key: 'TAPD_BASE', value: 'https://tapd.example' },
    ])
  })

  it('一份定义能一次写给好几家', () => {
    const edits = putEdits('x', blankInput(), ['claude', 'codex'])
    expect(edits.map((e) => e.agent)).toEqual(['claude', 'codex'])
    expect(edits.every((e) => e.op === 'put')).toBe(true)
  })

  it('编辑时取消掉的那家要被移除，不是原地不动', () => {
    // 只写不删的话，编辑框里取消一个勾等于什么都没做 —— 用户看到的是「取消了还在」。
    const edits = formEdits('chrome-devtools', blankInput(), ['codex'], chrome, writableScan)
    expect(edits).toEqual([
      { agent: 'codex', name: 'chrome-devtools', op: 'put', def: blankInput() },
      { agent: 'claude', name: 'chrome-devtools', op: 'drop', def: null },
    ])
  })

  it('新增时没有「原来」，只写不删', () => {
    expect(formEdits('x', blankInput(), ['claude'], null, writableScan)).toHaveLength(1)
  })

  it('可写文件里本来就没有的那家不会被凭空「移除」', () => {
    // grok 跑着 chrome-devtools，但那份在 `~/.claude.json`。不勾它 ≠ 要从 grok 的
    // 配置里删点什么 —— 那儿本来就没有。
    const edits = formEdits('chrome-devtools', blankInput(), [], chrome, writableScan)
    expect(edits.filter((e) => e.agent === 'grok')).toEqual([])
  })
})

describe('命令行拆分', () => {
  it('按空白拆，认引号', () => {
    expect(parseCommandLine('npx -y chrome-devtools-mcp@latest')).toEqual({
      command: 'npx',
      args: ['-y', 'chrome-devtools-mcp@latest'],
    })
    expect(parseCommandLine('"/Applications/My App/bin" mcp')).toEqual({
      command: '/Applications/My App/bin',
      args: ['mcp'],
    })
    expect(parseCommandLine("node --flag='a b'")).toEqual({
      command: 'node',
      args: ['--flag=a b'],
    })
  })

  it('空的和只有空白的给 null，不是空字符串命令', () => {
    expect(parseCommandLine('   ')).toEqual({ command: null, args: [] })
  })

  it('引号里的空字符串是一个真参数，不能被吃掉', () => {
    expect(parseCommandLine('cmd ""')).toEqual({ command: 'cmd', args: [''] })
  })
})

describe('名字校验', () => {
  it('空格和点不收 —— TOML 的裸键里那是两段路径', () => {
    expect(validName('chrome-devtools')).toBe(true)
    expect(validName('a_b1')).toBe(true)
    expect(validName('a b')).toBe(false)
    expect(validName('a.b')).toBe(false)
    expect(validName('')).toBe(false)
  })
})

describe('计划', () => {
  const report: McpWriteReport = {
    dryRun: true,
    stamps: [],
    failed: null,
    steps: [
      {
        agent: 'claude',
        name: 'x',
        kind: 'add',
        path: CLAUDE_WRITE,
        before: null,
        after: 'npx x',
        note: null,
        shadowedBy: null,
        done: false,
      },
      {
        agent: 'codex',
        name: 'x',
        kind: 'remove',
        path: CODEX_WRITE,
        before: 'npx x',
        after: null,
        note: null,
        shadowedBy: null,
        done: false,
      },
    ],
    blocked: [],
  }

  it('按种类计数，确认框那一句概括用它', () => {
    expect(stepCounts(report)).toEqual({ add: 1, update: 0, remove: 1, enable: 0, disable: 0 })
  })

  it('有移除就算破坏性 —— 确认按钮要变红', () => {
    expect(isDestructive(report)).toBe(true)
    expect(isDestructive({ ...report, steps: [report.steps[0]] })).toBe(false)
  })
})

describe('计划翻成确认框', () => {
  const HOME = '/Users/me'
  const view = (report: McpWriteReport) => mcpPlanView('删掉', report, HOME)
  const step = (over: Partial<McpWriteReport['steps'][number]>) => ({
    agent: 'claude' as Agent,
    name: 'x',
    kind: 'add' as const,
    path: `${HOME}/.claude.json`,
    before: null,
    after: 'npx x',
    note: null,
    shadowedBy: null,
    done: false,
    ...over,
  })
  const report = (over: Partial<McpWriteReport> = {}): McpWriteReport => ({
    dryRun: true,
    steps: [step({})],
    blocked: [],
    stamps: [],
    failed: null,
    ...over,
  })

  beforeEach(() => setLang('zh'))

  it('每一步都说清「在哪个文件」「改成什么」', () => {
    const [row] = view(report()).rows
    expect(row.kind).toBe('add')
    expect(row.where).toContain('~/.claude.json')
    expect(row.detail).toBe('npx x')
  })

  it('概括里零的那几档不出现', () => {
    const s = view(report()).summary
    expect(s).toContain('1')
    expect(s).not.toContain('覆盖')
    expect(s).not.toContain('停用')
  })

  it('移除算破坏性：按钮变红，脚注换成「会丢配置」那句', () => {
    const v = view(report({ steps: [step({ kind: 'remove', before: 'npx x', after: null })] }))
    expect(v.danger).toBe(true)
    expect(v.footnote).toBe(t('tools.mcp.plan.removeNote'))
    // 移除时 after 是 null，那一行得退回去显示原来那条命令，不能空着。
    expect(v.rows[0].detail).toBe('npx x')
  })

  it('不带移除时脚注是备份说明，按钮不红', () => {
    const v = view(report())
    expect(v.danger).toBe(false)
    expect(v.footnote).toBe(t('tools.mcp.plan.backupNote'))
  })

  it('新建文件的那一步要提前说', () => {
    const v = view(report({ steps: [step({ note: 'newFile' })] }))
    expect(v.rows[0].note).toBe(t('tools.mcp.plan.newFile'))
    expect(v.rows[0].noteWarn).toBe(false)
  })

  it('被盖住的那一步是警告，且要把盖它的文件说出来', () => {
    const v = view(
      report({ steps: [step({ note: 'shadowed', shadowedBy: `${HOME}/work/.mcp.json` })] }),
    )
    expect(v.rows[0].noteWarn).toBe(true)
    expect(v.rows[0].note).toContain('~/work/.mcp.json')
  })

  it('做不了的每一条都写成一句完整的话，不是一个枚举名', () => {
    const v = view(
      report({
        steps: [],
        blocked: [{ agent: 'grok', name: 'x', reason: 'notInWritableSource', path: `${HOME}/a` }],
      }),
    )
    expect(v.blocked).toHaveLength(1)
    expect(v.blocked[0]).toContain('x')
    expect(v.blocked[0]).toContain('~/a')
    expect(v.blocked[0]).not.toContain('notInWritableSource')
  })

  it('按钮上带着步数 —— 「执行」两个字不说明要动几处', () => {
    expect(view(report({ steps: [step({}), step({ agent: 'codex' })] })).applyLabel).toContain('2')
  })
})

/**
 * 真跑完之后那句话。三种结局得分开说 —— 说混了就是骗人。
 *
 * 最要紧的是「做了一半」：七家 agent 的配置在七个文件里，没有哪个机制能把它们一起
 * 提交。只报一句「失败了」的话，用户根本不知道自己现在有几份配置已经被改了。
 */
describe('做完之后那句话', () => {
  const CLAUDE_JSON = `${HOME}/.claude.json`
  const CODEX_TOML = `${HOME}/.codex/config.toml`

  const step = (over: Partial<McpWriteReport['steps'][number]> = {}) => ({
    agent: 'claude' as Agent,
    name: 'x',
    kind: 'add' as const,
    path: CLAUDE_JSON,
    before: null,
    after: 'npx x',
    note: null,
    shadowedBy: null,
    done: false,
    ...over,
  })

  const ran = (over: Partial<McpWriteReport> = {}): McpWriteReport => ({
    dryRun: false,
    steps: [step({ done: true })],
    blocked: [],
    stamps: [],
    failed: null,
    ...over,
  })

  beforeEach(() => setLang('zh'))

  it('全做完就报做完了几步', () => {
    const out = mcpApplyOutcome(ran(), HOME)
    expect(out.error).toBe(false)
    expect(out.msg).toContain('1')
  })

  /** 确认期间文件被改了：一个字都没写，所以这句话里不能出现任何「已经写了」的意思。 */
  it('确认期间被改过就说清楚一个字都没写', () => {
    const out = mcpApplyOutcome(
      ran({
        steps: [step()],
        failed: { kind: 'stale', path: CLAUDE_JSON, detail: null },
      }),
      HOME,
    )
    expect(out.error).toBe(true)
    // 路径缩成 ~ 开头 —— 这句话是弹在角落里的，绝对路径根本放不下。
    expect(out.msg).toContain('~/.claude.json')
  })

  it('一步都没落盘的写失败只说失败的那个', () => {
    const out = mcpApplyOutcome(
      ran({
        steps: [step()],
        failed: { kind: 'write', path: CLAUDE_JSON, detail: '磁盘满了' },
      }),
      HOME,
    )
    expect(out.error).toBe(true)
    expect(out.msg).toContain('磁盘满了')
  })

  /**
   * 这条是整段的理由：claude 那份已经写下去了、codex 那份失败了。只说「失败」用户
   * 会以为什么都没发生，转头就去查为什么 claude 那边变了。
   */
  it('做了一半要把已经落盘的那几步说出来', () => {
    const out = mcpApplyOutcome(
      ran({
        steps: [step({ done: true }), step({ agent: 'codex', path: CODEX_TOML })],
        failed: { kind: 'write', path: CODEX_TOML, detail: '解不开' },
      }),
      HOME,
    )
    expect(out.error).toBe(true)
    expect(out.msg).toContain('~/.codex/config.toml')
    expect(out.msg).toContain('1')
  })
})
