// 工具管理 · Skills 写操作的纯逻辑。面板和对话框在覆盖率排除的目录里，
// 「收编哪些、冲突怎么记、修复接到哪」这些判断只有在这儿才测得到。

import { beforeEach, describe, expect, it } from 'vitest'
import {
  addExtraStore,
  adoptAllTargets,
  adoptTargets,
  conflictDone,
  currentConflict,
  effectiveMainStore,
  extraStores,
  inMainStore,
  incompleteSteps,
  isDestructive,
  mainStore,
  needsRepair,
  removeExtraStore,
  repairPlan,
  repairTarget,
  requestsWithResolutions,
  resolveCurrent,
  setMainStore,
  skipAll,
  skipCurrent,
  startConflicts,
  stepCounts,
  updateIsNoop,
  updateMessageParts,
} from '../src/toolsSkillsActions'
import type {
  AdoptConflict,
  SkillBody,
  SkillUpdateCheck,
  SkillEntry,
  SkillRef,
  SkillScan,
  StoreCandidate,
  WriteReport,
  WriteStep,
} from '../src/types'

const MAIN = '/home/u/.skills-manager/skills'

const body = (over: Partial<SkillBody> & { path: string }): SkillBody => ({
  store: null,
  files: 3,
  bytes: 100,
  modified: 0,
  risk: 'none',
  truncated: false,
  ...over,
})

const ref = (over: Partial<SkillRef> & { path: string }): SkillRef => ({
  store: '/home/u/.claude/skills',
  agents: ['claude'],
  health: { state: 'linked' },
  hops: [{ from: over.path, to: `${MAIN}/x`, exists: true }],
  resolved: `${MAIN}/x`,
  ...over,
})

const entry = (over: Partial<SkillEntry> & { name: string }): SkillEntry => ({
  refs: [],
  bodies: [],
  badges: [],
  risk: 'none',
  truncated: false,
  description: null,
  git: null,
  ...over,
})

const store = (over: Partial<StoreCandidate> & { path: string }): StoreCandidate => ({
  agents: [],
  scope: 'user',
  origin: 'own',
  exists: true,
  total: 1,
  links: 0,
  realDirs: 1,
  broken: 0,
  canBeMain: true,
  ...over,
})

const scan = (over: Partial<SkillScan> = {}): SkillScan => ({
  home: '/home/u',
  defaultMain: '/home/u/.agents/skills',
  stores: [store({ path: MAIN, realDirs: 39 })],
  suggestedMain: MAIN,
  skills: [],
  summary: { total: 0, broken: 0, twoHop: 0, duplicate: 0, cyclic: 0, fromGit: 0 },
  ...over,
})

const step = (over: Partial<WriteStep> & { kind: WriteStep['kind'] }): WriteStep => ({
  path: '/p',
  target: null,
  note: null,
  done: true,
  ...over,
})

const report = (over: Partial<WriteReport> = {}): WriteReport => ({
  dryRun: false,
  steps: [],
  conflicts: [],
  ...over,
})

const conflict = (name: string): AdoptConflict => ({
  name,
  main: { path: `${MAIN}/${name}`, files: 3, bytes: 100, modified: 0, truncated: false },
  external: { path: `/home/u/.cc-switch/skills/${name}`, files: 4, bytes: 120, modified: 0, truncated: false },
  files: [],
  skillMd: { plus: 2, minus: 1, truncated: false, hunks: [], clipped: false },
  suggestedRename: `${name}-from-cc-switch`,
})

beforeEach(() => {
  localStorage.clear()
  mainStore.value = null
})

describe('主 store', () => {
  it('记住用户选的那个', () => {
    setMainStore(MAIN)
    expect(mainStore.value).toBe(MAIN)
    expect(localStorage.getItem('toolsMainStore:v1')).toBe(MAIN)
  })

  it('没选过就用扫描给的建议', () => {
    expect(effectiveMainStore(scan())).toBe(MAIN)
  })

  it('选过的目录这次扫不到了就退回建议 —— 否则会往一个不存在的地方搬', () => {
    setMainStore('/home/u/.gone/skills')
    expect(effectiveMainStore(scan())).toBe(MAIN)
  })

  it('目录还在就用用户选的，不管建议是什么', () => {
    const other = '/home/u/.cc-switch/skills'
    setMainStore(other)
    expect(
      effectiveMainStore(scan({ stores: [store({ path: MAIN }), store({ path: other })] })),
    ).toBe(other)
  })

  it('存着的目录不够格当主 store 就退回建议', () => {
    // 早期下拉里混进过项目目录和 agent 自有目录，有人可能已经选中存下来了。
    // 光看「还在不在」放不掉这种 —— 它照样存在。
    const agy = '/home/u/.gemini/config/skills'
    setMainStore(agy)
    expect(
      effectiveMainStore(
        scan({ stores: [store({ path: MAIN }), store({ path: agy, canBeMain: false })] }),
      ),
    ).toBe(MAIN)
  })
})

