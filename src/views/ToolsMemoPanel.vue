<script setup lang="ts">
// 工具管理 · 全局配置面板。
//
// 判断全在 `src/toolsMemo.ts` —— `src/views/` 在 `vitest.config.ts` 是覆盖率排除的。
//
// 和另外三个面板的形状一样（健康条 + 主从两栏），但右栏不是只读详情，是**编辑器**：
// 这些文件本来就是让人写字的，看完还要跳出去开别的编辑器改，等于没做。编辑区直接
// 复用阶段 5 的 `CodeEditor.vue`。
//
// 三件这个面板独有、也最容易做错的事：
//
// 1. **改一份可能影响好几家。** opencode 和 grok 在自己的 `AGENTS.md` 缺席时都回退去读
//    `~/.claude/CLAUDE.md`。不把这条链路摆在编辑器上面，用户以为只在改 Claude。
// 2. **回退中的那一行点开的是实际生效的那份**，不是自己那个还不存在的空位置。要「自己
//    管」得显式接管，而且正文预填现在生效的内容 —— 从空文件开始存下去，那一刻这家就
//    丢了一整套指令。
// 3. **外部改动。** 这些文件用户随时在别的编辑器里改，而 `watch.rs` 是单会话 watcher
//    套不上来。所以打开面板和窗口重新聚焦时重扫一遍比指纹，保存时后端再比一次，对不上
//    拒绝盲写并给出「外面改了什么」的 diff。
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { revealSelected } from '../listScroll'
import { highlightSegments, renderText } from '../format'
import type { MemoDiff, MemoDoc, MemoRevision, MemoScan } from '../types'
import type { PlanView } from '../toolsPlan'
import * as api from '../api'
import { t } from '../i18n'
import { theme } from '../settings'
import { shortenPath } from '../toolsSkills'
import { selectFirstRow, shownOfTotal, startToolsListResize, toolsQuery } from '../toolsPanel'
import {
  allMemoRows,
  baseName,
  canSyncFork,
  editable,
  effectiveOf,
  externallyChanged,
  forkOf,
  forkOthers,
  formatBytes,
  importsOf,
  memoRows,
  otherReaders,
  takeoverPath,
  template,
  type MemoRow,
} from '../toolsMemo'
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
} from '../toolsMemoActions'
import { agentLabel } from '../agentMeta'
import { highlightAllCodeBlocks, langOfPath, rehighlightAllCodeBlocks } from '../shikiHighlight'
import {
  agentIcons,
  IconExternalLink,
  IconEye,
  IconFile,
  IconFilePlus,
  IconFolder,
  IconPencil,
  IconLink,
  IconRefresh,
  IconSave,
} from '../components/icons'
import CodeEditor from '../components/CodeEditor.vue'
import ConfirmModal from '../modals/ConfirmModal.vue'
import MemoDiffModal from '../modals/MemoDiffModal.vue'
import ToolsPlanModal from '../modals/ToolsPlanModal.vue'

const emit = defineEmits<{ (e: 'notify', msg: string, error?: boolean): void }>()

const scan = ref<MemoScan | null>(null)
const loading = ref(false)
const error = ref('')

const selectedKey = ref<string | null>(null)
const rows = computed(() => memoRows(scan.value, toolsQuery.value))
/**
 * 没过滤的全量。两处用它：
 *
 * 一是**选中那一行**按它找 —— 搜索把当前这行滤掉了不该把右边清空，那儿可能有还没保存
 * 的改动；`@` 引用跳过去的那个片段也一样，它未必落在当前的搜索结果里。（所以这里
 * 连 agent 勾选也不能带上：点掉一家不该让右边正在编辑的东西消失。）
 *
 * 二是**健康条上的分母**。见 `allMemoRows` 的注释 —— 这个面板的行是「位置」不是
 * 「文件」，后端那个 `summary.files` 当不了分母。
 */
const allRows = computed(() => allMemoRows(scan.value))
const selected = computed<MemoRow | null>(
  () => allRows.value.find((r) => r.key === selectedKey.value) ?? null,
)

const home = computed(() => scan.value?.home ?? '')
function short(path: string) {
  return shortenPath(path, home.value)
}

// ---------------------------------------------------------------------------
// 合并重复
// ---------------------------------------------------------------------------

/** 合并后内容的去处。用户没改过就是 `~/.agents/memo`。 */
const storePath = computed(() => memoStorePath(home.value))

/** 选中这一行所在的那组重复（同名 + 内容一模一样 + 磁盘上真有好几份）。 */
const dup = computed(() => dupOf(scan.value, openPath.value ?? selected.value?.path ?? null))
/** 同一组里除了当前这份以外的位置。 */
const dupElsewhere = computed(() => dupOthers(dup.value, openPath.value ?? selected.value?.path ?? null))
/** 全机器还能合的那几组 —— 健康条上「合并全部重复」按的就是它们。 */
const allDups = computed(() => mergeableDups(scan.value))

/** 当前这个位置是不是一条链接（合并之后就是）。 */
const linkTarget = computed(() => linkOf(openPath.value))

/** 列表那一行对应的位置是不是链接；是的话指向哪儿。 */
function linkOf(path: string | null): string | null {
  if (!scan.value || !path) return null
  return scan.value.files.find((f) => f.path === path)?.link ?? null
}
function isLinked(path: string | null): boolean {
  return linkOf(path) !== null
}

const plan = ref<PlanView | null>(null)
const planNames = ref<string[]>([])
const planBusy = ref(false)

