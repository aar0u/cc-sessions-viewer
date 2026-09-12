<script setup lang="ts">
// 配置集（方案 阶段 10）：把一套配置打成一个 JSON，或者把别人的那个装进来。
//
// **导入不走一条新的后端命令。** 包里的条目在 `toolsBundle.ts` 里翻成
// `McpEdit` / `HookEdit` / 一次 `toolsWriteMemo`，再走那三条已经有校验、dry-run、
// 备份和回读的路。判断本身全在那个纯逻辑模块里（有测试），这儿只管画和串。
//
// 两件事是这个弹框自己的：
//
// 1. **确认只有一次。** 三段写入并成一个 `ToolsPlanModal`。分三次确认的话，用户在
//    第二次上点取消时前面那一批已经落盘了 —— 他以为自己取消了整件事。
// 2. **装给哪几家由用户勾。** 方案 2.5 的教训：市面上的 MCP 管理器几乎都是"改一处
//    无条件同步到所有客户端"，而多 agent 场景下这是错的 —— 用户很可能只想让 Codex
//    加载某个重型 MCP。默认勾的是"包里提到、本机也认得"的那几家，不是全选。
import { computed, ref, watch } from 'vue'
import * as api from '../api'
import { t } from '../i18n'
import { ALL_AGENTS } from '../settings'
import { agentLabel } from '../agentMeta'
import { TAB_LABEL } from '../toolsPanel'
import { elidePath } from '../format'
import { shortenPath } from '../toolsSkills'
import { blankRevision } from '../toolsMemo'
import {
  ALL_INCLUDED,
  BUNDLE_CATEGORIES,
  BUNDLE_VERSION,
  bundleCounts,
  bundleFileName,
  bundleHookEdits,
  bundleIsEmpty,
  bundleKey,
  bundleMcpEdits,
  bundleAgentNames,
  bundleMemoTargets,
  bundlePlanView,
  bundleText,
  clearedValues,
  defaultPicks,
  isAgent,
  memoPickable,
  needsAgents,
  filterBundle,
  missingSecrets,
  readBundle,
  redactedArgs,
  suggestedAgents,
  type BundleCategory,
  type BundleProblem,
  type MemoTarget,
} from '../toolsBundle'
import type {
  Agent,
  Bundle,
  BundleInclude,
  HookEdit,
  McpEdit,
  McpFileStamp,
  McpScan,
  MemoScan,
} from '../types'
import { mcpApplyOutcome } from '../toolsMcp'
import ToolsPlanModal from './ToolsPlanModal.vue'
import { IconClose } from '../components/icons'

const props = defineProps<{ show: boolean; cwd?: string }>()
const emit = defineEmits<{
  (e: 'close'): void
  (e: 'notify', msg: string, error?: boolean): void
  /** 真的写下去了 —— 面板要重扫。 */
  (e: 'imported'): void
}>()

const mode = ref<'export' | 'import'>('export')
const busy = ref(false)

// ---------------------------------------------------------------------------
// 导出
// ---------------------------------------------------------------------------

/** 全量扫一次就够。四个勾选框在前端生效，不为了改一个勾再扫一遍磁盘。 */
const full = ref<Bundle | null>(null)
const loading = ref(false)
const include = ref<BundleInclude>({ ...ALL_INCLUDED })

const outgoing = computed(() => (full.value ? filterBundle(full.value, include.value) : null))
const outCounts = computed(() => (full.value ? bundleCounts(full.value) : null))

async function loadExport() {
  loading.value = true
  try {
    full.value = await api.toolsExportBundle(props.cwd, ALL_INCLUDED)
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    loading.value = false
  }
}

