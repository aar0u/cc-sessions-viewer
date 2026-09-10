# 内存 / 磁盘占用优化方案

> 基线：v0.3.26，macOS，2026-09-09 在本机实测。目标：修掉「内存持续升高直至 24 GB」和「磁盘从 ~300 MB 涨到 1.7 GB+」两个问题，且**不改变任何现有功能行为**。本文只出方案，不含实现。

---

## 0. 结论摘要

**内存不是单点泄漏，是 3 个机制叠加放大：**

| # | 机制 | 所在进程 | 严重度 |
|---|------|----------|--------|
| M1 | `watch.rs` 轮询线程无界累积 + `session:reset` 风暴 → 同一个 100 MB+ 会话文件被反复整份解析并整份推给前端 | Rust 主进程 + WebContent | **P0** |
| M2 | 每个打开过的 view tab 常驻整份 transcript（深响应式 Proxy），启动时把所有已保存 tab 的会话全量读进内存 | WebContent | **P0** |
| M3 | 大 tool 输出 / JSON / diff / 代码块无大小上限地做 Shiki 高亮 → DOM 节点爆炸；缓存按条数而非字节封顶 | WebContent | **P0/P1** |
| M4 | 内联 base64 图片常驻 JS 堆 + 解码位图 | WebContent | P1 |
| M5 | 后端全局缓存（搜索正文缓存等）无界 | Rust 主进程 | P1 |
| M6 | 重 I/O 命令是同步 command，在主线程解析大文件（卡顿，非泄漏，但放大 M1） | Rust 主进程 | P1 |
| M7 | 周期任务：托盘 5 分钟全量重扫、桌宠 50 ms × 3 次 IPC、事件广播到所有窗口 | 两者 | P2 |

**磁盘：app 本体只有 23 MB，增长全部来自 5 个「只增不减」的落盘点：**

| # | 位置 | 现状 | 严重度 |
|---|------|------|--------|
| D1 | `~/.claude/.session-viewer-trash/` 回收站 | 软删除永不清理，删得越多占得越多 | **P0** |
| D2 | `$TMPDIR/clipboard-*.png`、`$TMPDIR/cc-sessions-viewer-images/` | 每次粘贴图片落一个文件，app 从不清理 | P1 |
| D3 | `turn-signals.jsonl`、`panic.log` | 只追加不轮转 | P1 |
| D4 | `background-media/`、`desktop-pets/` | 无配额、无占用展示；宠物 spritesheet 每次打开目录都重写 | P2 |
| D5 | 度量口径 | 「300 MB → 1.7 G」与 bundle 23 MB 对不上，需要一个「存储占用」面板定位 | P1 |

**实施顺序：** 阶段 1（后端止血，零 UI 变化）→ 阶段 2（前端内存）→ 阶段 3（磁盘治理 + 存储面板）→ 阶段 4（可观测性）。阶段 1 单独就能消掉最陡的那条增长曲线。

**当前状态（2026-09-09）：四个阶段全部完成。** 唯一列在计划里但没做的是 M2-3（大块分级投递）——
量了真实语料发现前提不成立，实现后整体回滚，理由见阶段 2 末尾。

---

## 1. 本机实测基线

| 项目 | 数值 | 说明 |
|------|------|------|
| `/Applications/Sessions Viewer.app` | 23 MB | 单一 Mach-O 22 MB + icns |
| `~/Library/Application Support/com.wuchao.cc-sessions-viewer/` | 146 MB | `background-media` 135 MB（15 个文件，多为 4K mp4）+ `desktop-pets` 11 MB |
| `~/Library/Application Support/cc-sessions-viewer/` | 1.6 MB | `turn-signals.jsonl` 1.6 MB（只追加）、`panic.log` 18 KB |
| `~/Library/WebKit/com.wuchao.cc-sessions-viewer/` | 1.1 MB | localStorage 等，正常 |
| `~/Library/Caches/cc-sessions-viewer/` | 20 KB | 价格表缓存，正常 |
| `~/.claude/.session-viewer-trash/` | 0 B | 本机没删过；他人机器上是主要嫌疑 |
| `$TMPDIR/clipboard-*.png` | 11 个 / ~2 MB（1 天） | 重度用户每天几十 MB |
| 会话语料 | codex 416 文件 / 1.9 GB，最大单文件 160 MB；claude 74 MB；grok 185 MB；agy 215 MB；pi 325 MB | 单个 rollout 100 MB+ 很常见 |
| 运行中进程（刚启动 1 分 39 秒） | 主进程 RSS 82 MB，WebContent 110 MB | 这是「干净」基线，后续验证用 |

结论：本机磁盘侧没有复现 1.7 GB，但已能确认所有无界落盘点；内存侧从代码即可确认放大链路（见 §2）。

---

## 2. 内存持续增长：根因分析

### M1（P0）`watch.rs` 轮询线程累积 + `session:reset` 风暴

**证据**

- `src-tauri/src/watch.rs:218`：每次 `watch_session()` 都 `thread::spawn` 一条 1.5 s 轮询线程；退出条件只有「当前活跃 (agent, path) 不等于自己」。对**同一路径**重复调用 `watch_session` 不会停掉旧线程，会叠加。
- 触发重复调用的前端入口：
  - `src/App.vue:3627-3630` 窗口每次 `focus` → 整份 `readSession` + 再 `watchSession`（不先 unwatch）。切一次窗口多一条线程。
  - `src/App.vue:4079-4092` 收到 `session:reset` → `refreshSessions()`（整个项目 `list_sessions`）+ `loadSessionTab()` 整份重读。
