// 工具管理 · Skills 纯逻辑。
//
// 装置数据照着本机实测（方案文档 1.1）的形状造：主 store 放实体内容、中间 store
// 转一手、agent 那层走两跳，另一套 store 里躺着同名的第二份，外加一条死链。

import { describe, it, expect, beforeEach } from 'vitest'
import type { SkillEntry, SkillRef, SkillScan, StoreCandidate } from '../src/types'
import {
  BADGE_ORDER,
  agentsOf,
  badgeCounts,
  bodyStores,
  chainLines,
  deletableBody,
  removableLink,
  emptyFilter,
  filterSkills,
  healthTip,
  isUnhealthy,
  mainStoreOptions,
  riskIsConclusive,
  matchesQuery,
  queryPath,
  queryRank,
  resetSkillFilter,
  riskRank,
  shortenPath,
  skillFilter,
  isSkillPinned,
  pinnedSkills,
  agentReach,
  sharedDirRef,
  sortSkills,
  toggleSkillPin,
  visibleSkills,
  worstBadge,
  worstRisk,
} from '../src/toolsSkills'

const HOME = '/Users/wuchao'
const MAIN = `${HOME}/.skills-manager/skills`
const MID = `${HOME}/.agents/skills`
const CLAUDE = `${HOME}/.claude/skills`
const OTHER = `${HOME}/.cc-switch/skills`

function body(store: string, name: string, over: Partial<SkillEntry['bodies'][number]> = {}) {
  return {
    path: `${store}/${name}`,
    store,
    files: 3,
    bytes: 1200,
    modified: 1_700_000_000_000,
    risk: 'none' as const,
    truncated: false,
    ...over,
  }
}

/**
 * 实体目录。`reachedBy` 单独给：实体目录的 `agents` 几乎总是空的（没人直接扫
 * `.skills-manager`），而「有没有链子落在它身上」是另一回事 —— 后端 `fill_reached_by`
 * 算的就是这个差别，前端这几个判断吃的是算完的结果。
 */
function realDir(store: string, name: string, reachedBy: SkillRef['agents'] = []): SkillRef {
  return {
    path: `${store}/${name}`,
    store,
    agents: [],
    reachedBy,
    health: { state: 'realDir' },
    hops: [],
    resolved: `${store}/${name}`,
  }
}

function linked(
  store: string,
  name: string,
  chain: string[],
  agents: SkillRef['agents'] = [],
  reachedBy: SkillRef['agents'] = agents,
): SkillRef {
  const from = [`${store}/${name}`, ...chain.slice(0, -1)]
  return {
    path: `${store}/${name}`,
    store,
    agents,
    reachedBy,
    health: { state: 'linked' },
    hops: chain.map((to, i) => ({ from: from[i], to, exists: true })),
    resolved: chain[chain.length - 1],
  }
}

function broken(
  store: string,
  name: string,
  target: string,
  agents: SkillRef['agents'] = [],
): SkillRef {
  return {
    path: `${store}/${name}`,
    store,
    agents,
    reachedBy: agents,
    health: { state: 'broken', detail: target },
    hops: [{ from: `${store}/${name}`, to: target, exists: false }],
    resolved: null,
  }
}

/** 两跳 + 重复，本机 14 条两跳链里最典型的那种。 */
const hyperframes: SkillEntry = {
  name: 'hyperframes',
  refs: [
    realDir(MAIN, 'hyperframes'),
    linked(MID, 'hyperframes', [`${MAIN}/hyperframes`]),
    // grok 的 `[compat.claude] skills` 让它也读 ~/.claude/skills，所以是两家。
    linked(CLAUDE, 'hyperframes', [`${MID}/hyperframes`, `${MAIN}/hyperframes`], ['claude', 'grok']),
    realDir(OTHER, 'hyperframes'),
  ],
  bodies: [body(MAIN, 'hyperframes', { risk: 'high' }), body(OTHER, 'hyperframes')],
  badges: ['duplicate', 'twoHop'],
  risk: 'high',
  truncated: false,
  description: 'Build hyperframe decks',
  git: null,
}

/** 死链，本机剩下的 2 个漏网之一。 */
const pinme: SkillEntry = {
  name: 'pinme',
  refs: [broken(CLAUDE, 'pinme', `${MID}/pinme`, ['claude', 'grok'])],
  bodies: [],
  badges: ['broken'],
  risk: 'none',
  truncated: false,
  description: null,
  git: null,
}

