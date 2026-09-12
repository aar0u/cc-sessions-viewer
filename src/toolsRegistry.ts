// 工具管理 · 发现面板的纯逻辑。
//
// 和 toolsMcp.ts / toolsSkills.ts 同一个理由住在这儿：面板本体在 `src/views/`，
// 而那个目录在 `vitest.config.ts` 是覆盖率排除的。「过期的响应要不要丢」
// 「列表区现在该显示哪一种状态」这类判断写进 .vue 就再也没人测得到。

import type {
  PreviewErrKind,
  PreviewError,
  RegistryError,
  RegistryErrKind,
  RegistryHit,
  RegistryPreview,
  RegistrySearch,
} from './types'
import { t } from './i18n'

/**
 * 一次请求要多少条。
 *
 * 接口上限 200 且**没有 offset / cursor**，翻不了页 —— 所以这不是「第一页」，
 * 是「全部」。定 100 不定 200：多出来的那 100 条是相关度最低的一截，用户不会翻到，
 * 而响应体要大一倍。
 */
export const SEARCH_LIMIT = 100

/** 接口的硬门槛。比这短的一个字符都不发出去。 */
export const MIN_QUERY_CHARS = 2

/**
 * 顶栏防抖之上再叠的那一层。
 *
 * 另外四个面板输入即过滤内存数组，200 ms 就够了；这儿每次输入是一次 HTTP，
 * 打一个词的工夫能打出七八次请求。按 Enter 可以立即发，不用等这 500 ms。
 */
export const SEARCH_DEBOUNCE_MS = 500

// ---------------------------------------------------------------------------
// 查询词
// ---------------------------------------------------------------------------

/** 够不够长发出去。按 **char** 数不是字节数 —— 两个汉字是六个字节。 */
export function queryTooShort(query: string): boolean {
  return [...query.trim()].length < MIN_QUERY_CHARS
}

/**
 * 列表区现在该显示哪一种状态。
 *
 * 抽成一个函数而不是在模板里堆 `v-if`：四个状态之间的**优先级**才是要被钉住的东西
 * （正在加载时不该闪一下「没找到」，出错时不该继续显示上一次的结果）。
 */
export type DiscoverState = 'idle' | 'tooShort' | 'loading' | 'error' | 'empty' | 'ready'

export function discoverState(
  query: string,
  loading: boolean,
  error: RegistryError | null,
  result: RegistrySearch | null,
): DiscoverState {
  const q = query.trim()
  if (q === '') return 'idle'
  if (queryTooShort(q)) return 'tooShort'
  // 加载态压在错误和结果之上：重新搜的时候屏幕上不该还挂着上一次的失败提示。
  if (loading) return 'loading'
  if (error) return 'error'
  if (!result) return 'idle'
  return result.hits.length === 0 ? 'empty' : 'ready'
}

// ---------------------------------------------------------------------------
// 过期响应
// ---------------------------------------------------------------------------

/**
 * 这份响应还作不作数。
 *
 * 网络请求没有先发先到这回事：打 `react` 的过程中会发出 `re` / `rea` / `reac` 几轮，
 * 慢的那一轮回来得比快的晚，屏幕上就会停在一个**更短的词**的结果上，而搜索框里写着
 * 完整的词 —— 看上去就是「搜出来的东西不对」。
 *
 * 比的是接口回显的 `query`，不是我们发出去的那个：两者不一致说明中间被改写过，
 * 那种响应本来就不该信。
 */
export function isFresh(result: RegistrySearch, currentQuery: string): boolean {
  return result.query.trim() === currentQuery.trim()
}

// ---------------------------------------------------------------------------
// 排序
// ---------------------------------------------------------------------------

/** `relevance` 用接口给的次序；`installs` 是纯本地重排，不重新发请求。 */
export type HitSort = 'relevance' | 'installs'

export const HIT_SORTS: readonly HitSort[] = ['relevance', 'installs'] as const