describe('收编计划', () => {
  it('已经在主 store 里的那份不算收编对象', () => {
    // 它就是收编的**去处**。算进去的话按钮亮着、点了什么也不会发生。
    const e = entry({ name: 'x', bodies: [body({ path: `${MAIN}/x` })] })
    expect(adoptTargets(e, MAIN)).toEqual([])
  })

  it('外部那几份逐个列出来', () => {
    const e = entry({
      name: 'x',
      bodies: [
        body({ path: `${MAIN}/x` }),
        body({ path: '/home/u/.cc-switch/skills/x' }),
        body({ path: '/home/u/.agents/skills/x' }),
      ],
    })
    expect(adoptTargets(e, MAIN).map((r) => r.body)).toEqual([
      '/home/u/.cc-switch/skills/x',
      '/home/u/.agents/skills/x',
    ])
    expect(adoptTargets(e, MAIN).every((r) => r.resolution === null)).toBe(true)
  })

  it('前缀匹配不能误伤名字相近的目录', () => {
    // `/skills-manager/skills-backup` 不在 `/skills-manager/skills` 里。
    expect(inMainStore(`${MAIN}-backup/x`, MAIN)).toBe(false)
    expect(inMainStore(`${MAIN}/x`, MAIN)).toBe(true)
    expect(inMainStore(MAIN, MAIN)).toBe(true)
  })

  it('Windows 的反斜杠路径也认得出「已经在主 store 里」', () => {
    // 只判 `/` 的话 Windows 上每一条内容都会被判成「不在主 store 里」，
    // 收编会把已经在里面的东西再搬一遍。
    const win = 'C:\\Users\\me\\.agents\\skills'
    expect(inMainStore(`${win}\\pinme`, win)).toBe(true)
    expect(inMainStore(win, win)).toBe(true)
    expect(inMainStore(`${win}-backup\\pinme`, win)).toBe(false)
  })

  it('整批收编把所有 skill 的外部内容汇到一起', () => {
    const s = scan({
      skills: [
        entry({ name: 'a', bodies: [body({ path: '/ext/a' }), body({ path: `${MAIN}/a` })] }),
        entry({ name: 'b', bodies: [body({ path: '/ext/b' })] }),
      ],
    })
    expect(adoptAllTargets(s, MAIN).map((r) => `${r.name}:${r.body}`)).toEqual([
      'a:/ext/a',
      'b:/ext/b',
    ])
  })
})

describe('冲突三选一', () => {
  it('逐条走，选过的记下来', () => {
    let s = startConflicts([conflict('a'), conflict('b')])
    expect(currentConflict(s)?.name).toBe('a')
    s = resolveCurrent(s, { kind: 'keepMain' })
    expect(currentConflict(s)?.name).toBe('b')
    s = resolveCurrent(s, { kind: 'keepBoth', value: 'b-from-cc-switch' })
    expect(conflictDone(s)).toBe(true)
    expect(s.resolutions).toEqual({
      a: { kind: 'keepMain' },
      b: { kind: 'keepBoth', value: 'b-from-cc-switch' },
    })
  })

  it('跳过的条目会被整条剔掉，不是带着「还没选」再发一次', () => {
    // 带 resolution: null 再发一次的话，后端会把它当成「还没选」又报回一个冲突 ——
    // 用户点「跳过」等于没跳过，面板会一直弹同一个框。
    let s = startConflicts([conflict('a'), conflict('b')])
    s = skipCurrent(s)
    s = resolveCurrent(s, { kind: 'useExternal' })
    const requests = [
      { name: 'a', body: '/ext/a', resolution: null },
      { name: 'b', body: '/ext/b', resolution: null },
    ]
    expect(requestsWithResolutions(requests, s)).toEqual([
      { name: 'b', body: '/ext/b', resolution: { kind: 'useExternal' } },
    ])
  })

  it('「全部跳过」只影响还没处理的，已经选过的仍然算数', () => {
    let s = startConflicts([conflict('a'), conflict('b'), conflict('c')])
    s = resolveCurrent(s, { kind: 'keepMain' })
    s = skipAll(s)
    expect(conflictDone(s)).toBe(true)
    expect(s.skipped).toEqual(['b', 'c'])
    const requests = ['a', 'b', 'c'].map((n) => ({ name: n, body: `/ext/${n}`, resolution: null }))
    expect(requestsWithResolutions(requests, s).map((r) => r.name)).toEqual(['a'])
  })
})

