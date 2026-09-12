<script setup lang="ts">
// 试跑一条 hook。
//
// 这是写 hook 唯一靠谱的验证方式：装上去之后它只在真实回合里触发，出了错的表现是
// 「agent 那边好像卡了一下」，没有任何地方会告诉你是这条命令的锅。
//
// 三件事必须同时摊开：**喂进去的 JSON**（写不对十有八九是以为收到的字段长别的样）、
// stdout / stderr（命令自己说了什么）、退出码（2 是「拦住这一步」，各家都认）。
// 少任何一样，用户就只能靠猜。
import { computed, ref, watch } from 'vue'
import type { HookScan, HookTestResult } from '../types'
import * as api from '../api'
import { t } from '../i18n'

const props = defineProps<{
  show: boolean
  command: string
  /** 打开时预选的事件。 */
  event: string
  scan: HookScan | null
  cwd?: string
}>()

const emit = defineEmits<{ (e: 'close'): void }>()

const picked = ref('')
const running = ref(false)
const result = ref<HookTestResult | null>(null)
const error = ref('')

watch(
  () => [props.show, props.event] as const,
  ([show, event]) => {
    if (!show) return
    picked.value = event
    result.value = null
    error.value = ''
  },
  { immediate: true },
)

/** 能选的事件：全量目录。试跑不该被「这条现在挂在哪儿」限制住。 */
const catalog = computed(() => (props.scan?.events ?? []).map((e) => e.name))

async function run() {
  if (running.value || !picked.value) return
  running.value = true
  error.value = ''
  result.value = null
  try {
    result.value = await api.toolsTestHook(props.command, picked.value, props.cwd)
  } catch (e) {
    error.value = String(e)
  } finally {
    running.value = false
  }
}

/** 退出码那一行：被信号打断时 `exitCode` 是 null，不能显示成 0。 */
const exitLabel = computed(() => {
  const r = result.value
  if (!r) return ''
  return r.exitCode === null
    ? t('tools.hooks.test.killed')
    : t('tools.hooks.test.exit', { code: String(r.exitCode) })
})
</script>

<template>
  <Transition name="fade">
    <div v-if="show" class="app-overlay" @click.self="emit('close')">
      <div class="modal hook-test-modal" role="dialog" aria-modal="true">
        <h3>{{ t('tools.hooks.test.title', { event: picked }) }}</h3>

        <code class="tools-cmd hook-test-cmd">{{ command }}</code>
        <p class="hook-test-note">{{ t('tools.hooks.test.note') }}</p>

        <div class="tools-form-row">
          <span class="tools-form-label">{{ t('tools.hooks.test.event') }}</span>
          <select v-model="picked" class="input">
            <option v-for="name in catalog" :key="name" :value="name">{{ name }}</option>
          </select>
          <button class="btn" :class="{ running }" :disabled="running || !picked" @click="run">
            <span v-if="running" class="chip-spinner" aria-hidden="true" />
            {{ t(running ? 'tools.hooks.test.running' : 'tools.hooks.test.run') }}
          </button>
        </div>

        <p v-if="error" class="tools-form-error">{{ error }}</p>

        <template v-if="result">
          <div class="hook-test-verdict">
            <span class="tools-badge" :class="{ off: result.blocking || result.timedOut }">
              {{ exitLabel }}
            </span>
            <span class="tools-sub">
              {{ t('tools.hooks.test.duration', { ms: String(result.durationMs) }) }}
            </span>
          </div>
          <p v-if="result.timedOut" class="tools-warn">{{ t('tools.hooks.test.timedOut') }}</p>
          <p v-else-if="result.blocking" class="tools-warn">{{ t('tools.hooks.test.blocking') }}</p>

          <div class="tools-form-section">
            <h4>{{ t('tools.hooks.test.payload') }}</h4>
            <pre class="hook-test-out">{{ result.payload }}</pre>
          </div>
          <div class="tools-form-section">
            <h4>{{ t('tools.hooks.test.stdout') }}</h4>
            <pre class="hook-test-out">{{ result.stdout || t('tools.hooks.test.empty') }}</pre>
          </div>
          <div class="tools-form-section">
            <h4>{{ t('tools.hooks.test.stderr') }}</h4>
            <pre class="hook-test-out">{{ result.stderr || t('tools.hooks.test.empty') }}</pre>
          </div>
        </template>

        <div class="modal-actions">
          <button class="btn" @click="emit('close')">{{ t('tools.hooks.test.close') }}</button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.hook-test-modal {
  width: 640px;
  max-width: 92vw;
  max-height: 86vh;
  overflow-y: auto;
  border: none;
}
.hook-test-cmd {
  display: block;
  margin-top: 2px;
  white-space: pre-wrap;
  word-break: break-all;
}
.hook-test-note {
  margin: 8px 0 12px;
  font-size: 11.5px;
  color: var(--text-mute);
  line-height: 1.6;
}
.hook-test-modal .tools-form-row .input {
  flex: 1;
  min-width: 0;
}
.hook-test-verdict {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 14px;
}
/* 输出框固定等宽 + 自己滚：hook 吐十几行 JSON 是常态，撑高弹框就把下面的输出挤出屏幕。 */
.hook-test-out {
  margin: 0;
  padding: 7px 9px;
  max-height: 160px;
  overflow: auto;
  border-radius: 7px;
  background: var(--surface-2);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11.5px;
  color: var(--text-dim);
  white-space: pre-wrap;
  word-break: break-all;
}
.hook-test-modal .modal-actions {
  margin-top: 18px;
}
</style>