- `src-tauri/src/watch.rs:352-360`：文件指纹变了但 `Msg` 数没变 → 一律 emit `session:reset`。Claude Code 实时会话里大量 JSONL 记录（`progress`、`file-history-snapshot`、`queue-operation`、tool_use 分片等）不产生 `Msg`，Codex 的 `token_count` / `turn_context` 同理 → **一个实时会话每写一行就可能触发一次整份重读**。这条只是为 Pi 的 `/rename` 元数据（`session_info`）加的。
- `src-tauri/src/watch.rs:155`：`watch_session` 自己先整份 `read_session` 一遍只为数条数，与前端刚做的整份读重复。
- `src-tauri/src/watch.rs:307`：指纹「读-比-写」不在同一把锁里，N 条轮询线程同时看到旧指纹 → N 条一起整份解析。
- `src-tauri/src/watch.rs:191`：watch 的是**父目录**（`~/.codex/sessions/YYYY/MM/DD/` 或 `~/.claude/projects/<dir>/`），同目录任意会话写入都会 spawn 一条 200 ms 睡眠线程。

**放大效果**：对一个 160 MB 的 rollout，一次 `read_session` 在 Rust 里要构建几百 MB 的 `Vec<Msg>`，序列化成 JSON 再推给 WebView 后 JS 堆再涨 3–5 倍。这条链路每 1.5 s 被 N 条线程重复触发，主进程 RSS 因 malloc 碎片只升不降，WebContent 每次都 `JSON.parse` 一份 100 MB+ 的 payload。这与「用着用着内存升到 24 G」的描述完全一致。

**修法（全部不改可见行为）**

1. **单一轮询者**：把轮询线程句柄/停止标记放进 `WatchState`，`unwatch` / 替换时显式停掉；`watch_session` 对相同 (agent, path) 幂等（已在 watch 直接返回，不再重读、不再起线程）。
2. **去掉「条数相等 → reset」的通用规则**：改成 trait 钩子 `SessionSource::metadata_fingerprint(path) -> Option<String>`（默认 `None`），只有 Pi 实现它（读 `session_info` 标题）；指纹变化才 emit `session:reset`。其他 agent 条数相等时静默。
3. **指纹 compare-and-set**：读指纹、比较、写回在同一个锁作用域内完成；额外加「最小处理间隔」（如 500 ms），封顶单文件解析频率。
4. **前端 `onFocus` 改用已有的 `check_watched_session`**（`src-tauri/src/lib.rs:332`）触发一次增量检查，不再整份重读 + 重新 watch。
5. **`watch_session` 的初始计数**改为接受前端传入的 `known_count`（前端刚 `readSession` 得到的长度），省掉那次重复解析。
6. 收尾时清理 `LAST_COUNT` / `LAST_STAT` / `DEBOUNCE_SEQ` 里对应 path 的条目（现在只增不删）。

**后续可选（不在本轮）**：Claude / Codex 增量 tail 解析（记录字节 offset，只解析新增行）。收益大但要重构解析器的跨行状态，作为阶段 2 之后的独立项。

---

### M2（P0）前端整份 transcript 常驻 + 深响应式 + 启动全量恢复

**证据**

- `src/viewTabs.ts:35`：`msgs: Msg[]` 放在 `ref<ViewTab[]>` 里 → 深响应式，每个 `Msg` / `Block` / 字符串属性都会被 Proxy 包裹并建 Dep。tab 按设计「切项目时隐藏但不杀」，所以**所有打开过且未关闭的 tab 各自常驻一整份会话**。
- `src/views/ChatView.vue` 有 31 处遍历 `props.messages` 的 computed / watch（如 `historicalExecutionTimes` :376、`resultByToolId` :524、`toolUseById` :537、`stats` :1063、:1665、:1763），每个都会把全部 Msg 触碰一遍 → 全量 Proxy 化 + 每个 computed 一套 Dep 表。虚拟列表（阈值 80 条，:1205）只减少 DOM，不减少这部分。
- `src/App.vue:3777`：启动恢复时对**每一个**已保存的 session tab 调 `loadSessionTab` 全量读；`:3794-3795`、`:3826` 对 chat tab 同样全量读。用户攒了十几个 tab（含几个 100 MB+ 的 codex 会话）→ 启动即数 GB。
- `src/App.vue:4063` 追加时用 `concat` 产生新数组（好事：说明改成不可变快照的代价很低）。

**修法**

1. **`msgs` 改为非响应式快照**：`createViewTab` / `loadSessionTab` / append 处统一 `markRaw(msgs)`；ChatView 依赖 `props.messages` 的**引用变化**重算（现在已是整体替换语义）。GUI chat 的 `ChatSession.msgs` 是原地 `push`，需要改成 `shallowReactive` 数组或每次替换引用（`s.msgs = s.msgs.concat(m)`），逐一核对 `src/chatSessions.ts` 里的写点。
2. **后台 tab 惰性加载 + 淘汰**：
   - 启动恢复只创建 tab 壳（`loadingMsgs = true`），**激活时**才 `loadSessionTab`。
   - 非激活 tab 的 `msgs` 超过 N 分钟未展示（建议 10 分钟）或已加载 tab 数超过 K（建议每项目 3、全局 8，按 LRU）→ 置空并标记 `evicted`，再次激活时从磁盘重读（只读数据，用户无感）。
   - chat tab 例外（实时状态不可丢），仅对 `type === 'session'` 生效。
