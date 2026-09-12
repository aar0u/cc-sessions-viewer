<script setup lang="ts">
// 工具管理 · Hooks 面板。
//
// 判断全在 `src/toolsHooks.ts` —— `src/views/` 在 `vitest.config.ts` 是覆盖率排除的。
//
// 和 MCP 面板同一条规矩：写入一律先 dry-run 出计划、弹框确认了才真跑。多两件事：
//
// 1. **回合信号受保护**。本 app 自己装的那一条横跨五家 agent，删了 GUI 聊天就收不到
//    回合结束事件。这儿只给一个「去设置里重置」的指路，不重复开第二个装卸入口 ——
//    前端拦一道、后端 `hooks_write::apply` 的第一行再拦一道。
// 2. **试跑**。hook 装上去之后只在真实回合里触发，写错了表现成「agent 那边好像卡了
//    一下」。所以给一个拿假事件真跑一遍的入口，把喂进去的 JSON 和输出都摊开。
import { computed, nextTick, ref, watch } from 'vue'
import { revealSelected } from '../listScroll'
import { highlightSegments } from '../format'
import type { Agent, HookEdit, HookEntry, HookScan, HookWriteReport } from '../types'
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
  HOOK_STATES,
  addEdits,
  agentEvents,
  agentSupported,
  blankFilter,
  byAgent,
  canRemove,
  canTest,
  commandHead,
  hookKinds,
  hookPlanView,
  hookState,
  removeAgentEdits,
  removeEdits,
  sourceErrors,
  sourcePaths,
  stateCounts,
  testEvent,
  toggleEvent,
  toggleState,
  visibleHooks,
  writePath,
  type HookFilter,
} from '../toolsHooks'
import { agentLabel } from '../agentMeta'
import {
  agentIcons,
  IconFolder,
  IconPlay,
  IconPlus,
  IconRefresh,
  IconSettings,
  IconTrash,
} from '../components/icons'
import ToolsPlanModal from '../modals/ToolsPlanModal.vue'
import HookFormModal from '../modals/HookFormModal.vue'
import HookTestModal from '../modals/HookTestModal.vue'

const props = defineProps<{ cwd?: string }>()
const emit = defineEmits<{
  (e: 'notify', msg: string, error?: boolean): void
  (e: 'open-settings'): void
}>()

const scan = ref<HookScan | null>(null)
const loading = ref(false)
const error = ref('')
const selectedKey = ref<string | null>(null)
const filter = ref<HookFilter>(blankFilter())

const home = computed(() => scan.value?.home ?? '')
const counts = computed(() => stateCounts(scan.value?.hooks ?? []))
const list = computed(() => visibleHooks(scan.value, toolsQuery.value, filter.value))
const selected = computed<HookEntry | null>(
  () => scan.value?.hooks.find((h) => h.fingerprint === selectedKey.value) ?? null,
)
const badSources = computed(() => sourceErrors(scan.value))

/** 健康条上那排事件角标：只列已经配了 hook 的。全量目录在「添加」里。 */
const configuredEvents = computed(() => (scan.value?.events ?? []).filter((e) => e.configured > 0))

function short(path: string) {
  return shortenPath(path, home.value)
}

