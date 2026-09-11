# 工具管理（MCP / Skills / Hooks / 全局配置）调研与方案

## 目标、结论与边界

给 Sessions Viewer 增加一个**全屏浮层的「工具管理」**，统一管理各 agent 的 MCP server、Skills、Hooks 和**全局 AGENT 配置**（`~/.claude/CLAUDE.md`、`~/.codex/AGENTS.md` 这类全局指令文件）。核心命题不是"再做一个 MCP 商店"，而是：**这台机器上已经装了 N 个 agent，它们的工具配置散在 7 个不同格式的文件里，没人知道当前到底生效了什么。**

本文档只出方案，不含实现。

**已确认的结论：**

- Skills 的跨 agent 共享**必须**走 symlink + Windows 三级降级，这条路径已有开源实现可参考（Skills-Manager，MIT）。第四章是这部分的完整设计。
- MCP 在各 agent 间**格式不统一**（JSON / TOML / 第三方扩展三种），不能做成"一份配置写到所有 agent"。只能做成"同一个 server 定义 → 各 agent 适配器分别落盘"。
- Hooks 的写入管线**本仓库已经有了**（`src-tauri/src/turn.rs` 已能原子写入 claude/codex/agy/grok/kimicode/pi 六家的 hook 配置），新功能是把它从"只管 turn-signal"扩展成"管任意 hook"。
- 全局 AGENT 配置是四块里**最简单**的一块（就是几个 Markdown 文件），但有两个非平凡点：`@` import 的两种路径风格，以及**改一个文件会影响多个 agent**（opencode 和 grok 都会回退去读 Claude 的 `CLAUDE.md`）。见 1.3 和 3.6。
- 明确**不做**市场/商店、不做一键安装任意 npm 包、不做 MCP 代理层。理由见第七章。

**边界：**

- 只管**用户级（user scope）**配置。项目级（`.mcp.json` / `.claude/settings.json`）第一期只读展示，不写入 —— 那是 git 管理的文件，误写会污染用户仓库。
- Pi 的 MCP **只读**。实测 Pi 没有原生 MCP 配置键，它的 MCP 由第三方 npm 扩展 `pi-mcp-adapter` 提供，配置归属不在我们手里（见 1.2）。
- 不接管已有工具的 store。用户机器上已经有 `~/.skills-manager/skills`、`~/.cc-switch/skills`、`~/.agents/skills` 三套，我们**认领而不是替代**（见 3.4）。

---

## 一、本机实测现状（问题的真实规模）

以下全部是 2026-09-10 在本机实测的数据，不是推测。

### 1.1 Skills：三套 store、14 条两跳链、26 个重复

**测量时点说明：** 首次勘察（2026-09-10 20:00）时 `~/.claude/skills` 有 47 个链接、其中 **22 个是死链（47%）**。看到数据后用户当场手工把死链清掉了，下面是清理后（同日 21:00）的复测：

```
目录                                总计   链接   断链   实体目录
~/.claude/skills                     22     22      1      0
~/.codex/skills                       9      9      1      0
~/.skills-manager/skills             39      0      0     39
~/.agents/skills                     18     17      0      1
~/.cc-switch/skills                  33      0      0     33
```

**这次清理本身就是最强的需求证据，而不是需求消失了：**

- 死链是**靠人肉发现、人肉清除**的——机器上没有任何工具告诉他"这 22 个 skill 已经失效"，是做这份调研时顺手量出来才暴露的。
- 清完还剩 2 个漏网的：`~/.claude/skills/pinme` 和 `~/.codex/skills/smux`，都指向 `../../.agents/skills/` 下已不存在的目标。**手工清理会漏，这正是要做自动检查的理由。**
- **结构性问题一个都没被解决**，下面三条清理前后完全一样。

四个发现：

**① 剩余断链 2 个，且会再次发生。** 断链的成因（三套 store 互不知情、任一方删除条目不会通知链接方）没有任何改变。这次清干净了，下次装/删 skill 还会再长出来。

**② 两跳链占比从 40/47 升到 14/22（64%）。** 实际链路是：

```
~/.claude/skills/hyperframes  →  ../../.agents/skills/hyperframes  →  ~/.skills-manager/skills/hyperframes
```

`~/.agents/skills` 自己就有 17 个 symlink 指向 `~/.skills-manager/skills`。清理删掉的主要是一跳死链，剩下的反而更集中在两跳结构上。任何一跳断掉整条就废，所以**链接健康检查不能只看第一跳**，要 resolve 到底并把完整链路显示出来。

**③ 三套 store 内容大面积重复（未变）。** `~/.skills-manager/skills` 和 `~/.cc-switch/skills` 有 26 个同名 skill；`~/.skills-manager` 和 `~/.agents` 有 18 个同名。同一个 skill 在磁盘上存两三份，改哪份生效完全看链接指向谁。

**④ 相对链接和绝对链接混用（未变）。** `~/.claude/skills` 的链接目标分布：

| 目标 store | 清理前 | 清理后 |
| --- | --- | --- |
| `../../.agents/skills`（相对链接） | 40 | 15 |
| `/Users/wuchao/.skills-manager/skills`（绝对链接） | 5 | 5 |
| `/Users/wuchao/.cc-switch/skills`（绝对链接） | 2 | 2 |

两种形式都得支持读取；写入时统一用绝对路径（见 4.5）。

### 1.2 各 agent 的配置落点（实测）

| Agent | MCP | Skills | Hooks | 全局指令 |
| --- | --- | --- | --- | --- |
| Claude Code | `~/.claude.json` 的 `mcpServers`（user/local scope）；项目级 `.mcp.json` | `~/.claude/skills/<name>/SKILL.md`；项目级 `.claude/skills/` | `~/.claude/settings.json` 的 `hooks` 键 | `~/.claude/CLAUDE.md` |
| Codex | `~/.codex/config.toml` 的 `[mcp_servers.<name>]` | `~/.codex/skills/`（实测 9 个，全是 symlink） | `~/.codex/hooks.json` | `~/.codex/AGENTS.md` |
| Grok Build | `~/.grok/config.toml`（待勘察具体键名） | `~/.grok/skills/`（存在但为空）；另有 `[marketplace]` 走 git 源 | `~/.grok/config.toml` 的 `[[hooks.<Event>]]` | `~/.grok/AGENTS.md`（+ 兼容 `CLAUDE.md`） |
| agy | `~/.gemini/settings.json` 的 `mcpServers`（本机是 `[]` 数组，不是对象，**格式待确认**） | 无独立 skills 目录 | `~/.gemini/config/hooks.json` | 无 home 级约定 |
| Pi | **无原生键**。MCP 由 npm 扩展 `pi-mcp-adapter` 提供；`~/.pi/agent/mcp-cache.json` 缓存了已连接 server 及其工具 schema | 无独立 skills 目录 | `~/.pi/agent/extensions/*.ts`（扩展，不是静态 hook 文件） | `~/.pi/agent/memory/MEMORY.md`（扩展提供） |
| opencode | `~/.config/opencode/opencode.json`（本机只有 `$schema`，未配置，键名待确认） | 无独立 skills 目录 | 待勘察 | `~/.config/opencode/AGENTS.md`（回退 `CLAUDE.md`） |
| Kimi | 本机未安装，全部待勘察 | — | — | — |

**格式分布：** MCP / Hooks 横跨 JSON（claude / agy / opencode / codex-hooks）、TOML（codex-mcp / grok）、TS 扩展（pi）三种，任何"统一一份配置"的设计在这里都会碎；只有全局指令那一列全是 Markdown，是四块里唯一格式统一的。

**已有资产：** `turn.rs` 里已经实现了 TOML 的 `atomic_write_toml`（带 `.toml.bak` 备份）和 JSON 的 `atomic_write_pi_file`（tmp + rename），以及六家 agent 的 hook 合并逻辑。新功能直接复用，不重造。

### 1.3 全局 AGENT 配置（实测）

| Agent | 全局指令文件 | 本机状态 | 依据 |
| --- | --- | --- | --- |
| Claude Code | `~/.claude/CLAUDE.md` | ✓ 888 B / 16 行 | 官方约定 |
| Codex | `~/.codex/AGENTS.md` | ✓ 29 B / 1 行（内容就是一条 import） | 官方约定 |
| Grok Build | `~/.grok/AGENTS.md`；**并额外兼容读 `CLAUDE.md`** | ✗ 未创建 | 自带文档 `~/.grok/docs/user-guide/01-getting-started.md:252` |
| opencode | `~/.config/opencode/AGENTS.md`；**不存在时回退 `~/.claude/CLAUDE.md`** | ✗ 未创建 | opencode 官方 Instructions 文档 |
| agy | **无 home 级全局约定**，只有目录级 `GEMINI.md` / `AGENTS.md`（从 cwd 往上走到 repo root） | ✗ | 自带文档 `~/.gemini/antigravity-cli/builtin/skills/agy-customizations/docs/rules.md` |
| Pi | `~/.pi/agent/memory/MEMORY.md`，由 `npm:pi-memory` 扩展维护，**不是 Pi 原生约定** | ✓ 2705 B | 本机 `settings.json` 的 `packages` 列表 |
| Kimi | 本机未安装 | — | 待勘察 |