/**
 * 排好序的结果。
 *
 * 相关度那一档**原样返回**，不做任何稳定化处理 —— 接口给的次序就是相关度本身，
 * 我们没有比它更好的信息。
 */
export function sortHits(hits: RegistryHit[], sort: HitSort): RegistryHit[] {
  if (sort === 'relevance') return hits
  // 安装量相同时按名字兜底，否则同一次搜索两次渲染的次序可能不一样。
  return [...hits].sort((a, b) => b.installs - a.installs || a.skillId.localeCompare(b.skillId))
}

// ---------------------------------------------------------------------------
// 身份
// ---------------------------------------------------------------------------

/**
 * 列表行的唯一键。
 *
 * **不能用 name。** 一页里 `code-review` 出现过 8 次（不同 source），拿名字当 key
 * 会让 Vue 复用错行，点第三条选中的是第一条。
 */
export function hitKey(hit: RegistryHit): string {
  return `${hit.source}/${hit.skillId}`
}

// ---------------------------------------------------------------------------
// 展示
// ---------------------------------------------------------------------------

/**
 * 安装量。跨度是 122 ~ 914678，不加千分位的话六位数根本读不出量级。
 *
 * 固定用 `en-US` 的分组而不是跟着界面语言走：这是个数字不是文案，而各语言的分组
 * 习惯不同（有的四位一组），同一个数在四种语言下长得不一样只会让人以为读错了。
 */
export function formatInstalls(n: number): string {
  return n.toLocaleString('en-US')
}

/** 详情页和「在浏览器打开」用的 skills.sh 地址。 */
export function skillsShUrl(hit: RegistryHit): string {
  return `https://www.skills.sh/${hit.source}/${hit.skillId}`
}

/** GitHub 仓库地址。域名源没有，返回 null。 */
export function repoUrl(hit: RegistryHit): string | null {
  return hit.installable ? `https://github.com/${hit.source}` : null
}

/**
 * skills.sh 页面上那条安装命令。
 *
 * 它要求本机有 node、会下载并执行任意 npm 包、没法 dry-run —— 装完才知道装了什么。
 * 所以这一行不是后台悄悄跑掉的：点「复制并安装」会在详情区拉起一个**看得见的**终端
 * （`InstallTerminal.vue`），命令原样打进去回车，每一行输出都在用户眼前，要停随时
 * Ctrl-C。装之前该看清楚的（描述 / 文件清单 / 风险点）在同一屏已经摊开过了。
 */
