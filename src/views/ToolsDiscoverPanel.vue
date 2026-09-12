<script setup lang="ts">
// 工具管理 · 发现面板（阶段 A：只读搜索）。
//
// 判断全在 `src/toolsRegistry.ts` —— `src/views/` 在 `vitest.config.ts` 是覆盖率
// 排除的。
//
// **这是五个面板里唯一会发网络请求的一个**，所以它和另外四个有三处行为差异，
// 每一处都是被数据逼出来的，不是风格选择：
//
// 1. **输入 = 一次 HTTP，不是过滤内存数组。** 顶栏那 200 ms 之上再叠 500 ms；
//    按回车可以立刻发（`toolsSearchSubmit`）。
// 2. **1 个字不发请求。** 接口的硬门槛是 2 个字符，短了直接 HTTP 400 返回一句英文。
//    与其把那句话糊用户脸上，不如根本不发。
// 3. **过期响应必须丢。** 打 `react` 的过程中会发出好几轮，慢的那轮回来得更晚，
//    屏幕上就会停在一个更短的词的结果上 —— 看上去就是「搜出来的东西不对」。
//
// 列表第二行放的是 **source 而不是描述**：接口只给五个字段，没有描述
// （方案文档 1.1 记了实测过程）。描述要等阶段 B 的 git 检出才有。
import { computed, onUnmounted, reactive, ref, watch } from 'vue'
import { formatSize, highlightSegments } from '../format'
import type { PreviewError, RegistryError, RegistryHit, RegistryPreview, RegistrySearch } from '../types'
import * as api from '../api'
import { t } from '../i18n'
import { startToolsListResize, toolsQuery, toolsSearchSubmit } from '../toolsPanel'
import {
  HIT_SORTS,
  SEARCH_DEBOUNCE_MS,
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
  registryErrorText,
  repoUrl,
  skillsShUrl,
  sortHits,
  type HitSort,
} from '../toolsRegistry'
import { fileIconFor, IconClose, IconCopy, IconExternalLink, IconRefresh } from '../components/icons'
import SkillDetailSkeleton from '../components/SkillDetailSkeleton.vue'
import ToolsListSkeleton from '../components/ToolsListSkeleton.vue'
import SkillFindings from '../components/SkillFindings.vue'
import InstallTerminal from '../components/InstallTerminal.vue'
import ConfirmModal from '../modals/ConfirmModal.vue'

const emit = defineEmits<{ (e: 'notify', msg: string, error?: boolean): void }>()

const result = ref<RegistrySearch | null>(null)
const error = ref<RegistryError | null>(null)
const loading = ref(false)
const sort = ref<HitSort>('relevance')
const listEl = ref<HTMLElement>()

/**
 * 换次序要回到顶上。
 *
 * 不回的话，翻到第 60 条时点「按安装量」，屏幕上换的是一批**同样陌生**的名字 ——
 * 重排确实发生了，但看上去像没反应。而按这一档该先看的恰好是头几条。
 */
function changeSort(next: HitSort) {
  if (next === sort.value) return
  sort.value = next
  if (listEl.value) listEl.value.scrollTop = 0
}
const selectedKey = ref<string | null>(null)

const state = computed(() => discoverState(toolsQuery.value, loading.value, error.value, result.value))
const list = computed(() => sortHits(result.value?.hits ?? [], sort.value))
const selected = computed<RegistryHit | null>(
  () => list.value.find((h) => hitKey(h) === selectedKey.value) ?? null,
)

function hl(text: string) {
  return highlightSegments(text, toolsQuery.value)
}

// ---------------------------------------------------------------------------
// 详情预览
// ---------------------------------------------------------------------------

const preview = ref<RegistryPreview | null>(null)
const previewError = ref<PreviewError | null>(null)
const previewLoading = ref(false)
const pstate = computed(() =>
  previewState(selected.value, previewLoading.value, previewError.value, preview.value),
)