三个必须处理的点：

**① `@` import 有两种路径风格。** 实测本机两个文件的全部内容差不多就是 import：

```
~/.claude/CLAUDE.md  第 1 行：  @RTK.md                              ← 相对（相对文件自身所在目录）
~/.codex/AGENTS.md   全文：     @/Users/wuchao/.codex/RTK.md          ← 绝对
```

编辑器要能解析 `@` 行、把被引用的文件一起列出来并可点开，否则用户在 app 里看到的"全局配置"只有一行 `@RTK.md`，等于什么都没看到。

**② 被 import 的片段已经在两个 agent 之间漂移了。** `~/.claude/RTK.md` 964 B、`~/.codex/RTK.md` 482 B，同名同用途，内容已经不一样。这和 1.1 的 skill 重复是同一个病：**同一份内容手工维护多个副本**。

**③ 改一个文件会影响多个 agent。** opencode 在自己的 `AGENTS.md` 不存在时**回退读 `~/.claude/CLAUDE.md`**，grok 也会额外读 `CLAUDE.md` 做兼容。也就是说本机现在 `~/.claude/CLAUDE.md` 实际上在给 **Claude / opencode / grok 三家**供稿，而用户大概率不知道。UI 必须把这种级联显示出来。

---

## 二、友商调研

### 2.1 Skills-Manager（最直接的对标）

`github.com/jiweiyeah/Skills-Manager`，991 star，MIT，**Tauri 2 + Rust + React 19**——和本仓库同栈，代码可直接参考。

**值得抄的：**

| 点 | 说明 |
| --- | --- |
| **三级降级建链** | symlink → junction → copy，见第四章。这是整个仓库最值钱的部分 |
| **`is_symlink_or_junction()`** | Rust 的 `is_symlink()` 对 junction 返回 false，必须额外查 `FILE_ATTRIBUTE_REPARSE_POINT` |
| **copy 模式打标记** | 降级拷贝时在目录里写 `.skills-manager-source.json`，记住"这是我管的 + 源在哪"。删除前先看这个文件，避免误删用户自己放的目录 |
| **`LinkStatus` 五态** | `Valid / Broken / WrongTarget / NotALink / Missing`——比布尔"启用/未启用"信息量大得多，1.1 里那批断链用布尔状态根本表达不出来 |
| **收编（`import_to_hub`）** | 见下方"三个必须抄的机制" |
| **删除即全量解链（`delete_skill_from_disk`）** | 见下方"三个必须抄的机制" |
| **内置 skill 编辑器** | 见下方"三个必须抄的机制" |
| **风险扫描** | `risk.rs` + `risk/rules.rs`：38 条规则、4 类（Destructive / Network / Privilege / Payload），按"是否在 Markdown 代码示例块 / 注释 / docs 目录"做置信度降权，再叠加一层可选的 LLM 二审（带缓存）。skill 是会被 agent 直接执行的代码，这一层是必要的 |
| **"不需要管理员权限"写进 README** | Windows 用户最大的顾虑，正面回答 |

#### 三个必须抄的机制

**① 收编：把散落的实体目录归拢进主 store**（`linker.rs:504 import_to_hub`）

这是解决 1.1 那种"三套 store + 实体目录散在各处"的**唯一正确入口**。流程是：

```
源是 symlink？ → read_link 并 canonicalize（相对路径按父目录解析）拿到真实目录
                ↓
rename 到 hub  → 跨文件系统失败时回退 copy_dir_all + remove_dir_all
                ↓              （删源失败会回滚掉已复制的目标）
在原位置建一条指向 hub 的软链
```

关键是**移动而不是复制**——原位置只剩链接，物理上就只有一份了，从根上消灭"改哪份生效说不清"。这条正好对应 1.1 的发现③（26 + 18 个重复）。

**② 删除即全量解链**（`commands/skills.rs:134 delete_skill_from_disk`）

删 skill 时先遍历所有已配置的工具，逐个检查链接状态，把指向这个 skill 的链接**先删掉**，最后才 `remove_dir_all` 删源目录。顺序不能反——先删源的话，链接瞬间全变死链，而此时已经没有信息知道该去哪些目录清理了。

**1.1 里那 22 个死链，本质就是"某个工具删了 skill 但没做这一步"的产物。** 这条是死链问题的根治手段，不是缓解手段。

**③ 内置 skill 编辑器**（Monaco + 文件树）

不是"SKILL.md 表单"，是一个作用域限定在 skill 目录内的**迷你 IDE**：`@monaco-editor/react` + 995 行的 `FileTree.tsx`，后端 `commands/files.rs` 提供完整的 `read_directory_tree / read_file / write_file / create_file / create_directory / delete_path / rename_path`。skill 不止 SKILL.md，还有 scripts / references / assets，只给一个 frontmatter 表单是不够用的。

同时它还有 `detect_available_editors` / `open_in_editor`，可以把整个 skill 目录甩给 VS Code / Cursor 打开。**这半边本仓库已经有了**（`lib.rs:2040 open_in_editor`，且已处理过 JetBrains 不能直接 exec 的坑），新增的只是内置编辑器那一半。

**抄机制，不抄实现：** 它为此引入了 `@monaco-editor/react`（Monaco 完整体，含 worker，压缩后仍是 MB 级）。本仓库不引第三方编辑器包，用已有的 shiki 自己搭，见 3.4。

**要抄但必须补的（上面三个机制各有一个盲区）：**

- **收编遇到同名会静默跳过。** `import_to_hub` 开头就是 `if target.exists() { return Ok(()); }`——hub 里已有同名 skill 时**直接返回成功，什么都不做，也不告诉用户**。本机 26 个 `skills-manager ∩ cc-switch` 重复条目会全部命中这条，一个都收编不了，界面上还显示操作成功。补法见 3.4 的"冲突三选一"。
- **删除只清理 `Valid` 状态的链接。** `delete_skill_from_disk` 的 match 里 `Broken` / `WrongTarget` / `NotALink` 全落进 `_ => {}` 被吞掉；而且它只遍历 `config.collect_tool_configs()`——**自己配置里认得的工具**。别的工具建的链接（cc-switch、`~/.agents` 那条链）它根本看不见。所以它的保证是"自己建的、当前健康的链接不留残骸"，不是"全机器无死链"。补法见 3.4 的"反向索引"。
- **copy 降级模式没有任何回写同步。** 全仓库搜不到 resync 逻辑，`write_copy_mode_metadata` 只在 enable 那一刻写一次。源改了、拷贝端不会更新，而 `check_link_for_tool` 只比对元数据里的 `source_path` 对不对、**不看内容漂移**，界面还会显示 `Valid`。对 skills 场景这是硬伤，第 4.4 节给补法。

**明确不抄的：**

- **`cmd /C mklink /J` 开子进程。** 能跑，但 `cmd.exe` 的引号规则和普通程序不同，路径含 `&` `^` `%` 有风险，而且每次建链弹一次进程。第 4.2 节给替代方案。
- **32 个工具硬编码。** 我们只有 7 个 agent，且已有 `AGENT_META` capability 体系，走 capability 而不是再列一张表。

### 2.2 Claude Code 原生 `/mcp` 与 scope 模型

**值得抄的是它的 scope 模型**，这是目前最成熟的一套：

| Scope | 生效范围 | 是否随仓库共享 | 存储位置 |
| --- | --- | --- | --- |
| local | 仅当前项目 | 否 | `~/.claude.json` |
| project | 仅当前项目 | 是（提交进 git） | 项目根的 `.mcp.json` |
| user | 所有项目 | 否 | `~/.claude.json` |

另外两个细节值得照搬：

- **禁用 ≠ 删除。** `/mcp` 面板里 toggle 掉的 server 记录在 `~/.claude.json` 的 `disabledMcpServers` / `enabledMcpServers`，配置本身还在。我们的 UI 必须区分"停用"和"移除"。
- **`.mcp.json` 支持环境变量展开**：`${VAR}` 和 `${VAR:-default}`。读取时要处理，否则展示出来是一串没解析的占位符。

不抄：`/mcp` 是终端里的交互列表，看不到全局（只显示当前项目生效的），也没有"哪个 server 吃了多少 token"这类信息。

### 2.3 Cline MCP Marketplace

分类浏览、按安装量/星标/最新排序、一键安装、需要 API key 时自动弹输入框并给出申请链接。体验确实好。

**但这正是要排除的方向。** 2026 年 MCP 生态最大的公开痛点就是它催生的：单个工具定义占 200–500 token，一个 93 工具的 GitHub MCP server 初始化就吃掉约 55k token，5–10 个 server 的常见组合在用户敲第一个字之前就烧掉 100k–200k 上下文。"一键装一切"的商店把这个问题放大了。

**我们要做的是反过来的事：让用户看见成本、并且方便地关掉。** 见 3.3 的 token 预算条。

### 2.4 cc-switch

用户机器上已经装了（`~/.cc-switch/skills`，33 个实体目录，2 个链接从 `~/.claude/skills` 指过来）。它主打的是 provider/配置切换，skills 是附带能力。

**教训是反面的：** 它和 Skills-Manager 各维护一套 store，互不知道对方存在，于是 1.1 里那种"同一个 skill 存三份、链接指向随机"的局面就出现了。**新功能绝不能再开第四个 store。**

