// 工具管理 · 配置集纯逻辑。
//
// 这一组测的是**唯一会造成破坏的那一步**：一个别人发来的 JSON，翻成本机的写请求
// 之后落在哪些路径上。装置数据照本机实测的形状造 —— 两份 `RTK.md` 是被
// `~/.claude/CLAUDE.md` / `~/.codex/AGENTS.md` 各自 `@` 进来的片段，agy 没有
// home 级约定，grok 自己那份不存在。

import { describe, it, expect } from 'vitest'
import type {
  Bundle,
  HookWriteReport,
  McpScan,
  McpServerDef,
  McpWriteReport,
  MemoAgentInfo,
  MemoFile,
  MemoScan,
} from '../src/types'
import {
  ALL_INCLUDED,
  BUNDLE_KIND,
  BUNDLE_REDACTED,
  BUNDLE_VERSION,
  bundleCounts,
  bundleFileName,
  bundleHookEdits,
  bundleIsEmpty,
  bundleKey,
  bundleMcpEdits,
  bundleAgentNames,
  bundleMemoTargets,
  bundlePlanView,
  bundleText,
  clearedValues,
  filterBundle,
  defaultPicks,
  isAgent,
  memoPickable,
  needsAgents,
  missingSecrets,
  readBundle,
  redactedArgs,
  suggestedAgents,
} from '../src/toolsBundle'

import { ALL_AGENTS as ALL_AGENTS_FOR_TEST } from '../src/settings'

const HOME = '/Users/me'
const CLAUDE_MD = `${HOME}/.claude/CLAUDE.md`
const CODEX_MD = `${HOME}/.codex/AGENTS.md`
const GROK_MD = `${HOME}/.grok/AGENTS.md`
const PI_MD = `${HOME}/.pi/agent/memory/MEMORY.md`

function agent(over: Partial<MemoAgentInfo> & { agent: MemoAgentInfo['agent'] }): MemoAgentInfo {
  return {
    installed: true,
    supported: true,
    path: null,
    exists: false,
    fallback: null,
    effective: null,
    fallenBack: false,
    extra: [],
    ...over,
  }
}

function file(path: string, bytes: number): MemoFile {
  return {
    path,
    name: path.split('/').pop()!,
    exists: true,
    bytes,
    revision: { exists: true, size: bytes, mtimeMs: 1_700_000_000_000 },
    link: null,
    readers: [],
    importedBy: null,
    imports: [],
    error: null,
  }
}

const scan: MemoScan = {
  home: HOME,
  agents: [
    agent({ agent: 'claude', path: CLAUDE_MD, exists: true, effective: CLAUDE_MD }),
    agent({ agent: 'codex', path: CODEX_MD, exists: true, effective: CODEX_MD }),
    agent({ agent: 'grok', path: GROK_MD, exists: false, fallback: CLAUDE_MD, effective: CLAUDE_MD, fallenBack: true }),
    agent({ agent: 'pi', path: PI_MD, exists: true, effective: PI_MD }),
    // agy 没有 home 级约定 —— 包里给它一份也落不了地。
    agent({ agent: 'agy', supported: false }),
  ],
  files: [file(CLAUDE_MD, 888), file(CODEX_MD, 29), file(PI_MD, 2705)],
  forks: [],
  dups: [],
  summary: { files: 3, present: 3, broken: 0, forks: 0, dups: 0 },
}

function bundle(over: Partial<Bundle> = {}): Bundle {
  return {
    kind: BUNDLE_KIND,
    version: BUNDLE_VERSION,
    createdAt: 1_700_000_000_000,
    app: '0.3.26',
    mcp: [],
    hooks: [],
    memo: [],
    skills: [],
    redacted: [],
    ...over,
  }
}

const GITHUB = {
  name: 'github',
  transport: 'stdio' as const,
  command: 'npx',
  args: ['-y', '@modelcontextprotocol/server-github', `--api-key=${BUNDLE_REDACTED}`],
  envKeys: ['GITHUB_TOKEN'],
  headerKeys: [],
  url: null,
  cwd: null,
  agents: ['claude', 'codex'],
}

// ---------------------------------------------------------------------------

