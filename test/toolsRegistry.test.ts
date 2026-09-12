// 工具管理 · 发现面板的纯逻辑。
//
// 这个面板和另外四个最大的不同是**输入会发网络请求**，所以下面绝大多数测试钉的都是
// 「什么时候不该发」和「回来的东西什么时候不该信」—— 那两件事在界面上的失败形态是
// 静默的：搜出来的东西看着挺正常，只是和搜索框里写的词对不上。

import { beforeEach, describe, expect, it } from 'vitest'
import type {
  PreviewError,
  RegistryError,
  RegistryHit,
  RegistryPreview,
  RegistrySearch,
} from '../src/types'
import { setLang } from '../src/settings'
import { t } from '../src/i18n'
import {
  HIT_SORTS,
  MIN_QUERY_CHARS,
  SEARCH_LIMIT,
  asPreviewError,
  asRegistryError,
  discoverState,
  formatInstalls,
  hitKey,
  hitSubtitle,
  installCommand,
  isFresh,
  isPreviewFor,
  otherPathsNote,
  previewDescription,
  previewErrorText,
  previewState,
  queryTooShort,
  registryErrorText,
  repoUrl,
  skillsShUrl,
  sortHits,
} from '../src/toolsRegistry'

function hit(over: Partial<RegistryHit> = {}): RegistryHit {
  return {
    source: 'emilkowalski/skills',
    skillId: 'prototype',
    name: 'prototype',
    installs: 82653,
    installable: true,
    ...over,
  }
}

function search(over: Partial<RegistrySearch> = {}): RegistrySearch {
  return { query: 'type', searchType: 'fuzzy', hits: [hit()], ...over }
}

beforeEach(() => setLang('zh'))

describe('查询词', () => {
  /**
   * 接口的硬门槛是 2 个字符，短了直接 HTTP 400 回一句英文。与其把那句话糊到用户
   * 脸上，不如根本不发。
   */
  it('少于两个字符不发出去', () => {
    for (const q of ['', ' ', 'a', '  x  ', '\t']) {
      expect(queryTooShort(q)).toBe(true)
    }
  })

  /** 按 char 不按字节：两个汉字是六个字节，按字节判会把一个合法查询当成太短。 */
  it('两个汉字算两个字符', () => {
    expect(queryTooShort('中文')).toBe(false)
    expect(queryTooShort('ab')).toBe(false)
    expect(MIN_QUERY_CHARS).toBe(2)
  })

  /** 接口上限 200 且没有 offset / cursor —— 这不是「第一页」，是「全部」。 */
  it('一次要的条数在接口上限之内', () => {
    expect(SEARCH_LIMIT).toBeLessThanOrEqual(200)
  })
})

describe('列表区该显示什么', () => {
  const none = null

  it('没输入时是引导页，不是「没找到」', () => {
    expect(discoverState('', false, none, none)).toBe('idle')
    expect(discoverState('   ', false, none, none)).toBe('idle')
  })

  it('输入太短时说清楚，不发请求也不报错', () => {
    expect(discoverState('a', false, none, none)).toBe('tooShort')
  })

  /**
   * 加载态压在错误和结果之上。不压的话重新搜的时候屏幕上还挂着上一次的失败提示，
   * 看起来像是这一次又失败了。
   */
  it('正在搜的时候既不显示旧错误也不显示旧结果', () => {
    const err: RegistryError = { kind: 'offline', detail: 'x' }
    expect(discoverState('type', true, err, search())).toBe('loading')
  })

  it('出错就是出错，有结果就是有结果', () => {
    const err: RegistryError = { kind: 'http', detail: '503' }
    expect(discoverState('type', false, err, none)).toBe('error')
    expect(discoverState('type', false, none, search())).toBe('ready')
  })

  /** 搜到零条和还没搜是两回事：一个该说「没找到 xxx」，一个该说「搜点什么吧」。 */
  it('零结果和还没开始搜要分开', () => {
    expect(discoverState('zzzz', false, none, search({ hits: [] }))).toBe('empty')
    expect(discoverState('zzzz', false, none, none)).toBe('idle')
  })
})

