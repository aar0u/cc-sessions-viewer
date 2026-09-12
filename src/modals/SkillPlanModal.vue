<script setup lang="ts">
// 写操作的确认框：把后端 dry-run 出来的计划逐条摆出来，用户点了才真跑。
//
// 为什么逐条列而不是一句「确定要删除吗」：这个功能改的是用户全机器的 skill 目录，
// 而删除要清的那几条链接（`~/.agents`、`~/.cc-switch`）恰恰是用户自己都不知道存在的。
// 「删 5 项」里的那 5 项必须看得见 —— 这是比上游 Skills-Manager 多做的那一步。
import { computed } from 'vue'
import type { WriteReport } from '../types'
import { t } from '../i18n'
import { shortenPath } from '../toolsSkills'
import { isDestructive, stepCounts } from '../toolsSkillsActions'

const props = defineProps<{
  show: boolean
  title: string
  report: WriteReport | null
  home: string
  busy: boolean
}>()

const emit = defineEmits<{ (e: 'confirm'): void; (e: 'cancel'): void }>()

const counts = computed(() => (props.report ? stepCounts(props.report) : null))
const danger = computed(() => (props.report ? isDestructive(props.report) : false))

/** 标题下那一句概括。只报非零的那几类，省得「移动 0 · 建链 0 · 删除 1」。 */
const summary = computed(() => {
  const c = counts.value
  if (!c) return ''
  const parts: string[] = []
  if (c.move) parts.push(t('tools.skills.plan.countMove', { n: String(c.move) }))
  if (c.link) parts.push(t('tools.skills.plan.countLink', { n: String(c.link) }))
  if (c.unlink) parts.push(t('tools.skills.plan.countUnlink', { n: String(c.unlink) }))
  if (c.deleteDir) parts.push(t('tools.skills.plan.countDelete', { n: String(c.deleteDir) }))
  return parts.join(' · ')
})

function short(path: string) {
  return shortenPath(path, props.home)
}
</script>

<template>
  <Transition name="fade">
    <div v-if="show && report" class="app-overlay" @click.self="emit('cancel')">
      <div class="modal skill-plan-modal" role="dialog" aria-modal="true">
        <h3>{{ title }}</h3>
        <p class="skill-plan-summary">{{ summary }}</p>

        <div class="skill-plan-steps">
          <div v-for="(s, i) in report.steps" :key="i" class="skill-plan-step" :class="s.kind">
            <span class="skill-plan-kind">{{ t(`tools.skills.step.${s.kind}`) }}</span>
            <div class="skill-plan-paths">
              <span class="skill-plan-path">{{ short(s.path) }}</span>
              <span v-if="s.target" class="skill-plan-target">→ {{ short(s.target) }}</span>
              <span v-if="s.note" class="skill-plan-step-note">{{ t(`tools.skills.note.${s.note}`) }}</span>
            </div>
          </div>
        </div>

        <p v-if="danger" class="skill-plan-warning">{{ t('tools.skills.plan.irreversible') }}</p>
        <!-- 备份不是留给用户去翻的东西：最后一步就把它删了。说清楚免得有人去找。 -->
        <p v-else-if="counts?.backup" class="skill-plan-note">{{ t('tools.skills.plan.backupNote') }}</p>

        <div class="modal-actions">
          <button class="btn" :disabled="busy" @click="emit('cancel')">
            {{ t('common.cancel') }}
          </button>
          <button
            class="btn"
            :class="[danger ? 'danger' : 'primary', { running: busy }]"
            :disabled="busy"
            @click="emit('confirm')"
          >
            <span v-if="busy" class="chip-spinner" aria-hidden="true" />
            {{ t('tools.skills.plan.apply', { n: String(report.steps.length) }) }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.skill-plan-modal {
  width: 620px;
  max-width: 92vw;
  /* `.modal` 的 border 和 `--shadow-lg` 的第一层（`0 0 0 1px`）是两圈描边，叠在一起
     在壁纸模式下亮成一道硬边。外轮廓只要阴影这一层。 */
  border: none;
}
.skill-plan-summary {
  margin: 2px 0 10px;
  font-size: 12.5px;
  color: var(--text-dim);
}
.skill-plan-steps {
  max-height: 46vh;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 2px;
}
.skill-plan-step {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 4px 10px;
  min-width: 0;
}
.skill-plan-kind {
  flex-shrink: 0;
  min-width: 56px;
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--surface-2);
  color: var(--text-mute);
  font-size: 10.5px;
  text-align: center;
}
.skill-plan-step.deleteDir .skill-plan-kind {
  background: var(--danger-soft);
  color: var(--danger);
}
.skill-plan-paths {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.skill-plan-path,
.skill-plan-target {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11.5px;
  word-break: break-all;
}
.skill-plan-target {
  color: var(--text-mute);
}
.skill-plan-step-note {
  font-size: 11px;
  color: var(--text-mute);
}
/* 底部那句备份说明。和步骤里的 note 分开两个类：同名会把 8px 上边距套到每一行步骤上。 */
.skill-plan-note {
  font-size: 11px;
  color: var(--text-mute);
  margin: 8px 0 0;
}
.skill-plan-warning {
  margin: 10px 0 0;
  font-size: 12px;
  color: var(--danger);
}
/* `.modal` 的按钮间距是靠 `p { margin-bottom: 18px }` 撑出来的，而这个框最后一块
   常常是步骤盒子（没有警告也没有备份说明时），不给就直接贴在按钮上。 */
.skill-plan-modal .modal-actions {
  margin-top: 16px;
}
</style>