describe('readBundle', () => {
  it('不是 JSON 就说不是 JSON', () => {
    expect(readBundle('not json at all').problem).toBe('notJson')
    // 顶层是数组也不行 —— `JSON.parse` 会过，但它没有 `kind`。
    expect(readBundle('[1,2,3]').problem).toBe('notJson')
  })

  it('没有类型标记的一律拒绝', () => {
    expect(readBundle('{"mcp":[]}').problem).toBe('notBundle')
    expect(readBundle('{"kind":"something/else","version":1}').problem).toBe('notBundle')
  })

  /**
   * 版本比本机新时**拒绝**，不是"尽力而为地读"：新版本多出来的字段可能恰恰是限制
   * 写入范围的那一个，按老规则读等于当它不存在。
   */
  it('版本比本机新就不读，并且说出对面是几版', () => {
    const r = readBundle(JSON.stringify({ kind: BUNDLE_KIND, version: 99 }))
    expect(r.problem).toBe('tooNew')
    expect(r.version).toBe(99)
    expect(r.bundle).toBeNull()
  })

  it('读得进一个正常的包', () => {
    const r = readBundle(bundleText(bundle({ mcp: [GITHUB] })))
    expect(r.problem).toBeNull()
    expect(r.bundle?.mcp).toEqual([GITHUB])
  })

  /** 形状不对的条目**逐条丢掉**，不是整包拒收 —— 别人手改坏了一行不该连累其余。 */
  it('形状不对的条目被丢掉，其余照读', () => {
    const raw = JSON.stringify({
      kind: BUNDLE_KIND,
      version: 1,
      mcp: [{ name: 'ok', command: 'x' }, { command: 'no name' }, 'garbage', null],
      hooks: [
        { command: 'echo hi', events: ['Stop'] },
        { command: 'no events', events: [] },
        { events: ['Stop'] },
      ],
      memo: [{ name: 'A.md', text: 'hi' }, { name: 'no text' }],
      skills: [{ name: 'one' }, 42],
    })
    const b = readBundle(raw).bundle!
    expect(b.mcp.map((s) => s.name)).toEqual(['ok'])
    expect(b.hooks.map((h) => h.command)).toEqual(['echo hi'])
    expect(b.memo.map((m) => m.name)).toEqual(['A.md'])
    expect(b.skills.map((s) => s.name)).toEqual(['one'])
  })

  /** 认不出来的 transport 归成 `unknown`，不是原样带着一个野字符串往下走。 */
  it('认不出来的 transport 归成 unknown', () => {
    const b = readBundle(
      JSON.stringify({ kind: BUNDLE_KIND, version: 1, mcp: [{ name: 'x', transport: 'carrier-pigeon' }] }),
    ).bundle!
    expect(b.mcp[0].transport).toBe('unknown')
  })

  it('数组里混进非字符串就只留字符串', () => {
    const b = readBundle(
      JSON.stringify({ kind: BUNDLE_KIND, version: 1, mcp: [{ name: 'x', args: ['-y', 3, null, 'ok'] }] }),
    ).bundle!
    expect(b.mcp[0].args).toEqual(['-y', 'ok'])
  })

  it('存下去再读回来是同一个包', () => {
    const b = bundle({ mcp: [GITHUB], memo: [{ agent: 'claude', parent: null, name: 'CLAUDE.md', text: '# hi\n' }] })
    expect(readBundle(bundleText(b)).bundle).toEqual(b)
  })
})

describe('bundleCounts', () => {
  it('四类各数各的', () => {
    const b = bundle({ mcp: [GITHUB], hooks: [{ command: 'x', events: ['Stop'], matcher: null, timeout: null, agents: [] }] })
    expect(bundleCounts(b)).toEqual({ mcp: 1, hooks: 1, memo: 0, skills: 0 })
    expect(bundleIsEmpty(b)).toBe(false)
    expect(bundleIsEmpty(bundle())).toBe(true)
  })
})

