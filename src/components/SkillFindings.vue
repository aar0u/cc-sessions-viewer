<script setup lang="ts">
// 一个 skill 的风险明细。
//
// 两处用：本地 Skills 详情，和「发现」面板装之前那一屏。**这两处必须是同一段代码**
// —— 装之前看到的风险和装完之后看到的风险不一样的话，装之前那一屏就没有意义了。
// 后端那边也是同一份（`skills::describe_body`）。
//
// 降级说明（`level !== baseLevel`）不能省：一条 `curl` 出现在 SKILL.md 的代码块里和
// 出现在一个 `.sh` 文件里不是一回事，`risk.rs` 会按上下文降级。不写出来的话用户看到
// 的是一个「低危」，却无从判断它凭什么低。
import type { RiskFinding } from '../types'
import { t } from '../i18n'

defineProps<{
  findings: RiskFinding[]
  /** 文件没扫全（数量 / 深度触顶，或有文件扫不动）。**必须报出来** —— 一份只扫了
      一半的目录和一份真的干净的目录，不报的话在界面上长得一模一样。 */
  truncated: boolean
  /** 小节标题。两个面板的措辞不同（「风险点 (3)」/「装之前先看这几条 (3)」）。 */
  heading: string
}>()
</script>

<template>
  <div v-if="findings.length > 0" class="skill-section">
    <h4>
      {{ heading }}
      <span v-if="truncated" class="skill-note">{{ t('tools.skills.truncatedRisk') }}</span>
    </h4>
    <div v-for="(f, i) in findings" :key="i" class="skill-finding">
      <span class="skill-risk" :class="f.level">{{ t(`tools.skills.risk.${f.level}`) }}</span>
      <span class="skill-finding-rule">{{ f.rule }}</span>
      <span class="skill-finding-at">{{ f.file }}:{{ f.line }}</span>
      <code class="skill-finding-code">{{ f.excerpt }}</code>
      <span v-if="f.level !== f.baseLevel" class="skill-note">
        {{ t('tools.skills.downgraded', {
          base: t(`tools.skills.risk.${f.baseLevel}`),
          context: t(`tools.skills.context.${f.context}`),
        }) }}
      </span>
    </div>
  </div>
</template>

<style scoped>
.skill-section {
  margin-top: 18px;
}
.skill-section h4 {
  margin: 0 0 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-mute);
  display: flex;
  align-items: center;
  gap: 8px;
}
.skill-finding {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  padding: 4px 0;
  border-bottom: 1px solid var(--border);
}
.skill-finding-rule {
  font-size: 12px;
  color: var(--text);
}
.skill-finding-at {
  font-size: 11px;
  color: var(--text-dim);
}
.skill-note {
  font-size: 11px;
  color: var(--text-dim);
  font-weight: 400;
}
.skill-finding-code {
  flex: 1;
  min-width: 160px;
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11px;
  color: var(--text-mute);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
