// 分屏格子里子视图实例的注册表：paneId → 该 pane 的 ChatView 实例。
//
// App.vue 需要对**聚焦格子**的 ChatView 调 onLiveAppend / flashMessage。多分屏时同屏有 N 个
// PaneContent 实例，单个 template ref 顶不住，于是每个 PaneContent 把自己内部的 ChatView
// 登记进这张表，App 按聚焦 paneId 取。
//
// reactive(Map) 让 `.get(id)` 的读取可被 computed 追踪；存进去的实例对象用 markRaw 包一层，
// 免得 Vue 去深度代理组件实例（既没必要也会踩坑）。

import { markRaw, reactive } from 'vue'
import type ChatView from './views/ChatView.vue'

export interface PaneViews {
  chatView: InstanceType<typeof ChatView> | null
}

const registry = reactive(new Map<number, PaneViews>())

export function registerPaneViews(paneId: number, views: PaneViews) {
  registry.set(paneId, markRaw(views))
}

export function unregisterPaneViews(paneId: number) {
  registry.delete(paneId)
}

export function paneViewsOf(paneId: number | null | undefined): PaneViews | null {
  return paneId == null ? null : registry.get(paneId) ?? null
}
