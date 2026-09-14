<script setup lang="ts">
// 工具管理 · Skills 面板：左边列表、右边详情、上面一条健康条。
//
// 这是个壳：判断全在 `toolsSkills.ts`（读）和 `toolsSkillsActions.ts`（写），
// 这里只管拉数据、摆位置、把用户的点击转成那两个模块的调用。写操作一律两步走 ——
// 先 dry-run 拿计划弹确认框，用户点了再用**同样的参数**真跑一遍。
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import type {
  Agent,
  AdoptRequest,
  SkillDetail,
  SkillEntry,
  SkillRef,
  SkillScan,
  SkillUpdateCheck,
  ToolSurfaceInfo,
  WriteReport,
} from '../types'
import * as api from '../api'
import { t } from '../i18n'
import { elidePath, formatSize, highlightSegments } from '../format'
import { buildFileTree, flattenTree, treeDepth, type TreeNode } from '../fileTree'
import { agentLabel } from '../agentMeta'
import { agentIcons, fileIconFor, IconCheck, IconChevronDown, IconChevronRight, IconClose, IconDownload, IconFolder, IconGithub, IconLink, IconPencil, IconPinUp, IconPlus, IconRefresh, IconScopeProject, IconScopeUser, IconTrash, IconWrench } from '../components/icons'
import {
  panelAgents,
  selectFirstRow,
  shownOfTotal,
  startToolsListResize,
  toolsAgents,
  toolsQuery,
} from '../toolsPanel'
import { resetSpotlight, revealSelected } from '../listScroll'
import { useHStrip } from '../hstrip'
import {
  BADGE_ORDER,
  SKILL_SCOPES,
  agentsOf,
  chainLines,
  removableLink,
  healthTip,
  agentReach,
  isSkillPinned,
  mainStoreOptions,
  pinnedSkills,
  SKILL_SORTS,
  setSkillSort,
  skillSort,
  type SkillSort,
  queryPath,
  riskIsConclusive,
  scopeCounts,
  sharedDirRef,
  shortenPath,
  skillFilter,
  skillScopes,
  storeScopeMap,
  toggleSkillPin,
  visibleSkills,
  worstBadge,
  type SkillScope,
} from '../toolsSkills'
import {
  addExtraStore,
  adoptAllTargets,
  adoptTargets,
  conflictDone,
  currentConflict,
  effectiveMainStore,
  inMainStore,
  extraStores,
  removeExtraStore,
  repairPlan,
  repairTarget,
  requestsWithResolutions,
  resolveCurrent,
  setMainStore,
  skipAll,
  skipCurrent,
  startConflicts,
  updateIsNoop,
  updateMessageParts,
  type ConflictState,
} from '../toolsSkillsActions'
import ConfirmModal from '../modals/ConfirmModal.vue'
import SkillPlanModal from '../modals/SkillPlanModal.vue'
import SkillConflictModal from '../modals/SkillConflictModal.vue'
import SkillEditor from '../components/SkillEditor.vue'
import SkillDetailSkeleton from '../components/SkillDetailSkeleton.vue'
import SkillFindings from '../components/SkillFindings.vue'

const props = defineProps<{ cwd?: string }>()
const emit = defineEmits<{ (e: 'notify', msg: string, error?: boolean): void }>()

const scan = ref<SkillScan | null>(null)
const surfaces = ref<ToolSurfaceInfo[]>([])
const loading = ref(false)
const error = ref('')
const selectedName = ref<string | null>(null)
const detail = ref<SkillDetail | null>(null)
const detailError = ref('')
/**
 * 详情还在路上。
 *
 * 详情不是扫描给的，是按名字单独读一次：把 skill 目录整个走一遍，每个文件读进来
 * 过一遍风险规则。本机 `hyperframes` 47 个文件要 2.1 秒（小的 13ms～160ms）——
 * 而列表点一下是瞬时的，右边那半边就那么空着两秒，看上去像点了没反应。
 */
const detailLoading = ref(false)

/** 待确认的计划：dry-run 的结果 + 用户点「执行」时要跑的那一下。 */
const plan = ref<{ title: string; report: WriteReport; apply: () => Promise<void> } | null>(null)
/** 收编冲突的三选一。 */
const conflicts = ref<ConflictState | null>(null)
/** 冲突对应的那一批请求 —— 用户选完之后要贴着选择重发。 */
const conflictRequests = ref<AdoptRequest[]>([])
const busy = ref(false)
/**
 * 正在跑的是哪个按钮。
 *
 * 只有一个全局 `busy` 不够：点「删除」之后要先跑一趟 dry-run 才弹确认框，这中间
 * 按钮毫无反应，看上去像没点上 —— 用户会再点一次。转圈得落在**被点的那个**按钮上。
 */
const pending = ref<string | null>(null)
/** 冲突这一批是从哪个按钮发起的，选完之后重发时要沿用同一个转圈位置。 */
const conflictKey = ref('adopt')

const home = computed(() => scan.value?.home ?? '')
const main = computed(() => effectiveMainStore(scan.value))

/**
 * store 路径 → 用户级 / 项目级。行上的记号和健康条那两个筛子都查它。
 *
 * 档次是 store 的属性，一次扫描里不会变，所以摊成一张表算一次 —— 每行各自去
 * `scan.stores` 里线性找是 O(行 × store)。
 */
const storeScopes = computed(() => storeScopeMap(scan.value))

const list = computed(() =>
  visibleSkills(
    scan.value?.skills ?? [],
    { ...skillFilter.value, query: toolsQuery.value },
    [...toolsAgents.value],
    pinnedSkills.value,
    skillSort.value,
    storeScopes.value,
  ),
)

/** 行前面那一到两个档次记号。两档都占的就画两个 —— 那是它的实情。 */
function scopesOf(entry: SkillEntry): SkillScope[] {
  return skillScopes(entry, storeScopes.value)
}

const SCOPE_ICONS = { user: IconScopeUser, project: IconScopeProject } as const

const selected = computed<SkillEntry | null>(
  () => list.value.find((s) => s.name === selectedName.value)
    ?? scan.value?.skills.find((s) => s.name === selectedName.value)
    ?? null,
)

