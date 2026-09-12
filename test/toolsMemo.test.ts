// 工具管理 · 全局配置纯逻辑。
//
// 装置数据照本机实测的形状造：`~/.claude/CLAUDE.md` 全文只有一行 `@RTK.md`（相对），
// `~/.codex/AGENTS.md` 只有一行绝对路径的 `@…/RTK.md`，两个 RTK.md 内容已经漂了；
// grok / opencode 自己那份没有、回退去读 Claude 的；pi 除了自己那份 `AGENTS.md`，
// 还被 `npm:pi-memory` 额外喂一份 `MEMORY.md`。

import { describe, it, expect, beforeEach } from 'vitest'
import type { MemoAgentInfo, MemoFile, MemoScan } from '../src/types'
import { resetToolsPanel, toggleToolsAgent } from '../src/toolsPanel'
import {
  allMemoRows,
  baseName,
  blankRevision,
  canSyncFork,
  editable,
  externallyChanged,
  effectiveOf,
  forkOf,
  forkOthers,
  formatBytes,
  importsOf,
  memoRows,
  otherReaders,
  sameRevision,
  takeoverPath,
  template,
} from '../src/toolsMemo'

const HOME = '/Users/me'
const CLAUDE_MD = `${HOME}/.claude/CLAUDE.md`
const CLAUDE_RTK = `${HOME}/.claude/RTK.md`
const CODEX_MD = `${HOME}/.codex/AGENTS.md`
const CODEX_RTK = `${HOME}/.codex/RTK.md`
const GROK_MD = `${HOME}/.grok/AGENTS.md`
const OPENCODE_MD = `${HOME}/.config/opencode/AGENTS.md`
const AGY_MD = `${HOME}/.gemini/config/GEMINI.md`
const PI_MD = `${HOME}/.pi/agent/AGENTS.md`
const PI_MEMORY = `${HOME}/.pi/agent/memory/MEMORY.md`

const rev = (size: number) => ({ exists: size > 0, size, mtimeMs: size > 0 ? 1_700_000_000_000 : null })

function file(over: Partial<MemoFile> & { path: string }): MemoFile {
  return {
    name: baseName(over.path),
    exists: true,
    bytes: 100,
    revision: rev(100),
    link: null,
    readers: [],
    importedBy: null,
    imports: [],
    error: null,
    ...over,
  }
}

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

const scan: MemoScan = {
  home: HOME,
  agents: [
    agent({ agent: 'claude', path: CLAUDE_MD, exists: true, effective: CLAUDE_MD }),
    agent({ agent: 'codex', path: CODEX_MD, exists: true, effective: CODEX_MD }),
    agent({
      agent: 'grok',
      path: GROK_MD,
      exists: false,
      fallback: CLAUDE_MD,
      effective: CLAUDE_MD,
      fallenBack: true,
    }),
    agent({ agent: 'agy', path: AGY_MD, exists: false }),
    agent({
      agent: 'opencode',
      path: OPENCODE_MD,
      exists: false,
      fallback: CLAUDE_MD,
      effective: CLAUDE_MD,
      fallenBack: true,
    }),
    agent({ agent: 'kimicode', path: `${HOME}/.kimi-code/AGENTS.md`, exists: false }),
    agent({
      agent: 'pi',
      path: PI_MD,
      exists: true,
      effective: PI_MD,
      extra: [PI_MEMORY],
    }),
  ],
  files: [
    file({
      path: CLAUDE_MD,
      bytes: 888,
      readers: [
        { agent: 'claude', role: 'own', active: true },
        { agent: 'grok', role: 'fallback', active: true },
        { agent: 'opencode', role: 'fallback', active: true },
      ],
      imports: [
        { raw: '@RTK.md', line: 1, path: CLAUDE_RTK, exists: true, bytes: 964, nested: 0 },
      ],
    }),
    file({
      path: CODEX_MD,
      bytes: 29,
      readers: [{ agent: 'codex', role: 'own', active: true }],
      imports: [
        { raw: `@${CODEX_RTK}`, line: 1, path: CODEX_RTK, exists: true, bytes: 482, nested: 0 },
      ],
    }),
    file({ path: CLAUDE_RTK, bytes: 964, importedBy: CLAUDE_MD }),
    file({ path: CODEX_RTK, bytes: 482, importedBy: CODEX_MD }),
    file({
      path: GROK_MD,
      exists: false,
      bytes: 0,
      revision: rev(0),
      readers: [{ agent: 'grok', role: 'own', active: false }],
    }),
    file({
      path: OPENCODE_MD,
      exists: false,
      bytes: 0,
      revision: rev(0),
      readers: [{ agent: 'opencode', role: 'own', active: false }],
    }),
    file({
      path: PI_MD,
      bytes: 300,
      readers: [{ agent: 'pi', role: 'own', active: true }],
    }),
    // 约定路径之外那一份：没人 import 它，只有 `role: 'extra'` 这一个身份。
    file({
      path: PI_MEMORY,
      bytes: 2705,
      readers: [{ agent: 'pi', role: 'extra', active: true }],
    }),
  ],
  forks: [
    {
      name: 'RTK.md',
      sides: [
        { path: CLAUDE_RTK, bytes: 964 },
        { path: CODEX_RTK, bytes: 482 },
      ],
    },
  ],
  dups: [],
  summary: { files: 8, present: 6, broken: 0, forks: 1, dups: 0 },
}

