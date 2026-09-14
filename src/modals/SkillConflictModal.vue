<script setup lang="ts">
// 收编时的同名冲突：两套 store 里都有、内容还不一样，让用户三选一。
//
// 上游 Skills-Manager 的 `import_to_hub` 在这种情况下**静默跳过**，显示成功但什么
// 都没做 —— 本机 33 个重复条目会全部命中。所以这个框存在的意义就是「不许静默」，
// 而且必须让用户看见差在哪：文件清单逐条比、SKILL.md 给增删行数。
//
// 内容一致的不会走到这儿（后端直接按「保留主 store 的」合并），不然用户要点 33 次
// 「随便哪个都行」。
//
// SKILL.md 从「+18 −4」升级成逐行差异（阶段 5）：行数只回答「改得多不多」，而这个框
// 要用户回答的是「留哪个」—— 那得知道**改的是哪几行**。渲染复用 `DiffBlock.vue`，
// 和聊天里的工具结果、Git 改动视图是同一套行样式。
import { computed, ref, watch } from 'vue'
import type { AdoptConflict, Resolution } from '../types'
import { t } from '../i18n'
import { formatSize } from '../format'
import { shortenPath } from '../toolsSkills'
import DiffBlock from '../components/DiffBlock.vue'

const props = defineProps<{
  show: boolean
  conflict: AdoptConflict | null
  /** 还剩几条（含当前这条）。 */
  remaining: number
  home: string
}>()

const emit = defineEmits<{
  (e: 'choose', resolution: Resolution): void
  (e: 'skip'): void
  (e: 'skip-all'): void
  (e: 'cancel'): void
}>()

type Choice = 'keepMain' | 'useExternal' | 'keepBoth'
const choice = ref<Choice>('keepMain')
const renameTo = ref('')

// 换到下一条冲突时把选择归零 —— 上一条选的「都留着」不该顺延到这一条。
watch(
  () => props.conflict?.name,
  () => {
    choice.value = 'keepMain'
    renameTo.value = props.conflict?.suggestedRename ?? ''
  },
  { immediate: true },
)

const diffFiles = computed(() =>
  (props.conflict?.files ?? []).filter((f) => f.status !== 'same'),
)
const sameCount = computed(
  () => (props.conflict?.files ?? []).length - diffFiles.value.length,
)

function short(path: string) {
  return shortenPath(path, props.home)
}

function fmtTime(ms: number | null) {
  return ms ? new Date(ms).toLocaleDateString() : '—'
}

function confirm() {
  if (choice.value === 'keepBoth') {
    const name = renameTo.value.trim()
    if (!name) return
    emit('choose', { kind: 'keepBoth', value: name })
    return
  }
  emit('choose', { kind: choice.value })
}
</script>