describe('过期响应', () => {
  /**
   * 网络请求没有先发先到这回事。打 `react` 的过程中会发出 `re` / `rea` / `reac`
   * 几轮，慢的那轮回来得更晚，屏幕上就会停在一个**更短的词**的结果上，而搜索框里
   * 写着完整的词 —— 看上去就是「搜出来的东西不对」，而且不会报任何错。
   */
  it('接口回显的词和框里的对不上就丢掉', () => {
    expect(isFresh(search({ query: 'rea' }), 'react')).toBe(false)
    expect(isFresh(search({ query: 'react' }), 'react')).toBe(true)
  })

  it('两边的首尾空白不算差异', () => {
    expect(isFresh(search({ query: 'react' }), '  react  ')).toBe(true)
  })
})

describe('排序', () => {
  const a = hit({ skillId: 'a', installs: 10 })
  const b = hit({ skillId: 'b', installs: 900 })
  const c = hit({ skillId: 'c', installs: 100 })

  /** 接口给的次序就是相关度本身，我们没有比它更好的信息，所以原样返回。 */
  it('相关度那一档原样不动', () => {
    const list = [a, b, c]
    expect(sortHits(list, 'relevance')).toBe(list)
  })

  it('安装量那一档从多到少', () => {
    expect(sortHits([a, b, c], 'installs').map((h) => h.skillId)).toEqual(['b', 'c', 'a'])
  })

  /** 安装量相同时按名字兜底，否则同一份结果两次渲染的次序可能不一样。 */
  it('安装量打平时次序仍然是确定的', () => {
    const x = hit({ skillId: 'zeta', installs: 5 })
    const y = hit({ skillId: 'alpha', installs: 5 })
    expect(sortHits([x, y], 'installs').map((h) => h.skillId)).toEqual(['alpha', 'zeta'])
    expect(sortHits([y, x], 'installs').map((h) => h.skillId)).toEqual(['alpha', 'zeta'])
  })

  it('重排不改原数组 —— 相关度那一档还要拿回原次序', () => {
    const list = [a, b, c]
    sortHits(list, 'installs')
    expect(list.map((h) => h.skillId)).toEqual(['a', 'b', 'c'])
  })

  it('两档都在', () => {
    expect([...HIT_SORTS]).toEqual(['relevance', 'installs'])
  })
})

describe('身份', () => {
  /**
   * `code-review` 在一页里出现过 8 次（不同 source）。拿名字当 key 会让 Vue 复用
   * 错行 —— 点第三条选中的是第一条，而且不会报任何错。
   */
  it('唯一键带上 source，同名结果才分得开', () => {
    const one = hit({ source: 'a/b', skillId: 'code-review' })
    const two = hit({ source: 'c/d', skillId: 'code-review' })
    expect(hitKey(one)).not.toBe(hitKey(two))
  })
})

describe('展示', () => {
  it('安装量加千分位 —— 六位数不分组根本读不出量级', () => {
    expect(formatInstalls(914678)).toBe('914,678')
    expect(formatInstalls(122)).toBe('122')
  })

  /** 这是个数字不是文案。跟着界面语言走的话同一个数在四种语言下长得不一样。 */
  it('分组方式不跟着界面语言变', () => {
    const zh = formatInstalls(1234567)
    setLang('ja')
    expect(formatInstalls(1234567)).toBe(zh)
  })

  it('两个外链都按 source 拼出来', () => {
    const h = hit()
    expect(skillsShUrl(h)).toBe('https://www.skills.sh/emilkowalski/skills/prototype')
    expect(repoUrl(h)).toBe('https://github.com/emilkowalski/skills')
  })

  /**
   * 域名源（`code.deepline.com` 之类）不是 git 仓库，`/skills.json`、
   * `/<slug>/SKILL.md` 全 404 —— 给它一个 GitHub 链接就是给一个必然 404 的按钮。
   */
  it('域名源没有仓库链接，也没有安装命令', () => {
    const h = hit({ source: 'code.deepline.com', installable: false })
    expect(repoUrl(h)).toBeNull()
    expect(installCommand(h)).toBeNull()
    // 但 skills.sh 那一条还在：那是它唯一的出路。
    expect(skillsShUrl(h)).toBe('https://www.skills.sh/code.deepline.com/prototype')
  })

  /** 这条命令我们自己不执行，只是抄给想在终端装的人 —— 所以必须和页面上那条一致。 */
  it('安装命令用的是 skillId 不是 name', () => {
    const h = hit({ skillId: 'agent-development', name: 'agent development' })
    expect(installCommand(h)).toBe(
      'npx skills add https://github.com/emilkowalski/skills --skill agent-development',
    )
  })
})