### 2.5 一批 MCP 管理 GUI

`mcp-manager`（Web，Claude+Cursor 双向同步）、`MCP-Manager-GUI`（Electron，跨客户端 toggle + 导入导出）、`mcp-server-manager`（Go + HTMX，改一处同步到所有客户端）、`MCP Linker`（Tauri，跨平台）。

共性优点：**auto-discovery**（扫描已有配置文件自动导入，不要求用户手填）和**导入导出配置集**（切换不同工作场景）。这两个都值得要。

共性缺点：几乎都是"改一处 → 无条件同步到所有客户端"。这在多 agent 场景是错的——用户很可能只想让 Codex 用某个重型 MCP，不想让所有 agent 都加载它。**同步必须是按 agent 勾选的，不能是全局广播。**

### 2.6 汇总

| 抄（含必须补的） | 不抄 |
| --- | --- |
| symlink / junction / copy 三级降级 | 一键装一切的商店 |
| 链接五态健康检查（resolve 到底） | 改一处无条件全量同步 |
| **收编：移动进主 store + 原位置留链**（补：同名冲突三选一，不静默跳过） | 再开一个独立 store |
| **删除即全量解链，先解链后删源**（补：反向索引，覆盖别的工具建的链接和非 Valid 残骸） | 32 工具硬编码表 |
| **内置 skill 编辑器：文件树 + 代码编辑**（抄机制不抄实现，不引 Monaco） | `cmd /C` 开子进程建链 |
| 禁用 ≠ 删除 | copy 模式不回写（补：内容指纹 + `Stale` 态） |
| scope 模型（user / project / local） | |
| skill 风险扫描（规则 + 上下文降权） | |
| auto-discovery 自动导入现有配置 | |
| 配置集导入导出 | |
| token 预算可视化（自研，没人做） | |

---

## 三、方案：全屏浮层「工具管理」

### 3.1 形态与入口

设置弹窗（`SettingsModal.vue`）已经有 9 个 tab 了，工具管理的信息密度（每个 agent × 四类工具 × 状态）塞不进 880×640 的设置窗。而它又不该占掉一个 view tab——它是跨项目、跨 agent 的全局操作，不属于任何一个 pane。

所以：**全屏浮层**，盖在整个 app 之上，`Esc` 关闭，关掉之后回到原来的位置。

**入口在侧栏底部，和设置同一行。** 现在 `.sidebar-footer` 里只有一个占满宽度的设置按钮（`src/components/Sidebar.vue:562`，类名 `.trash-tab` 是回收站还在这儿时留下的历史遗留）。改成：设置让出右端，工具管理作为一个方形图标按钮贴在行尾。

```
┌─ .sidebar-footer ───────────────────────────────┐
│  ⚙ Settings             [↓] ●   │      🔧      │
│  └─── .trash-tab  flex:1 ───────┘  └── 30×30 ──┘│
└─────────────────────────────────────────────────┘
                           ↑              ↑
              有新版本时才出现的         工具管理
              release 按钮 + 红点
```

改动面很小：

| 位置 | 改动 |
| --- | --- |
| `Sidebar.vue` 的 `.sidebar-footer` | 从一个 button 变成两个兄弟 button；新增 `(e: 'open-tools'): void` emit，`App.vue` 接上浮层开关 |
| `.sidebar-footer`（`style.css:1479`） | `flex-direction: column` → `row`，加 `align-items: center`、`gap: 4px` |
| `.trash-tab`（`style.css:1486`） | `width: 100%` → `flex: 1; min-width: 0` |
| 新增 `.sidebar-tools-btn` | 30×30 方形，圆角 / hover 用和 `.trash-tab` 同一套 token，`flex-shrink: 0` |

三个注意点：

- **`.update-dot` / `.sidebar-release-btn` 的 `right` 偏移不用动。** 它们是 `position: absolute` 相对 `.trash-tab` 定位的（`style.css:1513` / `:1529`），设置按钮变窄之后它们跟着新的右边缘走，仍然贴在设置行尾，不会和新图标抢位置。
- **新按钮必须是兄弟节点，不能塞进设置按钮内部。** 那个 release 入口现在已经是 `<span role="button">` 嵌在 `<button>` 里了（嵌套可交互元素），不该再叠第二个；而且工具管理是常驻的，不像 release 那样只在有新版本时出现。
- **图标用 `IconWrench`**（`icons.ts:129`，ChatView 的工具调用块已经在用它，语义一致），配 `v-tooltip="t('sidebar.tools')"`。不要用 emoji —— chrome 区域的 emoji 是被刻意清掉的（见 CLAUDE.md 设计系统一节）。

快捷键给 `⌘K`：已确认没被占用——`App.vue:4060` 起那串 `key === …` 分支里没有 `k`，Rust 菜单和设置里的快捷键表也都没有。加的时候记得同步 `SettingsModal.vue:163` 的 `shortcutGroups` 全局组，否则快捷键表会缺一条。

### 3.2 信息架构

```
╭─ 工具管理 ──────────────────────────────────────────────────────────────────────── ⌘K  ✕ Esc ─╮
│   MCP      Skills     Hooks     全局配置        ⌕ 搜索…                                       │
│  ─────                                                                                        │
│                                                                                               │
│  agent  ●C  ●X  ●G  ○A  ●O  ○K  ●P        ● 已启用  ○ 已隐藏                                  │
├───────────────────────────────────────────────────────────────────────────────────────────────┤
│  ▲  2 条失效链接 · 3 套未认领的 store · 26 个重复 skill    [查看] [一键修复]                  │
├──────────────────────────────────┬────────────────────────────────────────────────────────────┤
│ 列表（虚拟滚动）                 │ 详情 / 编辑                                                │
│                                  │                                                            │
│ 条目右侧永远是一排 agent 角标，  │ 右栏随左栏选中项切换；写操作一律先出                       │
│ 一眼看出「这东西在哪些 agent     │ diff 预览再落盘。                                          │
│ 里生效」—— 这是整个设计的核心。  │                                                            │
╰──────────────────────────────────┴────────────────────────────────────────────────────────────╯
```

顶部 agent 过滤器是一排 agent 图标 toggle，默认全选。列表项右侧永远是一排 agent 角标，表示"这个东西在哪些 agent 里生效"——这是整个设计的核心：**一眼看出覆盖面**。下面每个面板的草图都按这个壳子展开。

### 3.3 MCP 面板

```
╭─ 工具管理 · MCP ────────────────────────────────────────────────────────────────────── ✕ Esc ─╮
│ 服务器 (3)           + 添加      │ chrome-devtools              [停用]  [删除]                │
│ ──────────────────────────────── │ ────────────────────────────────────────────────────────── │
│ ● chrome-devtools                │ 传输   stdio                                               │
│   stdio · 26 工具 · ~9.1k        │ 命令   npx chrome-devtools-mcp@latest                      │
│   C  X  ·  ·  O  ·  ·            │ 环境   CHROME_PATH=${CHROME_PATH:-/Applications/…}         │
│                                  │                                                            │
│ ● computer-use                   │ 在哪些 agent 生效                                          │
│   stdio · 12 工具 · ~4.2k        │   [✓] Claude     ~/.claude.json        user scope          │
│   C  ·  ·  ·  ·  ·  ·            │   [✓] Codex      ~/.codex/config.toml                      │
│                                  │   [ ] Grok       键名待勘察（阶段 0）                      │
│ ○ tauri-mcp-server               │   [✓] opencode   ~/.config/opencode/opencode.json          │
│   stdio · 18 工具 · ~5.1k        │                                                            │
│   C  X  ·  ·  ·  ·  ·            │ 上下文预算（当前启用集合）                                 │
│                                  │   ████████████░░░░░░░░░░░░░░░   18.4k / 200k   9.2%        │
│ ○ = 全部 agent 都已停用          │   这一个 server 占了其中 9.1k —— 一半                      │
╰──────────────────────────────────┴────────────────────────────────────────────────────────────╯
```

右下角那条预算条是**没有友商做的差异点**——列表里的 `C X G A O K P` 是七家 agent 的角标，亮起表示在那家生效。

- **状态点**：绿=已连接（能拿到 tool list）、黄=配置存在但未验证、红=启动失败、灰=已停用。
- **token 预算条**：面板顶部显示"当前 agent 已启用的 MCP 合计约 N 个工具 / 约 M token"，超过阈值变黄。工具数从缓存的 tool list 数（Pi 的 `mcp-cache.json` 就现成有；其它 agent 需要我们自己跑一次 `tools/list` 握手并缓存）。**这是没有友商做的差异点，也是 2.3 里那个痛点的正面解法。**
- **详情页**：transport（stdio / http / sse / ws）、command / url、env、headers、scope，以及"在哪些 agent 生效"的勾选矩阵。改完点保存，按各 agent 的适配器分别落盘。
- **auto-discovery**：首次进入扫描全部 agent 配置文件，把已有 server 按 `(command, args)` 归一化去重，同名不同定义的标记冲突让用户选。

### 3.4 Skills 面板

**核心决策：不新建 store，认领已有的。**