/**
 * 看过详情的条目的描述，按 `source/skillId` 记着。
 *
 * 搜索接口不给描述（方案文档 1.1），只有走过一次 git 才拿得到。记下来之后列表第二行
 * 从 source 换成描述 —— 翻回来还看得见，省得为了想起「这个是干嘛的」再等一次 clone。
 * 只活在这个面板的生命周期里：切走再回来重新搜，那时候结果本来也换了。
 */
const described = reactive(new Map<string, string>())

// ---------------------------------------------------------------------------
// 搜索
// ---------------------------------------------------------------------------

let timer: ReturnType<typeof setTimeout> | undefined
/**
 * 第几轮请求。回来的那一轮不是最新的就整个丢掉。
 *
 * 光比查询词不够：同一个词按两次回车会发两轮，前一轮回来得晚就会把后一轮的结果
 * 盖掉 —— 内容碰巧一样所以看不出来，但 loading 会提前熄灭。
 */
let round = 0

async function run(query: string) {
  const q = query.trim()
  const mine = ++round
  loading.value = true
  error.value = null
  try {
    const got = await api.toolsRegistrySearch(q, SEARCH_LIMIT)
    // 两道过期闸：这轮还是不是最新的一轮，以及接口回显的词还是不是框里那个。
    if (mine !== round || !isFresh(got, toolsQuery.value)) return
    result.value = got
    // 结果换了，选中项多半已经不在里面了。留着它详情区会指向一条不存在的行。
    if (!got.hits.some((h) => hitKey(h) === selectedKey.value)) selectedKey.value = null
  } catch (e) {
    if (mine !== round) return
    error.value = asRegistryError(e)
    // **不保留上一次的结果。** 出错了还挂着一列旧结果，用户会以为那就是这次搜到的。
    result.value = null
    selectedKey.value = null
  } finally {
    if (mine === round) loading.value = false
  }
}

/** 排期一次搜索。`now` 走回车那条路，跳过防抖。 */
function schedule(now: boolean) {
  clearTimeout(timer)
  const q = toolsQuery.value.trim()
  // 空查询和太短的查询都不发请求，但要把上一次的结果清掉 —— 不然删字删到只剩一个,
  // 屏幕上还挂着两个字时的那一列。
  if (q === '' || state.value === 'tooShort') {
    round += 1
    loading.value = false
    error.value = null
    result.value = null
    selectedKey.value = null
    return
  }
  if (now) {
    void run(q)
    return
  }
  timer = setTimeout(() => void run(q), SEARCH_DEBOUNCE_MS)
}

watch(toolsQuery, () => schedule(false), { immediate: true })
watch(toolsSearchSubmit, () => schedule(true))
onUnmounted(() => {
  clearTimeout(timer)
  // 面板关掉之后那个 2 秒的定时器还在跑；不清的话它会给一个已经不存在的终端弹框。
  clearTimeout(ranTimer)
})

// ---------------------------------------------------------------------------
// 动作
// ---------------------------------------------------------------------------

function select(key: string) {
  selectedKey.value = selectedKey.value === key ? null : key
}

/**
 * 选中的是谁变了就重新取。
 *
 * 挂在 watch 上而不是写在 `select()` 里：选中项还会被**别的路径**改掉 ——
 * 搜索结果换了之后旧的选中项不在里面（`run()` 会清空它），查询词删空时也会清。
 * 那些路径一条都不该留着上一条的文件清单和风险点在屏幕上。
 *
 * 比的是 `source/skillId` 而不是对象本身：重排和重搜会造出新的对象，
 * 按引用比会在同一条上白跑一次 git。
 */
watch(() => (selected.value ? hitKey(selected.value) : null), () => void loadPreview())

/**
 * 第几轮预览。和搜索那边同一个理由：首次 3 秒级、命中缓存 200 ms 级，差两个数量级，
 * 点第二条时第一条很可能还没回来。
 */
let pround = 0