async function load() {
  loading.value = true
  error.value = ''
  try {
    const [s, surf] = await Promise.all([
      api.toolsScanSkills(props.cwd, extraStores.value),
      api.toolSurfaces(props.cwd),
    ])
    scan.value = s
    surfaces.value = surf
    // 选中的那条可能已经被删掉 / 改名了。
    if (selectedName.value && !s.skills.some((k) => k.name === selectedName.value)) {
      selectedName.value = null
      detail.value = null
    } else if (selectedName.value) {
      void loadDetail(selectedName.value)
    }
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

async function loadDetail(name: string) {
  detailError.value = ''
  detailLoading.value = true
  // 展开状态跟着 skill 走：上一条展开的 `references`，换一条之后该回到折起来。
  for (const k of Object.keys(fileDirState)) delete fileDirState[k]
  try {
    const got = await api.toolsSkillDetail(name, props.cwd, extraStores.value)
    // 读的过程里用户可能已经点去了别的 skill。慢的那一条回来得晚，不能把它的内容
    // 盖到别人身上 —— 2 秒的窗口足够点好几下了。
    if (selectedName.value !== name) return
    detail.value = got
  } catch (e) {
    if (selectedName.value !== name) return
    detail.value = null
    detailError.value = String(e)
  } finally {
    if (selectedName.value === name) detailLoading.value = false
  }
}

function select(name: string) {
  selectedName.value = name
  detail.value = null
  void loadDetail(name)
}

onMounted(() => {
  void load()
  document.addEventListener('pointerdown', onDocPointerDown)
})
selectFirstRow(() => list.value, () => selectedName.value !== null, (s) => select(s.name))
onUnmounted(() => document.removeEventListener('pointerdown', onDocPointerDown))
watch(() => props.cwd, load)

// ---------------------------------------------------------------------------
// 健康条
// ---------------------------------------------------------------------------

const summary = computed(() => scan.value?.summary ?? null)

function toggleBadgeFilter(badge: (typeof BADGE_ORDER)[number]) {
  skillFilter.value = {
    ...skillFilter.value,
    badge: skillFilter.value.badge === badge ? null : badge,
  }
}

/** 「来自 github」不是毛病，是来源，所以它和四个角标互不相干，各占一个开关。 */
function toggleFromGit() {
  skillFilter.value = { ...skillFilter.value, fromGit: !skillFilter.value.fromGit }
}

/** 每档各有几条。两档都占的两边都记 —— 和角标那排同一套算法，加起来可以超过总数。 */
const scopeTotals = computed(() => scopeCounts(scan.value?.skills ?? [], storeScopes.value))

/**
 * 档次是两个**独立**开关，默认都开（= 不过滤）。
 *
 * 不做成角标那样的单选：用户要的是「只看项目里的」和「只看我自己的」，这是两个子集
 * 而不是五选一。两个都关掉列表会空 —— 那是字面结果，再点一下就回来，比「关了等于
 * 没关」好懂。
 */
function toggleScope(scope: SkillScope) {
  const on = skillFilter.value.scopes
  skillFilter.value = {
    ...skillFilter.value,
    scopes: on.includes(scope)
      ? on.filter((s) => s !== scope)
      : SKILL_SCOPES.filter((s) => s === scope || on.includes(s)),
  }
}

// 六个角标横着排，窗口一窄就装不下。以前它们硬挤在健康条里，把右边的主 store 顶出
// 屏幕；现在改成吃掉剩余宽度、自己横向滑（和会话页 tab 条同一份实现）。
const badgeViewportRef = ref<HTMLElement>()
const badgeTrackRef = ref<HTMLElement>()
const {
  panning: badgePanning,
  canLeft: badgeCanLeft,
  canRight: badgeCanRight,
  trackStyle: badgeTrackStyle,
  revealEl: revealBadge,
  onWheel: onBadgeWheel,
  onPanPointerDown: onBadgePanDown,
} = useHStrip(badgeViewportRef, badgeTrackRef)

/**
 * 两侧的淡出宽度。
 *
 * 不用「铺一层底色渐变盖住边缘」那招（tab 条是那么做的）：健康条自己不铺底色，
 * 壁纸模式下透出来的是用户的图，盖一层实色渐变会在这一行上糊出两块不透的补丁。
 * 改用 mask 让内容本身淡掉，底下是什么都不碍事。
 */
/**
 * 点完角标再把它滑回视野里。
 *
 * 要等一拍：点下去左边的「43 skills」会变成「1/43 skills」，那几个字一变宽，滑动区
 * 就跟着窄一截 —— 在同一拍里量，量到的是上一个宽度，露出来的那一项右边还是缺一角。
 */
function revealBadgeAfterFilter(ev: MouseEvent) {
  const el = ev.currentTarget as HTMLElement
  nextTick(() => revealBadge(el))
}

const badgeFade = computed(() => ({
  '--badge-fade-l': badgeCanLeft.value ? '20px' : '0px',
  '--badge-fade-r': badgeCanRight.value ? '20px' : '0px',
}))


// 候选规则住在 `toolsSkills.ts`（有单测），这儿不再自己抄一遍 —— 抄出来的那份
// 漏了「项目目录 / agent 自有目录不能当主 store」，下拉里就混进了 sales-app 的
// `.agents/skills` 和 agy 的 `~/.gemini/config/skills`。
const storeOptions = computed(() => (scan.value ? mainStoreOptions(scan.value) : []))

// 主 store 下拉。原生 <select> 在这条健康条上是唯一一处系统控件，字体和圆角都跟
// 周围对不上；换成和设置里同一套 `.set-dropdown-*` 菜单（那套就是为了顶掉原生
// <select> 写的），顺带能把「几个实体」排到右边对齐。
const storeMenuOpen = ref(false)
const storeWrapEl = ref<HTMLElement>()

const mainStoreLabel = computed(() => {
  const hit = storeOptions.value.find((o) => o.path === main.value)
  return hit ? storeLabel(hit.path) : t('tools.skills.mainStorePick')
})

function pickMainStore(path: string) {
  storeMenuOpen.value = false
  setMainStore(path || null)
}

/**
 * 「新增主目录」：让用户自己指一个。
 *
 * 内置的候选只有各家 agent 声明的目录和三个第三方管理器，skill 放在别处（外置盘、
 * 同步盘、自己的 dotfiles 仓库）时一个都不合用。
 *
 * 挑中的目录如果本来就在候选里，直接选中就好，不往自定义列表里再塞一条重的；
 * 挑中的如果是个**不够格**的已知目录（项目目录 / agent 自有目录），不能因为「用户
 * 说了算」就放行 —— 内容搬进 agent 自有目录，下一次那家自己重写目录就没了。
 */
async function addMainStore() {
  storeMenuOpen.value = false
  const { open } = await import('@tauri-apps/plugin-dialog')
  const picked = await open({ directory: true, multiple: false })
  const dir = typeof picked === 'string' ? picked : picked?.[0]
  if (!dir) return

  const known = scan.value?.stores.find((s) => s.path === dir)
  if (known && !known.canBeMain) {
    emit('notify', t('tools.skills.storeNotEligible', { path: short(dir) }), true)
    return
  }
  addExtraStore(dir)
  setMainStore(dir)
  await load()
}

/** 从候选列表里去掉一个自定义目录。磁盘上那个目录一个字节都不动。 */
async function dropMainStore(path: string) {
  removeExtraStore(path)
  await load()
}

function onDocPointerDown(e: MouseEvent) {
  if (!storeMenuOpen.value) return
  if (storeWrapEl.value && !storeWrapEl.value.contains(e.target as Node)) storeMenuOpen.value = false
}

// ---------------------------------------------------------------------------
// 写操作：一律先 dry-run
// ---------------------------------------------------------------------------

/** 跑一次 dry-run，有内容就弹确认框；空计划直接说「没什么要做的」。 */
async function propose(
  key: string,
  title: string,
  run: (dryRun: boolean) => Promise<WriteReport>,
  after?: (report: WriteReport) => void,
) {
  if (busy.value) return
  busy.value = true
  pending.value = key
  try {
    const report = await run(true)
    if (report.conflicts.length > 0) {
      conflicts.value = startConflicts(report.conflicts)
      return
    }
    if (report.steps.length === 0) {
      emit('notify', t('tools.skills.plan.nothing'))
      return
    }
    plan.value = {
      title,
      report,
      apply: async () => {
        const done = await run(false)
        after?.(done)
        await load()
      },
    }
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
    pending.value = null
  }
}

async function applyPlan() {
  const current = plan.value
  if (!current || busy.value) return
  busy.value = true
  try {
    await current.apply()
    plan.value = null
    emit('notify', t('tools.skills.plan.applied'))
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

function adopt(requests: AdoptRequest[], key: string) {
  const store = main.value
  if (!store) {
    emit('notify', t('tools.skills.needMainStore'), true)
    return
  }
  if (requests.length === 0) {
    emit('notify', t('tools.skills.plan.nothing'))
    return
  }
  conflictRequests.value = requests
  conflictKey.value = key
  void propose(key, t('tools.skills.action.adopt'), (dry) =>
    api.toolsAdoptSkills(requests, store, dry),
  )
}

function adoptSelected() {
  if (!selected.value || !main.value) return
  adopt(adoptTargets(selected.value, main.value), 'adopt')
}

/**
 * 「全部搬进主 store」里的「全部」= 列表里现在这些，跟着筛选和搜索走。
 *
 * 按钮就钉在角标筛选器旁边，筛到「重复 26」点下去却搬全机器 43 条，是屏幕上写着
 * 一件事、实际做另一件事。要搬全部就把筛选清掉 —— 列表本来就是全部。
 */
function adoptEverything() {
  if (!main.value) return
  adopt(adoptAllTargets(list.value, main.value), 'adoptAll')
}

/**
 * 按钮上那个数：这一下要搬几**份**内容。0 就置灰，省得点了只弹一句「没什么可做的」。
 *
 * 它比列表条数大是正常的 —— 一条 skill 在三个地方各存了一份就要各搬一次。数「份」
 * 不数「条」是因为这个数要往下一屏传：计划框里写的是「移动 62 · 建链 62」，按钮上
 * 写条数的话两屏对不上。条数另算一个，只用在 tooltip 里把这笔账说清楚。
 */
const adoptAllCount = computed(() =>
  main.value ? adoptAllTargets(list.value, main.value).length : 0,
)

/** 这些份数来自几条 skill（已经全在主 store 里的那几条不算）。 */
const adoptAllSkills = computed(() =>
  main.value
    ? list.value.filter((s) => adoptTargets(s, main.value as string).length > 0).length
    : 0,
)

function repairSelected() {
  const entry = selected.value
  if (!entry) return
  const target = repairTarget(entry, main.value)
  const items = repairPlan(entry, target)
  if (items.length === 0) {
    emit('notify', t('tools.skills.plan.nothing'))
    return
  }
  void propose('repair', t('tools.skills.action.repair'), (dry) =>
    api.toolsRepairLinks(items, dry),
  )
}

const keepBodies = ref(false)
/**
 * 勾和取消勾都要二次确认。
 *
 * 这个勾决定的是「删除」到底删什么 —— 勾上只清各家 agent 的入口、文件夹留着，
 * 不勾连文件一起删。两个方向都是改写另一个按钮的破坏力，而它自己长得像个普通选项，
 * 所以两边都拦一下，把改完之后「删除」会做什么直接说出来。
 */
const keepBodiesAsk = ref<boolean | null>(null)

function askKeepBodies(next: boolean) {
  keepBodiesAsk.value = next
}

function confirmKeepBodies() {
  if (keepBodiesAsk.value !== null) keepBodies.value = keepBodiesAsk.value
  keepBodiesAsk.value = null
}

/** 取消时把勾恢复原样 —— DOM 上那个 checkbox 已经自己翻过去了。 */
function cancelKeepBodies() {
  keepBodiesAsk.value = null
  const on = keepBodies.value
  keepBodies.value = !on
  void nextTick(() => (keepBodies.value = on))
}

function deleteSelected() {
  const entry = selected.value
  if (!entry) return
  void propose(
    'delete',
    t('tools.skills.action.delete'),
    (dry) =>
      api.toolsDeleteSkill(
        entry.name,
        { keepBodies: keepBodies.value },
        props.cwd,
        extraStores.value,
        dry,
      ),
    () => {
      selectedName.value = null
      detail.value = null
    },
  )
}

/**
 * 只删这一份内容，别处的一个字节都不动。
 *
 * 和「删除」那个按钮的区别：那个是把这个 skill 连同它所有的内容和入口一起清掉；
 * 这个只清掉某一份没人读的实体目录 —— 但仍然要先解掉落在它身上的链接，否则删完
 * 就是一条新的死链。计划由后端算，这里照旧先 dry-run 再弹框。
 */
/**
 * 把一份过期的受管副本按源重拷一遍（方案 4.4 / 验收 #8）。
 *
 * 不走 `propose` 那条 dry-run + 确认框的路：这一步只做一件事，而且**不会丢东西** ——
 * 后端在副本有本地修改时直接拒绝（`CopyChanged` / `Diverged`），所以被覆盖掉的只可能
 * 是一份和源同源的旧内容。有本地修改的那两态前端根本不给这个按钮，后端再拦一道。
 */
async function resyncCopy(path: string) {
  if (busy.value) return
  busy.value = true
  pending.value = `resync:${path}`
  try {
    await api.toolsResyncCopy(path)
    await load()
    if (selectedName.value) await loadDetail(selectedName.value)
    emit('notify', t('tools.skills.action.resynced', { path: short(path) }))
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
    pending.value = null
  }
}

function deleteBody(path: string) {
  const entry = selected.value
  if (!entry) return
  void propose(`delBody:${path}`, t('tools.skills.action.deleteBody'), (dry) =>
    api.toolsDeleteBody(entry.name, path, props.cwd, extraStores.value, dry),
  )
}

/**
 * 拆掉一条没人读的活链接。
 *
 * 走和别的写入一样的「先 dry-run 出计划、确认了才真跑」—— 这一条虽然只有一步，但它
 * 动的是用户机器上的文件，计划框里那一行会把「拆哪条、它现在指着谁」摆出来。
 */
function unlinkRef(path: string) {
  void propose(`unlink:${path}`, t('tools.skills.action.unlinkRef'), (dry) =>
    api.toolsUnlinkRef(path, dry),
  )
}

function toggleAgent(agent: Agent, on: boolean) {
  const entry = selected.value
  if (!entry) return
  const store = ownSkillsDir(agent)
  if (!store) return

  // 已经读得到却按「启用」——在它自己的目录里再建一条链接只是多一条绕远路，
  // 正是这个面板要清理的东西。什么都不做，并且说清为什么。
  const reach = reachOf(entry, agent)
  if (on && reach.state !== 'off') {
    explainReach('alreadyOn', entry, agent)
    return
  }
  // 只靠共用目录生效的，关不掉：那个目录不归这家管，删掉会连着断别家。
  if (!on && reach.state === 'shared') {
    explainReach('cannotOff', entry, agent)
    return
  }

  const body = on ? repairTarget(entry, main.value) : null
  if (on && !body) {
    emit('notify', t('tools.skills.noBody'), true)
    return
  }
  void propose(
    `toggle:${agent}`,
    on ? t('tools.skills.action.enable') : t('tools.skills.action.disable'),
    (dry) => api.toolsToggleSkill(entry.name, store, body, on, dry),
  )
}

// ---------------------------------------------------------------------------
// 从远端更新
// ---------------------------------------------------------------------------

/**
 * 待确认的更新。`null` 时不弹框。
 *
 * 和别的写操作不一样，这一步**先要联网 fetch** 才知道会发生什么（落后几个提交、
 * 哪些本地改动会被冲掉），所以走不了 `propose()` 那条 dry-run 的路 —— 那边的
 * `WriteReport` 描述的是软链手术，而这里一步都不是。
 */
const updateCheck = ref<SkillUpdateCheck | null>(null)

/** 主 body 是不是从远端 clone 来的。不是就不显示「更新」。 */
const gitInfo = computed(() => detail.value?.git ?? null)

const updateMessage = computed(() =>
  updateCheck.value
    ? updateMessageParts(updateCheck.value)
        .map((p) => t(p.key, p.vars))
        .join('\n')
    : '',
)

async function checkUpdate() {
  const body = detail.value?.primary
  if (!body || busy.value) return
  busy.value = true
  pending.value = 'update'
  try {
    const check = await api.toolsCheckSkillUpdate(body)
    // 已经是最新、本地也没改过：弹一个只能点「取消」的框是在浪费用户一次点击。
    if (updateIsNoop(check)) {
      emit('notify', t('tools.skills.update.upToDate', { sha: check.local }))
      return
    }
    updateCheck.value = check
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
    pending.value = null
  }
}

async function applyUpdate() {
  const body = detail.value?.primary
  const name = selectedName.value
  updateCheck.value = null
  if (!body || !name || busy.value) return
  busy.value = true
  pending.value = 'update'
  try {
    const sha = await api.toolsUpdateSkill(body)
    emit('notify', t('tools.skills.update.done', { sha }))
    // 内容换了一批文件：清单、frontmatter、风险都得重算，角标（重复 / 死链）也可能变。
    await load()
    await loadDetail(name)
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
    pending.value = null
  }
}

// ---------------------------------------------------------------------------
// 内置编辑器
// ---------------------------------------------------------------------------

/**
 * 编辑器开着没有。
 *
 * 开在 `detail.primary` 上，也就是**实体**目录 —— 这个面板处理的 skill 大多是一堆指向
 * 同一份内容的链接，顺着链接写在某些文件系统上会把链接本身替换成普通文件，等于把这个
 * 面板费劲维护的那张网剪断一条。
 */
const editing = ref(false)

const editBody = computed(() => detail.value?.primary ?? null)

/** 存过盘之后重扫：大小、风险、frontmatter 全可能变了。 */
async function onEditorSaved() {
  const name = selectedName.value
  if (!name) return
  await load()
  await loadDetail(name)
}

// ---------------------------------------------------------------------------
// 冲突
// ---------------------------------------------------------------------------

function onConflictChoice(choice: Parameters<typeof resolveCurrent>[1]) {
  if (!conflicts.value) return
  conflicts.value = resolveCurrent(conflicts.value, choice)
  void maybeResubmit()
}

function onConflictSkip() {
  if (!conflicts.value) return
  conflicts.value = skipCurrent(conflicts.value)
  void maybeResubmit()
}

function onConflictSkipAll() {
  if (!conflicts.value) return
  conflicts.value = skipAll(conflicts.value)
  void maybeResubmit()
}

/** 冲突全处理完 → 贴着选择重发一次 dry-run，走正常的确认框。 */
async function maybeResubmit() {
  const state = conflicts.value
  if (!state || !conflictDone(state)) return
  const store = main.value
  const requests = requestsWithResolutions(conflictRequests.value, state)
  conflicts.value = null
  if (!store || requests.length === 0) return
  await propose(conflictKey.value, t('tools.skills.action.adopt'), (dry) =>
    api.toolsAdoptSkills(requests, store, dry),
  )
}

function closeConflicts() {
  conflicts.value = null
}

// ---------------------------------------------------------------------------
// 展示辅助
// ---------------------------------------------------------------------------

function short(path: string) {
  return shortenPath(path, home.value)
}

/**
 * 下拉里的 store 路径。
 *
 * 内置的几个都在 `~` 下、缩完只有两三段；用户自己加的可能是
 * `/Volumes/ssd/…/一长串/skills`，原样铺出来会把菜单撑成整屏宽。中间省略而不是
 * 末尾省略：末尾那两段（`scratchpad/my-skills`）才是分得清哪个是哪个的部分。
 */
function storeLabel(path: string) {
  return elidePath(short(path), 2)
}

/**
 * 在系统文件管理器里定位这个目录。
 *
 * 后端的 `reveal_in_finder` 会退到**最近一个存在的祖先**，所以还没建出来的兜底
 * 主 store（`~/.agents/skills`）点了也不会报错，而是打开 `~/` —— 这正是想要的：
 * 用户想看的是「那地方现在长什么样」。
 */
async function reveal(path: string) {
  try {
    await api.revealInFinder(path)
  } catch (e) {
    emit('notify', String(e), true)
  }
}

function badgeOf(entry: SkillEntry) {
  return worstBadge(entry.badges)
}

function refTip(ref: SkillRef) {
  const tip = healthTip(ref.health, home.value)
  return t(tip.key, tip.vars)
}

/** 这家自己的 skills 目录。后端不给（这家不支持 skills）时是 null。 */
function ownSkillsDir(agent: Agent): string | null | undefined {
  return surfaces.value.find((s) => s.agent === agent)?.skillsDir
}

function reachOf(entry: SkillEntry, agent: Agent) {
  return agentReach(entry, agent, ownSkillsDir(agent))
}

function agentEnabled(entry: SkillEntry, agent: Agent): boolean {
  return reachOf(entry, agent).state !== 'off'
}

/**
 * 「为什么这个开关点不动」的说明框。
 *
 * 原来走 `notify` 弹红条。两个毛病：这段话有三四行，塞进一条会自己消失的横幅里根本
 * 读不完；而且它**不是错误** —— 用户点了一个本来就没有意义的动作，需要的是解释，
 * 不是报错。所以改成一个只有「我知道了」的告知框。
 */
const reachNote = ref<{ title: string; body: string } | null>(null)

function explainReach(kind: 'alreadyOn' | 'cannotOff', entry: SkillEntry, agent: Agent) {
  reachNote.value = {
    title: t(`tools.skills.reach.${kind}`),
    body: agentToggleTip(entry, agent),
  }
}

/** 开关的悬停说明：除了「开/关」，还要说清它是**靠哪个目录**生效的。 */
function agentToggleTip(entry: SkillEntry, agent: Agent): string {
  const label = agentLabel(agent, true)
  const reach = reachOf(entry, agent)
  if (reach.state === 'off') return `${label} · ${t('tools.skills.action.enable')}`
  if (reach.state === 'own') return `${label} · ${t('tools.skills.action.disable')}`
  // 共用目录：把是哪个目录、还有谁在读一并说了，不然「为什么关不掉」无从得知。
  const dirs = reach.via.map((r) => short(r.store)).join(' / ')
  const others = [...new Set(reach.via.flatMap((r) => r.agents))]
    .filter((a) => a !== agent)
    .map((a) => agentLabel(a, true))
    .join(' / ')
  return t('tools.skills.sharedReach', { agent: label, dirs, others: others || '—' })
}

// ---------------------------------------------------------------------------
// 公共目录
// ---------------------------------------------------------------------------

/**
 * `~/.agents/skills` —— 谁的目录都不是，但好几家都读它。
 *
 * 取 `defaultMain` 而不是写死路径：兜底主 store 和这个公共目录本来就是同一个地方，
 * Windows 上的写法也只有后端知道。
 */
const sharedDir = computed(() => scan.value?.defaultMain ?? null)

/** 会读公共目录的那几家（只算这排上列出来的）。 */
const sharedDirAgents = computed<Agent[]>(() => {
  const hit = scan.value?.stores.find((s) => s.path === sharedDir.value)
  return (hit?.agents ?? []).filter((a) => shownAgents.value.includes(a))
})

function sharedRefOf(entry: SkillEntry) {
  return sharedDirRef(entry, sharedDir.value)
}

function sharedOn(entry: SkillEntry): boolean {
  return sharedRefOf(entry) !== null
}

function sharedTip(entry: SkillEntry): string {
  const ref = sharedRefOf(entry)
  const path = short(sharedDir.value ?? '')
  const agents = sharedDirAgents.value.map((a) => agentLabel(a, true)).join(' / ') || '—'
  // 内容就摆在这儿（公共目录同时是主 store）：没有链接可拆，说清楚而不是给个点不动的开关。
  if (ref?.health.state === 'realDir') return t('tools.skills.sharedDirBody', { path, agents })
  const action = ref ? t('tools.skills.action.disable') : t('tools.skills.action.enable')
  return `${t('tools.skills.sharedDirTip', { path, agents })} · ${action}`
}

/**
 * 公共目录的开关。
 *
 * 和 agent 开关走的是同一条后端命令，只是 store 换成了公共目录 —— `toggle` 本来就
 * 不认「这是谁的目录」，它只管在一个目录里建 / 拆一条链接。
 */
function toggleShared(on: boolean) {
  const entry = selected.value
  const dir = sharedDir.value
  if (!entry || !dir) return
  const ref = sharedRefOf(entry)
  // 内容本身就摆在公共目录里，没有链接可拆 —— 同样是「解释」不是「报错」，走告知框。
  if (!on && ref?.health.state === 'realDir') {
    reachNote.value = {
      title: t('tools.skills.reach.cannotOff'),
      body: sharedTip(entry),
    }
    return
  }
  const body = on ? repairTarget(entry, main.value) : null
  if (on && !body) {
    emit('notify', t('tools.skills.noBody'), true)
    return
  }
  void propose(
    'toggle:shared',
    on ? t('tools.skills.action.enable') : t('tools.skills.action.disable'),
    (dry) => api.toolsToggleSkill(entry.name, dir, body, on, dry),
  )
}


/**
 * 「启用于」那排列谁：**本机装了的全都列**，不受设置里的可见 agent 影响。
 *
 * 不按「有没有 skills 目录」再筛一道 —— 装了却不显示，用户第一反应是功能少了，
 * 而不是「这家不支持」。所以照列，真不支持的那几个在 tooltip 里说清原因（见
 * `supportsSkills`）。
 *
 * 注：本机这七家**全都支持** skills（agy / opencode / pi 三家一度被记成不支持，是勘察
 * 错了，见方案文档「勘察返工」一节）。`supportsSkills` 留着是因为它读的是后端真实
 * 给不给 `skillsDir`，将来接一家没有 skills 的 agent 时它自己会亮，而不是渲染出一个
 * 点了没反应的开关。
 */
const shownAgents = computed(() => panelAgents())

/** 这家能不能用 skills —— 有没有一个它会去扫的目录。 */
function supportsSkills(agent: Agent): boolean {
  return !!surfaces.value.find((s) => s.agent === agent)?.skillsDir
}

// 骨架的宽度写死成一张不规则的表。等宽的骨架看起来像表格而不像列表，
// 反而提示不出「这儿马上会是一行行长短不一的 skill」。
const SKEL_ROWS = [
  { name: '46%', chip: '34px', desc: '88%' },
  { name: '62%', chip: '28px', desc: '71%' },
  { name: '38%', chip: '0px', desc: '94%' },
  { name: '55%', chip: '42px', desc: '63%' },
  { name: '70%', chip: '28px', desc: '82%' },
  { name: '43%', chip: '0px', desc: '90%' },
  { name: '58%', chip: '34px', desc: '68%' },
  { name: '50%', chip: '28px', desc: '85%' },
  { name: '65%', chip: '0px', desc: '77%' },
]
const SKEL_CHIP = ['52px', '52px', '52px', '52px']
/** 够铺满一屏列表；多出来的由 `.skill-skel-list` 裁掉，不会多出一根滚动条。 */
const SKEL_COUNT = 20

// hover 跟随浮块：和会话 / 回收站 / 导出历史那三个列表同一套交互 —— 鼠标移到某行上，
// 把它的 offsetTop / offsetHeight 写进 --spot-y / --spot-h 驱动 .list-spotlight。
// 滚动期间临时隐藏、停 140ms 再恢复，否则光标不动、内容在动时浮块会跟着抖。
// （这段和那三个视图逐字相同，值得抽成 composable，但那要一次改四处，留给后面单独做。）
const listEl = ref<HTMLElement>()
/** 这一轮 `list` 变化是不是换次序引起的 —— 决定上面那个 watch 走哪一条。 */
let sortJustChanged = false

function changeSort(sort: SkillSort) {
  if (sort === skillSort.value) return
  sortJustChanged = true
  setSkillSort(sort)
}
const spotlightEl = ref<HTMLElement>()
let scrolling = false
let scrollIdle = 0

function markScrolling() {
  if (!scrolling) {
    scrolling = true
    listEl.value?.classList.remove('has-spot')
  }
  clearTimeout(scrollIdle)
  scrollIdle = window.setTimeout(() => {
    scrolling = false
  }, 140)
}

function onListMouseOver(e: MouseEvent) {
  if (scrolling) return
  const sa = listEl.value
  const sp = spotlightEl.value
  if (!sa || !sp) return
  const row = (e.target as HTMLElement | null)?.closest<HTMLElement>('.skill-row')
  if (!row || !sa.contains(row)) return
  // 从隐藏态重新出现时先 no-slide 直接跳到目标行再淡入，省掉整屏滑过去的突兀感。
  const reappearing = !sa.classList.contains('has-spot')
  if (reappearing) sp.classList.add('no-slide')
  sp.style.setProperty('--spot-y', `${row.offsetTop}px`)
  sp.style.setProperty('--spot-h', `${row.offsetHeight}px`)
  sa.classList.add('has-spot')
  if (reappearing) {
    requestAnimationFrame(() => requestAnimationFrame(() => sp.classList.remove('no-slide')))
  }
}

function onListMouseLeave() {
  listEl.value?.classList.remove('has-spot')
}

// 列表换了一批（改过滤器、清搜索词、重新扫描）要做两件事：
//
// 1. 浮块作废 —— 它停在上一批某一行的偏移量上，而它算进 scrollHeight，会把滚动
//    位置钉在一个新列表根本够不着的地方；
// 2. 选中那行重新露出来 —— 先筛到两条、点中一条、再把筛选关掉，那一条会落回 50
//    条的中段，右边详情画着它而左边找不着。
//
// 顺序不能反：先把浮块收回去，scrollHeight 才是新列表的真实长度，`scrollIntoView`
// 才算得准。
watch(list, async () => {
  resetSpotlight(listEl.value, spotlightEl.value)
  await nextTick()
  // 换次序是个例外：这时候「把选中那行追回来」恰好是反的。用户点「按名称」问的是
  // 「从头按字母看一遍」，而 `revealSelected` 会把列表停在那条 skill 的新位置上 ——
  // 可能是第 37 条，屏幕上是一段中间，看上去像没排。
  if (sortJustChanged) {
    sortJustChanged = false
    if (listEl.value) listEl.value.scrollTop = 0
    return
  }
  revealSelected(listEl.value, '.skill-row.active')
})

onUnmounted(() => clearTimeout(scrollIdle))

// 文件清单的目录树。**默认折起来**：一个 skill 动辄 50+ 个文件，全展开之后详情页
// 下半屏全是 `references/…` 的长路径，反而看不出「它由哪几块组成」。折着看到的是
// `SKILL.md` + `assets 51 个文件` + `references 11 个文件` 这样的骨架，要细节再点开。
const fileDirState = reactive<Record<string, boolean>>({})
const fileTree = computed(() => buildFileTree(detail.value?.files ?? []))
const flatFileNodes = computed(() => flattenTree(fileTree.value, fileDirOpen))

function fileDirOpen(path: string): boolean {
  return fileDirState[path] === true
}

function toggleFileDir(path: string) {
  fileDirState[path] = !fileDirOpen(path)
}

/** 目录行右边那个数：底下一共有几个文件（不含目录本身）。 */
function countFiles(node: TreeNode<{ path: string }>): number {
  if (node.item) return 1
  return node.children.reduce((n, c) => n + countFiles(c), 0)
}

function skel(i: number) {
  return SKEL_ROWS[(i - 1) % SKEL_ROWS.length]
}

function fmtTime(ms: number | null) {
  if (!ms) return ''
  return new Date(ms).toLocaleDateString()
}

// ---------------------------------------------------------------------------
// 搜索命中
// ---------------------------------------------------------------------------

/**
 * 把命中的那几个字切出来，和会话列表 / 回收站用的是同一个 `.kw-hit`。
 *
 * 不高亮的话，搜 `hyperframes` 出来一堆 `lottie` / `tailwind` / `three`（命中在描述里，
 * 而描述那一行是省略号截断的），整个列表看上去像根本没过滤。
 */
function hl(text: string) {
  return highlightSegments(text, toolsQuery.value)
}

/** 第二行显示什么：只命中路径时顶出那条路径，否则还是描述。 */
function rowSubtitle(s: SkillEntry): string {
  const hit = queryPath(s, toolsQuery.value)
  if (hit) return short(hit)
  return s.description ?? short(s.bodies[0]?.path ?? '')
}
</script>

<template>
  <!-- 健康条：一眼看到「坏了几条」，点角标就只看那一类。 -->
  <div class="list-head tools-health">
    <template v-if="summary">
      <span
        class="tools-health-total"
        v-tooltip="list.length === summary.total
          ? ''
          : t('tools.total.filtered', {
            shown: String(list.length),
            total: String(summary.total),
          })"
      >{{ t('tools.skills.total', { n: shownOfTotal(list.length, summary.total) }) }}</span>
      <!-- 档次不是毛病也不是来源，是「这东西跟着谁走」—— 和右边那排隔开一条线，
           免得被当成第七种病。两个都默认开着，也就是默认不过滤。
           它排在角标前面、且**不进滑动区**：角标那排横着一长条，谁先被挤出屏幕
           由宽度说了算，而「只看项目级」是天天要点的，不能靠滑一段才找得到。 -->
      <button
        v-for="scope in SKILL_SCOPES"
        :key="scope"
        type="button"
        class="skill-badge-chip scope"
        :class="[scope, {
          active: skillFilter.scopes.includes(scope),
          zero: scopeTotals[scope] === 0,
        }]"
        :aria-pressed="skillFilter.scopes.includes(scope)"
        v-tooltip="t(`tools.skills.scopeTip.${scope}`)"
        @click="toggleScope(scope)"
      >
        <component :is="SCOPE_ICONS[scope]" class="skill-badge-ic" />
        {{ t(`tools.scope.${scope}`) }}
        <b>{{ scopeTotals[scope] }}</b>
      </button>
      <span class="skill-head-divider" aria-hidden="true" />
      <!-- 六个角标吃掉剩下的所有宽度，装不下就横着滑（同会话页 tab 条，见 hstrip.ts）。
           以前它们是硬挤在这一行里的，窗口一窄就把右边的「主 store」整个顶出屏幕 ——
           而主 store 是搬家动作的落点，不知道自己往哪儿搬，那个按钮就没法点。 -->
      <div
        ref="badgeViewportRef"
        class="skill-badge-scroll"
        :style="badgeFade"
        @wheel="onBadgeWheel"
        @pointerdown="onBadgePanDown"
      >
        <div
          ref="badgeTrackRef"
          class="skill-badge-track"
          :class="{ panning: badgePanning }"
          :style="badgeTrackStyle"
        >
          <button
            v-for="badge in BADGE_ORDER"
            :key="badge"
            type="button"
            class="skill-badge-chip"
            :class="[badge, { active: skillFilter.badge === badge, zero: summary[badge] === 0 }]"
            v-tooltip="t(`tools.skills.badgeTip.${badge}`)"
            @click="toggleBadgeFilter(badge); revealBadgeAfterFilter($event)"
          >
            {{ t(`tools.skills.badge.${badge}`) }}
            <b>{{ summary[badge] }}</b>
          </button>
          <!-- 来源，不是毛病：只有这些还能拉到新版本，所以和那四个并排最有用。 -->
          <button
            type="button"
            class="skill-badge-chip fromGit"
            :class="{ active: skillFilter.fromGit, zero: summary.fromGit === 0 }"
            v-tooltip="t('tools.skills.badgeTip.fromGit')"
            @click="toggleFromGit(); revealBadgeAfterFilter($event)"
          >
            <IconGithub class="skill-badge-ic" />
            {{ t('tools.skills.badge.fromGit') }}
            <b>{{ summary.fromGit }}</b>
          </button>
        </div>
      </div>
    </template>
    <template v-else-if="loading">
      <span class="skill-skel-bar" style="width: 58px" />
      <span v-for="i in 4" :key="i" class="skill-skel-bar chip" :style="{ width: SKEL_CHIP[i - 1] }" />
    </template>

    <span v-if="!summary" class="skill-head-gap" />

    <!-- 首扫完成前这个下拉是空的，渲染出来就是个塌掉的小方块，比没有还难看。 -->
    <template v-if="!scan && loading">
      <span class="skill-skel-bar" style="width: 46px" />
      <span class="skill-skel-bar chip" style="width: 168px; margin-left: 0" />
      <span class="skill-skel-bar chip" style="width: 104px; margin-left: 0" />
    </template>
    <template v-else>
      <div class="skill-main-store">
        <span>{{ t('tools.skills.mainStore') }}</span>
        <div ref="storeWrapEl" class="set-dropdown-wrap">
          <button
            type="button"
            class="set-dropdown-btn skill-store-btn"
            :class="{ active: storeMenuOpen }"
            :aria-expanded="storeMenuOpen"
            aria-haspopup="menu"
            @click.stop="storeMenuOpen = !storeMenuOpen"
          >
            <span class="skill-store-cur">{{ mainStoreLabel }}</span>
            <IconChevronDown class="set-dropdown-chev" />
          </button>
          <div v-if="storeMenuOpen" class="set-dropdown-menu skill-store-menu" role="menu">
            <!-- 这几条路径长得很像，光看一列路径分不出哪个是「内容该待的地方」。 -->
            <p class="skill-store-title">{{ t('tools.skills.mainStoreTitle') }}</p>
            <button
              v-for="s in storeOptions"
              :key="s.path"
              type="button"
              class="set-dropdown-item skill-store-item"
              role="menuitem"
              v-tooltip="short(s.path)"
              @click.stop="pickMainStore(s.path)"
            >
              <span class="set-dropdown-check"><IconCheck v-if="s.path === main" /></span>
              <span class="skill-store-path">{{ storeLabel(s.path) }}</span>
              <span class="skill-store-count">
                {{ t('tools.skills.realDirs', { n: String(s.realDirs) }) }}
              </span>
              <!-- 行本身是 <button>，这里不能再嵌一个（同 .sidebar-release-btn 的理由）。
                   `@click.stop` 是必须的：点文件夹不该顺手把主 store 换掉。 -->
              <span
                class="skill-reveal"
                role="button"
                tabindex="-1"
                v-tooltip="t('list.action.reveal')"
                :aria-label="t('list.action.reveal')"
                @click.stop="reveal(s.path)"
                @keydown.enter.stop.prevent="reveal(s.path)"
                @keydown.space.stop.prevent="reveal(s.path)"
              >
                <IconFolder />
              </span>
              <!-- 只有自己加的才给撤下来的口子；内置候选是本机客观存在的目录，
                   藏起来只会让人以为它没了。 -->
              <span
                v-if="extraStores.includes(s.path)"
                class="skill-reveal"
                role="button"
                tabindex="-1"
                v-tooltip="t('tools.skills.storeForget')"
                :aria-label="t('tools.skills.storeForget')"
                @click.stop="dropMainStore(s.path)"
                @keydown.enter.stop.prevent="dropMainStore(s.path)"
                @keydown.space.stop.prevent="dropMainStore(s.path)"
              >
                <IconClose />
              </span>
            </button>

            <button
              type="button"
              class="set-dropdown-item skill-store-item skill-store-add"
              role="menuitem"
              @click.stop="addMainStore"
            >
              <span class="set-dropdown-check"><IconPlus /></span>
              <span>{{ t('tools.skills.storeAdd') }}</span>
            </button>
          </div>
        </div>
      </div>
      <!-- 数字不是装饰：它是「这个按钮吃的是筛完的列表」唯一看得见的证据。筛选一变
           它跟着变，否则用户没法在点之前确认自己筛对了没有。 -->
      <button
        type="button"
        class="skill-head-btn"
        :class="{ running: pending === 'adoptAll' }"
        :disabled="busy || !main || adoptAllCount === 0"
        v-tooltip="t('tools.skills.action.adoptAllTip', {
          n: String(adoptAllCount),
          k: String(adoptAllSkills),
        })"
        @click="adoptEverything"
      >
        <span v-if="pending === 'adoptAll'" class="chip-spinner" aria-hidden="true" />
        {{ t('tools.skills.action.adoptAll') }}
        <b>{{ adoptAllCount }}</b>
      </button>
    </template>
    <button
      type="button"
      class="skill-head-btn icon"
      :disabled="loading"
      v-tooltip="t('tools.skills.refresh')"
      @click="load"
    >
      <IconRefresh />
    </button>
  </div>

  <div class="tools-body">
    <!-- 列表 -->
    <section
      ref="listEl"
      class="tools-list"
      :aria-label="t('tools.listPane')"
      @scroll="markScrolling"
      @mouseover="onListMouseOver"
      @mouseleave="onListMouseLeave"
    >
      <!-- 次序摆在列表列自己头上，不摆进健康条：健康条横跨列表和详情两栏，而排序
           只管左边这一列；混在那一排角标里还会被当成又一个筛子。跟着列表一起滚会
           在最需要它的时候（翻到一半想换个看法）滚没了，所以 sticky 钉住。 -->
      <div v-if="summary" class="skill-sort" role="group" :aria-label="t('tools.skills.sortLabel')">
        <button
          v-for="s in SKILL_SORTS"
          :key="s"
          type="button"
          class="tools-chip"
          :class="{ active: skillSort === s }"
          @click="changeSort(s)"
        >{{ t(`tools.skills.sort.${s}`) }}</button>
      </div>

      <p v-if="error" class="tools-placeholder error">{{ error }}</p>
      <div
        v-else-if="loading && !scan"
        class="skill-skel-list"
        role="status"
        :aria-label="t('tools.skills.loading')"
      >
        <div v-for="i in SKEL_COUNT" :key="i" class="skill-skel-row">
          <span class="skill-skel-line">
            <span class="skill-skel-bar" :style="{ width: skel(i).name }" />
            <span class="skill-skel-bar chip" :style="{ width: skel(i).chip }" />
          </span>
          <span class="skill-skel-bar desc" :style="{ width: skel(i).desc }" />
        </div>
      </div>
      <p v-else-if="list.length === 0" class="tools-placeholder">{{ t('tools.skills.empty') }}</p>
      <!-- 浮块和行共用这一层定位参考系：offsetTop 是相对最近的定位祖先算的。 -->
      <div v-else class="tools-list-inner">
        <div ref="spotlightEl" class="list-spotlight" aria-hidden="true" />
        <button
          v-for="s in list"
          :key="s.name"
          type="button"
          class="skill-row"
          :class="{ active: s.name === selectedName, pinned: isSkillPinned(s.name) }"
          @click="select(s.name)"
        >
          <span class="skill-row-top">
            <!-- 名字和来源图标绑在一起：图标要**紧跟名字**，名字长到要省略号时
                 也不能被一起裁掉，所以省略发生在里层，图标在外层 flex 里不收缩。 -->
            <span class="skill-row-title">
              <!-- 记号在名字**前面**：这一行右边已经排了 agent 图标 + 角标 + 风险三组，
                   再往那头加只会让名字那半边更挤；而「这是项目里的还是我全局装的」
                   是读名字之前就要知道的事。 -->
              <span
                v-for="sc in scopesOf(s)"
                :key="sc"
                class="skill-row-scope"
                :class="sc"
                v-tooltip="t(`tools.scope.${sc}`)"
              >
                <component :is="SCOPE_ICONS[sc]" />
              </span>
              <span class="skill-row-name"><span
                v-for="(seg, i) in hl(s.name)"
                :key="i"
                :class="{ 'kw-hit': seg.hit }"
              >{{ seg.text }}</span></span>
              <span
                v-if="s.git"
                class="skill-row-git"
                v-tooltip="t('tools.skills.fromGitTip', { remote: s.git.remote })"
              >
                <IconGithub />
              </span>
            </span>
            <!-- 状态全部靠右：名字长短不一，跟在名字后面的角标会在列表中间排成一条锯齿。 -->
            <span class="skill-row-meta">
              <component
                :is="agentIcons[a]"
                v-for="a in agentsOf(s)"
                :key="a"
                class="skill-agent-dot"
              />
              <span v-if="badgeOf(s)" class="skill-badge" :class="badgeOf(s)!">
                {{ t(`tools.skills.badge.${badgeOf(s)}`) }}
              </span>
              <!-- 风险不给药丸底：一行里两个药丸会糊成一坨，而角标（结构坏了）比风险更急。 -->
              <span v-if="s.risk !== 'none'" class="skill-row-risk" :class="s.risk">
                {{ t(`tools.skills.risk.${s.risk}`) }}{{ riskIsConclusive(s) ? '' : '+' }}
              </span>
            </span>
          </span>
          <span class="skill-row-desc"><span
            v-for="(seg, i) in hl(rowSubtitle(s))"
            :key="i"
            :class="{ 'kw-hit': seg.hit }"
          >{{ seg.text }}</span></span>
          <!-- 置顶落在描述行右端而不是跟角标挤在顶行：顶行已经有「N 家 agent + 角标
               + 风险」三组东西，再插一颗按钮，名字那半边就只剩几个字。
               行本身是 <button>，这里不能再嵌一个（同 .sidebar-release-btn 的理由）。 -->
          <span
            class="skill-pin"
            :class="{ on: isSkillPinned(s.name) }"
            role="button"
            tabindex="-1"
            v-tooltip="isSkillPinned(s.name) ? t('list.action.unpin') : t('list.action.pin')"
            :aria-label="isSkillPinned(s.name) ? t('list.action.unpin') : t('list.action.pin')"
            :aria-pressed="isSkillPinned(s.name)"
            @click.stop="toggleSkillPin(s.name)"
            @keydown.enter.stop.prevent="toggleSkillPin(s.name)"
            @keydown.space.stop.prevent="toggleSkillPin(s.name)"
          >
            <IconPinUp />
          </span>
        </button>
      </div>
    </section>

    <!-- 列表 / 详情之间的拖拽条。宽度是四个面板共用的一份状态，住在 toolsPanel.ts。 -->
    <div
      class="tools-resizer"
      role="separator"
      aria-orientation="vertical"
      @pointerdown="startToolsListResize"
    />

    <!-- 详情 -->
    <section class="tools-detail" :aria-label="t('tools.detailPane')">
      <p v-if="!selected" class="tools-placeholder">{{ t('tools.noSelection') }}</p>
      <template v-else>
        <header class="skill-detail-head">
          <h3>{{ selected.name }}</h3>
          <!-- 挨着名字放：夹在按钮堆里根本注意不到，而它改的是「删除」的含义。 -->
          <label
            class="skill-keep-bodies"
            :class="{ on: keepBodies }"
            v-tooltip="t('tools.skills.keepBodiesTip')"
          >
            <input
              type="checkbox"
              :checked="keepBodies"
              @change="askKeepBodies(($event.target as HTMLInputElement).checked)"
            />
            <span class="skill-keep-box" aria-hidden="true"><IconCheck /></span>
            <span>{{ t('tools.skills.keepBodies') }}</span>
          </label>
          <div class="skill-detail-actions">
            <button
              type="button"
              class="skill-head-btn"
              :class="{ running: pending === 'adopt' }"
              :disabled="busy || !main || adoptTargets(selected, main).length === 0"
              v-tooltip="t('tools.skills.action.adoptTip')"
              @click="adoptSelected"
            >
              <span v-if="pending === 'adopt'" class="chip-spinner" aria-hidden="true" />
              <IconWrench v-else />
              {{ t('tools.skills.action.adopt') }}
            </button>
            <button
              type="button"
              class="skill-head-btn"
              :class="{ running: pending === 'repair' }"
              :disabled="busy || repairPlan(selected, repairTarget(selected, main)).length === 0"
              v-tooltip="t('tools.skills.action.repairTip')"
              @click="repairSelected"
            >
              <span v-if="pending === 'repair'" class="chip-spinner" aria-hidden="true" />
              <IconLink v-else />
              {{ t('tools.skills.action.repair') }}
            </button>
            <!-- 只有主 body 是个带 remote 的 clone 才出现。手写的 skill、
                 没有 remote 的本地仓库都没有「最新版本」可言。 -->
            <button
              v-if="gitInfo"
              type="button"
              class="skill-head-btn"
              :class="{ running: pending === 'update' }"
              :disabled="busy"
              v-tooltip="t('tools.skills.action.updateTip', { remote: gitInfo.remote })"
              @click="checkUpdate"
            >
              <span v-if="pending === 'update'" class="chip-spinner" aria-hidden="true" />
              <IconDownload v-else />
              {{ t('tools.skills.action.update') }}
            </button>
            <!-- 开在实体目录上；没有实体内容（全是死链）时没得可编辑。 -->
            <button
              type="button"
              class="skill-head-btn"
              :disabled="busy || !editBody"
              v-tooltip="t('tools.skills.action.editTip')"
              @click="editing = true"
            >
              <IconPencil />
              {{ t('tools.skills.action.edit') }}
            </button>
            <button
              type="button"
              class="skill-head-btn danger"
              :class="{ running: pending === 'delete' }"
              :disabled="busy"
              @click="deleteSelected"
            >
              <span v-if="pending === 'delete'" class="chip-spinner" aria-hidden="true" />
              <IconTrash v-else />
              {{ t('tools.skills.action.delete') }}
            </button>
          </div>
        </header>

        <p v-if="selected.description" class="skill-desc">{{ selected.description }}</p>
        <p v-if="detailError" class="tools-placeholder error">{{ detailError }}</p>

        <!-- 在哪几家里启用 -->
        <div class="skill-section">
          <h4>{{ t('tools.skills.enabledIn') }}</h4>
          <div class="skill-agent-row">
            <button
              v-for="a in shownAgents"
              :key="a"
              type="button"
              class="skill-agent-toggle"
              :class="{ on: agentEnabled(selected, a), running: pending === `toggle:${a}` }"
              :disabled="busy || !supportsSkills(a)"
              :aria-pressed="agentEnabled(selected, a)"
              v-tooltip="supportsSkills(a)
                ? agentToggleTip(selected, a)
                : t('tools.skills.noSkillsSupport', { agent: agentLabel(a, true) })"
              @click="toggleAgent(a, !agentEnabled(selected, a))"
            >
              <span v-if="pending === `toggle:${a}`" class="chip-spinner" aria-hidden="true" />
              <component :is="agentIcons[a]" v-else />
            </button>

            <!-- 公共目录。它不是一家 agent，但 codex / grok / kimi / opencode / pi 都读它 ——
                 开一条链接顶三条，而且主 store 挪到别处之后这儿不会自己有内容。 -->
            <template v-if="sharedDir && sharedDirAgents.length > 0">
              <span class="skill-agent-sep" aria-hidden="true" />
              <button
                type="button"
                class="skill-shared-toggle"
                :class="{ on: sharedOn(selected), running: pending === 'toggle:shared' }"
                :disabled="busy"
                :aria-pressed="sharedOn(selected)"
                v-tooltip="sharedTip(selected)"
                @click="toggleShared(!sharedOn(selected))"
              >
                <span v-if="pending === 'toggle:shared'" class="chip-spinner" aria-hidden="true" />
                <span
                  v-else
                  class="set-toggle-track"
                  :class="{ on: sharedOn(selected) }"
                  aria-hidden="true"
                >
                  <span class="set-toggle-thumb" />
                </span>
                <span class="skill-shared-path">{{ short(sharedDir) }}</span>
              </button>
            </template>
          </div>
        </div>

        <!-- 内容 -->
        <div class="skill-section">
          <h4>{{ t('tools.skills.bodies', { n: String(selected.bodies.length) }) }}</h4>
          <p v-if="selected.bodies.length === 0" class="skill-note">{{ t('tools.skills.noBody') }}</p>
          <div v-for="b in selected.bodies" :key="b.path" class="skill-body-row">
            <span class="skill-path">{{ short(b.path) }}</span>
            <span v-if="main && inMainStore(b.path, main)" class="skill-tag">{{ t('tools.skills.inMain') }}</span>
            <span class="skill-meta">
              {{ t('tools.skills.fileCount', { n: String(b.files) }) }} · {{ formatSize(b.bytes) }}
              <template v-if="b.modified"> · {{ fmtTime(b.modified) }}</template>
              <template v-if="b.truncated"> · {{ t('tools.skills.truncated') }}</template>
            </span>
            <button
              type="button"
              class="skill-reveal"
              v-tooltip="t('list.action.reveal')"
              :aria-label="t('list.action.reveal')"
              @click="reveal(b.path)"
            >
              <IconFolder />
            </button>
            <!-- 每一份都能单独删。同一个 skill 常常一份在全局、一份在某个项目里
                 （`~/.skills-manager/skills/X` 和 `~/apps/proj/.agents/skills/X`），
                 而右上角那个「删除」是连同所有内容和入口一起清掉 —— 想只退掉全局那份、
                 留着项目里那份，原来一个入口都没有。 -->
            <button
              type="button"
              class="skill-reveal danger"
              :disabled="busy"
              v-tooltip="t('tools.skills.action.deleteBodyTip')"
              :aria-label="t('tools.skills.action.deleteBody')"
              @click="deleteBody(b.path)"
            >
              <span v-if="pending === `delBody:${b.path}`" class="chip-spinner" aria-hidden="true" />
              <IconTrash v-else />
            </button>
          </div>
        </div>

        <!-- 引用链路 -->
        <div class="skill-section">
          <h4>{{ t('tools.skills.refs', { n: String(selected.refs.length) }) }}</h4>
          <div v-for="r in selected.refs" :key="r.path" class="skill-ref">
            <div class="skill-ref-head">
              <span class="skill-health" :class="r.health.state" v-tooltip="refTip(r)">
                {{ t(`tools.skills.state.${r.health.state}`) }}
              </span>
              <!-- 画的是 `reachedBy` 不是 `agents`：实体目录那几行没有任何一家**直接**
                   扫它，可它常常是别人链条的终点。按 `agents` 画的话，一份两条链都
                   落在上面的内容会被标成「没有 agent 读它」，旁边还配一个删除按钮。 -->
              <component
                :is="agentIcons[a]"
                v-for="a in r.reachedBy"
                :key="a"
                class="skill-agent-dot"
              />
              <span v-if="r.reachedBy.length === 0" class="skill-tag">
                {{ t('tools.skills.thirdParty') }}
              </span>

              <!-- 副本过期：源改了，这家 agent 读到的还是旧的。一键重拷。 -->
              <button
                v-if="r.health.state === 'copyStale'"
                type="button"
                class="skill-sync"
                :disabled="busy"
                v-tooltip="t('tools.skills.action.resyncTip', { source: short(r.health.detail) })"
                @click="resyncCopy(r.path)"
              >
                <span v-if="pending === `resync:${r.path}`" class="chip-spinner" aria-hidden="true" />
                <IconRefresh v-else />
                {{ t('tools.skills.action.resync') }}
              </button>
              <!-- 副本被就地改过 / 两边都改了：**不给**一键。重拷会把用户在副本上写的
                   东西抹掉，而哪一边该留下只有他自己知道。同 Hooks 面板那条规矩：宁可
                   不给按钮，也不给一个会吃掉数据的按钮。 -->
              <span
                v-else-if="r.health.state === 'copyEdited' || r.health.state === 'copyDiverged'"
                class="skill-note"
              >{{ t('tools.skills.copyManual') }}</span>
            </div>
            <div
              v-for="(line, i) in chainLines(r, home)"
              :key="i"
              class="skill-chain-line"
              :class="{ missing: !line.exists }"
              :style="{ paddingLeft: 10 + line.depth * 14 + 'px' }"
            >
              <span v-if="line.depth > 0" class="skill-chain-arrow">└─</span>
              {{ line.path }}
              <span v-if="!line.exists" class="skill-chain-missing">{{ t('tools.skills.missing') }}</span>
              <!-- 图标紧跟在它描述的那条路径后面，不推到行尾 —— 见 .skill-reveal 的注释。
                   路径不存在就不给「打开文件夹」：后端会退到最近一个存在的祖先，点下去
                   打开的是别的目录，比不给还糟。 -->
              <button
                v-if="line.exists"
                type="button"
                class="skill-reveal skill-chain-btn"
                v-tooltip="t('list.action.reveal')"
                :aria-label="t('list.action.reveal')"
                @click="reveal(line.abs)"
              >
                <IconFolder />
              </button>
              <!-- 没人读的活链接：除了拆掉它没有别的出路。「停用」是按 agent 关的，
                   碰不到第三方 store 里这一条；「清理死链」只碰解析不到东西的。 -->
              <button
                v-if="line.depth === 0 && removableLink(r)"
                type="button"
                class="skill-reveal skill-chain-btn danger"
                :disabled="busy"
                v-tooltip="t('tools.skills.action.unlinkRefTip')"
                :aria-label="t('tools.skills.action.unlinkRef')"
                @click="unlinkRef(line.abs)"
              >
                <span v-if="pending === `unlink:${line.abs}`" class="chip-spinner" aria-hidden="true" />
                <IconTrash v-else />
              </button>
            </div>
          </div>
        </div>

        <!-- 风险明细。和发现面板共用一个组件：装之前看到的和装完看到的必须一样。 -->
        <SkillFindings
          v-if="detail"
          :findings="detail.findings"
          :truncated="detail.truncated"
          :heading="t('tools.skills.findings', { n: String(detail.findings.length) })"
        />

        <!-- frontmatter -->
        <div v-if="detail?.frontmatter" class="skill-section">
          <h4>{{ t('tools.skills.frontmatter') }}</h4>
          <div v-if="detail.frontmatter.name" class="skill-fm-row">
            <span class="skill-fm-key">name</span><span>{{ detail.frontmatter.name }}</span>
          </div>
          <div v-if="detail.frontmatter.description" class="skill-fm-row">
            <span class="skill-fm-key">description</span><span>{{ detail.frontmatter.description }}</span>
          </div>
          <div v-if="detail.frontmatter.allowedTools.length" class="skill-fm-row">
            <span class="skill-fm-key">allowed-tools</span>
            <span>{{ detail.frontmatter.allowedTools.join(', ') }}</span>
          </div>
          <div v-for="f in detail.frontmatter.extra" :key="f.key" class="skill-fm-row">
            <span class="skill-fm-key">{{ f.key }}</span><span>{{ f.value }}</span>
          </div>
        </div>

        <!-- 详情还在读。骨架照着下面三节的真实版式摆：标题 + 几行，让右半边先占住
             位置。只在**还没有内容**的时候出现 —— 刷新时旧内容留着原地换掉，
             把已经看得见的东西换成骨架比空着还难受。 -->
        <SkillDetailSkeleton v-if="detailLoading && !detail && !detailError" />

        <!-- 文件清单 -->
        <div v-if="detail && detail.files.length > 0" class="skill-section">
          <h4>{{ t('tools.skills.files', { n: String(detail.files.length) }) }}</h4>
          <!-- 和 Git 改动视图同一棵树，只是默认折起来（见 fileDirOpen 的注释）。 -->
          <div
            v-for="node in flatFileNodes"
            :key="node.path"
            class="skill-file-row"
            :class="{ dir: node.children.length > 0 }"
            :style="{ paddingLeft: treeDepth(node.path) * 14 + 'px' }"
            @click="node.children.length && toggleFileDir(node.path)"
          >
            <!-- 每一行都留出折叠三角那一格（文件行是个空位），不然同一层的文件名和
                 目录名会差开一个图标的宽度，看上去像缩进错了。 -->
            <IconChevronRight
              v-if="node.children.length"
              class="skill-file-arrow"
              :class="{ open: fileDirOpen(node.path) }"
              aria-hidden="true"
            />
            <span v-else class="skill-file-arrow" aria-hidden="true" />
            <component
              :is="node.children.length ? IconFolder : fileIconFor(node.name)"
              class="skill-file-ic"
            />
            <span class="skill-path">{{ node.name }}</span>
            <span v-if="node.item" class="skill-meta">{{ formatSize(node.item.bytes) }}</span>
            <span v-else class="skill-meta">{{ t('tools.skills.dirCount', { n: String(countFiles(node)) }) }}</span>
          </div>
        </div>
      </template>
    </section>
  </div>

  <ConfirmModal
    :show="keepBodiesAsk !== null"
    :title="t(keepBodiesAsk ? 'tools.skills.keepAsk.onTitle' : 'tools.skills.keepAsk.offTitle')"
    :message="t(keepBodiesAsk ? 'tools.skills.keepAsk.onMsg' : 'tools.skills.keepAsk.offMsg')"
    :ok-text="t('tools.skills.keepAsk.ok')"
    :danger="keepBodiesAsk === false"
    @confirm="confirmKeepBodies"
    @cancel="cancelKeepBodies"
  />

  <!-- 强制更新。`dismissable: false` —— 点遮罩关掉就等于「已阅」，而这是唯一一次
       提醒「你改过的文件会被覆盖」。 -->
  <ConfirmModal
    :show="!!updateCheck"
    :title="t('tools.skills.update.title')"
    :message="updateMessage"
    :ok-text="t('tools.skills.update.ok')"
    :danger="(updateCheck?.changed.length ?? 0) > 0"
    :dismissable="false"
    @confirm="applyUpdate"
    @cancel="updateCheck = null"
  />

  <SkillPlanModal
    :show="!!plan"
    :title="plan?.title ?? ''"
    :report="plan?.report ?? null"
    :home="home"
    :busy="busy"
    @confirm="applyPlan"
    @cancel="plan = null"
  />

  <SkillConflictModal
    :show="!!conflicts && !conflictDone(conflicts)"
    :conflict="conflicts ? currentConflict(conflicts) : null"
    :remaining="conflicts ? conflicts.queue.length - conflicts.index : 0"
    :home="home"
    @choose="onConflictChoice"
    @skip="onConflictSkip"
    @skip-all="onConflictSkipAll"
    @cancel="closeConflicts"
  />

  <!-- 纯告知：解释为什么刚才那下没反应。没有可撤销的动作，所以只有一个「我知道了」。 -->
  <ConfirmModal
    :show="reachNote !== null"
    :title="reachNote?.title ?? ''"
    :message="reachNote?.body ?? ''"
    :ok-text="t('common.gotIt')"
    :danger="false"
    acknowledge
    @confirm="reachNote = null"
    @cancel="reachNote = null"
  />

  <SkillEditor
    v-if="editBody && selectedName"
    :show="editing"
    :name="selectedName"
    :body="editBody"
    @close="editing = false"
    @saved="onEditorSaved"
  />
