<script setup lang="ts">
// 添加一条 hook。
//
// 没有「编辑」：hook 的身份就是那条命令，改命令等于换一条。给一个编辑框会让人以为
// 「改完还是同一条」，而落盘的动作其实是删旧的、写新的 —— 那两步在受保护 / 只读来源
// 上的结果完全不同（旧的删不掉，新的写进去了，于是两条都在跑）。要改就删了重加。
//
// 事件是**多选**：同一条命令挂在 Stop 和 SessionEnd 上是常态，一次一个会让人点四遍。
import { computed, ref, watch } from 'vue'
import type { Agent, HookScan } from '../types'
import { t } from '../i18n'
import { shortenPath } from '../toolsSkills'
import { panelAgents } from '../toolsPanel'
import { agentSupports, canWrite, validCommand, writePath } from '../toolsHooks'
import { agentLabel } from '../agentMeta'
import { agentIcons, IconCheck } from '../components/icons'

const props = defineProps<{ show: boolean; scan: HookScan | null; busy: boolean }>()

const emit = defineEmits<{
  (
    e: 'submit',
    payload: {
      command: string
      events: string[]
      matcher: string | null
      timeout: number | null
      agents: Agent[]
    },
  ): void
  (e: 'cancel'): void
}>()

const command = ref('')
const matcher = ref('')
const timeout = ref('')
const events = ref<Set<string>>(new Set())
const picked = ref<Set<Agent>>(new Set())

watch(
  () => props.show,
  (show) => {
    if (!show) return
    command.value = ''
    matcher.value = ''
    timeout.value = ''
    events.value = new Set()
    picked.value = new Set()
  },
  { immediate: true },
)

/** 全量事件目录，各自标着几家支持。这儿**不筛**「已配置的」—— 添加就是要看全的。 */
const catalog = computed(() => props.scan?.events ?? [])

/** 能写的那几家。没有可写文件的（没装 / 没这个机制）根本勾不上。 */
const targets = computed(() =>
  panelAgents().map((agent) => ({
    agent,
    path: writePath(props.scan, agent),
    writable: canWrite(props.scan, agent),
  })),
)

/**
 * 勾上的 agent 里，有哪几家不认某个勾上的事件。
 *
 * 提前说出来而不是等后端报 `unknownEvent`：那时候用户已经点过确认了，而计划里躺着
 * 一半能做一半不能做，比一开始就说清楚难懂得多。
 */
const unsupported = computed(() => {
  const out: string[] = []
  for (const agent of picked.value) {
    const miss = [...events.value].filter((e) => !agentSupports(props.scan, agent, e))
    if (miss.length > 0) out.push(`${agentLabel(agent, true)}：${miss.join('、')}`)
  }
  return out
})

/**
 * 保存按钮为什么是灰的。
 *
 * 灰按钮 + 没有解释就是「点了没反应」的另一种写法 —— 尤其这张表有三个必填项，
 * 差哪一个从按钮上看不出来。
 */
const missing = computed(() => {
  if (!validCommand(command.value)) return t('tools.hooks.form.needCommand')
  if (events.value.size === 0) return t('tools.hooks.form.needEvent')
  if (picked.value.size === 0) return t('tools.hooks.form.needAgent')
  return null
})

const ready = computed(() => missing.value === null)

function toggleEvent(name: string) {
  const next = new Set(events.value)
  if (next.has(name)) next.delete(name)
  else next.add(name)
  events.value = next
}

function toggleAgent(agent: Agent) {
  const next = new Set(picked.value)
  if (next.has(agent)) next.delete(agent)
  else next.add(agent)
  picked.value = next
}

function submit() {
  if (!ready.value || props.busy) return
  const secs = Number(timeout.value)
  emit('submit', {
    command: command.value.trim(),
    events: [...events.value],
    matcher: matcher.value.trim() === '' ? null : matcher.value.trim(),
    // 不是数字就当没填 —— 猜一个数写进配置比留空危险得多。
    timeout: timeout.value.trim() !== '' && Number.isFinite(secs) ? secs : null,
    agents: [...picked.value],
  })
}

function short(path: string) {
  return shortenPath(path, props.scan?.home ?? '')
}
</script>