async function loadPreview(refresh = false) {
  const hit = selected.value
  // 取消选中时把上一条的内容清掉：留着的话点空白处之后详情区还挂着别人的风险点。
  preview.value = null
  previewError.value = null
  // 域名源不去取：没有任何公开的取内容路径（方案文档 1.4），起一次 git 只是白等
  // 三秒换一句「装不了」，而详情里那段说明已经在那儿了。
  if (!hit || !hit.installable) {
    pround += 1
    previewLoading.value = false
    return
  }
  const mine = ++pround
  previewLoading.value = true
  try {
    const got = await api.toolsRegistryPreview(hit.source, hit.skillId, refresh)
    // 两道闸：这轮还是不是最新的一轮，以及回来的东西是不是当前选中那条的。
    // 不比对的话屏幕上会是另一个 skill 的文件清单，而标题写着你刚点的那个 ——
    // 这种错不报任何错，却恰好错在这一屏唯一要回答的问题上。
    if (mine !== pround || !isPreviewFor(got, hit)) return
    preview.value = got
    const desc = previewDescription(got)
    if (desc) described.set(hitKey(hit), desc)
  } catch (e) {
    if (mine !== pround) return
    previewError.value = asPreviewError(e)
  } finally {
    if (mine === pround) previewLoading.value = false
  }
}

/**
 * 「重新取」= 扔掉缓存里那份重新克隆。
 *
 * 缓存是**浅克隆**，`ls-tree` 只看得见克隆那一刻的 HEAD —— 仓库后来更新了，
 * 不重新克隆是看不见的。所以这个按钮不是「刷新界面」，是「重新下载」。
 */
function refreshPreview() {
  void loadPreview(true)
}

function open(url: string | null) {
  if (url) void api.openUrl(url)
}

/**
 * 「复制并安装」：命令进剪贴板，同时在详情区拉起一个终端把它打进去。
 *
 * **我们不代替用户执行**（方案文档第七章的那条仍然成立）：`npx skills add` 会下载并
 * 执行任意 npm 包、没法 dry-run。所以它跑在一个**看得见**的终端里 —— 每一行输出都在
 * 眼前，要停随时 Ctrl-C。装之前该看清楚的（描述 / 文件 / 风险点）上面那一屏已经摊过了。
 *
 * 剪贴板那一份不省：终端里的命令没法选中复制走，而有人就是想拿到别处去跑。
 */
const terminalFor = ref<{ hit: RegistryHit; command: string; cwd: string } | null>(null)

/**
 * 命令已经真的打进去并回车了 —— 弹框告诉用户一声。
 *
 * 试过两版轻的都不行：toast 飘在角上、几秒就没；换成详情区里常驻的一行，还是漏。
 * 原因是结构性的 —— 这一刻用户的眼睛在终端里盯着输出，而任何摆在终端**外面**的
 * 提示都在他的视线之外。所以这一下必须挡住屏幕，让他非看不可。
 *
 * 不能一开终端就弹：那一刻命令还没发出去（要等 shell 的提示符先出来），提前说
 * 「已执行」是在替一件还没发生的事打包票。
 */
const ranAck = ref(false)

const termEl = ref<{ focus: () => void } | null>(null)

/**
 * 弹框晚三秒半再出来。
 *
 * 命令刚回车那一瞬屏幕上正开始刷输出（clone、found N skills），此刻盖一个框上去，
 * 挡掉的恰好是「它真的在跑」这个证据。等一会儿，让用户先看见终端动起来，再告诉他
 * 接下来轮到他了。
 */
const RAN_DELAY_MS = 3500
let ranTimer = 0

function onRan() {
  clearTimeout(ranTimer)
  ranTimer = window.setTimeout(() => {
    ranAck.value = true
  }, RAN_DELAY_MS)
}

/** 关掉弹框 = 「我看完了」，键盘立刻还给终端，用户接着勾选 agent。 */
function ackRan() {
  ranAck.value = false
  termEl.value?.focus()
}

