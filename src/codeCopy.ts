// 代码块「悬停复制」装饰器。renderText / shiki 产出的 <pre class="code-block"> 本身没有
// 复制按钮；这里在 v-html 注入后扫一遍 DOM，把每个 markdown 围栏代码块包进 .code-wrap，
// 右上角塞一个 hover 才显形的复制按钮。
//
// 设计要点：
//   · 只认 `.text-run pre.code-block`（renderText 的围栏代码块都落在 v-html 容器 .text-run
//     里），从而避开 tool-use 的 inline-tool-code / JSON 卡片，精准命中散文里的代码块。
//   · 包裹层在 shiki 用 pre.replaceWith(shikiPre) 换掉内层 <pre> 后依然存在（replaceWith 只
//     替换节点本身、保留父节点），所以装饰与高亮互不干扰、调用先后无所谓。
//   · 用「父节点是否已是 .code-wrap」做幂等判断，重复 sweep 不会二次包裹。
//   · 复制内容惰性读取：直接取 <code>/<pre> 的 textContent。shiki 高亮后的 <pre> 逐字
//     保留原始代码（行内是 span，行间是真实换行），textContent 就是原文。

import { langLabel } from './shikiHighlight'

// lucide copy / check 的原始 SVG（按钮要 innerHTML 注入，拿不到 Vue 图标组件的字符串，内联之）。
const COPY_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>'
const CHECK_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>'

function codeOf(wrap: HTMLElement): string {
  const pre = wrap.querySelector('pre')
  if (!pre) return ''
  return pre.querySelector('code')?.textContent ?? pre.textContent ?? ''
}

/** 给 root 里所有 markdown 围栏代码块加上「悬停复制」按钮（幂等，可反复调用）。 */
export function decorateCodeBlocks(root: HTMLElement | null): void {
  if (!root) return
  const blocks = root.querySelectorAll<HTMLPreElement>('.text-run pre.code-block')
  blocks.forEach((pre) => {
    const parent = pre.parentElement
    if (!parent || parent.classList.contains('code-wrap')) return // 已包裹 → 跳过

    const wrap = document.createElement('div')
    wrap.className = 'code-wrap'
    parent.insertBefore(wrap, pre)
    wrap.appendChild(pre)

    // 左上角语言标签（在复制按钮左边）。围栏信息串来自 pre.dataset.lang（format.ts 写入的
    // 原始别名，或 shiki 替换后已归一的规范名，两种都能识别）。未知语言不展示。
    const label = langLabel(pre.dataset.lang || '')
    if (label) {
      const tag = document.createElement('span')
      tag.className = 'code-lang'
      tag.textContent = label
      wrap.appendChild(tag)
    }

    const btn = document.createElement('button')
    btn.type = 'button'
    btn.className = 'code-copy'
    btn.setAttribute('aria-label', 'Copy code')
    btn.innerHTML = COPY_SVG
    let resetTimer = 0
    btn.addEventListener('click', (e) => {
      e.preventDefault()
      e.stopPropagation()
      void navigator.clipboard?.writeText(codeOf(wrap))
      btn.innerHTML = CHECK_SVG
      btn.classList.add('copied')
      window.clearTimeout(resetTimer)
      resetTimer = window.setTimeout(() => {
        btn.innerHTML = COPY_SVG
        btn.classList.remove('copied')
      }, 1200)
    })
    wrap.appendChild(btn)
  })
}