/** 干净的一条。 */
const gitPush: SkillEntry = {
  name: 'git-push',
  refs: [realDir(MAIN, 'git-push'), linked(CLAUDE, 'git-push', [`${MAIN}/git-push`], ['claude'])],
  bodies: [body(MAIN, 'git-push')],
  badges: [],
  risk: 'low',
  truncated: false,
  description: 'Commit and push',
  git: null,
}

/** 从远端 clone 来的那一类。本机 `humanizer` 就是这个形状。 */
const humanizer: SkillEntry = {
  name: 'humanizer',
  refs: [realDir(MID, 'humanizer'), linked(CLAUDE, 'humanizer', [`${MID}/humanizer`], ['claude'])],
  bodies: [body(MID, 'humanizer')],
  badges: [],
  risk: 'none',
  truncated: false,
  description: 'Rewrite AI-sounding text',
  git: { remote: 'https://github.com/blader/humanizer.git', branch: 'main' },
}

const skills = [hyperframes, pinme, gitPush]

function store(path: string, over: Partial<StoreCandidate> = {}): StoreCandidate {
  return {
    path,
    agents: [],
    scope: 'user',
    origin: 'own',
    exists: true,
    total: 0,
    links: 0,
    realDirs: 0,
    broken: 0,
    // 后端算的是 `scope === 'user' && origin === 'shared'`；装置里默认是 agent 自有
    // 目录（own），够格的那几个各自显式打开。
    canBeMain: false,
    ...over,
  }
}

/** 跨 agent 的公共 store —— 就这一类够格当主 store。 */
function shared(path: string, over: Partial<StoreCandidate> = {}): StoreCandidate {
  return store(path, { origin: 'shared', canBeMain: true, ...over })
}

const scan: SkillScan = {
  home: HOME,
  defaultMain: MID,
  stores: [
    store(CLAUDE, { agents: ['claude', 'grok'], total: 22, links: 22, broken: 1 }),
    shared(MID, { total: 18, links: 17, realDirs: 1 }),
    shared(MAIN, { total: 39, realDirs: 39 }),
    shared(OTHER, { total: 33, realDirs: 33 }),
    store(`${HOME}/.kimi-code/skills`, { agents: ['kimicode'], exists: false }),
    // 项目级：扫描结果里真的有（cwd 下的 `.agents/skills`），但换个项目就没了。
    store(`/Users/wuchao/develop/sales-app/.agents/skills`, {
      scope: 'project',
      origin: 'shared',
      total: 10,
      realDirs: 10,
    }),
    // agy 的自有目录：有 1 个实体内容，照样不能当主 store。
    store(`${HOME}/.gemini/config/skills`, { agents: ['agy'], total: 1, realDirs: 1 }),
  ],
  suggestedMain: MAIN,
  skills,
  summary: { total: 3, broken: 1, twoHop: 1, duplicate: 1, cyclic: 0, fromGit: 1 },
}

describe('角标优先级', () => {
  it('一行只显示最重的那个角标', () => {
    // 既断链又重复时先要解决的是断链 —— 重复至少还能用，断链完全不生效。
    expect(worstBadge(['duplicate', 'twoHop', 'broken'])).toBe('broken')
    expect(worstBadge(['duplicate', 'twoHop'])).toBe('twoHop')
    expect(worstBadge([])).toBeNull()
  })

  it('角标次序表覆盖了所有角标，漏一个就会排到最前面去', () => {
    const all = new Set(skills.flatMap((s) => s.badges))
    for (const badge of all) expect(BADGE_ORDER).toContain(badge)
    expect(BADGE_ORDER).toHaveLength(5)
  })

  it('风险取最高', () => {
    expect(worstRisk(['low', 'critical', 'medium'])).toBe('critical')
    expect(worstRisk([])).toBe('none')
    expect(riskRank('critical')).toBeGreaterThan(riskRank('high'))
  })
})