describe('bundleMcpEdits', () => {
  const b = bundle({ mcp: [GITHUB, { ...GITHUB, name: 'other' }] })
  const all = new Set([bundleKey('mcp', 0), bundleKey('mcp', 1)])

  it('每个目标 agent 各一条 put', () => {
    const edits = bundleMcpEdits(b, new Set([bundleKey('mcp', 0)]), ['claude', 'grok'])
    expect(edits.map((e) => [e.agent, e.name, e.op])).toEqual([
      ['claude', 'github', 'put'],
      ['grok', 'github', 'put'],
    ])
  })

  it('没勾的一条都不带', () => {
    expect(bundleMcpEdits(b, new Set(), ['claude'])).toEqual([])
    expect(bundleMcpEdits(b, all, ['claude']).map((e) => e.name)).toEqual(['github', 'other'])
  })

  /**
   * 这一条是这个模块最要紧的不变量。
   *
   * 包里本来就只有键名（后端导出时把值全抹了）。写成空串会**盖掉**用户 shell 里
   * 继承得到的那份 —— server 拿到一个空 token，报的是认证失败，而配置文件上明明
   * 有这个键，这种坏法查起来最久。所以一个键都不写，改成在预览里列出来让人补。
   */
  it('env 和 headers 一个键都不写', () => {
    const [e] = bundleMcpEdits(b, all, ['claude'])
    expect(e.def!.env).toEqual([])
    expect(e.def!.headers).toEqual([])
  })

  it('命令和参数原样带过去', () => {
    const [e] = bundleMcpEdits(b, all, ['claude'])
    expect(e.def!.command).toBe('npx')
    expect(e.def!.args).toEqual(GITHUB.args)
  })

  it('缺的值逐条列出来', () => {
    expect(missingSecrets(b, new Set([bundleKey('mcp', 0)]))).toEqual(['github.env.GITHUB_TOKEN'])
    expect(missingSecrets(b, new Set())).toEqual([])
  })

  /** 被抹过值的 args 是**照原样写进去**的，所以必须在预览里点出来。 */
  it('被抹过的参数要点出来', () => {
    expect(redactedArgs(b, new Set([bundleKey('mcp', 0)]))).toEqual([
      `github: --api-key=${BUNDLE_REDACTED}`,
    ])
  })
})

// ---------------------------------------------------------------------------

/**
 * 覆盖同名 server 时 `env` / `headers` 是**整片覆盖**（它们在 `mcp_write.rs` 的
 * `JSON_OWNED_KEYS` 里），不是逐键合并。而计划框的 `before` / `after` 只画命令行 ——
 * 命令行没变的时候，那一行看上去和没动一样，用户是在 server 起不来之后才发现
 * token 没了。
 */