</template>

<style scoped>
.skill-head-gap {
  flex: 1;
}
/* 角标的滑动区：吃掉健康条剩下的宽度，装不下就横着滑（实现见 `hstrip.ts`）。
   `min-width: 0` 不能省 —— flex 子项默认 `min-width: auto`，六个不换行的药丸会把
   容器顶到内容那么宽，于是「吃掉剩余宽度」变成「把右边挤出去」，等于没改。 */
.skill-badge-scroll {
  position: relative;
  display: flex;
  align-items: center;
  flex: 1;
  min-width: 0;
  overflow: hidden;
  /* 被切掉的那半个药丸淡出，顺带就是「这边还有」的提示。两侧的宽度由 JS 按
     「还能不能往这边滑」给，滑到头就收回 0，免得凭空吃掉一个药丸的可读性。 */
  -webkit-mask-image: linear-gradient(
    to right,
    transparent 0,
    #000 var(--badge-fade-l, 0px),
    #000 calc(100% - var(--badge-fade-r, 0px)),
    transparent 100%
  );
  mask-image: linear-gradient(
    to right,
    transparent 0,
    #000 var(--badge-fade-l, 0px),
    #000 calc(100% - var(--badge-fade-r, 0px)),
    transparent 100%
  );
}
.skill-badge-track {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 0 0 auto;
  transform: translateX(0);
  transition: transform 0.28s cubic-bezier(0.22, 0.61, 0.36, 1);
  will-change: transform;
}
/* 滚轮 / 拖拽进行中：关掉动画，1:1 跟手 */
.skill-badge-track.panning {
  transition: none;
}
.skill-badge-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 9px;
  border-radius: 999px;
  border: 1px solid var(--border);
  color: var(--text-dim);
  font-size: 11.5px;
  white-space: nowrap;
  transition: background 0.12s, color 0.12s, border-color 0.12s, opacity 0.12s;
}
.skill-badge-chip b {
  font-variant-numeric: tabular-nums;
  color: var(--text);
}
.skill-badge-chip:hover {
  background: var(--surface-hover);
}
.skill-badge-chip.active {
  border-color: var(--accent);
  color: var(--text);
}
/* 数是 0 的那类压暗，但**不隐藏** —— 「断链 0」本身就是要让人看到的结论。 */
.skill-badge-chip.zero {
  opacity: 0.45;
}
.skill-badge-ic {
  width: 12px;
  height: 12px;
  opacity: 0.8;
}
/* 档次和左边那排「毛病」隔开一条竖线（同 `.list-head-branch-divider` 那一套）。 */
.skill-head-divider {
  width: 1px;
  height: 16px;
  margin: 0 2px;
  background: var(--border);
  flex-shrink: 0;
}
/* 选中态借行里那两个记号的颜色 —— 药丸和行前的小图标说的是同一件事，
   颜色对不上就得靠读字才知道哪个筛子对应哪个记号。 */