async function copyAndInstall(hit: RegistryHit) {
  const command = installCommand(hit)
  if (!command) return
  try {
    await navigator.clipboard.writeText(command)
  } catch {
    // 剪贴板失败不该挡住安装 —— 那才是这个按钮的主业。
  }
  try {
    const cwd = await api.homeDir()
    clearTimeout(ranTimer)
    ranAck.value = false
    terminalFor.value = { hit, command, cwd }
  } catch (e) {
    emit('notify', String(e), true)
  }
}

function closeTerminal() {
  clearTimeout(ranTimer)
  terminalFor.value = null
  ranAck.value = false
}
</script>

<template>
  <div class="list-head tools-health">
    <template v-if="state === 'ready' && result">
      <span class="tools-health-total">
        {{ t('tools.discover.found', { n: String(result.hits.length) }) }}
      </span>
      <span
        v-if="result.searchType"
        class="disc-kind"
        v-tooltip="t('tools.discover.searchTypeTip')"
      >{{ t(`tools.discover.searchType.${result.searchType}`) }}</span>

      <!-- 相关度那一档是接口给的次序，安装量那一档是纯本地重排，都不重新发请求。 -->
      <button
        v-for="s in HIT_SORTS"
        :key="s"
        type="button"
        class="tools-chip"
        :class="{ active: sort === s }"
        @click="changeSort(s)"
      >{{ t(`tools.discover.sort.${s}`) }}</button>

      <span class="tools-gap" />
      <button
        type="button"
        class="tools-icon-btn"
        v-tooltip="t('tools.discover.retry')"
        :aria-label="t('tools.discover.retry')"
        @click="schedule(true)"
      >
        <IconRefresh />
      </button>
    </template>
    <span v-else-if="state === 'loading'" class="tools-health-empty">
      {{ t('tools.discover.loading') }}
    </span>
    <span v-else class="tools-health-empty">{{ t('tools.discover.idleHint') }}</span>
  </div>

  <div class="tools-body">
    <section ref="listEl" class="tools-list" :aria-label="t('tools.listPane')">
      <p v-if="state === 'idle'" class="tools-placeholder">{{ t('tools.discover.idle') }}</p>
      <p v-else-if="state === 'tooShort'" class="tools-placeholder">
        {{ t('tools.discover.tooShort') }}
      </p>
      <!-- 骨架而不是一行「正在搜索…」：右边详情区在同样的等待里画的就是骨架，
           左边摆一行居中小字的话，同一屏上会有两种「正在读」的语言。 -->
      <ToolsListSkeleton v-else-if="state === 'loading'" />
      <!-- 失败时把分类过的那句话摆在上面，`ureq` 的英文原文折在「详细信息」里 ——
           四种语言的界面里只有一种看得懂那句原文。 -->
      <div v-else-if="state === 'error' && error" class="tools-placeholder error disc-error">
        <p>{{ registryErrorText(error) }}</p>
        <details v-if="error.detail">
          <summary>{{ t('tools.discover.errDetail') }}</summary>
          <code>{{ error.detail }}</code>
        </details>
        <button type="button" class="tools-act" @click="schedule(true)">
          <IconRefresh />
          {{ t('tools.discover.retry') }}
        </button>
      </div>
      <p v-else-if="state === 'empty'" class="tools-placeholder">
        {{ t('tools.discover.empty', { q: toolsQuery.trim() }) }}
      </p>
      <div v-else class="tools-list-inner">
        <button
          v-for="h in list"
          :key="hitKey(h)"
          type="button"
          class="tools-row"
          :class="{ active: hitKey(h) === selectedKey }"
          @click="select(hitKey(h))"
        >
          <span class="tools-row-top">
            <span class="tools-row-title disc-title">
              <span class="tools-row-name"><span
                v-for="(seg, i) in hl(h.name)"
                :key="i"
                :class="{ 'kw-hit': seg.hit }"
              >{{ seg.text }}</span></span>
              <span v-if="hitSubtitle(h, described).described" class="disc-src">{{ h.source }}</span>
            </span>
            <span class="tools-row-meta">
              <span
                v-if="!h.installable"
                class="tools-badge off"
                v-tooltip="t('tools.discover.blockedTip')"
              >{{ t('tools.discover.badge.blocked') }}</span>
              <span
                class="disc-installs"
                v-tooltip="t('tools.discover.installsTip', { n: formatInstalls(h.installs) })"
              >{{ formatInstalls(h.installs) }}</span>
            </span>
          </span>
          <!-- 第二行默认是 source（接口不给描述，方案文档 1.1）；看过一次详情之后
               换成描述，那时候 source 挪到名字右边 —— 两样都还在。 -->
          <span class="tools-row-desc">{{ hitSubtitle(h, described).text }}</span>
        </button>
      </div>
    </section>

    <div
      class="tools-resizer"
      role="separator"
      aria-orientation="vertical"
      @pointerdown="startToolsListResize"
    />

    <section
      class="tools-detail"
      :class="{ 'disc-term-mode': !!terminalFor }"
      :aria-label="t('tools.detailPane')"
    >
      <!-- 终端接管整个详情区。不做成弹框：安装会跑几十秒、会问 y/n、会报错，
           那期间用户要能一边看输出一边照着左边那一屏核对装的是哪一个。 -->
      <template v-if="terminalFor">
        <header class="tools-detail-head">
          <h3>{{ terminalFor.hit.name }}</h3>
          <span class="disc-src">{{ terminalFor.hit.source }}</span>
          <span class="tools-gap" />
          <button type="button" class="tools-act" @click="closeTerminal">
            <IconClose />
            {{ t('tools.discover.closeTerminal') }}
          </button>
        </header>
        <p class="disc-note">{{ t('tools.discover.terminalHint') }}</p>
        <InstallTerminal
          ref="termEl"
          :command="terminalFor.command"
          :cwd="terminalFor.cwd"
          @ran="onRan"
          @fail="(m) => emit('notify', m, true)"
        />
      </template>
      <p v-else-if="!selected" class="tools-placeholder">{{ t('tools.discover.detailPending') }}</p>
      <template v-else>
        <header class="tools-detail-head">
          <h3>{{ selected.name }}</h3>
          <span
            v-if="!selected.installable"
            class="tools-badge off"
            v-tooltip="t('tools.discover.blockedTip')"
          >{{ t('tools.discover.badge.blocked') }}</span>
          <span class="tools-gap" />
          <button
            v-if="repoUrl(selected)"
            type="button"
            class="tools-act"
            @click="open(repoUrl(selected))"
          >
            <IconExternalLink />
            {{ t('tools.discover.openRepo') }}
          </button>
          <button type="button" class="tools-act" @click="open(skillsShUrl(selected))">
            <IconExternalLink />
            {{ t('tools.discover.openPage') }}
          </button>
          <!-- 这条命令我们自己不执行（方案文档第七章）：它会下载并执行任意 npm 包、
               没法 dry-run。但想自己在终端装的人不该被挡住。 -->
          <button
            v-if="installCommand(selected)"
            type="button"
            class="tools-act"
            v-tooltip="installCommand(selected) ?? ''"
            @click="copyAndInstall(selected)"
          >
            <IconCopy />
            {{ t('tools.discover.copyAndInstall') }}
          </button>
          <button
            v-if="selected.installable"
            type="button"
            class="tools-icon-btn"
            v-tooltip="t('tools.discover.refreshPreview')"
            :aria-label="t('tools.discover.refreshPreview')"
            :disabled="previewLoading"
            @click="refreshPreview"
          >
            <IconRefresh />
          </button>
        </header>

        <!-- 描述来自 frontmatter，只有走过一次 git 才拿得到（接口不给）。 -->
        <p v-if="preview && previewDescription(preview)" class="disc-desc">
          {{ previewDescription(preview) }}
        </p>

        <div class="tools-section">
          <h4>{{ t('tools.discover.detail.source') }}</h4>
          <div class="disc-kv">
            <span class="disc-key">{{ t('tools.discover.detail.repo') }}</span>
            <code class="tools-cmd">{{ selected.source }}</code>
          </div>
          <div class="disc-kv">
            <span class="disc-key">{{ t('tools.discover.detail.id') }}</span>
            <!-- 显示名和安装名可能不一样（200 条样本里 5 条），装进去的是这个。 -->
            <code class="tools-cmd">{{ selected.skillId }}</code>
          </div>
          <div class="disc-kv">
            <span class="disc-key">{{ t('tools.discover.detail.installs') }}</span>
            <span>{{ formatInstalls(selected.installs) }}</span>
          </div>
          <!-- 仓库内布局千奇百怪（猜路径只命中 3/7），所以这条必须显示出来。 -->
          <div v-if="preview" class="disc-kv">
            <span class="disc-key">{{ t('tools.discover.detail.path') }}</span>
            <code class="tools-cmd">{{ preview.repoPath }}</code>
          </div>
          <div v-if="preview" class="disc-kv">
            <span class="disc-key">{{ t('tools.discover.detail.commit') }}</span>
            <code class="tools-cmd">{{ preview.commit }}</code>
            <span v-if="preview.cached" class="disc-src">{{ t('tools.discover.detail.cached') }}</span>
          </div>
          <p v-if="preview && otherPathsNote(preview)" class="disc-note">
            {{ otherPathsNote(preview) }}
          </p>
        </div>

        <p v-if="!selected.installable" class="tools-warn">
          {{ t('tools.discover.blockedNote') }}
        </p>

        <!-- 正在从仓库取。首次是浅克隆 + sparse-checkout，3 秒级 —— 右半边空着三秒
             看上去像点了没反应。和本地 Skills 详情同一个骨架。 -->
        <SkillDetailSkeleton v-if="pstate === 'loading'" />

        <div v-else-if="pstate === 'error' && previewError" class="tools-placeholder error disc-error">
          <p>{{ previewErrorText(previewError, selected.skillId) }}</p>
          <details v-if="previewError.detail">
            <summary>{{ t('tools.discover.errDetail') }}</summary>
            <code>{{ previewError.detail }}</code>
          </details>
          <button
            v-if="selected.installable"
            type="button"
            class="tools-act"
            @click="refreshPreview"
          >
            <IconRefresh />
            {{ t('tools.discover.retry') }}
          </button>
        </div>

        <template v-else-if="preview">
          <!-- 风险点。和本地 Skills 详情同一个组件、后端同一份 describe_body ——
               装之前看到的和装完看到的必须是一回事。 -->
          <SkillFindings
            :findings="preview.findings"
            :truncated="preview.truncated"
            :heading="t('tools.skills.findings', { n: String(preview.findings.length) })"
          />

          <div class="tools-section">
            <h4>{{ t('tools.skills.frontmatter') }}</h4>
            <template v-if="preview.frontmatter">
              <div v-if="preview.frontmatter.name" class="disc-kv">
                <span class="disc-key">name</span><span>{{ preview.frontmatter.name }}</span>
              </div>
              <div v-if="preview.frontmatter.description" class="disc-kv">
                <span class="disc-key">description</span>
                <span>{{ preview.frontmatter.description }}</span>
              </div>
              <div v-if="preview.frontmatter.allowedTools.length" class="disc-kv">
                <span class="disc-key">allowed-tools</span>
                <span>{{ preview.frontmatter.allowedTools.join(', ') }}</span>
              </div>
              <!-- 认不出来的字段原样显示，不猜也不丢：各家 skill 规范还在变。 -->
              <div v-for="f in preview.frontmatter.extra" :key="f.key" class="disc-kv">
                <span class="disc-key">{{ f.key }}</span><span>{{ f.value }}</span>
              </div>
            </template>
            <p v-else class="disc-note">{{ t('tools.discover.detail.noFrontmatter') }}</p>
          </div>

          <div class="tools-section">
            <h4>{{ t('tools.skills.files', { n: String(preview.files.length) }) }}</h4>
            <div v-for="f in preview.files" :key="f.path" class="disc-file">
              <component :is="fileIconFor(f.path)" />
              <span class="disc-file-name">{{ f.path }}</span>
              <span class="disc-file-size">{{ formatSize(f.bytes) }}</span>
            </div>
            <p v-if="preview.truncated" class="tools-warn">
              {{ t('tools.discover.detail.truncated') }}
            </p>
          </div>
        </template>
      </template>
    </section>
  </div>

  <!-- 挡住屏幕的那一下。前两版（toast / 详情区常驻一行）都漏掉了，因为这一刻用户的
       眼睛在终端里，任何摆在终端外面的提示都在他视线之外。 -->
  <ConfirmModal
    :show="ranAck"
    :title="t('tools.discover.ranTitle')"
    :message="t('tools.discover.ran')"
    :ok-text="t('common.gotIt')"
    :danger="false"
    acknowledge
    @confirm="ackRan"
    @cancel="ackRan"
  />
