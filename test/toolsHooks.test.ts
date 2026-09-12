// 工具管理 · Hooks 纯逻辑。
//
// 装置数据照本机实测的形状造：本 app 的回合信号横跨 claude/codex/grok/kimi/agy 五家
// （所以它必须能在只勾一家时仍然出现），codex 的 TOML 里有一条被 `enabled = false`
// 关掉的，还有一条只存在于项目级 `.claude/settings.json`（只读，删不动）。

import { describe, it, expect, beforeEach } from 'vitest'
import type { Agent, HookAt, HookEntry, HookScan, HookSource, HookWriteReport } from '../src/types'
import { t } from '../src/i18n'
import { setLang } from '../src/settings'
import { resetToolsPanel, toggleToolsAgent } from '../src/toolsPanel'
import {
  HOOK_STATES,
  addEdits,
  agentEvents,
  agentSupported,
  agentSupports,
  blankFilter,
  byAgent,
  canRemove,
  canTest,
  canWrite,
  commandHead,
  hookKinds,
  hookPlanView,
  hookState,
  isDestructive,
  removeAgentEdits,
  removeEdits,
  sourceErrors,
  sourcePaths,
  stateCounts,
  stepCounts,
  testEvent,
  toggleEvent,
  toggleState,
  visibleHooks,
  writePath,
} from '../src/toolsHooks'

const HOME = '/Users/me'
const CLAUDE_USER = `${HOME}/.claude/settings.json`
const CLAUDE_PROJECT = `${HOME}/work/.claude/settings.json`
const CODEX_USER = `${HOME}/.codex/hooks.json`

function source(path: string, writable: boolean): HookSource {
  return {
    path,
    scope: writable ? 'user' : 'project',
    origin: 'own',
    format: path.endsWith('.toml') ? 'tomlGrouped' : 'groupedJson',
    writable,
    exists: true,
    conditional: false,
  }
}

function at(agent: Agent, path: string, writable: boolean, event: string, over = {}): HookAt {
  return {
    agent,
    source: source(path, writable),
    def: {
      event,
      matcher: null,
      group: null,
      kind: 'command',
      command: 'notify.sh',
      timeout: null,
      enabled: true,
      ...over,
    },
  }
}

function entry(over: Partial<HookEntry> = {}): HookEntry {
  const hooks = over.hooks ?? [at('claude', CLAUDE_USER, true, 'Stop')]
  return {
    fingerprint: 'notify.sh',
    command: 'notify.sh',
    hooks,
    agents: [...new Set(hooks.map((h) => h.agent))],
    events: [...new Set(hooks.map((h) => h.def.event))],
    managed: false,
    enabled: true,
    ...over,
  }
}

const managed = entry({
  fingerprint: 'managed',
  command: '~/.claude/cc-sessions-viewer-turn.sh stop',
  managed: true,
  hooks: [
    at('claude', CLAUDE_USER, true, 'Stop'),
    at('codex', CODEX_USER, true, 'Stop'),
    at('grok', `${HOME}/.grok/config.toml`, true, 'Stop'),
  ],
})

const disabled = entry({
  fingerprint: 'old.sh',
  command: 'old.sh',
  enabled: false,
  hooks: [at('codex', `${HOME}/.codex/config.toml`, false, 'Stop', { enabled: false })],
})

const readOnly = entry({
  fingerprint: 'repo.sh',
  command: 'repo.sh',
  hooks: [at('claude', CLAUDE_PROJECT, false, 'PreToolUse', { matcher: 'Bash' })],
})

const scan: HookScan = {
  home: HOME,
  hooks: [managed, entry(), disabled, readOnly],
  agents: [
    {
      agent: 'claude',
      installed: true,
      supported: true,
      writePath: CLAUDE_USER,
      events: ['Stop', 'PreToolUse', 'SessionStart'],
      sources: [
        { ...source(CLAUDE_PROJECT, false), hooks: 1, error: '不是合法的 JSON：期望 , 或 }' },
        { ...source(CLAUDE_USER, true), hooks: 2, error: null },
      ],
    },
    {
      agent: 'codex',
      installed: true,
      supported: true,
      writePath: CODEX_USER,
      events: ['Stop', 'PreToolUse'],
      sources: [{ ...source(CODEX_USER, true), hooks: 1, error: null }],
    },
    {
      agent: 'opencode',
      installed: true,
      supported: false,
      writePath: null,
      events: [],
      sources: [],
    },
  ],
  events: [
    { name: 'Stop', agents: ['claude', 'codex', 'grok'], configured: 3 },
    { name: 'PreToolUse', agents: ['claude', 'codex'], configured: 1 },
  ],
  summary: { hooks: 4, defs: 6, managed: 3, disabled: 1, events: 2 },
}