beforeEach(() => resetToolsPanel())

describe('左栏', () => {
  it('先七家 agent，再是片段和额外来源', () => {
    const rows = memoRows(scan, '')
    expect(rows.filter((r) => r.kind === 'agent')).toHaveLength(7)
    expect(rows.filter((r) => r.kind === 'fragment').map((r) => r.path)).toEqual([
      CLAUDE_RTK,
      CODEX_RTK,
    ])
  })

  // 不单列一行的话，pi 的 MEMORY.md 在这个面板上根本不存在 —— 而 pi 确实在读它。
  it('约定路径之外、agent 还会读的那些也各占一行', () => {
    const extras = memoRows(scan, '').filter((r) => r.kind === 'extra')
    expect(extras.map((r) => r.path)).toEqual([PI_MEMORY])
    expect(extras[0].state).toBe('extra')
    expect(extras[0].name).toBe('MEMORY.md')
    expect(editable(extras[0])).toBe(true)
  })

  it('额外来源的文件不在了就是断链，不是「额外读到」', () => {
    const gone: MemoScan = {
      ...scan,
      files: scan.files.map((f) =>
        f.path === PI_MEMORY ? { ...f, exists: false, bytes: 0, revision: rev(0) } : f,
      ),
    }
    expect(allMemoRows(gone).find((r) => r.path === PI_MEMORY)!.state).toBe('broken')
  })

  it('三种 agent 状态各自分清楚', () => {
    const byAgent = Object.fromEntries(memoRows(scan, '').map((r) => [r.agent, r.state]))
    expect(byAgent.claude).toBe('ok')
    // 自己没有、实际读别人的 —— 和「什么都没有」不是一回事
    expect(byAgent.grok).toBe('fallback')
    // 自己没有、也没有回退，能新建
    expect(byAgent.kimicode).toBe('missing')
  })

  // 现在七家都有 home 级约定了（pi 是 `~/.pi/agent/AGENTS.md`，agy 是
  // `~/.gemini/config/GEMINI.md`），但这一档由后端的 `memo_path()` 决定 —— 哪天多一家
  // 没有这套机制的，行还得画得出来。
  it('不支持的那家点不开 —— 给个输入框让用户白写比什么都不给更糟', () => {
    const none: MemoScan = { ...scan, agents: [agent({ agent: 'agy', supported: false })] }
    const row = memoRows(none, '').find((r) => r.agent === 'agy')!
    expect(row.state).toBe('unsupported')
    expect(row.path).toBeNull()
    expect(editable(row)).toBe(false)
  })

  it('回退中的那一行点开的是**实际生效**的那份，不是自己那个空位置', () => {
    const grok = memoRows(scan, '').find((r) => r.agent === 'grok')!
    // grok 明明有一整套全局指令在生效，点开却是个空编辑器 —— 那是在说谎
    expect(grok.path).toBe(CLAUDE_MD)
    expect(grok.name).toBe('CLAUDE.md')
    expect(grok.bytes).toBe(888)
    // 自己那个位置仍然记着，「让这家接管」要写的就是它
    expect(grok.own).toBe(GROK_MD)
    expect(takeoverPath(grok)).toBe(GROK_MD)
  })

  it('两家回退到同一个文件不会挤成一行', () => {
    const rows = memoRows(scan, '').filter((r) => r.path === CLAUDE_MD)
    expect(rows.map((r) => r.agent)).toEqual(['claude', 'grok', 'opencode'])
    expect(new Set(rows.map((r) => r.key)).size).toBe(3)
  })

  it('没在回退就没有「接管」这回事', () => {
    const rows = memoRows(scan, '')
    expect(takeoverPath(rows.find((r) => r.agent === 'claude')!)).toBeNull()
    expect(takeoverPath(rows.find((r) => r.agent === 'kimicode')!)).toBeNull()
  })

  it('文件还不存在的那几家仍然能点开 —— 保存即新建', () => {
    const kimi = memoRows(scan, '').find((r) => r.agent === 'kimicode')!
    expect(editable(kimi)).toBe(true)
  })

  it('agent 过滤器只作用在上半截，片段不属于任何一家', () => {
    toggleToolsAgent('codex')
    const rows = memoRows(scan, '')
    expect(rows.filter((r) => r.kind === 'agent').map((r) => r.agent)).toEqual(['codex'])
    // 两个 RTK.md 都还在：`~/.claude/RTK.md` 是被 CLAUDE.md 引用的，而 CLAUDE.md 有三家在读
    expect(rows.filter((r) => r.kind === 'fragment')).toHaveLength(2)
    // 额外来源同理：它挂在 pi 上，但点掉 pi 不该让这个文件从面板上消失
    expect(rows.filter((r) => r.kind === 'extra')).toHaveLength(1)
  })

  it('搜文件名、搜路径、搜 agent 都算', () => {
    expect(memoRows(scan, 'rtk')).toHaveLength(2)
    expect(memoRows(scan, '.codex').map((r) => r.path)).toEqual([CODEX_MD, CODEX_RTK])
    expect(memoRows(scan, 'grok').map((r) => r.agent)).toEqual(['grok'])
  })

  it('搜得到自己那个还不存在的位置 —— 回退中那行显示的是别人的路径', () => {
    expect(memoRows(scan, '.grok').map((r) => r.agent)).toEqual(['grok'])
  })

  it('没扫到就是空列表，不是崩', () => {
    expect(memoRows(null, '')).toEqual([])
  })
})