describe('两跳链', () => {
  it('链路整条画出来，不是只画第一跳', () => {
    // 只画一跳的话，「第一跳在、第二跳断了」看起来和健康的一模一样。
    const ref = hyperframes.refs.find((r) => r.store === CLAUDE)!
    const lines = chainLines(ref, HOME)
    expect(lines.map((l) => l.path)).toEqual([
      '~/.claude/skills/hyperframes',
      '~/.agents/skills/hyperframes',
      '~/.skills-manager/skills/hyperframes',
    ])
    expect(lines.map((l) => l.depth)).toEqual([0, 1, 2])
  })

  it('实体目录只有一行', () => {
    expect(chainLines(realDir(MAIN, 'x'), HOME)).toEqual([
      { path: '~/.skills-manager/skills/x', abs: `${MAIN}/x`, exists: true, depth: 0 },
    ])
  })

  it('断链的最后一行标出断点', () => {
    const lines = chainLines(pinme.refs[0], HOME)
    expect(lines[lines.length - 1]).toEqual({
      path: '~/.agents/skills/pinme',
      abs: `${HOME}/.agents/skills/pinme`,
      exists: false,
      depth: 1,
    })
  })

  it('每一行都留着没缩过的真路径', () => {
    // 「在文件管理器中显示」只认绝对路径，`~/…` 那个形状打不开。
    const ref = hyperframes.refs.find((r) => r.store === CLAUDE)!
    for (const l of chainLines(ref, HOME)) {
      expect(l.abs.startsWith('/')).toBe(true)
      expect(l.path.startsWith('~/')).toBe(true)
    }
  })

  it('受管副本把它记录的源也画出来', () => {
    const ref: SkillRef = {
      path: `${CLAUDE}/copied`,
      store: CLAUDE,
      agent: 'claude',
      health: { state: 'managedCopy', detail: `${MAIN}/copied` },
      hops: [],
      resolved: `${MAIN}/copied`,
    }
    expect(chainLines(ref, HOME).map((l) => l.path)).toEqual([
      '~/.claude/skills/copied',
      '~/.skills-manager/skills/copied',
    ])
  })
})

describe('能不能单独删这一份内容', () => {
  it('只有「没人够得着的实体目录」才给删除按钮', () => {
    expect(deletableBody(realDir(MAIN, 'x'))).toBe(true)
  })

  it('有 agent 直接读它的实体目录不给删 —— 那是「删除整个 skill」的事', () => {
    expect(deletableBody({ ...realDir(MAIN, 'x'), agents: ['claude'], reachedBy: ['claude'] })).toBe(
      false,
    )
  })

  /**
   * 真机上报出来的那一幕：`~/.skills-manager/skills/css-animations` 没有任何 agent
   * 直接扫 `.skills-manager`，`agents` 是空的 —— 但 `~/.claude/skills/css-animations`
   * 和 `~/.agents/skills/css-animations` 两条链最后都落在它身上。按 `agents` 判会在
   * 那一行长出删除按钮，点下去两条链一起断。
   */
  it('被别人的链子指着的实体目录不给删，哪怕没人直接扫它所在的目录', () => {
    expect(deletableBody(realDir(MAIN, 'x', ['claude', 'codex']))).toBe(false)
  })

  it('链接和断链都不给删 —— 拆入口走「停用」，清死链走「修链接」', () => {
    expect(deletableBody(linked(CLAUDE, 'x', [`${MAIN}/x`]))).toBe(false)
    expect(deletableBody(broken(CLAUDE, 'x', `${MAIN}/gone`))).toBe(false)
    expect(deletableBody({ ...realDir(MAIN, 'x'), health: { state: 'managedCopy', detail: `${MAIN}/y` } })).toBe(false)
  })
})