/** 先 dry-run 拿计划给用户看。一步都还没做。 */
async function askMerge(names: string[]) {
  if (names.length === 0 || busy.value) return
  busy.value = true
  try {
    const report = await api.toolsMergeMemo(names, storePath.value, true)
    planNames.value = names
    plan.value = memoMergePlan(report, home.value, names)
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

/**
 * 用户点了确认。
 *
 * 合并之后当前打开的那个文件**变成了一条链接**，`rev` 里那份指纹说的还是合并前的
 * 实体文件 —— 不重新打开的话，下一次保存会被判成「外面改过了」。所以走完整的重扫 +
 * 重开，而不是只刷新列表。
 */
async function applyMerge() {
  if (!plan.value || planBusy.value) return
  planBusy.value = true
  try {
    const report = await api.toolsMergeMemo(planNames.value, storePath.value, false)
    const outcome = mergeOutcome(report)
    emit('notify', outcome.msg, outcome.error)
    plan.value = null
    await load()
    const path = openPath.value
    if (path) await openFile(path)
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    planBusy.value = false
  }
}

/** 换个去处。目录选完只记在 localStorage，不落盘到后端（见 `toolsMemoActions`）。 */
async function pickStore() {
  const { open } = await import('@tauri-apps/plugin-dialog')
  const picked = await open({ directory: true, multiple: false })
  const dir = typeof picked === 'string' ? picked : picked?.[0]
  if (dir) setMemoStore(dir)
}

// ---------------------------------------------------------------------------
// 打开的那个文件
// ---------------------------------------------------------------------------

/** 正在编辑哪个路径。和 `selected.path` 通常一样，接管时是 `selected.own`。 */
const openPath = ref<string | null>(null)
const text = ref('')
/** 上一次落盘（或打开时读到）的内容。`dirty` 就是拿它和 `text` 比。 */
const saved = ref('')
/** 打开时那份指纹。保存时原样回传，后端拿它判外部改动。 */
const rev = ref<MemoRevision | null>(null)
const fileError = ref('')
const busy = ref(false)
/** 「现在建」之后要把光标送进去 —— 按完按钮还得自己点一下编辑区，等于没建。 */
const editorEl = ref<InstanceType<typeof CodeEditor> | null>(null)

const dirty = computed(() => text.value !== saved.value)
const canEdit = computed(() => selected.value !== null && editable(selected.value))

/**
 * 右边默认是**读**。这几份文件绝大多数时候是拿来看的 —— 一上来就是编辑器，既容易误改，
 * 又把写好的 markdown 拍回一堆 `##` 和 `**`。要改按「编辑」。
 *
 * 只有 markdown 有预览这回事，别的后缀（真出现了）只能是编辑器，所以真正生效的是
 * `viewMode` 而不是 `mode`。
 */
const mode = ref<'read' | 'edit'>('read')
const isMarkdown = computed(() => /\.mdx?$/i.test(openPath.value ?? ''))
const viewMode = computed(() => (isMarkdown.value ? mode.value : 'edit'))
const previewEl = ref<HTMLElement | null>(null)

/* 预览里的围栏代码块交给 shiki，和聊天里那份是同一条路；换主题要重上色（颜色是烤进
   行内 style 的，不跟 CSS 变量走）。 */
watch(
  [() => viewMode.value, () => text.value, () => openPath.value],
  () => {
    if (viewMode.value !== 'read') return
    void nextTick(() => void highlightAllCodeBlocks(previewEl.value))
  },
  { immediate: true },
)
watch(theme, () => {
  if (viewMode.value === 'read') void rehighlightAllCodeBlocks(previewEl.value)
})

/** 现在编辑的是不是「给这家新建的一份」—— 决定要不要显示接管提示。 */
const takingOver = computed(
  () => selected.value !== null && openPath.value !== null && openPath.value === takeoverPath(selected.value),
)

async function openFile(path: string, prefill?: string) {
  busy.value = true
  fileError.value = ''
  try {
    const doc = await api.toolsReadMemo(path)
    openPath.value = path
    rev.value = doc.revision
    // 文件不存在时**不预填**：预填等于点一下列表就凭空造出一个未保存的改动，下一次
    // 切行还要被拦住问「要不要丢弃」—— 用户什么都没写，却要替他决定丢不丢。右边改成
    // 问一句「要现在建吗」，建不建他自己说。
    //
    // `prefill` 是显式动作带来的（「让这家接管」预填的是现在生效的那份内容），那种是
    // 用户已经开口要了，直接进编辑器。
    text.value = doc.revision.exists ? doc.text : (prefill ?? '')
    saved.value = doc.revision.exists ? doc.text : ''
    drafting.value = !doc.revision.exists && prefill !== undefined
    // 每换一个文件都回到「读」。唯一的例外是接管：那份正文是刚预填的，人就是来写的。
    mode.value = prefill === undefined ? 'read' : 'edit'
  } catch (e) {
    openPath.value = path
    rev.value = null
    text.value = ''
    saved.value = ''
    drafting.value = false
    fileError.value = String(e)
  } finally {
    busy.value = false
  }
}

/**
 * 这个位置还没有文件，而用户已经说了「现在建」——从这一刻起右边才是编辑器。
 *
 * 单独记一个状态而不是看 `dirty`：把模板全删光再接着写是正常操作，那一瞬间 `dirty`
 * 是 false，编辑器不能在手底下变回一张卡片。
 */
const drafting = ref(false)

function startDraft() {
  const path = openPath.value
  if (!path || busy.value) return
  text.value = template(path)
  drafting.value = true
  mode.value = 'edit'
  void nextTick(() => editorEl.value?.focus())
}

/** 右边现在该画编辑器，还是画「要不要建」那张卡片。 */
const showEditor = computed(() => rev.value?.exists === true || drafting.value)

/** 点了另一行 / 另一个片段。当前这个改了没存就先问一句。 */
const leaving = ref<{ key: string; path: string; prefill?: string } | null>(null)

function go(key: string, path: string, prefill?: string) {
  if (path === openPath.value && key === selectedKey.value) return
  if (dirty.value) {
    leaving.value = { key, path, prefill }
    return
  }
  selectedKey.value = key
  void openFile(path, prefill)
}

function discardAndGo() {
  const next = leaving.value
  leaving.value = null
  if (!next) return
  selectedKey.value = next.key
  void openFile(next.path, next.prefill)
}

function selectRow(row: MemoRow) {
  if (!editable(row) || !row.path) {
    // 不支持的那几家仍然能选中 —— 右边要说清「为什么这儿什么都没有」。
    selectedKey.value = row.key
    openPath.value = null
    text.value = ''
    saved.value = ''
    rev.value = null
    fileError.value = ''
    return
  }
  go(row.key, row.path)
}

/** 让这家自己管：开它自己的位置，正文预填现在生效的那份。 */
function takeover() {
  const row = selected.value
  const path = row ? takeoverPath(row) : null
  if (!row || !path) return
  go(row.key, path, text.value)
}

// ---------------------------------------------------------------------------
// 扫描 + 外部改动
// ---------------------------------------------------------------------------

/** 磁盘上已经和我们读到的那份不一样了。横幅一直挂着，直到重新加载或存下去。 */
const stale = ref(false)

async function load(keepSelection = true) {
  loading.value = true
  error.value = ''
  try {
    const next = await api.toolsScanMemo()
    scan.value = next
    if (!keepSelection) return
    if (selectedKey.value && !memoRows(next, '').some((r) => r.key === selectedKey.value)) {
      selectedKey.value = null
      openPath.value = null
    }
    stale.value = externallyChanged(next, openPath.value, rev.value)
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

/** 窗口重新聚焦 —— 十次里有九次就是刚去别的编辑器改完回来。 */
function onFocus() {
  void load()
}

onMounted(() => {
  void load()
  window.addEventListener('focus', onFocus)
})

// 「自己那份还不存在」的行（`missing` / 断掉的片段 `broken`）不自动选：开面板第一眼
// 落在一个「这个文件还没有」的空位置上，说的是全机器最不重要的那件事。
selectFirstRow(
  () => rows.value,
  () => selectedKey.value !== null,
  (r) => selectRow(r),
  (r) => r.state !== 'missing' && r.state !== 'broken',
)

const listEl = ref<HTMLElement>()

/** 把命中的那几个字切出来 —— 和会话列表 / 回收站用的是同一个 `.kw-hit`。 */
function hl(text: string) {
  return highlightSegments(text, toolsQuery.value)
}

// 列表换了一批（改过滤器、清搜索词、重新扫描），把选中那行重新滚进视野：先筛窄、
// 点中一条、再把筛选关掉，那一条会落回全量列表的中段 —— 右边详情画着它，左边却
// 一眼找不着。看得见就不动（`revealSelected` 自己判）。
watch(rows, async () => {
  await nextTick()
  revealSelected(listEl.value, '.tools-row.active')
})

onBeforeUnmount(() => window.removeEventListener('focus', onFocus))

/** 丢掉本地改动，拿磁盘上的那份重开。 */
const reloading = ref(false)
async function reload() {
  const path = openPath.value
  if (!path) return
  reloading.value = false
  stale.value = false
  await openFile(path)
  await load()
}

function requestReload() {
  if (dirty.value) {
    reloading.value = true
    return
  }
  void reload()
}

// ---------------------------------------------------------------------------
// 写
// ---------------------------------------------------------------------------

async function save() {
  const path = openPath.value
  const expected = rev.value
  // 接管刚存下去的那一下要把选中挪到那一家自己的行上。存之前先记住 —— 存完重扫一遍
  // 之后它已经不在回退状态了。
  const wasTakeover = takingOver.value
  if (!path || !expected || busy.value || !dirty.value) return
  busy.value = true
  fileError.value = ''
  try {
    const doc: MemoDoc = await api.toolsWriteMemo(path, text.value, expected)
    rev.value = doc.revision
    saved.value = text.value
    stale.value = false
    await load()
    if (wasTakeover) {
      // 按 `own` 找，不按 `path` —— 回退中的两家 `path` 是同一个（都指着别人那份），
      // 拿 `path` 找会把选中跳到 Claude 那一行去。
      const row = allRows.value.find((r) => r.own === path)
      if (row) selectedKey.value = row.key
    }
    emit('notify', t('tools.memo.saved', { path: short(path) }))
  } catch (e) {
    // 后端拒了盲写。不是弹个错就完 —— 用户要的是「外面到底改了什么」。
    fileError.value = String(e)
    stale.value = true
  } finally {
    busy.value = false
  }
}

async function openExternally() {
  if (!openPath.value) return
  try {
    await api.openPathExternal(openPath.value)
  } catch (e) {
    emit('notify', String(e), true)
  }
}

async function reveal(path: string) {
  try {
    await api.revealInFinder(path)
  } catch (e) {
    emit('notify', String(e), true)
  }
}

// ---------------------------------------------------------------------------
// 并排比较：分叉 / 冲突
// ---------------------------------------------------------------------------

const diff = ref<MemoDiff | null>(null)
const diffKind = ref<'fork' | 'conflict'>('fork')
/** 分叉时的「同步过去」目标。 */
const syncTo = ref<string | null>(null)

const fork = computed(() => forkOf(scan.value, openPath.value))
const forkSides = computed(() => forkOthers(fork.value, openPath.value))

async function compareFork(other: string) {
  const left = openPath.value
  if (!left || busy.value) return
  busy.value = true
  try {
    diff.value = await api.toolsDiffMemo(left, other)
    diffKind.value = 'fork'
    syncTo.value = other
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

/** 「外面改了什么」：打开时读到的那份 vs 现在磁盘上的。两边都只在内存里。 */
async function compareDisk() {
  const path = openPath.value
  if (!path || busy.value) return
  busy.value = true
  try {
    const doc = await api.toolsReadMemo(path)
    diff.value = await api.toolsDiffMemoText(
      saved.value,
      doc.text,
      t('tools.memo.conflict.left'),
      t('tools.memo.conflict.right'),
    )
    diffKind.value = 'conflict'
    syncTo.value = null
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

/**
 * 「以这份为准同步过去」—— 分叉唯一提供的写操作。两个方向都给：哪份是对的只有用户
 * 知道，只提供「左盖右」就逼着人先把右边的内容手抄到编辑器里再存。
 *
 * 不自动合并：两份同名文件的差异是人有意写的还是忘了同步，只有用户知道。这儿做的
 * 就是一次明说的覆盖，被覆盖的那份由 `atomic_write_backed_up` 留一份 `.bak`。
 */
async function syncFork(dir: 'toRight' | 'toLeft') {
  const other = syncTo.value
  const mine = openPath.value
  if (!other || !mine || busy.value) return
  busy.value = true
  try {
    if (dir === 'toRight') {
      // 写的是编辑器里那份（左边那栏就是它），不是磁盘上的 —— 没保存的改动也一起过去。
      const target = await api.toolsReadMemo(other)
      await api.toolsWriteMemo(other, text.value, target.revision)
      emit('notify', t('tools.memo.fork.synced', { path: short(other) }))
    } else {
      const source = await api.toolsReadMemo(other)
      const target = await api.toolsReadMemo(mine)
      const doc = await api.toolsWriteMemo(mine, source.text, target.revision)
      // 盖掉的是正开着的这份：编辑器要跟着换内容，否则右栏还是旧的、还标着「没保存」。
      rev.value = doc.revision
      text.value = source.text
      saved.value = source.text
      stale.value = false
      emit('notify', t('tools.memo.fork.synced', { path: short(mine) }))
    }
    diff.value = null
    syncTo.value = null
    await load()
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

function onDiffPrimary() {
  if (diffKind.value === 'fork') void syncFork('toRight')
  else {
    diff.value = null
    requestReload()
  }
}

/** 反方向 —— 分叉才有这个按钮，冲突时 `secondaryLabel` 是空的。 */
function onDiffSecondary() {
  if (diffKind.value === 'fork') void syncFork('toLeft')
}

/**
 * 比较框底下给哪几个按钮。标签给空的那个按钮框里就不画。
 *
 * 冲突那种只有「重新加载」，而且两边一样也要给 —— 那时候点它才能把「外面改过了」
 * 这个状态清掉。
 */
const diffActions = computed(() => {
  if (diffKind.value !== 'fork') {
    return {
      primaryLabel: t('tools.memo.stale.reload'),
      primaryTip: t('tools.memo.stale.reloadTip'),
      secondaryLabel: undefined,
      secondaryTip: undefined,
    }
  }
  if (!canSyncFork(diff.value)) {
    return {
      primaryLabel: undefined,
      primaryTip: undefined,
      secondaryLabel: undefined,
      secondaryTip: undefined,
    }
  }
  const mine = short(openPath.value ?? '')
  const other = short(syncTo.value ?? '')
  return {
    primaryLabel: t('tools.memo.fork.sync'),
    primaryTip: t('tools.memo.fork.syncTip', { from: mine, to: other }),
    secondaryLabel: t('tools.memo.fork.syncBack'),
    secondaryTip: t('tools.memo.fork.syncTip', { from: other, to: mine }),
  }
})

// ---------------------------------------------------------------------------
// 级联 / 引用
// ---------------------------------------------------------------------------

const cascade = computed(() => otherReaders(scan.value, openPath.value, selected.value?.agent ?? null))
const imports = computed(() => importsOf(scan.value, openPath.value))
const info = computed(() => effectiveOf(scan.value, selected.value?.agent ?? null))

function importRowKey(path: string) {
  return allRows.value.find((r) => r.path === path)?.key ?? path
}

</script>

<template>
  <!-- 健康条。这个面板的两个数字是「有几份在」和「有没有分叉」—— 后者是它存在的理由。 -->
  <div class="list-head tools-health">
    <template v-if="scan">
      <span
        class="tools-health-total"
        v-tooltip="rows.length === allRows.length
          ? ''
          : t('tools.total.filtered', {
            shown: String(rows.length),
            total: String(allRows.length),
          })"
      >
        {{ t('tools.memo.total', { n: shownOfTotal(rows.length, allRows.length) }) }}
      </span>
      <span class="memo-stat" v-tooltip="t('tools.memo.presentTip')">
        {{ t('tools.memo.present', { n: String(scan.summary.present) }) }}
      </span>
      <span
        v-if="scan.summary.broken > 0"
        class="memo-stat bad"
        v-tooltip="t('tools.memo.brokenTip')"
      >
        {{ t('tools.memo.broken', { n: String(scan.summary.broken) }) }}
      </span>
      <span
        v-if="scan.summary.forks > 0"
        class="memo-stat bad"
        v-tooltip="t('tools.memo.forksTip')"
      >
        {{ t('tools.memo.forks', { n: String(scan.summary.forks) }) }}
      </span>
      <!-- 重复和分叉是同一枚硬币的两面，所以挨着放。区别是这个**能动手**：
           内容一模一样，合并掉不会丢任何东西。 -->
      <span
        v-if="scan.summary.dups > 0"
        class="memo-stat warn"
        v-tooltip="t('tools.memo.dupsTip')"
      >
        {{ t('tools.memo.dups', { n: String(scan.summary.dups) }) }}
      </span>
    </template>
    <span v-else-if="loading" class="tools-health-empty">{{ t('tools.memo.loading') }}</span>

    <span class="tools-gap" />

    <!-- 去处。没有重复要合时不占位置 —— 平时它跟这个面板没关系。 -->
    <template v-if="allDups.length > 0">
      <button
        type="button"
        class="tools-act"
        :disabled="busy || loading"
        v-tooltip="t('tools.memo.dup.mergeAllTip', { n: String(allDups.length) })"
        @click="askMerge(allDups.map((d) => d.name))"
      >
        {{ t('tools.memo.dup.mergeAll') }}
      </button>
      <span class="memo-store">
        <span class="memo-store-label">{{ t('tools.memo.store') }}</span>
        <button
          type="button"
          class="memo-store-btn"
          v-tooltip="t('tools.memo.storeTip')"
          @click="pickStore"
        >{{ short(storePath) }}</button>
        <button
          v-if="memoStore"
          type="button"
          class="memo-inline-btn"
          v-tooltip="t('tools.memo.storeReset')"
          @click="setMemoStore(null)"
        >{{ t('tools.memo.storeReset') }}</button>
      </span>
    </template>

    <button
      type="button"
      class="tools-icon-btn"
      :disabled="loading"
      v-tooltip="t('tools.memo.refresh')"
      @click="load()"
    >
      <IconRefresh />
    </button>
  </div>

  <div class="tools-body">
    <!-- 列表：先七家 agent，再是被 `@` 引用进来的片段。 -->
    <section ref="listEl" class="tools-list" :aria-label="t('tools.listPane')">
      <p v-if="error" class="tools-placeholder error">{{ error }}</p>
      <p v-else-if="loading && !scan" class="tools-placeholder">{{ t('tools.memo.loading') }}</p>
      <p v-else-if="rows.length === 0" class="tools-placeholder">{{ t('tools.memo.empty') }}</p>
      <div v-else class="tools-list-inner">
        <button
          v-for="r in rows"
          :key="r.key"
          type="button"
          class="tools-row"
          :class="{ active: r.key === selectedKey, dim: r.state === 'unsupported' }"
          @click="selectRow(r)"
        >
          <span class="tools-row-top">
            <span class="tools-row-title memo-row-title">
              <component :is="r.agent ? agentIcons[r.agent] : IconFile" class="mcp-agent-ic" />
              <span class="tools-row-name"><span
                v-for="(seg, i) in hl(r.agent ? agentLabel(r.agent, true) : r.name)"
                :key="i"
                :class="{ 'kw-hit': seg.hit }"
              >{{ seg.text }}</span></span>
            </span>
            <span class="tools-row-meta">
              <span
                v-if="r.state !== 'ok'"
                class="tools-badge"
                :class="r.state"
              >{{ t(`tools.memo.state.${r.state}`) }}</span>
              <IconLink
                v-if="isLinked(r.path)"
                class="memo-row-link"
                v-tooltip="t('tools.memo.dup.linked', { path: short(linkOf(r.path) ?? '') })"
              />
              <span v-if="r.bytes > 0" class="tools-sub">{{ formatBytes(r.bytes) }}</span>
            </span>
          </span>
          <!-- 第二行是「这一行到底对应磁盘上的哪个文件」。回退中的那几家这儿显示的
               是别人的路径 —— 这正是要让人一眼看见的事。没有路径的那几家不画这一行：
               角标已经说了「没有这套机制」，再复述一遍只是把行撑高。 -->
          <span v-if="r.path" class="tools-row-desc memo-row-path"><span
            v-for="(seg, i) in hl(short(r.path))"
            :key="i"
            :class="{ 'kw-hit': seg.hit }"
          >{{ seg.text }}</span></span>
        </button>
      </div>
    </section>

    <div
      class="tools-resizer"
      role="separator"
      aria-orientation="vertical"
      @pointerdown="startToolsListResize"
    />

    <!-- 详情 = 编辑器。 -->
    <section class="tools-detail memo-detail" :aria-label="t('tools.detailPane')">
      <p v-if="!selected" class="tools-placeholder">{{ t('tools.memo.pickFile') }}</p>

      <template v-else>
        <header class="tools-detail-head">
          <h3>{{ openPath ? baseName(openPath) : selected.name }}</h3>
          <span v-if="dirty" class="memo-dot" v-tooltip="t('tools.memo.dirty')" aria-hidden="true" />
          <span class="memo-head-path">{{ openPath ? short(openPath) : '' }}</span>
          <span class="tools-gap" />

          <button
            v-if="takeoverPath(selected) && !takingOver"
            type="button"
            class="tools-act"
            :disabled="busy"
            v-tooltip="t('tools.memo.action.takeoverTip', {
              agent: agentLabel(selected.agent!, true),
              path: short(takeoverPath(selected) ?? ''),
            })"
            @click="takeover"
          >
            {{ t('tools.memo.action.takeover') }}
          </button>
          <!-- 读 / 改来回切。默认在「读」，改完可以切回去看渲染后的样子。 -->
          <button
            v-if="showEditor && isMarkdown && canEdit"
            type="button"
            class="tools-act"
            v-tooltip="mode === 'read' ? t('tools.memo.action.editTip') : t('tools.memo.action.readTip')"
            @click="mode = mode === 'read' ? 'edit' : 'read'"
          >
            <IconPencil v-if="mode === 'read'" />
            <IconEye v-else />
            {{ mode === 'read' ? t('tools.memo.action.edit') : t('tools.memo.action.read') }}
          </button>
          <button
            type="button"
            class="tools-act"
            :disabled="!openPath"
            v-tooltip="t('list.action.reveal')"
            :aria-label="t('list.action.reveal')"
            @click="reveal(openPath!)"
          >
            <IconFolder />
          </button>
          <button
            type="button"
            class="tools-act"
            :disabled="!openPath"
            v-tooltip="t('tools.memo.action.externalTip')"
            @click="openExternally"
          >
            <IconExternalLink />
          </button>
          <button
            type="button"
            class="tools-act primary"
            :class="{ running: busy }"
            :disabled="!canEdit || !dirty || busy"
            v-tooltip="t('tools.memo.action.saveTip')"
            @click="save"
          >
            <span v-if="busy" class="chip-spinner" aria-hidden="true" />
            <IconSave v-else />
            {{ t('tools.memo.action.save') }}
          </button>
        </header>

        <!-- 这家根本没有 home 级约定。给个编辑器让用户白写一份没人读的文件更糟。 -->
        <p v-if="selected.state === 'unsupported'" class="tools-note memo-note">
          {{ t('tools.memo.unsupportedNote', { agent: agentLabel(selected.agent!, true) }) }}
        </p>

        <template v-else>
          <p v-if="fileError" class="tools-warn">
            {{ fileError }}
            <button type="button" class="memo-inline-btn" @click="compareDisk">
              {{ t('tools.memo.stale.diff') }}
            </button>
          </p>

          <!-- 外面改过了。挂着不走，直到重新加载或存下去。 -->
          <p v-else-if="stale" class="tools-warn">
            {{ t('tools.memo.stale.msg') }}
            <button type="button" class="memo-inline-btn" @click="compareDisk">
              {{ t('tools.memo.stale.diff') }}
            </button>
            <button type="button" class="memo-inline-btn" @click="requestReload">
              {{ t('tools.memo.stale.reload') }}
            </button>
          </p>

          <!-- 接管中：正文是预填的，还没落盘。 -->
          <p v-if="takingOver" class="tools-note memo-note">
            {{ t('tools.memo.takeoverNote', {
              agent: agentLabel(selected.agent!, true),
              from: short(info?.effective ?? ''),
            }) }}
          </p>
          <!-- 回退中：现在读的是别人那份。 -->
          <p
            v-else-if="selected.state === 'fallback'"
            class="tools-note memo-note"
          >
            {{ t('tools.memo.fallbackNote', {
              agent: agentLabel(selected.agent!, true),
              own: short(selected.own ?? ''),
            }) }}
          </p>
          <p v-else-if="rev && !rev.exists && drafting" class="tools-note memo-note">
            {{ t('tools.memo.missingNote') }}
          </p>

          <!-- 级联提示。这个面板最容易踩的坑：改 CLAUDE.md，grok 和 opencode 也读到。 -->
          <p v-if="cascade.length > 0" class="tools-note memo-cascade">
            {{ t('tools.memo.cascade') }}
            <span v-for="r in cascade" :key="r.agent" class="memo-cascade-one">
              <component :is="agentIcons[r.agent]" class="mcp-agent-ic" />
              {{ agentLabel(r.agent, true) }}
              <span class="tools-sub">{{ t(`tools.memo.role.${r.role}`) }}</span>
            </span>
          </p>

          <!-- 重复：同名而且内容一模一样。这条**能动手** —— 合并掉不丢任何东西，
               而放着不管的代价是「改一份，另外几家还读着旧的，且没人提醒你」。 -->
          <p v-if="dup && mergeable(dup)" class="tools-warn memo-dup">
            {{ t('tools.memo.dup.title', { name: dup.name, n: String(dup.paths.length) }) }}
            <span class="memo-dup-hint">{{ t('tools.memo.dup.hint') }}</span>
            <span v-for="p in dupElsewhere" :key="p" class="memo-dup-where">{{ short(p) }}</span>
            <button
              type="button"
              class="memo-inline-btn strong"
              :disabled="busy"
              v-tooltip="t('tools.memo.dup.mergeTip', { store: short(storePath) })"
              @click="askMerge([dup.name])"
            >
              {{ t('tools.memo.dup.merge') }}
            </button>
          </p>

          <!-- 合并之后这儿只是个指过去的链接。不画出来的话，合并完看上去和没合一样，
               而「在这儿改会不会影响别家」这个问题也就没了答案 —— 会，那正是重点。 -->
          <p v-else-if="linkTarget" class="tools-note memo-note">
            <IconLink class="memo-note-ic" />
            {{ t('tools.memo.dup.done', { store: short(linkTarget) }) }}
          </p>

          <!-- 分叉。只提示不自动合并，给一个明说的「以这份为准同步过去」。 -->
          <p v-if="forkSides.length > 0" class="tools-warn memo-fork">
            {{ t('tools.memo.fork.title', { name: fork?.name ?? '', n: String(fork?.sides.length ?? 0) }) }}
            <button
              v-for="s in forkSides"
              :key="s.path"
              type="button"
              class="memo-inline-btn"
              :disabled="busy"
              @click="compareFork(s.path)"
            >
              {{ t('tools.memo.fork.compare', { path: short(s.path) }) }}
            </button>
          </p>

          <!-- `@` 引用进来的片段。只展开第一层：第二层只报个数，不再往下钻（防环）。 -->
          <div v-if="imports.length > 0" class="memo-imports">
            <span class="memo-imports-label">{{ t('tools.memo.detail.imports') }}</span>
            <button
              v-for="im in imports"
              :key="im.line"
              type="button"
              class="memo-import"
              :class="{ broken: !im.exists }"
              :disabled="!im.path"
              v-tooltip="!im.path
                ? t('tools.memo.imports.unresolved')
                : !im.exists
                  ? t('tools.memo.imports.brokenTarget', { path: short(im.path) })
                  : im.nested > 0
                    ? t('tools.memo.imports.nested', { n: String(im.nested) })
                    : short(im.path)"
              @click="im.path && go(importRowKey(im.path), im.path)"
            >
              {{ im.raw }}
              <b v-if="im.exists">{{ formatBytes(im.bytes) }}</b>
            </button>
          </div>

          <!-- 还没有这个文件，而且用户还没说要建。**不替他建**：右边问一句，按钮他自己按。 -->
          <div v-if="openPath && !showEditor" class="memo-create">
            <IconFilePlus class="memo-create-icon" aria-hidden="true" />
            <p class="memo-create-msg">{{ t('tools.memo.create.msg') }}</p>
            <code class="memo-create-path">{{ short(openPath) }}</code>
            <button
              type="button"
              class="btn primary memo-create-btn"
              :disabled="busy"
              v-tooltip="t('tools.memo.create.tip')"
              @click="startDraft"
            >
              {{ t('tools.memo.create.btn') }}
            </button>
          </div>
          <!-- 默认这一屏：渲染后的 markdown。改完切回来也看的是当前正文（含未保存的）。 -->
          <div
            v-else-if="openPath && viewMode === 'read'"
            ref="previewEl"
            class="memo-preview md"
            v-html="renderText(text)"
          />
          <CodeEditor
            v-else-if="openPath"
            ref="editorEl"
            :key="openPath"
            v-model="text"
            :lang="langOfPath(openPath)"
            class="memo-editor"
            @save="save"
          />
        </template>
      </template>
    </section>
  </div>

  <!-- 分叉 / 冲突的并排比较。 -->
  <MemoDiffModal
    :show="diff !== null"
    :title="diffKind === 'fork'
      ? t('tools.memo.fork.diffTitle', { name: fork?.name ?? '' })
      : t('tools.memo.conflict.title')"
    :diff="diff"
    :left-label="diffKind === 'fork' ? short(openPath ?? '') : t('tools.memo.conflict.left')"
    :right-label="diffKind === 'fork' ? short(syncTo ?? '') : t('tools.memo.conflict.right')"
    v-bind="diffActions"
    danger
    :busy="busy"
    @primary="onDiffPrimary"
    @secondary="onDiffSecondary"
    @close="diff = null"
  />

  <!-- 改了没存还要走（切文件 / 重新加载）。 -->
  <ConfirmModal
    :show="leaving !== null"
    :title="t('tools.memo.discardTitle')"
    :message="t('tools.memo.discardMsg', { file: openPath ? short(openPath) : '' })"
    :ok-text="t('tools.memo.discardOk')"
    danger
    @confirm="discardAndGo"
    @cancel="leaving = null"
  />
  <ConfirmModal
    :show="reloading"
    :title="t('tools.memo.stale.reload')"
    :message="t('tools.memo.stale.reloadDirty')"
    :ok-text="t('tools.memo.discardOk')"
    danger
    @confirm="reload"
    @cancel="reloading = false"
  />

  <!-- 合并的二次确认。和 MCP / Hooks 共用同一个弹框：它不认识任何一种 report，
       翻译在 `toolsMemoActions.memoMergePlan` 里（那儿有测试）。 -->
  <ToolsPlanModal
    :show="plan !== null"
    :plan="plan"
    :busy="planBusy"
    @confirm="applyMerge"
    @cancel="plan = null"
  />
</template>

<style scoped>
/* 重复提示。用 warn 底色（和分叉同一档），但它多一个能点的按钮。 */
.memo-dup {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}
.memo-dup-hint {
  color: var(--text-dim);
}
/* 另外几个位置。等宽 + 弱化 —— 它们是证据，不是主语。 */
.memo-dup-where {
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text-mute);
  padding: 1px 5px;
  border-radius: 4px;
  background: var(--surface-2);
}
/* Tailwind 的 preflight 把 `svg` 设成了 `display: block` —— 这条注记是个普通 `<p>`，
   不是 flex，图标于是自己占一行、`vertical-align` 也就没人理。写回 inline-block。 */
.memo-note-ic {
  display: inline-block;
  width: 12px;
  height: 12px;
  flex-shrink: 0;
  vertical-align: -2px;
  margin-right: 4px;
  opacity: 0.7;
}
/* 列表行右侧那个链接标。合并之后每一行都该一眼看出「这儿只是指过去」。 */
.memo-row-link {
  width: 11px;
  height: 11px;
  flex-shrink: 0;
  opacity: 0.55;
}
/* 健康条右侧的去处。挨着「合并全部重复」，因为它回答的正是「搬到哪儿」。 */
.memo-store {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  white-space: nowrap;
}
.memo-store-label {
  font-size: 11px;
  color: var(--text-mute);
}
.memo-store-btn {
  font-family: var(--mono);
  font-size: 11px;
  color: var(--text-dim);
  padding: 2px 6px;
  border-radius: 5px;
  background: var(--surface-2);
  transition: background 0.12s, color 0.12s;
}
.memo-store-btn:hover {
  background: var(--surface-hover);
  color: var(--text);
}

/* 行 / 角标 / 小节 / 动作按钮那一套在 `style.css`，四个面板共用。
   这儿只写这个面板独有的：右栏是个编辑器（要撑满，不跟着内容滚），加上几条提示条。 */
.memo-detail {
  display: flex;
  flex-direction: column;
  gap: 8px;
  /* 编辑器自己滚。整栏滚的话敲到一半标题和提示条会跟着跑上去。 */
  overflow: hidden;
  padding-bottom: 14px;
}
.memo-detail > .tools-detail-head {
  flex-shrink: 0;
}

/* 健康条上那几个数。它们是**计数**不是筛选器 —— 借 `.tools-chip` 会长出 hover 和
   点击态，看上去像能点。 */
.memo-stat {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11.5px;
  color: var(--text-mute);
}
.memo-stat.bad {
  color: var(--danger);
}
/* 重复不是坏事，是**待办**：内容还没分家，合掉不丢东西。画成红的会让人以为出错了。 */
.memo-stat.warn {
  color: var(--warning, #b7791f);
}

.memo-head-path {
  flex: 1;
  min-width: 0;
  font-size: 11.5px;
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  color: var(--text-mute);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 没存的小圆点。比在标题里加个 `*` 好认，也不会把路径顶得换行。 */
.memo-dot {
  width: 6px;
  height: 6px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--brand);
}

.memo-note,
.memo-cascade,
.memo-fork {
  flex-shrink: 0;
  margin: 0;
}
.memo-cascade {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 10px;
}
.memo-cascade-one {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.memo-fork {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}

/* 提示条里那几个「看看差异」「重新加载」「和这份比」。它们长在一句话里，不是独立
   动作按钮 —— 画成 `.tools-act` 那样的方框会喧宾夺主。 */
.memo-inline-btn {
  padding: 1px 8px;
  border-radius: 999px;
  border: 1px solid currentColor;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 11.5px;
  cursor: pointer;
  white-space: nowrap;
}
.memo-inline-btn:hover:not(:disabled) {
  background: color-mix(in srgb, currentColor 12%, transparent);
}
.memo-inline-btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.memo-imports {
  flex-shrink: 0;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}
.memo-imports-label {
  font-size: 11.5px;
  color: var(--text-mute);
}
.memo-import {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 8px;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: transparent;
  color: var(--text-dim);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11px;
  cursor: pointer;
}
.memo-import:hover:not(:disabled) {
  background: var(--surface-hover);
  color: var(--text);
}
.memo-import b {
  font-weight: 400;
  color: var(--text-mute);
}
/* 断链的那条：指向的文件不存在。仍然能点 —— 点开就是那个文件的「保存即新建」，
   正好是修它最短的一条路。 */
.memo-import.broken {
  border-color: var(--danger);
  color: var(--danger);
}
.memo-import.broken:hover {
  background: var(--danger-soft);
  color: var(--danger);
}

.memo-editor {
  flex: 1;
  min-height: 0;
}

/* 读模式那一屏。边框、圆角、底色都跟编辑器一样 —— 来回切的时候框不动，只是里头的内容
   从源码变成渲染结果。 */
.memo-preview {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px 14px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface-2);
  font-size: 13px;
  line-height: 1.7;
}
.memo-preview :deep(> *:first-child) {
  margin-top: 0;
}

/* 「还没建」这一屏。给的是一句话加一个按钮，不是一个空编辑器 —— 空编辑器长得和
   「这份是空的」一模一样，而这两件事差着一个文件。
   画法照 `.empty` 那一套：居中一列、图标压淡，不套框 —— 一个虚线大框撑满整块内容区
   只会把「什么都没有」放大一遍。 */
.memo-create {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 24px;
  text-align: center;
}
.memo-create-icon {
  width: 30px;
  height: 30px;
  color: var(--text-mute);
  opacity: 0.5;
  stroke-width: 1.3;
}
.memo-create-msg {
  margin: 0;
  font-size: 13px;
  font-weight: 500;
  color: var(--text);
}
/* 路径是这屏里唯一的事实，单独拎成一枚 chip；太长时自己横滚，不撑破内容区。 */
.memo-create-path {
  max-width: 100%;
  overflow-x: auto;
  padding: 3px 9px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface-2);
  color: var(--text-mute);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11.5px;
  white-space: nowrap;
}
.memo-create-btn {
  margin-top: 6px;
}

/* 没有 home 级约定的那几家在列表里压暗。禁用态的行仍然可点（右边要说明原因），
   所以不能用 `:disabled`。 */
.tools-row.dim .tools-row-name {
  color: var(--text-mute);
}

/* 图标要和名字并排。`.tools-row-title` 在 style.css 里不是 flex（另外三个面板的图标
   都挂在右边的 `.tools-row-meta` 上），照用会让图标自己占一行。 */
.memo-row-title {
  display: flex;
  align-items: center;
  gap: 6px;
}
.memo-row-title .tools-row-name {
  flex: 1;
  min-width: 0;
}

.memo-row-path {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  display: block;
}
</style>