.skill-badge-chip.scope.user.active {
  border-color: var(--scope-user);
  color: var(--text);
}
.skill-badge-chip.scope.project.active {
  border-color: var(--scope-project);
  color: var(--text);
}
.skill-badge-chip.scope.active .skill-badge-ic {
  opacity: 1;
}
.skill-badge-chip.scope.user.active .skill-badge-ic {
  color: var(--scope-user);
}
.skill-badge-chip.scope.project.active .skill-badge-ic {
  color: var(--scope-project);
}

.skill-main-store {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--text-mute);
  white-space: nowrap;
}
/* 按钮上只留路径，「N 个实体」挪进菜单右侧 —— 健康条这一行本来就挤，
   收起来的状态没必要把数字也顶在上面。 */
.skill-store-btn {
  max-width: 260px;
}
.skill-store-cur {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.skill-store-menu {
  min-width: 268px;
  max-width: 420px;
}
.skill-store-title {
  margin: 2px 4px 5px;
  padding-bottom: 5px;
  border-bottom: 1px solid var(--border);
  font-size: 11.5px;
  color: var(--text-mute);
}
.skill-store-item {
  gap: 8px;
}
/* 「新增」是动作不是候选，和上面那几条路径隔开。 */
.skill-store-add {
  margin-top: 4px;
  padding-top: 7px;
  border-top: 1px solid var(--border);
  border-radius: 0 0 6px 6px;
  color: var(--text-dim);
}
.skill-store-add :deep(svg) {
  width: 13px;
  height: 13px;
}
.skill-store-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11.5px;
}
.skill-store-count {
  margin-left: auto;
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-mute);
}
/* 「在文件管理器中显示」。详情页的内容行和主 store 下拉共用一套 —— 两处都是
   「这条路径在磁盘上」，长相不该有两个说法。 */