首次进入做一次扫描：找出所有候选 store（`~/.agents/skills`、`~/.skills-manager/skills`、`~/.cc-switch/skills`，以及各 agent skills 目录里的实体目录），让用户**指定一个主 store**，其余标记为"外部 store（只读展示）"。主 store 的路径存进我们的配置，默认建议选条目最多的那个。

```
╭─ 工具管理 · Skills ─────────────────────────────────────────────────────────────────── ✕ Esc ─╮
│ Skills (48)   主 store ▾  + 新建 │ pinme                  [编辑]  [修复]  [删除]              │
│ ──────────────────────────────── │ ────────────────────────────────────────────────────────── │
│ ● pinme              ✕ 断链      │ 主 store   ~/.skills-manager/skills/pinme   ✕ 不存在       │
│   C  X  ·  ·  ·  ·  ·            │                                                            │
│ ● smux               ▲ 两跳      │ 链路                                                       │
│   ·  X  ·  ·  ·  ·  ·            │   ~/.claude/skills/pinme                                   │
│ ● doc-writer         ⧉ 重复      │     └─ symlink →  ~/.agents/skills/pinme                   │
│   C  X  ·  ·  ·  ·  ·            │                      └─ symlink →  ✕ 目标不存在            │
│ ● git-push                       │   ~/.codex/skills/pinme                                    │
│   C  ·  ·  ·  ·  ·  ·            │     └─ symlink →  ✕ 目标不存在                             │
│                                  │                                                            │
│                                  │ 修复建议                                                   │
│ ✕ 断链   ▲ 两跳   ⧉ 同名重复     │   ○ 从 ~/.cc-switch/skills/pinme 收编为主 store 条目       │
│                                  │   ○ 直接删除这 2 条死链                                    │
│                                  │                                       [应用修复]           │
╰──────────────────────────────────┴────────────────────────────────────────────────────────────╯
```

- **链接状态五态**，按 agent 分别显示，hover 出 tooltip 说明具体问题（`Broken: 目标 ~/.agents/skills/adapt 不存在`）。
- **两跳链要 resolve 到底**再判断有效性，并在详情里画出完整链路。
- **风险徽章**：进店即扫（规则引擎，本地、毫秒级），Critical/High 的 skill 在启用前弹确认。规则参考 Skills-Manager 的四类划分，但**上下文降权是必须的**——skill 文档里写 `rm -rf` 当例子太常见了，不降权会全是误报。

#### 收编：一键归拢（对应 2.1 机制①）

"认领已有 store"只是第一步，真正把局面收干净要靠收编。对每个散落的实体目录执行：**移动进主 store → 原位置留一条指向主 store 的链接**（不是复制，物理上只留一份）。

Skills-Manager 的 `import_to_hub` 在同名冲突时静默跳过，本机 26 个重复条目会全部命中——所以**冲突必须给用户三选一**：

| 选项 | 行为 |
| --- | --- |
| 保留主 store 的 | 删掉外部那份实体目录，原位置改成指向主 store 的链接 |
| 用外部的覆盖 | 外部那份移进主 store 覆盖，其余引用全部重指 |
| 都留，重命名 | 外部那份以 `<name>-from-ccswitch` 之类的名字并存 |

冲突面板要**并排显示两份的 diff**（SKILL.md 正文 + 文件清单 + mtime），不能让用户盲选。全部同名但内容一致时可以静默走"保留主 store 的"，只在内容真有差异时才打断。

```
╭─ 收编冲突 · doc-writer ────────────────────────────────────────╮
│                                                                 │
│  两套 store 里都有 doc-writer，内容不一致                       │
│                                                                 │
│    A  ~/.skills-manager/skills/doc-writer   4 文件 12 KB  08-21 │
│    B  ~/.cc-switch/skills/doc-writer        3 文件  9 KB  06-03 │
│                                                                 │
│    差异   SKILL.md  +18 −4        scripts/run.sh  仅 A 有       │
│           [ 并排 diff ]                                         │
│                                                                 │
│    ○ 以 A 为准，B 的链接改指 A                                  │
│    ○ 以 B 为准，A 的链接改指 B                                  │
│    ○ 都保留 —— B 改名为 doc-writer-cc                           │
│                                                                 │
│  内容一致的会静默合并，不弹这个框。本机 26 个重复条目全部       │
│  走这条路 —— 静默跳过就是上游「显示成功但什么都没做」的 bug。   │
│                                                                 │
│               [全部跳过]   [跳过这个]        [应用]             │
╰─────────────────────────────────────────────────────────────────╯
```

跨文件系统的移动要有 copy + delete 回退，且删源失败时回滚已复制的目标（这条直接照抄 `import_to_hub`）。

#### 删除：反向索引 + 全量解链（对应 2.1 机制②）

**顺序固定：先解链，再删源。** 反过来的话链接瞬间全变死链，而此时已经没有信息知道该去哪些目录清理——1.1 那 22 个死链就是这么来的。

比 Skills-Manager 多做一步：**不依赖自己的配置列表，改用反向索引。** 健康扫描本来就要遍历所有 agent 的 skills 目录，顺手建一张 `真实目标路径 → [引用它的所有条目]` 的表。删除时按这张表清理，好处是：

- 能清掉**别的工具建的**链接（cc-switch、`~/.agents` 那条链），Skills-Manager 的 `collect_tool_configs()` 看不见这些。
- 能清掉状态不是 `Valid` 的同名残骸（`WrongTarget`、已经 `Broken` 的），Skills-Manager 的 `_ => {}` 会吞掉它们。
- 两跳链能一次拆干净：删 `~/.skills-manager/skills/X` 时，`~/.agents/skills/X` 和 `~/.claude/skills/X` 一起清。

删除前弹确认，**列出即将被清理的每一条路径**，用户点确认才执行。整个删除是一个事务：任一解链失败则全部回滚，绝不出现"源没了、链接还在"的中间态。

```
╭─ 删除 skill「pinme」 ──────────────────────────────────────────╮
│                                                                 │
│  将删除主 store 的实体目录                                      │
│    ~/.skills-manager/skills/pinme            3 个文件 · 12 KB   │
│                                                                 │
│  并解除全机器 4 条指向它的链接                                  │
│    ✓ ~/.claude/skills/pinme        symlink                      │
│    ✓ ~/.codex/skills/pinme         symlink                      │
│    ▲ ~/.agents/skills/pinme        两跳链的中间节点             │
│    ▲ ~/.cc-switch/skills/pinme     实体副本 —— 一并删除         │
│                                                                 │
│  反向索引来自全盘扫描，不只是我们自己建的链接 ——                │
│  Skills-Manager 只遍历自己配置里认得的工具，所以会留残骸。      │
│                                                                 │
│  [ ] 只解链，保留主 store 的实体目录                            │
│                                                                 │
│                         [取消]            [删除 5 项]           │
╰─────────────────────────────────────────────────────────────────╯
```

#### 健康修复

针对断链（1.1 剩余 2 个，且成因未除会再长出来），三个动作——重新指向主 store 里的同名 skill / 从其它 store 收编这个 skill 再指过去 / 直接删掉死链接。批量修复要能一次处理全部，不能像这次那样手工清还漏两个。

#### 编辑器（对应 2.1 机制③）

skill 不止 SKILL.md，还有 scripts / references / assets，只给一个表单不够用。

**硬约束：不引入任何第三方编辑器包**（不上 Monaco，也不上 CodeMirror）。理由：Monaco 压缩后仍是 MB 级并且要带 worker，为一个二级功能把包体翻倍不划算；而仓库里做高亮和 Markdown 的东西**已经全都有了**：

| 需要的能力 | 现成的东西 |
| --- | --- |
| 语法高亮 | `src/shikiHighlight.ts`——`createHighlighterCore` + JS regex 引擎，40 种语言按需 `import()`，skill 目录里常见的 md / sh / py / js / ts / json / yaml / toml 全覆盖 |
| 高亮体积上限 | `src/renderLimits.ts` 的 `SHIKI_MAX_CHARS`，超限退化成纯 `<pre>`（`data-shiki="skip"`），已有先例 |
| 语言识别 | `canonicalLang()` / `langLabel()` |
| 代码块复制 | `src/codeCopy.ts` |
| Markdown 渲染（SKILL.md 预览） | `src/format.ts` 的 `renderText()`，自写的渲染器，带缓存，没有第三方 md 库 |

```
╭─ 工具管理 · Skills · 编辑 doc-writer ─────────────────────────────────────────────── ⌘S 保存 ─╮
│ 文件                   │ SKILL.md                                   ● 未保存                  │
│ ────────────────────── │ ──────────────────────────────────────────────────────────────────── │
│ ▾ doc-writer           │ frontmatter                                                          │
│     SKILL.md      ●    │   name           doc-writer                                          │
│   ▾ scripts/           │   description    生成与校对项目文档…                                 │
│       run.sh           │   allowed-tools  Read, Write, Bash                                   │
│   ▸ refs/              │ ──────────────────────────────────────────────────                   │
│                        │  1  ---                                                              │
│   + 新建文件           │  2  name: doc-writer                                                 │
│                        │  3  allowed-tools: Read, Write, Bash                                 │
│                        │  4  ---                                                              │
│                        │  5                                                                   │
│                        │  6  ## 用法                                                          │
│                        │  7                                                                   │
│ 高亮走已有的 shiki，   │  8  在 `docs/` 下按模板生成…                                         │
│ 不新增第三方编辑器包。 │                                                                      │
│                        │ [ 编辑 ] [ 预览 ]        renderText() 渲染 md 预览                   │
╰────────────────────────┴──────────────────────────────────────────────────────────────────────╯
```

