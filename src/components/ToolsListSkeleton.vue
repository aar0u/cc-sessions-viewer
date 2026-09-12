<script setup lang="ts">
// 列表区正在读时的骨架。
//
// 「发现」面板每打一次搜索就是一次 HTTP，慢的时候两三秒。原来那儿是一行居中的
// 「正在搜索…」—— 整块列表区是空的，而右边详情区在同样的等待里画的是骨架
// （`SkillDetailSkeleton`）。同一屏上两种「正在读」的语言，左边看上去像是坏了。
//
// 骨架照着 `.tools-row` 的真实版式摆：第一行是「名字 …… 安装量」，第二行是描述。
// 一行画成一根通长的灰条不行 —— 那看上去是几段正文，不像正在到位的列表。
//
// 宽度写死成一张不规则的表：每次搜索都重掷骰子的话，骨架自己会闪。
import { t } from '../i18n'

const ROWS = [
  { name: '34%', installs: '42px', desc: '68%' },
  { name: '46%', installs: '34px', desc: '54%' },
  { name: '28%', installs: '46px', desc: '73%' },
  { name: '39%', installs: '38px', desc: '61%' },
  { name: '31%', installs: '44px', desc: '70%' },
  { name: '44%', installs: '36px', desc: '49%' },
  { name: '36%', installs: '40px', desc: '65%' },
  { name: '26%', installs: '46px', desc: '58%' },
  { name: '41%', installs: '38px', desc: '72%' },
] as const
</script>

<template>
  <div class="tools-list-skel" role="status" :aria-label="t('tools.discover.loading')">
    <div v-for="(row, i) in ROWS" :key="i" class="tools-skel-row">
      <span class="tools-skel-top">
        <span class="skill-skel-bar" :style="{ width: row.name }" />
        <span class="tools-skel-gap" />
        <span class="skill-skel-bar" :style="{ width: row.installs }" />
      </span>
      <span class="skill-skel-bar" :style="{ width: row.desc }" />
    </div>
  </div>
</template>

<style scoped>
.tools-list-skel {
  display: flex;
  flex-direction: column;
}
/* 和 `.tools-row` 同一套内边距和行距，骨架换成真内容时行不会跳。 */
.tools-skel-row {
  display: flex;
  flex-direction: column;
  gap: 7px;
  padding: 12px 12px;
}
.tools-skel-top {
  display: flex;
  align-items: center;
  gap: 8px;
}
.tools-skel-gap {
  flex: 1;
}
/* 名字那一条比描述粗一档 —— 真行里它也是大一号的字。 */
.tools-skel-top .skill-skel-bar:first-child {
  height: 12px;
}
/* 整齐同步地闪会看成一块面板在呼吸；错开之后才像一条条正在到位的记录。 */
.tools-skel-row:nth-child(3n + 2) .skill-skel-bar {
  animation-delay: 0.12s;
}
.tools-skel-row:nth-child(3n) .skill-skel-bar {
  animation-delay: 0.24s;
}
</style>