describe('错误', () => {
  /**
   * 后端 reject 的是个对象，不是字符串。直接 `String(e)` 会得到 `[object Object]`，
   * 而就算取到 `detail`，那也是一句 `ureq` 的英文，四种语言里只有一种看得懂。
   */
  it('翻成当前语言的一句话，不是 [object Object]', () => {
    const msg = registryErrorText({ kind: 'offline', detail: 'dns error' })
    expect(msg).toBe(t('tools.discover.err.offline'))
    expect(msg).not.toContain('object')
    expect(msg).not.toContain('dns error')
  })

  it('四种原因各有各的话', () => {
    const kinds = ['offline', 'tooShort', 'http', 'badJson'] as const
    const said = kinds.map((k) => registryErrorText({ kind: k, detail: '' }))
    expect(new Set(said).size).toBe(kinds.length)
    for (const s of said) expect(s).not.toContain('tools.discover')
  })

  it('认得的形状原样收下', () => {
    const e: RegistryError = { kind: 'http', detail: '503 oops' }
    expect(asRegistryError(e)).toEqual(e)
  })

  /**
   * 认不出形状时当断网处理。Tauri 层自己抛出来的东西（命令名写错、序列化失败）是
   * 字符串，不走后端那个枚举 —— 那时候至少得有句人话，而不是一片空白。
   */
  it('认不出来的也要有句人话', () => {
    for (const raw of ['boom', null, undefined, { kind: 'nope' }, 42]) {
      const e = asRegistryError(raw)
      expect(e.kind).toBe('offline')
      expect(registryErrorText(e)).toBeTruthy()
    }
    // 原文留在 detail 里，折进「详细信息」，不丢。
    expect(asRegistryError('boom').detail).toBe('boom')
  })
})

// ---------------------------------------------------------------------------
// 详情预览
// ---------------------------------------------------------------------------

function preview(over: Partial<RegistryPreview> = {}): RegistryPreview {
  return {
    source: 'emilkowalski/skills',
    skillId: 'prototype',
    repoPath: 'skills/prototype',
    otherPaths: [],
    commit: 'd23d7f8',
    treeSha: '564cf82ef5b17f5e018b439dc7fd1e392dcf6188',
    frontmatter: {
      name: 'prototype',
      description: 'Build multiple genuinely different versions of a UI piece.',
      allowedTools: [],
      extra: [],
    },
    files: [
      { path: 'SKILL.md', bytes: 7488 },
      { path: 'PICKER.md', bytes: 7548 },
    ],
    findings: [],
    risk: 'none',
    truncated: false,
    cached: false,
    ...over,
  }
}

describe('详情区该显示什么', () => {
  const none = null

  it('没选中就是没选中，不是「正在读」', () => {
    expect(previewState(none, false, none, none)).toBe('none')
  })

  /**
   * 加载态压在错误和结果之上。不压的话点下一条时屏幕上还挂着上一条的文件清单，
   * 而标题已经换成新的了。
   */
  it('正在取的时候既不显示旧错误也不显示旧内容', () => {
    const err: PreviewError = { kind: 'clone', detail: 'x' }
    expect(previewState(hit(), true, err, preview())).toBe('loading')
  })

  it('出错就是出错，有内容就是有内容', () => {
    const err: PreviewError = { kind: 'notFound', detail: 'x' }
    expect(previewState(hit(), false, err, none)).toBe('error')
    expect(previewState(hit(), false, none, preview())).toBe('ready')
  })

  /**
   * 选中了、没在读、没出错、也没内容 —— 这是 watch 还没跑到的那一帧。算「正在读」，
   * 不算「读完了是空的」：闪一下空白详情比多转半帧难看得多。
   */
  it('选中了但还什么都没有，算正在读', () => {
    expect(previewState(hit(), false, none, none)).toBe('loading')
  })

  /**
   * 域名源根本不去取 —— 没有任何公开的取内容路径。要是也算「正在读」，那儿会挂一个
   * **永远转下去的骨架屏**，而下面「为什么装不了」那段话已经把事情说完了。
   */
  it('域名源不进加载态，更不会永远转下去', () => {
    const domain = hit({ source: 'code.deepline.com', installable: false })
    expect(previewState(domain, false, none, none)).toBe('none')
    expect(previewState(domain, true, none, none)).toBe('none')
  })
})

