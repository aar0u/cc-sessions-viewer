<script setup lang="ts">
// 工具管理的写操作确认框。MCP / Hooks 两个面板共用。
//
// 它**不认识任何一种 report**：各面板先把自己的报告翻成 `PlanView`（翻译函数住在
// 各自的 `.ts` 里，能被测到），这儿只管画。
//
// 为什么逐条列而不是一句「确定吗」：这个功能改的是用户全机器 agent 的配置文件，
// 「改 3 处」里的那 3 处必须看得见 —— 尤其是「写进去了但会被别的文件盖住」这种，
// 事后根本查不出来。
import { computed } from 'vue'
import type { PlanView } from '../toolsPlan'
import { t } from '../i18n'

const props = defineProps<{ show: boolean; plan: PlanView | null; busy: boolean }>()

const emit = defineEmits<{ (e: 'confirm'): void; (e: 'cancel'): void }>()

/**
 * 要不要分段。
 *
 * 「合并全部重复」一次能出二十来步，平铺开来看不出哪几步是在处理同一个文件。但
 * **只有一组时不画** —— 给一串本来就连贯的步骤加个标题只是多占一行，还显得像出了
 * 什么状况。没给 `group` 的面板（MCP / Hooks）也走这一条，行为完全不变。
 */
const grouped = computed(() => {
  const rows = props.plan?.rows ?? []
  const names = new Set(rows.map((r) => r.group).filter(Boolean))
  return names.size > 1
})

/** 这一行是不是某一段的头一行 —— 分割线画在它上面。 */
function startsGroup(i: number): boolean {
  if (!grouped.value) return false
  const rows = props.plan?.rows ?? []
  return i === 0 || rows[i - 1]?.group !== rows[i]?.group
}
</script>

<template>
  <Transition name="fade">
    <!-- `app-overlay-confirm` 把它叠到添加/编辑表单之上：计划是从那张表**派生**出来的
         二级确认，同一层 z-index 的话后挂载的表单会把它整个盖住，看上去像点了没反应。 -->
    <div v-if="show && plan" class="app-overlay app-overlay-confirm" @click.self="emit('cancel')">
      <div class="modal tools-plan-modal" role="dialog" aria-modal="true">
        <h3>{{ plan.title }}</h3>
        <p class="tools-plan-summary">{{ plan.summary }}</p>

        <div v-if="plan.rows.length > 0" class="tools-plan-steps">
          <template v-for="(r, i) in plan.rows" :key="i">
            <p v-if="startsGroup(i)" class="tools-plan-group" :class="{ first: i === 0 }">
              {{ r.group }}
            </p>
          <div class="tools-plan-step" :class="r.kind">
            <span class="tools-plan-kind">{{ r.kindLabel }}</span>
            <div class="tools-plan-body">
              <span class="tools-plan-where">{{ r.where }}</span>
              <code v-if="r.detail" class="tools-plan-detail">{{ r.detail }}</code>
              <span v-if="r.note" class="tools-plan-note-line" :class="{ warn: r.noteWarn }">
                {{ r.note }}
              </span>
            </div>
          </div>
          </template>
        </div>

        <div v-if="plan.blocked.length > 0" class="tools-plan-blocked">
          <h4>{{ t('tools.plan.blockedTitle') }}</h4>
          <p v-for="(b, i) in plan.blocked" :key="i">{{ b }}</p>
        </div>

        <p class="tools-plan-footnote" :class="{ danger: plan.danger }">{{ plan.footnote }}</p>

        <div class="modal-actions">
          <button class="btn" :disabled="busy" @click="emit('cancel')">
            {{ t('common.cancel') }}
          </button>
          <button
            class="btn"
            :class="[plan.danger ? 'danger' : 'primary', { running: busy }]"
            :disabled="busy || plan.rows.length === 0"
            @click="emit('confirm')"
          >
            <span v-if="busy" class="chip-spinner" aria-hidden="true" />
            {{ plan.applyLabel }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.tools-plan-modal {
  width: 620px;
  max-width: 92vw;
  /* `.modal` 的 border 和 `--shadow-lg` 的第一层是两圈描边，叠在一起在壁纸模式下
     亮成一道硬边。外轮廓只要阴影这一层。 */
  border: none;
}
.tools-plan-summary {
  margin: 2px 0 10px;
  font-size: 12.5px;
  color: var(--text-dim);
}
.tools-plan-steps {
  max-height: 42vh;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 2px;
}
/* 分段头：一条横线 + 文件名。横线画在标题上方（第一段除外 —— 那儿紧挨着容器边框，
   再来一条就是双线）。 */
.tools-plan-group {
  margin: 10px 0 2px;
  padding: 8px 10px 0;
  border-top: 1px solid var(--border);
  font-size: 11.5px;
  font-weight: 600;
  color: var(--text-dim);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
}
.tools-plan-group.first {
  margin-top: 0;
  padding-top: 2px;
  border-top: none;
}
.tools-plan-step {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 5px 10px;
  min-width: 0;
}
.tools-plan-kind {
  flex-shrink: 0;
  min-width: 44px;
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--surface-2);
  color: var(--text-mute);
  font-size: 10.5px;
  text-align: center;
}
.tools-plan-step.remove .tools-plan-kind {
  background: var(--danger-soft);
  color: var(--danger);
}
.tools-plan-body {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.tools-plan-where {
  font-size: 12px;
}
.tools-plan-detail {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11.5px;
  color: var(--text-mute);
  word-break: break-all;
}
.tools-plan-note-line {
  font-size: 11px;
  color: var(--text-mute);
}
.tools-plan-note-line.warn {
  color: var(--danger);
}
.tools-plan-blocked {
  margin-top: 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 8px 10px;
}
.tools-plan-blocked h4 {
  margin: 0 0 4px;
  font-size: 12px;
  color: var(--text-dim);
}
.tools-plan-blocked p {
  margin: 2px 0;
  font-size: 11.5px;
  color: var(--text-mute);
}
.tools-plan-footnote {
  margin: 10px 0 0;
  font-size: 11.5px;
  color: var(--text-mute);
}
.tools-plan-footnote.danger {
  color: var(--danger);
}
.tools-plan-modal .modal-actions {
  margin-top: 16px;
}
</style>