三层结构：

- **快捷层**：SKILL.md 的 frontmatter 表单化（`name` / `description` / `allowed-tools`）。`description` 写得烂是 skill 不触发的头号原因，表单里给写法提示。
- **完整层**：skill 目录的文件树 + 编辑区，作用域锁死在该 skill 目录内（拒绝 `../` 逃逸，后端也要再校验一次，不能只靠前端）。编辑区用**「透明 textarea 叠在高亮层上」**这个经典零依赖做法：
  - 底层 `<pre>` 放 shiki 输出的高亮 HTML，上层 `<textarea>` 文字设为 `color: transparent` 只留 `caret-color`，两层同步滚动。
  - 两层必须**同字体、同 `font-size` / `line-height` / `padding` / `white-space: pre-wrap` / `tab-size`**，差一点光标就和高亮错位。这是整个方案唯一需要下功夫的地方，也是验收要盯的点。
  - 高亮**防抖**（输入停 ~120ms 再跑），并且只在文件 ≤ `SHIKI_MAX_CHARS` 时高亮，超限直接当纯文本编辑——shiki 是 tokenizer，每次按键全量高亮大文件必卡。
  - 要自己处理的基本功只有两件：Tab 插入缩进（拦掉默认的焦点跳转）、以及用 `document.execCommand('insertText')` 写入以**保住浏览器原生的撤销栈**（直接赋值 `textarea.value` 会把 undo 历史清掉）。
  - SKILL.md 另给一个「编辑 / 预览」切换，预览直接喂给 `renderText()`。
- **甩给外部编辑器**：复用已有的 `lib.rs:2040 open_in_editor`（已处理 JetBrains 不能直接 exec 的坑，VS Code / Trae / Zed 走 bin CLI 且支持跳行），把整个 skill 目录丢过去。这半边不用新写。

**明确不做**（这些正是 Monaco 的价值，也正是我们不需要的）：多光标、代码折叠、minimap、括号匹配跳转、LSP / 自动补全、查找替换的正则模式。用户要这些就点"用外部编辑器打开"。内置编辑器的定位是**改个 prompt、调个脚本参数**，不是写项目。

写入统一走主 store 的真实路径，不经过链接写——避免某些 agent 的 skills 目录是只读挂载时写失败。

### 3.5 Hooks 面板

按 **event → agent → hook** 三级组织。

```
╭─ 工具管理 · Hooks ──────────────────────────────────────────────────────────────────── ✕ Esc ─╮
│ Hooks (5)      按事件分组 ▾      │ turn-signal                              受保护            │
│ ──────────────────────────────── │ ────────────────────────────────────────────────────────── │
│ ⊘ turn-signal        受保护      │ 本 app 自己写入的回合信号。删掉之后 GUI 聊天界面           │
│   PreToolUse  C X G A · K P      │ 收不到回合结束事件 —— 所以这条不可删、不可改。             │
│                                  │                                                            │
│ ● rtk-rewrite                    │ 事件      PreToolUse                                       │
│   PreToolUse  C · · · · · ·      │ matcher   *                                                │
│                                  │ 命令      ~/.claude/hooks/turn-signal.sh                   │
│ ● notify-lark                    │ 落点      ~/.claude/settings.json                          │
│   Stop        C X · · · · ·      │           ~/.codex/hooks.json  （另外 5 家同理）           │
│                                  │                                                            │
│ ○ pre-commit-fmt                 │ 干跑                                                       │
│   PostToolUse C · · · · · ·      │   [ 用一条假事件试跑 ]                                     │
│                                  │   exit 0 · 12 ms · 无输出 · 未阻断                         │
│ ⊘ 受保护  ● 启用  ○ 停用         │                                                            │
╰──────────────────────────────────┴────────────────────────────────────────────────────────────╯
```

Claude Code 当前的 hook 事件已经膨胀到 30 个以上（`SessionStart` / `PreToolUse` / `PostToolUse` / `PermissionRequest` / `Stop` / `PreCompact` / `SubagentStart` … ），全列出来是灾难。所以：

- **默认只显示"已配置了 hook 的事件"**，另有一个"添加 hook"的入口才展开全量事件列表，按 per-session / per-turn / tool-loop / async 四组归类。
- 各 agent 的事件集合不同（grok 有 `StopCancelled`，claude 没有；agy 是 `PreInvocation`/`Stop`），所以事件列表必须**按 agent 取交集/并集并标注支持情况**，不能写死一份。
- **本仓库自己装的 turn-signal hook 要特殊标记**为"由 Sessions Viewer 管理"，不允许在这里手删（会破坏任务状态角标），要删走设置页那个已有的重置入口。
- 危险提示：`PreToolUse` 返回 `permissionDecision: "deny"` 会直接拦住工具调用，`exit 2` 会阻塞。编辑器里对这类字段给明确警告。
- **干跑（dry-run）**：给一个"用一条假事件测试这个 hook"的按钮，把构造的 JSON 喂给 hook 命令，显示 stdout/stderr/exit code。写 hook 最痛的就是不知道为什么没生效。

### 3.6 全局配置面板

四块里最简单的一块——本质是几个 Markdown 文件的编辑器——但要把 1.3 那三个点处理掉，否则就是个没用的记事本。

左侧是 agent 列表，右侧是编辑区（直接复用 3.4 那套 textarea + shiki 叠层 + `renderText()` 预览，不另写一个编辑器）：

```
╭─ 工具管理 · 全局配置 ─────────────────────────────────────────────────────────────── ⌘S 保存 ─╮
│ 全局指令文件                     │ ~/.claude/CLAUDE.md                        888 B           │
│ ──────────────────────────────── │ ────────────────────────────────────────────────────────── │
│ ● Claude    CLAUDE.md    888 B   │ ⓘ 此文件同时被 opencode、grok 读取 —— 改它等于             │
│ ● Codex     AGENTS.md     29 B   │   同时改三家的全局指令。                                   │
│ ○ Grok      未创建      [新建]   │                                                            │
│ ↳ opencode  回退 → Claude        │  1  @RTK.md                                                │
│ ○ agy       无 home 级约定       │  2                                                         │
│ ● Pi        MEMORY.md   2.7 KB   │  3  ## 飞书通信铁律（全局）                                │
│                                  │  4                                                         │
│ ↳ = 自己没有，实际读别人的       │  ▾  @RTK.md  →  ~/.claude/RTK.md              964 B        │
│ ○ agy 那行是禁用态，不给         │      1  # RTK - Rust Token Killer                          │
│    输入框让用户白写。            │      2                                                     │
│                                  │      3  **Usage**: Token-optimized CLI proxy…              │
│                                  │                                                            │
│                                  │ ▲ 与 ~/.codex/RTK.md 已分叉  964 B vs 482 B                │
│                                  │   只提示，不自动合并          [ 并排 diff ]                │
╰──────────────────────────────────┴────────────────────────────────────────────────────────────╯
```

**① import 展开成树。** 解析 `@` 开头的行，相对路径按文件自身所在目录解析、绝对路径直接用，把被引用的文件作为子节点列出来并可点开编辑。断掉的 import（目标不存在）标红——这和 skills 的断链是同一类问题，共用一套提示。只解析第一层就够了，多层嵌套先显示"还有 N 层未展开"，不递归展开（防环）。

**② 分叉检测。** 不同 agent 引用到的**同名片段**（如两个 `RTK.md`）做内容比对，不一致时在两边都打 ⚠ 并提供并排 diff。这里**只提示、不自动合并**——全局指令是用户的个人偏好，有意分叉是完全合理的（比如给 Codex 的版本故意写得更短）。给一个"以这份为准同步过去"的显式按钮就够了。

**③ 生效链路提示。** 面板要算出并显示"这个 agent 当前实际读的是哪个文件"：opencode 的 `AGENTS.md` 不存在时标注"实际生效 `~/.claude/CLAUDE.md`（回退）"，grok 同理。反过来，编辑 `~/.claude/CLAUDE.md` 时顶部要提示"此文件同时被 opencode / grok 读取"——**这是本功能最容易踩的坑，用户以为只在改 Claude。**

**其它：**

- **capability 要能表达"不支持"**：agy 没有 home 级约定，UI 上是禁用态并说明原因，不能给个输入框让用户白写。Pi 那份是扩展提供的，标注来源。
- **新建**：文件不存在时给"创建"按钮，按该 agent 的约定路径创建，并预填一个最小模板。
- **不做 CLAUDE.md ↔ AGENTS.md 的自动双向同步**。听起来很美，但两边的约定、import 语法、被读取的时机都不同，自动同步只会制造难查的问题。要复制内容就走上面那个显式按钮。
- 项目级的 `CLAUDE.md` / `AGENTS.md` 第一期不管（和 MCP 的 project scope 同一条边界：那是 git 管理的文件）。