.skill-reveal {
  width: 22px;
  height: 22px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 5px;
  color: var(--text-mute);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.skill-reveal:hover:not(:disabled) {
  background: var(--surface-hover);
  color: var(--text);
}
/* 删除那一个悬停变红 —— 和链路里那些垃圾桶（`.skill-chain-btn.danger`）同一套记号，
   不然「打开目录」和「删掉目录」在同一行上长得一模一样。 */
.skill-reveal.danger:hover:not(:disabled) {
  background: var(--danger-soft);
  color: var(--danger);
}
.skill-reveal:disabled {
  opacity: 0.45;
  cursor: default;
}
.skill-reveal :deep(svg) {
  width: 13px;
  height: 13px;
}
/* 内容那一列的行是 `align-items: baseline`（为了让路径和后面那串小字对齐基线），
   按钮跟着基线走会偏高，单独拉回居中。**不给 `margin-left: auto`**：贴到行尾的话
   图标离它描述的那条路径隔了半屏空白，点之前还得确认一下自己点的是哪一条。 */
.skill-body-row .skill-reveal {
  align-self: center;
}
.skill-head-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 4px 10px;
  border-radius: 7px;
  border: 1px solid var(--border);
  color: var(--text-dim);
  font-size: 12px;
  white-space: nowrap;
  transition: background 0.12s, color 0.12s, opacity 0.12s;
}
.skill-head-btn:hover:not(:disabled) {
  background: var(--surface-hover);
  color: var(--text);
}
.skill-head-btn:disabled {
  opacity: 0.4;
  cursor: default;
}
/* 按钮上那个数：和角标里的 `<b>` 同一套 —— 数字用亮色、等宽，变动的时候按钮不抖。 */
.skill-head-btn b {
  font-variant-numeric: tabular-nums;
  color: var(--text);
}
.skill-head-btn:disabled b {
  color: inherit;
}
.skill-head-btn.icon {
  padding: 4px 7px;
}
.skill-head-btn.danger {
  color: var(--danger);
  border-color: color-mix(in srgb, var(--danger) 35%, var(--border));
}
.skill-head-btn.danger:hover:not(:disabled) {
  background: var(--danger-soft);
}
.skill-head-btn :deep(svg) {
  width: 13px;
  height: 13px;
}