describe('没人读的活链接', () => {
  /**
   * 收编完成之后就是这一幕：旧 store（`~/.skills-manager`、`~/.cc-switch`）里各剩
   * 一条指向新家的链接，没有任何 agent 在扫那两个目录。原来一个入口都没有 ——
   * 「停用」是按 agent 关的，碰不到不属于任何一家的这条；「清理死链」只碰解析不到
   * 东西的死链，而这条好端端指着实体。
   */
  it('没有 agent 读的活链接给删除按钮', () => {
    expect(removableLink(linked(`${HOME}/.skills-manager/skills`, 'x', [`${MAIN}/x`]))).toBe(true)
  })

  /** 有 agent 在读的要关就去关那家的开关；两个入口做同一件事，其中一个还说不清断了谁。 */
  it('有 agent 读的不给 —— 那是「停用」的事', () => {
    expect(removableLink(linked(CLAUDE, 'x', [`${MAIN}/x`], ['claude']))).toBe(false)
  })

  /**
   * 中间节点：`~/.agents/skills/x` 自己没有一家直接扫，但 `~/.claude/skills/x` 的链
   * 从它身上经过。拆了它，claude 那条整根断掉。
   */
  it('别人链条的中途一跳不给 —— 拆了它上游整条就断了', () => {
    expect(removableLink(linked(MID, 'x', [`${MAIN}/x`], [], ['claude']))).toBe(false)
  })

  it('实体目录不给 —— 拆链接和删内容是两件事', () => {
    expect(removableLink(realDir(MAIN, 'x'))).toBe(false)
  })

  /** 死链有「修链 · 清理死链」兜着，在这儿再开一个入口是两个按钮做同一件事。 */
  it('死链不给 —— 那条走「修链接」', () => {
    expect(removableLink(broken(CLAUDE, 'x', `${MAIN}/gone`))).toBe(false)
  })

  /** 两个按钮永远不会同时出现在一行上：一个只认实体，一个只认链接。 */
  it('和「删这份内容」互斥，同一行上不会冒出两个垃圾桶', () => {
    const rows = [
      realDir(MAIN, 'x'),
      linked(`${HOME}/.cc-switch/skills`, 'x', [`${MAIN}/x`]),
      broken(CLAUDE, 'x', `${MAIN}/gone`),
    ]
    for (const r of rows) {
      expect(deletableBody(r) && removableLink(r), r.health.state).toBe(false)
    }
  })
})

describe('健康提示', () => {
  it('断链的提示里带上具体的目标路径', () => {
    // 「Broken」三个字用户没法修，得说清断在哪儿。
    expect(healthTip(pinme.refs[0].health, HOME)).toEqual({
      key: 'tools.skills.health.broken',
      vars: { target: '~/.agents/skills/pinme' },
    })
  })

  it('断链和成环都算不健康，其余不算', () => {
    expect(isUnhealthy(pinme.refs[0])).toBe(true)
    expect(isUnhealthy(realDir(MAIN, 'x'))).toBe(false)
    expect(isUnhealthy(linked(CLAUDE, 'x', [`${MAIN}/x`]))).toBe(false)
    expect(isUnhealthy({ ...realDir(MAIN, 'x'), health: { state: 'cyclic' } })).toBe(true)
  })
})

describe('同名重复归并', () => {
  it('内容散在几个 store 就报几个来源', () => {
    expect(bodyStores(hyperframes)).toEqual([MAIN, OTHER])
    expect(bodyStores(gitPush)).toEqual([MAIN])
    expect(bodyStores(pinme)).toEqual([])
  })

  it('引用它的 agent 去重后按出现次序排', () => {
    // 一条引用可以被好几家读到（grok 的 compat），去重后按出现次序。
    expect(agentsOf(hyperframes)).toEqual(['claude', 'grok'])
    expect(agentsOf(pinme)).toEqual(['claude', 'grok'])
  })

  it('健康条按角标计数，一条 skill 可以同时计进两栏', () => {
    expect(badgeCounts(skills)).toEqual({
      duplicate: 1,
      twoHop: 1,
      // 副本降级只在 Windows 上发生，Unix 的装置里恒为 0 —— 但这一栏必须在，
      // 少一个 key 就是 `out[b] += 1` 在 NaN 上累加。
      copyStale: 0,
      cyclic: 0,
      broken: 1,
    })
  })
})

describe('扫描不完整时', () => {
  it('风险等级只是下限，不能当结论', () => {
    // 危险脚本可能就在第 401 个文件里。把部分扫描结果呈现为「干净」比不扫更糟 ——
    // 用户会据此放心。
    expect(riskIsConclusive(gitPush)).toBe(true)
    expect(riskIsConclusive({ truncated: true })).toBe(false)
  })
})