</template>

<style scoped>
/* 搜索方式（模糊 / 语义）。不是状态角标，所以不用 .tools-chip 那套底色。 */
.disc-kind {
  font-size: 11px;
  color: var(--text-dim);
}
.disc-installs {
  font-variant-numeric: tabular-nums;
  font-size: 11px;
  color: var(--text-dim);
}
.disc-error {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 8px;
}
.disc-error details code {
  display: block;
  margin-top: 4px;
  font-size: 11px;
  word-break: break-all;
}
.disc-error summary {
  cursor: pointer;
  font-size: 11px;
}
.disc-kv {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 3px 0;
  font-size: 12.5px;
}
.disc-key {
  min-width: 92px;
  flex-shrink: 0;
  font-size: 11.5px;
  color: var(--text-mute);
}

/* 终端模式下详情区换一套盒模型：
   - 一列 flex，终端才能吃掉剩下的全部高度；
   - **`overflow: hidden`** —— 滚动归 xterm 自己管。两层都能滚的话外层会把最后一行
     顶出可视区，看上去就是「终端底下被切了一截」；
   - 底部留白从 40px 收回 14px：那 40px 是给正文最后一行留的呼吸位，
     终端的最后一行是活的，留白只会把它挤出去。
   其它状态一概不动。 */