3. **大块正文分级下发（阶段 2b，改动较大）**：`read_session` 对单个 `Block.text` 超过 256 KB 的只回传头 64 KB + 尾 8 KB + `truncated: { bytes }`；UI 显示「展开完整内容」，点击时经新命令 `read_block(path, msgIdx, blockIdx)` 按需取。同时砍掉 IPC 体积和 JS 堆。

---

### M3（P0/P1）大块渲染无上限、缓存按条数封顶

**证据**

- `src/components/ToolResult.vue:126-127`：`diffHtml` / `jsonHtml` 对整段 tool 输出无条件高亮，`cat` 一个 5 MB 文件的结果会生成百万级 `<span>`。
- `src/shikiHighlight.ts:270-300`：`codeToHtml` 对任意长度代码块执行；`:144` 每个块再把源码 `encodeURIComponent` 存进 `data-source`，内存翻倍。
- `src/mermaid.ts:24` `renderedSvgCache` 无上限（只在切主题时清空）；`src/format.ts:753` `renderTextCache` 按 3000 **条**封顶，不按字节。
- 虚拟化阈值按**条数**（80），80 条以内哪怕每条 5 MB 也全量挂 DOM。

**修法**

1. 统一的大小闸门（常量集中放 `src/renderLimits.ts`）：Shiki > 100 KB 不高亮（纯 `<pre>`），JSON 美化 > 200 KB 跳过，diff 高亮 > 300 KB 跳过，超过阈值的块默认折叠并显示「内容较大（x KB）」。
2. 虚拟化改为「条数 > 80 **或** 文本总字节 > 2 MB」。
3. `data-source` 改存 `WeakMap<HTMLElement, string>`（随节点回收），或主题切换时从 `textContent` 重取。
4. 缓存改按字节：`renderTextCache` 20 MB、`renderedSvgCache` 最多 50 条 / 10 MB，LRU。

---

### M4（P1）内联 base64 图片

**证据**：`src-tauri/src/types.rs:321` `image_src` 为 `data:` URL；`src/views/ChatView.vue:706` 直接喂给 `<img>`。base64 字符串留在 JSON payload 与 JS 堆里（×1.33），WebKit 再为每张在 DOM 里的图片持有解码位图（2000×1500 截图 ≈ 12 MB）。

**修法**：`read_session` 时把图片字节写入内容寻址缓存目录 `<data_dir>/image-cache/<sha256>.<ext>`（同图去重），`image_src` 返回文件路径，前端走已有的 `convertFileSrc` 分支；`<img loading="lazy" decoding="async">`。缓存目录纳入阶段 3 的容量治理（LRU，默认上限 500 MB）。小于 32 KB 的图保持 `data:`。

---

### M5（P1）后端缓存无界

| 位置 | 问题 | 修法 |
|------|------|------|
| `src-tauri/src/agents/mod.rs:51` `USER_TEXT_CACHE` | 全局搜索一次后常驻**所有**会话的全部用户消息正文；`cached_user_text` 每次命中还 `clone()` 整个 Vec | 按总字节封顶（64 MB，LRU）；值改 `Arc<[..]>` 免克隆 |
| `src-tauri/src/turn.rs:154` `DESKTOP_TASKS` | 每个出现过 turn 信号的会话路径一条，不清理 | `completed/failed` 超过 24 h 自动剔除 |
| `src-tauri/src/agent_chat.rs:119-122` `streaming_agent_items` / `plan_item_texts` | 每轮增长、从不清 | 每轮 `turn/completed` 后清空 |
| `src-tauri/src/agent_chat.rs` `emitted_plan_turns` / `emitted_plan_items` | 去重判据，**不能**每轮清空，但每轮至少加一条、从不删；`emitted_plan_turns` 的 key 还是整段计划正文 | 改用 `RecentSet`（只保留最近 512 条），去重效果不变、内存有上限 |
| `src-tauri/src/agents/mod.rs` `USAGE_CACHE` | 条目数 = 磁盘上的会话总数，统计页跑一遍全灌进来且永不释放；key 是绝对路径 | 按条数封顶 20 000，超限按插入序淘汰到 75% |
| `src-tauri/src/agents/claude.rs` `SCAN_CACHE` | 同上，value 还带标题 / cwd 等字符串 | 同上 |
| `src-tauri/src/watch.rs:57/62/81` 三张 map | 只增不删 | 随 M1 第 6 条一起清 |

排查过但**确认无需改**的：`PENDING_PATH_SIGNALS`（每个 insert 在成功与超时两条路径上都有配对的 remove）、`CHATS`（随会话关闭移除）、`ChatMeta.messages`（就是这个会话的 transcript，长度等于会话本身）。

---

### M6（P1）同步 command 在主线程解析大文件

Tauri 2 里非 `async` 的 command 在**主线程**执行。`src-tauri/src/lib.rs` 中 `list_projects`(:63)、`list_sessions`(:253)、`read_session`(:282)、`watch_session`(:321)、`session_usage`(:612)、`agent_stats`(:635)、`session_last_prompt`、`soft_delete_session` 等全是同步。读一个 160 MB rollout 会把 UI 卡住 1–3 s，期间桌宠 50 ms 轮询、PTY 事件全部排队，用户看到的就是「越用越卡」。