describe('clearedValues', () => {
  function def(over: Partial<McpServerDef> = {}): McpServerDef {
    return {
      name: 'github',
      transport: 'stdio',
      command: 'npx',
      args: [],
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
  function localScan(over: Partial<McpScan> = {}): McpScan {
    return {
      home: HOME,
      servers: [],
      agents: [],
      summary: { servers: 0, active: 0, conflicts: 0, tools: 0, tokens: 0, measured: 0 },
      ...over,
    }
  }
  function source(writable: boolean) {
    return {
      path: `${HOME}/.claude.json`,
      scope: 'user' as const,
      origin: 'own' as const,
      format: 'jsonServers' as const,
      writable,
      exists: true,
      precedence: 100,
      conditional: false,
      project: null,
    }
  }
  function entry(defs: { agent: 'claude' | 'codex'; writable: boolean; def: McpServerDef }[]) {
    return {
      name: 'github',
      defs: defs.map((d) => ({
        agent: d.agent,
        source: source(d.writable),
        def: d.def,
        effective: true,
      })),
      agents: defs.map((d) => d.agent),
      inactiveAgents: [],
      conflict: false,
      fingerprints: [],
      tools: null,
      tokens: null,
    }
  }

  const b = bundle({ mcp: [GITHUB] })
  const all = new Set([bundleKey('mcp', 0)])
  const withToken = def({
    env: [
      { key: 'GITHUB_TOKEN', value: 'ghp_real', secret: true },
      { key: 'GITHUB_HOST', value: 'github.example', secret: false },
    ],
  })

  it('没扫过本机就什么都不说 —— 不拿"扫描还没回来"当"没东西会丢"', () => {
    expect(clearedValues(b, all, ['claude'], null)).toEqual([])
  })

  it('本机没有同名 server 就没东西可丢', () => {
    expect(clearedValues(b, all, ['claude'], localScan())).toEqual([])
  })

  /** 包里只提了 `GITHUB_TOKEN`，但 `env` 是整片覆盖，连 `GITHUB_HOST` 一起带走。 */
  it('包里没提到的键也一样会没，照样列出来', () => {
    const scan = localScan({
      servers: [entry([{ agent: 'claude', writable: true, def: withToken }])],
    })
    expect(clearedValues(b, all, ['claude'], scan)).toEqual([
      'claude/github.env.GITHUB_TOKEN',
      'claude/github.env.GITHUB_HOST',
    ])
  })

  it('headers 和 env 一样算', () => {
    const scan = localScan({
      servers: [
        entry([
          {
            agent: 'claude',
            writable: true,
            def: def({ headers: [{ key: 'Authorization', value: 'Bearer x', secret: true }] }),
          },
        ]),
      ],
    })
    expect(clearedValues(b, all, ['claude'], scan)).toEqual([
      'claude/github.headers.Authorization',
    ])
  })

  /** 键在值不在，本来就没东西可丢；报出来只会让人以为自己要丢东西。 */
  it('空值不算', () => {
    const scan = localScan({
      servers: [
        entry([
          {
            agent: 'claude',
            writable: true,
            def: def({ env: [{ key: 'GITHUB_TOKEN', value: '', secret: true }] }),
          },
        ]),
      ],
    })
    expect(clearedValues(b, all, ['claude'], scan)).toEqual([])
  })

  /** 导入只往可写的那一份里写，被它盖住的别的定义不受影响。 */
  it('不可写的那份不受影响，不算进来', () => {
    const scan = localScan({
      servers: [entry([{ agent: 'claude', writable: false, def: withToken }])],
    })
    expect(clearedValues(b, all, ['claude'], scan)).toEqual([])
  })

  it('只算这次要装的那几家', () => {
    const scan = localScan({
      servers: [
        entry([
          { agent: 'claude', writable: true, def: withToken },
          { agent: 'codex', writable: true, def: withToken },
        ]),
      ],
    })
    expect(clearedValues(b, all, ['codex'], scan)).toEqual([
      'codex/github.env.GITHUB_TOKEN',
      'codex/github.env.GITHUB_HOST',
    ])
  })

  it('没勾的 server 不算', () => {
    const scan = localScan({
      servers: [entry([{ agent: 'claude', writable: true, def: withToken }])],
    })
    expect(clearedValues(b, new Set(), ['claude'], scan)).toEqual([])
  })
})

describe('bundleHookEdits', () => {
  const h = { command: 'echo hi', events: ['Stop', 'SessionStart'], matcher: 'Bash', timeout: 30, agents: ['claude'] }
  const b = bundle({ hooks: [h] })

  /** 一条 hook 挂在 N 个事件上就是 N 条 edit —— 后端那一层本来就是按事件记的。 */
  it('每个事件 × 每个 agent 各一条', () => {
    const edits = bundleHookEdits(b, new Set([bundleKey('hooks', 0)]), ['claude', 'codex'])
    expect(edits).toHaveLength(4)
    expect(edits.map((e) => `${e.agent}/${e.event}`)).toEqual([
      'claude/Stop',
      'claude/SessionStart',
      'codex/Stop',
      'codex/SessionStart',
    ])
  })

  it('matcher 和超时原样带过去', () => {
    const [e] = bundleHookEdits(b, new Set([bundleKey('hooks', 0)]), ['claude'])
    expect(e.matcher).toBe('Bash')
    expect(e.timeout).toBe(30)
    expect(e.op).toBe('add')
  })
})

describe('bundleMemoTargets', () => {
  /** 按**角色**落，不按包里的路径 —— 导出那台机器的 home 和用户名都不一样。 */
  it('约定文件落在本机那家自己的路径上', () => {
    const b = bundle({ memo: [{ agent: 'claude', parent: null, name: 'CLAUDE.md', text: 'x' }] })
    const [t] = bundleMemoTargets(b, scan)
    expect(t.path).toBe(CLAUDE_MD)
    expect(t.kind).toBe('overwrite')
    expect(t.bytes).toBe(888)
  })

  /** 各家的文件名不一样：一份 `AGENTS.md` 落到 pi 头上得叫 `MEMORY.md`。 */
  it('落地用的是本机那家的文件名，不是包里的', () => {
    const b = bundle({ memo: [{ agent: 'pi', parent: null, name: 'AGENTS.md', text: 'x' }] })
    expect(bundleMemoTargets(b, scan)[0].path).toBe(PI_MD)
  })

  it('本机没有的那份是新建', () => {
    const b = bundle({ memo: [{ agent: 'grok', parent: null, name: 'AGENTS.md', text: 'x' }] })
    const [t] = bundleMemoTargets(b, scan)
    expect(t.path).toBe(GROK_MD)
    expect(t.kind).toBe('create')
    expect(t.bytes).toBe(0)
  })

  /** 片段跟着它的 `parent` 走，落在那家约定文件的**同一个目录**里 —— `@RTK.md`
   *  是相对路径，只有落在旁边才接得上。 */
  it('片段落在引用它那家的目录里', () => {
    const b = bundle({ memo: [{ agent: null, parent: 'codex', name: 'RTK.md', text: 'x' }] })
    const [t] = bundleMemoTargets(b, scan)
    expect(t.path).toBe(`${HOME}/.codex/RTK.md`)
    expect(t.fragment).toBe(true)
    expect(t.kind).toBe('create')
  })

  it('没有 home 级约定的那家落不了地', () => {
    const b = bundle({ memo: [{ agent: 'agy', parent: null, name: 'AGENTS.md', text: 'x' }] })
    const [t] = bundleMemoTargets(b, scan)
    expect(t.kind).toBe('blocked')
    expect(t.reason).toBe('unsupported')
    expect(t.path).toBeNull()
  })

  it('本机不认得的角色也落不了地', () => {
    const b = bundle({ memo: [{ agent: 'cursor', parent: null, name: 'X.md', text: 'x' }] })
    expect(bundleMemoTargets(b, scan)[0].reason).toBe('unsupported')
  })

  it('不知道跟谁走的片段落不了地', () => {
    const b = bundle({ memo: [{ agent: null, parent: null, name: 'RTK.md', text: 'x' }] })
    const [t] = bundleMemoTargets(b, scan)
    expect(t.kind).toBe('blocked')
    expect(t.reason).toBe('noParent')
  })

  /**
   * 「没记 parent」和「记了 parent 但本机落不了那一家」是两种不同的坏法，提示语
   * 也不一样 —— 并成一句的话，用户拿到的话和他手上的包对不上（本机实测那个包里
   * 的 `FROM-CURSOR.md` 明明写着 `parent: cursor`，提示却说"包里没记"）。
   */
  it('片段记了 parent 但本机落不了那一家，说的是另一句话', () => {
    for (const parent of ['cursor', 'agy']) {
      const b = bundle({ memo: [{ agent: null, parent, name: 'X.md', text: 'x' }] })
      const [t] = bundleMemoTargets(b, scan)
      expect(t.kind, parent).toBe('blocked')
      expect(t.reason, parent).toBe('noHome')
      // 提示语要报出它跟着谁 —— 这个字段就是拿去填 `{agent}` 的。
      expect(t.agent, parent).toBe(parent)
    }
  })

  /**
   * 名字会被拼进写入路径 —— 包是**别人发来的**，`../../.ssh/authorized_keys` 这种
   * 名字要么是手改坏了要么是故意的，两种都不能往下走。
   */
  it('名字里带路径的一律挡掉', () => {
    const bad = ['../../.ssh/authorized_keys', 'a/b.md', 'a\\b.md', '..', '']
    for (const name of bad) {
      const b = bundle({ memo: [{ agent: null, parent: 'claude', name, text: 'x' }] })
      const [t] = bundleMemoTargets(b, scan)
      expect(t.kind, name).toBe('blocked')
      expect(t.reason, name).toBe('badName')
      expect(t.path, name).toBeNull()
    }
  })

  it('正文原样带过去', () => {
    const b = bundle({ memo: [{ agent: 'claude', parent: null, name: 'CLAUDE.md', text: '# hi\n@RTK.md\n' }] })
    expect(bundleMemoTargets(b, scan)[0].text).toBe('# hi\n@RTK.md\n')
  })
})

describe('defaultPicks', () => {
  const b = bundle({
    mcp: [GITHUB],
    hooks: [{ command: 'x', events: ['Stop'], matcher: null, timeout: null, agents: [] }],
    memo: [
      { agent: 'claude', parent: null, name: 'CLAUDE.md', text: 'x' }, // 本机已有 → 覆盖
      { agent: 'grok', parent: null, name: 'AGENTS.md', text: 'x' }, // 本机没有 → 新建
      { agent: 'agy', parent: null, name: 'AGENTS.md', text: 'x' }, // 落不了地
    ],
  })

  /**
   * 覆盖一份 `CLAUDE.md` 是把用户攒了很久的个人指令整个换掉。这种事不能靠他注意到
   * 某一行没取消勾选 —— 默认就不勾。
   */
  it('全局指令只默认勾新建那几条', () => {
    const picked = defaultPicks(b, bundleMemoTargets(b, scan), ['claude', 'grok', 'agy'])
    expect(picked.has(bundleKey('mcp', 0))).toBe(true)
    expect(picked.has(bundleKey('hooks', 0))).toBe(true)
    expect(picked.has(bundleKey('memo', 0))).toBe(false)
    expect(picked.has(bundleKey('memo', 1))).toBe(true)
    expect(picked.has(bundleKey('memo', 2))).toBe(false)
  })

  /** 没勾那一家就不该替他写 —— 「装给哪几家」那一排是全局的，不是只管 MCP。 */
  it('没勾上的那家，它的全局指令也不默认勾', () => {
    const picked = defaultPicks(b, bundleMemoTargets(b, scan), ['claude'])
    expect(picked.has(bundleKey('memo', 1))).toBe(false)
  })
})

describe('memoPickable', () => {
  const targets = (b: Bundle) => bundleMemoTargets(b, scan)

  it('勾了那家才动得了', () => {
    const b = bundle({ memo: [{ agent: 'grok', parent: null, name: 'AGENTS.md', text: 'x' }] })
    const [t] = targets(b)
    expect(memoPickable(t, ['grok'])).toBe(true)
    expect(memoPickable(t, ['claude'])).toBe(false)
    expect(memoPickable(t, [])).toBe(false)
  })

  /** 片段跟着它的 `parent` 那一家走 —— 它本来就是被那份约定文件 `@` 进来的。 */
  it('片段看的是引用它那一家', () => {
    const b = bundle({ memo: [{ agent: null, parent: 'codex', name: 'RTK.md', text: 'x' }] })
    const [t] = targets(b)
    expect(memoPickable(t, ['codex'])).toBe(true)
    expect(memoPickable(t, ['claude'])).toBe(false)
  })

  it('落不了地的怎么勾都勾不上', () => {
    const b = bundle({ memo: [{ agent: 'agy', parent: null, name: 'AGENTS.md', text: 'x' }] })
    expect(memoPickable(targets(b)[0], ['agy'])).toBe(false)
  })

  /** 本机不认得的角色不能进 `agentLabel` —— 那是在 `undefined` 上取属性。 */
  it('本机不认得的角色一律不可勾', () => {
    const b = bundle({ memo: [{ agent: 'cursor', parent: null, name: 'X.md', text: 'x' }] })
    expect(memoPickable(targets(b)[0], ALL_AGENTS_FOR_TEST)).toBe(false)
  })
})

describe('needsAgents', () => {
  /**
   * 不能拿 `bundleMcpEdits(...).length` 判：那个函数是**按 agent 展开**的，一家都
   * 没勾时它恒为空，于是「至少勾一家」永远不会提示，用户看到的是「一步都排不出来」
   * —— 而那句话没告诉他该做什么。
   */
  it('勾了 MCP 或 hook 就必须指定 agent', () => {
    const b = bundle({
      mcp: [GITHUB],
      hooks: [{ command: 'x', events: ['Stop'], matcher: null, timeout: null, agents: [] }],
      memo: [{ agent: 'grok', parent: null, name: 'AGENTS.md', text: 'x' }],
    })
    expect(needsAgents(b, new Set([bundleKey('mcp', 0)]))).toBe(true)
    expect(needsAgents(b, new Set([bundleKey('hooks', 0)]))).toBe(true)
    // 全局指令自带角色，不靠那一排。
    expect(needsAgents(b, new Set([bundleKey('memo', 0)]))).toBe(false)
    expect(needsAgents(b, new Set())).toBe(false)
  })

  /** 这一条是上面那个坑的直接回归：展开出来是空的，判据却必须是 true。 */
  it('一家都没勾时，展开出来是空的，但仍然要提示', () => {
    const b = bundle({ mcp: [GITHUB] })
    const picked = new Set([bundleKey('mcp', 0)])
    expect(bundleMcpEdits(b, picked, [])).toEqual([])
    expect(needsAgents(b, picked)).toBe(true)
  })
})

describe('isAgent', () => {
  it('包里的 agent 名字是别人机器上写下的，得核一遍', () => {
    expect(isAgent('claude')).toBe(true)
    expect(isAgent('kimicode')).toBe(true)
    expect(isAgent('cursor')).toBe(false)
    expect(isAgent(null)).toBe(false)
  })
})

describe('bundleAgentNames', () => {
  /** 本机不认识的也要列出来：「那台机器上还跑着一个 cursor」本身就是信息。 */
  it('原样列出包里提到过的每一家，含本机不认识的', () => {
    const b = bundle({
      mcp: [{ ...GITHUB, agents: ['claude', 'cursor'] }],
      memo: [
        { agent: 'pi', parent: null, name: 'MEMORY.md', text: 'x' },
        { agent: null, parent: 'codex', name: 'RTK.md', text: 'x' },
      ],
    })
    expect(bundleAgentNames(b)).toEqual(['claude', 'cursor', 'pi', 'codex'])
  })

  it('空包是空的', () => {
    expect(bundleAgentNames(bundle())).toEqual([])
  })
})

describe('suggestedAgents', () => {
  it('包里提到、本机也认得的那几家', () => {
    const b = bundle({
      mcp: [GITHUB],
      skills: [{ name: 's', remote: null, agents: ['pi', 'cursor'] }],
    })
    // 次序跟着 ALL_AGENTS 走，不跟着包里出现的先后 —— 界面上那一排是固定次序的。
    expect(suggestedAgents(b)).toEqual(['claude', 'codex', 'pi'])
  })

  it('一家都没提到就是空的', () => {
    expect(suggestedAgents(bundle())).toEqual([])
  })

  /**
   * 只带全局指令的包（把一套写法分享给别人）是完全正常的用法。只看 `agents` 字段
   * 的话，这种包一家都选不出来 —— 界面上那一排全是灰的，下面的条目又摆在那儿，
   * 看上去像坏了。
   */
  it('只带全局指令的包也要选得出人来', () => {
    const b = bundle({
      memo: [
        { agent: 'pi', parent: null, name: 'MEMORY.md', text: 'x' },
        { agent: null, parent: 'codex', name: 'RTK.md', text: 'x' },
      ],
    })
    expect(suggestedAgents(b)).toEqual(['codex', 'pi'])
  })
})

describe('落盘', () => {
  it('文件名带日期', () => {
    expect(bundleFileName(new Date(2026, 8, 11))).toBe('tools-bundle-2026-09-11.json')
    expect(bundleFileName(new Date(2026, 0, 5))).toBe('tools-bundle-2026-01-05.json')
  })

  /** 这个文件是拿去发给别人的，对方多半会先打开看一眼 —— 存成人能读的样子。 */
  it('存成带缩进的 JSON，末尾有换行', () => {
    const text = bundleText(bundle())
    expect(text.endsWith('\n')).toBe(true)
    expect(text).toContain('\n  "kind"')
  })
})

describe('filterBundle', () => {
  const full = bundle({
    mcp: [GITHUB],
    hooks: [{ command: 'x', events: ['Stop'], matcher: null, timeout: null, agents: [] }],
    memo: [{ agent: 'claude', parent: null, name: 'CLAUDE.md', text: 'x' }],
    skills: [{ name: 's', remote: null, agents: [] }],
    redacted: ['github.env.GITHUB_TOKEN'],
  })

  it('没勾的那几类一条都不带', () => {
    const only = filterBundle(full, { mcp: false, hooks: true, memo: false, skills: false })
    expect(bundleCounts(only)).toEqual({ mcp: 0, hooks: 1, memo: 0, skills: 0 })
  })

  /** 抹掉的值全来自 MCP。不带 MCP 却留着那张单子，读的人会去找一个不存在的 server。 */
  it('不带 MCP 时那张「抹掉了什么」的单子也跟着走', () => {
    expect(filterBundle(full, { mcp: false, hooks: true, memo: true, skills: true }).redacted).toEqual([])
    expect(filterBundle(full, ALL_INCLUDED).redacted).toEqual(['github.env.GITHUB_TOKEN'])
  })
})

describe('bundlePlanView', () => {
  const mcpReport: McpWriteReport = {
    dryRun: true,
    steps: [
      {
        agent: 'claude',
        name: 'github',
        kind: 'add',
        path: `${HOME}/.claude.json`,
        before: null,
        after: 'npx -y server',
        note: null,
        shadowedBy: null,
        done: false,
      },
    ],
    blocked: [{ agent: 'pi', name: 'github', reason: 'unsupported', path: null }],
    stamps: [{ path: `${HOME}/.claude.json`, stamp: 'true:120:9' }],
    failed: null,
  }
  const hookReport: HookWriteReport = {
    dryRun: true,
    steps: [
      {
        agent: 'claude',
        kind: 'add',
        path: `${HOME}/.claude/settings.json`,
        event: 'Stop',
        matcher: null,
        command: 'echo hi',
        newFile: false,
        done: false,
      },
    ],
    blocked: [],
  }

  function targets(b: Bundle) {
    return bundleMemoTargets(b, scan)
  }

  /** 三段来源不同的计划并成**一个**确认框。分三次确认的话，用户在第二次上点取消时
   *  前面那一批已经落盘了 —— 他以为自己取消了整件事。 */
  it('三段合成一份，每段的行都在', () => {
    const b = bundle({ memo: [{ agent: 'grok', parent: null, name: 'AGENTS.md', text: 'hi' }] })
    const view = bundlePlanView(mcpReport, hookReport, targets(b), HOME)
    expect(view.rows).toHaveLength(3)
    expect(view.rows[2].where).toContain('.grok/AGENTS.md')
  })

  it('做不了的那几条也并在一起', () => {
    const b = bundle({ memo: [{ agent: 'agy', parent: null, name: 'AGENTS.md', text: 'hi' }] })
    const view = bundlePlanView(mcpReport, hookReport, targets(b), HOME)
    // MCP 那条 unsupported + 全局指令那条落不了地。
    expect(view.blocked).toHaveLength(2)
    expect(view.blocked[1]).toContain('AGENTS.md')
  })

  /** 覆盖一份已有的全局指令是这一整套里唯一会**丢东西**的一步 —— 确认按钮要变红。 */
  it('有覆盖就是危险操作', () => {
    const safe = bundle({ memo: [{ agent: 'grok', parent: null, name: 'AGENTS.md', text: 'hi' }] })
    expect(bundlePlanView(null, null, targets(safe), HOME).danger).toBe(false)

    const risky = bundle({ memo: [{ agent: 'claude', parent: null, name: 'CLAUDE.md', text: 'hi' }] })
    const view = bundlePlanView(null, null, targets(risky), HOME)
    expect(view.danger).toBe(true)
    expect(view.rows[0].noteWarn).toBe(true)
    expect(view.rows[0].note).toContain('888')
  })

  it('落不了地的那几条不算进要写的行', () => {
    const b = bundle({
      memo: [
        { agent: 'agy', parent: null, name: 'AGENTS.md', text: 'hi' },
        { agent: null, parent: null, name: 'RTK.md', text: 'hi' },
      ],
    })
    expect(bundlePlanView(null, null, targets(b), HOME).rows).toEqual([])
  })

  it('一段都没有时是一份空计划，不是崩', () => {
    const view = bundlePlanView(null, null, [], HOME)
    expect(view.rows).toEqual([])
    expect(view.blocked).toEqual([])
    expect(view.danger).toBe(false)
  })
})