beforeEach(() => {
  resetToolsPanel()
  setLang('zh')
})

describe('行状态', () => {
  it('回合信号压过一切 —— 它开着也不该显示成普通的「生效」', () => {
    expect(hookState(managed)).toBe('managed')
  })

  it('全被关掉的是停用，其余是生效', () => {
    expect(hookState(disabled)).toBe('off')
    expect(hookState(entry())).toBe('on')
  })

  it('三个状态各自计数', () => {
    expect(stateCounts(scan.hooks)).toEqual({ on: 2, off: 1, managed: 1 })
    expect(HOOK_STATES).toHaveLength(3)
  })
})

describe('过滤', () => {
  it('默认什么都不筛', () => {
    expect(visibleHooks(scan, '', blankFilter())).toHaveLength(4)
  })

  it('按状态筛', () => {
    const f = toggleState(blankFilter(), 'off')
    expect(visibleHooks(scan, '', f).map((h) => h.command)).toEqual(['old.sh'])
    // 再点一下就取消
    expect(toggleState(f, 'off').state).toBeNull()
  })

  it('按事件筛，再点一下取消', () => {
    const f = toggleEvent(blankFilter(), 'PreToolUse')
    expect(visibleHooks(scan, '', f).map((h) => h.command)).toEqual(['repo.sh'])
    expect(toggleEvent(f, 'PreToolUse').event).toBeNull()
  })

  it('搜命令、搜事件、搜 agent 都算', () => {
    expect(visibleHooks(scan, 'old', blankFilter())).toHaveLength(1)
    expect(visibleHooks(scan, 'pretooluse', blankFilter())).toHaveLength(1)
    expect(visibleHooks(scan, 'codex', blankFilter()).map((h) => h.command)).toContain('old.sh')
  })

  it('只勾一家时，横跨多家的那条仍然在 —— 按「落点全在」筛会让回合信号凭空消失', () => {
    toggleToolsAgent('codex')
    const shown = visibleHooks(scan, '', blankFilter())
    expect(shown).toContain(managed)
    expect(shown).toContain(disabled)
    // 只挂在 claude 上的那两条这时该收起来
    expect(shown).not.toContain(readOnly)
  })

  it('没扫到就是空列表，不是崩', () => {
    expect(visibleHooks(null, '', blankFilter())).toEqual([])
  })
})

describe('列表标题', () => {
  // 全是本机实测的真命令 —— 之前那版取第一个词，列表里并排出现了六行 `[`。
  it('脚本名优先于解释器：有意思的是 .js，不是 node', () => {
    expect(commandHead('node "/Users/me/Library/x/turn-signal-hook.cjs" "agy" "completed"')).toBe(
      'turn-signal-hook.cjs',
    )
    expect(commandHead('bash "/Users/me/.skills-manager/hooks/claude-skill-usage.sh"')).toBe(
      'claude-skill-usage.sh',
    )
  })

  it('裸路径取 basename', () => {
    expect(commandHead('/Users/me/.claude/hooks/rtk-rewrite.sh')).toBe('rtk-rewrite.sh')
  })

  it('前面裹着 shell 测试的照样能挑出脚本', () => {
    const cmd =
      '[ -n "$CCVIEWER_PORT" ] && node "/Users/me/lib/cc-viewer/server/lib/ask-bridge.js" || true # managed'
    expect(commandHead(cmd)).toBe('ask-bridge.js')
  })

  it('没有脚本时取程序名，并带上子命令 —— 不带的话同一个程序的五条会并排五行一样', () => {
    const base =
      '[ -n "$CMUX_SURFACE_ID" ] && [ "$CMUX_CODEX_HOOKS_DISABLED" != "1" ] && command -v cmux >/dev/null 2>&1 && cmux hooks codex '
    expect(commandHead(base + "stop || echo '{}'")).toBe('cmux hooks codex stop')
    expect(commandHead(base + "session-start || echo '{}'")).toBe('cmux hooks codex session-start')
  })

  it('标志被当噪声丢掉，子命令带到第一个不像子命令的词为止', () => {
    expect(commandHead('git --no-pager log')).toBe('git log')
    expect(commandHead('cmux hooks feed --source codex')).toBe('cmux hooks feed codex')
  })

  it('`command -v X && X …` 里连着出现两次的 X 只算一次', () => {
    expect(commandHead('command -v cmux >/dev/null 2>&1 && cmux hooks codex stop')).toBe(
      'cmux hooks codex stop',
    )
  })

  it('以变量开头的路径不算噪声 —— 整条丢掉的话标题只剩一个 bash', () => {
    expect(commandHead('bash "$CLAUDE_PROJECT_DIR/scripts/hooks/check-ids.sh"')).toBe(
      'check-ids.sh',
    )
    // 光秃秃一个变量仍然是噪声
    expect(commandHead('[ -n "$PORT" ] && notify.sh')).toBe('notify.sh')
  })

  it('挑不出来就原样，不编一个名字', () => {
    expect(commandHead('   ')).toBe('')
    expect(commandHead('&& ||')).toBe('&& ||')
  })
})

