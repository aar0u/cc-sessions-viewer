<script setup lang="ts">
// 工具管理 · MCP 面板（只读）。
//
// 面板本体只管渲染和取数，判断全在 `src/toolsMcp.ts` —— `src/views/` 在
// `vitest.config.ts` 是覆盖率排除的，写进这儿的逻辑没人测得到。
//
// 写入一律先 dry-run 出计划、弹框确认了才真跑（`propose`），和 Skills 面板同一条
// 规矩 —— 改的是用户全机器的 agent 配置文件。
import { computed, nextTick, ref, watch } from 'vue'
import { revealSelected } from '../listScroll'
import { highlightSegments } from '../format'
import type {
  Agent,
  McpEdit,
  McpEntry,
  McpScan,
  McpServerDef,
  McpServerInput,
  McpVar,
  McpWriteReport,
} from '../types'
import * as api from '../api'
import { t } from '../i18n'
import { shortenPath } from '../toolsSkills'
import {
  panelAgents,
  selectFirstRow,
  shownOfTotal,
  startToolsListResize,
  toolsQuery,
} from '../toolsPanel'
import {
  CONTEXT_BUDGET,
  MCP_STATES,
  agentReach,
  budgetLevel,
  budgetRatio,
  commandLine,
  mcpState,
  primaryDef,
  queryCommand,
  shadowedDefs,
  stateCounts,
  toggleState,
  urlDisplay,
  urlHasSecret,
  varDisplay,
  visibleServers,
  defToInput,
  enableEdits,
  formEdits,
  removeEdits,
  mcpApplyOutcome,
  mcpPlanView,
  syncEdit,
  syncState,
  touchedAgents,
  writePath,
  type McpFilter,
} from '../toolsMcp'
import { agentLabel } from '../agentMeta'
import {
  agentIcons,
  IconCheck,
  IconClose,
  IconFolder,
  IconPencil,
  IconPlus,
  IconRefresh,
  IconTrash,
} from '../components/icons'
import ToolsPlanModal from '../modals/ToolsPlanModal.vue'
import McpServerModal from '../modals/McpServerModal.vue'

const props = defineProps<{ cwd?: string }>()
const emit = defineEmits<{ (e: 'notify', msg: string, error?: boolean): void }>()

const scan = ref<McpScan | null>(null)
const loading = ref(false)
const error = ref('')
const selectedName = ref<string | null>(null)
const filter = ref<McpFilter>({ state: null })
/**
 * 点开了哪几个凭据。key 是 `server\u0000变量名`。
 *
 * 每次重新扫描都清空：一次「看一眼」不该一直把 token 摊在屏幕上，尤其是用户可能已经
 * 走开了。
 */
const revealed = ref<Set<string>>(new Set())
/**
 * 哪条 server 的地址点开了。存名字而不是布尔：换一条 server 就自动收回去，
 * 不用再为「切换时别忘了复位」留一个 watch。
 */
const urlShown = ref<string | null>(null)

const home = computed(() => scan.value?.home ?? '')
const counts = computed(() => stateCounts(scan.value))
const list = computed(() => visibleServers(scan.value, toolsQuery.value, filter.value))
const selected = computed<McpEntry | null>(
  () => scan.value?.servers.find((s) => s.name === selectedName.value) ?? null,
)
const def = computed(() => (selected.value ? primaryDef(selected.value) : null))

/** 解析失败的文件。一个坏逗号就能让一家 agent 的 server 整片消失，必须说出来。 */
const badSources = computed(() =>
  (scan.value?.agents ?? []).flatMap((a) =>
    a.sources.filter((s) => s.error).map((s) => ({ agent: a.agent, path: s.path, error: s.error })),
  ),
)

/** 预算条只算**生效集合**：关掉的和被盖住的不占上下文。 */
const budget = computed(() => {
  const summary = scan.value?.summary
  const tokens = summary?.tokens ?? 0
  return {
    tokens,
    tools: summary?.tools ?? 0,
    level: budgetLevel(tokens),
    percent: Math.round(budgetRatio(tokens) * 1000) / 10,
    width: `${budgetRatio(tokens) * 100}%`,
    // 没量到工具数的那几条不在上面的数字里。不说清楚，这条子就是在低报。
    unmeasured: (summary?.active ?? 0) - (summary?.measured ?? 0),
  }
})