<template>
  <Transition name="fade">
    <div v-if="show && conflict" class="app-overlay" @click.self="emit('cancel')">
      <div class="modal skill-conflict-modal" role="dialog" aria-modal="true">
        <h3>
          {{ t('tools.skills.conflict.title', { name: conflict.name }) }}
          <span v-if="remaining > 1" class="skill-conflict-count">
            {{ t('tools.skills.conflict.remaining', { n: String(remaining) }) }}
          </span>
        </h3>
        <p class="skill-conflict-sub">{{ t('tools.skills.conflict.sub') }}</p>

        <div class="skill-conflict-sides">
          <div class="skill-conflict-side">
            <!-- A 不一定已经在主 store 里：同一批收编里同名的第二份，比的是这一批的
                 第一份，它此刻还在别的 store。标签写死「主 store 里的」会和它下面那行
                 路径当场对不上。 -->
            <span class="skill-conflict-tag">A · {{ t(conflict.mainInStore
              ? 'tools.skills.conflict.mainSide'
              : 'tools.skills.conflict.incomingSide') }}</span>
            <span class="skill-conflict-path">{{ short(conflict.main.path) }}</span>
            <span class="skill-conflict-meta">
              {{ t('tools.skills.fileCount', { n: String(conflict.main.files) }) }} ·
              {{ formatSize(conflict.main.bytes) }} · {{ fmtTime(conflict.main.modified) }}
            </span>
          </div>
          <div class="skill-conflict-side">
            <span class="skill-conflict-tag">B · {{ t('tools.skills.conflict.externalSide') }}</span>
            <span class="skill-conflict-path">{{ short(conflict.external.path) }}</span>
            <span class="skill-conflict-meta">
              {{ t('tools.skills.fileCount', { n: String(conflict.external.files) }) }} ·
              {{ formatSize(conflict.external.bytes) }} · {{ fmtTime(conflict.external.modified) }}
            </span>
          </div>
        </div>

        <div class="skill-conflict-diff">
          <div v-if="conflict.skillMd" class="skill-conflict-md">
            <span>SKILL.md</span>
            <template v-if="conflict.skillMd.truncated">
              <span class="skill-conflict-meta">{{ t('tools.skills.conflict.diffTooBig') }}</span>
            </template>
            <template v-else>
              <span class="diff-plus">+{{ conflict.skillMd.plus }}</span>
              <span class="diff-minus">−{{ conflict.skillMd.minus }}</span>
            </template>
          </div>
          <!-- 左边那个「主」、右边那个「外部」——旧行是主 store 的，新行是外部那份。 -->
          <template v-if="conflict.skillMd?.hunks.length">
            <div class="skill-conflict-diff-scroll">
              <DiffBlock :hunks="conflict.skillMd.hunks" file-path="SKILL.md" />
            </div>
            <div v-if="conflict.skillMd.clipped" class="skill-conflict-meta">
              {{ t('tools.skills.conflict.diffClipped') }}
            </div>
          </template>
          <div v-for="f in diffFiles" :key="f.path" class="skill-conflict-file">
            <span class="skill-conflict-file-name">{{ f.path }}</span>
            <span class="skill-conflict-file-status" :class="f.status">
              {{ t(`tools.skills.conflict.file.${f.status}`) }}
            </span>
          </div>
          <div v-if="sameCount > 0" class="skill-conflict-meta">
            {{ t('tools.skills.conflict.sameCount', { n: String(sameCount) }) }}
          </div>
          <p v-if="conflict.main.truncated || conflict.external.truncated" class="skill-conflict-warn">
            {{ t('tools.skills.conflict.truncated') }}
          </p>
        </div>

        <div class="skill-conflict-choices">
          <label>
            <input v-model="choice" type="radio" value="keepMain" />
            <span>{{ t('tools.skills.conflict.keepMain') }}</span>
          </label>
          <label>
            <input v-model="choice" type="radio" value="useExternal" />
            <span>{{ t('tools.skills.conflict.useExternal') }}</span>
          </label>
          <label>
            <input v-model="choice" type="radio" value="keepBoth" />
            <span>{{ t('tools.skills.conflict.keepBoth') }}</span>
            <input
              v-model="renameTo"
              class="skill-conflict-rename"
              type="text"
              spellcheck="false"
              :disabled="choice !== 'keepBoth'"
              :aria-label="t('tools.skills.conflict.keepBoth')"
              @focus="choice = 'keepBoth'"
            />
          </label>
        </div>

        <div class="modal-actions">
          <button class="btn" @click="emit('skip-all')">
            {{ t('tools.skills.conflict.skipAll') }}
          </button>
          <button class="btn" @click="emit('skip')">
            {{ t('tools.skills.conflict.skip') }}
          </button>
          <button
            class="btn primary"
            :disabled="choice === 'keepBoth' && !renameTo.trim()"
            @click="confirm"
          >
            {{ t('tools.skills.conflict.apply') }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.skill-conflict-modal {
  width: 640px;
  max-width: 92vw;
  /* `.modal` 的 border 和 `--shadow-lg` 的第一层（`0 0 0 1px`）是两圈描边，叠在一起
     在壁纸模式下亮成一道硬边。外轮廓只要阴影这一层。 */
  border: none;
}
.skill-conflict-modal h3 {
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.skill-conflict-count {
  font-size: 11.5px;
  font-weight: 400;
  color: var(--text-mute);
}
.skill-conflict-sub {
  margin: 2px 0 12px;
  font-size: 12.5px;
  color: var(--text-dim);
}
.skill-conflict-sides {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px;
}
.skill-conflict-side {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
  min-width: 0;
}
.skill-conflict-tag {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-dim);
}
.skill-conflict-path {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11px;
  word-break: break-all;
}
.skill-conflict-meta {
  font-size: 11px;
  color: var(--text-mute);
}
/* diff 自己滚，不让它把整个弹框撑到屏幕外。 */
.skill-conflict-diff-scroll {
  max-height: 260px;
  overflow: auto;
  margin: 6px 0 2px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface-2);
}

.skill-conflict-diff {
  margin-top: 10px;
  max-height: 26vh;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 8px 10px;
}
.skill-conflict-md {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  padding-bottom: 4px;
}
.diff-plus {
  color: var(--diff-add-fg, #16a34a);
  font-variant-numeric: tabular-nums;
}
.diff-minus {
  color: var(--danger);
  font-variant-numeric: tabular-nums;
}
.skill-conflict-file {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 2px 0;
  font-size: 11.5px;
  min-width: 0;
}
.skill-conflict-file-name {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.skill-conflict-file-status {
  margin-left: auto;
  flex-shrink: 0;
  padding: 1px 7px;
  border-radius: 999px;
  background: var(--surface-2);
  color: var(--text-mute);
  font-size: 10.5px;
}
.skill-conflict-warn {
  margin: 8px 0 0;
  font-size: 11.5px;
  color: var(--danger);
}
.skill-conflict-choices {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 12px;
}
.skill-conflict-choices label {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12.5px;
  cursor: pointer;
}
.skill-conflict-rename {
  flex: 1;
  min-width: 0;
  padding: 3px 8px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface);
  color: var(--text);
  font: inherit;
  font-size: 12px;
}
.skill-conflict-rename:disabled {
  opacity: 0.45;
}
/* 同 SkillPlanModal：最后一块是单选组，不给上边距就贴着按钮。 */
.skill-conflict-modal .modal-actions {
  margin-top: 16px;
}
</style>