describe('落点', () => {
  it('按 agent 分组，顺序跟着 hooks 走', () => {
    expect(byAgent(managed).map((r) => r.agent)).toEqual(['claude', 'codex', 'grok'])
  })

  it('一家上挂了哪些事件，去重', () => {
    const e = entry({
      hooks: [
        at('claude', CLAUDE_USER, true, 'Stop'),
        at('claude', CLAUDE_USER, true, 'Stop'),
        at('claude', CLAUDE_USER, true, 'SessionStart'),
      ],
    })
    expect(agentEvents(e, 'claude')).toEqual(['Stop', 'SessionStart'])
  })

  it('可写路径和支持情况都从扫描结果里取，不猜', () => {
    expect(writePath(scan, 'claude')).toBe(CLAUDE_USER)
    expect(writePath(scan, 'opencode')).toBeNull()
    expect(agentSupports(scan, 'claude', 'SessionStart')).toBe(true)
    expect(agentSupports(scan, 'codex', 'SessionStart')).toBe(false)
    expect(canWrite(scan, 'claude')).toBe(true)
    expect(canWrite(scan, 'opencode')).toBe(false)
    // 「没这个机制」和「有但写不进去」是两件事，不能合成一个布尔
    expect(agentSupported(scan, 'claude')).toBe(true)
    expect(agentSupported(scan, 'opencode')).toBe(false)
  })
})

describe('能不能试跑', () => {
  it('command 型的能跑', () => {
    expect(canTest(entry())).toBe(true)
    expect(hookKinds(entry())).toEqual([])
  })

  it('prompt / url 型的不能 —— 那串字过一遍 shell 既跑不对，还会把 | 当成真的管道', () => {
    const e = entry({ hooks: [at('claude', CLAUDE_USER, true, 'Stop', { kind: 'prompt' })] })
    expect(canTest(e)).toBe(false)
    expect(hookKinds(e)).toEqual(['prompt'])
  })

  it('混着的也不给跑', () => {
    const e = entry({
      hooks: [
        at('claude', CLAUDE_USER, true, 'Stop'),
        at('codex', CODEX_USER, true, 'Stop', { kind: 'url' }),
      ],
    })
    expect(canTest(e)).toBe(false)
    expect(hookKinds(e)).toEqual(['url'])
  })

  it('一个落点都没有时不给跑，不是「默认能」', () => {
    expect(canTest(entry({ hooks: [] }))).toBe(false)
  })
})

describe('删除', () => {
  it('只动可写文件里的落点 —— 只读来源那条注定失败，不该混进计划里', () => {
    expect(removeEdits(readOnly)).toEqual([])
    expect(removeEdits(entry())).toHaveLength(1)
  })

  it('一条都下不出来时按钮要提前禁掉 —— 空数组点下去是彻底的没反应', () => {
    expect(canRemove(readOnly)).toBe(false)
    expect(canRemove(managed)).toBe(false)
    expect(canRemove(entry())).toBe(true)
  })

  it('删不动时要把它待在哪几个文件里说出来', () => {
    expect(sourcePaths(readOnly)).toEqual([CLAUDE_PROJECT])
    expect(sourcePaths(managed)).toHaveLength(3)
  })

  it('回合信号一条改动都不下 —— 后端还会再拦一次，两道都要在', () => {
    expect(removeEdits(managed)).toEqual([])
  })

  it('按 agent 删只下那一家的', () => {
    const e = entry({
      hooks: [at('claude', CLAUDE_USER, true, 'Stop'), at('codex', CODEX_USER, true, 'Stop')],
    })
    expect(removeAgentEdits(e, 'codex').map((x) => x.agent)).toEqual(['codex'])
  })

  it('删的时候带上事件和匹配器 —— 后端靠三者对齐才找得到要摘哪一条', () => {
    const e = entry({ hooks: [at('claude', CLAUDE_USER, true, 'PreToolUse', { matcher: 'Bash' })] })
    expect(removeEdits(e)[0]).toMatchObject({ event: 'PreToolUse', matcher: 'Bash', op: 'remove' })
  })
})