.skill-row {
  /* 抬到浮块之上，否则 hover 高亮会盖住文字。 */
  position: relative;
  z-index: 1;
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 11px 12px;
  border: 1px solid transparent;
  border-radius: 9px;
  text-align: left;
  transition: background 0.12s;
}
/* 不给 :hover 底色 —— hover 归 `.list-spotlight` 那块跟随鼠标的浮块管，
   两者都画就是一行里两层高亮。 */
.skill-row.active {
  background: var(--surface-hover);
  border-color: var(--border);
}
.skill-sort {
  display: flex;
  align-items: center;
  gap: 4px;
  /* `.tools-list` 自己有 6px 8px 的内边距，这儿只补下边距，左右和行对齐。 */
  padding: 2px 0 8px;
  /* 钉住时底下会滚过一整列行，不给底色的话文字会叠在一起。负 margin + 同宽 padding
     让底色铺满整列，包括 `.tools-list` 那 8px 的左右内边距。 */
  position: sticky;
  top: -6px;
  z-index: 2;
  margin: -6px -8px 0;
  padding-inline: 8px;
  padding-top: 8px;
  background: var(--surface);
}
/* 置顶行左侧一条 brand 竖条 —— 和会话卡片的 `.sess-pinned` 同一套记号。 */
.skill-row.pinned::before {
  content: '';
  position: absolute;
  left: 0;
  top: 28%;
  height: 44%;
  width: 2px;
  border-radius: 1px;
  background: var(--brand);
}
/* 置顶按钮：**绝对定位，不进文档流**。
 *
 * 这一条是量出来的，不是设计偏好。它在流里的时候（不管是 `display:none` 还是透明
 * 占位）会同时改两个量：20px 高的按钮比描述那行的文字行盒高，一出现就把整行从
 * 64px 撑到 68px —— 下面每一行跟着往下挪，肉眼就是「hover 划过去时列表在抖」；
 * 同时描述宽度掉 28px，末尾省略号也跟着跳。移出流之后两个量都不动，而且照样不
 * 预留位置。
 *
 * 代价是描述很长时它会压在末尾的省略号上 —— 那几个字本来就是被截掉的。
 *
 * `bottom: 9px` = 行的 11px 下内边距减去 (20px 按钮 − 16px 行盒)/2，让它和描述文字
 * 的视觉中线对齐。 */