export function installCommand(hit: RegistryHit): string | null {
  const repo = repoUrl(hit)
  return repo ? `npx skills add ${repo} --skill ${hit.skillId}` : null
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

const ERR_KINDS: readonly RegistryErrKind[] = ['offline', 'tooShort', 'http', 'badJson'] as const

/** 后端 reject 的是个对象，不是字符串。认不出形状时当断网处理 —— 那是最常见的一种。 */
export function asRegistryError(e: unknown): RegistryError {
  const kind = (e as RegistryError | null)?.kind
  if (kind && ERR_KINDS.includes(kind)) return e as RegistryError
  return { kind: 'offline', detail: String(e) }
}

/**
 * 给用户看的那句话。
 *
 * **不是 `String(e)`。** 后端 reject 的是对象，直接插值会得到 `[object Object]`；
 * 而就算取到 `detail`，那也是一句 `ureq` 的英文错误，四种语言的界面里只有一种看得懂。
 */
export function registryErrorText(e: RegistryError): string {
  return t(`tools.discover.err.${e.kind}`)
}

// ---------------------------------------------------------------------------
// 详情预览
// ---------------------------------------------------------------------------

/**
 * 详情区现在该显示哪一种状态。
 *
 * 和列表那边同样的理由抽成函数：要被钉住的是**优先级**。正在取的时候不该还挂着
 * 上一条的内容（那是另一个 skill 的文件清单和风险点，而这一屏的全部意义就是
 * 「装之前看清楚装的是什么」）。
 */
export type PreviewState = 'none' | 'loading' | 'error' | 'ready'

export function previewState(
  selected: RegistryHit | null,
  loading: boolean,
  error: PreviewError | null,
  preview: RegistryPreview | null,
): PreviewState {
  if (!selected) return 'none'
  // 域名源根本不去取（没有任何公开的取内容路径，方案文档 1.4），详情里那段
  // 「为什么装不了」已经把话说完了。再摆一个红框写同一件事只是重复。
  if (!selected.installable) return 'none'
  if (loading) return 'loading'
  if (error) return 'error'
  // 选中了、没在读、没出错、也没内容 —— 这是 watch 还没跑到的那一帧。
  // 算「正在读」：闪一下空白详情比多转半帧难看得多。
  return preview ? 'ready' : 'loading'
}

/**
 * 这份预览是不是当前选中那条的。
 *
 * 首次 3 秒级、命中缓存 200 ms 级 —— 差两个数量级，点第二条时第一条很可能还没回来。
 * 不比对的话屏幕上会是**另一个 skill 的文件清单和风险点**，而标题写着你刚点的那个。
 * 这种错不会报任何错，但它恰好错在这一屏唯一要回答的问题上。
 */
export function isPreviewFor(preview: RegistryPreview, hit: RegistryHit): boolean {
  return preview.source === hit.source && preview.skillId === hit.skillId
}

/** 装不了 / 取不到的原因，翻成当前语言的一句话。 */
const PREVIEW_ERR_KINDS: readonly PreviewErrKind[] = [
  'notInstallable',
  'cache',
  'clone',
  'notFound',
  'checkout',
] as const

export function asPreviewError(e: unknown): PreviewError {
  const kind = (e as PreviewError | null)?.kind
  if (kind && PREVIEW_ERR_KINDS.includes(kind)) return e as PreviewError
  // 认不出形状时当 clone 失败处理 —— 那是这条路径上最常见的一种（断网、私有仓库）。
  return { kind: 'clone', detail: String(e) }
}

export function previewErrorText(e: PreviewError, skillId: string): string {
  // `notFound` 那句要把名字念出来 —— 「这个仓库里没有 X」比「找不到」有用得多，
  // 用户下一步是去 GitHub 上核对拼写。别的几句用不到这个变量，多传无害。
  return t(`tools.discover.perr.${e.kind}`, { id: skillId })
}

/** frontmatter 里的描述。**接口不给描述**，所以这是全应用里唯一能拿到它的地方。 */
export function previewDescription(preview: RegistryPreview): string | null {
  const desc = preview.frontmatter?.description?.trim()
  return desc ? desc : null
}

/**
 * 列表行第二行显示什么。
 *
 * 默认是 **source** —— 搜索接口不给描述（方案文档 1.1）。**看过一次详情之后换成描述**，
 * 那时候 source 挪到名字右边，两样都还在。翻回来还看得见，省得为了想起「这个是干嘛的」
 * 再点一次、再等一次 clone。
 */
export interface HitSubtitle {
  text: string
  /** 是描述而不是 source。为真时行首那一列要把 source 显示在名字旁边。 */
  described: boolean
}

export function hitSubtitle(hit: RegistryHit, described: ReadonlyMap<string, string>): HitSubtitle {
  const desc = described.get(hitKey(hit))
  return desc ? { text: desc, described: true } : { text: hit.source, described: false }
}

/**
 * 同名目录在仓库里出现在多处时的那句提示。
 *
 * 早一轮对七个仓库猜过路径只命中 3/7，布局是真的乱。选中的那条已经显示在「仓库内路径」
 * 里；这句话是告诉用户**还有别的同名目录**，装的是哪一个由他自己确认。
 */
export function otherPathsNote(preview: RegistryPreview): string | null {
  const n = preview.otherPaths.length
  return n === 0 ? null : t('tools.discover.detail.otherPaths', { n: String(n) })
}
