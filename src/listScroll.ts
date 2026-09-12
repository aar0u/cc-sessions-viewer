// 列表滚动容器里的两件杂活：换一批内容时，浮块要作废，选中项要重新露出来。
//
// 两件事都只在「列表换了一批」这一刻发生，而那一刻在七个列表视图里各写各的。写成
// 函数放这儿是为了**只修一遍** —— 同样的毛病在四份拷贝里各修一次，改到第三份时
// 必然漏掉一份（这个仓库里已经有过先例）。
//
// 浮块的 mouseover 跟随逻辑还留在各视图里没动：那一段迟早要抽成 composable，但它
// 牵扯 scrolling / no-slide / reappearing 三个状态，是另一件事。

/**
 * 列表内容换了一批，把浮块收回原点。
 *
 * 浮块是 `position: absolute` + `top: var(--spot-y)` + 真实 `height`，所以它**算进
 * 滚动容器的 scrollHeight**。列表被过滤短了而 `--spot-y` 还停在上一批某一行的偏移
 * 量上时，scrollHeight 仍是上一批那么长 —— 浏览器于是不收回 `scrollTop`，剩下的几
 * 行留在视口**上方**，列表看上去整个是空的。
 *
 * 本机实测（50 个 skill 滚到中段，再点「来自 github」筛到 2 条）：scrollHeight
 * 1501 / scrollTop 736 / 两行分别在 `top: -645` 和 `-580`。屏幕上一行都看不见。
 *
 * 收回之后 scrollHeight 跟着缩，`scrollTop` 由浏览器自己夹回合法范围，不在这儿写
 * —— 手动置 0 会把「换一批但还很长」的列表也一并弹回顶部，那是另一件事。
 */
export function resetSpotlight(list: HTMLElement | undefined, spot: HTMLElement | undefined): void {
  list?.classList.remove('has-spot')
  if (!spot) return
  // 位置直接跳回去，不滑：浮块此刻是透明的，滑给谁看都看不见，反倒要多跑一次 34ms
  // 的 top 过渡，过渡期间 scrollHeight 是中间值。
  spot.classList.add('no-slide')
  spot.style.setProperty('--spot-y', '0px')
  spot.style.setProperty('--spot-h', '0px')
}


/**
 * 列表换了一批之后，把选中的那一行重新滚进视野。
 *
 * 典型一幕：先用「来自 github」筛到 2 条、点中其中一条，再把筛选关掉 —— 列表回到
 * 50 条，而选中那条按排序落在中段，屏幕上一眼看不见它。右边详情还画着它，左边却
 * 找不着，用户只能自己滚一遍去对。
 *
 * **看得见就不动。** 不加这一条的话，选中第二行时也会被滚成居中 —— 等于把上面那
 * 一行顶出屏幕，用户没要求任何事却发现列表自己动了。
 *
 * 调用方要在 DOM 更新之后再调（`await nextTick()`）：这函数是照着**当前**渲染出来
 * 的那一行量的。
 */
export function revealSelected(list: HTMLElement | undefined, selector: string): void {
  if (!list) return
  const row = list.querySelector<HTMLElement>(selector)
  if (!row) return
  const box = list.getBoundingClientRect()
  const at = row.getBoundingClientRect()
  if (at.top >= box.top && at.bottom <= box.bottom) return
  // `center` 而不是 `nearest`：真滚的时候是"它在屏幕外"，贴着边缘停下来还是要人再
  // 找一眼。上面那道"看得见就不动"已经把不该滚的情况挡掉了。
  row.scrollIntoView({ block: 'center' })
}