/**
 * 健康条左端那个数字的分母。别的三个面板直接拿后端 summary 当分母，这个面板不行 ——
 * 它的行是「位置」不是「文件」。
 */
describe('全量行', () => {
  it('agent 勾选也不管 —— 健康条的分母不该跟着过滤器缩水', () => {
    toggleToolsAgent('codex')
    expect(memoRows(scan, '').filter((r) => r.kind === 'agent')).toHaveLength(1)
    expect(allMemoRows(scan).filter((r) => r.kind === 'agent')).toHaveLength(7)
  })

  it('搜索词也不管', () => {
    expect(memoRows(scan, 'rtk')).toHaveLength(2)
    expect(allMemoRows(scan)).toHaveLength(10)
  })

  it('比后端数到的文件多 —— 这正是不能拿 summary.files 当分母的原因', () => {
    // 七行 agent 里有四家自己那份根本不存在（grok / opencode 在回退，kimicode 和 agy
    // 空着）—— 扫描数不到这些位置，列表却实实在在摆着这些行。
    expect(allMemoRows(scan)).toHaveLength(10)
    expect(scan.summary.files).toBeLessThan(allMemoRows(scan).length)
  })

  it('没扫到就是空列表，不是崩', () => {
    expect(allMemoRows(null)).toEqual([])
  })
})

describe('改这个文件会影响谁', () => {
  it('CLAUDE.md 还被 grok 和 opencode 读着 —— 这是本功能最容易踩的坑', () => {
    const others = otherReaders(scan, CLAUDE_MD, 'claude')
    expect(others.map((r) => r.agent)).toEqual(['grok', 'opencode'])
    expect(others.every((r) => r.role === 'fallback')).toBe(true)
  })

  it('自己不算在「还有谁」里', () => {
    expect(otherReaders(scan, CLAUDE_MD, 'claude').map((r) => r.agent)).not.toContain('claude')
  })

  it('失效的链路不提示 —— grok 自己建了之后再提示就是误报', () => {
    const withOwn: MemoScan = {
      ...scan,
      files: scan.files.map((f) =>
        f.path === CLAUDE_MD
          ? {
              ...f,
              readers: f.readers.map((r) =>
                r.agent === 'grok' ? { ...r, active: false } : r,
              ),
            }
          : f,
      ),
    }
    expect(otherReaders(withOwn, CLAUDE_MD, 'claude').map((r) => r.agent)).toEqual(['opencode'])
  })

  it('没选中文件时是空的，不是崩', () => {
    expect(otherReaders(scan, null, 'claude')).toEqual([])
    expect(otherReaders(null, CLAUDE_MD, 'claude')).toEqual([])
  })
})

