<script setup lang="ts">
// 添加 / 编辑一条 MCP server。
//
// 只收 stdio：远端 server 各家的 URL 键不一样（agy 的 streamable-http 用 `httpUrl`，
// `url` 在它那儿专指 SSE），没逐家实测前不写 —— 读是全支持的，这里不给编辑入口，
// 比写一个半对的键进去强。
//
// 命令收成**一整行**而不是「命令 + 参数数组」两栏：用户手上现成的就是一行
// `npx -y foo@latest`，拆成两栏等于让他自己去数哪个是第一个词。
import { computed, ref, watch } from 'vue'
import type { Agent, KeyValue, McpScan, McpServerInput } from '../types'
import { t } from '../i18n'
import { shortenPath } from '../toolsSkills'
import { panelAgents } from '../toolsPanel'
import { commandLine, parseCommandLine, validName, writePath } from '../toolsMcp'
import { agentLabel } from '../agentMeta'
import { agentIcons, IconCheck, IconClose, IconPlus } from '../components/icons'

const props = defineProps<{
  show: boolean
  scan: McpScan | null
  /** 编辑时的原名。null = 新增。 */
  editing: string | null
  /** 预填的定义。 */
  initial: McpServerInput | null
  /** 预勾选的 agent。 */
  initialAgents: Agent[]
  busy: boolean
}>()

const emit = defineEmits<{
  (e: 'submit', name: string, def: McpServerInput, agents: Agent[]): void
  (e: 'cancel'): void
}>()

const name = ref('')
const command = ref('')
const cwd = ref('')
const env = ref<KeyValue[]>([])
const picked = ref<Set<Agent>>(new Set())

watch(
  () => [props.show, props.editing, props.initial] as const,
  () => {
    if (!props.show) return
    name.value = props.editing ?? ''
    command.value = props.initial ? commandLine(props.initial) : ''
    cwd.value = props.initial?.cwd ?? ''
    env.value = (props.initial?.env ?? []).map((v) => ({ ...v }))
    picked.value = new Set(props.initialAgents)
  },
  { immediate: true },
)

/** 能写的那几家。没有可写文件的（比如 grok 的 server 全来自兼容来源）根本勾不上。 */
const targets = computed(() =>
  panelAgents().map((agent) => ({ agent, path: writePath(props.scan, agent) })),
)

const nameError = computed(() => {
  if (name.value === '') return null
  if (!validName(name.value)) return t('tools.mcp.form.nameInvalid')
  if (props.editing === null && props.scan?.servers.some((s) => s.name === name.value)) {
    return t('tools.mcp.form.nameTaken')
  }
  return null
})

const ready = computed(
  () =>
    name.value !== '' &&
    !nameError.value &&
    parseCommandLine(command.value).command !== null &&
    picked.value.size > 0,
)

function toggle(agent: Agent) {
  const next = new Set(picked.value)
  if (next.has(agent)) next.delete(agent)
  else next.add(agent)
  picked.value = next
}

function addVar() {
  env.value = [...env.value, { key: '', value: '' }]
}

function dropVar(i: number) {
  env.value = env.value.filter((_, at) => at !== i)
}

function submit() {
  if (!ready.value || props.busy) return
  const { command: cmd, args } = parseCommandLine(command.value)
  emit(
    'submit',
    name.value,
    {
      transport: 'stdio',
      command: cmd,
      args,
      url: null,
      // 键为空的那几行是用户加了没填的，别写进配置里。
      env: env.value.filter((v) => v.key !== ''),
      headers: [],
      cwd: cwd.value.trim() === '' ? null : cwd.value.trim(),
    },
    [...picked.value],
  )
}

function short(path: string) {
  return shortenPath(path, props.scan?.home ?? '')
}
</script>