另外 `claude.rs:342` 与 `codex.rs:2506` 的 `last_user_text` 用 `fs::read` **整文件读入内存**再倒序扫描；会话列表每滚一屏就对每张卡片调一次（`SessionsView.vue:284`），160 MB 的文件会整份读一遍。

**修法**：这批只读命令加 `#[tauri::command(async)]`（无共享状态，改动是纯注解）；`last_user_text` 改成从文件尾按 1 MB 块倒读。零行为变化。

---

### M7（P2）周期任务

| 位置 | 现状 | 修法 |
|------|------|------|
| `src-tauri/src/tray.rs:257-268` | 每 300 s `quick_stats()`：读取**所有 agent 近 30 天内改过的全部会话**的 turns（本机 ≈ 2.5 GB），无论托盘菜单是否打开 | 按 (path, mtime) 缓存每文件的 turn 聚合结果（类比 `USAGE_CACHE`），只重读变化文件；托盘未启用时不跑 |
| `src/components/DesktopPet.vue:374` | 50 ms 一次，每次 3 个 IPC（`cursorPosition` + `outerPosition` + `scaleFactor`）= 60 IPC/s | 先只取 `cursorPosition`，位置未变直接返回；间隔 100 ms；窗口不可见时暂停 |
| `src-tauri/src/pty.rs` / `watch.rs` / `agent_chat.rs` 的 `app.emit` | 广播到所有窗口，包括桌宠窗口（那边没人听，仍要序列化 + evaluateJavaScript） | 改 `emit_to("main", …)` |
| `src/gitRepository.ts:76` | 每 pane 5 s 一次 `git status` | 窗口失焦 / 隐藏时暂停 |
| xterm tab（`terminals.ts:1660/1968` scrollback 5000） | 常驻是设计，已有 `dispose` | 不动 |

---

## 3. 磁盘持续增长：根因分析

### 3.1 app 所有落盘点清单

| 位置 | 写入者 | 现有策略 | 建议策略 |
|------|--------|----------|----------|
| `~/.claude/.session-viewer-trash/` | `trash.rs` | 手动「清空回收站」，否则永久 | **D1** 保留期 30 天（可设置、可关闭），启动 + 每日自动清理过期项；设置页显示总大小 |
| `$TMPDIR/clipboard-*.png` | `lib.rs:1763-1768`、`:1813` | 无 | **D2** 迁到 `<data_dir>/attachments/<yyyy-mm>/`，30 天 + 500 MB LRU |
| `$TMPDIR/cc-sessions-viewer-images/chat-img-*` | `lib.rs:2156` | 无 | 同上 |
| `~/Library/Application Support/cc-sessions-viewer/turn-signals.jsonl` | `turn.rs:309`（hook 脚本追加） | 无，只增 | **D3** 启动时（watcher 起来前）截断；运行中超过 1 MB 时截断并重置 offset（`turn.rs:481-493` 已能处理 `file_len < offset`，安全） |
| 同目录 `panic.log` | `panic_log.rs:21` | 无 | 超过 256 KB 只保留尾部 |
| `<data_dir>/background-media/` | `background_media.rs` | 已按内容去重、可删 | **D4** 设置页显示占用；不做自动删（用户资产） |
| `<data_dir>/desktop-pets/codex/*/spritesheet.webp` | `desktop_pet_assets.rs:236` | **每次打开宠物目录都从 Codex asar 重写一遍**（11 MB 写放大） | 先比对哈希再写 |
| `~/.codex/pets/` | 用户自定义宠物 | 已有删除 | 显示占用 |
| `~/Library/Caches/cc-sessions-viewer/model-pricing-v3.json` | `pricing.rs` | 单文件覆盖 | 不动 |
| `~/.claude/.session-viewer-bookmarks.json`、`~/.grok/config.toml.bak`、kimi 索引 tmp/bak | 各模块 | 单文件覆盖 / 有清理 | 不动 |
| `$TMPDIR/cc-sessions-viewer/resume-<pid>.command` | `lib.rs:1302` | 按 pid 覆盖 | 不动 |

### 3.2 关于「300 MB → 1.7 GB」的口径