describe('修复计划', () => {
  it('两跳链算要修 —— 它「能用」，但中间断一跳整条就废', () => {
    const twoHop = ref({
      path: '/home/u/.claude/skills/smux',
      hops: [
        { from: '/home/u/.claude/skills/smux', to: '/home/u/.agents/skills/smux', exists: true },
        { from: '/home/u/.agents/skills/smux', to: `${MAIN}/smux`, exists: true },
      ],
    })
    expect(needsRepair(twoHop)).toBe(true)
    expect(repairPlan(entry({ name: 'smux', refs: [twoHop] }), `${MAIN}/smux`)).toEqual([
      { path: '/home/u/.claude/skills/smux', action: { kind: 'relink', value: `${MAIN}/smux` } },
    ])
  })

  it('健康的一跳链不动它', () => {
    const ok = ref({ path: '/home/u/.claude/skills/x' })
    expect(needsRepair(ok)).toBe(false)
    expect(repairPlan(entry({ name: 'x', refs: [ok] }), `${MAIN}/x`)).toEqual([])
  })

  it('死链：接得回就接，接不回才清掉', () => {
    const dead = ref({
      path: '/home/u/.claude/skills/pinme',
      health: { state: 'broken', detail: `${MAIN}/pinme` },
      hops: [{ from: '/home/u/.claude/skills/pinme', to: `${MAIN}/pinme`, exists: false }],
      resolved: null,
    })
    const e = entry({ name: 'pinme', refs: [dead] })
    expect(repairPlan(e, `${MAIN}/pinme`)).toEqual([
      { path: '/home/u/.claude/skills/pinme', action: { kind: 'relink', value: `${MAIN}/pinme` } },
    ])
    expect(repairPlan(e, null)).toEqual([
      { path: '/home/u/.claude/skills/pinme', action: { kind: 'unlink' } },
    ])
  })

  it('没有内容可接时，活着的链一条都不碰', () => {
    // 那是「停用」，不是「修复」—— 修复不该顺手把能用的东西拆了。
    const twoHop = ref({
      path: '/a',
      hops: [
        { from: '/a', to: '/b', exists: true },
        { from: '/b', to: '/c', exists: true },
      ],
      resolved: '/c',
    })
    expect(repairPlan(entry({ name: 'x', refs: [twoHop] }), null)).toEqual([])
  })

  it('实体目录不是链接，修不了也不用修', () => {
    const real = ref({ path: `${MAIN}/x`, health: { state: 'realDir' }, hops: [], resolved: `${MAIN}/x` })
    expect(repairPlan(entry({ name: 'x', refs: [real] }), `${MAIN}/x`)).toEqual([])
  })

  it('接回的目标优先选主 store 里的那份', () => {
    const e = entry({
      name: 'x',
      bodies: [body({ path: '/home/u/.cc-switch/skills/x' }), body({ path: `${MAIN}/x` })],
    })
    expect(repairTarget(e, MAIN)).toBe(`${MAIN}/x`)
    expect(repairTarget(e, null)).toBe('/home/u/.cc-switch/skills/x')
    expect(repairTarget(entry({ name: 'x' }), MAIN)).toBe(null)
  })
})

describe('写报告', () => {
  it('按类型计数，确认框用它说人话', () => {
    const r = report({
      steps: [step({ kind: 'unlink' }), step({ kind: 'unlink' }), step({ kind: 'deleteDir' })],
    })
    expect(stepCounts(r).unlink).toBe(2)
    expect(stepCounts(r).deleteDir).toBe(1)
    expect(stepCounts(r).link).toBe(0)
  })

  it('计划里有删目录就算危险', () => {
    expect(isDestructive(report({ steps: [step({ kind: 'link' })] }))).toBe(false)
    expect(isDestructive(report({ steps: [step({ kind: 'deleteDir' })] }))).toBe(true)
  })

  it('dry-run 的 done 全是 false，不该被当成「没做完」', () => {
    const plan = report({ dryRun: true, steps: [step({ kind: 'link', done: false })] })
    expect(incompleteSteps(plan)).toBe(0)
    const real = report({ steps: [step({ kind: 'link', done: false })] })
    expect(incompleteSteps(real)).toBe(1)
  })
})