describe('添加', () => {
  it('同一条命令写给每一家的每一个事件', () => {
    const edits = addEdits('notify.sh', ['Stop', 'SessionStart'], null, 5, ['claude', 'codex'])
    expect(edits).toHaveLength(4)
    expect(edits.every((e) => e.op === 'add' && e.command === 'notify.sh')).toBe(true)
    expect(edits.filter((e) => e.agent === 'claude').map((e) => e.event)).toEqual([
      'Stop',
      'SessionStart',
    ])
  })

  it('超时原样传下去，包括「没填」', () => {
    expect(addEdits('x', ['Stop'], null, null, ['claude'])[0].timeout).toBeNull()
    expect(addEdits('x', ['Stop'], null, 120000, ['claude'])[0].timeout).toBe(120000)
  })
})

describe('试跑用哪个事件', () => {
  it('优先这一行自己挂着的第一个', () => {
    expect(testEvent(readOnly, 'Stop')).toBe('PreToolUse')
  })

  it('一个都没有才用兜底的', () => {
    expect(testEvent(entry({ hooks: [], events: [] }), 'Stop')).toBe('Stop')
  })
})

describe('读不了的文件', () => {
  it('逐条带出来 —— 静悄悄少几条是这个面板最坏的失败方式', () => {
    const errs = sourceErrors(scan)
    expect(errs).toHaveLength(1)
    expect(errs[0]).toMatchObject({ agent: 'claude', path: CLAUDE_PROJECT })
  })
})

describe('计划翻成确认框', () => {
  const report = (over: Partial<HookWriteReport> = {}): HookWriteReport => ({
    dryRun: true,
    steps: [
      {
        agent: 'claude',
        kind: 'add',
        path: CLAUDE_USER,
        event: 'Stop',
        matcher: null,
        command: 'notify.sh',
        newFile: false,
        done: false,
      },
    ],
    blocked: [],
    ...over,
  })
  const view = (r: HookWriteReport) => hookPlanView('添加', r, HOME)

  it('每一步说清「哪家的哪个文件」「哪个事件」', () => {
    const [row] = view(report()).rows
    expect(row.where).toContain('~/.claude/settings.json')
    expect(row.detail).toBe('Stop')
  })

  it('有匹配器就跟在事件后面 —— 同一事件两个匹配器是两条不同的 hook', () => {
    const r = report({ steps: [{ ...report().steps[0], matcher: 'Bash' }] })
    expect(view(r).rows[0].detail).toBe('Stop · Bash')
  })

  it('移除算破坏性，脚注换成「会丢配置」那句', () => {
    const r = report({ steps: [{ ...report().steps[0], kind: 'remove' }] })
    expect(view(r).danger).toBe(true)
    expect(view(r).footnote).toBe(t('tools.hooks.plan.removeNote'))
    expect(isDestructive(r)).toBe(true)
  })

  it('新建文件要提前说', () => {
    const r = report({ steps: [{ ...report().steps[0], newFile: true }] })
    expect(view(r).rows[0].note).toBe(t('tools.hooks.plan.newFile'))
  })

  it('受保护那条写成一句完整的话，不是一个枚举名', () => {
    const r = report({
      steps: [],
      blocked: [{ agent: 'claude', event: 'Stop', reason: 'protected', path: null }],
    })
    const v = view(r)
    expect(v.blocked[0]).toContain('Stop')
    expect(v.blocked[0]).not.toContain('protected')
    expect(v.blocked[0]).toContain(t('tools.hooks.block.protected'))
  })

  it('概括和按钮都带着步数', () => {
    const two = report({ steps: [report().steps[0], { ...report().steps[0], kind: 'remove' }] })
    expect(stepCounts(two)).toEqual({ add: 1, remove: 1 })
    expect(view(two).applyLabel).toContain('2')
    expect(view(two).summary).toContain('1')
  })
})