- macOS「储存空间 → 应用程序」只统计 bundle，本 app 是 23 MB，300 MB 这个起点对不上。更可能是第三方清理工具把 `Application Support` + `WebKit` + `Caches` 合并统计（本机这样算是 ~170 MB，多几个 4K 背景视频就到 300 MB），而后续增长来自回收站、临时图片。
- 如果反馈者在 **Windows**：WebView2 会在 `%LOCALAPPDATA%\com.wuchao.cc-sessions-viewer\EBWebView\` 下积累 Code Cache / GPUCache / blob_storage，Tauri 应用涨到 GB 级是常见反馈。阶段 3 顺带在 Windows 上把 WebView2 数据目录固定并在设置页提供「清理缓存」。
- 与其猜，不如让 app 自己报：阶段 3 的「存储占用」面板逐项列出上表每个位置的大小（复用 `trash.rs:61 directory_size`）并提供对应清理按钮。上线后一份截图就能定位。

---

## 4. 分阶段实施计划

每个阶段可独立发版；每项都标注「行为变化」。

### 阶段 1 — 后端止血（✅ 已完成，零 UI 变化）

| 项 | 改动点 | 行为变化 |
|----|--------|----------|
| M1-1/3/5/6 | `watch.rs`：单一轮询者、幂等 watch、CAS 指纹 + 最小间隔、接受 `known_count`、unwatch 清 map | 无（实时 tail 行为不变） |
| M1-2 | `agents/mod.rs` 增加 `metadata_fingerprint` 钩子，`claude.rs` / `pi.rs` 实现；`watch.rs` 改为按钩子判断 | 无（rename 仍刷新标题；其他 agent 少了无意义的整份重读） |
| M1-4 | `App.vue:3627` `onFocus` 改调 `checkWatchedSession` | 无（新增消息仍会亮起 live 标记） |
| M6 | 只读重命令加 `(async)`；`last_user_text` 尾部倒读 | 无（UI 不再被大文件卡住） |
| M5 | 三处缓存封顶 / 清理 | 无 |
| M7 | `emit_to("main")`；桌宠轮询降频；托盘 turn 缓存 | 无（桌宠视线跟随略降精度，肉眼不可见） |
| D3 | `turn-signals.jsonl` / `panic.log` 轮转 | 无 |

**完成状态（2026-09-09）**

上表七项全部落地，无 UI 改动、无新增设置项。自动化验证：

| 检查 | 结果 |
|------|------|
| `cargo test --lib` | 511 passed / 0 failed / 3 ignored（此前 490，新增 21 条） |
| `npm run test:run` | 1037 passed / 82 files |
| `npx vue-tsc --noEmit` | 通过 |

新增的 Rust 用例覆盖本阶段每一处新逻辑：

- `watch.rs`：`forget_path` 清空全部 per-path map、指纹 CAS 声明对下一个调用者可见。
- `util.rs`：`read_tail_text` 丢弃被截断的首行 / 小文件整读；`scan_lines_backwards` 倒序访问、跨 1 MB 分块拼行、无命中返回 `None`。
- `agents/claude.rs` / `agents/pi.rs`：`metadata_fingerprint` 取最后一条 rename；Pi 的「清空标题」返回 `Some("")` 而非 `None`；无记录 / 文件不存在返回 `None`。
- `agents/mod.rs` + `stats/tray.rs`：两处缓存按插入序淘汰到 75%，字节 / 条数计数与留下的条目一致。
- `turn.rs`：`prune_desktop_tasks` 只清超期的 completed/failed，长跑中的任务不受影响；时钟回拨时不误删。
- `panic_log.rs`：`rotate_if_large` 超限只留尾部一半且首行不是半行，小文件与缺失文件不动。

§5 的 A、B 两组 RSS 对比脚本需在真机长时间使用后跑，留给验收。

### 阶段 2 — 前端内存（M2 / M3 / M4 ✅ 已完成；M2-3 实测后判定不做）

| 项 | 改动点 | 行为变化 |
|----|--------|----------|
| M2-1 ✅ | `viewTabs.ts` / `chatSessions.ts` `msgs` 改 `markRaw` 快照，核对 `ChatView.vue` 31 处依赖 | 无 |
| M2-2 ✅ | 启动惰性加载；后台 session tab LRU 淘汰 + 重激活重读 | 无（切回 tab 时多一次磁盘读，有 loading 态） |
| M3 ✅ | 大小闸门、按字节的缓存、**删掉** `data-source`（改读 `textContent`）、虚拟化按字节 | 超大块默认折叠并提示大小（可展开） |
| M4 ✅ | 图片改内容寻址文件缓存 + `convertFileSrc`；导出前读回内联 | 无 |
| M2-3（可选） | 大块分级下发 + `read_block` | 超大块「展开完整内容」按钮 |

**M2 / M3 完成状态（2026-09-09）**

新增两个模块：`src/renderLimits.ts`（所有渲染尺寸阈值集中在一处）、`src/msgSnapshot.ts`（`markRaw` 快照 + 两条使用纪律）。

M2 的关键设计：

- `ViewTab` 新增 `msgsLoaded` / `lastShownAt` 两个字段。`msgsLoaded === false` = 「壳」：启动恢复只建壳不读盘，被某个 pane 显示时才装载。
- 「可见」判定用的是**每个 pane 的 active tab**，不是 `activeViewTabId`（那只是聚焦格子的投影）。分屏下同时可见多个 tab，正在显示的永远不淘汰。
- 后台 session tab 同时受两条约束：已装载数超过 8 个按 LRU 淘汰，或闲置超过 10 分钟释放。定时器每 60 秒扫一次兜底。chat tab 完全不参与 —— 它的 msgs 是进程实时推来的，磁盘上没有等价来源。
- 方案原写「每项目 3 个」的上限，实现时去掉了：全局上限才是真正的内存边界，按项目限制只会在内存充裕时白白多掉几个 tab。

改动中修掉的两个隐患：

- `session:append` 按路径找 tab 时用 `find` 取第一个匹配。同一会话开了多个 tab 时可能选中一个壳 tab，把尾段接到空列表上得到一份残缺 transcript。现在优先选真正装载了的那个，壳 tab 直接跳过追加。
- 显式打开入口（`openChat` / `openTrashSession` 等）会先把 `loadingMsgs` 置起来再 await；按需装载必须让路，否则同一个 160 MB 的文件会被并行读两遍。

自动化验证：

| 检查 | 结果 |
|------|------|
| `npm run test:run` | 1067 passed / 84 files（此前 1037 / 82，新增 30 条） |
| `npx vue-tsc --noEmit` | 通过 |
| `cargo test --lib` | 511 passed（后端未改动） |

新增用例：`renderLimits`（体积统计、提前收工、两条虚拟化阈值）、`viewTabs`（快照非响应式、LRU 与闲置淘汰、可见 tab 与 chat tab 永不释放）、`ToolResult`（三档闸门跳过染色但内容不少、超大块默认折叠并标体积）、`shikiHighlight`（超限跳过、不再留 `data-source`、`textContent` 反复重画逐字还原）、`format`（markdown 缓存字节封顶）、`mermaid`（命中复用、超限 LRU 淘汰、切主题清空）。

真机 RSS 对比（打开 160 MB 的 codex 会话；10 个 tab 恢复后的启动内存）留给验收。

**M4 / M5 完成状态（2026-09-09）**

M4 —— 新增 `src-tauri/src/image_cache.rs`：`read_session` 返回前把消息里的 `data:` 图片按
sha256 写进 `<data_dir>/image-cache/<hash>.<ext>`，`image_src` 换成文件路径，前端走已有的
`convertFileSrc` 分支。同一张图（同一会话里重复出现、或多个会话共用）只占一份磁盘、一份解码
位图。小于 32 KB 的图保持内联 —— 为它们各开一个文件句柄不划算。写盘是 temp + `rename`，
中途崩溃不会在缓存里留下半张图；命中判据是「文件存在且长度一致」。

落点选在 `read_session` 这个 command 上，**不是** `post_process_session_msgs`：实时对话流推来的
消息必须保持内联，否则聊天历史回填拿不到字节；真正值得省的是整份 transcript 那一大坨 IPC payload。

`imageSrc` 变成路径后有四个消费方会坏，都一并修了 —— 其中三个**在此之前就是坏的**（Codex 的
`@文件` 图片、剪贴板截图一直是路径形态）：

- Markdown / HTML / JSON 导出：新增 `src/imageInline.ts`，导出前把本地路径读回 base64。导出产物
  重新变回自包含（换台机器、用普通浏览器打开都还在）。读不到的保持原样，留死链好过丢图。
- ↑ 历史回填：`chatInputHistory` 现在把本地路径存成带 `sourcePath` 的占位附件，`ChatComposer`
  读盘补上字节再挂进附件栏 —— 不能先挂空壳，否则用户在读盘完成前回车就发出一张空图。读盘期间
  又翻了历史 / 换了会话的，落后的那次回填作废；期间新贴的图接在结果后面，不整栏覆盖。

M5 —— 阶段 1 封了三处，这轮排查又发现四处并一起封顶：`agent_chat.rs` 的
`emitted_plan_turns` / `emitted_plan_items` 改用 `RecentSet`（最近 512 条，去重效果不变）、
`agents/mod.rs` 的 `USAGE_CACHE` 与 `claude.rs` 的 `SCAN_CACHE` 按条数封顶 20 000、超限按插入序
淘汰到 75%。详见 §2 M5 的表格与「确认无需改」清单。

自动化验证：

| 检查 | 结果 |
|------|------|
| `npm run test:run` | 1082 passed / 85 files（此前 1067 / 84，新增 15 条） |
| `npx vue-tsc --noEmit` | 通过 |
| `cargo test --lib` | 529 passed / 0 failed / 3 ignored（此前 511，新增 18 条） |

新增用例：`image_cache`（去重、原子写、非 base64 / 未知 MIME 不动、小图保持内联、多消息批量外置）、
`agent_chat`（`RecentSet` 去重与超限淘汰）、`agents/mod` + `claude`（两张缓存的淘汰边界与 mtime 失效）、
`imageInline`（路径识别、去重读盘、读失败保持原样、无图时原样返回不拷贝）、`export`（三种导出都内联）、
`chatInputHistory`（路径图片留 `sourcePath` 占位、`data:` 图片不必再读盘）、`ChatComposer`（读盘回填、
读失败丢弃、读盘期间翻页作废）。

真机验证（dev 实例 + MCP 桥，2026-09-09）：

- 三个含图会话走 `read_session`，13 张图里 12 张外置成 `<hash>.png/.jpg`、1 张 22 KB 的保持内联；
  落盘文件 `file --mime-type` 与扩展名逐一对上，目录里没有残留的 temp 文件。
- 再读一遍同样三个会话：文件数、inode、mtime 全部不变 —— 命中判据生效，没有重复写盘。
- 界面上四张外置图都从 `asset://` 正常渲染（`decoding="async"` / `loading="lazy"` 在位），
  点开灯箱是 1920×887 的原图。