describe('用户自己加的目录', () => {
  beforeEach(() => {
    localStorage.clear()
    extraStores.value = []
    setMainStore(null)
  })

  it('加进来会落到 localStorage', () => {
    addExtraStore('/Volumes/ssd/skills')
    expect(extraStores.value).toEqual(['/Volumes/ssd/skills'])
    expect(JSON.parse(localStorage.getItem('toolsExtraStores:v1')!)).toEqual(['/Volumes/ssd/skills'])
  })

  it('同一个目录加两次只算一条', () => {
    // 重复一条只会让下拉里多一行一模一样的，选哪个都一样。
    addExtraStore('/Volumes/ssd/skills')
    addExtraStore('/Volumes/ssd/skills')
    expect(extraStores.value).toEqual(['/Volumes/ssd/skills'])
  })

  it('去掉的正是当前主 store 时要松手', () => {
    // 不松手的话，收编会往一个后端已经不扫的目录搬 —— 搬完那些 skill 就从面板上消失了。
    addExtraStore('/Volumes/ssd/skills')
    setMainStore('/Volumes/ssd/skills')
    removeExtraStore('/Volumes/ssd/skills')
    expect(extraStores.value).toEqual([])
    expect(mainStore.value).toBeNull()
  })

  it('去掉别的目录不碰当前主 store', () => {
    addExtraStore('/Volumes/ssd/skills')
    addExtraStore('/Volumes/ssd/other')
    setMainStore('/Volumes/ssd/skills')
    removeExtraStore('/Volumes/ssd/other')
    expect(mainStore.value).toBe('/Volumes/ssd/skills')
  })
})

describe('从远端更新', () => {
  const check = (over: Partial<SkillUpdateCheck> = {}): SkillUpdateCheck => ({
    remote: 'https://github.com/blader/humanizer.git',
    branch: 'main',
    local: 'a1b2c3d',
    latest: 'e4f5a6b',
    behind: 3,
    changed: [],
    untracked: 0,
    ...over,
  })

  const keys = (c: SkillUpdateCheck) => updateMessageParts(c).map((p) => p.key)

  it('已最新 + 没改过 = 没什么可做', () => {
    expect(updateIsNoop(check({ behind: 0, latest: 'a1b2c3d' }))).toBe(true)
  })

  it('已最新但本地改过，仍然有事可做 —— 还原', () => {
    // 这是「我把它改坏了，想拉回官方版本」那一种，不能当成没事发生。
    expect(updateIsNoop(check({ behind: 0, changed: ['SKILL.md'] }))).toBe(false)
  })

  it('落后就有事可做', () => {
    expect(updateIsNoop(check())).toBe(false)
  })

  it('没改过就不吓那一句', () => {
    // 每次都吓一句「会强制覆盖」，用户下次就不看了 —— 真要覆盖时也不看。
    expect(keys(check())).toEqual(['tools.skills.update.from', 'tools.skills.update.behind'])
  })

  it('改过才出现覆盖警告，并带上文件数和名字', () => {
    const parts = updateMessageParts(check({ changed: ['SKILL.md', 'config.json'] }))
    const warn = parts.find((p) => p.key === 'tools.skills.update.overwrite')
    expect(warn?.vars).toEqual({ n: '2', files: 'SKILL.md, config.json' })
  })

  it('改过的文件太多就省略，不把弹框撑成一屏', () => {
    const parts = updateMessageParts(check({ changed: ['a', 'b', 'c', 'd', 'e'] }))
    const warn = parts.find((p) => p.key === 'tools.skills.update.overwrite')
    expect(warn?.vars).toEqual({ n: '5', files: 'a, b, c …' })
  })

  it('未跟踪的文件单独说 —— 它们不会被动', () => {
    const parts = updateMessageParts(check({ untracked: 2 }))
    expect(parts.find((p) => p.key === 'tools.skills.update.untracked')?.vars).toEqual({ n: '2' })
    // 一个都没有时不多说一句废话。
    expect(keys(check({ untracked: 0 }))).not.toContain('tools.skills.update.untracked')
  })

  it('已最新时说的是「已经是最新」，不是「落后 0 个提交」', () => {
    expect(keys(check({ behind: 0, changed: ['SKILL.md'] }))).toEqual([
      'tools.skills.update.from',
      'tools.skills.update.current',
      'tools.skills.update.overwrite',
    ])
  })
})