describe('指向非目录的链接', () => {
  const notDir: SkillRef = {
    path: `${CLAUDE}/textfile`,
    store: CLAUDE,
    agents: ['claude'],
    health: { state: 'notADirectory', detail: `${MAIN}/textfile` },
    hops: [{ from: `${CLAUDE}/textfile`, to: `${MAIN}/textfile`, exists: true }],
    resolved: null,
  }

  it('算不健康 —— 它「存在」但没有任何内容可用', () => {
    // git 在 Windows 上默认 core.symlinks=false，仓库里的 symlink 会被检出成文本文件。
    expect(isUnhealthy(notDir)).toBe(true)
  })

  it('提示里指出那个目标不是目录', () => {
    expect(healthTip(notDir.health, HOME)).toEqual({
      key: 'tools.skills.health.notADirectory',
      vars: { target: '~/.skills-manager/skills/textfile' },
    })
  })
})

describe('受管副本的四种毛病（验收 #8）', () => {
  const SOURCE = `${HOME}/.agents/skills/alpha`

  // 副本降级只发生在 Windows，Unix 的装置数据里没有这几态 —— 但 `healthTip` 是
  // 平台无关的纯函数，漏掉任何一支，用户在 Windows 上看到的就是一行 key 字面量。
  it('四种状态各有各的说法，路径都缩写过', () => {
    for (const state of ['copyStale', 'copyEdited', 'copyDiverged', 'copyOrphaned'] as const) {
      expect(healthTip({ state, detail: SOURCE }, HOME)).toEqual({
        key: `tools.skills.health.${state}`,
        vars: { source: '~/.agents/skills/alpha' },
      })
    }
  })

  it('「副本过期」排在断链前面 —— 内容还在，只是旧了', () => {
    expect(worstBadge(['copyStale', 'broken'])).toBe('broken')
    expect(worstBadge(['duplicate', 'copyStale'])).toBe('copyStale')
  })
})

describe('搜索与过滤', () => {
  it('名字、描述、路径都能搜到', () => {
    expect(matchesQuery(hyperframes, 'hyper')).toBe(true)
    expect(matchesQuery(hyperframes, 'decks')).toBe(true)
    // 用户常常是「装在 cc-switch 里的那批」这样找的。
    expect(matchesQuery(hyperframes, 'cc-switch')).toBe(true)
    expect(matchesQuery(gitPush, 'cc-switch')).toBe(false)
  })

  it('空搜索不过滤', () => {
    expect(filterSkills(skills, emptyFilter())).toHaveLength(3)
    expect(filterSkills(skills, { ...emptyFilter(), query: '   ' })).toHaveLength(3)
  })

  it('按角标过滤', () => {
    const only = filterSkills(skills, { ...emptyFilter(), badge: 'broken' })
    expect(only.map((s) => s.name)).toEqual(['pinme'])
  })

  it('按 agent 过滤看的是引用，不是内容所在地', () => {
    // 内容躺在 ~/.skills-manager 里，但引用它的是 claude —— 用户问的是「claude 能用哪些」。
    const mine = filterSkills(skills, { ...emptyFilter(), agent: 'claude' })
    expect(mine).toHaveLength(3)
    expect(filterSkills(skills, { ...emptyFilter(), agent: 'codex' })).toHaveLength(0)
    // grok 通过 compat 读同一个目录，所以它也能用到这些。
    expect(filterSkills(skills, { ...emptyFilter(), agent: 'grok' })).toHaveLength(2)
  })

  it('风险下限是「不低于」，不是「等于」', () => {
    expect(filterSkills(skills, { ...emptyFilter(), minRisk: 'medium' }).map((s) => s.name)).toEqual([
      'hyperframes',
    ])
    expect(filterSkills(skills, { ...emptyFilter(), minRisk: 'none' })).toHaveLength(3)
  })

  it('多个条件同时生效', () => {
    const out = filterSkills(skills, {
      query: 'hyper',
      agent: 'claude',
      badge: 'duplicate',
      minRisk: 'high',
    })
    expect(out.map((s) => s.name)).toEqual(['hyperframes'])
  })
})