describe('过期的预览', () => {
  /**
   * 首次 3 秒级、命中缓存 200 ms 级，差两个数量级 —— 点第二条时第一条很可能还没回来。
   * 不比对的话屏幕上会是**另一个 skill 的文件清单和风险点**，而标题写着你刚点的那个。
   * 这种错不报任何错，却恰好错在这一屏唯一要回答的问题上。
   */
  it('回来的不是当前选中那条就丢掉', () => {
    const mine = hit({ source: 'a/b', skillId: 'x' })
    expect(isPreviewFor(preview({ source: 'a/b', skillId: 'x' }), mine)).toBe(true)
    expect(isPreviewFor(preview({ source: 'a/b', skillId: 'y' }), mine)).toBe(false)
    // 同名不同仓库是这一页里最常见的情况（`code-review` 一页出现过 8 次）。
    expect(isPreviewFor(preview({ source: 'c/d', skillId: 'x' }), mine)).toBe(false)
  })
})

describe('预览的错误', () => {
  it('五种原因各有各的话', () => {
    const kinds = ['notInstallable', 'cache', 'clone', 'notFound', 'checkout'] as const
    const said = kinds.map((k) => previewErrorText({ kind: k, detail: '' }, 'prototype'))
    expect(new Set(said).size).toBe(kinds.length)
    for (const s of said) expect(s).not.toContain('tools.discover')
  })

  /** 「这个仓库里没有 X」比「找不到」有用得多 —— 下一步是去核对拼写。 */
  it('找不到的那句要把名字念出来', () => {
    expect(previewErrorText({ kind: 'notFound', detail: '' }, 'prototype')).toContain('prototype')
  })

  it('认得的形状原样收下', () => {
    const e: PreviewError = { kind: 'notFound', detail: 'no such thing' }
    expect(asPreviewError(e)).toEqual(e)
  })

  /** Tauri 层自己抛的是字符串，不走后端那个枚举。那时候至少得有句人话。 */
  it('认不出来的当拉取失败处理，原文留在 detail 里', () => {
    for (const raw of ['boom', null, undefined, { kind: 'nope' }, 42]) {
      const e = asPreviewError(raw)
      expect(e.kind).toBe('clone')
      expect(previewErrorText(e, 'x')).toBeTruthy()
    }
    expect(asPreviewError('boom').detail).toBe('boom')
  })
})

describe('描述', () => {
  it('从 frontmatter 里取，两头空白不算内容', () => {
    expect(previewDescription(preview())).toContain('Build multiple')
    expect(previewDescription(preview({ frontmatter: null }))).toBeNull()
    const blank = preview()
    blank.frontmatter = { name: 'x', description: '   ', allowedTools: [], extra: [] }
    expect(previewDescription(blank)).toBeNull()
  })

  /**
   * 接口不给描述，只有走过一次 git 才拿得到。看过之后第二行换成描述，source 挪到
   * 名字右边 —— 翻回来还看得见，省得为了想起「这个是干嘛的」再等一次 clone。
   */
  it('看过详情的那条第二行换成描述，没看过的还是 source', () => {
    const seen = hit({ source: 'a/b', skillId: 'seen' })
    const fresh = hit({ source: 'a/b', skillId: 'fresh' })
    const described = new Map([[hitKey(seen), '做一个原型']])
    expect(hitSubtitle(seen, described)).toEqual({ text: '做一个原型', described: true })
    expect(hitSubtitle(fresh, described)).toEqual({ text: 'a/b', described: false })
  })

  /** 缓存按 `source/skillId` 记。按名字记的话同名的八条会互相串味。 */
  it('描述缓存分得清同名不同仓库', () => {
    const one = hit({ source: 'a/b', skillId: 'code-review' })
    const two = hit({ source: 'c/d', skillId: 'code-review' })
    const described = new Map([[hitKey(one), '甲的说明']])
    expect(hitSubtitle(one, described).described).toBe(true)
    expect(hitSubtitle(two, described).described).toBe(false)
  })
})

describe('同名目录', () => {
  /** 只有一条命中时不该说话 —— 那是绝大多数情况，多一句只会变成噪音。 */
  it('只命中一条时没有那句提示', () => {
    expect(otherPathsNote(preview())).toBeNull()
  })

  it('还有别的同名目录时说清楚装的是哪一个', () => {
    const note = otherPathsNote(preview({ otherPaths: ['a/skills/prototype', 'b/prototype'] }))
    expect(note).toBeTruthy()
    expect(note).toContain('2')
    expect(note).not.toContain('tools.discover')
  })
})