.tools-detail.disc-term-mode {
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding-bottom: 14px;
}

/* 第二行让给描述之后，source 挪到名字右边 —— `.tools-row-name` 是 block，
   不改成一行 flex 的话它会自己掉到下一行去，变成三行一条。 */
.disc-title {
  display: flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
}
.disc-title .tools-row-name {
  flex: 0 1 auto;
}
/* 描述是 frontmatter 里的整段话（实测四行），不夹成一行的话一条结果能占掉半屏，
   列表就没法用眼睛扫了。全文在右边详情里。 */
.tools-row-desc {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 名字右边那一小截：source，以及「来自本地缓存」那种补充说明。 */
.disc-src {
  font-size: 11px;
  color: var(--text-dim);
  white-space: nowrap;
}
.disc-desc {
  margin: 8px 0 0;
  font-size: 12.5px;
  line-height: 1.55;
  color: var(--text-mute);
}
.disc-note {
  margin: 6px 0 0;
  font-size: 11.5px;
  color: var(--text-dim);
}
.disc-file {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 3px 0;
  font-size: 12px;
}
.disc-file :deep(svg) {
  width: 13px;
  height: 13px;
  flex-shrink: 0;
  color: var(--text-dim);
}
.disc-file-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.disc-file-size {
  margin-left: auto;
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
  font-size: 11px;
  color: var(--text-dim);
}
</style>