describe('来源过滤', () => {
  // 单独一份：别的用例断言的是整张列表，往共享的 `skills` 里塞一条会把它们全打乱。
  const mixed = [...skills, humanizer]

  it('只看从远端 clone 来的', () => {
    const only = filterSkills(mixed, { ...emptyFilter(), fromGit: true })
    expect(only.map((s) => s.name)).toEqual(['humanizer'])
  })

  it('不开这个开关就不过滤', () => {
    expect(filterSkills(mixed, emptyFilter())).toHaveLength(mixed.length)
  })

  it('和角标各管各的 —— 来源不是毛病', () => {
    // 「来自 github」和「重复 / 断链」是两个维度：一个 clone 照样可以是重复的，
    // 装置里那条不是，所以两个条件一起用就该是空。
    expect(filterSkills(mixed, { ...emptyFilter(), fromGit: true, badge: 'duplicate' })).toHaveLength(0)
    expect(filterSkills(mixed, { ...emptyFilter(), badge: 'duplicate' }).map((s) => s.name)).toEqual([
      'hyperframes',
    ])
  })
})

describe('排序', () => {
  it('有问题的排前面，同档按风险再按名字', () => {
    // 这个面板存在的理由就是「机器上没东西告诉你哪些坏了」，按字母序等于把结论埋起来。
    expect(sortSkills(skills).map((s) => s.name)).toEqual(['pinme', 'hyperframes', 'git-push'])
  })

  it('不改原数组', () => {
    const before = skills.map((s) => s.name)
    sortSkills(skills)
    expect(skills.map((s) => s.name)).toEqual(before)
  })

  it('置顶压过 badge 和风险', () => {
    // 自动排序猜的是「你大概最该先看哪个」，置顶是用户自己说的「我就要看这个」。
    // git-push 没角标没风险，本来排最后。
    expect(sortSkills(skills, ['git-push']).map((s) => s.name)).toEqual([
      'git-push',
      'pinme',
      'hyperframes',
    ])
  })

  it('多个置顶按置顶顺序排，不跟着 badge 重排', () => {
    // 那一列的次序是用户自己攒出来的，不该被自动规则搅乱。
    expect(sortSkills(skills, ['git-push', 'hyperframes']).map((s) => s.name)).toEqual([
      'git-push',
      'hyperframes',
      'pinme',
    ])
  })

  it('置顶了一条不存在的 skill 不影响其余排序', () => {
    // 收编 / 删除之后置顶列里可能留着已经没了的名字。
    expect(sortSkills(skills, ['gone']).map((s) => s.name)).toEqual(
      sortSkills(skills).map((s) => s.name),
    )
  })

  it('visibleSkills 把置顶一路传到排序', () => {
    const out = visibleSkills(skills, emptyFilter(), [], ['git-push'])
    expect(out[0].name).toBe('git-push')
  })
})

describe('搜索时的排序', () => {
  // 真机上的原始报告：搜 `hyperframes`，49 条里命中 12 条，而同名那一条自己**一个角标
  // 都没有**，按健康度被 11 条「重复」压到列表最底下 —— 用户打完字在第一屏看不到它，
  // 得出的结论是「搜索后列表没有被过滤」。
  it('名字完全相等的排第一，压过「坏得最厉害的排前面」', () => {
    const out = visibleSkills(skills, { ...emptyFilter(), query: 'hyperframes' }, [])
    expect(out[0].name).toBe('hyperframes')
  })

  it('名字 > 描述 > 路径', () => {
    expect(queryRank(hyperframes, 'hyperframes')).toBe(0)
    expect(queryRank(hyperframes, 'hyper')).toBe(1)
    expect(queryRank(hyperframes, 'frames')).toBe(2)
    // 描述里有、名字里没有
    expect(queryRank(hyperframes, 'decks')).toBe(3)
    // 只落在路径上
    expect(queryRank(hyperframes, 'cc-switch')).toBe(4)
  })

  it('没搜索时这一档整个透明 —— 排序和从前逐字相同', () => {
    expect(queryRank(hyperframes, '')).toBe(0)
    expect(queryRank(gitPush, '   ')).toBe(0)
    expect(sortSkills(skills, [], '').map((s) => s.name)).toEqual(
      sortSkills(skills).map((s) => s.name),
    )
  })

  it('置顶仍然压过命中位置 —— 那是用户自己说的「我就要看这个」', () => {
    // `i` 三条都命中：pinme / git-push 落在名字上（2），hyperframes 落在描述里（3）。
    // 不置顶的话 hyperframes 排最后。
    const plain = visibleSkills(skills, { ...emptyFilter(), query: 'i' }, [])
    expect(plain.map((s) => s.name)).toEqual(['pinme', 'git-push', 'hyperframes'])
    const pinned = visibleSkills(skills, { ...emptyFilter(), query: 'i' }, [], ['hyperframes'])
    expect(pinned.map((s) => s.name)).toEqual(['hyperframes', 'pinme', 'git-push'])
  })
})