`image-cache` 目录本身的容量治理（LRU，默认上限 500 MB）属于阶段 3。

### M2-3（大块分级投递）—— 实测后判定不做（2026-09-09）

原计划：`read_session` 对超大文本块只回摘要 + 句柄，正文落 `block-cache`，前端点「加载完整内容」
再走 `read_block` 取回。**已按计划完整实现了一遍，然后量了真实语料，发现前提不成立，已整体回滚。**

本机 441 个 JSONL 会话（含多个 >100 MB）的实测：

| 指标 | 实测值 |
|------|--------|
| 最大的一个 `tool_result` 文本块 | 153 KB |
| >256 KB 的文本块 | 0 个 |
| 32–64 KB 区间的块 | 2 249 个，合计 81.7 MB |

也就是说：**按 512 KB 分级会一个块都不触发；按 32 KB 分级会给几千个普通结果套上「加载完整内容」
按钮，并且把 JSON / diff 高亮打断。** 两头都不划算。

JSONL 里那些真正巨大的记录，没有一条会变成需要分级的文本块：

- base64 图片 —— 已由 M4 外置到 `image-cache`，不再进 `Block.text`。
- `compacted` 记录 —— 读取路径根本不解析成块。
- `event_msg` / `item_completed` 的 `CommandExecution` —— 读取路径直接跳过。
- `role=user` 消息 —— 不能截断：历史回填会把半条消息重新发给模型。