### 3.7 交互细节

- 所有写操作**先备份后写**，复用 `turn.rs` 的 `atomic_write_toml`（`.bak`）/ `atomic_write_pi_file`（tmp + rename）。Markdown 文件同样走 tmp + rename。
- 任何写入前显示 diff 预览（改了哪个文件的哪几行），用户确认后才落盘。这条对 `~/.claude.json` 尤其重要——那是个 52KB 的大文件，里面还有会话历史，写坏了代价高。
- 批量操作（给 5 个 agent 同时启用一个 skill）要做成一个事务：任一失败则全部回滚，不留半启用状态。
- **外部改动要能感知**：全局指令文件用户随时会在别的编辑器里直接改。已有的 `src-tauri/src/watch.rs` 是**单会话**watcher（`watch_session` 一次只盯一个文件），套不上这里，所以简单做：打开面板和窗口重新聚焦时比对 mtime，发现变了就提示重载；保存时再比对一次 mtime，不一致则拒绝写入并给 diff，避免拿旧内容覆盖掉用户的外部修改。

---

## 四、跨平台 symlink 方案

这是 Skills 面板能不能在 Windows 上活下来的关键。

### 4.1 问题

macOS/Linux 的 symlink 不分类型，就是一个存目标路径的 inode，是文件还是目录等 `open()` 时再解析。**Windows 的 symlink 在创建那一刻就要声明指向文件还是目录**，类型写进 reparse point，之后改不了。声明成 file 的链接即使目标是目录，资源管理器和大部分 API 也当文件处理——于是 skill 文件夹在 Windows 上"变成了一个文件"，agent 扫描 skills 目录时直接跳过。

三个具体触发点：

1. **建链时没声明目录类型**。Node 的 `fs.symlink` 第三参数在 Windows 才生效，先建链接后建目录时探测失败会退回 `'file'`；Rust 的 `symlink_dir` / `symlink_file` 是两个函数，选错就废。
2. **git checkout**。Git for Windows 默认 `core.symlinks=false`，仓库里的 symlink 会被检出成**一个内容是目标路径字符串的普通文本文件**。如果用户的 skills store 在 git 里，这条必中。
3. **传输过程被压平**。zip、OneDrive/iCloud 同步、scp、CI 缓存会把 symlink 解引用或存成普通文件。

### 4.2 三级降级

参考 Skills-Manager 的 `crates/core/src/services/linker.rs`，落到本仓库的写法：

```rust
/// 在 Windows 上把一个目录"挂"到 link 位置。按可靠性从高到低降级：
/// 1. 目录符号链接 —— 开发者模式或管理员下能成，语义最干净（可相对、可跨卷、可 UNC）
/// 2. 目录联接 junction —— 普通账户就能建，不需要任何提权；代价是只能指向本地绝对路径
/// 3. 整目录拷贝 —— 前两条都不行时（比如目标在网络盘）的兜底，必须打标记 + 定期回写
#[cfg(windows)]
fn link_dir(original: &Path, link: &Path) -> Result<LinkKind, String> {
    if std::os::windows::fs::symlink_dir(original, link).is_ok() {
        return Ok(LinkKind::Symlink);
    }
    if create_junction(original, link).is_ok() {
        return Ok(LinkKind::Junction);
    }
    copy_with_marker(original, link)?;
    Ok(LinkKind::Copy)
}

#[cfg(unix)]
fn link_dir(original: &Path, link: &Path) -> Result<LinkKind, String> {
    std::os::unix::fs::symlink(original, link)
        .map(|_| LinkKind::Symlink)
        .map_err(|e| format!("建链失败: {e}"))
}
```

**`create_junction` 不要开 `cmd /C mklink /J`。** 用 `junction` crate（或直接 `DeviceIoControl(FSCTL_SET_REPARSE_POINT)`），原因：`cmd.exe` 的引号规则和普通程序不同，路径含 `&` `^` `%` 有注入/解析风险；而且每次建链弹一个进程，批量启用 40 个 skill 就是 40 次。

### 4.3 检测与删除：两个必须的辅助函数

这两个不写，Windows 上的状态显示和删除会全错。

```rust
/// Windows 的 junction 不会被 Rust 的 `is_symlink()` 认出来（那只认
/// IO_REPARSE_TAG_SYMLINK），必须额外查 reparse point 属性位。
/// 只用 is_symlink() 判断"这个 skill 启用了没"，Windows 上永远返回 false。
pub fn is_link(path: &Path) -> bool {
    let Ok(meta) = path.symlink_metadata() else { return false };
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        return meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[allow(unreachable_code)]
    false
}

/// junction 和目录符号链接都是目录型的，`remove_file` 删不掉；
/// 而 `remove_dir_all` 会顺着链接把**源目录的内容**删光。只能用 `remove_dir`。
pub fn remove_link(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    { fs::remove_dir(path).or_else(|_| fs::remove_file(path)) }
    #[cfg(unix)]
    { fs::remove_file(path) }
}
```

### 4.4 copy 降级模式必须补回写同步

Skills-Manager 缺的就是这块（2.1）。降级成拷贝之后，用户在管理器里改了 skill，那个 agent 读到的永远是旧版本，而界面还显示健康。

补法：

- 拷贝时除了写 `source_path`，**再写一个内容指纹**（源目录所有文件的 `mtime + size` 哈希，不读全文，快）。
- 三个时机重新比对指纹：app 启动、skill 保存后、工具管理浮层打开时。不一致就重拷并提示。
- 健康检查里**新增第六态 `Stale`**（Skills-Manager 的五态没有这个），UI 上显示"内容已过期"并给"立即同步"按钮。

### 4.5 其它必须处理的坑

| 坑 | 说明 | 处理 |
| --- | --- | --- |
| junction 不支持 UNC / 网络路径 | store 放在网络盘或 OneDrive 同步目录时 junction 直接失败 | 静默降级 copy，但**要在 UI 上说明原因**，不能装作正常 |
| 不同运行时对 junction 判定不一致 | Rust std 认为 junction 不是 symlink；Node（libuv）把 junction 的 reparse tag 也映射成 symlink。如果某个 agent 的扫描器是 Node 写的且默认跳过 symlink，junction 会被跳过 | **逐个 agent 在 Windows 上实测**，不能推断。见 4.6 |
| 递归删除会跟进 junction | junction 在多数工具眼里就是普通目录，agent CLI 若有"清空 skills 目录"的逻辑，可能顺着链接删掉源 | 主 store 做只读标记 + 启动时校验条目数骤降则告警 |
| `core.symlinks=false` | 用户把 store 放进 git 时，Windows 上 checkout 出来是文本文件 | 检测到 store 在 git 仓库内时提示配置 `core.symlinks=true` + 开发者模式 |
| `canonicalize()` 在 Windows 返回 `\\?\` 前缀 | 路径比对时两边都要 canonicalize，不能一边原始一边规范 | 比对函数统一处理 |
| 相对 vs 绝对链接混用 | 实测本机 40 个相对 + 7 个绝对（1.1） | 读取时都 resolve 成绝对再比对；**写入统一用绝对路径**（junction 本来也只支持绝对） |

### 4.6 验证矩阵

Windows 侧必须实测这张表才算完成，不能靠推断：

| 场景 | 验证内容 |
| --- | --- |
| 开发者模式开 / 关 | 分别确认走到哪一级降级 |
| 管理员 / 普通账户 | 同上 |
| store 在本地磁盘 / OneDrive / 网络盘 | junction 成功与否 |
| 每个 agent × 每种链接类型 | **agent 能否真的识别到 skill**（这是唯一的成功标准，链接建出来不算数） |
| 源目录改动后 | copy 模式是否检测到 Stale 并同步 |
| 删除链接 | 源目录内容是否完好 |

---

## 五、后端设计

按 CLAUDE.md 的架构原则（"不要在 `lib.rs` 里加 agent 专属 match 分支，放不进 trait 说明 trait 形状错了"），新增一个模块：

```
src-tauri/src/tools/
├── mod.rs        // ToolSurface trait + source(agent) 分发
├── link.rs       // 第四章那套：link_dir / is_link / remove_link / 指纹
├── mcp.rs        // MCP 归一化模型 + 各 agent 适配器
├── skills.rs     // store 发现、链接状态、SKILL.md 解析
├── hooks.rs      // 事件目录 + 读写（复用 turn.rs 的原子写）
├── memo.rs       // 全局指令文件：路径解析、@import 解析、生效链路、分叉检测
└── risk.rs       // skill 风险规则引擎
```

trait 大致形状：

```rust
pub trait ToolSurface {
    /// 这个 agent 支持哪几类工具（有的 agent 没有 skills 目录）
    fn capabilities(&self) -> ToolCapabilities;

    fn mcp_config_path(&self) -> Option<PathBuf>;
    fn read_mcp(&self) -> Result<Vec<McpServer>, String>;
    fn write_mcp(&self, servers: &[McpServer]) -> Result<WriteReport, String>;

