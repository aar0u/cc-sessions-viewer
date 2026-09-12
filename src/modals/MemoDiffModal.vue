<script setup lang="ts">
// 全局配置的并排比较框。两处用它，形状是同一个：
//
// 1. **分叉** —— 两个同名文件内容不一样（本机的 `~/.claude/RTK.md` 和
//    `~/.codex/RTK.md` 就是）。左边是你正在看的那份，右边是另一份。
// 2. **保存被拒** —— 文件在外面被改过了。左边是你打开时读到的，右边是磁盘上现在的。
//
// 左右**各占一半**，两边的行一一对齐（`splitDiff.ts` 摊的）。合并式那种两个文件的行
// 交替排，而这个框说的就是「这两份哪儿不一样」—— 谁是谁得一眼看出来。
//
// 它自己不发起任何写：两个主按钮各往外抛一个事件，具体做什么（往哪边同步 / 重新加载）
// 由面板决定。差异本身一律由后端算（`memo::diff_text`），前端不碰 —— 两边各写一个
// 行差异算法必然给出两种 hunk。
import { computed, ref, watch } from 'vue'
import type { MemoDiff } from '../types'
import { t } from '../i18n'
import { splitRows } from '../splitDiff'

const props = defineProps<{
  show: boolean
  title: string
  diff: MemoDiff | null
  /** 两侧标题。分叉时是两个路径，冲突时是「你打开时」/「磁盘上现在」。 */
  leftLabel: string
  rightLabel: string
  /** 主按钮的字。不给就只有一个「关闭」。 */
  primaryLabel?: string
  primaryTip?: string
  /** 反方向那个按钮。分叉才有 —— 冲突时「拿打开时那份盖掉磁盘」不是这个框的事。 */
  secondaryLabel?: string
  secondaryTip?: string
  danger?: boolean
  busy?: boolean
}>()

const emit = defineEmits<{ (e: 'primary'): void; (e: 'secondary'): void; (e: 'close'): void }>()

const rows = computed(() => splitRows(props.diff?.hunks ?? []))

/** 转圈画在点的那个按钮上 —— 两个方向都是覆盖，画错边比不画更糟。 */
const pending = ref<'primary' | 'secondary' | null>(null)
watch(
  () => [props.busy, props.show],
  ([busy, show]) => {
    if (!busy || !show) pending.value = null
  },
)

function run(which: 'primary' | 'secondary') {
  pending.value = which
  if (which === 'primary') emit('primary')
  else emit('secondary')
}
</script>

<template>
  <Transition name="fade">
    <div v-if="show && diff" class="app-overlay" @click.self="emit('close')">
      <div class="modal memo-diff-modal" role="dialog" aria-modal="true">
        <h3>{{ title }}</h3>

        <div class="memo-diff-body">
          <div class="memo-split">
            <!-- 表头跟着同一个 grid 走，列宽才和下面的行严丝合缝。 -->
            <div class="memo-split-head del" v-tooltip="leftLabel">− {{ leftLabel }}</div>
            <div class="memo-split-head add" v-tooltip="rightLabel">+ {{ rightLabel }}</div>

            <!-- 扫描之后有人把两边改成一样了：要说「一样」，不是给一个空框让人猜。 -->
            <p v-if="diff.same" class="memo-split-msg">{{ t('tools.memo.fork.same') }}</p>
            <p v-else-if="diff.truncated" class="memo-split-msg">
              {{ t('tools.memo.fork.tooBig') }}
            </p>
            <template v-else>
              <template v-for="(r, i) in rows" :key="i">
                <div v-if="r.sep" class="memo-split-sep">···</div>
                <template v-else>
                  <div class="memo-split-cell" :class="r.left.kind">
                    <span class="memo-split-no">{{ r.left.no ?? '' }}</span>
                    <span class="memo-split-sign">{{ r.left.kind === 'del' ? '−' : ' ' }}</span>
                    <span class="memo-split-text">{{ r.left.text }}</span>
                  </div>
                  <div class="memo-split-cell right" :class="r.right.kind">
                    <span class="memo-split-no">{{ r.right.no ?? '' }}</span>
                    <span class="memo-split-sign">{{ r.right.kind === 'add' ? '+' : ' ' }}</span>
                    <span class="memo-split-text">{{ r.right.text }}</span>
                  </div>
                </template>
              </template>
            </template>
          </div>
        </div>
        <p v-if="diff.clipped" class="memo-diff-msg">{{ t('tools.memo.fork.clipped') }}</p>

        <!-- 两个覆盖按钮的左右和上面两栏一个顺序：左边那个是「以左边为准」。反过来排
             一眼看上去像指着旁边那一栏，点错的是一次覆盖写。 -->
        <div class="modal-actions">
          <button class="btn" :disabled="busy" @click="emit('close')">
            {{ t('tools.memo.fork.close') }}
          </button>
          <button
            v-if="primaryLabel"
            class="btn"
            :class="[danger ? 'danger' : 'primary', { running: busy && pending !== 'secondary' }]"
            :disabled="busy"
            v-tooltip="primaryTip ?? ''"
            @click="run('primary')"
          >
            <span v-if="busy && pending !== 'secondary'" class="chip-spinner" aria-hidden="true" />
            {{ primaryLabel }}
          </button>
          <button
            v-if="secondaryLabel"
            class="btn"
            :class="[danger ? 'danger' : 'primary', { running: busy && pending === 'secondary' }]"
            :disabled="busy"
            v-tooltip="secondaryTip ?? ''"
            @click="run('secondary')"
          >
            <span v-if="busy && pending === 'secondary'" class="chip-spinner" aria-hidden="true" />
            {{ secondaryLabel }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
/* 并排要比合并式宽一倍才不至于每行都折 —— 一半的宽度放的是整整一份文件的行。 */
.memo-diff-modal {
  width: min(1080px, calc(100vw - 64px));
  max-height: calc(100vh - 96px);
  display: flex;
  flex-direction: column;
}

/* 只占内容需要的高度：两边一样时框里就一句话，撑满一屏是空的。 */
.memo-diff-body {
  flex: 0 1 auto;
  min-height: 0;
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg);
}

