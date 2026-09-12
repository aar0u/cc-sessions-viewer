import { beforeEach, describe, expect, it } from 'vitest'
import type { MemoDup, MemoMergeReport, MemoScan, MemoStep, MemoStepKind } from '../src/types'
import {
  dupOf,
  dupOthers,
  memoMergePlan,
  memoStore,
  memoStorePath,
  mergeOutcome,
  mergeable,
  mergeableDups,
  setMemoStore,
} from '../src/toolsMemoActions'

function dup(name: string, paths: string[], bodies = paths): MemoDup {
  return { name, bytes: 964, paths, bodies }
}

function scanWith(dups: MemoDup[]): MemoScan {
  return {
    home: '/Users/x',
    agents: [],
    files: [],
    forks: [],
    dups,
    summary: { files: 0, present: 0, broken: 0, forks: 0, dups: dups.length },
  }
}

function step(
  kind: MemoStepKind,
  path: string,
  target: string | null = null,
  done = false,
  group = 'RTK.md',
): MemoStep {
  return { kind, path, target, done, group }
}

function report(steps: MemoStep[], blocked: string[] = [], dryRun = true): MemoMergeReport {
  return { dryRun, steps, blocked }
}

beforeEach(() => setMemoStore(null))

describe('主 store', () => {
  it('没挑过就落在 ~/.agents/memo —— 和 skills 的主 store 同一个父目录', () => {
    expect(memoStorePath('/Users/x')).toBe('/Users/x/.agents/memo')
  })

  it('home 末尾多一个斜杠不会拼出两道斜杠', () => {
    expect(memoStorePath('/Users/x/')).toBe('/Users/x/.agents/memo')
  })

  it('挑过之后用挑的那个', () => {
    setMemoStore('/somewhere/shared')
    expect(memoStorePath('/Users/x')).toBe('/somewhere/shared')
    expect(memoStore.value).toBe('/somewhere/shared')
  })

  it('清掉之后回到默认', () => {
    setMemoStore('/somewhere/shared')
    setMemoStore(null)
    expect(memoStorePath('/Users/x')).toBe('/Users/x/.agents/memo')
  })

  it('存的是空白字符串时当没挑过 —— 否则会往一个叫空的目录里搬', () => {
    setMemoStore('   ')
    expect(memoStorePath('/Users/x')).toBe('/Users/x/.agents/memo')
  })
})

describe('哪些能合', () => {
  const scan = scanWith([
    dup('RTK.md', ['/Users/x/.claude/RTK.md', '/Users/x/.codex/RTK.md', '/Users/x/.grok/RTK.md']),
  ])

  it('按路径找到它所在的那一组', () => {
    expect(dupOf(scan, '/Users/x/.codex/RTK.md')?.name).toBe('RTK.md')
  })

  it('不在任何一组里就是 null', () => {
    expect(dupOf(scan, '/Users/x/.claude/CLAUDE.md')).toBeNull()
  })

  it('没扫描结果时不炸', () => {
    expect(dupOf(null, '/Users/x/.claude/RTK.md')).toBeNull()
    expect(mergeableDups(null)).toEqual([])
  })

  it('「另外几个地方」不含自己', () => {
    const others = dupOthers(dupOf(scan, '/Users/x/.codex/RTK.md'), '/Users/x/.codex/RTK.md')
    expect(others).toEqual(['/Users/x/.claude/RTK.md', '/Users/x/.grok/RTK.md'])
  })

  /**
   * 这条是按 `bodies` 判而不是按 `paths` 判的理由：合并过一轮之后三个位置还在
   * （`paths` 仍是 3），但内容只剩主 store 那一份，再合一次什么都做不了。
   */
  it('位置还有三个、实体只剩一份时不给合', () => {
    const already = dup('RTK.md', ['/a/RTK.md', '/b/RTK.md', '/c/RTK.md'], ['/a/RTK.md'])
    expect(mergeable(already)).toBe(false)
    expect(mergeableDups(scanWith([already]))).toEqual([])
  })

  it('两份实体就够格', () => {
    expect(mergeable(dup('RTK.md', ['/a/RTK.md', '/b/RTK.md']))).toBe(true)
  })

  it('null 不能合', () => {
    expect(mergeable(null)).toBe(false)
  })
})

