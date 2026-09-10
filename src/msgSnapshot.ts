// 消息列表的「非响应式快照」。
//
// 为什么需要：`ViewTab.msgs` 和 `ChatSession.msgs` 都挂在深响应式容器里（`ref` /
// `reactive`），Vue 会把数组、每个 `Msg`、每个 `Block`、每个嵌套 diff hunk 全部包成
// Proxy 并各建一张依赖表。一份 10 万条消息的 transcript 因此要多付几倍内存，而且
// ChatView 里三十多个 computed 每次重算都要把整份数据摸一遍。
//
// 而这份数据**从来不被就地改写** —— 追加走 `concat` / 展开，替换走整体赋值，所有
// 读取方都只依赖数组引用的变化。既然如此，深响应式一点用都没有，纯是开销。
//
// `markRaw` 给对象打一个「别代理我」的标记，Vue 遇到它就整棵跳过。
//
// 两条使用纪律（改动这些代码时必须守住）：
//   1. **每一个赋值点都要包**。`markRaw` 标记的是传进去的那个对象；`concat` /
//      `[...msgs, m]` 返回的是**新数组**，没有标记，赋回响应式容器就会重新被代理，
//      优化悄无声息地失效。所以是 `snapshotMsgs(a.concat(b))`，不是 `snapshotMsgs(a).concat(b)`。
//   2. **读取要走被追踪的那个属性**。`props.messages` / `tab.msgs` 这一层 get 仍然是
//      响应式的，引用一变所有依赖都会失效。但 `props.messages[i]` 之后就不再追踪了，
//      所以不要把数组存进局部变量再在 computed 里读它的元素。

import { markRaw } from 'vue'
import type { Msg } from './types'

/** 标记一份消息列表为不可变快照，跳过 Vue 的深层代理。 */
export function snapshotMsgs(msgs: Msg[]): Msg[] {
  return markRaw(msgs)
}