    fn skills_dir(&self) -> Option<PathBuf>;
    fn hooks_config_path(&self) -> Option<PathBuf>;
    fn supported_hook_events(&self) -> &'static [&'static str];
    fn read_hooks(&self) -> Result<Vec<HookEntry>, String>;
    fn write_hooks(&self, hooks: &[HookEntry]) -> Result<WriteReport, String>;

    /// 该 agent 约定的全局指令文件路径（agy 返回 None —— 它没有 home 级约定）。
    fn memo_path(&self) -> Option<PathBuf>;
    /// 约定路径不存在时实际会被读到的回退文件：opencode / grok 会落到
    /// `~/.claude/CLAUDE.md`。返回 None 表示没有回退，缺了就是没有全局指令。
    fn memo_fallback(&self) -> Option<PathBuf> { None }
}
```

全局指令那块不需要每家写一遍读写——文件都是 Markdown，`memo.rs` 里一套 `read_memo(path)` / `write_memo(path, content)` 通吃，trait 上只需要各家把**路径和回退规则**报出来。`@import` 的解析也是共用的（两种路径风格在同一个解析器里处理）。

`WriteReport` 带上"改了哪个文件、备份在哪、diff 摘要"，给 3.7 的 diff 预览用。

`ToolCapabilities` 挂到前端已有的 `AGENT_META.capabilities` 上（新增 `mcp` / `skills` / `toolHooks` / `globalMemo` 四个布尔），UI 按 capability 决定显不显示，不再加 agent 判断。agy 的 `globalMemo` 为 false，面板上是禁用态并说明原因。

Tauri command 一律走 `tools::source(&agent)?.<method>()`，和现有 `agents::source()` 的模式对齐。

---

## 六、开发阶段

### 6.0 贯穿全程的约定

这几条是仓库既有的不变量，工具管理不能破例：

| 约定 | 具体要求 |
| --- | --- |
| 后端独占文件 I/O | 前端一行 `fs` 都不写。扫描、读写、链接操作全在 `src-tauri/src/tools/` |
| 新命令三件套 | `lib.rs` 里定义 → `lib.rs:2618` 的 `generate_handler!` 注册 → `src/api.ts` 导出 + `src/types.ts` 补类型。漏第二步是最常见的低级错误 |
| 不在 `lib.rs` 写 per-agent 分支 | 和 `SessionSource` 一样，七家的差异全部收进 `ToolSurface`。命令体里出现 `match agent` 就说明 trait 形状错了 |
| 浮层放 `src/modals/` | 但那个目录在 `vitest.config.ts:31` 是**覆盖率排除**的。所以**纯逻辑必须抽成 `src/tools*.ts` 模块**（像已有的 `chatToolbar.ts` / `trashToolbar.ts`），否则这个功能等于没有单测 |
| 零新增第三方依赖 | `package.json` 不动。高亮用 `shikiHighlight.ts`，md 用 `format.ts` 的 `renderText()` |
| 不留兼容层 | 改到哪清到哪，不写双路径、不留废弃的 key 和 emit |
| 每阶段的关门检查 | `npx vue-tsc --noEmit` / `npm run test:run` / `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` / `cargo test --manifest-path src-tauri/Cargo.toml` 四条全绿才算完 |

依赖关系（能并行的地方就并行）：

```
0 勘察 ──┬─→ 1 地基 ──┬─→ 2 Skills 只读 ──→ 3 浮层壳+入口 ──→ 4 Skills 可写 ──→ 5 编辑器
         │            │                                   │
         │            └────────────────────────────────────┼─→ 6 MCP
         │                                                 ├─→ 7 Hooks
         └─────────────────────────────────────────────────┴─→ 8 全局配置
                                                                     │
                                             4·6·7·8 全部完成 ──→ 9 Windows 实测 ──→ 10 导入导出