<template>
  <Transition name="fade">
    <div v-if="show" class="app-overlay" @click.self="emit('cancel')">
      <div class="modal tools-form-modal" role="dialog" aria-modal="true">
        <h3>{{ t('tools.hooks.form.title') }}</h3>

        <label class="tools-form-row">
          <span class="tools-form-label">{{ t('tools.hooks.form.command') }}</span>
          <input
            v-model="command"
            class="input mono"
            :placeholder="t('tools.hooks.form.commandPlaceholder')"
            spellcheck="false"
          />
        </label>
        <p class="hook-form-hint">{{ t('tools.hooks.form.commandHint') }}</p>

        <label class="tools-form-row">
          <span class="tools-form-label">{{ t('tools.hooks.form.matcher') }}</span>
          <input
            v-model="matcher"
            class="input mono"
            :placeholder="t('tools.hooks.form.matcherPlaceholder')"
            spellcheck="false"
          />
        </label>
        <p class="hook-form-hint">{{ t('tools.hooks.form.matcherHint') }}</p>

        <label class="tools-form-row">
          <span class="tools-form-label">{{ t('tools.hooks.form.timeout') }}</span>
          <input
            v-model="timeout"
            class="input mono"
            inputmode="numeric"
            :placeholder="t('tools.hooks.form.timeoutPlaceholder')"
            spellcheck="false"
          />
        </label>
        <p class="hook-form-hint">{{ t('tools.hooks.form.timeoutHint') }}</p>

        <div class="tools-form-section">
          <h4>{{ t('tools.hooks.form.events') }}</h4>
          <p class="hook-form-hint">{{ t('tools.hooks.form.eventsHint') }}</p>
          <div class="hook-form-events">
            <button
              v-for="e in catalog"
              :key="e.name"
              type="button"
              class="hook-form-event"
              :class="{ on: events.has(e.name) }"
              :aria-pressed="events.has(e.name)"
              v-tooltip="t('tools.hooks.form.eventAgents', { n: String(e.agents.length) })"
              @click="toggleEvent(e.name)"
            >
              {{ e.name }}
            </button>
          </div>
        </div>

        <div class="tools-form-section">
          <h4>{{ t('tools.hooks.form.agents') }}</h4>
          <button
            v-for="target in targets"
            :key="target.agent"
            type="button"
            class="tools-form-target"
            :class="{ on: picked.has(target.agent), locked: !target.writable }"
            :disabled="!target.writable"
            v-tooltip="target.writable
              ? t('tools.hooks.writeTo', { path: short(target.path ?? '') })
              : t('tools.hooks.noWritePath')"
            @click="toggleAgent(target.agent)"
          >
            <span class="tools-form-box"><IconCheck v-if="picked.has(target.agent)" /></span>
            <component :is="agentIcons[target.agent]" class="mcp-agent-ic" />
            <span class="tools-form-target-name">{{ agentLabel(target.agent, true) }}</span>
            <span class="tools-form-target-path">
              {{ target.writable ? short(target.path ?? '') : t('tools.hooks.form.noTarget') }}
            </span>
          </button>
        </div>

        <!-- 勾了但那家不认的事件。等后端报回来就晚了 —— 那时候计划已经半好半坏。 -->
        <p v-for="line in unsupported" :key="line" class="tools-form-error">
          {{ t('tools.hooks.block.unknownEvent') }} · {{ line }}
        </p>

        <div class="modal-actions">
          <span v-if="missing" class="hook-form-missing">{{ missing }}</span>
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
            {{ t('tools.hooks.form.save') }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
/* 表单骨架（`.tools-form-modal` / `.tools-form-row` / `.tools-form-section` /
   `.tools-form-target` …）在 style.css，和 MCP 的添加框共用。这儿只留 hook 自己的
   事件多选和那几行说明。 */
.hook-form-hint {
  margin: -4px 0 10px 78px;
  font-size: 11px;
  color: var(--text-mute);
  line-height: 1.6;
}
.tools-form-section .hook-form-hint {
  margin-left: 0;
  margin-top: 0;
}
.hook-form-events {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
}
.hook-form-event {
  padding: 3px 9px;
  border-radius: 999px;
  border: 1px solid var(--border);
  font-size: 11.5px;
  color: var(--text-dim);
  transition: background 0.12s, border-color 0.12s, color 0.12s;
}
.hook-form-event:hover {
  background: var(--surface-hover);
}
.hook-form-event.on {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--bg);
}
/* 差哪一项就写在按钮左边。`.modal-actions` 是 flex + 靠右，`margin-right: auto`
   把这句话推到最左，两个按钮留在原位。 */
.hook-form-missing {
  margin-right: auto;
  font-size: 11.5px;
  color: var(--text-mute);
}
</style>