describe('只命中路径的那一行', () => {
  // 名字和描述里都没有刚打的那个词，整行看上去和搜索毫无关系。
  it('给出命中的那条路径', () => {
    expect(queryPath(hyperframes, 'cc-switch')).toContain('cc-switch')
  })

  it('名字或描述已经命中就不顶路径 —— 那一行自己会解释自己', () => {
    expect(queryPath(hyperframes, 'hyper')).toBeNull()
    expect(queryPath(hyperframes, 'decks')).toBeNull()
  })

  it('没搜索就没有这回事', () => {
    expect(queryPath(hyperframes, '')).toBeNull()
  })

  it('一处都没命中时是 null，不是瞎给一条路径', () => {
    expect(queryPath(gitPush, 'cc-switch')).toBeNull()
  })
})

describe('置顶', () => {
  beforeEach(() => {
    localStorage.clear()
    pinnedSkills.value = []
  })

  it('置顶 / 取消置顶落到 localStorage', () => {
    toggleSkillPin('pinme')
    expect(isSkillPinned('pinme')).toBe(true)
    expect(JSON.parse(localStorage.getItem('toolsSkillPins:v1')!)).toEqual(['pinme'])

    toggleSkillPin('pinme')
    expect(isSkillPinned('pinme')).toBe(false)
    expect(JSON.parse(localStorage.getItem('toolsSkillPins:v1')!)).toEqual([])
  })

  it('新置顶排在已置顶那批后面', () => {
    // 排在前面的话，每钉一条已经钉好的那几条就整体下移一格，置顶列的次序会跳。
    toggleSkillPin('a')
    toggleSkillPin('b')
    expect(pinnedSkills.value).toEqual(['a', 'b'])
  })
})

describe('agent 读不读得到', () => {
  // 本机 `tailwind` 的形状：一条在 ~/.claude/skills（claude + grok 读），一条在
  // 公共目录（grok / opencode / pi 读）。同一屏上「行里的角标」和「详情的开关」
  // 一度对不上，就是因为开关只认「这家自己的目录」。
  const tailwind: SkillEntry = {
    name: 'tailwind',
    refs: [
      realDir(MAIN, 'tailwind'),
      linked(CLAUDE, 'tailwind', [`${MAIN}/tailwind`], ['claude', 'grok']),
      linked(MID, 'tailwind', [`${MAIN}/tailwind`], ['grok', 'opencode', 'pi']),
    ],
    bodies: [body(MAIN, 'tailwind')],
    badges: [],
    risk: 'none',
    truncated: false,
    description: 'Tailwind conventions',
    // 本机这条是手写的，不是 clone。
    git: null,
  }

  it('自己目录里那条 → own，能关', () => {
    const reach = agentReach(tailwind, 'claude', CLAUDE)
    expect(reach.state).toBe('own')
    expect(reach.own?.store).toBe(CLAUDE)
  })

  it('只靠共用目录读到 → shared，不是 off', () => {
    // 这里是那个 bug 的回归点：opencode 自己的目录里什么都没有，但它读公共目录。
    const reach = agentReach(tailwind, 'opencode', `${HOME}/.config/opencode/skill`)
    expect(reach.state).toBe('shared')
    expect(reach.own).toBeNull()
    expect(reach.via.map((r) => r.store)).toEqual([MID])
  })

  it('两个目录都读得到 → own 优先（停用只动自己那条）', () => {
    const reach = agentReach(tailwind, 'grok', CLAUDE)
    expect(reach.state).toBe('own')
    expect(reach.own?.store).toBe(CLAUDE)
    expect(reach.via.map((r) => r.store)).toEqual([CLAUDE, MID])
  })

  it('一条都没有 → off', () => {
    expect(agentReach(tailwind, 'codex', `${HOME}/.codex/skills`).state).toBe('off')
  })

  it('这家不支持 skills（没有自有目录）也不会崩', () => {
    expect(agentReach(tailwind, 'pi', null).state).toBe('shared')
  })

  it('断链不算读得到', () => {
    expect(agentReach(pinme, 'claude', CLAUDE).state).toBe('off')
  })
})