.memo-split {
  display: grid;
  /* `minmax(0, 1fr)` 而不是 `1fr`：不断行的长行会把 `1fr` 顶宽，两栏就不是一半一半了。 */
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: var(--font-code-size);
  line-height: 1.55;
}

.memo-split-head {
  position: sticky;
  top: 0;
  z-index: 1;
  padding: 6px 10px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
  font-size: 11.5px;
  /* 路径从中间挤掉没意义 —— 两份分叉的文件区别就在开头那几段目录上。 */
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 和行里的 `+` / `-` 同色 —— 两边用不同的绿和红就看不出这是同一个对照。 */
.memo-split-head.del {
  color: var(--diff-removed);
}
.memo-split-head.add {
  color: var(--diff-added);
  border-left: 1px solid var(--border);
}

.memo-split-cell {
  display: flex;
  min-width: 0;
  padding-right: 8px;
}
.memo-split-cell.right {
  border-left: 1px solid var(--border);
}
.memo-split-cell.del {
  background: color-mix(in srgb, var(--diff-removed) 14%, transparent);
}
.memo-split-cell.add {
  background: color-mix(in srgb, var(--diff-added) 14%, transparent);
}
/* 对面多出来的那几行这边是空的。给一层淡灰，不然看着像文件里真有这么几个空行。 */
.memo-split-cell.pad {
  background: color-mix(in srgb, var(--text-mute) 8%, transparent);
}
.theme-dark .memo-split-cell.del {
  background: color-mix(in srgb, var(--diff-removed) 18%, transparent);
}
.theme-dark .memo-split-cell.add {
  background: color-mix(in srgb, var(--diff-added) 18%, transparent);
}

.memo-split-no {
  width: 40px;
  flex: none;
  text-align: right;
  padding-right: 8px;
  color: var(--text-mute);
  user-select: none;
  opacity: 0.7;
  font-variant-numeric: tabular-nums;
}
.memo-split-sign {
  width: 12px;
  flex: none;
  text-align: center;
  user-select: none;
  white-space: pre;
}
.memo-split-cell.del .memo-split-sign {
  color: var(--diff-removed);
}
.memo-split-cell.add .memo-split-sign {
  color: var(--diff-added);
}
.memo-split-text {
  min-width: 0;
  color: var(--text);
  white-space: pre-wrap;
  word-break: break-all;
}

.memo-split-sep {
  grid-column: 1 / -1;
  padding: 4px 0 4px 52px;
  color: var(--text-mute);
  user-select: none;
  opacity: 0.7;
}

.memo-split-msg {
  grid-column: 1 / -1;
  margin: 0;
  padding: 14px 12px;
  font-size: 12px;
  color: var(--text-mute);
  line-height: 1.6;
}

.memo-diff-msg {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--text-mute);
  line-height: 1.6;
}

/* 按钮不能贴着比较框 —— 上面是一整块有底色的东西，挨着看就像框的一部分。 */
.memo-diff-modal .modal-actions {
  margin-top: 16px;
}
</style>