async function load() {
  loading.value = true
  error.value = ''
  try {
    scan.value = await api.toolsScanHooks(props.cwd)
    if (selectedKey.value && !scan.value.hooks.some((h) => h.fingerprint === selectedKey.value)) {
      selectedKey.value = null
    }
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

watch(() => props.cwd, () => void load(), { immediate: true })

function select(key: string) {
  selectedKey.value = selectedKey.value === key ? null : key
}

selectFirstRow(() => list.value, () => selectedKey.value !== null, (h) => select(h.fingerprint))

const listEl = ref<HTMLElement>()

/** 把命中的那几个字切出来 —— 和会话列表 / 回收站用的是同一个 `.kw-hit`。 */
function hl(text: string) {
  return highlightSegments(text, toolsQuery.value)
}

/** 事件名是一颗颗药丸，整颗染色比在里面切片段干净。 */
function eventHit(event: string): boolean {
  const q = toolsQuery.value.trim().toLowerCase()
  return q !== '' && event.toLowerCase().includes(q)
}

// 列表换了一批（改过滤器、清搜索词、重新扫描），把选中那行重新滚进视野：先筛窄、
// 点中一条、再把筛选关掉，那一条会落回全量列表的中段 —— 右边详情画着它，左边却
// 一眼找不着。看得见就不动（`revealSelected` 自己判）。
watch(list, async () => {
  await nextTick()
  revealSelected(listEl.value, '.tools-row.active')
})


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
const plan = ref<{ title: string; report: HookWriteReport; edits: HookEdit[] } | null>(null)
const form = ref(false)

const planView = computed(() =>
  plan.value ? hookPlanView(plan.value.title, plan.value.report, home.value) : null,
)

/**
 * 跑一次 dry-run，有步骤就弹确认框。
 *
 * 一步都排不出来时**不当作成功**：把 `blocked` 里的第一条原因直接报给用户。「点了没
 * 反应」正是这类面板最容易掉进去的坑 —— 尤其是受保护和「不在可写文件里」这两种。
 */
async function propose(title: string, edits: HookEdit[]) {
  if (busy.value || edits.length === 0) return
  busy.value = true
  try {
    const report = await api.toolsApplyHooks(edits, props.cwd, true)
    if (report.steps.length === 0) {
      emit(
        'notify',
        report.blocked.length > 0
          ? t(`tools.hooks.block.${report.blocked[0].reason}`, {
              path: report.blocked[0].path ? short(report.blocked[0].path) : '',
            })
          : t('tools.hooks.plan.nothing'),
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

async function applyPlan() {
  const current = plan.value
  if (!current || busy.value) return
  busy.value = true
  try {
    const done = await api.toolsApplyHooks(current.edits, props.cwd, false)
    plan.value = null
    form.value = false
    await load()
    emit('notify', t('tools.hooks.plan.applied', { n: String(done.steps.length) }))
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

/** 从我们能写的每个文件里摘掉这一行。受保护的那条连按钮都不给。 */
function removeHook(entry: HookEntry) {
  void propose(t('tools.hooks.plan.removeTitle'), removeEdits(entry))
}

/** 详情表里单独摘掉一家。 */
function removeOne(entry: HookEntry, agent: Agent) {
  void propose(t('tools.hooks.plan.removeTitle'), removeAgentEdits(entry, agent))
}

function submitForm(payload: {
  command: string
  events: string[]
  matcher: string | null
  timeout: number | null
  agents: Agent[]
}) {
  void propose(
    t('tools.hooks.plan.addTitle'),
    addEdits(payload.command, payload.events, payload.matcher, payload.timeout, payload.agents),
  )
}

// ---------------------------------------------------------------------------
// 试跑
// ---------------------------------------------------------------------------

const testing = ref<{ command: string; event: string } | null>(null)

function openTest(entry: HookEntry) {
  testing.value = {
    command: entry.command,
    event: testEvent(entry, scan.value?.events[0]?.name ?? 'Stop'),
  }
}
</script>

<template>
  <!-- 健康条：状态角标 + 已配置的事件。事件那一排是这个面板的主角 —— 「哪些时刻会
       有东西插进来」比「一共几条」重要得多。 -->
  <div class="list-head tools-health">
    <template v-if="scan">
      <span
        class="tools-health-total"
        v-tooltip="list.length === scan.summary.hooks
          ? ''
          : t('tools.total.filtered', {
            shown: String(list.length),
            total: String(scan.summary.hooks),
          })"
      >
        {{ t('tools.hooks.total', { n: shownOfTotal(list.length, scan.summary.hooks) }) }}
      </span>
      <span class="hook-defs" v-tooltip="t('tools.hooks.defsTip')">
        {{ t('tools.hooks.defs', { n: String(scan.summary.defs) }) }}
      </span>
      <button
        v-for="state in HOOK_STATES"
        :key="state"
        type="button"
        class="tools-chip"
        :class="[state, { active: filter.state === state, zero: counts[state] === 0 }]"
        v-tooltip="t(`tools.hooks.stateTip.${state}`)"
        @click="filter = toggleState(filter, state)"
      >
        {{ t(`tools.hooks.state.${state}`) }}
        <b>{{ counts[state] }}</b>
      </button>

      <span class="tools-gap" />

      <div class="hook-events" v-tooltip="t('tools.hooks.eventsTip')">
        <span class="hook-events-label">{{ t('tools.hooks.events') }}</span>
        <button
          v-for="e in configuredEvents"
          :key="e.name"
          type="button"
          class="hook-event"
          :class="{ active: filter.event === e.name }"
          @click="filter = toggleEvent(filter, e.name)"
        >
          {{ e.name }}
          <b>{{ e.configured }}</b>
        </button>
      </div>
    </template>
    <span v-else-if="loading" class="tools-health-empty">{{ t('tools.hooks.loading') }}</span>

    <button
      type="button"
      class="tools-icon-btn"
      :disabled="loading || busy"
      v-tooltip="t('tools.hooks.action.add')"
      :aria-label="t('tools.hooks.action.add')"
      @click="form = true"
    >
      <IconPlus />
    </button>
    <button
      type="button"
      class="tools-icon-btn"
      :disabled="loading"
      v-tooltip="t('tools.hooks.refresh')"
      @click="load"
    >
      <IconRefresh />
    </button>
  </div>

  <div class="tools-body">
    <!-- 列表 -->
    <section ref="listEl" class="tools-list" :aria-label="t('tools.listPane')">
      <p v-if="error" class="tools-placeholder error">{{ error }}</p>
      <p v-else-if="loading && !scan" class="tools-placeholder">{{ t('tools.hooks.loading') }}</p>
      <p v-else-if="list.length === 0" class="tools-placeholder">{{ t('tools.hooks.empty') }}</p>
      <div v-else class="tools-list-inner">
        <button
          v-for="h in list"
          :key="h.fingerprint"
          type="button"
          class="tools-row"
          :class="{ active: h.fingerprint === selectedKey }"
          @click="select(h.fingerprint)"
        >
          <span class="tools-row-top">
            <span class="tools-row-title">
              <span class="tools-row-name"><span
                v-for="(seg, i) in hl(commandHead(h.command))"
                :key="i"
                :class="{ 'kw-hit': seg.hit }"
              >{{ seg.text }}</span></span>
            </span>
            <span class="tools-row-meta">
              <component :is="agentIcons[a]" v-for="a in h.agents" :key="a" class="mcp-agent-ic" />
              <span
                v-if="hookState(h) !== 'on'"
                class="tools-badge"
                :class="hookState(h)"
              >{{ t(`tools.hooks.state.${hookState(h)}`) }}</span>
            </span>
          </span>
          <!-- 第二行是「它什么时候跑」。事件比命令参数重要 —— 命令在详情里给全的。 -->
          <span class="tools-row-desc hook-row-events">
            <span
              v-for="e in h.events"
              :key="e"
              class="hook-tag"
              :class="{ 'kw-hit': eventHit(e) }"
            >{{ e }}</span>
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
      <p v-if="!selected" class="tools-placeholder">{{ t('tools.noSelection') }}</p>
      <template v-else>
        <header class="tools-detail-head">
          <h3>{{ commandHead(selected.command) }}</h3>
          <span
            v-if="hookState(selected) !== 'on'"
            class="tools-badge"
            :class="hookState(selected)"
          >{{ t(`tools.hooks.state.${hookState(selected)}`) }}</span>
          <span class="tools-gap" />
          <!-- prompt / url 型的那串字不是拿去过 shell 的，试跑对它没有意义，所以
               这儿是禁用 + 说清为什么，而不是照跑一遍给出一个假结果。 -->
          <button
            type="button"
            class="tools-act"
            :disabled="busy || !canTest(selected)"
            v-tooltip="canTest(selected)
              ? t('tools.hooks.action.testTip')
              : t('tools.hooks.action.testKind', { kind: hookKinds(selected).join('、') })"
            @click="openTest(selected)"
          >
            <IconPlay />
            {{ t('tools.hooks.action.test') }}
          </button>
          <!-- 受保护的那条这儿换成指路，不是把删除按钮画灰：灰按钮只说「不行」，
               不说「那该去哪儿」。 -->
          <button
            v-if="selected.managed"
            type="button"
            class="tools-act"
            v-tooltip="t('tools.hooks.action.resetTip')"
            @click="emit('open-settings')"
          >
            <IconSettings />
            {{ t('tools.hooks.action.reset') }}
          </button>
          <!-- 只存在于项目配置 / 只读来源里的那些一条改动都下不出来。不禁用的话点下去
               是彻底的没反应 —— 那比一个灰按钮糟得多。 -->
          <button
            v-else
            type="button"
            class="tools-act danger"
            :disabled="busy || !canRemove(selected)"
            v-tooltip="canRemove(selected)
              ? t('tools.hooks.action.removeTip')
              : t('tools.hooks.action.removeLocked', {
                  path: sourcePaths(selected).map(short).join('、'),
                })"
            @click="removeHook(selected)"
          >
            <IconTrash />
            {{ t('tools.hooks.action.remove') }}
          </button>
        </header>

        <p v-if="selected.managed" class="tools-warn">{{ t('tools.hooks.managedNote') }}</p>

        <div class="tools-section">
          <h4>{{ t('tools.hooks.detail.command') }}</h4>
          <!-- 完整命令，不截断：列表标题已经是缩写了，这儿是唯一能看全的地方。 -->
          <code class="tools-cmd hook-full-cmd">{{ selected.command }}</code>
        </div>

        <!-- 落点表。一行一条真实的配置条目，因为「同一条命令在 claude 上挂 Stop、在
             codex 上挂 Stop + PreToolUse」这种差异只有摊开才看得出来。 -->
        <div class="tools-section">
          <h4>{{ t('tools.hooks.detail.reach') }}</h4>
          <template v-for="row in byAgent(selected)" :key="row.agent">
            <div class="hook-agent-head">
              <component :is="agentIcons[row.agent]" class="mcp-agent-ic" />
              <span class="tools-reach-agent">{{ agentLabel(row.agent, true) }}</span>
              <span class="hook-agent-events">{{ agentEvents(selected, row.agent).join(' · ') }}</span>
              <span class="tools-gap" />
              <button
                v-if="!selected.managed && row.hooks.some((h) => h.source.writable)"
                type="button"
                class="tools-act"
                :disabled="busy"
                v-tooltip="t('tools.hooks.action.removeOne')"
                :aria-label="t('tools.hooks.action.removeOne')"
                @click="removeOne(selected, row.agent)"
              >
                <IconTrash />
              </button>
            </div>
            <div v-for="(at, i) in row.hooks" :key="row.agent + i" class="tools-reach-row hook-def">
              <span class="hook-tag">{{ at.def.event }}</span>
              <span class="hook-matcher">
                {{ at.def.matcher ?? t('tools.hooks.matcherAll') }}
              </span>
              <span v-if="at.def.kind !== 'command'" class="tools-badge">
                {{ t('tools.hooks.detail.kind', { kind: at.def.kind }) }}
              </span>
              <span v-if="!at.def.enabled" class="tools-badge off">
                {{ t('tools.hooks.disabledTag') }}
              </span>
              <!-- 单位不统一（有的秒有的毫秒），所以只报数字 —— 但光一个 `10` 读不出
                   是什么，得带上「超时」两个字。 -->
              <span
                v-if="at.def.timeout !== null"
                class="tools-sub"
                v-tooltip="t('tools.hooks.timeoutTip')"
              >{{ t('tools.hooks.timeoutValue', { n: String(at.def.timeout) }) }}</span>
              <span class="tools-gap" />
              <span class="tools-reach-path">{{ short(at.source.path) }}</span>
              <span class="tools-sub">
                {{ t(`tools.scope.${at.source.scope}`) }}
                <template v-if="at.source.origin !== 'own'">
                  · {{ t(`tools.origin.${at.source.origin}`) }}
                </template>
                <template v-if="at.source.conditional">· {{ t('tools.conditional') }}</template>
              </span>
              <button
                type="button"
                class="tools-reveal"
                v-tooltip="t('list.action.reveal')"
                :aria-label="t('list.action.reveal')"
                @click="reveal(at.source.path)"
              >
                <IconFolder />
              </button>
            </div>
          </template>
        </div>

        <!-- 没挂上的那几家。空着不说，用户会以为「这条已经到处都装好了」。 -->
        <div
          v-if="panelAgents().some((a) => !selected!.agents.includes(a))"
          class="tools-section"
        >
          <h4>{{ t('tools.hooks.detail.missing') }}</h4>
          <div
            v-for="a in panelAgents().filter((x) => !selected!.agents.includes(x))"
            :key="a"
            class="tools-reach-row"
          >
            <component :is="agentIcons[a]" class="mcp-agent-ic" />
            <span class="tools-reach-agent">{{ agentLabel(a, true) }}</span>
            <!-- 「这家没有 hook 这个机制」和「有但我们写不进去」是两回事，混成一句
                 会让用户以为 opencode 只是缺个配置文件。 -->
            <span class="tools-sub">
              {{ !agentSupported(scan, a)
                ? t('tools.hooks.notSupported')
                : writePath(scan, a)
                  ? t('tools.hooks.writeTo', { path: short(writePath(scan, a) ?? '') })
                  : t('tools.hooks.noWritePath') }}
            </span>
          </div>
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

  <HookFormModal
    :show="form"
    :scan="scan"
    :busy="busy"
    @submit="submitForm"
    @cancel="form = false"
  />

  <HookTestModal
    :show="testing !== null"
    :command="testing?.command ?? ''"
    :event="testing?.event ?? ''"
    :scan="scan"
    :cwd="cwd"
    @close="testing = null"
  />

  <!-- 读不了的配置文件。静悄悄少几条是这个面板最坏的失败方式。 -->
  <div v-if="badSources.length > 0" class="tools-bad-sources" role="status">
    <p v-for="s in badSources" :key="s.path">
      {{ t('tools.hooks.sourceError', {
        agent: agentLabel(s.agent, true),
        path: short(s.path),
        error: s.error,
      }) }}
    </p>
  </div>
</template>

<style scoped>
/* 列表 / 详情 / 角标那套骨架（`.tools-row` / `.tools-chip` / `.tools-section` …）在
   style.css，MCP 面板和这儿共用 —— 两个面板长得就该是一样的。这儿只写 hook 独有的
   几条：事件角标、落点行、完整命令。
   注意：能被 `<component :is>` 渲染出来的类（`.mcp-agent-ic`）一律不能写在这儿 ——
   scoped 属性打不到子组件的根节点上，规则会静默失效。 */
.hook-defs {
  /* 和 `.tools-health-total` 一样不换行。健康条是一条 flex，窄下来时这一项会被压到
     文字宽度以下，于是「36 处落点」当场断成两行 —— 而旁边那一项是 nowrap 的，
     两行一高一低更难看。挤出去的那部分交给 `.hook-events` 吸收，它本来就是
     `min-width: 0` + `overflow-x: auto`。 */
  flex-shrink: 0;
  white-space: nowrap;
  font-size: 11.5px;
  color: var(--text-mute);
}
/* 事件常有十几个，健康条里塞不下。横向滚 + 右边一道渐隐 —— 硬切一半的
   「SessionSta」读起来像渲染坏了，渐隐读起来才是「右边还有」。 */
.hook-events {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
  overflow-x: auto;
  scrollbar-width: none;
  -webkit-mask-image: linear-gradient(to right, #000 calc(100% - 24px), transparent);
  mask-image: linear-gradient(to right, #000 calc(100% - 24px), transparent);
  padding-right: 8px;
}
.hook-events::-webkit-scrollbar {
  display: none;
}
.hook-events-label {
  flex-shrink: 0;
  font-size: 11.5px;
  color: var(--text-mute);
}
.hook-event {
  flex-shrink: 0;
  padding: 2px 7px;
  border-radius: 999px;
  border: 1px solid var(--border);
  font-size: 11px;
  color: var(--text-dim);
  transition: background 0.12s, border-color 0.12s;
}
.hook-event:hover {
  background: var(--surface-hover);
}
.hook-event.active {
  border-color: var(--accent);
  color: var(--text);
}
.hook-event b {
  margin-left: 4px;
  font-weight: 600;
  color: var(--text-mute);
}

.hook-row-events {
  display: flex;
  flex-wrap: wrap;
  gap: 3px;
}
.hook-tag {
  padding: 1px 6px;
  border-radius: 4px;
  background: var(--surface-2);
  font-size: 10.5px;
  color: var(--text-dim);
  white-space: nowrap;
}
/* 药丸整颗染色。全局 `.kw-hit` 的背景在这儿赢不了 —— scoped 选择器多带一个属性，
   特异性高一档，所以得在这儿显式写一遍。 */
.hook-tag.kw-hit {
  background: rgba(255, 213, 79, 0.55);
  color: var(--text);
}
:root.theme-dark .hook-tag.kw-hit {
  background: rgba(255, 213, 79, 0.32);
}

.hook-full-cmd {
  display: block;
  white-space: pre-wrap;
  word-break: break-all;
}

.hook-agent-head {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 10px;
  padding-bottom: 2px;
}
.hook-agent-head:first-of-type {
  margin-top: 0;
}
.hook-agent-events {
  font-size: 11px;
  color: var(--text-mute);
}
.hook-def {
  padding-left: 14px;
}
.hook-matcher {
  min-width: 0;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11.5px;
  color: var(--text-mute);
}
</style>