describe('计划翻译', () => {
  const steps = [
    step('ensureDir', '/Users/x/.agents/memo'),
    step('move', '/Users/x/.claude/RTK.md', '/Users/x/.agents/memo/RTK.md'),
    step('link', '/Users/x/.claude/RTK.md', '/Users/x/.agents/memo/RTK.md'),
    step('drop', '/Users/x/.codex/RTK.md'),
    step('link', '/Users/x/.codex/RTK.md', '/Users/x/.agents/memo/RTK.md'),
  ]

  it('路径缩成 ~ 开头 —— 计划里路径又多又长', () => {
    const plan = memoMergePlan(report(steps), '/Users/x', ['RTK.md'])
    expect(plan.rows[1].where).toBe('~/.claude/RTK.md')
    expect(plan.rows[1].detail).toBe('~/.agents/memo/RTK.md')
  })

  /** 删除那一步画成红的，但**不能**把确认按钮也变红 —— 见下一条。 */
  it('删和拆链归到 remove 那一档上色', () => {
    const plan = memoMergePlan(report(steps), '/Users/x', ['RTK.md'])
    expect(plan.rows[3].kind).toBe('remove')
    expect(memoMergePlan(report([step('unlink', '/a/RTK.md')]), '/Users/x', ['RTK.md']).rows[0].kind)
      .toBe('remove')
  })

  /**
   * 合并不是危险操作：删掉的每一份都和留下的那份逐字节相同，后端在删之前还会
   * 再复核一次。确认按钮变红只会吓退用户去做一件对的事。
   */
  it('确认按钮不变红', () => {
    expect(memoMergePlan(report(steps), '/Users/x', ['RTK.md']).danger).toBe(false)
  })

  it('删除那一步带一句「内容不会丢」，别的步骤不带', () => {
    const plan = memoMergePlan(report(steps), '/Users/x', ['RTK.md'])
    expect(plan.rows[3].note).toBeTruthy()
    expect(plan.rows[2].note).toBeUndefined()
  })

  it('概括按种类计数，零的那几档不出现', () => {
    const plan = memoMergePlan(report(steps), '/Users/x', ['RTK.md'])
    expect(plan.summary).toContain('2')
    expect(plan.summary).not.toContain('拆链')
  })

  /**
   * 「合并全部重复」一次能出二十来步，不分段就是一堵墙。分割线画在哪由弹框决定，
   * 但归属必须由这里带过去 —— 翻译时丢掉 group，弹框再聪明也分不了段。
   */
  it('每一步归属的文件名要带到计划行上', () => {
    const plan = memoMergePlan(
      report([
        step('move', '/Users/x/.claude/RTK.md', '/s/RTK.md', false, 'RTK.md'),
        step('move', '/Users/x/.codex/AGENTS.md', '/s/AGENTS.md', false, 'AGENTS.md'),
      ]),
      '/Users/x',
      ['RTK.md', 'AGENTS.md'],
    )
    expect(plan.rows.map((r) => r.group)).toEqual(['RTK.md', 'AGENTS.md'])
  })

  it('做不了的那几条原样带过去', () => {
    const plan = memoMergePlan(report([], ['CLAUDE.md：主 store 里已经有一份不一样的']), '/Users/x', [
      'CLAUDE.md',
    ])
    expect(plan.blocked).toEqual(['CLAUDE.md：主 store 里已经有一份不一样的'])
  })
})

describe('做完之后那句话', () => {
  it('全做完就是成功', () => {
    const out = mergeOutcome(report([step('move', '/a', '/b', true)], [], false))
    expect(out.error).toBe(false)
    expect(out.msg).toContain('1')
  })

  /**
   * 又成功又有做不了的：一组合上了、另一组被占位文件挡住。只看 blocked 会把
   * 「做了一半」报成彻底失败，只看 done 会把它报成全好了 —— 两种都在骗人。
   */
  it('做了一半要说清楚停在哪儿', () => {
    const out = mergeOutcome(
      report([step('move', '/a', '/b', true), step('drop', '/c')], ['/c 被人改过了'], false),
    )
    expect(out.error).toBe(true)
    expect(out.msg).toContain('/c 被人改过了')
  })

  it('一步都没做时把原因当主语', () => {
    const out = mergeOutcome(report([], ['RTK.md：现在已经不是重复了'], false))
    expect(out.error).toBe(true)
    expect(out.msg).toBe('RTK.md：现在已经不是重复了')
  })

  it('既没做也没原因时也要有话说，不能是空字符串', () => {
    expect(mergeOutcome(report([], [], false)).msg).not.toBe('')
  })
})