```

6 / 7 / 8 三个面板互不依赖，壳子（阶段 3）一好就能并行开。**阶段 9 是发布前置条件，不能跳。**

---

### 阶段 0 · 勘察补全

纯读，不写代码。把 1.2 / 1.3 两张表里的"待确认"清零：

- grok / opencode / kimi 的 MCP 配置键名分别叫什么
- agy 的 `mcpServers` 是数组还是对象（本机是空数组，看不出元素形状）
- kimi 有没有 home 级全局指令文件约定

**完成定义**：两张表没有"待确认"三个字。勘察不掉的（比如某家确实没有该能力）写明"无此能力"，并在 `AgentCapabilities` 里落成 `false`。

---

### 阶段 1 · 地基

**先做一次小重构，再搭骨架。**

| 动作 | 文件 |
| --- | --- |
| 把 `atomic_write_toml`（`turn.rs:1670`）和 `atomic_write_pi_file`（`turn.rs:1016`）从 `turn.rs` 私有提升为共享 | 移到 `src-tauri/src/util.rs`，`turn.rs` 改成调用方。**是移动不是复制**——留两份就是双路径 |
| 新建 `ToolSurface` trait + `tools::surface(agent)` 分发器，形状对齐 `agents/mod.rs` 的 `SessionSource` / `source()` | `src-tauri/src/tools/mod.rs` |
| 三级降级建链、六态链接检查、内容指纹 | `src-tauri/src/tools/link.rs`（四、那一节的代码骨架直接落地） |
| 注册模块 | `src-tauri/src/lib.rs` 的 `mod` 声明区（`:15`–`:46`）加 `mod tools;` |
| 能力位 | `src/agentMeta.ts:3` 的 `AgentCapabilities` 加 `mcp` / `skills` / `toolHooks` / `globalMemo`，七家逐个填 |
| 能力位的测试 | `test/agentMeta.test.ts` 补断言（尤其 agy 的 `globalMemo: false`） |

**验证**：`cargo test` 里 `link.rs` 的单测用 tempdir 覆盖——建链/读链/断链/两跳/换目标/删链六态各一条，以及 junction 与 symlink 在 `is_link()` 下的一致性。macOS 全绿即可，Windows 留到阶段 9。

---

### 阶段 2 · Skills 只读

只读不写，先把"看得见"做出来。

| 层 | 文件 | 内容 |
| --- | --- | --- |
| 后端 | `tools/skills.rs` | store 发现、全盘反向索引、SKILL.md frontmatter 解析、风险规则引擎（含上下文降权） |
| 命令 | `lib.rs` | `tools_scan_skills` → `SkillScan`，`tools_skill_detail(name)` → `SkillDetail`（含完整链路） |
| 类型 | `src-tauri/src/types.rs` + `src/types.ts` | `SkillEntry` / `LinkHealth` / `LinkHop` / `StoreCandidate` / `RiskBadge` |
| 前端桥 | `src/api.ts` | 两个 wrapper |
| **纯逻辑** | `src/toolsSkills.ts` | 分组、过滤、搜索、角标与健康徽章的计算——**不碰 DOM，可单测** |
| 测试 | `test/toolsSkills.test.ts` | 用固定装置数据覆盖两跳链判定、同名重复归并、徽章优先级 |

**验证**：能列出 1.1 剩余的 2 个断链、14 条两跳链、26 个重复 skill；另外手工造一批死链（删掉主 store 里若干条目），检出率 100%。

---

### 阶段 3 · 浮层壳 + 侧栏入口

按 3.1 那张草图落地，这一阶段**不含任何业务功能**，只有壳和导航。

| 文件 | 改动 |
| --- | --- |
| `src/modals/ToolsModal.vue` | 新建。四个 tab 的壳、搜索框、agent 过滤器、健康条、主从两栏骨架。`Esc` 关闭 |
| `src/components/Sidebar.vue:562` | footer 从一个 button 变两个兄弟 button；新增 `(e: 'open-tools'): void` emit；图标 `IconWrench`，`v-tooltip="t('sidebar.tools')"` |
| `src/style.css:1479` / `:1486` | `.sidebar-footer` 改 row；`.trash-tab` 改 `flex: 1; min-width: 0`；新增 `.sidebar-tools-btn` |
| `src/App.vue` | 接 `@open-tools`，加 `showTools` ref 和浮层挂载 |
| `src/App.vue:4060` | 快捷键链上加 `key === 'k'` 分支 |
| `src/components/SettingsModal.vue:163` | `shortcutGroups` 全局组补一条 `⌘K` |
| `src/locales/{en,zh,zh-TW,ja}.ts` | `sidebar.tools` + `tools.tab.*` 四个 tab 名 + `tools.title` |
| `test/components/Sidebar.test.ts` | 补断言：点击新按钮 emit `open-tools`；**有新版本时 release 按钮与红点仍在设置按钮内**（这是 3.1 那条回归风险） |

**验证**：侧栏底部图标和 `⌘K` 都能开合；`updateAvailable` 为真时红点/release 按钮不跑位；侧栏拖到最窄时 Settings 文案不被挤断。改完记得**重载 dev webview**——多文件改动 HMR 会漏。

---

### 阶段 4 · Skills 可写

| 命令 | 行为 |
| --- | --- |
| `tools_adopt_skills(plan)` | 收编：移进主 store + 原位留链。同名冲突返回 `ConflictReport` 让前端弹三选一，**绝不静默跳过** |
| `tools_toggle_skill(name, agent, on)` | 启用/停用（建链/解链），不动实体目录 |
| `tools_delete_skill(name, opts)` | 先按反向索引全量解链，再删源。返回 `DeletePlan` 供确认框展示每一条路径 |
| `tools_repair_links(plan)` | 批量修复断链 |

**都要有 dry-run**：命令带 `dry_run: bool`，先返回 `WriteReport`（改哪个文件、备份在哪、diff 摘要）给前端做确认框，用户点了才真跑。事务语义：任一步失败全部回滚。

前端纯逻辑（冲突三选一的状态机、批量选择集合）抽到 `src/toolsSkillsActions.ts` + 对应测试。

**验证**：本机三套 store 收敛成一套，14 条两跳链压成一跳，26 个重复条目走完三选一，断链清零。删除前的确认框必须列全 `~/.agents` 和 `~/.cc-switch` 那两条——这是比 Skills-Manager 多做的那一步。

---

### 阶段 5 · 内置编辑器

| 文件 | 内容 |
| --- | --- |
| `src/components/CodeEditor.vue` | **叶子组件**（放 `components/` 才进覆盖率）：透明 textarea 叠在 shiki `<pre>` 上，同步滚动、Tab 缩进、`execCommand('insertText')` 保原生撤销栈、≤ `SHIKI_MAX_CHARS` 才高亮、输入防抖 ~120ms |
| `src/components/SkillEditor.vue` | 文件树 + frontmatter 表单 + 编辑/预览切换（预览走 `format.ts` 的 `renderText()`） |
| `tools/files.rs` + 3 个命令 | `tools_list_tree` / `tools_read_file` / `tools_write_file`，**作用域锁死在该 skill 目录内，后端再校验一次 `../` 逃逸**，不能只靠前端 |
| `test/components/CodeEditor.test.ts` | Tab 缩进、撤销栈、超限退化成纯文本 |

写入走主 store 的真实路径，不经过链接写。"用外部编辑器打开"复用 `lib.rs:2040` 的 `open_in_editor`，不新写。

**验证**：改完一个 skill 的 SKILL.md 和 scripts 能即时生效；`package.json` 零新增依赖；光标与高亮在中英混排、Tab 缩进、软换行下都不错位——这是本阶段唯一有技术风险的点，要专门盯。

---

### 阶段 6 · MCP 面板

`tools/mcp.rs` + `ToolSurface` 的 `read_mcp` / `write_mcp`。三件事：

1. **auto-discovery**：扫全部 agent 配置，按 `(command, args)` 归一化去重，同名不同定义标冲突。
2. **按 agent 落盘**：JSON 走 `serde_json`，TOML 走阶段 1 提到 `util.rs` 的 `atomic_write_toml`。`~/.claude.json` 是 52KB 且含会话历史，**写前备份、写后校验可解析**。
3. **token 预算**：跑一次 `tools/list` 握手缓存工具数（Pi 的 `mcp-cache.json` 现成可读），估算 token 并画预算条。这是 2.3 那个痛点的正面解法。

**验证**：本机 3 个 user-scope server 能读能改能跨 agent 同步；预算条数字和实际工具数对得上。

---

### 阶段 7 · Hooks 面板

`tools/hooks.rs` + `read_hooks` / `write_hooks` / `supported_hook_events`。

- 默认只显示"已配置了 hook 的事件"，全量事件列表藏在"添加"里。
- 事件集合**按 agent 取并集并标注支持情况**，不写死一份。
- **turn-signal 标记为受保护**：读 `turn.rs:861` 的 `turn_hook_status()` 判定，UI 上不可删不可改，要删引导去设置页已有的重置入口。
- 干跑：构造假事件喂给 hook 命令，显示 stdout / stderr / exit code。

**验证**：turn-signal 不被误删，设置页的 Hooks 状态卡片和新面板显示一致。

---

### 阶段 8 · 全局配置面板

`tools/memo.rs` + `memo_path()` / `memo_fallback()`。文件都是 Markdown，**一套 `read_memo` / `write_memo` + 一个 `@import` 解析器通吃七家**，trait 上各家只报路径和回退规则。

- `@import` 只展开第一层（防环），相对路径按文件自身目录解析、绝对路径直接用。
- 分叉检测只提示、不自动合并，给显式的"以这份为准同步过去"。
- 生效链路双向标注（编辑 CLAUDE.md 时提示"同时被 opencode / grok 读取"）。
- 外部改动：打开面板和窗口聚焦时比对 mtime，保存时再比一次，不一致拒绝盲写并给 diff。`watch.rs` 是单会话 watcher，套不上，别试。

编辑区直接复用阶段 5 的 `CodeEditor.vue`。

**验证**：两种 `@` 路径风格都能展开可编辑；级联提示正确；两个 `RTK.md` 的分叉被检出并能并排 diff。

---

### 阶段 9 · Windows 实测（发布前置）

把 4.6 那张矩阵逐格跑完：七家 agent × 三级降级（symlink / junction / copy）能否被识别到 skill。**每格要有实测结论，不是推断。**

顺带验 PowerShell 那五条不变量（`where.exe`、`&` 调用运算符、`''` 转义、`powershell_refresh_path()`、`-ExecutionPolicy Bypass`）在新加的命令里没被违反。

---

### 阶段 10 · 配置集导入导出

把"一套 MCP + skills + hooks + 全局配置"打包成可分享的配置集。放最后，前面九步稳了再说。

---

### 建议的第一刀

阶段 0 → 1 → 2 是一条直线，做完就能在终端里看到本机 skills 的真实健康度（还没有 UI）。这三步风险最低、信息收益最大，适合先落。阶段 3 的壳和入口可以和阶段 2 并行，因为它不依赖任何业务数据。

---

## 七、明确不做

| 不做 | 理由 |
| --- | --- |
| MCP / Skills 商店 | 见 2.3。"一键装一切"正是当前生态的问题来源，我们做的是相反方向的事 |
| 一键安装任意 npm 包 | 安全面太大，且装完的东西我们管不了生命周期 |
| MCP 代理层（统一网关） | 那是另一个产品，且会让"到底是谁在调工具"变得不可追 |
| 写项目级配置（`.mcp.json` / `.claude/settings.json` / 项目 `CLAUDE.md`） | 那是 git 管理的文件，第一期只读展示 |
| `CLAUDE.md` ↔ `AGENTS.md` 自动双向同步 | 见 3.6。两边的约定、import 语法、被读取的时机都不同，自动同步只会制造难查的问题；只给显式的"以这份为准同步过去"按钮 |
| 自动合并分叉的 import 片段 | 全局指令是个人偏好，有意分叉完全合理（给 Codex 的版本故意写得更短）。只提示 + 并排 diff，动手由用户决定 |
| 接管 Pi 的 MCP | Pi 没有原生 MCP 键，配置归属在第三方 npm 扩展手里，只做只读展示 |
| 第四个 skills store | 见 2.4。本机已经三套了 |
| skill 的 AI 翻译 | Skills-Manager 有，但和本 app 的定位无关 |

---

## 八、验收标准

1. **断链能被发现、被解释、被一键修复**：本机剩余的 `~/.claude/skills/pinme` 和 `~/.codex/skills/smux` 必须被检出；另外构造一批死链（删掉主 store 里的若干条目）后全量检出率 100%，修完断链数为 0。手工清理漏掉两个就是这条的反面教材。
2. **两跳链能正确 resolve 并在详情里画出完整链路**，不会把"第一跳存在但第二跳断了"报成健康。
3. **Windows 上每个 agent 都实测确认能识别到通过我们建立的链接的 skill**，4.6 的矩阵每格有结论。
4. **任何写操作前都有 diff 预览和备份**，`~/.claude.json`（52KB，含会话历史）写入前后可校验完整性。
5. **停用和删除在 UI 上是两个明确不同的操作**，停用不丢配置。
6. **删除一个 skill 后，全机器不残留任何指向它的条目**——包括 `~/.agents` 那条两跳链的中间节点、以及 cc-switch 等其它工具建的链接。删除前的确认框必须把这些路径全部列出来。
7. **收编能把本机 26 个重复条目处理干净**：内容一致的静默合并，有差异的走三选一并展示 diff；结束后主 store 之外不再有实体 skill 目录。
8. copy 降级模式能检测到源内容变化并提示同步，不会静默显示健康。
9. MCP 面板能显示当前启用集合的工具数与 token 估算。
10. 已有的 turn-signal hook 不被新功能破坏，设置页的 Hooks 状态卡片和新面板显示一致。
11. **全局指令文件的 `@import` 能展开**：`~/.claude/CLAUDE.md` 里只有一行 `@RTK.md` 时，面板上能直接看到并编辑 `~/.claude/RTK.md` 的内容；相对与绝对两种路径风格都要过。
12. **级联影响必须显式提示**：编辑 `~/.claude/CLAUDE.md` 时顶部标出"此文件同时被 opencode / grok 读取"；opencode 未创建自己的 `AGENTS.md` 时标出"实际生效的是 `~/.claude/CLAUDE.md`（回退）"。
13. **外部改动不被覆盖**：在外部编辑器改了全局指令文件后，app 内保存前能检出 mtime 变化并拒绝盲写。
14. **入口不破坏原有的更新提示**：有新版本时，设置行的红点与 release 按钮仍在设置按钮的右端，不与工具管理图标重叠或错位；侧栏在最窄宽度下设置文案不被挤断。