function short(path: string) {
  return shortenPath(path, home.value)
}

async function load() {
  loading.value = true
  error.value = ''
  revealed.value = new Set()
  urlShown.value = null
  try {
    scan.value = await api.toolsScanMcp(props.cwd)
    if (selectedName.value && !scan.value.servers.some((s) => s.name === selectedName.value)) {
      selectedName.value = null
    }
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

watch(() => props.cwd, () => void load(), { immediate: true })

function select(name: string) {
  selectedName.value = selectedName.value === name ? null : name
}

selectFirstRow(() => list.value, () => selectedName.value !== null, (s) => select(s.name))

const listEl = ref<HTMLElement>()

/** 把命中的那几个字切出来 —— 和会话列表 / 回收站用的是同一个 `.kw-hit`。 */
function hl(text: string) {
  return highlightSegments(text, toolsQuery.value)
}

// 列表换了一批（改过滤器、清搜索词、重新扫描），把选中那行重新滚进视野：先筛窄、
// 点中一条、再把筛选关掉，那一条会落回全量列表的中段 —— 右边详情画着它，左边却
// 一眼找不着。看得见就不动（`revealSelected` 自己判）。
watch(list, async () => {
  await nextTick()
  revealSelected(listEl.value, '.tools-row.active')
})


function varKey(server: string, v: McpVar) {
  return `${server}\u0000${v.key}`
}

function toggleUrl(server: string, d: McpServerDef) {
  if (!urlHasSecret(d)) return
  urlShown.value = urlShown.value === server ? null : server
}

function toggleReveal(server: string, v: McpVar) {
  if (!v.secret) return
  const next = new Set(revealed.value)
  const key = varKey(server, v)
  if (next.has(key)) next.delete(key)
  else next.add(key)
  revealed.value = next
}

function isRevealed(server: string, v: McpVar) {
  return revealed.value.has(varKey(server, v))
}

async function reveal(path: string) {
  try {
    await api.revealInFinder(path)
  } catch (e) {
    emit('notify', String(e), true)
  }
}

// ---------------------------------------------------------------------------
// 写：一律先 dry-run
// ---------------------------------------------------------------------------

const busy = ref(false)
const plan = ref<{ title: string; report: McpWriteReport; edits: McpEdit[] } | null>(null)
const form = ref<{ editing: string | null; initial: McpServerInput | null; agents: Agent[] } | null>(
  null,
)

const planView = computed(() =>
  plan.value ? mcpPlanView(plan.value.title, plan.value.report, home.value) : null,
)

/**
 * 跑一次 dry-run，有步骤就弹确认框。
 *
 * 一步都排不出来时**不当作成功**：把 `blocked` 里的原因直接报给用户 —— 「点了没反应」
 * 正是这类面板最容易掉进去的坑。
 */
async function propose(title: string, edits: McpEdit[]) {
  if (busy.value || edits.length === 0) return
  busy.value = true
  try {
    const report = await api.toolsApplyMcp(edits, props.cwd, true, [])
    if (report.steps.length === 0) {
      emit(
        'notify',
        report.blocked.length > 0
          ? t(`tools.mcp.block.${report.blocked[0].reason}`, {
              path: report.blocked[0].path ? short(report.blocked[0].path) : '',
            })
          : t('tools.mcp.plan.nothing'),
        report.blocked.length > 0,
      )
      return
    }
    plan.value = { title, report, edits }
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

/**
 * 真跑。把 dry-run 那份指纹原样交回去 —— 确认框开着的时候别处改过同一个文件，
 * 后端就拒了，不会按一份过期的计划往下写。
 *
 * 出错时**照样把计划撤掉并重扫**：多文件不是原子的，可能已经写进去几个了，留着
 * 旧计划让人再点一次就是在一份已经变了的磁盘上按旧结论办事。
 */
async function applyPlan() {
  const current = plan.value
  if (!current || busy.value) return
  busy.value = true
  try {
    const done = await api.toolsApplyMcp(current.edits, props.cwd, false, current.report.stamps)
    plan.value = null
    form.value = null
    await load()
    const out = mcpApplyOutcome(done, home.value)
    emit('notify', out.msg, out.error)
  } catch (e) {
    plan.value = null
    await load()
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

/** 勾选框：勾上 = 写进这家的可写文件，取消 = 从那个文件里移除。 */
function toggleSync(entry: McpEntry, agent: Agent) {
  if (!scan.value) return
  const edit = syncEdit(entry, scan.value, agent)
  if (!edit) {
    emit('notify', t('tools.mcp.block.noWritableSource'), true)
    return
  }
  void propose(
    t(edit.op === 'drop' ? 'tools.mcp.action.unsync' : 'tools.mcp.action.sync', {
      agent: agentLabel(agent, true),
    }),
    [edit],
  )
}

function setEnabled(entry: McpEntry, on: boolean) {
  if (!scan.value) return
  void propose(t(on ? 'tools.mcp.action.enable' : 'tools.mcp.action.disable'), enableEdits(entry, scan.value, on))
}

function removeServer(entry: McpEntry) {
  if (!scan.value) return
  void propose(t('tools.mcp.action.remove'), removeEdits(entry, scan.value))
}

function openAdd() {
  form.value = { editing: null, initial: null, agents: [] }
}

function openEdit(entry: McpEntry) {
  const def = primaryDef(entry)
  if (!def || !scan.value) return
  form.value = {
    editing: entry.name,
    initial: defToInput(def),
    // 只预勾选**可写文件里真有它**的那几家。按「沾过它」预勾会把一条来自兼容来源
    // 的定义画成「已在 grok」，保存时就真的往 grok 自己的配置里写了一份用户从没要过的。
    agents: touchedAgents(entry).filter((a) => syncState(entry, scan.value, a) === 'checked'),
  }
}

function submitForm(name: string, def: McpServerInput, agents: Agent[]) {
  const editing = form.value?.editing
  const entry = editing ? (scan.value?.servers.find((s) => s.name === editing) ?? null) : null
  void propose(
    t(editing ? 'tools.mcp.form.editTitle' : 'tools.mcp.form.addTitle'),
    formEdits(name, def, agents, entry, scan.value),
  )
}

/** 这条 server 在这家 agent 里的一句话解释，给勾选矩阵当 tooltip。 */
function reachTip(entry: McpEntry, agent: Agent): string {
  const reach = agentReach(entry, agent)
  if (reach.state === 'absent') return t('tools.mcp.reach.absentTip')
  const path = short(reach.def.source.path)
  return reach.state === 'off'
    ? t('tools.mcp.reach.offTip', { path })
    : t('tools.mcp.reach.onTip', { path })
}
</script>

<template>
  <!-- 健康条：状态角标 + 预算条。预算条是这个面板的主角，所以给它整半边。 -->
  <div class="list-head tools-health">
    <template v-if="scan">
      <span
        class="tools-health-total"
        v-tooltip="list.length === scan.summary.servers
          ? ''
          : t('tools.total.filtered', {
            shown: String(list.length),
            total: String(scan.summary.servers),
          })"
      >
        {{ t('tools.mcp.total', { n: shownOfTotal(list.length, scan.summary.servers) }) }}
      </span>
      <button
        v-for="state in MCP_STATES"
        :key="state"
        type="button"
        class="tools-chip"
        :class="[state, { active: filter.state === state, zero: counts[state] === 0 }]"
        v-tooltip="t(`tools.mcp.stateTip.${state}`)"
        @click="filter = toggleState(filter, state)"
      >
        {{ t(`tools.mcp.state.${state}`) }}
        <b>{{ counts[state] }}</b>
      </button>

      <span class="tools-gap" />

      <!-- 上下文预算。没有友商做这条 —— 「一键装一切」的商店只会把这个问题放大。 -->
      <div class="mcp-budget" :class="budget.level">
        <span class="mcp-budget-label">{{ t('tools.mcp.budget.title') }}</span>
        <span class="mcp-budget-track">
          <span class="mcp-budget-fill" :style="{ width: budget.width }" />
        </span>
        <span
          class="mcp-budget-value"
          v-tooltip="budget.unmeasured > 0
            ? t('tools.mcp.budget.unmeasured', { n: String(budget.unmeasured) })
            : t('tools.mcp.budget.tip', { tools: String(budget.tools) })"
        >
          {{ t('tools.mcp.budget.value', {
            tokens: String(budget.tokens),
            total: String(CONTEXT_BUDGET / 1000),
            percent: String(budget.percent),
          }) }}
          <template v-if="budget.unmeasured > 0">＋</template>
        </span>
      </div>
    </template>
    <span v-else-if="loading" class="tools-health-empty">{{ t('tools.mcp.loading') }}</span>

    <button
      type="button"
      class="tools-icon-btn"
      :disabled="loading || busy"
      v-tooltip="t('tools.mcp.action.add')"
      :aria-label="t('tools.mcp.action.add')"
      @click="openAdd"
    >
      <IconPlus />
    </button>
    <button
      type="button"
      class="tools-icon-btn"
      :disabled="loading"
      v-tooltip="t('tools.mcp.refresh')"
      @click="load"
    >
      <IconRefresh />
    </button>
  </div>

  <div class="tools-body">
    <!-- 列表 -->
    <section ref="listEl" class="tools-list" :aria-label="t('tools.listPane')">
      <p v-if="error" class="tools-placeholder error">{{ error }}</p>
      <p v-else-if="loading && !scan" class="tools-placeholder">{{ t('tools.mcp.loading') }}</p>
      <p v-else-if="list.length === 0" class="tools-placeholder">{{ t('tools.mcp.empty') }}</p>
      <div v-else class="tools-list-inner">
        <button
          v-for="s in list"
          :key="s.name"
          type="button"
          class="tools-row"
          :class="{ active: s.name === selectedName }"
          @click="select(s.name)"
        >
          <span class="tools-row-top">
            <span class="tools-row-title">
              <span class="tools-row-name"><span
                v-for="(seg, i) in hl(s.name)"
                :key="i"
                :class="{ 'kw-hit': seg.hit }"
              >{{ seg.text }}</span></span>
            </span>
            <span class="tools-row-meta">
              <component
                :is="agentIcons[a]"
                v-for="a in s.agents"
                :key="a"
                class="mcp-agent-ic"
              />
              <span
                v-if="mcpState(s) !== 'on'"
                class="tools-badge"
                :class="mcpState(s)"
              >{{ t(`tools.mcp.state.${mcpState(s)}`) }}</span>
            </span>
          </span>
          <!-- 第二行是「它到底是什么」：传输 + 工具数 + token。没量到工具数的显示
               「未测量」而不是 0 —— 0 会被当成「这个 server 不提供工具」。
               搜索只命中命令行时，第二行改成那条命令行 —— 不然这一行没有任何东西
               能解释它为什么在结果里。 -->
          <span v-if="queryCommand(s, toolsQuery)" class="tools-row-desc mcp-row-cmd"><span
            v-for="(seg, i) in hl(queryCommand(s, toolsQuery)!)"
            :key="i"
            :class="{ 'kw-hit': seg.hit }"
          >{{ seg.text }}</span></span>
          <span v-else class="tools-row-desc">
            <span class="mcp-transport">{{ t(`tools.mcp.transport.${primaryDef(s)?.transport ?? 'unknown'}`) }}</span>
            <template v-if="s.tools !== null">
              · {{ t('tools.mcp.toolCount', { n: String(s.tools) }) }}
              · {{ t('tools.mcp.tokenApprox', { n: String(s.tokens ?? 0) }) }}
            </template>
            <template v-else>· {{ t('tools.mcp.unmeasured') }}</template>
          </span>
        </button>
      </div>
    </section>

    <div
      class="tools-resizer"
      role="separator"
      aria-orientation="vertical"
      @pointerdown="startToolsListResize"
    />

    <!-- 详情 -->
    <section class="tools-detail" :aria-label="t('tools.detailPane')">
      <p v-if="!selected || !def" class="tools-placeholder">{{ t('tools.noSelection') }}</p>
      <template v-else>
        <header class="tools-detail-head">
          <h3>{{ selected.name }}</h3>
          <span
            v-if="mcpState(selected) !== 'on'"
            class="tools-badge"
            :class="mcpState(selected)"
          >{{ t(`tools.mcp.state.${mcpState(selected)}`) }}</span>
          <span class="tools-gap" />
          <!-- 停用和删除是两个明确不同的操作：停用不丢配置。不支持停用开关的那几家
               由后端报进 blocked，点了会当面说原因，而不是静悄悄什么都不做。 -->
          <button
            type="button"
            class="tools-act"
            :disabled="busy"
            v-tooltip="t('tools.mcp.action.editTip')"
            @click="openEdit(selected)"
          >
            <IconPencil />
            {{ t('tools.mcp.action.edit') }}
          </button>
          <button
            type="button"
            class="tools-act"
            :disabled="busy"
            v-tooltip="t('tools.mcp.action.toggleTip')"
            @click="setEnabled(selected, mcpState(selected) === 'off')"
          >
            <IconCheck v-if="mcpState(selected) === 'off'" />
            <IconClose v-else />
            {{ t(mcpState(selected) === 'off' ? 'tools.mcp.action.enable' : 'tools.mcp.action.disable') }}
          </button>
          <button
            type="button"
            class="tools-act danger"
            :disabled="busy"
            v-tooltip="t('tools.mcp.action.removeTip')"
            @click="removeServer(selected)"
          >
            <IconTrash />
            {{ t('tools.mcp.action.remove') }}
          </button>
        </header>

        <!-- 冲突先说，因为它让下面那份「命令」只是好几份里的一份。 -->
        <p v-if="selected.conflict" class="tools-warn">
          {{ t('tools.mcp.conflictNote', { n: String(selected.fingerprints.length) }) }}
        </p>
        <p v-else-if="def.incomplete" class="tools-warn">{{ t('tools.mcp.incompleteNote') }}</p>

        <div class="tools-section">
          <h4>{{ t('tools.mcp.detail.definition') }}</h4>
          <div class="mcp-kv">
            <span class="mcp-key">{{ t('tools.mcp.detail.transport') }}</span>
            <span>{{ t(`tools.mcp.transport.${def.transport}`) }}</span>
          </div>
          <!-- 地址里带密钥是托管 MCP 的常规写法，所以和环境变量同一个待遇：默认
               打码，点一下才给原文。没东西可打码的地址就是一段普通文本。 -->
          <div v-if="def.url" class="mcp-kv">
            <span class="mcp-key">{{ t('tools.mcp.detail.url') }}</span>
            <code
              class="tools-cmd"
              :class="{ secret: urlHasSecret(def) }"
              :role="urlHasSecret(def) ? 'button' : undefined"
              :tabindex="urlHasSecret(def) ? 0 : undefined"
              v-tooltip="urlHasSecret(def)
                ? t(urlShown === selected.name ? 'tools.mcp.hideUrl' : 'tools.mcp.showUrl')
                : undefined"
              @click="toggleUrl(selected.name, def)"
              @keydown.enter.prevent="toggleUrl(selected.name, def)"
              @keydown.space.prevent="toggleUrl(selected.name, def)"
            >{{ urlDisplay(def, urlShown === selected.name) }}</code>
          </div>
          <div v-else class="mcp-kv">
            <span class="mcp-key">{{ t('tools.mcp.detail.command') }}</span>
            <code class="tools-cmd">{{ commandLine(def) || t('tools.mcp.detail.noCommand') }}</code>
          </div>
          <div v-if="def.cwd" class="mcp-kv">
            <span class="mcp-key">{{ t('tools.mcp.detail.cwd') }}</span>
            <code class="tools-cmd">{{ short(def.cwd) }}</code>
          </div>
          <!-- 像凭据的默认打码。点一下才给原文 —— 面板一打开就把 token 摊在屏幕上，
               用户截个图发出去就泄了。 -->
          <div v-for="v in [...def.env, ...def.headers]" :key="v.key" class="mcp-kv">
            <span class="mcp-key">{{ v.key }}</span>
            <code
              class="tools-cmd"
              :class="{ secret: v.secret }"
              :role="v.secret ? 'button' : undefined"
              :tabindex="v.secret ? 0 : undefined"
              v-tooltip="v.secret
                ? t(isRevealed(selected.name, v) ? 'tools.mcp.hideSecret' : 'tools.mcp.showSecret')
                : undefined"
              @click="toggleReveal(selected.name, v)"
              @keydown.enter.prevent="toggleReveal(selected.name, v)"
              @keydown.space.prevent="toggleReveal(selected.name, v)"
            >{{ varDisplay(v, isRevealed(selected.name, v)) }}</code>
          </div>
        </div>

        <!-- 在哪些 agent 生效。这是整个设计的核心：一眼看出覆盖面。 -->
        <div class="tools-section">
          <h4>{{ t('tools.mcp.detail.reach') }}</h4>
          <p class="tools-note">{{ t('tools.mcp.syncNote') }}</p>
          <div v-for="a in panelAgents()" :key="a" class="tools-reach-row">
            <!-- 勾选框说的是「**我们动得了的那个文件**里有没有它」，右边那枚药丸说的
                 是「跑不跑」。两者常常不一致（跑着的那份来自项目配置），画成一个就会
                 出现「取消勾选却什么都没发生」。 -->
            <button
              type="button"
              class="mcp-sync"
              :class="syncState(selected, scan, a)"
              :disabled="busy || syncState(selected, scan, a) === 'locked'"
              v-tooltip="syncState(selected, scan, a) === 'locked'
                ? t('tools.mcp.block.noWritableSource')
                : t(syncState(selected, scan, a) === 'checked'
                  ? 'tools.mcp.syncOffTip'
                  : 'tools.mcp.syncOnTip', { path: short(writePath(scan, a) ?? '') })"
              :aria-label="agentLabel(a, true)"
              :aria-pressed="syncState(selected, scan, a) === 'checked'"
              @click="toggleSync(selected, a)"
            >
              <span class="mcp-sync-box"><IconCheck v-if="syncState(selected, scan, a) === 'checked'" /></span>
            </button>
            <component :is="agentIcons[a]" class="mcp-agent-ic" />
            <span class="tools-reach-agent">{{ agentLabel(a, true) }}</span>
            <span class="mcp-reach-state" :class="agentReach(selected, a).state" v-tooltip="reachTip(selected, a)">
              {{ t(`tools.mcp.reach.${agentReach(selected, a).state}`) }}
            </span>
            <template v-for="r in [agentReach(selected, a)]" :key="r.state">
              <template v-if="r.state !== 'absent'">
                <span class="tools-reach-path">{{ short(r.def.source.path) }}</span>
                <span class="tools-sub">
                  {{ t(`tools.scope.${r.def.source.scope}`) }}
                  <template v-if="r.def.source.origin !== 'own'">
                    · {{ t(`tools.origin.${r.def.source.origin}`) }}
                  </template>
                  <template v-if="r.def.source.conditional">
                    · {{ t('tools.conditional') }}
                  </template>
                </span>
                <button
                  type="button"
                  class="tools-reveal"
                  v-tooltip="t('list.action.reveal')"
                  :aria-label="t('list.action.reveal')"
                  @click="reveal(r.def.source.path)"
                >
                  <IconFolder />
                </button>
              </template>
            </template>
          </div>
        </div>

        <!-- 被盖住的那些。改了不生效的那份却什么都没发生时，这儿是唯一的解释。 -->
        <div
          v-if="panelAgents().some((a) => shadowedDefs(selected!, a).length > 0)"
          class="tools-section"
        >
          <h4>{{ t('tools.mcp.detail.shadowed') }}</h4>
          <p class="tools-note">{{ t('tools.mcp.shadowedNote') }}</p>
          <template v-for="a in panelAgents()" :key="a">
            <div v-for="d in shadowedDefs(selected, a)" :key="a + d.source.path" class="tools-reach-row">
              <component :is="agentIcons[a]" class="mcp-agent-ic" />
              <span class="mcp-shadow-path">{{ short(d.source.path) }}</span>
              <!-- 命令跟在路径**后面**，不推到行尾：它说的就是这条路径里写着什么，
                   隔半屏空白就得来回对着看是哪一条。 -->
              <code class="tools-cmd dim mcp-shadow-cmd">{{ commandLine(d.def) }}</code>
            </div>
          </template>
        </div>
      </template>
    </section>
  </div>

  <ToolsPlanModal
    :show="plan !== null"
    :plan="planView"
    :busy="busy"
    @confirm="applyPlan"
    @cancel="plan = null"
  />

  <McpServerModal
    :show="form !== null"
    :scan="scan"
    :editing="form?.editing ?? null"
    :initial="form?.initial ?? null"
    :initial-agents="form?.agents ?? []"
    :busy="busy"
    @submit="submitForm"
    @cancel="form = null"
  />

  <!-- 读不了的配置文件。静悄悄少几行是这个面板最坏的失败方式。 -->
  <div v-if="badSources.length > 0" class="tools-bad-sources" role="status">
    <p v-for="b in badSources" :key="b.agent + b.path">
      {{ t('tools.mcp.sourceError', {
        agent: agentLabel(b.agent, true),
        path: short(b.path),
        error: b.error ?? '',
      }) }}
    </p>
  </div>
</template>

<style scoped>
/* 骨架（`.tools-health` / `.tools-body` / `.tools-list` / `.tools-resizer` /
   `.tools-detail` / `.tools-placeholder`）和面板共用的行、角标、小节（`.tools-row` /
   `.tools-chip` / `.tools-badge` / `.tools-section` / `.tools-act` …）都在 style.css。
   这儿只留 MCP 自己的：预算条、勾选框、打码的值、覆盖链那几行。 */
.tools-chip.conflict.active,
.tools-chip.incomplete.active {
  border-color: var(--danger);
  color: var(--danger);
}
.tools-badge.conflict,
.tools-badge.incomplete {
  background: var(--danger-soft);
  color: var(--danger);
}
.mcp-key {
  min-width: 92px;
  flex-shrink: 0;
  font-size: 11.5px;
  color: var(--text-mute);
}

.mcp-budget {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11.5px;
  color: var(--text-mute);
  white-space: nowrap;
}
.mcp-budget-track {
  width: 132px;
  height: 6px;
  border-radius: 999px;
  background: var(--surface-active);
  overflow: hidden;
  flex-shrink: 0;
}
.mcp-budget-fill {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: var(--text-mute);
  transition: width 0.25s ease;
}
/* 条子的颜色就是结论本身：绿说「随便用」，黄红说「该关几个了」。 */
.mcp-budget.warn .mcp-budget-fill {
  background: var(--status-blocked);
}
.mcp-budget.danger .mcp-budget-fill {
  background: var(--status-error);
}
.mcp-budget.warn .mcp-budget-value,
.mcp-budget.danger .mcp-budget-value {
  color: var(--text);
}
.mcp-budget-value {
  font-variant-numeric: tabular-nums;
  cursor: default;
}

.mcp-transport {
  text-transform: uppercase;
  letter-spacing: 0.02em;
}

/* 搜索命中命令行时顶上来的那一行。命令行可以很长，必须单行省略。 */
.mcp-row-cmd {
  display: block;
  font-family: var(--font-mono, ui-monospace, monospace);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.mcp-kv {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 3px 0;
  font-size: 12.5px;
}
/* 打码的值是可点的，所以要长得像可点的。 */
.tools-cmd.secret {
  cursor: pointer;
  border-bottom: 1px dashed var(--line-strong);
}
.tools-cmd.secret:hover {
  color: var(--text);
}
/* 勾选框。真方框不是圆点 —— 圆的会被读成单选。描边用 `--text-mute`：`--line-strong`
   在部分主题里是 12% 不透明度的结构线，14px 的小方框上根本看不见。 */
.mcp-sync {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 2px;
  border-radius: 5px;
}
.mcp-sync:hover:not(:disabled) .mcp-sync-box {
  border-color: var(--text);
}
.mcp-sync.locked {
  opacity: 0.35;
  cursor: default;
}
.mcp-sync-box {
  width: 14px;
  height: 14px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1.5px solid var(--text-mute);
  border-radius: 4px;
  color: transparent;
}
.mcp-sync.checked .mcp-sync-box {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--bg);
}
.mcp-sync-box :deep(svg) {
  width: 11px;
  height: 11px;
  stroke-width: 3.2;
}
.mcp-sync:focus-visible {
  outline: none;
}
.mcp-sync:focus-visible .mcp-sync-box {
  box-shadow: 0 0 0 2px var(--surface), 0 0 0 3.5px var(--accent);
}
.mcp-reach-state {
  font-size: 11.5px;
  color: var(--text-mute);
  cursor: default;
}
.mcp-reach-state.on {
  color: var(--text-dim);
}
.mcp-shadow-path {
  flex-shrink: 0;
  color: var(--text-dim);
}
.mcp-shadow-cmd {
  flex: 1;
  min-width: 0;
}
</style>