<template>
  <Transition name="fade">
    <div v-if="show" class="app-overlay" @click.self="emit('cancel')">
      <div class="modal tools-form-modal" role="dialog" aria-modal="true">
        <h3>{{ t(editing ? 'tools.mcp.form.editTitle' : 'tools.mcp.form.addTitle') }}</h3>

        <label class="tools-form-row">
          <span class="tools-form-label">{{ t('tools.mcp.form.name') }}</span>
          <input
            v-model.trim="name"
            class="input"
            :disabled="editing !== null"
            :placeholder="t('tools.mcp.form.namePlaceholder')"
            spellcheck="false"
          />
        </label>
        <p v-if="nameError" class="tools-form-error">{{ nameError }}</p>

        <label class="tools-form-row">
          <span class="tools-form-label">{{ t('tools.mcp.form.command') }}</span>
          <input
            v-model="command"
            class="input mono"
            :placeholder="t('tools.mcp.form.commandPlaceholder')"
            spellcheck="false"
          />
        </label>

        <label class="tools-form-row">
          <span class="tools-form-label">{{ t('tools.mcp.form.cwd') }}</span>
          <input
            v-model="cwd"
            class="input mono"
            :placeholder="t('tools.mcp.form.cwdPlaceholder')"
            spellcheck="false"
          />
        </label>

        <div class="tools-form-section">
          <div class="tools-form-section-head">
            <h4>{{ t('tools.mcp.form.env') }}</h4>
            <button type="button" class="mcp-form-add" @click="addVar">
              <IconPlus />
              {{ t('tools.mcp.form.addVar') }}
            </button>
          </div>
          <div v-for="(v, i) in env" :key="i" class="mcp-form-var">
            <input
              v-model.trim="v.key"
              class="input mono"
              :placeholder="t('tools.mcp.form.varKey')"
              spellcheck="false"
            />
            <input
              v-model="v.value"
              class="input mono"
              :placeholder="t('tools.mcp.form.varValue')"
              spellcheck="false"
            />
            <button
              type="button"
              class="mcp-form-drop"
              v-tooltip="t('tools.mcp.form.dropVar')"
              :aria-label="t('tools.mcp.form.dropVar')"
              @click="dropVar(i)"
            >
              <IconClose />
            </button>
          </div>
        </div>

        <div class="tools-form-section">
          <h4>{{ t('tools.mcp.form.targets') }}</h4>
          <button
            v-for="target in targets"
            :key="target.agent"
            type="button"
            class="tools-form-target"
            :class="{ on: picked.has(target.agent), locked: !target.path }"
            :disabled="!target.path"
            v-tooltip="target.path ? short(target.path) : t('tools.mcp.block.noWritableSource')"
            @click="toggle(target.agent)"
          >
            <span class="tools-form-box"><IconCheck v-if="picked.has(target.agent)" /></span>
            <component :is="agentIcons[target.agent]" class="mcp-agent-ic" />
            <span class="tools-form-target-name">{{ agentLabel(target.agent, true) }}</span>
            <span class="tools-form-target-path">
              {{ target.path ? short(target.path) : t('tools.mcp.form.noTarget') }}
            </span>
          </button>
        </div>

        <div class="modal-actions">
          <button class="btn" :disabled="busy" @click="emit('cancel')">
            {{ t('common.cancel') }}
          </button>
          <button
            class="btn primary"
            :class="{ running: busy }"
            :disabled="!ready || busy"
            @click="submit"
          >
            <span v-if="busy" class="chip-spinner" aria-hidden="true" />
            {{ t('tools.mcp.form.save') }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
/* 表单骨架（`.tools-form-modal` / `.tools-form-row` / `.tools-form-section` /
   `.tools-form-target` …）在 style.css，和 Hooks 的添加框共用。这儿只留 MCP 自己的
   环境变量那几行。 */
.mcp-form-add {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11.5px;
  color: var(--text-dim);
  padding: 2px 7px;
  border-radius: 6px;
}
.mcp-form-add:hover {
  background: var(--surface-hover);
  color: var(--text);
}
.mcp-form-add :deep(svg) {
  width: 12px;
  height: 12px;
}
.mcp-form-var {
  display: flex;
  gap: 6px;
  margin-bottom: 5px;
}
.mcp-form-var .input:first-child {
  width: 40%;
}
.mcp-form-var .input:nth-child(2) {
  flex: 1;
  min-width: 0;
}
.mcp-form-drop {
  flex-shrink: 0;
  width: 26px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 6px;
  color: var(--text-mute);
}
.mcp-form-drop:hover {
  background: var(--surface-hover);
  color: var(--danger);
}
.mcp-form-drop :deep(svg) {
  width: 13px;
  height: 13px;
}
</style>