describe('公共目录', () => {
  it('里面有健康引用就算开着', () => {
    expect(sharedDirRef(hyperframes, MID)?.store).toBe(MID)
  })

  it('没有引用就是关着', () => {
    expect(sharedDirRef(gitPush, MID)).toBeNull()
  })

  it('死链不算 —— 读不到东西', () => {
    // pinme 在 ~/.claude/skills 有条断链，公共目录这边什么都没有。
    expect(sharedDirRef(pinme, MID)).toBeNull()
  })

  it('还没扫出来（没有公共目录）时不算开着', () => {
    expect(sharedDirRef(hyperframes, null)).toBeNull()
  })
})

describe('store', () => {
  it('只有跨 agent 的用户级共享目录能当主 store', () => {
    const options = mainStoreOptions(scan)
    expect(options.map((s) => s.path)).toEqual([MAIN, OTHER, MID])
  })

  it('项目目录不能当主 store —— 换个项目就没了，别的项目全断链', () => {
    const paths = mainStoreOptions(scan).map((s) => s.path)
    expect(paths).not.toContain('/Users/wuchao/develop/sales-app/.agents/skills')
  })

  it('agent 自有目录不能当主 store —— 那是链接落脚的地方，不是内容所在地', () => {
    // 搬进去就从「一份内容所有 agent 共享」变成「绑死在某一家」，和这个面板要做的
    // 事正好相反。有实体内容也不行。
    const paths = mainStoreOptions(scan).map((s) => s.path)
    expect(paths).not.toContain(`${HOME}/.gemini/config/skills`)
    expect(paths).not.toContain(CLAUDE)
  })

  it('不存在的 store 不进候选', () => {
    expect(mainStoreOptions(scan).some((s) => !s.exists)).toBe(false)
  })

  it('兜底目录哪怕不存在也要进候选 —— 新机器上下拉不能是空的', () => {
    // 一个 store 都没有时，用户得有个地方开始；第一次收编会把它建出来。
    const fresh: SkillScan = {
      ...scan,
      stores: [store(CLAUDE, { agents: ['claude'] }), shared(MID, { exists: false })],
    }
    expect(mainStoreOptions(fresh).map((s) => s.path)).toEqual([MID])
  })

  it('空的共享目录也能当主 store —— 不再要求有实体内容', () => {
    const fresh: SkillScan = { ...scan, stores: [shared(MID, { total: 0, realDirs: 0 })] }
    expect(mainStoreOptions(fresh).map((s) => s.path)).toEqual([MID])
  })

  it('Windows 的反斜杠路径照样缩写', () => {
    // 后端在 Windows 上回的是 `C:\\Users\\me\\...`。只判 `/` 的话每条路径都缩不掉，
    // 整个面板会摊开一屏的 `C:\\Users\\…`。缩完保留原来的分隔符。
    const win = 'C:\\Users\\me'
    expect(shortenPath('C:\\Users\\me\\.agents\\skills', win)).toBe('~\\.agents\\skills')
    expect(shortenPath(win, win)).toBe('~')
    // 前缀像但不是同一个目录的，不能误缩。
    expect(shortenPath('C:\\Users\\meow\\x', win)).toBe('C:\\Users\\meow\\x')
  })

  it('路径缩写只在 home 下生效', () => {
    expect(shortenPath(`${HOME}/.claude/skills`, HOME)).toBe('~/.claude/skills')
    expect(shortenPath(HOME, HOME)).toBe('~')
    expect(shortenPath('/opt/skills', HOME)).toBe('/opt/skills')
    // `/Users/wuchao2` 不是 home 的子路径，别误伤。
    expect(shortenPath(`${HOME}2/x`, HOME)).toBe(`${HOME}2/x`)
    expect(shortenPath('/opt/x', '')).toBe('/opt/x')
  })
})

describe('面板状态', () => {
  beforeEach(() => resetSkillFilter())

  it('重置会把所有条件清干净', () => {
    skillFilter.value = { query: 'x', agent: 'codex', badge: 'broken', minRisk: 'high' }
    resetSkillFilter()
    expect(skillFilter.value).toEqual(emptyFilter())
  })
})