describe('分叉', () => {
  it('两个 RTK.md 内容不一样，两边都要能查到这处分叉', () => {
    expect(forkOf(scan, CLAUDE_RTK)?.name).toBe('RTK.md')
    expect(forkOf(scan, CODEX_RTK)?.name).toBe('RTK.md')
  })

  it('没分叉的文件查不到', () => {
    expect(forkOf(scan, CLAUDE_MD)).toBeNull()
    expect(forkOf(scan, null)).toBeNull()
  })

  it('另一边是谁 —— 并排 diff 拿它当右侧', () => {
    const fork = forkOf(scan, CLAUDE_RTK)
    expect(forkOthers(fork, CLAUDE_RTK).map((s) => s.path)).toEqual([CODEX_RTK])
    expect(forkOthers(null, CLAUDE_RTK)).toEqual([])
  })
})

describe('import', () => {
  it('相对和绝对两种写法都解析到了绝对路径', () => {
    expect(importsOf(scan, CLAUDE_MD)[0]).toMatchObject({ raw: '@RTK.md', path: CLAUDE_RTK })
    expect(importsOf(scan, CODEX_MD)[0]).toMatchObject({ path: CODEX_RTK })
  })

  it('没有 import 的文件给空数组', () => {
    expect(importsOf(scan, CLAUDE_RTK)).toEqual([])
    expect(importsOf(scan, null)).toEqual([])
  })
})

describe('指纹', () => {
  it('空白指纹是「还没读过」', () => {
    expect(blankRevision()).toEqual({ exists: false, size: 0, mtimeMs: null })
  })

  it('三个字段全等才算同一份 —— 少比一个就是拿旧内容盖掉别人的修改', () => {
    const a = { exists: true, size: 10, mtimeMs: 1 }
    expect(sameRevision(a, { ...a })).toBe(true)
    expect(sameRevision(a, { ...a, size: 11 })).toBe(false)
    expect(sameRevision(a, { ...a, mtimeMs: 2 })).toBe(false)
    expect(sameRevision(a, { ...a, exists: false })).toBe(false)
  })

  it('重扫之后指纹变了就是外面改过了', () => {
    const loaded = scan.files[0].revision
    expect(externallyChanged(scan, CLAUDE_MD, loaded)).toBe(false)
    const moved: MemoScan = {
      ...scan,
      files: scan.files.map((f) =>
        f.path === CLAUDE_MD ? { ...f, revision: { ...f.revision, size: 999 } } : f,
      ),
    }
    expect(externallyChanged(moved, CLAUDE_MD, loaded)).toBe(true)
  })

  it('扫不到这个路径就不报冲突 —— 没依据的警告比没有警告糟', () => {
    const loaded = { exists: true, size: 1, mtimeMs: 1 }
    expect(externallyChanged(scan, '/nowhere/AGENTS.md', loaded)).toBe(false)
    expect(externallyChanged(scan, CLAUDE_MD, null)).toBe(false)
    expect(externallyChanged(null, CLAUDE_MD, loaded)).toBe(false)
  })

  it('刚新建的那种：打开时不存在，现在存在了', () => {
    const loaded = blankRevision()
    expect(externallyChanged(scan, CLAUDE_MD, loaded)).toBe(true)
  })
})

describe('杂项', () => {
  it('模板只给一个标题 —— 再多就是替用户决定他的全局指令该写什么', () => {
    expect(template('/x/AGENTS.md')).toBe('# AGENTS.md\n\n')
  })

  it('字节数写成人能读的', () => {
    expect(formatBytes(888)).toBe('888 B')
    expect(formatBytes(964)).toBe('964 B')
    expect(formatBytes(2048)).toBe('2.0 KB')
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB')
  })

  it('basename 认两种分隔符', () => {
    expect(baseName('/a/b/c.md')).toBe('c.md')
    expect(baseName('C:\\a\\b.md')).toBe('b.md')
  })

  it('一家现在实际读的是哪份', () => {
    expect(effectiveOf(scan, 'grok')).toMatchObject({ effective: CLAUDE_MD, fallenBack: true })
    expect(effectiveOf(scan, 'claude')).toMatchObject({ effective: CLAUDE_MD, fallenBack: false })
    expect(effectiveOf(scan, null)).toBeNull()
  })
})

describe('分叉能不能覆盖', () => {
  const base = { left: '/a/RTK.md', right: '/b/RTK.md', hunks: [], clipped: false }

  it('两边一样就不给覆盖按钮 —— 点了什么都不会发生，只白留一份 .bak', () => {
    expect(canSyncFork({ ...base, same: true, truncated: false })).toBe(false)
  })

  it('有差异就给', () => {
    expect(canSyncFork({ ...base, same: false, truncated: false })).toBe(true)
  })

  it('太大没逐行比也给 —— 没比过不等于一样', () => {
    expect(canSyncFork({ ...base, same: false, truncated: true })).toBe(true)
  })

  it('没 diff 就没按钮', () => {
    expect(canSyncFork(null)).toBe(false)
  })
})