回滚按「不留兼容层」的要求做干净了：删掉 `src-tauri/src/block_cache.rs`、`src/blockText.ts`、
`test/blockText.test.ts`，并逐处还原 `types.rs`、`lib.rs`、`storage_gc.rs`、`app_storage.rs`、
`types.ts`、`api.ts`、`export.ts`、`ToolResult.vue`、`style.css`、4 个语言包与 2 个测试文件；
残留的空目录 `<data_dir>/block-cache` 也一并删除。

---

### 阶段 3 — 磁盘治理 + 存储面板（✅ 已完成）

| 项 | 改动点 | 状态 |
|----|--------|------|
| D1 | `trash.rs::purge_expired(days)`；启动 + 24 h 定时；设置页「回收站保留天数」 | ✅ **默认改为 0 = 永久保留**（见下） |
| D2 | 附件从 `$TMPDIR` 迁到 `<data_dir>/attachments/YYYY-MM/`；30 天 / 500 MB 双上限；首轮顺带清 `$TMPDIR` 里 7 天前的旧文件 | ✅ 顺带修好「3 天后图片不可用」 |
| D4 | 宠物 spritesheet 先比对再写 | ✅ 原先每次 `desktop_pet_catalog` 都无条件重写 11 MB |
| D5 | 设置页「存储」面板：逐项大小 + 清理按钮 | ✅ Windows WebView2 行只报大小（见下） |
| — | `image-cache` 容量治理（承接 M4） | ✅ 500 MB 上限，与其它目录共用同一套 GC |

新增 `storage_gc.rs` 做通用目录治理：`prune(root, Policy { max_age, max_bytes }, now)` 先按
mtime 过期删，再按「最旧优先」削到字节预算内，然后收掉空目录；`spawn_maintenance(app)` 是全局
唯一的维护线程（启动跑一次，之后每 24 h 一轮），依次处理 legacy `$TMPDIR`、`attachments`、
`image-cache` 和回收站。**按 mtime 不按 atime** —— `noatime` 挂载下 atime 不可信。

**两处对计划的有意偏离，需要单独确认：**

1. **回收站保留期默认 0（永久），不是计划里的 30 天。** 升级后自动永久删除用户数据是不可逆动作，
   没有得到明确同意之前不做。设置项本身完整可用（0 / 7 / 30 / 90），面板里能直接看到回收站占多大。
   要改成默认 30 只需改 `app_storage.rs` 里一个常量。
2. **Windows 的 WebView2 行只显示大小，没有「清理缓存」按钮**（`clearable: false`）。webview 运行期间
   那些文件是锁住的，而且本机无法验证 Windows 行为，宁可先不给一个可能失败的按钮。

日志行只统计 `turn-signals.jsonl` 和 `panic.log`，**不含 hook 脚本**；清理日志是截断不是删除。
`background-media` 是用户自己放的素材，只报大小、不可清理。

### 阶段 4 — 可观测性（✅ 已完成）

`diagnostics.rs::runtime_diagnostics` 一条命令摊开 12 项：主进程 / 渲染进程 RSS、线程数、
四张常驻缓存（搜索正文字节、用量条目、扫描条目、监听路径）、运行中的 chat 数、记着的 turn 数，
以及 `image-cache` / `attachments` / 回收站三个目录的字节数。设置页「存储」底部展示，带「复制」
按钮，一份截图或一段粘贴就能定位到层。主进程或渲染进程 RSS 超 4 GB 时往 `diagnostics.log` 记一条
warn，两条之间至少隔 10 分钟（面板打开时是在轮询这条命令的）。

**渲染进程 RSS 的认领方式值得记一笔**：`com.apple.WebKit.WebContent` 由 WebKit 经 XPC 拉起，
父进程是 launchd（ppid=1），命令行里不带任何应用信息 —— 靠进程树或进程名根本认不出是谁的。
最后用一次批量 `lsof -p <pids> -Fpn` 看谁打开着本 app 的缓存 / 数据目录来认领，实测约 34 ms。

自动化验证：

| 检查 | 结果 |
|------|------|
| `npm run test:run` | 1087 passed / 85 files |
| `npx vue-tsc --noEmit` | 通过 |
| `cargo test --lib` | 547 passed / 0 failed / 3 ignored |
| `cargo build --no-default-features` | 0 warning |

新增用例：`storage_gc`（过期删、字节预算按最旧优先削、空目录回收、不跟符号链接、空目录不报错、
预算内不动）、`attachments`（按月分目录、去重命名、`clipboard-` 前缀保留以兼容既有识别、legacy
`$TMPDIR` 清理）、`trash`（`purge_expired` 只删过期项、保留期 0 不删、`deletedAt=0` 永远保留）、
`desktop_pet_assets`（内容相同不重写）、`diagnostics`（自身 pid 的 RSS / 线程数可读、warn 限流、
warn 行格式）、`SettingsModal`（存储面板加载、清理、保留期切换与失败回滚、诊断加载与复制）。

