<script setup lang="ts">
// 一个 skill 详情正在读时的骨架。
//
// 两处用：本地 Skills 详情（按名字走一次目录 + 逐个文件过风险规则，大的要两秒）
// 和「发现」面板的远端预览（浅克隆 + sparse-checkout，首次 3 秒级）。**同一段等待，
// 同一个形状** —— 两边各画一套的话，同一个应用里会出现两种「正在读」的语言。
//
// 骨架照着详情三节的真实版式摆：风险行是「等级药丸 + 规则名 + 出处」，
// frontmatter 是「键 + 值」，文件行是「图标 + 文件名 + 大小」。一行画成一根通长的
// 灰条是不行的 —— 详情栏有六七百像素宽，那看上去是几段正文，不像正在到位的记录。
//
// 宽度写死成一张不规则的表：每次点一条都重掷骰子的话，骨架自己会闪。
import { t } from '../i18n'

const SECTIONS = [
  {
    key: 'findings',
    head: '74px',
    rows: [
      ['30px', '92px', '46%'],
      ['30px', '108px', '38%'],
      ['30px', '86px', '52%'],
    ],
  },
  {
    key: 'frontmatter',
    head: '96px',
    rows: [
      ['62px', '22%'],
      ['62px', '54%'],
    ],
  },
  {
    key: 'files',
    head: '60px',
    rows: [
      ['15px', '26%', '40px'],
      ['15px', '34%', '36px'],
      ['15px', '22%', '44px'],
      ['15px', '30%', '38px'],
      ['15px', '19%', '42px'],
    ],
  },
] as const
</script>

<template>
  <div class="skill-detail-skel" role="status" :aria-label="t('tools.skills.loadingDetail')">
    <div v-for="sec in SECTIONS" :key="sec.key" class="skel-section">
      <span class="skill-skel-bar head" :style="{ width: sec.head }" />
      <div v-for="(row, i) in sec.rows" :key="i" class="skill-skel-line">
        <span v-for="(w, j) in row" :key="j" class="skill-skel-bar" :style="{ width: w }" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.skill-detail-skel {
  display: flex;
  flex-direction: column;
}
.skel-section {
  margin-top: 18px;
}
/* 整齐同步地闪会看成一块面板在呼吸；错开之后才像一条条正在到位的记录。 */
.skel-section:nth-child(2n) .skill-skel-bar {
  animation-delay: 0.12s;
}
.skel-section:nth-child(3n) .skill-skel-bar {
  animation-delay: 0.24s;
}
</style>