.skill-pin {
  position: absolute;
  right: 10px;
  bottom: 9px;
  width: 20px;
  height: 20px;
  display: none;
  align-items: center;
  justify-content: center;
  border-radius: 5px;
  color: var(--text-mute);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.skill-row:hover .skill-pin,
.skill-pin.on {
  display: inline-flex;
}
.skill-pin:hover {
  background: var(--surface-hover);
  color: var(--text);
}
.skill-pin.on {
  color: var(--brand);
  background: var(--brand-soft, var(--surface-hover));
}
.skill-pin :deep(svg) {
  width: 13px;
  height: 13px;
}
.skill-row-top {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.skill-row-title {
  flex: 1;
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.skill-row-name {
  min-width: 0;
  font-size: 13px;
  font-weight: 500;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 档次记号。
 *
 * `flex-shrink: 0` 是必须的：名字长到要省略号时，收缩的必须是 `.skill-row-name`
 * 里层那段文字，而不是把这个 12px 的图标压成一条线。
 *
 * 不给它加底色药丸 —— 一行里已经有角标和风险两种药丸了，第三种会把名字挤到没边。 */
.skill-row-scope {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
}
.skill-row-scope :deep(svg) {
  width: 12px;
  height: 12px;
}
.skill-row-scope.user {
  color: var(--scope-user);
}
.skill-row-scope.project {
  color: var(--scope-project);
}
/* 压暗：它是出身，不是状态。和右边那排角标抢注意力就本末倒置了。 */
.skill-row-git {
  display: inline-flex;
  flex-shrink: 0;
  color: var(--text-mute);
}
.skill-row-git :deep(svg) {
  width: 12px;
  height: 12px;
}
.skill-row.active .skill-row-git {
  color: var(--text-dim);
}
/* 状态贴右边缘，名字占满剩下的宽度 —— 角标就排成一条直列，扫一眼能数清坏了几条。 */
.skill-row-meta {
  display: flex;
  align-items: center;
  gap: 5px;
  flex-shrink: 0;
}
.skill-row-risk {
  flex-shrink: 0;
  font-size: 10.5px;
  font-weight: 500;
  color: var(--text-mute);
  white-space: nowrap;
}
.skill-row-risk.medium {
  color: var(--brand);
}
.skill-row-risk.high,
.skill-row-risk.critical {
  color: var(--danger);
}
.skill-row-desc {
  min-width: 0;
  font-size: 11.5px;
  color: var(--text-mute);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.skill-agent-dot {
  width: 12px;
  height: 12px;
  flex-shrink: 0;
  opacity: 0.7;
}

/* ---- 骨架 ----------------------------------------------------------------
   首屏扫的是全机器的 skill 目录，慢到肉眼可见。一行「扫描中…」既不占位也不
   预告版式，列表会从空白直接跳成满屏。骨架照着 `.skill-row` 的两行结构摆。 */
.skill-skel-list {
  height: 100%;
  padding: 2px 0;
  /* 铺满一屏是为了让骨架占住版面；多出来的裁掉，否则加载时先闪一根滚动条。 */
  overflow: hidden;
}
.skill-skel-row {
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 11px 12px;
}
.skill-skel-line {
  display: flex;
  align-items: center;
  gap: 8px;
}
.skill-skel-bar {
  height: 11px;
  border-radius: 4px;
  /* 不用 `--surface-hover`：那是给「悬停时比底色亮一点」用的，和底色差不到一档，
     摊在详情栏那六百像素宽的空白上几乎看不见。跟着 `--text-mute` 调，深浅两套主题
     都保证和底色分得开。 */
  background: color-mix(in srgb, var(--text-mute) 26%, transparent);
  animation: skill-skel-pulse 1.4s ease-in-out infinite;
}
.skill-skel-bar.chip {
  height: 13px;
  border-radius: 999px;
  margin-left: auto;
  opacity: 0.6;
}
.skill-skel-bar.desc {
  height: 9px;
}
/* 整齐同步地闪会看成一块面板在呼吸；错开之后才像一条条正在到位的记录。 */
.skill-skel-row:nth-child(2n) .skill-skel-bar {
  animation-delay: 0.12s;
}
.skill-skel-row:nth-child(3n) .skill-skel-bar {
  animation-delay: 0.24s;
}
.skill-skel-bar.desc {
  animation-delay: 0.18s;
}

@keyframes skill-skel-pulse {
  0%,
  100% {
    opacity: 0.35;
  }
  50% {
    opacity: 0.9;
  }
}
@media (prefers-reduced-motion: reduce) {
  .skill-skel-bar {
    animation: none;
  }
}

.skill-badge,
/* `.skill-risk` 的样式在 style.css（全局）—— 发现面板也要用同一个药丸。 */
.skill-tag,
.skill-health {
  flex-shrink: 0;
  padding: 1px 7px;
  border-radius: 999px;
  font-size: 10.5px;
  font-weight: 500;
  white-space: nowrap;
}
.skill-badge {
  background: var(--surface-2);
  color: var(--text-dim);
}
.skill-badge.broken,
.skill-badge.cyclic {
  background: var(--danger-soft);
  color: var(--danger);
}
/* 副本过期不是"坏了"，是"旧了" —— 内容还在，只是不是最新的。用 brand 而不是红色：
   和真断链并排时，同一个红会让人分不出哪条是"现在就用不了"。 */
.skill-badge.copyStale {
  background: color-mix(in srgb, var(--brand) 14%, transparent);
  color: var(--brand);
}
.skill-tag {
  background: var(--surface-2);
  color: var(--text-mute);
}
.skill-health {
  border: 1px solid var(--border);
  color: var(--text-dim);
  cursor: default;
}
.skill-health.broken,
.skill-health.cyclic,
.skill-health.notADirectory,
/* 源没了的副本是真的坏了：它是仅存的一份，再也同步不回来。 */
.skill-health.copyOrphaned {
  border-color: color-mix(in srgb, var(--danger) 40%, var(--border));
  color: var(--danger);
}
.skill-health.copyStale,
.skill-health.copyEdited,
.skill-health.copyDiverged {
  border-color: color-mix(in srgb, var(--brand) 40%, var(--border));
  color: var(--brand);
}

/* 「立即同步」。和旁边的 reveal 图标按钮不同，它带字 —— 这一步会重写一整个目录，
   一个只有图标的按钮说不清它要做什么。 */
.skill-sync {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  padding: 2px 8px;
  border-radius: 999px;
  border: 1px solid color-mix(in srgb, var(--brand) 40%, var(--border));
  background: transparent;
  color: var(--brand);
  font-size: 11px;
  cursor: pointer;
}
.skill-sync:hover:not(:disabled) {
  background: color-mix(in srgb, var(--brand) 12%, transparent);
}
.skill-sync:disabled {
  opacity: 0.5;
  cursor: default;
}
.skill-sync :deep(svg) {
  width: 11px;
  height: 11px;
}

.skill-detail-head {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.skill-detail-head h3 {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
}
.skill-detail-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-left: auto;
  flex-wrap: wrap;
}
.skill-keep-bodies {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 9px 2px 7px;
  border: 1px solid var(--border);
  border-radius: 999px;
  font-size: 11.5px;
  color: var(--text-mute);
  cursor: pointer;
  transition: background 0.12s, border-color 0.12s, color 0.12s;
}
/* 原生 checkbox 在 11.5px 这一档被 WebKit 画成一个小圆点 —— 看着像单选钮，勾没勾也
   分不出来。所以自己画一个：**方框**才读得出「开关」，勾上整块反色 + 一个勾，扫一眼
   就知道「删除」的含义已经变了。
   原生的那个不删掉、只压成 0 尺寸：label 的点击和键盘 tab / 空格都还是它在管。 */
.skill-keep-bodies input {
  appearance: none;
  -webkit-appearance: none;
  width: 0;
  height: 0;
  margin: 0;
  opacity: 0;
  flex-shrink: 0;
}
.skill-keep-box {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  /* 描边用 `--text-mute` 而不是 `--line-strong`：后者在部分主题里是 12% 不透明度的
     结构线（Dracula 实测 `rgb(248 248 242 / 12%)`），画在 14px 的小方框上几乎看不见，
     而这是个要用户**注意到**的开关。 */
  border: 1.5px solid var(--text-mute);
  border-radius: 4px;
  color: transparent;
  transition: background 0.12s, border-color 0.12s, color 0.12s;
}
.skill-keep-bodies:hover .skill-keep-box {
  border-color: var(--text);
}
/* 勾的颜色跟着反色按钮那一套走：light 黑底白勾 / dark 白底黑勾。 */
.skill-keep-bodies.on .skill-keep-box {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--bg);
}
.skill-keep-box :deep(svg) {
  width: 11px;
  height: 11px;
  stroke-width: 3.2;
}
/* 原生框藏起来了，键盘焦点环得自己画到方框上，否则 tab 过去完全没有反馈。 */
.skill-keep-bodies input:focus-visible + .skill-keep-box {
  box-shadow: 0 0 0 2px var(--surface), 0 0 0 3.5px var(--accent);
}
/* 勾上之后「删除」的含义就变了，这个状态得一眼看见，不能和没勾长得一样。 */
.skill-keep-bodies.on {
  border-color: var(--accent);
  background: var(--surface-active);
  color: var(--text);
}
.skill-desc {
  margin: 8px 0 0;
  font-size: 12.5px;
  color: var(--text-dim);
  line-height: 1.6;
}

.skill-section {
  margin-top: 18px;
}
.skill-section h4 {
  margin: 0 0 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-mute);
  display: flex;
  align-items: center;
  gap: 8px;
}
.skill-agent-row {
  display: flex;
  gap: 4px;
}
.skill-agent-toggle {
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 7px;
  border: 1px solid var(--border);
  opacity: 0.35;
  transition: opacity 0.12s, background 0.12s, border-color 0.12s;
}
.skill-agent-toggle.on {
  opacity: 1;
  border-color: var(--accent);
  background: var(--surface-active);
}
.skill-agent-toggle:disabled {
  opacity: 0.15;
  cursor: default;
}
/* 正在跑的那个例外：按钮这会儿是 disabled 的，但转圈要看得见才有意义。 */
.skill-agent-toggle.running:disabled,
.skill-head-btn.running:disabled {
  opacity: 1;
}
.skill-agent-sep {
  width: 1px;
  height: 16px;
  margin: 0 2px;
  background: var(--border);
}
/* agent 是图标，公共目录是路径 —— 它不是一家，长得不一样才不会被当成第八家。
   但也不能长成一句说明：带上设置里那套轨道 + 圆点，一眼看出是个能拨的开关。 */
.skill-shared-toggle {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 26px;
  padding: 0 9px 0 6px;
  border-radius: 7px;
  color: var(--text-mute);
  transition: background 0.12s, color 0.12s;
}
.skill-shared-toggle:hover:not(:disabled) {
  background: var(--surface-hover);
  color: var(--text-dim);
}
.skill-shared-toggle.on {
  color: var(--text);
}
.skill-shared-toggle:disabled {
  opacity: 0.5;
}
.skill-shared-toggle.running:disabled {
  opacity: 1;
}
.skill-shared-path {
  font-size: 11px;
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
}
/* 小一号的轨道：这排里 agent 图标才 15px，标准的 34×20 会把整行撑高一截。 */
.skill-shared-toggle .set-toggle-track {
  width: 26px;
  height: 15px;
  border-radius: 8px;
}
.skill-shared-toggle .set-toggle-thumb {
  width: 11px;
  height: 11px;
}
.skill-shared-toggle .set-toggle-track.on .set-toggle-thumb {
  transform: translateX(11px);
}

.skill-agent-toggle :deep(svg),
.skill-agent-toggle :deep(img) {
  width: 15px;
  height: 15px;
}

.skill-file-row.dir {
  cursor: pointer;
  color: var(--text-mute);
  font-weight: 500;
}
.skill-file-row.dir:hover {
  background: var(--surface-hover);
}
/* 以前这儿是个 `▸` 字符，11px 的字形在 12px 的格子里几乎看不见（而仓库的规矩是
   图标一律用 `icons.ts` 里的 inline SVG）。文件行复用同一个类当空位。 */
.skill-file-arrow {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  color: var(--text-mute);
  transition: transform 0.12s ease;
}
.skill-file-ic {
  flex-shrink: 0;
  width: 15px;
  height: 15px;
  color: var(--text-mute);
}
.skill-file-row.dir .skill-file-ic {
  color: var(--text-dim);
}
.skill-file-arrow.open {
  transform: rotate(90deg);
}

.skill-body-row,
.skill-file-row,
.skill-fm-row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 3px 0;
  font-size: 12px;
  min-width: 0;
}
/* 这一行里混着 SVG 和文字，`baseline` 会把图标吊在文字基线上（SVG 的基线是它的
   下边缘），看着像掉下去半格。另外两行是纯文字，保持 `baseline`。 */
.skill-file-row {
  align-items: center;
  gap: 6px;
}
.skill-fm-key {
  min-width: 110px;
  color: var(--text-mute);
  font-size: 11.5px;
}
.skill-path {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11.5px;
  word-break: break-all;
}
.skill-meta,
.skill-note {
  color: var(--text-mute);
  font-size: 11px;
  white-space: nowrap;
}
.skill-note {
  white-space: normal;
}

.skill-ref {
  margin-bottom: 10px;
}
.skill-ref-head {
  display: flex;
  align-items: center;
  gap: 6px;
}
.skill-chain-line {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11.5px;
  color: var(--text-dim);
  line-height: 1.8;
  word-break: break-all;
}
.skill-chain-line.missing {
  color: var(--danger);
}
.skill-chain-arrow {
  opacity: 0.5;
  margin-right: 4px;
}
.skill-chain-missing {
  margin-left: 6px;
  font-size: 10.5px;
}
/* 链路这几行是纯文本块（要让长路径 break-all 换行，不能改成 flex），所以按钮就跟着
   文字走 inline 流，`vertical-align` 把它拉回和这一行的字对齐。 */
.skill-chain-btn {
  width: 18px;
  height: 18px;
  margin-left: 2px;
  vertical-align: -4px;
}
.skill-chain-btn :deep(svg) {
  width: 12px;
  height: 12px;
}
.skill-chain-btn.danger:hover {
  background: var(--danger-soft);
  color: var(--danger);
}
.skill-chain-btn:disabled {
  opacity: 0.45;
  cursor: default;
}

</style>