真机验证（dev 实例 + MCP 桥，2026-09-09）：

- `storage_usage` 六行全部返回真实大小：background-media 134.6 MB、desktop-pets 11.3 MB、
  image-cache 6.1 MB、logs 32 KB，合计 152.0 MB。
- `runtime_diagnostics`：主进程 79.5 MB、渲染进程 97.1 MB、35 线程，四张缓存与句柄计数都在位。
- `save_temp_image` / `save_clipboard_image` 落到 `attachments/2026-09/` 下，不再进 `$TMPDIR`。
- `clear_storage('attachments')` 清空成功；`clear_storage('backgroundMedia')` 被正确拒绝
  （"Storage item is not clearable"）；`set_trash_retention(30)` 生效并写入
  `~/Library/Application Support/cc-sessions-viewer/preferences.json`，非法值 `3000` 被拒绝。
- 设置页「存储」面板逐项渲染正常：路径 + 大小 + 打开文件夹 + 清理按钮、合计行带刷新、
  保留期下拉显示「Forever」、诊断卡片与小方块齐全。
- 「打开文件夹」按钮走 `reveal_in_finder`：目录存在时正常打开；对着一个从没建过的子目录
  点也不会静默失败 —— 后端 `nearest_existing` 退到最近存在的祖先（实测两次点击各开出
  一个正确的 Finder 窗口）。

面板视觉在这一轮做了重排（原先是一列一模一样的设置行，信息密度低）：

- 顶部一块「合计 + 分段条」：一眼看出谁占的。配色沿用 `/context` 卡片 —— 蓝色族 = 清得掉，
  中性灰 = 用户自己的素材和日志；鼠标停在某一项上时其余段淡出。
- 列表按占用从大到小排，每行是「色点 + 名称 + 中段省略的路径 + 大小 + 占比 + 两个按钮」。
- 诊断区拆成两层：主进程 / 渲染进程内存做成带刻度的卡片（4 GB 告警线当满格，≥40% 转琥珀、
  ≥75% 转红），其余七项计数做成等宽小方块。刷新 / 复制两个按钮收进标题行。
- `formatSize` 补了 GB 一档 —— 合计和内存卡片会到 GB 级，停在 `1740.8 MB` 读起来费劲。

---

## 5. 验证方法

**A. 内存曲线脚本（scratchpad，不进仓库）**

```bash
# 每 10 s 记录主进程 + WebContent 的 RSS，跑 15 分钟
PID=$(pgrep -f 'Sessions Viewer.app/Contents/MacOS/cc-sessions-viewer' | head -1)
while true; do
  main=$(ps -o rss= -p "$PID")
  web=$(ps -axo rss=,command= | grep 'WebKit.WebContent' | grep -v grep | sort -rn | head -1 | awk '{print $1}')
  threads=$(ps -M -p "$PID" | wc -l)
  echo "$(date +%T) main=$((main/1024))MB web=$((web/1024))MB threads=$threads"
  sleep 10
done
```

场景 1：打开 160 MB codex 会话 + 一个正在跑的 Claude Code 实时会话，来回切窗口焦点 20 次。
场景 2：恢复 10 个已保存 tab 启动。
场景 3：全局搜索 3 次后静置 5 分钟。

通过标准：主进程线程数不随焦点切换增长；15 分钟内 RSS 平台化（不单调上升）；WebContent 峰值较修改前下降 ≥ 60%。

**B. 单元测试（`cargo test` / `vitest`）**

- `watch.rs`：同路径二次 `watch_session` 不新建轮询线程；条数相等且无元数据变化不 emit reset；Pi 元数据变化仍 emit。
- `trash.rs`：`purge_expired` 只删过期项、保留期 0 不删。
- `turn.rs` / `panic_log.rs`：轮转后 watcher offset 正确、尾部保留。
- `renderLimits.ts` + `ToolResult.vue`：超阈值块走纯文本 / 折叠分支。
- `viewTabs.ts`：淘汰后再激活能重新加载。

**C. 手工回归清单**

实时 tail 追加与 live 标记；Pi `/rename` 后标题刷新；tab 恢复；图片显示（Claude base64、Codex 路径、失效占位）；回收站还原；导出（md/html/json）内容完整（导出走独立读取，不受大块折叠影响）；桌宠视线跟随；托盘统计数值不变。

---

## 6. 风险与明确不做的事

- **不在本轮做增量 tail 解析**：改解析器跨行状态风险高，M1 的六条已能把重复解析压到「每次真实变更一次」。
- **`markRaw` 是阶段 2 最需要小心的一步**：`ChatView.vue` 有 31 处读 `props.messages`，需逐条确认依赖的是引用变化而非深层属性；chat 会话的原地 `push` 必须全部改成引用替换，否则实时对话不刷新。用 `vitest` 覆盖 `chatSessions.ts` 的写点。
- **回收站自动清理是唯一的行为变化**，最终按「默认永久保留」落地：保留期默认 0，用户可在设置页「存储」里改成 7 / 30 / 90 天。不替用户默认删数据，同时把回收站占用摆在面板第一行。
- **大块折叠**只影响展示，不影响导出与复制；阈值集中在一个文件，可调。
- Windows 侧的 WebView2 缓存目录只在存储面板里报大小，没给清理按钮：运行期文件被锁，且本机无法验证 Windows 行为。等有 Windows 机器实测后再补。