async function save() {
  const b = outgoing.value
  if (!b || busy.value) return
  if (bundleIsEmpty(b)) {
    emit('notify', t('tools.bundle.export.empty'), true)
    return
  }
  busy.value = true
  try {
    const { save: saveDialog } = await import('@tauri-apps/plugin-dialog')
    const chosen = await saveDialog({
      defaultPath: bundleFileName(new Date()),
      filters: [{ name: 'JSON', extensions: ['json'] }],
    })
    if (!chosen) return
    const at = await api.writeFile(chosen, bundleText(b))
    emit('notify', t('tools.bundle.export.saved', { path: at }))
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

// ---------------------------------------------------------------------------
// 导入
// ---------------------------------------------------------------------------

const incoming = ref<Bundle | null>(null)
const problem = ref<BundleProblem | null>(null)
const problemVersion = ref(0)
const memoScan = ref<MemoScan | null>(null)
/** 本机现在配着的 MCP。只用来算"这次导入会清掉哪些值"，不参与写入。 */
const mcpScan = ref<McpScan | null>(null)
const picked = ref<Set<string>>(new Set())
const targetAgents = ref<Set<Agent>>(new Set())

const memoTargets = computed<MemoTarget[]>(() =>
  incoming.value && memoScan.value ? bundleMemoTargets(incoming.value, memoScan.value) : [],
)
const inCounts = computed(() => (incoming.value ? bundleCounts(incoming.value) : null))
const agentList = computed(() => ALL_AGENTS.filter((a) => targetAgents.value.has(a)))
/** 导出那台机器上跑着哪几家。认得的换成人话，不认得的原样列出来。 */
const sourceAgents = computed(() =>
  incoming.value
    ? bundleAgentNames(incoming.value)
        .map((a) => (isAgent(a) ? agentLabel(a) : a))
        .join(' · ')
    : '',
)

const pickedMcp = computed(() =>
  incoming.value ? bundleMcpEdits(incoming.value, picked.value, agentList.value) : [],
)
const pickedHooks = computed(() =>
  incoming.value ? bundleHookEdits(incoming.value, picked.value, agentList.value) : [],
)
const pickedMemo = computed(() => memoTargets.value.filter((m) => picked.value.has(m.key)))
const secrets = computed(() =>
  incoming.value ? missingSecrets(incoming.value, picked.value) : [],
)
const maskedArgs = computed(() => (incoming.value ? redactedArgs(incoming.value, picked.value) : []))
const cleared = computed(() =>
  incoming.value
    ? clearedValues(incoming.value, picked.value, agentList.value, mcpScan.value)
    : [],
)

function toggle(key: string) {
  const next = new Set(picked.value)
  if (next.has(key)) next.delete(key)
  else next.add(key)
  picked.value = next
}

function toggleAgent(agent: Agent) {
  const next = new Set(targetAgents.value)
  if (next.has(agent)) next.delete(agent)
  else next.add(agent)
  targetAgents.value = next
  // 取消勾一家，跟着它走的那几份全局指令也要一起松开 —— 留着的话计划框里会冒出
  // 一条用户刚刚明确说了不要的写入。
  const still = new Set(picked.value)
  for (const m of memoTargets.value) {
    if (!memoPickable(m, agentList.value)) still.delete(m.key)
  }
  picked.value = still
}

async function choose() {
  if (busy.value) return
  busy.value = true
  try {
    const { open: openDialog } = await import('@tauri-apps/plugin-dialog')
    const chosen = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    })
    if (typeof chosen !== 'string') return
    const read = readBundle(await api.toolsReadBundle(chosen))
    problem.value = read.problem
    problemVersion.value = read.version ?? 0
    incoming.value = read.bundle
    if (!read.bundle) return
    // 全局指令要落在**本机**哪个路径上，得先知道本机的形状。
    memoScan.value = await api.toolsScanMemo()
    mcpScan.value = await api.toolsScanMcp(props.cwd)
    const named = suggestedAgents(read.bundle)
    targetAgents.value = new Set(named)
    // 默认勾哪几条全局指令要看勾了哪几家，所以 agent 得先定下来。
    picked.value = defaultPicks(read.bundle, bundleMemoTargets(read.bundle, memoScan.value), named)
  } catch (e) {
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

// ---------------------------------------------------------------------------
// 写：一律先 dry-run，三段并成一次确认
// ---------------------------------------------------------------------------

const plan = ref<{
  mcp: McpEdit[]
  hooks: HookEdit[]
  memo: MemoTarget[]
  /** 预览时各个 MCP 配置文件的样子，点确认时原样交回后端核对。 */
  stamps: McpFileStamp[]
  view: ReturnType<typeof bundlePlanView>
} | null>(null)

async function propose() {
  if (busy.value || !incoming.value) return
  if (agentList.value.length === 0 && needsAgents(incoming.value, picked.value)) {
    emit('notify', t('tools.bundle.import.noAgents'), true)
    return
  }
  const mcp = pickedMcp.value
  const hooks = pickedHooks.value
  const memo = pickedMemo.value
  if (mcp.length === 0 && hooks.length === 0 && memo.length === 0) {
    emit('notify', t('tools.bundle.import.nothing'), true)
    return
  }
  busy.value = true
  try {
    const mcpReport = mcp.length > 0 ? await api.toolsApplyMcp(mcp, props.cwd, true, []) : null
    const hookReport = hooks.length > 0 ? await api.toolsApplyHooks(hooks, props.cwd, true) : null
    const view = bundlePlanView(mcpReport, hookReport, memo, memoScan.value?.home ?? '')
    if (view.rows.length === 0) {
      emit('notify', view.blocked[0] ?? t('tools.bundle.import.nothing'), true)
      return
    }
    plan.value = { mcp, hooks, memo, stamps: mcpReport?.stamps ?? [], view }
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
    let n = 0
    if (current.mcp.length > 0) {
      // 把预览时那份指纹交回去。预览之后被别处改过的话后端会拒 —— 和下面全局指令
      // 那一段是同一条规矩。
      const done = await api.toolsApplyMcp(current.mcp, props.cwd, false, current.stamps)
      if (done.failed) throw new Error(mcpApplyOutcome(done, memoScan.value?.home ?? '').msg)
      n += done.steps.length
    }
    if (current.hooks.length > 0) {
      n += (await api.toolsApplyHooks(current.hooks, props.cwd, false)).steps.length
    }
    // 全局指令没有批量写入：一次一个文件，每次都把**刚读到的**指纹交回去。那台机器
    // 上的文件在预览之后被别处改过的话，后端会拒 —— 这正是要的。
    for (const m of current.memo) {
      if (!m.path) continue
      const before = m.kind === 'overwrite' ? (await api.toolsReadMemo(m.path)).revision : blankRevision()
      await api.toolsWriteMemo(m.path, m.text, before)
      n += 1
    }
    plan.value = null
    emit('notify', t('tools.bundle.import.done', { n: String(n) }))
    emit('imported')
    emit('close')
  } catch (e) {
    // 中途出错时**把计划撤掉**，不是留着让人再点一次。前面几步可能已经落盘了，
    // 拿同一份旧计划重跑就是在一份已经变了的磁盘上按旧结论办事。撤掉之后用户点
    // 「导入」会重新 dry-run：做成了的那几条自然排不出步骤，剩下的照样列出来。
    plan.value = null
    emit('notify', String(e), true)
  } finally {
    busy.value = false
  }
}

// ---------------------------------------------------------------------------

function reset() {
  mode.value = 'export'
  include.value = { ...ALL_INCLUDED }
  full.value = null
  incoming.value = null
  problem.value = null
  memoScan.value = null
  picked.value = new Set()
  targetAgents.value = new Set()
  plan.value = null
}

watch(
  () => props.show,
  (on) => {
    if (!on) {
      reset()
      return
    }
    void loadExport()
  },
  { immediate: true },
)

/** 类别名直接借 tab 的 —— 四类和四个面板是一一对应的，另起一套名字只会对不上。 */
function catLabel(c: BundleCategory): string {
  return t(TAB_LABEL[c])
}
</script>

<template>
  <Transition name="fade">
    <div v-if="show" class="app-overlay" @click.self="emit('close')">
      <div class="modal bundle-modal" role="dialog" aria-modal="true">
        <div class="bundle-head">
          <h3>{{ t('tools.bundle.title') }}</h3>
          <div class="bundle-modes">
            <button
              v-for="m in (['export', 'import'] as const)"
              :key="m"
              class="tools-chip"
              :class="{ active: mode === m }"
              @click="mode = m"
            >
              {{ t(`tools.bundle.mode.${m}`) }}
            </button>
          </div>
          <button class="ct-btn" v-tooltip="t('tools.bundle.close')" @click="emit('close')">
            <IconClose />
          </button>
        </div>

        <!-- ================================ 导出 ================================ -->
        <div v-if="mode === 'export'" class="bundle-body">
          <p class="bundle-intro">{{ t('tools.bundle.export.intro') }}</p>

          <p v-if="loading" class="bundle-note">{{ t('tools.bundle.loading') }}</p>
          <template v-else-if="outCounts">
            <label v-for="c in BUNDLE_CATEGORIES" :key="c" class="bundle-cat">
              <input v-model="include[c]" type="checkbox" />
              <span class="bundle-cat-name">{{ catLabel(c) }}</span>
              <span class="bundle-cat-n">{{ t('tools.bundle.count', { n: String(outCounts[c]) }) }}</span>
            </label>

            <!-- 这两句都必须在**勾选框旁边**说，不是事后在别处解释。
                 全局指令那条尤其要说：这个模块对 MCP 的凭据抠到了"一个值都不导出"，
                 而全局指令是整篇原文照搬的 —— 两头的松紧差这么多，不写出来的人
                 会默认"它也帮我处理过了"。 -->
            <p v-if="include.memo" class="bundle-note">{{ t('tools.bundle.memo.note') }}</p>
            <p v-if="include.skills" class="bundle-note">{{ t('tools.bundle.skills.note') }}</p>

            <div class="bundle-section">
              <h4>{{ t('tools.bundle.redacted.title') }}</h4>
              <p class="bundle-note">{{ t('tools.bundle.redacted.why') }}</p>
              <p v-if="!outgoing || outgoing.redacted.length === 0" class="bundle-note">
                {{ t('tools.bundle.redacted.none') }}
              </p>
              <ul v-else class="bundle-keys">
                <li v-for="r in outgoing.redacted" :key="r"><code>{{ r }}</code></li>
              </ul>
            </div>
          </template>
        </div>

        <!-- ================================ 导入 ================================ -->
        <div v-else class="bundle-body">
          <div class="bundle-pick">
            <button class="btn" :disabled="busy" @click="choose">
              {{ t('tools.bundle.import.pick') }}
            </button>
            <span v-if="incoming" class="bundle-from">
              {{ t('tools.bundle.import.from', { app: incoming.app || '?' }) }}
            </span>
          </div>

          <p v-if="problem === 'tooNew'" class="bundle-bad">
            {{ t('tools.bundle.problem.tooNew', { n: String(problemVersion), cur: String(BUNDLE_VERSION) }) }}
          </p>
          <p v-else-if="problem" class="bundle-bad">{{ t(`tools.bundle.problem.${problem}`) }}</p>

          <template v-if="incoming && inCounts">
            <p v-if="bundleIsEmpty(incoming)" class="bundle-note">
              {{ t('tools.bundle.import.empty') }}
            </p>

            <template v-else>
              <!-- 装给哪几家。方案 2.5：同步必须按 agent 勾，不能是全局广播。 -->
              <div class="bundle-section">
                <h4>{{ t('tools.bundle.import.agents') }}</h4>
                <!-- 默认勾的就是这几家。不写出来的话，用户改完勾选就再也想不起来
                     原样是什么了。 -->
                <p v-if="sourceAgents" class="bundle-note">
                  {{ t('tools.bundle.item.agents', { list: sourceAgents }) }}
                </p>
                <div class="bundle-agents">
                  <button
                    v-for="a in ALL_AGENTS"
                    :key="a"
                    class="tools-chip"
                    :class="{ active: targetAgents.has(a) }"
                    @click="toggleAgent(a)"
                  >
                    {{ agentLabel(a) }}
                  </button>
                </div>
              </div>

              <div v-if="inCounts.mcp > 0" class="bundle-section">
                <h4>{{ catLabel('mcp') }}</h4>
                <label v-for="(s, i) in incoming.mcp" :key="i" class="bundle-item">
                  <input type="checkbox" :checked="picked.has(bundleKey('mcp', i))" @change="toggle(bundleKey('mcp', i))" />
                  <span class="bundle-item-name">{{ s.name }}</span>
                  <code class="bundle-item-detail">{{ s.command ?? s.url ?? '—' }}</code>
                </label>
              </div>

              <div v-if="inCounts.hooks > 0" class="bundle-section">
                <h4>{{ catLabel('hooks') }}</h4>
                <label v-for="(h, i) in incoming.hooks" :key="i" class="bundle-item">
                  <input type="checkbox" :checked="picked.has(bundleKey('hooks', i))" @change="toggle(bundleKey('hooks', i))" />
                  <span class="bundle-item-name">{{ h.events.join(' · ') }}</span>
                  <code class="bundle-item-detail">{{ h.command }}</code>
                </label>
              </div>

              <div v-if="inCounts.memo > 0" class="bundle-section">
                <h4>{{ catLabel('memo') }}</h4>
                <label
                  v-for="m in memoTargets"
                  :key="m.key"
                  class="bundle-item"
                  :class="{ blocked: !memoPickable(m, agentList) }"
                >
                  <input
                    type="checkbox"
                    :checked="picked.has(m.key)"
                    :disabled="!memoPickable(m, agentList)"
                    @change="toggle(m.key)"
                  />
                  <span class="bundle-item-name">{{ m.name }}</span>
                  <span class="bundle-tag" :class="m.kind">{{ t(`tools.bundle.memo.${m.kind}`) }}</span>
                  <!-- 片段为什么落在那个目录里，得说一句 —— 不然 `BUNDLE-SELFTEST.md`
                       出现在 `~/.claude/` 下面看着像凭空冒出来的。`isAgent` 那一道不能省：
                       `parent` 是别人机器上写下的，`agentLabel` 查不到就是在
                       `undefined` 上取属性，整个弹框白屏。 -->
                  <span v-if="m.fragment && isAgent(m.agent)" class="bundle-tag">
                    {{ t('tools.bundle.memo.fragment', { agent: agentLabel(m.agent) }) }}
                  </span>
                  <code
                    v-if="m.path"
                    class="bundle-item-detail"
                    v-tooltip="m.path"
                  >
                    {{ elidePath(shortenPath(m.path, memoScan?.home ?? ''), 2) }}
                  </code>
                  <span v-else class="bundle-item-detail warn">
                    {{ t(`tools.bundle.block.${m.reason}`, { agent: m.agent ?? '' }) }}
                  </span>
                </label>
              </div>

              <!-- skill 一条都不装，所以不给勾选框 —— 给了就是在说"这个能装"。 -->
              <div v-if="inCounts.skills > 0" class="bundle-section">
                <h4>{{ catLabel('skills') }}</h4>
                <p class="bundle-note">{{ t('tools.bundle.import.readOnly') }}</p>
                <div v-for="(s, i) in incoming.skills" :key="i" class="bundle-item readonly">
                  <span class="bundle-item-name">{{ s.name }}</span>
                  <code v-if="s.remote" class="bundle-item-detail">{{ s.remote }}</code>
                  <span v-else class="bundle-item-detail">{{ t('tools.bundle.skills.local') }}</span>
                </div>
              </div>

              <!-- 覆盖同名 server 是整条替换，本机那份的 env / header 值会一起没。
                   计划框只画命令行，命令行没变时那一行看着和没动一样 —— 所以这条
                   警告必须在**按下"排一遍"之前**就在这儿。 -->
              <div v-if="cleared.length > 0" class="bundle-section">
                <h4>{{ t('tools.bundle.cleared.title') }}</h4>
                <p class="bundle-note">{{ t('tools.bundle.cleared.why') }}</p>
                <ul class="bundle-keys">
                  <li v-for="k in cleared" :key="k"><code>{{ k }}</code></li>
                </ul>
              </div>

              <div v-if="secrets.length > 0" class="bundle-section">
                <h4>{{ t('tools.bundle.missing.title') }}</h4>
                <p class="bundle-note">{{ t('tools.bundle.missing.why') }}</p>
                <ul class="bundle-keys">
                  <li v-for="k in secrets" :key="k"><code>{{ k }}</code></li>
                </ul>
              </div>

              <div v-if="maskedArgs.length > 0" class="bundle-section">
                <h4>{{ t('tools.bundle.args.title') }}</h4>
                <p class="bundle-note">{{ t('tools.bundle.args.why') }}</p>
                <ul class="bundle-keys">
                  <li v-for="a in maskedArgs" :key="a"><code>{{ a }}</code></li>
                </ul>
              </div>
            </template>
          </template>
        </div>

        <div class="modal-actions">
          <button class="btn" :disabled="busy" @click="emit('close')">
            {{ t('tools.bundle.close') }}
          </button>
          <button
            v-if="mode === 'export'"
            class="btn primary"
            :class="{ running: busy }"
            :disabled="busy || loading || !outgoing"
            @click="save"
          >
            <span v-if="busy" class="chip-spinner" aria-hidden="true" />
            {{ t('tools.bundle.export.save') }}
          </button>
          <button
            v-else
            class="btn primary"
            :class="{ running: busy }"
            :disabled="busy || !incoming"
            @click="propose"
          >
            <span v-if="busy" class="chip-spinner" aria-hidden="true" />
            {{ t('tools.bundle.import.apply') }}
          </button>
        </div>
      </div>
    </div>
  </Transition>

  <ToolsPlanModal
    :show="plan !== null"
    :plan="plan?.view ?? null"
    :busy="busy"
    @confirm="applyPlan"
    @cancel="plan = null"
  />
</template>

<style scoped>
.bundle-modal {
  width: min(760px, calc(100vw - 64px));
  max-height: calc(100vh - 96px);
  display: flex;
  flex-direction: column;
}

.bundle-head {
  display: flex;
  align-items: center;
  gap: 12px;
}
.bundle-head h3 {
  margin: 0;
  flex-shrink: 0;
}
.bundle-modes {
  display: flex;
  gap: 6px;
  margin-left: auto;
}

.bundle-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  margin: 12px 0 4px;
}

.bundle-intro,
.bundle-note {
  margin: 0 0 10px;
  font-size: 12px;
  line-height: 1.7;
  color: var(--text-mute);
}

.bundle-bad {
  margin: 10px 0 0;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--danger, #d24);
}

/* 一整行可点，不是只有那个 13px 的方框。 */
.bundle-cat {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 4px;
  cursor: pointer;
  border-radius: 6px;
}
.bundle-cat:hover {
  background: var(--surface-hover);
}
.bundle-cat-name {
  font-size: 13px;
}
.bundle-cat-n {
  margin-left: auto;
  font-size: 11.5px;
  color: var(--text-mute);
  font-variant-numeric: tabular-nums;
}

.bundle-section {
  margin-top: 16px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}
.bundle-section h4 {
  margin: 0 0 8px;
  font-size: 12px;
  font-weight: 600;
}

.bundle-agents {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.bundle-pick {
  display: flex;
  align-items: center;
  gap: 10px;
}
.bundle-from {
  font-size: 12px;
  color: var(--text-mute);
}

.bundle-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 4px;
  border-radius: 6px;
  cursor: pointer;
  min-width: 0;
}
.bundle-item:hover {
  background: var(--surface-hover);
}
/* 只列不装的那几条不该有"能点"的样子。 */
.bundle-item.readonly {
  cursor: default;
  padding-left: 22px;
}
.bundle-item.readonly:hover {
  background: none;
}
.bundle-item.blocked {
  cursor: not-allowed;
  opacity: 0.62;
}
.bundle-item-name {
  font-size: 12.5px;
  flex-shrink: 0;
}
/* 中段省略（`elidePath`），不是 `direction: rtl` —— 那个会把开头的 `~` 和 `/`
   甩到行尾，`~/.grok/AGENTS.md` 会显示成 `grok/AGENTS.md./~`。实测栽过一次，
   仓库里另外两处早有同样的注释。 */
.bundle-item-detail {
  margin-left: auto;
  font-size: 11px;
  color: var(--text-mute);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  text-align: right;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}
.bundle-item-detail.warn {
  color: var(--warn, #b70);
  font-family: inherit;
}

.bundle-tag {
  flex-shrink: 0;
  font-size: 10.5px;
  padding: 1px 6px;
  border-radius: 999px;
  border: 1px solid var(--border);
  color: var(--text-mute);
}
.bundle-tag.overwrite {
  color: var(--diff-removed);
  border-color: var(--diff-removed);
}

.bundle-keys {
  margin: 0;
  padding-left: 18px;
  font-size: 11.5px;
  line-height: 1.9;
  color: var(--text-mute);
}
</style>
