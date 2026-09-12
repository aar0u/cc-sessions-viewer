# 工具管理（MCP / Skills / Hooks / 全局配置）调研与方案

## 目标、结论与边界

给 Sessions Viewer 增加一个**占满主区的「工具管理」视图**，统一管理各 agent 的 MCP server、Skills、Hooks 和**全局 AGENT 配置**（`~/.claude/CLAUDE.md`、`~/.codex/AGENTS.md` 这类全局指令文件）。核心命题不是"再做一个 MCP 商店"，而是：**这台机器上已经装了 N 个 agent，它们的工具配置散在 7 个不同格式的文件里，没人知道当前到底生效了什么。**

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
| Grok Build | `~/.grok/config.toml` 的 `[mcp_servers.<name>]`；**并默认扫描 `~/.claude.json` 与 `~/.cursor/mcp.json`** | `~/.grok/skills/`（存在但为空）；另有 `[marketplace]` 走 git 源 | `~/.grok/config.toml` 的 `[[hooks.<Event>]]` | `~/.grok/AGENTS.md`（+ 兼容 `CLAUDE.md`） |
| agy | `~/.gemini/settings.json` 的 `mcpServers`，**是对象不是数组**（本机 `{}`）；工具 schema 逐个缓存在 `~/.gemini/antigravity-cli/mcp/<server>/<tool>.json` | **`~/.gemini/config/skills/<name>/SKILL.md`**（注意是 `config/` 下面，不是 `~/.gemini/skills`）；工作区级 `.agents/` `.agent/` `_agents/` `_agent/` 四个别名下的 `skills/`，从 cwd 走到仓库根 | `~/.gemini/config/hooks.json` | 无 home 级约定 |
| Pi | **无原生键**。MCP 由 npm 扩展 `pi-mcp-adapter` 提供；`~/.pi/agent/mcp-cache.json` 缓存了已连接 server 及其工具 schema | **`$PI_AGENT_DIR`（默认 `~/.pi/agent`）`/skills`**；项目级 `<cwd>/.pi/skills`；另有一档跨 agent 的 `~/.agents/skills` 和从 cwd 走到 git root 的 `<dir>/.agents/skills`（仅项目受信任时）| `~/.pi/agent/extensions/*.ts`（扩展，不是静态 hook 文件） | `~/.pi/agent/memory/MEMORY.md`（扩展提供） |
| opencode | `~/.config/opencode/opencode.json` 的 **`mcp`** 键（不是 `mcpServers`），值形状也不同 —— 见下方 ② | **`~/.config/opencode/skill(s)/<name>/SKILL.md`**；项目级 `.opencode/skill(s)/`；另外**默认**扫 `~/.claude/skills` 和 `~/.agents/skills`（`OPENCODE_DISABLE_EXTERNAL_SKILLS` / `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` 关），以及 `opencode.json` 的 `skills.paths` | **无静态 hook 键**，靠 `plugin: string[]` 装 JS 插件（同 Pi） | `~/.config/opencode/AGENTS.md`（回退 `CLAUDE.md`）；另可用 `instructions: string[]` 追加 |
| Kimi | **独立文件** `$KIMI_CODE_HOME/mcp.json`（默认 `~/.kimi-code/mcp.json`），键是 `mcpServers`；另有项目级 `<root>/.mcp.json` 与 `<cwd>/.kimi-code/mcp.json` | `~/.kimi-code/skills/` | `~/.kimi-code/config.toml` 的 `[[hooks]]`（**数组表，不是 grok 的 `[[hooks.<Event>]]`**） | `$KIMI_CODE_HOME/AGENTS.md` |

**格式分布：** MCP / Hooks 横跨 JSON（claude / agy / opencode / kimi / codex-hooks）、TOML（codex-mcp / grok-mcp / kimi-hooks）、TS 扩展（pi）三种，任何"统一一份配置"的设计在这里都会碎；只有全局指令那一列全是 Markdown，是四块里唯一格式统一的。

阶段 0 勘察补上了三个此前"待确认"的点，其中两个改变了设计：

**① grok 默认就在读 Claude 的 MCP 配置。** 它按 `config.toml > ~/.claude.json > ~/.cursor/mcp.json > <project>/.mcp.json` 的优先级合并四个来源，同名时高优先级胜出；关掉要写 `[compat.claude] mcps = false`（依据：自带文档 `~/.grok/docs/user-guide/07-mcp-servers.md:228`）。**这意味着 1.3 ③ 那个"改一个文件影响多个 agent"的问题在 MCP 上同样存在**，而且更隐蔽——用户在 Claude 里加一个 server，grok 那边会跟着多出来。MCP 面板必须把这条来源链标出来。

**② opencode 的 MCP 是七家里形状最不一样的一个。** 键叫 `mcp` 不叫 `mcpServers`；条目用 `type: "local" | "remote"` 区分；**`command` 是一个数组，命令和参数混在一起**（`["npx", "-y", "foo"]`），没有独立的 `args`；环境变量键叫 `environment` 不叫 `env`（依据：本机 `~/.config/opencode/node_modules/@opencode-ai/sdk/dist/gen/types.gen.d.ts:946` 的 `McpLocalConfig` / `:984` 的 `McpRemoteConfig`）。归一化时 opencode 是唯一需要拆/拼 `command` 数组的一家。

**③ kimi 的 MCP 不在 `config.toml` 里，是独立的 `mcp.json`。** 三个作用域：用户级 `$KIMI_CODE_HOME/mcp.json`、项目级 `<project root>/.mcp.json`（Claude 兼容格式）、项目本地 `<cwd>/.kimi-code/mcp.json`，三者都是 `{ "mcpServers": { "<name>": {…} } }`。它自带的 `/mcp-config` skill 里特意强调 **"never assume `~/.kimi-code`"** —— 必须先解析 `KIMI_CODE_HOME`。仓库里 `agents/kimi.rs:63` 的 `kimi_home()` 已经是这个逻辑，直接复用。

顺带确认的两条：**opencode 没有静态 hook 配置**（`Config` 顶层无 `hooks` 键，只有 `plugin: string[]`，装 JS 插件），所以它和 Pi 一样，Hooks 面板对它是禁用态；**opencode 另有一个 `instructions: string[]`**，可以往全局指令里追加额外文件，3.6 的生效链路要把这条算进去。

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
| Kimi | `$KIMI_CODE_HOME/AGENTS.md`（默认 `~/.kimi-code/AGENTS.md`）；**运行时不读 `CLAUDE.md`** | ✗ 未创建 | 自带 `import-from-cc-codex` / `/mcp-config` skill 正文（从 `~/.kimi-code/bin/kimi` 提取） |

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

## 三、方案：整页「工具管理」

### 3.1 形态与入口

设置弹窗（`SettingsModal.vue`）已经有 9 个 tab 了，工具管理的信息密度（每个 agent × 四类工具 × 状态）塞不进 880×640 的设置窗。而它又不该占掉一个 view tab——它是跨项目、跨 agent 的全局操作，不属于任何一个 pane。

所以：**和统计 / 回收站同一档的整页视图**——导航占侧栏那一列、搜索和关闭占顶栏、内容占主区，`Esc` 关闭，关掉之后回到原来的位置。

> 这里原本写的是「全屏浮层（盖在整个 app 之上的居中卡片）」，阶段 3 做出来给用户看之后当场被否掉了：**不是弹框**。弹框是「打断一下」，工具管理是「进去待一会儿」，形态要和 app 自己的三段式（侧栏 / 顶栏 / 主区）对齐，而不是浮在它上面。详见 [阶段 3 · 形态修正](#阶段-3-形态修正)。

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
| `Sidebar.vue` 的 `.sidebar-footer` | 从一个 button 变成两个兄弟 button；新增 `(e: 'open-tools'): void` emit，`App.vue` 接上面板开关 |
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
│                                  │   [ ] Grok       [mcp_servers.chrome-devtools]             │
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
| 主区那半边放 `src/views/` | 但那个目录在 `vitest.config.ts:31` 是**覆盖率排除**的。所以**纯逻辑必须抽成 `src/tools*.ts` 模块**（像已有的 `chatToolbar.ts` / `trashToolbar.ts`），否则这个功能等于没有单测。导航和顶栏放 `src/components/`，那儿是测得到的 |
| 零新增第三方依赖 | `package.json` 不动。高亮用 `shikiHighlight.ts`，md 用 `format.ts` 的 `renderText()` |
| 不留兼容层 | 改到哪清到哪，不写双路径、不留废弃的 key 和 emit |
| 每阶段的关门检查 | `npx vue-tsc --noEmit` / `npm run test:run` / `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` / `cargo test --manifest-path src-tauri/Cargo.toml` 四条全绿才算完 |

依赖关系（能并行的地方就并行）：

```
0 勘察 ──┬─→ 1 地基 ──┬─→ 2 Skills 只读 ──→ 3 面板壳+入口 ──→ 4 Skills 可写 ──→ 5 编辑器
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
| 能力位 | **不进 `agentMeta.ts`**，由后端 `ToolSurface` 派生，见下 |
| 前端桥 | `tool_surfaces` 命令 + `src/api.ts` 的 `toolSurfaces()` + `src/types.ts` 的 `ToolSurfaceInfo` |

**和原计划的一处出入：能力位放后端，不放 `agentMeta.ts`。** 原本打算在 `AgentCapabilities` 上加四个布尔量，写的时候发现那会造出两份真相——`agentMeta.ts` 说某家有 skills，后端 `skills_dir()` 却返回 `None`，UI 就打开一个永远空的面板。现在的形状是：

- trait 上只有路径方法（`mcp_config_path` / `skills_dir` / `hooks_config_path` / `memo_path`），各家只报路径；
- `capabilities()` 是 trait 的**默认方法**，直接由「路径是不是 `Some`」推导，各家不能自己声明，这类不一致在结构上就不可能发生；
- 一个只读命令 `tool_surfaces` 把七家的能力位 + 落点一次给前端，浮层打开时拉一次。

这些是外部世界的事实（某家把 MCP 从 `config.toml` 挪到 `mcp.json` 就变了），存两份必然漂移；`agentMeta.ts` 里那些 `history` / `guiChat` 保持原样不动。

**验证**：`cargo test` 新增 52 条（`link.rs` 32 + `tools/mod.rs` 19 + `util.rs` 1），本机 560 → 612。`tools/` 两个文件过 `cargo fmt --check`（仓库其余部分本来就不 fmt-clean，CI 也不跑它，没去扩散改动）。

**降级复制那一级刻意写成平台无关的**，只有 junction 创建是 `#[cfg(windows)]`。它是这个模块里最危险的一段（会真的搬运和删除用户文件），整段藏在 `#[cfg(windows)]` 后面的话 macOS 单测一行都覆盖不到，要等阶段 9 才第一次跑——那时候出问题的成本高得多。现在 copy 的六条测试在 macOS 上就跑。

对抗式 review 提了四条，都改了：

| 问题 | 改法 |
| --- | --- |
| 复制用 `is_dir()` 判断递归，会**跟随嵌套链接**走出源目录、撞环；中途失败还留半成品挡住重试 | 改用 `symlink_metadata` 并**明确拒绝**嵌套链接（错误带具体路径）；复制先落到临时目录，marker 写完再整体 rename 到位，失败路径上目标始终不存在 |
| `remove_link` 只看标记文件在不在就 `remove_dir_all`，普通目录里碰巧同名就被整个删掉 | 标记文件加 `kind` 固定串，一律走解析 + 校验；删副本必须由调用方传 `expected_original` 且与记录的源一致，三项全过才删。传 `None` 只允许删真链接 |
| copy 的健康判定只比源指纹，**副本被改过测不出来**，来源也没校验 | marker 同时记源指纹和副本指纹；判定顺序改成 来源不符 → `CopyEdited` → `CopyStale`。`CopyEdited` 必须排在前面，否则按「源变了」去同步会吃掉用户在副本上的修改 |
| 快照只报 user 级 MCP，漏了 grok / kimi 的项目级来源 | `mcp_extra_sources()` 换成 `mcp_sources(cwd)`，返回带 `scope` / `origin` / `writable` 的分层来源；`tool_surfaces` 加 `cwd` 参数。kimi 三个作用域补齐，grok 四个来源补齐 |

第二轮 review 又挑出四条，也都改了：

| 问题 | 改法 |
| --- | --- |
| `cmd /C mklink /J` 把路径直接交给 shell 再解析一次。`&` `^` `\|` `(` `)` `%` 在 Windows 文件名里全合法——一个叫 `foo & bar` 的 skill 目录就能截断命令行 | 不做转义（cmd 的引用规则角落太多，写对了也难验证），改成**检测到元字符就放弃 junction**，落到复制降级——那条路完全不经过 shell。检测函数编译进所有平台，好在 macOS 上单测 |
| grok 的项目级来源只拼了单个 `cwd`，漏了逐层遍历和项目级 Cursor 配置；顺序还反了 | 按它自带文档重做：`.grok/config.toml` 从 cwd **逐层走到 git root**（越深越优先），跨来源排 `config.toml > Claude > Cursor > .mcp.json`。kimi 的顺序同样反了（它的 skill 说 "later entries override earlier"，列举顺序是 user → project-root → project-local，所以 **project-local 最高**） |
| 共享 `file_revision` 丢了 Pi 原本的「必须是普通文件」校验 —— 后面的 `rename` 会把用户链到 dotfiles 仓库的 `settings.json` 悄悄换成普通文件 | 在共享函数里恢复该校验。这是我重构时丢的，属于真实回归 |
| 源目录里碰巧有个叫 `.session-viewer-link.json` 的文件时，复制后会被 marker 覆盖；而 `fingerprint` 按**文件名**跳过该名字，嵌套目录里的同名文件改了也测不出来 | 源目录根部已有同名文件直接拒绝复制；`fingerprint` 改成只跳过**树根**那一个 |

第三轮又五条，四条照改，一条我换了改法：

| 问题 | 改法 |
| --- | --- |
| 受管副本删除有 TOCTOU：校验 marker 和 `remove_dir_all` 之间，别的进程可能把路径换成普通目录 | 改成**先原子搬走再删**：`rename` 到私有临时名 → 从搬到的位置重新读校验 marker → 才 `remove_dir_all`；对不上就原样搬回。这样递归删除作用的永远是一个刚亲自校验过、且别人已拿不到路径的目录 |
| grok 的 `[compat.claude] mcps = false` 能关掉兼容扫描，但快照无条件把 `~/.claude.json` 报成生效来源 | 真去读 `config.toml` 的 `compat.<vendor>.mcps`，环境变量 `GROK_<VENDOR>_MCPS_ENABLED` 优先。关掉就不报——报了等于骗用户「Claude 那边加的 server 在 grok 里也生效」 |
| opencode 的 `instructions: string[]` 阶段 0 确认过，模型里没有 | trait 加 `memo_extra_sources()`。只解析字面路径，带通配符的条目跳过——glob 展开要先定 cwd 和匹配语义，属于阶段 8，现在报一个不存在的路径更糟 |
| `~/.claude.json` 同时装 local 和 user scope，我按最高的 local 记了一条，于是 user scope 的 server 被显示成高于项目配置 | `McpScope` 加 `Local` 档，这个文件**报成两条**：同路径、不同 scope、不同优先级。用一条记的话必须二选一，两种都会画出错误的覆盖关系 |

第四轮三条，两条照改，一条只接受一半：

| 问题 | 改法 |
| --- | --- |
| 删真链接时完全不看期望源：用户自己建的、指向他自己某个目录的 symlink，只要占着这个位置就会被删 | `remove_link` 的第二个参数从 `Option<&Path>` 换成显式的 `RemovalIntent`：`PointingAt(expected)` 解析到底再比对，指向别处一律拒绝；`BrokenOnly` 只删解析不到任何东西的死链（健康修复要用，死链没法比对目标）。删除是唯一会毁东西的操作，意图不该用一个 `Option` 含糊过去 |
| grok 的项目级 `.mcp.json` 受 Claude import marker 影响，可能根本没加载，却被报成生效来源 | `McpSource` 加 `conditional` 标记。那个 marker 落在哪儿没有公开说明，**判定不了就别假装判定得了**——标成「可能未生效」，比直接画进覆盖链诚实 |

**marker 伪造那条只接受一半。** 两个真问题改了：`fs::read` 会跟随 symlink（一条指向别处合法 marker 的链接就能让任意目录看起来受管），现在要求 marker 必须是普通文件；marker 里加记 `copy_path`，位置对不上就不认，挡住「把受管副本整个拷走一份」和「误把这个文件复制进别的目录」。

但 review 还要求「校验两个 fingerprint」和「引入目录外的所有权登记」，这两条我没做：**前者会把合法流程堵死**——副本被就地改过（`CopyEdited`）时指纹本来就对不上，按它校验就永远删不掉了；**后者是对的方向但不属于阶段 1**——那份登记会和磁盘漂移（用户挪走目录之后登记就是错的），得连同主 store 的配置一起设计，放在阶段 3。残留风险写在 `remove_link` 的文档注释里了：受管副本的凭据终究是目录里那个 JSON，有人能往用户 home 里写就能伪造；但到那一步他直接删目录更省事，不在威胁模型内。

第五轮（最后一轮）四条，三条照改，一条只改其中不对的一半：

| 问题 | 改法 |
| --- | --- |
| 复制的临时目录名只由「链接名 + pid + 毫秒」拼成，还用 `create_dir_all` 建——它在路径已存在时直接成功，而且**跟随 symlink**。同一用户下的另一个进程能预测出这个名字并抢先放一个指向别处的链接，于是复制写到它指定的地方去，失败清理的 `remove_dir_all` 也删到那儿 | 换成独占创建：`fs::create_dir`（已存在就报 `AlreadyExists`）+ 名字里加递增计数，被抢占只是一次失败的重试，不会变成越界写。清理也只清本次调用亲手建出来的那个 |
| `source_fingerprint` 在复制**之后**才取。源在复制途中被改过的话，marker 记的是新指纹、副本装的是旧内容，此后每次健康检查都报 `CopyInSync` | 复制前后各取一次源指纹，不一致就丢掉重来（只重一轮，再不一致就报错让调用方处理）。挡不住的情况写在注释里了：指纹的 mtime 只到毫秒，源在同一毫秒内改完又改回去看着一模一样；真实编辑不会这样，不值得为它把指纹换成全文 hash（skill 目录里可能有几 MB assets，每次健康扫描都要重算） |
| Claude / kimi 的项目级来源挂在 `repo_root(cwd)` 上，**非 git 目录一条都报不出来**，而 agent 照样会读那儿的 `.mcp.json` | 加 `util::project_root()`：有 git root 用 git root，没有就用 cwd 自己。找项目配置一律走它，`repo_root` 只留给真需要「走到 git root 为止」的 grok 逐层遍历 |

**删除那条只接受一半。** Windows 分支里 `remove_dir(path).or_else(|_| fs::remove_file(path))` 是真错的——那个兜底等于说「remove_dir 不行就当文件删」，校验之后被换成普通文件就会被直接删掉。改成先 `symlink_metadata` 查类型，只删对得上的那一种。

但 review 要求给真链接也加上副本那套「先原子搬走再复查」，这条没做：副本走的是 `remove_dir_all`，被骗一次是递归删一棵树；真链接不管怎么被换，`remove_dir` / `remove_file` 拿掉的都只是一个目录项，源内容一个字节都不会少。代价差着数量级，不值得为它把每次删除都变成一次搬移。残留窗口写进 `remove_link` 的文档注释了。

**这轮还自查出一条 review 没提的**：`grok_stops_reporting_a_vendor_it_has_been_told_not_to_scan` 会改进程环境变量，而进程环境是全局的，和它并发跑的用例会偶发读到那个临时值（全量 `cargo test` 约 1/4 概率挂在 `grok_ranks_its_own_config_above_the_vendors_it_merges`）。测试模块里加了把锁，所有读 grok 来源的用例统一走 `grok_sources()`。连跑 10 遍全绿。

**没照改的一条**（第三轮）：review 要求为七家实现 `supported_hook_events()`。补全事件集合需要逐家实证（codex / agy / opencode 我没查过），现在填就是编，而那是阶段 7 的事件目录工作。但它指出的问题成立——快照里那个 `hookEvents` **永远是空数组**，UI 照着渲染就是「这个 agent 一个事件都没有」，比没有这个字段更误导。所以把这个字段从阶段 1 **删掉**，阶段 7 连同真实数据一起加。

优先级不再靠数组顺序暗示，`McpSource` 上加了显式的 `precedence`（大的覆盖小的）——顺序只能表达全序，但实际有并列（grok 的两份 Cursor 配置），而且「数组第几个」这种隐式约定传到前端之后没人守得住。

另外 Pi 的能力位改成**按扩展实际安装状态派生**：`MEMORY.md` 是 `npm:pi-memory` 维护的，没装就没人读，给编辑面板等于骗用户白写。现在去读 `~/.pi/agent/settings.json` 的 `packages` 判断。

`LinkHealth` 因此从七态变成八态，多的是 `CopyEdited`。

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

#### 阶段 2 完成记录

三个数字和 1.1 的实测**逐条对上**（`cargo test --lib report_this_machine -- --ignored --nocapture`）：

```
/Users/wuchao/.claude/skills          total=22  links=22  real=0   broken=1
/Users/wuchao/.codex/skills           total=9   links=9   real=0   broken=1
/Users/wuchao/.grok/skills            total=0   links=0   real=0   broken=0
/Users/wuchao/.kimi-code/skills       不存在
/Users/wuchao/.agents/skills          total=18  links=17  real=1   broken=0
/Users/wuchao/.skills-manager/skills  total=39  links=0   real=39  broken=0
/Users/wuchao/.cc-switch/skills       total=33  links=0   real=33  broken=0

suggested main: ~/.skills-manager/skills
summary: total=46 broken=2 two_hop=14 duplicate=26 cyclic=0
```

断的两条就是 1.1 点名的 `pinme` 和 `smux`。死链检出率由
`every_deliberately_killed_link_is_detected_none_missed` 守着：造 30 条链接、杀掉其中 10 个源，
**逐条对名字**，不抽样——漏检正是这个功能要解决的问题本身，用「大致对得上」验收等于没验。

落地与计划的三处出入：

| 计划 | 实际 | 为什么 |
| --- | --- | --- |
| 风险引擎并进 `tools/skills.rs` | 单独 `tools/risk.rs` | 第五章的模块表本来就把它单列。规则表 + 上下文分级 300 行，混进扫描模块会盖掉后者的主线 |
| 类型放 `src-tauri/src/types.rs` | 放在 `tools/skills.rs` 和 `tools/risk.rs` 里 | 和阶段 1 的 `ToolSurfaceInfo` 一致。`types.rs` 是给 `SessionSource` 那条链共享的，工具管理的形状只有 `tools/` 自己用 |
| 复用 `link.rs` 的 `LinkHealth` | 新增 `RefHealth` | `LinkHealth` 的每个状态都要先知道「应该指向哪」，而扫描发生在用户指定主 store **之前**。硬凑一个期望值只会凭空造出 `WrongTarget` |

另外三件事是写的时候才发现必须做的：

- **受管副本不能算第二份内容。** Windows 降级复制出来的副本在磁盘上是实打实的目录，不问一句就会被当成实体内容，于是每个降级过的 skill 都凭空多一个「重复」角标。`link.rs` 为此导出了 `copy_original()`。
- **文件清单不跟随链接。** 跟进去就走出这个 skill 了：统计会把别处的文件算进来，风险扫描更会把别人的脚本报成这个 skill 的问题。
- **扫描性能。** 本机一次全盘是 738 个文件 / 2.5 MB。最初实现 debug 12 秒 / release 1.3 秒，而这个面板是「进店即扫」的。两步降到 **298 ms**：15 条规则先合并成一条正则做预筛（绝大多数行什么都不命中，逐条跑等于每行扫 15 遍），各份内容之间互不相干再用 rayon 并行。预筛纯属优化，漏掉任何一条规则就是静默少报，所以 `the_prefilter_never_hides_a_rule_that_would_have_matched` 拿每条规则自己的样本正向验一遍。`tools_skill_detail` 也不再走全量 `scan()`——详情页只需要一份内容，没必要把全机器重读一遍。

**风险引擎的上下文降权**（3.4 点名要有的那条）按命中位置分四档：可执行脚本 0 级、注释行 1 级、Markdown 代码块 1 级、Markdown 散文 2 级。SKILL.md 里拿 `rm -rf /` 当反面例子太常见，不降权的话说明文档会全被报成 Critical，角标一旦全红就等于没有角标。`base_level` 原样留着，UI 要能解释「为什么它不是 Critical」。

**测试**：Rust +37（`risk.rs` 16 + `skills.rs` 21），本机 612 → 649；前端 +26（`toolsSkills.test.ts` 24 + `api.test.ts` 2），1101 → 1127。四道门全绿。

#### 阶段 2 的 review（一轮，四条全改）

| 问题 | 改法 |
| --- | --- |
| **项目级 skills 完全没进扫描范围。** `tools_scan_skills` 不接 cwd，只收各 agent 的 user 级目录 —— 而本仓库 `.claude/skills/` 下就有 7 个正在用的 skill（`git-push`、`openspec-*`、`tauri-dev-mcp`），全被报成不存在 | trait 加 `skills_sources(cwd)`，命令加 `cwd` 参数。**只填实证过的**：claude 的 `<project>/.claude/skills`（本仓库实测）、codex 的 `<project>/.codex/skills`（`~/apps/work` 下四个仓库实测）、grok 按它自己 `08-skills.md:21-29` 的来源表逐层走到 repo root。其余四家没有证据，一条不编 |
| 文件数 400 / 深度 8 触顶后静默返回，`risk` 照样当成完整结论。恶意脚本排在第 401 个文件就显示「无风险」 | `SkillBody` / `SkillEntry` / `SkillDetail` 加 `truncated`；`risk::scan_file` 返回 `skipped`。前端 `riskIsConclusive()` 据此把话说成「至少 X」。**把部分扫描结果呈现为「干净」比不扫更糟 —— 用户会据此放心** |
| 相对链接的目标没规范化就拿去去重。本机 `~/.claude/skills` 有 15 条相对链接（`../../.agents/skills/X`），逐跳 join 出来和实体目录字符串完全不同，同一份内容会被拆成两个 body，凭空长出「重复」角标 | 新增 `identity()`：身份走 `canonicalize`，**展示用的逐跳路径保持链接真正写的东西** |
| 链接指向普通文件时只判断 `exists()`，报成健康 `Linked`，可它没有任何内容 —— 用户看到一个「好端端却用不了」的 skill | 新增 `RefHealth::NotADirectory`，计入 store 的 `broken`。CLAUDE.md 里记着 git 在 Windows 上默认 `core.symlinks=false`、仓库里的 symlink 会被检出成**文本文件**，这条正是那个后果 |

顺着第一条还带出两件 review 没提的：

- **一个目录不止一家读。** grok 的 `[compat.claude] skills = true` 让它照样扫 `~/.claude/skills`，`StoreCandidate.agent` 只能记一家，等于告诉用户「这些 skill 在 grok 里用不了」。改成 `agents: Vec<String>` 按路径聚合。同时 `grok_compat_enabled` 从写死 `mcps` 改成按面分键 —— 它的 `05-configuration.md:385` 把 `skills` / `mcps` / `hooks` 各列一行，**共用一个开关就是错的**。`McpScope` / `McpOrigin` 随之更名 `ConfigScope` / `ConfigOrigin`：两个面共用一套分层规则，各起一套名字只会让「谁覆盖谁」出现两个说法。
- **`truncated` 差点变成狼来了。** 第一版把二进制和 `.git` 也算成「没扫完」，于是本机 `dm-watch` / `humanizer` 全亮着这个标记 —— 而它们只是带了张 PNG、本身是个 git 仓库。这个标记存在的全部意义是让人在**该**当真的时候当真，永远亮着就是摆设。现在 `walk` 跳过 `.git`，二进制归「扫不了」而不是「没扫成」，只有超限文本和读不了的文件才算。本机文件数 761 → 603，误报清零。

改完的本机复测（cwd = 本仓库）：

```
summary: total=52  broken=2  two_hop=15  duplicate=33  cyclic=0        扫描 291 ms
新出现：<repo>/.claude/skills  total=7 real=7   —— 之前完全看不见
```

`broken` 仍是 `pinme` / `smux` 那两条。`two_hop` 14 → 15 多的一条是 `~/.cursor/skills/create-promo-video`，之前根本没扫到（grok 的 cursor compat）。`duplicate` 26 → 33 多的七条**是真的**：本仓库 `.agents/skills/` 和 `.claude/skills/` 下各存了一份 openspec-* 和 `git-push`，`diff -rq` 确认内容一致但是两个独立的实体目录 —— 正是这个面板要暴露的那类问题，而且就在自己仓库里。

**测试**：Rust 649 → 657，前端 1127 → 1131。四道门全绿。

---

### 阶段 3 · 面板壳 + 侧栏入口

按 3.1 那张草图落地，这一阶段**不含任何业务功能**，只有壳和导航。

| 文件 | 改动 |
| --- | --- |
| ~~`src/modals/ToolsModal.vue`~~ | 新建。四个 tab 的壳、搜索框、agent 过滤器、健康条、主从两栏骨架。`Esc` 关闭。**已被形态修正删除**，见下 |
| `src/components/Sidebar.vue:562` | footer 从一个 button 变两个兄弟 button；新增 `(e: 'open-tools'): void` emit；图标 `IconWrench`，`v-tooltip="t('sidebar.tools')"` |
| `src/style.css:1479` / `:1486` | `.sidebar-footer` 改 row；`.trash-tab` 改 `flex: 1; min-width: 0`；新增 `.sidebar-tools-btn` |
| `src/App.vue` | 接 `@open-tools`，加 `showTools` ref 和面板挂载 |
| `src/App.vue:4060` | 快捷键链上加 `key === 'k'` 分支 |
| `src/components/SettingsModal.vue:163` | `shortcutGroups` 全局组补一条 `⌘K` |
| `src/locales/{en,zh,zh-TW,ja}.ts` | `sidebar.tools` + `tools.tab.*` 四个 tab 名 + `tools.title` |
| `test/components/Sidebar.test.ts` | 补断言：点击新按钮 emit `open-tools`；**有新版本时 release 按钮与红点仍在设置按钮内**（这是 3.1 那条回归风险） |

**验证**：侧栏底部图标和 `⌘K` 都能开合；`updateAvailable` 为真时红点/release 按钮不跑位；侧栏拖到最窄时 Settings 文案不被挤断。改完记得**重载 dev webview**——多文件改动 HMR 会漏。

#### 阶段 3 完成记录

在跑着的 dev app 里逐条验过（`tauri dev --features dev-mcp` + MCP driver，重载过 webview）：

| 验收项 | 实测 |
| --- | --- |
| 侧栏图标开合 | `.sidebar-footer` 的子节点是 `["trash-tab", "sidebar-tools-btn"]`，`flex-direction: row`，`.trash-tab` 的 `flex-grow: 1`。点击开面板 |
| `⌘K` 开合 | 按一次开、再按一次关；`Esc` 也关 |
| 关掉后壳状态归零 | 重开时回到 Skills tab、搜索框空 |
| 切 tab 清搜索 | 输 `hyperframes` 再切到 MCP → 搜索框清空，placeholder 变成「搜索 server…」 |
| agent 过滤器 | 7 个图标；点第一个 → 「显示 1 个」、其余 6 个压暗；再点 → 回到「显示 7 个」 |
| **红点 / release 不跑位** | 造出 `has-update` 状态实测：设置按钮 `8..205`，红点 `164..171`，release `175..197`，扳手 `209..239` —— 两者都还在设置按钮范围内，且都不与扳手重叠。3.1 预判的「跟着新的右边缘走」成立 |
| 侧栏拖到最窄 | `--sidebar-w: 220px`（`SIDEBAR_MIN_WIDTH`）下扳手仍是完整 30×30 且在侧栏内（右边缘 211 < 220）；设置按钮 `scrollWidth === clientWidth`，文案没被挤断 |

一处和计划不同：

| 计划 | 实际 | 为什么 |
| --- | --- | --- |
| （未指定层级） | `z-index: 85` | 50 是 `.app-overlay`（设置等基础弹窗），80 是右键菜单 / 全局搜索，**90 是 `.app-overlay-confirm`**。工具管理要盖住前两档，但不能压过确认框 —— 后面阶段从这个面板里弹的「确认删除 skill」正是那一档，压过去就点不到了 |

壳状态没有写进 `.vue`，而是 `src/toolsPanel.ts`（tab / agent 过滤 / 搜索词）—— 6.0 的约定：`src/modals/` 在 `vitest.config.ts:31` 是覆盖率排除的，判断留在组件里就没人测得到。两条判断值得单独说：

- **agent 过滤器「空集 = 全选」。** 如果空集表示「什么都不显示」，用户一个个点掉到最后一个时面板会突然变空，看上去像坏了。空集当全选则退化成「没有过滤」，和刚打开时一致。
- **切 tab 清搜索词、但保留 agent 过滤。** 四个面板搜的不是一类东西（server 名 / skill 名 / hook 事件 / 文件路径），带着上一个 tab 的关键词过去多半零结果，用户会以为新 tab 是空的；而「我关心哪几家」跨 tab 一直成立。

另外补了一条 review 没人提但很容易静默出问题的测试：`t()` 在 key 缺失时**原样返回 key 本身**（见 `i18n.test.ts`），UI 上就是一行 `tools.tab.mcp` 这样的字面量，不报错不崩。这次一次加了 17 个 key × 4 种语言，所以拿面板实际用到的 key 逐个断言四种语言都不返回 key 本身。

**测试**：前端 +15（`toolsPanel.test.ts` 13 + `Sidebar.test.ts` 2），1131 → 1146。四道门全绿。

#### 阶段 3 · 形态修正

把上面那个浮层拿给用户看，回来的原话是「**不是弹框**」，附了两张标注图：工具面板的三块（tab 区 / 搜索+关闭 / 主体）要分别落到 app 自己的三块上——**1 = 侧栏那一列，2 = 顶栏（nav title 在左、close icon 贴最右），3 = 主区**。

所以形态从「居中卡片 + 背板」改成「整页视图」：

| 面板的这一块 | 原来（浮层） | 现在（整页） |
| --- | --- | --- |
| 四个入口 | 卡片顶部一排横向 tab | **顶掉侧栏**，竖着排，和项目列表长一样（`.sidebar` / `.proj-item`） |
| agent 过滤器 | tab 下面一行 | 侧栏顶部那一格 —— 平时那儿就是 agent 切换器 |
| 搜索框 | 卡片右上角 | 顶栏中列，和会话 / 回收站的搜索框**同一个 x** |
| 关闭 | 卡片右上角 ×、点背板也关 | 顶栏最右（那一列平时是空的）一个 ×，**标题前面再加一个返回箭头**（用户后补的要求：整页视图要有「退回去」的入口，不能只有右上角那个 ×），`Esc` 和 `⌘K` 照旧 |
| 主体 | 卡片内的主从两栏 | 主区的主从两栏 |
| 标题 | 卡片里的「工具管理」 | 顶栏 nav title：`工具管理 / <当前面板>`；agent 首字母标记那一格让给返回按钮 —— 面板是跨 agent 的，挂个「C」只会误导 |

| 文件 | 改动 |
| --- | --- |
| `src/modals/ToolsModal.vue` | **删除**（不留兼容层） |
| `src/views/ToolsView.vue` | 新建。健康条 + 主从两栏骨架 |
| `src/components/ToolsNav.vue` | 新建。agent 过滤器 + 四个入口；复用 `.sidebar` / `.sidebar-top` / `.proj-list` / `.proj-item` |
| `src/components/topbar/ToolsTopbar.vue` | 新建。搜索（`useDebouncedSearch`，IME 安全）+ 关闭；`⌘F` 聚焦搜索框 |
| `src/App.vue` | 顶栏分发链最前面加 `ToolsTopbar`；侧栏 `v-show="sidebarOpen && !showTools"` 旁挂 `ToolsNav`；主区加 `.tools-layer`；`Esc` 收面板；`topbarContextTitle/Meta` 加分支；标题前的返回按钮（顶掉 `.topbar-agent-mark`） |
| `src/style.css` | 新增 `.tools-layer` / `.pane-grid.is-covered` / `.topbar-back-btn` |
| `src/toolsPanel.ts` | 新增 `clearToolsFilter()`（只清过滤、不动 tab），`resetToolsPanel` 复用它 |
| `src/locales/{en,zh,zh-TW,ja}.ts` | 新增 `tools.filterReset`、`tools.back` |
| `test/components/ToolsNav.test.ts` / `ToolsTopbar.test.ts` | 新建，12 条 |

三个实现上的决定，都不是随手选的：

- **不能让工具层和分屏树做 `v-if` / `v-else`。** 那样每开一次面板就把 `.pane-grid` 连同里面的终端和会话拆一次。所以工具层是 `.main` 里的绝对定位层，底下那层只加 `is-covered`（`visibility: hidden`）——和已有的 `.view-layer.is-covered` 同一套理由：`display: none` 会让虚拟列表收到 0 高的测量，回来滚动锚点就飘了。
- **工具层不铺自己的底色。** `.main` 已经铺过一层，而自定义壁纸模式下那层是**半透明**的（`:root.has-custom-background .main`）。先前铺了 `var(--bg)`，结果工具管理成了全 app 唯一一块不透壁纸的区域——第一版截图里一眼就能看出来。底下那层反正是 `visibility: hidden`，本来也不需要遮。
- **`z-index: 2`，不是 6。** 侧栏分隔条是 5，而它的判定区（`::before { inset: 0 -4px }`）向主区伸出 4px。层压过去的话，面板开着时拖侧栏的手感会突然变窄一半——`elementFromPoint` 实测过：z-index 6 时分隔条右侧 +3px 落在 `.tools-list` 上，改成 2 之后回到 `.sidebar-resizer`。

`Esc` 的归属也值得写一笔。工具管理现在不是弹窗了，所以 `Esc` 的优先级要排在**所有弹层后面**（设置 / 全局搜索 / 确认框开着时，这一下属于它们）。与其在 `App.vue` 里列一串 `show*` 布尔（每加一个弹窗就得回来补一次），不如直接问 DOM 有没有弹层挂着——但要**跳过正在淡出的那个**：`Transition` 的 `*-leave-active` 期间元素还在 DOM 上，它已经是"关掉了"的状态，不该再占着 `Esc`。

在跑着的 dev app 里逐条验过（重载过 webview）：

| 验收项 | 实测 |
| --- | --- |
| 三块的落点 | 导航 `x:0 w:248`（= `--sidebar-w`），工具层 `x:248 w:1112`，搜索框 `x:550`（和别的视图同列），关闭按钮 `x:1322`（窗宽 1360，贴最右） |
| nav title | `工具管理 / Skills`，切到 MCP 后变 `工具管理 / MCP`；agent 首字母标记不再显示（面板是跨 agent 的） |
| 返回按钮 | 标题前 `x:262 w:20 h:20`（正好是 agent 标记那一格，标题不左右跳），tooltip「返回」；点一下回到会话，agent 标记「C」原样回来 |
| 切面板 | 点 MCP → 搜索词清空、placeholder 变「搜索 server…」、主体文案跟着变 |
| 过滤器 + 重置 | 点第 3 个 agent → 只剩它 `aria-pressed=true`、其余压暗、「显示 1 个」；重置按钮这时才出现，点完回到「显示 7 个」且**仍停在 MCP** |
| 底层不被拆 | 面板开着时 `.pane-grid` 仍挂在 DOM 上、`visibility: hidden`；关掉后立刻 `visible`，标题回到 `sales-app / 列表` |
| 统计视图同理 | 在统计页开面板 → `.global-view-layer` 仍挂着且 hidden；关掉后内容原样回来 |
| 顶栏按钮不打架 | 面板开着时点「统计概览」→ 进统计（不是把看不见的统计收起来），面板同时收掉 |
| `Esc` 优先级 | 面板 + 全局搜索同时开 → `Esc` 只关搜索；面板 + 设置 → `Esc` 归设置（`SettingsModal` 本来就不接 `Esc`，于是什么都不关，面板留着）；只有面板时 `Esc` 关面板 |
| `⌘B` | 面板开着时收侧栏 → 导航跟着消失，工具层铺满 `x:0 w:1360`，关闭按钮仍在 `x:1322` |
| 侧栏拖到最窄 | `--sidebar-w: 220px` 下 7 个 agent 图标仍是 26×26 排得下（`scrollWidth === clientWidth`），四个入口不溢出 |
| 分隔条手感 | 分隔条右侧 +2px / +3px 的 `elementFromPoint` 都是 `.sidebar-resizer` |

**测试**：前端 +13（`ToolsNav` 7 + `ToolsTopbar` 5 + `toolsPanel` 的 `clearToolsFilter` 1），1146 → 1159。四道门全绿（Rust 这轮没动，657 不变）。

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

#### 阶段 4 完成记录

四个命令都在 `src-tauri/src/tools/skills_write.rs`（新文件，含 20 条测试），全部 `dry_run: bool`：
`tools_adopt_skills` / `tools_toggle_skill` / `tools_delete_skill` / `tools_repair_links`。
内部统一成 `enum Op`（`EnsureDir` / `MoveDir` / `Link` / `Unlink` / `DeleteDir`）交给 `run()` 跑，
每做成一步压一条 `Undo`，中途失败按栈倒着撤。

在跑着的 dev app 里逐条验过（`tauri dev --features dev-mcp` + MCP driver，重载过 webview，
窗口置前 —— 见下面「rAF」那条）。本机现状：46 个 skill、三套 store
（`~/.skills-manager/skills` 39 实体 / `~/.cc-switch/skills` 33 实体 / `~/.agents/skills` 1 实体），
重复 26、两跳 15、成环 0、断链 2。

| 验收项 | 实测（全部 dry-run，没有真跑） |
| --- | --- |
| 收编全部 | 34 个散落在主 store 之外的实体 → 92 步：`move 7` + `link 33` + `backup 26` + `deleteDir 26`，**1 条冲突**（`dm-watch`） |
| 三选一对话框 | `dm-watch` 两侧并排（A 主 store 31 文件 162.1 KB / B `~/.cc-switch` 22 文件 47.7 KB），逐文件列「不同 / 只在 A」，`SKILL.md +0 −0`，三个单选 + 改名输入框（预填 `dm-watch-from-cc-switch`） |
| 断链修复 | `smux`：`~/.codex/skills/smux` → `../../.agents/skills/smux`（不存在）。计划 = 解链 1 + 建链 1，重指到 `~/.cc-switch/skills/smux` |
| 两跳压平 | 17 条待修（15 两跳 + 2 断链）→ 34 步（17 解链 + 17 建链），修完断链归零 |
| **删除列全所有引用** | `hyperframes`：`解链 ~/.claude/skills/hyperframes` → `解链 ~/.agents/skills/hyperframes` → `删除 ~/.cc-switch/skills/hyperframes` → `删除 ~/.skills-manager/skills/hyperframes`。两条「用户自己都不知道存在」的引用都在框里，且两步删除排在最后 |
| 启用/停用 | `git-push` 点 Claude → 单步 `建链 ~/.claude/skills/git-push → ~/.skills-manager/skills/git-push` |
| 主区不拆分屏 | 面板开着时 `.pane-grid` 仍在 DOM 里、`visibility: hidden`；`Esc` 和顶栏返回按钮都能关，关掉 `visibility: visible`，格子原样 |
| 层级 | `.tools-layer` z-index 2 < `.sidebar-resizer` 5，侧栏拖拽区没被抢 |
| 四种语言 | 新增 92 个 key × 4 种语言，`tools.*` 四个文件 110 个 key 完全对齐 |

三处和计划不一样：

| 计划 | 实际 | 为什么 |
| --- | --- | --- |
| 「26 个重复条目走完三选一」 | 只有 **1** 条弹了对话框 | 26 条重复里 25 条两边**逐字节一样**，后端直接按「保留主 store 的」合并掉了。要是照计划让每条都弹一次，用户得点 26 次「随便哪个都行」—— 那是上游 Skills-Manager 静默跳过的反面，一样不可用。判定用的是**内容哈希**（`content_map()` / FNV-1a 64），不是 `link::fingerprint`：后者含 mtime，同样的内容换个时间戳就算"不同"，26 条会一条不落全弹出来。单测里专门断言了 fingerprint 在 mtime 上确实会分叉 |
| 「14 条两跳链」 | 15 条 | 计划是阶段 0 勘察时数的，中间机器上又多了一条 |
| `WriteStep.note` 是 `Option<String>` | 改成 `Option<StepNote>` 枚举 | 后端原来直接塞英文散文（`"dead link — nothing to restore"`），中文界面的删除确认框里就蹦出一行英文 —— 偏偏是最需要看懂的地方。改成 code 过线，四种语言各自出文案 |

几条值得单独记的判断：

- **不可逆的排最后，而且排完就不回头。** `run()` 把 `DeleteDir` 全部挪到队尾；删除开始之前失败，按 `Undo` 栈整个回滚；删除开始之后失败，**拒绝回滚**，只如实报告做到哪一步了 —— 目录已经没了，"回滚"只会把系统带到一个更说不清的状态。有测试钉住这个次序。
- **`plan_delete` 先问「这是不是链接」，再问「这是不是内容」。** 反过来写（先用 `same_path` 判断是不是某个 body）会让每一条链接都被跳过：`same_path` 会 canonicalize，链接解析之后和 body 就是同一个路径。那样删除只删实体、一条链都不解 —— 恰好制造出这个功能本来要消灭的那批断链。这条是写测试时才炸出来的。
- **收编用「备份 → 建链 → 删备份」，不是「删 → 建链」。** 中间任何一步失败，备份还在原地，`Undo::MoveBack` 能把目录整个搬回来。
- **rAF 在被遮挡的 WKWebView 里是冻住的。** 验证过程中点了取消，`.app-overlay` 停在 `fade-leave-active` 再也不动，直到把窗口置前才走完离场动画。这不是这个阶段引入的（全 app 的弹窗共用同一个 `Transition`），但 `App.vue` 里那个 Esc 守卫正是为它写的：判断"有没有弹窗挡着"时跳过带 `leave-active` 的节点，否则一个卡住的离场动画能让 Esc 永久失灵。

**测试**：Rust +20（657 → 677），前端 +25（`toolsSkillsActions.test.ts` 20 + `toolsPanel.test.ts` 5），1159 → 1184。四道门全绿。

**留给后面的**：SKILL.md 的并排逐行 diff 归阶段 5（有编辑器了才有地方摆）；这一阶段只给 +/− 行数。

**壁纸模式下的三处返工**（同一轮用户反馈）：

- **确认框要半透明。** 壁纸模式下这两个框是全屏唯一不透壁纸的色块。按 `.settings-modal`
  那条先例加 `color-mix(in srgb, var(--surface) 90%, transparent)` —— 里面全是路径和逐文件
  差异，比设置框还需要底色，所以取 90% 而不是菜单那档 78%。
- **外轮廓交给阴影，不要 border。** `.modal` 自带 `border: 1px solid var(--border)`，而
  `--shadow-lg` 的第一层本身就是 `0 0 0 1px` 描边 —— 两圈叠起来在壁纸上亮成一道硬边
  （`style.css:1835` 早有一条同样的注释）。去掉 border。
- **按钮和内容贴着了。** `.modal` 的按钮间距一直是靠 `p { margin-bottom: 18px }` 撑的，
  而这两个框最后一块是列表盒子/单选组，不是段落。给 `.modal-actions` 补 16px 上边距。

**只列本机装了的 agent**（第七轮反馈）。工具管理和会话列表在这件事上不一样：会话那边跟着
设置里勾的可见 agent 走，这边是**全机器的全景、不受设置控制** —— 所以只能按「装没装」筛。
判定加在 trait 上（`ToolSurface::config_home()` + `ToolSurfaceInfo.installed`），不是在
`lib.rs` 里写七个 match 分支。用**配置目录**而不是可执行文件：CLI 可能装在 nvm / homebrew /
自编译的任意位置，而配置目录是它第一次跑完一定会建的，也正是这个面板要读写的东西。

实测本机七家**全都装了**，所以这条筛在这台机器上是空操作 —— 用户截图里那三个灰掉的
（agy / opencode / pi）不是没装，是**它们根本没有 skills 这个机制**（阶段 0 勘察，本文
79–81 行：三家都是「无独立 skills 目录」，没有任何路径是它们会去扫的）。

这里来回了一轮，结论值得记：先试过「只列有 skills 目录的」，用户的反馈是
「既然安装了，都要显示出来，不然我怎么应用到没有 skills 的 agent？」—— 他要的不是筛掉，
是**看得见 + 告诉我为什么点不了**。原来那句「{agent} 没有 skills 目录」听着像是自己哪儿
没配好，于是他以为是功能缺失。所以最终形态是：**装了的全列**，不支持的那几个置灰，
tooltip 直说「没有 skills 这个机制，不会去扫任何 skills 目录，这里开不了；它最接近的
替代是全局指令文件（AGENTS.md / CLAUDE.md）」——顺手把用户真正想干的事指向阶段 8 的
全局配置面板。文案 key 也从 `noSkillsDir` 换成了 `noSkillsSupport`：前者描述现象，
后者说的是原因。

**文件清单做成 Git 改动视图那棵树**（第六轮反馈）。树的逻辑抽成了 `src/fileTree.ts`
（`buildFileTree` / `flattenTree` / `treeDepth`）+ 11 条单测，`GitChangesView` 一起换过去 ——
里面有条不显然的规则：**只有一个子目录、自己又不是文件的节点要和子节点合并**，写第二遍
难保和第一遍一致。**默认折起来**（第一版做成了全展开，用户当场纠正）：一个 skill 动辄 50+
个文件，全展开之后详情页下半屏全是 `references/…` 的长路径，反而看不出它由哪几块组成；
折着看到的是 `SKILL.md` + `assets 51 个文件` 这样的骨架，要细节再点开。

**异步按钮要转圈**（第五轮反馈）。点「删除」之后要先跑一趟 dry-run 才弹确认框，这中间
按钮毫无反应，看上去像没点上 —— 用户会再点一次。只有一个全局 `busy` 不够，转圈得落在
**被点的那个**按钮上，所以记的是 `pending: string | null`（`'delete'` / `'adopt'` /
`` `toggle:${agent}` `` …）。正在跑的那个按钮虽然 disabled，但用 `.running` 把透明度扳回 1
—— 转圈是它唯一的反馈，压暗了等于没有。

**「保留文件」挪到名字旁边，并且两个方向都要二次确认**（第四轮反馈）。原来它夹在
「搬进主 store / 修链接 / 删除」中间，长得像个普通选项，而它决定的是**「删除」到底删什么**。
现在紧跟 skill 名字，勾上时变成高亮胶囊；勾和取消勾各弹一次确认，把改完之后「删除」会做
什么直接说出来（取消勾那条是 `danger` 样式，因为它把破坏力调大了）。取消时把勾恢复原样
—— DOM 上的 checkbox 已经自己翻过去了，得主动翻回来。

**角标和「保留内容」看不懂**（第三轮反馈，原话：「这几个太难懂了，简短人话解释下，从用户视角
（小白，不会代码）」）。`两跳` / `成环` 是照着实现写的词 —— 用户看到的不是「链接跳几次」，
而是「这个 skill 到底能不能用」。改成动作化的说法，并给健康条上每个角标加悬停说明
（`tools.skills.badgeTip.*`，四种语言）：

| 原 | 现 | 悬停里说的 |
| --- | --- | --- |
| 重复 | 重复 | 同一个 skill 存了好几份真文件，改一份别的不跟着变 |
| 两跳 | **绕远路** | 要连走两道以上快捷方式才摸到真文件，中间断一道就全断 |
| 成环 | **绕回自己** | 快捷方式互相指、绕回自己，永远走不到真文件 |
| 断链 | **找不到** | 指过去那地方已经没东西了，agent 空手而归 |
| 保留内容 | **保留文件** | 只清掉各家 agent 的入口，文件夹留在电脑上，以后还能再启用 |

**列表交互三改**（第二轮反馈）：

- **确认框 90% → 80%。**
- **hover 换成会话列表那块跟随鼠标的浮块。** 复用现成的 `.list-spotlight`：行上 `mouseover`
  把 `offsetTop / offsetHeight` 写进 `--spot-y / --spot-h`，滚动期间隐藏、停 140ms 恢复。
  为此把 `style.css` 里的 `.scroll-area.has-spot .list-spotlight` 放宽成 `.has-spot …` ——
  工具管理的列表栏是它自己的滚动容器，但要的是同一块浮块。行本身的 `:hover` 底色去掉了，
  两个都画就是一行里两层高亮；行加 `position: relative; z-index: 1` 压在浮块之上。
  这段跟随逻辑和 `SessionsView` / `TrashView` / `ExportHistoryView` 里那三份逐字相同，
  该抽成 composable —— 但那要一次动四个文件，其中三个是在跑的视图且没有单测，
  这一轮不碰，单独记一笔。
- **行距放松**：`8px 10px` → `11px 12px`，`gap 4 → 5`（骨架同步跟上，否则加载完会「跳一下」）。
- **列表栏可拖宽。** 状态在 `toolsPanel.ts`（不是 `.vue`）：四个面板各渲染自己的
  `.tools-body`，宽度得是它们共用的一份。`[240, 560]` 硬上限之外还夹一条
  **`window.innerWidth - 420`** —— 只有静态上限的话，600px 宽的窗口里列表能拉到 560、
  详情剩 40px，而详情里是路径、链路和逐条风险点，挤没了这个面板就退化成一张普通清单。
  拖拽中不落盘，松手才写。三条都有单测（含窄窗口那条）。

**列表版式 + 骨架。** 原来角标跟在名字后面，名字长短不一，角标就在列表中间排成一条锯齿；
风险和角标又都是药丸，一行里两个药丸糊成一坨。改成：名字占满一行、状态（agent 点 + 角标 +
风险）整体贴右，风险去掉药丸底只留彩色文字，描述独占第二行。首扫要扫全机器的 skill 目录，
慢到肉眼可见，原来只有一行「扫描中…」，列表从空白直接跳成满屏 —— 换成铺满整栏的骨架
（照 `.skill-row` 的两行结构，宽度按一张不规则表循环，错开脉冲延迟），健康条那半边同样处理，
顺手挡掉了首扫完成前那个塌成小方块的空 `<select>`。骨架动画沿用 `CliEnvironmentCheck.vue`
的做法：透明度脉冲 + `prefers-reduced-motion` 关掉。

**界面上不叫「收编」。** 拿给用户看，第一句话是「收编按钮，用户不太容易看懂什么意思」——
「收编」是本文给这套机制起的内部叫法，端到界面上没人知道它要动什么。按钮文案改成直说动作的
**「搬进主 store」/「全部搬进主 store」**（四种语言同改），tooltip 把代价也说全：「把这份内容从
现在的位置移到主 store，原地留一条链接 —— 各家 agent 还是从老路径读，不会断。」文档里继续叫
收编，那是概念名；按钮上要的是动词。

**「启用于」那排：装了的都要显示。** 第一版把不支持 skills 的 agent 直接从那排里筛掉了，
用户当场否掉：「既然安装了，都要显示出来，不然我怎么应用到没有 skills 的 agent？」。改成
**列全部已装 agent**，不支持的置灰 + tooltip 说清原因。文案 key 从 `noSkillsDir` 改成
`noSkillsSupport` —— 前者（「没有 skills 目录」）描述的是现象，听着像用户哪儿没配好；
后者说的是原因。

---

#### 阶段 4 · 勘察返工：agy / opencode / Pi 三家**都支持 skills**

阶段 0 的表格里这三家写的是「无独立 skills 目录」，据此把它们的开关做成了置灰。
**这个结论是错的**，用户直接拿三家的界面截图打脸（opencode 的 Skills 面板里列着本机的
`lottie` / `dm-watch`，Pi 的 `/skill:` 补全里列着 `git-push`）。逐个翻二进制和磁盘重查，
结论如下（都写进了 `every_agent_has_a_user_skills_dir` 等三条单测，别再"研究"没了）：

| agent | 用户级目录 | 依据 |
| --- | --- | --- |
| agy | `~/.gemini/config/skills` | 1.2.1 自带定制文档「Global Configuration (Machine-Local): Path `~/.gemini/config/`」；磁盘上本机已有 `agy-web/SKILL.md` |
| opencode | `~/.config/opencode/skill`（`{skill,skills}` 都认） | 1.18.30 文档表「Global skills / External skills (auto-loaded) `~/.claude/skills/<name>/SKILL.md`, `~/.agents/skills/<name>/SKILL.md`」+ 二进制里那段扫描代码 |
| Pi | `~/.pi/agent/skills`（`$PI_AGENT_DIR` 可改） | 0.85.1 bundle 的 `getAgentDir()` + `includeDefaults` 默认档；另有 source 为 `agents` 的 `~/.agents/skills` |

**踩到的坑**：agy 差一层 —— 是 `~/.gemini/config/skills` 不是 `~/.gemini/skills`，按错的话建出来的
目录 agy 永远扫不到，而 UI 会显示成已启用。单测里专门钉了这一条。

顺带一个结构性收获：`~/.agents/skills` 是**多家都会主动扫**的跨 agent 公共目录。它因此成了
兜底主 store 的选择 —— 内容放那儿，不用建任何链接就已经有好几家读得到。

> **2026-09-11 复核，这张表当时漏了两家。** 用户实测 codex 和 kimi 也识别 `~/.agents/skills`，
> 本机支持的七家里**只有 claude 和 agy 不认**。复核证据：
>
> | agent | 读 `~/.agents/skills` | 依据 |
> | --- | --- | --- |
> | grok / opencode / pi | 是 | 各自加载器里写死，前一轮已实证 |
> | kimi | 是 | 二进制里 `USER_GENERIC_DIRS = [".agents/skills"]` join `osHomeDir`，走 `pushFirstExisting(roots, …, "user")`；project 级另有 `PROJECT_GENERIC_DIRS = [".agents/skills"]` 和 `PROJECT_BRAND_DIRS = [".kimi-code/skills"]` |
> | codex | 是 | 用户实测；0.154.0 的二进制里 `.agents/skills` 就挨着 `.codex/agents`、`.codex/hooks` 躺在同一张目录表里 |
> | claude | **否** | 只读自己的 `~/.claude/skills`（反过来是 opencode 去读它） |
> | agy | **否** | `.agents` 只出现在**工作区**四个别名里，user 级只有 `~/.gemini/config/skills` |
>
> 修的是两处：kimi 之前**根本没写 `skills_sources`**，落到默认实现上只报自己那个
> `~/.kimi-code/skills`；codex 的只有 `~/.codex/skills` + `<repo>/.codex/skills`。两家都补上了
> user 级的 `~/.agents/skills`（`ConfigOrigin::Shared`），kimi 连 project 级两档一起补。
>
> **codex 只补了 user 一档**：项目级 `<repo>/.agents/skills` 认不认没有实证，宁可少报一个，
> 也不要让面板承诺一件没验过的事。
>
> 这张表现在正反两边都钉在 `exactly_the_right_agents_read_the_cross_agent_hub` 里 ——
> 光断言「这五家读得到」不够，**必须同时断言「claude / agy 读不到」**：多报一家是骗用户
> （他以为放进公共目录就完事了，实际那家根本扫不到），少报一家会把用户明明能用的 skill
> 标成「读不到」。两个方向的错都只能靠反向断言拦住。
>
> UI 那边一个字都没改：`sharedDirTip` / `sharedDirBody` / `sharedReach` 的 `{agents}`
> 全部取自扫描结果里这个 store 的 `agents` 字段，没在前端写死过名单。

---

#### 阶段 4 · 主 store 候选规则修正

下拉里混进了两类不该出现的目录，用户圈出来的：`~/develop/flutter/sales-app/.agents/skills`
（**项目级**）和 `~/.gemini/config/skills`（**agy 自有**）。原来的规则只有「存在 + 有实体内容」，
两个都满足。补上真正的判据 —— `can_be_main = scope == User && origin == Shared`：

- **项目级目录**跟着仓库走，换个项目就没了。主 store 是全机器一份、所有 agent 链过去的地方，
  设成项目目录等于让别的项目全断链。
- **agent 自有目录**（`~/.claude/skills`、`~/.gemini/config/skills`、`~/.config/opencode/skill` …）
  是给链接落脚的地方。内容搬进去就从「一份内容所有 agent 共享」变成「绑死在某一家」，
  和这个面板要做的事正好相反。

判据放在后端（`StoreCandidate.can_be_main`），前端只读这个布尔 —— 前端原本有两处各自算了一遍
候选（`mainStoreOptions` 和面板里的 `storeOptions`），漏的正是面板那一份。现在面板直接用
`mainStoreOptions`，一份真相。

**一个 store 都没有时的兜底**：`suggested_main` 挑不出来就退到 `~/.agents/skills`，且该目录
**哪怕还不存在也要出现在下拉里**（`SkillScan.default_main`）—— 否则新机器上下拉是空的，用户
没有任何办法开始。写操作第一步本来就是 `EnsureDir`，不用额外做什么。同时放宽了
`realDirs > 0` 这条：一个空的 `~/.agents/skills` 是完全合法的起点。

另有一条迁移问题：修复前用户可能已经把不够格的目录存进了 localStorage。`effectiveMainStore`
原来只校验「还在不在」，放不掉这种（它照样存在），补上了 `canBeMain` 这一关。

---

#### 阶段 4 · Windows 路径

后端七家全从 `dirs::home_dir()`（Windows 上是 `%USERPROFILE%`）起步，加一个点目录，形状和
macOS 一致；opencode 唯一看着像 XDG 的那个，二进制里也是 `XDG_CONFIG_HOME || join(home, ".config")`，
**没有 win32 分支**，所以 `~/.config/opencode` 在 Windows 上同样成立。真正的坑在前端那几个按 `/`
硬切的函数 —— Windows 上后端回的是 `C:\Users\me\.agents\skills`：

- `shortenPath` 只判 `/` 的话每条路径都缩不掉，整个面板会摊开一屏 `C:\Users\…`；
- `inMainStore` 只判 `/` 的话 **每一条**内容都会被判成「不在主 store 里」，收编会把已经在里面的
  东西再搬一遍。

两个都改成分隔符两种都认，缩写后**保留原来的分隔符**（显示的是用户机器上真实的样子）。
`fileTree.ts` 不用改：它吃的是相对路径，后端 `walk()` 已经 `replace('\\', "/")` 过了。
符号链接本身（junction）的 Windows 行为仍留在阶段 9 实测。

---

#### 阶段 4 · 又一轮界面反馈

| 反馈 | 改动 |
| --- | --- |
| 「即使点开工具，底部的这个还是要保留显示」 | 侧栏底部那一行（设置 + 工具管理）抽成 `SidebarFooter.vue`，`Sidebar` 和 `ToolsNav` 共用。工具管理开着时扳手是 active 态，点它退回会话。抽组件不是为了省字：写两份的话「有新版本」的红点和 release 入口迟早只在一边更新 |
| 「主 store 下拉框太丑了，请自定义写样式」 | 换成设置里那套 `.set-dropdown-*` 自定义菜单（那套本来就是为了顶掉原生 `<select>` 写的）。按钮上只留路径，「N 个实体」挪进菜单右侧对齐 |
| 「增加标题：选择你认为的主目录」 | 菜单顶部加一行带下边框的说明 —— 几条路径长得很像，光看一列路径分不出哪个是「内容该待的地方」 |
| 「弹框透明度改成 86%」 | `.skill-store-menu` 在壁纸模式下单独覆盖到 86%（一般浮层是 78%）：里面是一列长得很像的路径，78% 时底图纹理会从字缝里透上来 |
| 「hover 后增加固定的功能」 | 列表行加置顶。状态按**名字**记在 `localStorage`（同名的重复条目本来就合成一行，按路径记的话收编搬完家置顶就丢了），排序上**压过 badge 和风险** —— 自动排序猜的是「你大概最该先看哪个」，置顶是用户自己说的「我就要看这个」 |
| 「置顶按钮应该放在我标记的位置，不然上面太挤了」 | 从顶行的角标区挪到描述行右端。顶行已经有「N 家 agent + 角标 + 风险」三组东西，再插一颗按钮名字那半边就只剩几个字 |

置顶按钮已置顶的一直显示，行左侧另给一条 brand 竖条（和会话卡片的 `.sess-pinned` 同一套记号）。

**「置顶不要默认给它让位置」。** 第一版做成 `opacity: 0` 占位，理由是 hover 时描述宽度不变、
省略号不会闪。用户否掉了：预留一格会让**每一行**的描述都短一截，而绝大多数行永远不会被置顶。
改成 `display: none` → hover 时才 `inline-flex`，代价是 hover 那一刻描述重排一下。

---

#### 阶段 4 · 「在文件管理器中显示」

详情页「内容」的每条路径、主 store 下拉的每个选项，后面各加一颗文件夹按钮。复用已有的
`api.revealInFinder` 和 **`list.action.reveal`** 这个文案 key —— 一开始新建了个
`tools.skills.reveal`（写成「在访达中打开」），发现库里早有一条平台中立的「在文件管理器中显示」，
新造一个只会让同一件事在 app 里有两个说法，删掉了。

两处细节：

- 下拉里的那颗必须 `@click.stop`，否则点文件夹会顺手把主 store 换掉。行本身是 `<button>`，
  所以它是 `<span role="button">`（同 `.sidebar-release-btn` 的理由）。
- 内容行**不给** `margin-left: auto`。贴到行尾的话图标离它描述的那条路径隔了半屏空白，
  点之前还得确认一下自己点的是哪一条。

后端 `reveal_in_finder` 会退到最近一个存在的祖先，所以还没建出来的兜底主 store
（`~/.agents/skills`）点了不会报错，而是打开 `~/` —— 正是想要的：用户想看的是那地方现在长什么样。

主 store 下拉的壁纸模式透明度从 86% 再提到 **92%**（一般浮层 78%）。

---

#### 阶段 4 · 公共目录 `~/.agents/skills` 是一等公民

**起因是一条假阴性。** 详情页的 agent 开关只认「这家自己的 skills 目录里有没有那条链接」，
而另外几家（codex / grok / kimi / opencode / pi，见 1210 行附近那张复核表）还会去扫跨 agent 的
`~/.agents/skills`。本机 `tailwind` 因此在
**同一屏上自相矛盾**：列表行的角标是 `claude · grok · opencode · pi`，详情里只有 claude 亮着。
点一下那个假装关着的 opencode，就会在 `~/.config/opencode/skill/tailwind` 造一条指向它**本来
就读得到**的内容的软链 —— 正是这个面板要清理的「绕远路 / 重复」，由面板自己制造出来。

`toolsSkills.ts` 加了 `agentReach(entry, agent, ownDir)`，三态：

| 态 | 含义 | 开关行为 |
| --- | --- | --- |
| `off` | 一条健康引用都没有 | 点「启用」→ 在自有目录建链 |
| `own` | 自有目录里有一条 | 点「停用」→ 只拆自己这条 |
| `shared` | 只靠共用目录读到 | 两个方向都拦住并说清原因：这儿建链是多余的，这儿拆链会连着断别家 |

**然后是用户追问的那一步：「即使用户选的主目录不是它，也需要在它里面建立链接，对吧？」** 对。
收编只保证「内容从哪儿搬走、原位就留一条链接」——**内容本来就不在公共目录的 skill，不会自己
长过去**。主 store 一旦挑成 `~/.cc-switch/skills`，那三家就读不到，除非往三个自有目录各塞一条
链接：3 条链接干 1 条的活。

所以详情页「启用于」那排最右加了一个 `~/.agents/skills` 开关（一条竖分隔线隔开，长得是路径不是
图标 —— 它不是第八家 agent）。第一版只有一个描边小药丸 + 路径，用户说「看着都不像个开关，像静态
文案」：换成设置里那套轨道 + 圆点（`.set-toggle-track` / `.set-toggle-thumb`，小一号成 26×15 —— 这
排里 agent 图标才 15px，标准的 34×20 会把整行撑高一截），描边去掉，一眼看出是能拨的。走的是同一条
`tools_toggle_skill`，只是 store 换成公共目录；
`toggle` 本来就不认「这是谁的目录」，零后端改动。公共目录同时是主 store（内容实体就在里面）时，
说明文案讲清「这儿没有链接可拆」，而不是给一个点不动的开关。

---

#### 阶段 4 · 新增主目录

内置候选只有各家 agent 自己声明的目录 + 三个第三方管理器（`SHARED_STORES`）。skill 放在外置盘、
同步盘、自己的 dotfiles 仓库时一个都不合用，下拉底部因此加了「新增主目录…」（`plugin-dialog`
的目录选择器，与设置里换数据目录同一套）。

**关键是这份列表必须传给后端。** 纯前端记一个路径是错的：后端不扫的目录不能当主 store ——
收编会把内容搬进去，下一次扫描却看不见，UI 上就是「skill 凭空消失了」。所以
`tools_scan_skills` / `tools_skill_detail` / `tools_delete_skill` 三条命令都多收一个
`extra: Vec<String>`，前端每次调用都带上（`toolsExtraStores:v1`）。删除那条尤其不能漏:
反向索引漏了自定义目录，删完会在那儿留一条死链，而清死链正是这个面板的存在理由之一。

后端不落盘这份列表，理由和主 store 一样：存两份就有两个真相，用户在别处把目录挪走之后
后端那份还是错的。

两条挡板：

- **挑中的目录不够格就拒绝**，不能因为「用户说了算」放行。用户在对话框里翻到 `~/.claude/skills`
  是很容易的事，而它装的是链接不是内容 —— 内容搬进去，下一次那家自己重写目录就没了。实现上
  不靠前端判断：`store_paths` 把自定义目录放在**最后一档** push，撞上已知目录时保留原来的身份，
  于是 `~/.claude/skills` 照样是 `Own` / `can_be_main = false`（前端据此给出拒绝文案）。
- **撤下来的正是当前主 store 时要松手**（`removeExtraStore` 里 `setMainStore(null)`），
  否则收编会往一个后端已经不扫的目录搬。

自定义行右边多一颗 ✕（只有自定义的有；内置候选是本机客观存在的目录，藏起来只会让人以为它没了），
只从候选列表里去掉，磁盘上那个目录一个字节都不动。

路径显示改用 `elidePath(short(path), 2)` **中间省略**：内置的几个缩完只有两三段，用户加的可能是
`/Volumes/ssd/…/一长串/skills`，原样铺出来会把菜单撑成整屏宽；末尾省略又正好切掉最能分清
哪个是哪个的那两段。菜单 `max-width: 420px`，整行 hover 出完整路径。

---

#### 阶段 4 · 从远端拉来的 skill 的「更新」

**先勘察本机的实际形状**（不是猜的）：

| 目录 | `.git` | origin | 工作区 |
| --- | --- | --- | --- |
| `~/.agents/skills/humanizer` | 目录，顶层就是它自己 | `https://github.com/blader/humanizer.git` | 干净 |
| `~/.cc-switch/skills/humanizer` | 同上，**另一份独立 clone** | 同上 | 干净 |
| `~/.skills-manager/skills/humanizer` | 同上，**第三份** | 同上 | 干净 |
| `~/.skills-manager/skills/dm-watch` | 目录 | **没有 remote** | 3 个文件被改过 |

所以判定拆成两条，缺一不可：

- **顶层就是这个目录**（`<body>/.git` 是个目录）。skill 只是某个大仓库（dotfiles 之类）里的
  一个子目录时，`reset --hard` 冲掉的是**整个仓库**里其它无关的改动。`.git` 是文件
  （submodule / worktree 的 gitdir 指针）也不认，真正的仓库在别处。
- **有 remote**。`dm-watch` 那种本地仓库没地方可拉，给它一个「更新」按钮是在骗人。

判定只读 `.git/config` + `.git/HEAD` 两个文件，**不起子进程** —— 每开一次详情页都要算一次。
`origin_url` 是个手写的 INI 小解析器，认 `[remote "origin"]` 那一节；前面先出现
`[remote "upstream"]` 时不能把它的 url 当成 origin 的（有测试）。游离 HEAD 认不出分支，
也就没有「拉哪一支」可言，同样不认。

**两步走，中间那一步要联网。** 走不了 `propose()` 那条 dry-run 的路 —— 那边的 `WriteReport`
描述的是软链手术，而这里一步都不是：

1. `tools_check_skill_update` → `git fetch` + `rev-list --count` + `status --porcelain`，
   算出「落后几个提交、哪些本地改动会被冲掉」。
2. 二次确认框（`dismissable: false`，点遮罩关掉就等于「已阅」，而这是唯一一次提醒）。
3. `tools_update_skill` → `fetch` + `reset --hard FETCH_HEAD`。

弹框正文按情况逐行拼（`updateMessageParts`，纯逻辑有单测），四种组合各说各的。
**「你改过的 N 个文件会被强制覆盖」这一句只在真的会覆盖时出现** —— 每次都吓一句，
用户下次就不看了，真要覆盖那次也不看。已经是最新、本地也没改过时**根本不弹框**，
只提示一句「已经是最新的」：一个只能点「取消」的框是在浪费一次点击。

**不跑 `git clean`。** 改过的已跟踪文件被覆盖是这次操作说好的代价；用户自己新加的、
git 根本没跟踪的文件（笔记、草稿）不在这个约定里，顺手删掉是越权。弹框里单说一行
「未被 git 跟踪的 N 个文件会原样保留」，否则用户会以为那些也没了。

`fetch` 带 `http.lowSpeedLimit=1000 http.lowSpeedTime=20`：面板的 `busy` 跟着这一步走，
不带阈值的话一次卡死的 fetch 等于整个面板再也点不动，连「取消」都没有。

**live 实测抓到一个 off-by-one。** `git status --porcelain` 是**定宽**格式（前两列状态码、
第三列空格、路径从第 4 字节起），而通用的 `git()` 帮手会 `trim()` stdout —— ` M SKILL.md`
行首那个空格一没，第一行的路径整体左移一位，弹框上写着「你改过的 **KILL.md** 会被覆盖」。
修法是分出 `git_raw()`（一个字节不动）给列对齐的输出用，并把解析抽成纯函数 `parse_status`
锁进测试（`MM` / ` D` / `A ` / `R  a -> b` / `??` 各一条）。

按钮只作用于**主 body**（详情页的文件清单、frontmatter、风险都取自它）。`humanizer` 在本机
有三份独立 clone，弹框里把目标 remote 原样写出来，用户看得见动的是哪一份；三份合一是
「收编」要解决的事，不是这里。

---

#### 阶段 4 · 「来自 github」角标

**角标排在「找不到」后面，但它不是毛病，是来源。** 所以没有并进 `SkillBadge`
（那四个是 `count_badge` 数出来的问题），而是 `SkillEntry.git` + `ScanSummary.from_git`
单独一栏，过滤器也单独一个 `fromGit: boolean` —— 一个 clone 完全可以既干净又是 clone，
混进 badge 那套会让「重复 33」这种数字失去意义。本机实测 `humanizer` 同时带
「重复」和「来自 github」，正是这个分离的理由。

判定复用 `skills_git::detect()`，和详情页「更新」按钮**认的是同一份 body**（主 body）——
列表说「这条来自 github」而详情里没有更新按钮，是最难查的一类 bug。代价是全盘扫描多跑
几十次 `<body>/.git` 的 `is_dir()`，不在就到此为止，可以忽略。

列表行的名字后面也跟一个小 github 图标（hover 出 remote）。位置有个坑：`.skill-row-name`
原本是 `flex: 1` + 省略号，图标直接当兄弟节点会被挤到行尾去（跟名字之间隔着半行空白），
塞进名字里又会跟着一起被裁掉。所以拆成两层 —— 外层 `.skill-row-title` 拿走 `flex: 1`，
省略发生在里层的名字上，图标 `flex-shrink: 0` 紧跟其后。名字压到 70px 时实测：名字出省略号、
图标照样完整在框内。颜色压到 `--text-mute`：它是出身，不是状态，不该和右边那排角标抢注意力。

**一度在健康条上加过一个「超过 20 条才出现」的搜索框，当轮就撤了。** 顶栏中列本来就有一个
（和统计 / 回收站同一个位置，⌘F 聚焦到那儿），用户看漏了才提的需求。同一件事两个入口，
哪怕绑的是同一个 `toolsQuery`，也只是把健康条挤窄 —— 撤得干净：state、模板、样式、
`useDebouncedSearch` 的引用一起删掉，没留任何开关。

（顺带记下一个既有问题：健康条是一行 `nowrap` 的 flex，窗口窄到 900px 左右时右边的主 store
下拉会被顶出屏幕。不是这一轮引入的，留着待办。）

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

#### 做完之后

落地的比表里多两件：`src/codeEditor.ts`（缩进与高亮层 HTML 的纯逻辑）和
`src/skillFrontmatter.ts`（frontmatter 的**写**侧）。两个都是「不碰 DOM 但极容易写错」的东西，
留在 `.vue` 里就只能靠手点验证。`package.json` 零新增依赖，如约。

**两层对齐这件事，最后是量出来的，不是看出来的。** 方法：把 textarea 的字染成实心红、
高亮层全部染成青，然后拿真实的 28KB SKILL.md 加一段中英日韩混排 + 全角标点 + Tab 缩进 +
emoji + 组合重音的样本去逼软换行——**对不齐就会看到青色重影**。结果整屏没有一个青点。
补了一条更硬的：8000 个 `x`（无空格，只能按字符断行）在两层里都断成 **76 个视觉行**。

这条量测顺手回答了一个本来准备去修的问题：textarea 有滚动条时 `clientWidth` 比高亮层
**少 5px**（821 vs 826），按理说换行点会错开。实测没错开，`scrollHeight` 两边都是 10980，
一格不差——WebKit 的 textarea 把滚动条**盖在**文本区上，文本的排版宽度仍然是边框盒减内边距，
和高亮层一致。所以**不要**给高亮层补这 5px 的留白：补了才会真的错开。

**`.skill-head-btn` 这类名字是借不到的。** 第一版 `SkillEditor.vue` 直接用了
`ToolsSkillsPanel.vue` 里的 `.skill-head-btn` / `.tools-placeholder` / `.skill-meta` /
`.skill-fm-key` / `.skill-file-arrow`——那些全在 `<style scoped>` 里，隔着组件一条都不生效。
截图上是一排 `display: block` 的裸按钮，图标顶在文字上面。真正全局的只有 `.chip-spinner`
和 `.modal-close`。改成自己的 `.skill-editor-*` 并在本地写了一份。**类名在这个仓库里不构成复用契约，
`style.css` 里才是。**

**标题里的路径不能用 `direction: rtl` 做中间省略。** 那个会把开头的 `/` 甩到末尾，
`/Users/x/…/humanizer` 显示成 `Users/x/…/humanizer/`，看着像条相对路径。用 `format.ts`
现成的 `elidePath`。

几条值得单独记的判断：

- **写入走 `execCommand('insertText')`，不是赋值 `textarea.value`。** 赋值会把浏览器原生的
  撤销栈整个清空：用户按一次 ⌘Z，文件直接回到打开时的样子。实测 undo / redo 在 Tab 缩进之后
  都还在。要注意的是 WebKit 会把**连续几次 `insertText` 合并成一个撤销单元**（间隔 700ms
  也照合），所以 ⌘Z 是成块回退而不是逐次——这是 WebKit 的行为，不是这边能控制的。
- **先铺纯文本，再上色。** 每次输入同步重画转义过的纯文本层（位置永远是对的），高亮走 120ms
  防抖。shiki 回来时如果文本已经变了就整轮丢弃——旧 token 贴到新文本上是整屏错位，宁可这一轮不上色。
- **高亮层末尾必须多补一个 `\n`。** textarea 给结尾的换行留一行高度，`<pre>` 不留；少了它
  光标走到文末时高亮整体上移一行。`tokensToHtml` 和 `plainToHtml` 补得一模一样，否则上色跑完
  那一刻整层会跳一行。
- **frontmatter 表单只碰单行标量。** 块标量（`|` / `>`）和缩进列表一律置灰，让用户去下面的
  原文里改。按单行改会把后面几行吞掉——这个函数会把用户的 SKILL.md 写回磁盘，读错只是显示不准，
  写错是把人家的文件改坏。本机 `humanizer` 的 `description` 正好是 `|` 块标量，一开就命中。
- **序列化和反序列化必须严格对称。** `setFmValue` 会给 `a: b`、`true`、`1.0` 这类值加引号
  （不加就被 YAML 读成嵌套 map / 布尔 / 数字），而双引号里用反斜杠转义、单引号里把引号写两遍——
  两边的 `unquote` 原来都不还原，`description` 里带个引号，详情页就显示成 `say \"hi\"`。
  前后端各补了一份对称的还原，各带测试。
- **作用域锁死在后端，前端那道只是省一次往返。** `tools/files.rs` 的 `ensure_inside` 会走到
  第一个存在的祖先再 canonicalize——`..` 检查抓不到**指向目录外的软链**，而 skill 目录里到处是软链。
  活体验证过四种越界：`../outside/secret.txt`、`/etc/hosts`、一条指向外面的软链、以及拿一个没有
  `SKILL.md` 的目录当根，全部被拒；软链的目标文件内容没变，软链本身也还是软链。
- **保存带上读的时候那份 `rev`。** 拿过期的 `rev` 再写会被拒（「changed outside the editor」），
  不会把用户在别的编辑器里的改动默默盖掉。活体验证过。

**顺手还掉了阶段 4 欠的那条**：收编冲突框里的 SKILL.md 从「+18 −4」升级成**逐行差异**。
`line_hunks` 用 LCS 回溯出完整编辑脚本（和只求长度的 `lcs_len` 分开写：那个两行滚动 DP 就够，
这个要知道「哪几行」变了，必须留整张表；两边都卡在 800 行内，最坏 800×800 的 `u32` ≈ 2.5MB），
再按 3 行上下文切块、相邻的并起来，400 行封顶并标 `clipped`。渲染直接复用 `DiffBlock.vue`。
行数只回答「改得多不多」，而这个框要用户回答的是「留哪个」——那得看见改的是哪几行。
本机实测：`humanizer` 两份 +231 −325，正好撞上 400 行上限，`clipped` 提示如期出现；
`dm-watch` 的 SKILL.md 两边完全一样（0 hunk），冲突来自别的文件，也如实不画 diff。
全程 `dryRun: true` + 最后点「取消」，三份 body 的文件数和体积一字未动。

**验证过程中的一个坑（复述，不是新发现）**：被遮挡的 WKWebView 里 rAF 是冻住的，
`.app-overlay` 会卡在 `fade-enter-from` 永远出不来，看上去像弹框根本没开。`document.hidden`
是 `true` 就是它。把窗口置前（`osascript … set frontmost`）之后一切正常。阶段 4 记过一次。

**测试**：Rust +8（712 → 720），前端 +109（`codeEditor` 28 + `skillFrontmatter` 32 +
`CodeEditor` 20 + `SkillEditor` 29），1239 → 1348。四道门全绿。

**留给后面的**：「新建 skill」还是没有归属（3.4 的图里有，哪个阶段的文件表里都没有）；
健康条在 ~900px 下溢出（老问题，和这一阶段无关）。

---

### 阶段 6 · MCP 面板

`tools/mcp.rs` + `ToolSurface` 的 `read_mcp` / `write_mcp`。三件事：

1. **auto-discovery**：扫全部 agent 配置，按 `(command, args)` 归一化去重，同名不同定义标冲突。
2. **按 agent 落盘**：JSON 走 `serde_json`，TOML 走阶段 1 提到 `util.rs` 的 `atomic_write_toml`。`~/.claude.json` 是 52KB 且含会话历史，**写前备份、写后校验可解析**。
3. **token 预算**：跑一次 `tools/list` 握手缓存工具数（Pi 的 `mcp-cache.json` 现成可读），估算 token 并画预算条。这是 2.3 那个痛点的正面解法。

**验证**：本机 3 个 user-scope server 能读能改能跨 agent 同步；预算条数字和实际工具数对得上。

---

#### 阶段 6 完成记录（只读那一半）

**格式挂在来源上，不挂在 agent 上。** `McpSource` 多了一个 `format` 字段
（`jsonServers` / `jsonProjectServers` / `tomlServers` / `opencodeJson`），`tools/mcp.rs`
里因此**一行 agent 判断都没有**。理由是反过来必错：`.mcp.json` 这一个文件 claude /
grok / kimi / pi 四家都读，而 codex 和 grok 的 user 级配置又都是 TOML，挂在 agent 上就会
出现「同一个文件被两家用两种解析器读」。

这条不是推演出来的 —— 第一版就踩了：codex 吃的是默认实现（`mcpServers` 那种 JSON），
于是它整个 `[mcp_servers.*]` 被当成坏 JSON，codex 的两个 server 一个都读不到。补了
`mcp_format()` 这个 trait 钩子（只有 opencode 和 codex 需要说一句话），并加了一条按**文件
后缀**整片校验的测试（`every_toml_source_is_parsed_as_toml_and_every_json_source_is_not`），
一家一家去记「谁是 TOML」迟早再漏一次。

**勘察返工：Pi 是有 MCP 配置文件的。** 1.2 那张表里写的「Pi 无原生键，MCP 由 npm 扩展
提供」只对了一半 —— 扩展确实是可选的，但装上之后它的配置落点是固定且可读写的。依据是
本机 `~/.pi/agent/npm/node_modules/pi-mcp-adapter/config.ts:450-530` 的
`getConfigSources()`，六档，顺序即合并顺序（后面的覆盖前面的）：

| 档 | 路径 | 说明 |
| --- | --- | --- |
| `shared-global` | `~/.config/mcp/mcp.json` | import |
| `agents-global` | `~/.agents/mcp.json` | import |
| `agents-nested-global` | `~/.agents/mcp/mcp.json` | import |
| `pi-global` | `$PI_AGENT_DIR/mcp.json` | **唯一的 `writePath`** |
| `shared-project` | `<cwd>/.mcp.json` | 注意是 cwd 本身，不是 git root |
| `pi-project` | `<cwd>/.pi/mcp.json` | |

原来的注释写的是「server 声明不落在一个我们能读写的固定文件里，所以一律不报」，
照它走的话本机 pi 实际加载的 server 会被整片报成「pi 没有 MCP」。已改。

**打开面板不启动任何 server。** 方案 3.3 原话是「跑一次 `tools/list` 握手缓存工具数」，
实现时降级成**只读缓存**：pi 的 `~/.pi/agent/mcp-cache.json`（工具的 name /
description / inputSchema 全在，能算准）和 agy 的
`~/.gemini/antigravity-cli/mcp/<server>/<tool>.json`。理由是握手要拉起用户配的
`npx …` 进程 —— 点一下「工具管理」就在用户机器上起十几个进程，是这个面板最不该有的
副作用。现场探测留给后面做成一个**显式动作**（选中某条 server 之后点「探测」）。

代价是诚实地标出来：量不到的条目显示「工具数未测量」而不是 0（0 会被读成「这个
server 不提供工具」），预算条旁边带一个 `＋` 和「还有 N 个没有缓存，实际只会更高」。

**凭据默认打码。** 本机 `~/.claude.json` 里就躺着一个明文 access token。后端按**键名**
判（`TOKEN` / `SECRET` / `AUTH` / `API_KEY` …，不看值 —— 值的形状没有可靠特征），前端
默认只露头尾各两个字符、中间固定四个点（点数固定，不泄漏长度），点一下才给原文，
重新扫描就复位。

**解析失败不吞。** 一个坏逗号能让一家 agent 的 server 整片消失，而「静悄悄少几行」是
这个面板最坏的失败方式 —— 用户看到的是「我配的 server 没了」。每个来源单独报
`error`，面板底部横着一条说明是哪个文件、什么原因。上面 codex 那个 bug 就是被这条
立刻照出来的。

**scoped 类名不是复用契约（第二次）。** MCP 面板照着 Skills 面板的类名写完，渲染出来
是一堆没有样式的 div —— `.tools-body` / `.tools-list` / `.skill-row` 全在
`ToolsSkillsPanel.vue` 的 `<style scoped>` 里。处理办法分两半：

- **四个面板共用的壳**（`.tools-health` / `.tools-body` / `.tools-list` /
  `.tools-resizer` / `.tools-detail` / `.tools-placeholder`）提到 `style.css`，
  一份定义。`ToolsView.vue` 里那份重复的占位骨架也一并删掉。
- **行、药丸、小标题**这些各面板自己定义（MCP 用 `.mcp-*`），不去借 `.skill-*`。
  等 Hooks / 全局配置也长出同一套行的时候再提取 —— 两份还不够说明问题。

**本机实测结果**（cwd = 某 Flutter 项目）：7 个 server，其中 `chrome-devtools` 是真冲突
（`~/.claude.json` 写的是 `npx chrome-devtools-mcp@latest`，项目 `.mcp.json` 写的是
`npx -y chrome-devtools-mcp@latest --autoConnect`，claude 走项目那份、grok 因为兼容读
`~/.claude.json` 且优先级更高而走 user 那份 —— **同一台机器上两家跑的不是一个东西**）；
`tapd` 只有 env 没有 command，报「配置残缺」；codex 的 `computer-use` 是
`enabled = false`，报「关着」。预算条约 15k / 200k · 7.5%。

**用户纠正：codex 的 MCP 项目级也能配。** 原来只挂了 `$CODEX_HOME/config.toml` 一档。
证据不是听报告就改的 —— 本机 codex 0.154.0 二进制里自带的文档串写着「Project
`.codex/config.toml`: settings for a trusted repository, including sandbox, **MCP**,
hooks, model, and reasoning defaults.」，同一份串里还有 `Overridden by project config:`
和 `git repo root`；磁盘上 10 个仓库有这个文件，9 个写了 `[mcp_servers.*]`，本仓库自己
就是其中一个。

落法：`<git root>/.codex/config.toml`，Project / Own / TomlServers，优先级 200（压过
user 级的 100），**`conditional: true`**。标条件生效是因为 codex 认这份文件要过信任闸，
而信任有两道 —— `[projects."<路径>"] trust_level`（读得到）和 `trusted_hash`（复现不
了）。判定不了就别假装判定得了，跟 grok 那条项目级来源同一个处理。`writable: false`：
可写的那一档全机器只有一个，仍然是 user 级那份（有测试 `at_most_one_writable_source_
per_agent_and_it_is_the_user_level_own_one` 守着）。

改完实测：`chrome-devtools` 的定义从 6 条涨到 7 条，codex 那条来自本仓库的
`.codex/config.toml` 且标着「可能未生效」—— 也就是说本机上这条 server 现在有**三种**
不同的命令行分布在三家 agent 上，冲突判定照样成立。

---

#### 阶段 6 完成记录（写入那一半）

**只换 `mcpServers` 那一段，别的字节一个不动。** `~/.claude.json` 本机 187 KB、5541 行，
其中 MCP 只占 20 行，剩下全是会话历史。整份 `serde_json` 来回会把它全部重排（默认的
`Map` 是 BTreeMap，键会被排序），写坏一次就是把用户的历史一起搭进去。做法是手写一个
JSON 扫描器 `top_level_span()` 定位根对象里某个键的值区间，只替换那一段。

实测证据：加一条再删一条之后，`{k: v for k in before if k != "mcpServers"} ==
{同 after}` 成立，顶层 104 个键连顺序都没变，`mcpServers` 本身也回到原样。中间那次写入
的 diff 只有 42 行，全落在 1194 行之后的那个块里。

扫描器的坑在别的值里：`{"note":"a \" } brace","list":[{"k":"}"},[1,2]]}` 这种字符串里的
花括号、数组里的对象，扫偏一格就是把文件从中间劈开。所以有一条专门的用例守着。
坏在 `mcpServers` **之外**的文件它拦不住（找到键就收工），那是靠写之前把**打算写出去的
整段文本**再解析一遍拦下来的，也有用例。

**TOML 一个多余的键都不敢写。** codex 的 `RawMcpServerConfig` 有 28 个字段，里面
**没有 `type`**，而它的二进制里有 `unknown field \`` 这串报错 —— 塞一个它不认识的键进
`[mcp_servers.*]`，整份 config.toml 就废了。所以 TOML 只写
`command` / `args` / `env` / `cwd` / `enabled` 这几个确认过的，传输方式让它自己按有没有
`command` 推（和我们读的时候同一条规则）。headers 更极端：codex 的键叫 `http_headers`，
grok 的键名没能从它的二进制里确认，而两家共用 `TomlServers` 这一个格式 —— 于是干脆
**拒绝**写，报 `headersUnsupported`，不猜。

**「停用」只对确认认这个开关的格式开放。** codex（`RawMcpServerConfig` 里有 `enabled`，
本机 config.toml 里就躺着一条 `enabled = false`）和 opencode（`@opencode-ai/sdk` 的
`McpLocalConfig` 有）两家。claude / agy / kimi / pi 那几份 JSON 没有可确认的停用开关，
给它们写一个没人读的 `enabled: false`，**面板显示「已停用」而 server 照跑** —— 比不提供
这个功能坏得多。实测点「停用」，codex 那步执行、claude 报 `noEnableSwitch`、grok 报
`notInWritableSource`，一条都没有静默跳过。

**勾选框问的不是「跑不跑」。** 右边那枚药丸说的是这家 agent 实际加载不加载，勾选框说的
是「**我们能写的那个文件**里有没有它」。两者经常不一致：本机 grok 跑着 `chrome-devtools`，
但那份在 `~/.claude.json`（兼容来源），grok 自己的 `~/.grok/config.toml` 里没有。按「跑不
跑」画勾选框的话，用户取消勾选会发现什么都没发生。取消勾选 = 从可写文件里移除，不是
写一个 `enabled: false`（见上一条）。

编辑框里取消掉的那家要真的被移除（`formEdits` 同时排 put 和 drop），只写不删的话
「取消了还在」。反过来，预勾选只勾 `checked` 的那几家 —— 按「沾过它」预勾会把一条来自
兼容来源的定义画成「已在 grok」，保存时就真往 grok 自己的配置里写了一份用户从没要过的。

**远端 server 这一档先不写。** 各家放 URL 的键不一样 —— agy（Gemini CLI）的
streamable-http 用 `httpUrl`，`url` 在它那儿专指 SSE。没逐家实测前写一个半对的键，比
不提供这个入口坏。读的那一侧顺手把 `httpUrl` 认上了（别家没这个键，多认一个不会误伤），
否则 agy 里配的每个 http server 都会被判成「既没 command 也没 url」的残缺条目。

**写入三件套**：写前备份到 `<文件名>.bak`、指纹校验挡外部改动（`atomic_write_backed_up`，
从原来只服务 TOML 的 `atomic_write_toml` 里提出来共用）、写后回读再解析一遍，解不开就
从备份还原。`util::file_revision` 顺带带来一条收益：目标是 symlink 时直接报错，不会把
用户链到 dotfiles 仓库的配置文件替换成普通文件。

**scoped 类名不是复用契约（第三次）。** 「添加 server」弹框里照着面板写的
`.mcp-agent-ic` 一点样式都没有，SVG 没有固有尺寸，撑成了半个弹框那么大。这条规则已经
提到 `style.css`。前两次是 `.tools-*` 骨架和 `.skill-*` 行 —— 同一个坑第三次了，判据很
清楚：**两个组件都要用的规则就不能留在 `<style scoped>` 里**。

**本机实测**（cwd = 某 Flutter 项目）：往 claude / codex / kimicode 三家同时写一条
`viewer-selftest`，三步全成；grok 立刻通过兼容读 `~/.claude.json` 也看到了它（说明覆盖
关系是真的在算，不是抄 agent 列表）；`~/.kimi-code/mcp.json` 从无到有建了出来；codex 那
条停用成功、另两家报出原因；最后三家一起删掉，`~/.claude.json` 回到语义完全一致的状态。

---

### 阶段 7 · Hooks 面板

`tools/hooks.rs` + `read_hooks` / `write_hooks` / `supported_hook_events`。

- 默认只显示"已配置了 hook 的事件"，全量事件列表藏在"添加"里。
- 事件集合**按 agent 取并集并标注支持情况**，不写死一份。
- **turn-signal 标记为受保护**：读 `turn.rs:861` 的 `turn_hook_status()` 判定，UI 上不可删不可改，要删引导去设置页已有的重置入口。
- 干跑：构造假事件喂给 hook 命令，显示 stdout / stderr / exit code。

**验证**：turn-signal 不被误删，设置页的 Hooks 状态卡片和新面板显示一致。

#### 阶段 7 完成记录

**后端**（`tools/hooks.rs` 读 + `tools/hooks_write.rs` 写，共 29 个单测）

`HookFormat` 挂在 `ToolSurface` 上，和 `McpFormat` 同一条规矩：**格式跟着文件走，不跟着
agent 走**。codex 一家就同时有 `hooks.json`（`GroupedJson`）和 `config.toml`
（`TomlGrouped`），所以两个解析器都不认识任何一家 agent 的名字。四种格式：

| 格式 | 形状 | 谁用 |
| --- | --- | --- |
| `GroupedJson` | `hooks: { <Event>: [ { matcher?, hooks: [...] } ] }` | claude `settings.json`、codex `hooks.json` |
| `TomlGrouped` | `[[hooks.<Event>]]` + `hooks = [...]` | grok、codex `config.toml` |
| `TomlList` | `[[hooks]]`，每条自带 `event` | kimi |
| `AgyJson` | `{ <hook 名>: { <Event>: [...] } }` | agy —— 根上是一个个**有名字的** hook |

**`HookSource` 故意没有 `precedence`。** hook 是**叠加**语义不是覆盖语义：user 级配一条、
项目里再配一条，两条都会跑。照搬 MCP 那套优先级会画出一个根本不存在的"被覆盖"关系，
让用户以为删掉高优先级那条、低优先级那条就会"顶上来"。

事件目录逐家实证，不写死一份共用的：claude 14 个、codex 12 个（没有 `Notification`，
没有 `StopFailure`）、grok 5 个、kimi 5 个、agy 5 个。`hook_events()` 返回空数组的两家
（opencode / pi）在 `capabilities()` 里就是 `hooks: false`。

写入沿用 MCP 那一套：写前解析校验 → `.bak` → 原子写 → 写后回读再解析，回读不过就从
备份还原并报错。实测 `~/.claude/settings.json` 和 `~/.codex/hooks.json` 加一条再删掉，
**字节完全一致**，顶层 key 顺序没变，`.bak` 内容等于改动前的原文。

**前端**

- `src/toolsHooks.ts`（纯逻辑，41 个单测）+ `ToolsHooksPanel.vue` + 添加框 + 试跑框。
- `src/shellWords.ts`：命令行分词，MCP 的 `parseCommandLine` 和 Hooks 的 `commandHead`
  共用。两边各抄一份的结果必然是两种拆法。

**列表按命令归并成一行。** 本机的回合信号一条命令挂在 5 家 × 9 个事件上（21 处落点），
摊开就是 21 行；归并后是 1 行。健康条同时报两个数（"13 条 hook / 36 处落点"），差得多是
正常的。

**行标题是从命令里挑出来的，不是编的。** 第一版直接取第一个词，结果列表里并排出现六行
`[` —— 本机有五条 hook 长成 `[ -n "$X" ] && … && cmux hooks codex stop || echo '{}'`。
改成：滤掉 shell 噪声（`[` / `command` / `&&` / 标志 / 重定向）→ 优先取脚本文件名
（`node …/ask-bridge.js` 里有意思的是 ask-bridge.js，不是 node）→ 没有脚本就取程序名
并带上后面的子命令（`cmux hooks codex stop`）。两个实测出来的坑：`command -v X && X …`
会让 X 连着出现两次（要并掉），`$CLAUDE_PROJECT_DIR/scripts/x.sh` 这种以变量开头的**路径**
不能当噪声丢掉（丢了标题只剩一个 `bash`）。

**三道闸，一道都不能少**

1. **回合信号受保护。** 前端不给删除按钮（换成"去设置里重置"，点了直接把设置开到 Hooks
   那一页）；后端 `hooks_write::apply` 的**第一行**就是 `is_turn_hook_command()` 判定，
   任何路径绕不过去。实测绕过 UI 直接调 `tools_apply_hooks` 删它，拿回来的是
   `blocked: [protected]`、`steps: []`。
2. **删不动的提前禁掉。** 只存在于项目配置 / 只读来源里的那条（本机是 sales-app 的
   `check-test-ids-post-write.sh`），`removeEdits` 一条改动都下不出来，而 `propose` 收到
   空数组会直接返回 —— 不提前判就是**彻底的没反应**。现在按钮是禁用的，tooltip 说清它
   待在哪个文件里。
3. **`prompt` / `url` 型的不给试跑。** 那串字是喂给模型 / 拿去发请求的，不是过 shell 的；
   照跑一遍会把里面的 `|` 当成真的管道，还会给出一个假结果。

**其余几条设计**

- 添加框里事件是**多选**，同一条命令挂 Stop + SessionEnd 是常态。勾了某家不认的事件
  当场就说（"Claude Code：TurnStarted"），不等后端报 `unknownEvent` —— 那时候用户已经
  点过确认，计划里躺着一半能做一半不能做。
- 没有"编辑"：hook 的身份就是那条命令，改命令等于换一条。给编辑框会让人以为"改完还是
  同一条"，而落盘其实是删旧写新，这两步在只读来源上的结果完全不同（旧的删不掉、新的
  写进去了，于是两条都在跑）。
- 超时**原样显示不换算**：各家单位不统一（codex 的 `hooks.json` 里躺着 `120000`）。
  但光一个 `10` 读不出是什么，所以带上"超时"两个字。
- agent 过滤器按**落点**筛，不按"落点全在"：后者会让横跨五家的回合信号在只勾一家时
  凭空消失。

**顺手做掉的两件**

- **共用样式提出来了。** MCP 面板当初那行注释写的是"等 Hooks 也长出同一套行的时候再提取"
  —— 现在就是那个时候。行 / 角标 / 小节 / 动作按钮（`.tools-row` / `.tools-chip` /
  `.tools-badge` / `.tools-section` / `.tools-act` …）和写操作表单（`.tools-form-row` /
  `.tools-form-target` …）从两个 `<style scoped>` 搬进 `style.css`；MCP 自己的预算条、
  勾选框、打码值留在原地。确认框也合成了一个 `ToolsPlanModal`，各面板把自己的 report
  翻成 `PlanView`（翻译函数带测试）。scoped 的类名从来不是复用契约 —— 照着另一个 `.vue`
  的类名写出来的是一堆没有样式的 div，而且不报错。
- **确认框被表单盖住的 bug。** 计划弹框和添加表单同为 `z-index: 50`，后挂载的表单把计划
  整个盖住，看上去像"点了保存没反应"。给计划加上仓库里早就有的 `app-overlay-confirm`
  （`z-index: 90`）。**这个 bug 阶段 6 就有**，当时只用 `textContent` 验的，没截图。

**和设置页对不对得上（验收 #10）**

| agent | 设置页回合信号卡片 | Hooks 面板 |
| --- | --- | --- |
| claude | 5/5 | 5 处落点 / 5 个事件 |
| codex | 3/3 | 3 / 3 |
| agy | 2/2 | 2 / 2 |
| grok | 6/6 | 6 处落点 / 5 个事件（`Notification` 配了两个 matcher） |
| kimi | 5/5 | 5 / 5 |
| pi | 4/4 | 不出现 |

Pi 那格**对不上是对的**：它的回合信号是 `~/.pi/agent/extensions/` 下的一个 TypeScript
扩展（`hookType: "extension"`），不是 hook 配置文件里的条目。所以把"这家 agent 没有 hook
这个机制"改成了"没有 hook 配置文件这套机制 —— 它的扩展点是插件 / 扩展，不归这个面板管"：
前一句和设置页里那张写着"Pi 已安装"的卡片摆在一起是自相矛盾的，后一句不是。

**没做的一条**：方案里写的是"读 `turn_hook_status()` 判定受保护"。实际用的是
`turn::is_turn_hook_command()` —— 同一个模块里新加的一个公开函数，判定走的是**安装/卸载
用的同一份路径**（`hook_script_path()` + `legacy_hook_script_path()`）。`turn_hook_status()`
要跑一整轮七家的磁盘扫描才能回答"这条命令是不是我们装的"，而 `apply` 里每条改动都要问
一次这个问题。

**四道闸**：`vue-tsc` 干净、vitest 1444 个全过、clippy `-D warnings` 干净、cargo 790 个全过。

---

### 阶段 8 · 全局配置面板

`tools/memo.rs` + `memo_path()` / `memo_fallback()`。文件都是 Markdown，**一套 `read_memo` / `write_memo` + 一个 `@import` 解析器通吃七家**，trait 上各家只报路径和回退规则。

- `@import` 只展开第一层（防环），相对路径按文件自身目录解析、绝对路径直接用。
- 分叉检测只提示、不自动合并，给显式的"以这份为准同步过去"。
- 生效链路双向标注（编辑 CLAUDE.md 时提示"同时被 opencode / grok 读取"）。
- 外部改动：打开面板和窗口聚焦时比对 mtime，保存时再比一次，不一致拒绝盲写并给 diff。`watch.rs` 是单会话 watcher，套不上，别试。

编辑区直接复用阶段 5 的 `CodeEditor.vue`。

**验证**：两种 `@` 路径风格都能展开可编辑；级联提示正确；两个 `RTK.md` 的分叉被检出并能并排 diff。

#### 阶段 8 完成记录

**后端**（`tools/memo.rs`，27 个单测）

`memo_path()` / `memo_fallback()` 挂在 `ToolSurface` 上，各家只报路径和回退规则；读写和
`@import` 解析一套通吃七家。本机扫出来的形状：

| agent | 约定路径 | 现状 |
| --- | --- | --- |
| claude | `~/.claude/CLAUDE.md` | 在，888 B，一行相对 `@RTK.md` |
| codex | `~/.codex/AGENTS.md` | 在，29 B，一行绝对 `@/Users/…/.codex/RTK.md` |
| grok | `~/.grok/AGENTS.md` | 不在 → 回退读 Claude 那份 |
| opencode | `~/.config/opencode/AGENTS.md` | 不在 → 回退读 Claude 那份 |
| kimi | `~/.kimi-code/AGENTS.md` | 不在，也没有回退 |
| pi | `~/.pi/agent/memory/MEMORY.md` | 在，2705 B |
| agy | —— | 没有 home 级约定，禁用态 |

`@import` **只展开第一层**，第二层只报个数（`nested`）不再往下钻 —— 防环最省事的办法
是根本不进第二层。相对路径按**文件自身目录**解析、绝对路径直接用，整行以 `@` 开头才算，
围栏代码块里的不算（正文里的 `@anthropic-ai/sdk` 都当 import 的话，面板上会凭空多出
一堆红色断链）。

**分叉按内容比，不按大小。** 同名同长度但内容不同是真会发生的（两份手改过的
`AGENTS.md`），只比 `len` 会漏掉。

**`MemoRevision` 只到毫秒。** 纳秒精度在前后端之间来回一趟必然掉精度，反过来让每次
保存都误判成冲突。

**`textdiff.rs` 从 `skills_write.rs` 提出来了。** `line_hunks` / `lcs_len` / `group_hunks`
原先住在 skills 的写入模块里，全局配置要用同一套行差异。抄第二份的结果必然是两种 hunk。

`diff_text(a, b, 左标签, 右标签)` 是唯一的实现，两个命令走它：`tools_diff_memo` 读两个
**路径**（分叉），`tools_diff_memo_text` 收两段**正文**（冲突 —— 一边是打开时读到的、
一边是刚重新读回来的，磁盘上没有第二个路径可传）。

**前端**

- `src/toolsMemo.ts`（纯逻辑，29 个单测）+ `ToolsMemoPanel.vue` + `MemoDiffModal.vue`。
- 右栏不是只读详情，是**编辑器**（复用阶段 5 的 `CodeEditor.vue`，⌘S 保存）：这些文件
  本来就是让人写字的，看完还要跳出去开别的编辑器改，等于没做。

**三件这个面板独有的事**

1. **改一份可能影响好几家。** 编辑 `~/.claude/CLAUDE.md` 时顶上直接列出"Grok Build
   回退到这儿 / opencode 回退到这儿"。只算 `active` 的链路 —— grok 一旦自己建了
   `AGENTS.md`，这条就不生效了，再提示就是误报（实测：接管之后 Claude 那栏的级联提示
   立刻只剩 opencode）。
2. **回退中那一行点开的是实际生效的那份**，不是自己那个还不存在的空位置。grok 明明有
   一整套全局指令在生效，点开却是个空编辑器，那是在说谎。要自己管得按「让它自己管」
   显式接管，而且正文**预填现在生效的内容** —— 从空文件开始存下去，那一刻这家就丢了
   一整套指令。
3. **外部改动。** `watch.rs` 是单会话 watcher，套不上这些散落在各家 home 下的文件，所以
   只在两个时刻比：面板挂载 / 窗口重新聚焦时重扫一遍比指纹，保存时后端再比一次。

**实测下来的几处修正**

- 搜索要同时搜 `own`。回退中那几行显示的是**别人的**路径，只搜 `path` 的话搜 `.grok`
  一无所获。单测先红的。
- 选中按**没过滤**的全量找。搜索把当前这行滤掉了不该把右边清空 —— 那儿可能有还没保存
  的改动；`@` 引用跳过去的片段也未必落在当前搜索结果里。
- 接管存完之后按 `own` 找回自己那一行，不按 `path`：回退中的两家 `path` 是同一个（都
  指着 Claude 那份），拿 `path` 找会把选中跳到 Claude 那一行去。
- 图标要和名字并排。`.tools-row-title` 在 `style.css` 里不是 flex（另外三个面板的图标
  都挂在右边的 `.tools-row-meta` 上），照用的结果是图标自己占一行 —— **第五次**栽在
  "scoped 类名不是复用契约"上。
- 健康条上那三个数**不借 `.tools-chip`**：它们是计数不是筛选器，借过来会长出 hover 和
  点击态，看上去像能点。数字也并进句子（"5 份在磁盘上"），`标签 + <b>数字</b>` 排出来
  是"在 5"。
- diff 的两侧路径是**图例**不是并排两栏。各占一半的话看上去像"左半边是左文件"，而下面
  的 diff 是合并式的、两个文件的行交替排。

**实测（本机，做完全部还原）**

| 做的事 | 结果 |
| --- | --- |
| 两种 `@` 路径风格 | 相对 `@RTK.md` → `~/.claude/RTK.md`，绝对 `@/Users/…/.codex/RTK.md` 都解析到位，两个片段都在列表里可点可编辑 |
| 级联提示 | 编辑 `CLAUDE.md` 时列出 grok + opencode；grok 接管后立刻只剩 opencode |
| 分叉并排 diff | `RTK.md` 两份检出，21 行 add / 18 行 del |
| 新建（kimi） | 保存即建出 36 B，字节完全一致，**没有** `.bak`（本来就没有原文要备份） |
| 外部改动 | 磁盘上改完点重扫 → 横幅"在外面被改过了"；再保存 → 后端拒，磁盘仍是外面那份，没被盖掉 |
| 冲突 diff | "你打开时" vs "磁盘上现在"，只列出外面多加的那一行 |
| 重新加载 | 有未保存改动时先问一句，确认后拿磁盘那份重开，横幅和小圆点一起消失 |
| 分叉同步 | codex → kimi 字节完全一致，被覆盖的那份原样留在 `.bak` 里 |
| 接管（grok） | 存完 grok 不再是回退态，选中跟着挪到它自己那一行 |
| 还原之后 | `~/.claude/CLAUDE.md` / `RTK.md` / `~/.codex/AGENTS.md` / `RTK.md` 四份**逐字节等于**动手前的快照 |

接管那一下顺带暴露了一件本来就该看见的事：预填的正文里那行 `@RTK.md` 是**相对**的，
挪到 `~/.grok/AGENTS.md` 之后解析成 `~/.grok/RTK.md` —— 面板当场把它标成断链，列表里
多出一行红的。**不替用户改写这个路径**：他复制过去的是哪一行就是哪一行，悄悄改掉比
让他看见断链糟得多。断链的引用仍然能点，点开就是那个文件的"保存即新建"，正好是修它
最短的一条路。

**四道闸**：`vue-tsc` 干净、vitest 1473 个全过、clippy `-D warnings` 干净、cargo 815 个全过。

---

---

### 阶段 9 · Windows 实测（发布前置）

把 4.6 那张矩阵逐格跑完：七家 agent × 三级降级（symlink / junction / copy）能否被识别到 skill。**每格要有实测结论，不是推断。**

顺带验 PowerShell 那五条不变量（`where.exe`、`&` 调用运算符、`''` 转义、`powershell_refresh_path()`、`-ExecutionPolicy Bypass`）在新加的命令里没被违反。

#### 阶段 9 完成记录（部分 —— 矩阵那格没做，见末尾）

这一阶段三件事，两件做完了，第三件在这台机器上做不了。

**一、copy 降级的回写同步（4.4 / 验收 #8）—— 做完了**

原先的状态是：`link.rs` 里 `CopyStale` / `CopyEdited` 两个枚举成员、`copy_health()`、
`fingerprint()` 全都写好也测过了，但**一个调用方都没有**。整个文件顶上挂着一条
`#![allow(dead_code)]`，注释写的是「留到阶段 4 再拿掉」，阶段 4 过完没人拿。于是
"副本悄悄过期"这件事在扫描里根本不会被问到 —— 验收 #8 卡在这儿。

先把那条整块放行删掉，让编译器说话。它报了 9 项，分两类：

- **只在 Windows 上有调用方**的（junction 那一路、降级复制本身）。它们刻意编译进所有
  平台，这样 macOS 的单测能覆盖到。这类改挂定点的 `#[cfg_attr(not(windows), allow(dead_code))]`。
- **真的没人用**的：`LinkHealth` / `check_link` / `copy_health` / `is_copy_link`。
  `is_copy_link` 直接删了（`copy_original(p).is_some()` 就是它）；其余三个接上了。

接法是给扫描一个**不需要外部期望值**的入口：

```rust
pub fn copy_state(path) -> Option<CopyState>   // { original, sync }
pub enum CopySync { InSync, SourceChanged, CopyChanged, Diverged, SourceMissing }
```

`check_link` 要调用方先知道"这条链接**应该**指向哪"，而扫描发生在用户指定主 store
**之前**（`RefHealth` 的注释早就写着这件事）—— 但副本不一样：期望值就写在它自己的
marker 里。这就是它接得上而 `check_link` 接不上的原因。

**「两边都变了」必须是独立一态。** 旧的 `copy_health` 是"先判副本、再判源"、返回先命中
的那个，于是两边都改过时报 `CopyEdited`。名字看着没错，但照它去处理（把副本回写到源）
就把源上的改动吃掉了 —— 恰恰是最该拦的一种。改成两边各自比、再组合成四象限。

`RefHealth` 相应地从一个 `ManagedCopy` 拆成五个：一致 / 源变了 / 副本被改了 / 两边都改了
/ 源没了。列表上共用一个 `copyStale` 角标（"这条要不要管"），具体是哪一种在详情里逐条说。

**只有"只有源变了"给一键。** 副本被就地改过（`CopyChanged` / `Diverged`）时重拷会把
用户写的东西抹掉，所以前端不给按钮、后端 `resync_copy()` 再拒一道 —— 同 Hooks 面板那条
规矩：宁可不给按钮，也不给一个会吃掉数据的按钮。真正要"回写到源"的那条路没有做，
因为该留哪边只有用户知道，猜错的代价是丢数据。

换装是"先在旁边建好，再两次 rename"：新副本建在临时目录里 → 旧的挪开 → 新的就位 →
最后删旧的。任何一步失败，那个位置上要么是旧副本要么是新副本，不会是半拉子目录 ——
agent 随时可能正在读它。

**实测（macOS，用一份和 Rust 同算法的指纹脚本造的受管副本）**

| 做的事 | 结果 |
| --- | --- |
| 造一份和源一致的副本 | 扫出来 `managedCopy`，无角标，`copyStale: 0` |
| 改源（改一个文件 + 加一个文件） | `copyStale`，角标出来，健康条 `副本过期 1` |
| 点「立即同步」 | 新增的文件进来了、改动的内容换过去了、没留临时目录、marker 的 `copy_path` 仍指着它自己；角标和健康条一起归零 |
| 在副本上就地改一行再点 | 后端拒："it has local edits that a re-copy would discard"，文件 md5 一个字没变 |
| 两边各改一行 | 报「两边都改了」，**不给**同步按钮，改成一句"两边内容对不上，同步由你决定" |

**二、PowerShell 五条不变量复查 —— 查出一条真的违反了**

新加的四个 tools 模块里只有一处起 PowerShell：`hooks_write.rs` 的试跑
（`shell_command`）。逐条对：

| 不变量 | 结论 |
| --- | --- |
| `where.exe` 而不是裸 `where` | 不涉及，没用到 |
| 引号路径要用 `&` 调用运算符 | 不涉及，我们没有拼引号路径 —— 喂进去的是用户自己写的那条命令 |
| `'` 靠双写转义 | 不涉及，我们不做引用 |
| **每条 CLI 命令前面先 `powershell_refresh_path()`** | **违反了，已修** |
| `-ExecutionPolicy Bypass` | 本来就有 |

漏第 4 条的原因很典型：试跑跑的是**用户写的**命令，看着不像"我们在调 CLI"。但那串命令
十有八九就是在调一个 node CLI（本机那几条 `cmux …` 就是），而 GUI 拉起来的进程继承到的
PATH 可能缺 nvm/npm 那几个目录。后果特别坏 —— 试跑报 command not found，用户会照着去改
一条**本来是好的** hook。修法是把脚本拼装抽成 `windows_hook_script()`（编译进测试，所以
macOS 上就能断言那条前缀真在），单测同时把 `-ExecutionPolicy Bypass` 那几个 flag 钉住。

`link.rs` 的 junction 走的是 `cmd /C mklink /J`，不属于这五条（那是 cmd 不是 PowerShell），
它自己的闸是 `is_cmd_safe()`：检出元字符就放弃 junction、落到复制降级，完全不经过 shell。
`skills_git.rs` 直接 spawn `git`，不过 shell。

**三、4.6 那张验证矩阵 —— 没做，做不了**

这台机器是 macOS（Darwin 25.6.0）。矩阵每一格要的都是"**在 Windows 上**实测"，而方案里
把话说死了：「每格要有实测结论，不是推断」「不能靠推断」。所以这里**不填任何一格**，
连"大概率可以"都不写 —— 一张填了推断的表比空表危险得多，下一个人会拿它当结论。

验收 #3 因此**仍未满足**，发布前必须在一台 Windows 机器上跑完下面这张表：

| 场景 | 要确认的 |
| --- | --- |
| 开发者模式开 / 关 | 各自落到哪一级降级（symlink / junction / copy） |
| 管理员 / 普通账户 | 同上 |
| store 在本地磁盘 / OneDrive / 网络盘 | junction 成不成 |
| 七家 agent × 三种链接类型 | **agent 能不能真的识别到这个 skill** —— 链接建出来不算数 |
| 源目录改动后 | copy 模式能否检出过期并同步（这一条的逻辑已经在 macOS 上测完了，Windows 上要验的是"降级真的会发生") |
| 删除链接 | 源目录内容是否完好 |

外加这一轮新冒出来的一条：**`powershell.exe` 是 Windows PowerShell 5.1，不支持 `&&`。**
试跑走的是 `-Command`，而 hook 命令里带 `&&` 是常态（本机五条里有五条）。5.1 上这会是
一个语法错误，用户看到的是"试跑失败"而 hook 本身没问题。要不要改用 `pwsh.exe`（7.x，
支持 `&&`）取决于**各家 agent 在 Windows 上到底用哪个 shell 跑 hook** —— 这个问题只能在
Windows 上量，所以这里不猜，列进矩阵。

**四道闸**：`vue-tsc` 干净、vitest 1475 个全过、clippy `-D warnings` 干净、cargo 825 个全过。

---

---

### 阶段 10 · 配置集导入导出

把"一套 MCP + skills + hooks + 全局配置"打包成可分享的配置集。放最后，前面九步稳了再说。

#### 阶段 10 完成记录

**值一律不导出。** 这是整个模块存在的前提：配置集是拿去**发给别人**的东西。MCP 的
`env` / `headers` 只导出键名，一个值都不留 —— 不是"只抹看着像凭据的那几个"。
`looks_secret()` 判的是键名，而键名是人随手起的，一个叫 `GH` 的变量装的可能就是 token；
赌错一次的代价是一个凭据进了聊天记录。本机那条 `tapd.env.TAPD_ACCESS_TOKEN`（40 字符）
实测不在包里，序列化结果里连 `"env"` 这个字段名都没有。

`args` 不能整条抹 —— 它同时是这个 server 的身份，抹掉包就没用了。所以只做**按形状**
的遮蔽：`--key=value` 里键名像凭据就换掉值。`--api-key abc` 那种分成两个词的**不碰**：
该抹的是下一个词，而下一个词也可能是下一个 flag，猜错就把包改坏了。这种交给 `redacted`
那张单子提醒人。（`looks_secret` 的针是按环境变量写法排的（`API_KEY`），而 flag 一律
写成 `--api-key` —— 中间要换一刀 `-` → `_`，不换的话一条都抹不掉。单测先红的。）

**导入不走一条新的后端命令。** 包里的条目在 `src/toolsBundle.ts` 里翻成
`McpEdit` / `HookEdit` / 一次 `toolsWriteMemo`，再走那三条已经有校验、dry-run、备份和
回读的路。在后端写一个 `tools_import_bundle` 等于把那几道闸重造一遍，而重造的那份一定
会先漏掉其中一道。后端这一阶段只多了两个命令：`tools_export_bundle`（导出）和
`tools_read_bundle`（把用户挑中的文件读成文本，8 MB 上限，**不解析**）。

**三段并成一次确认。** MCP 的 dry-run、hooks 的 dry-run、全局指令的写入清单合成一个
`ToolsPlanModal`。分三次确认的话，用户在第二次上点取消时前面那一批已经落盘了 ——
他以为自己取消了整件事。

**装给哪几家由用户勾**（方案 2.5）。市面上那批 MCP 管理器几乎都是"改一处无条件同步到
所有客户端"，多 agent 场景下这是错的。默认勾的是"包里提到、本机也认得"的那几家。

**几处刻意的决定**

| 决定 | 理由 |
| --- | --- |
| env 的键名**也不写进配置文件** | 只有键名没有值，写成空串会**盖掉** shell 里继承到的那份 —— server 报的是认证失败，而配置文件上明明有这个键，这种坏法查起来最久。改成在预览里逐条列出来让人补 |
| skill 只记名字和 git 来源 | 从别人的包里一键铺开 skill 文件，等于运行来路不明的代码（同"不做一键安装任意 npm 包"） |
| 全局指令按**角色**落，不按包里的路径 | 导出那台机器的用户名和 home 都不一样。片段（`@` 进来的那些）按导出时记下的 `parent` 落在那家约定文件的**同一个目录**里 —— `@RTK.md` 是相对路径，只有落在旁边才接得上 |
| 绝对路径的 `@import` **不替用户改写** | 和阶段 8 接管那一处同一条规矩：悄悄改掉比让他看见断链糟得多 |
| 覆盖已有的全局指令**默认不勾** | 覆盖一份 `CLAUDE.md` 是把用户攒了很久的个人指令整个换掉。这种事不能靠他注意到某一行没取消勾选 |
| 版本比本机新就**拒绝**，不"尽力而为地读" | 新版本多出来的字段可能恰恰是限制写入范围的那一个，按老规则读等于当它不存在 |
| 回合信号不进包 | 它跟着安装走不跟着配置走。带出去，导入端会在自己那条之外再装一条一模一样的，每个回合触发两次 |
| 只收 **user 级**那一档 | 导入端只往各家的 user 级文件里写。项目级（含 claude 的 local 级 —— 它也存在 `~/.claude.json` 里但只对一个项目生效）跟着**仓库**走，而仓库自己会被 clone 过去；塞进包里就变成了在那台机器上对所有项目生效，而它的命令里往往还写死着导出这台机器上的项目路径 |
| **关掉的**那几条也不收 | `McpServerInput` 和 `HookEdit` 都没有 `enabled` 字段，导入端装出来的一律是开着的。把一条用户明确关掉的东西带过去、再自动打开，比不带过去坏得多 —— 尤其 hook 是会**跑命令**的 |
| 覆盖同名 server 会清掉本机的值，**导入前就说** | `Mutation::Put` 是整条替换不是按键合并：本机那份的 `env` / `headers` 会一起没，连包里没提到的键也一样。而计划框的 `before` / `after` 只画命令行，命令行没变时那一行看着和没动一样 |

**名字会被拼进写入路径。** 包是**别人发来的**，`../../.ssh/authorized_keys` 这种名字
要么是手改坏了要么是故意的 —— `safeName()` 挡在翻译那一层，两种都不往下走。

**实测（本机，做完全部还原）**

| 做的事 | 结果 |
| --- | --- |
| 导出整包（cwd = 一个 Flutter 项目） | MCP 4 / Hooks 13 / 全局指令 5 / Skills 50；`redacted` 列出 16 处。面板上是 MCP 7 —— 少掉的三条：`dart` / `figma-desktop` 在项目的 `.mcp.json` 里，`computer-use` 在 `~/.codex/config.toml` 里但被关着 |
| 搜本机真实 env 值 | 11 个值逐个搜，**`TAPD_ACCESS_TOKEN` 不在包里**；命中的两处是 `node_repl` 自己的 `command`（`…/bin/node_repl` 包含 `…/bin/node`）和 codex 那份 `AGENTS.md` 的正文（里面写着 `@…/.codex/RTK.md`，恰好等于 `CODEX_HOME`）—— 两处都是本来就要导出的字段 |
| 存盘再读回来 | 除 `createdAt` 外逐字段相等 |
| 只勾 Hooks | 其余三类为空，`redacted` 单子跟着清掉（抹掉的值全来自 MCP，留着单子读的人会去找一个不存在的 server） |
| 喂一个非 JSON / 非本 app 的包 / v99 的包 | 三句拒绝各自出来，v99 那句把两边版本号都说了 |
| 喂一个 9 MB 的文件 | 后端在读之前就停："larger than the 8388608 byte limit" |
| 导入预览 | 5 条全局指令分别判成 新建 / 新建（片段 · 跟着 Claude 走）/ 覆盖 / 放不下（agy 没有 home 级约定）/ 放不下（文件名里带路径）；默认只勾上两条新建 |
| 写入 4 处 | `~/.claude.json` 多一条 server（**没有 `env` 键**）、`~/.claude/settings.json` 多一条 Stop hook、两份全局指令字节完全一致 |
| `~/.claude.json` 完整性（验收 #4） | 186 KB / 58 个项目的会话历史：去掉新增那条 server 之后和动手前**逐字段相等**，`.bak` 与动手前快照**逐字节相同** |
| `../../.ssh/evil` | 磁盘上没有这个文件 —— 挡住了 |
| 没勾的那条覆盖 | `~/.claude/CLAUDE.md` 的 md5 一个字没变 |
| 勾上一条覆盖 | 确认按钮变红，脚注换成"现在的内容会从那个位置上消失"，那一行标红并写明"磁盘上那份 16 字节会被整个换掉"；写完新内容就位、旧内容原样留在 `.bak` 里 |
| 还原之后 | `mcpServers` 与动手前快照相等，`settings.json` 相等，两份新建的全局指令删掉，两个 `.bak` 放回动手前的内容 |
| 四个面板回归 | MCP 7 行 / Hooks 13 行 / 全局配置 9 行 / Skills 50 行，健康条照常 |
| 再审之后重跑导入预览 | 陌生角色 `cursor` 在「那台机器上：…」里原样列出（不抛）；它带的片段判成"放不下 · 包里说它跟着 cursor 走，而本机放不下 cursor 的全局指令"；取消勾 Grok Build，`~/.grok/AGENTS.md` 那行当场变灰并取消勾选；七家全取消再点"排一遍"，出来的是"至少勾一家，不然没地方可写。"，计划框不弹 |

**实测下来的两处修正**

- 弹框里的 chip 用的是 `.active`，不是 `.on`。照着自己的习惯写了个 `on`，结果"当前
  是导出还是导入"根本看不出来 —— 又一次栽在"类名不是猜出来的"。
- 路径列**不能用 `direction: rtl` 从左边截**：`~/.grok/AGENTS.md` 会被渲染成
  `grok/AGENTS.md./~`（开头的 `~` 和 `/` 是中性字符，被甩到行尾）。改用 `elidePath`
  中段省略 + 完整路径挂 tooltip。仓库里 `format.ts` 和 `SkillEditor.vue` 早各有一条
  同样的注释 —— 这是第三次。

**再审一遍抓出来的**

前一轮（"包里的每一条 hook 都必须在本机真的这么配着"那条不变量）：

- **按命令归并 vs 按事件配置**那道缝。扫描把同一条命令的所有落点并成一个
  `HookEntry`，而匹配器和超时是**每个事件各自**的。取 `hooks.first()` 的匹配器发给
  全部事件，本机的 `codex-skill-usage.sh` 当场中招 —— `PostToolUse` 上那个
  `^(Bash|mcp__.*)$` 被盖到了 `Stop` 头上，而 `Stop` 根本没有工具名可匹配，那条 hook
  到了导入端永远不会触发。修法是导出时按 `(matcher, timeout)` 再分一次组；本机实测
  12 条变 13 条，两家的 skill-usage 各自裂成两条。
- 写入侧缺了两道对称的闸：删一条本来就不在的、加一条已经在的，都直接放行。补
  `presence_block()` 和 `HookBlockReason::AlreadyThere`。
- 「装给哪几家」那一排原来**不管全局指令** —— 只勾 Kimi Code，计划框里照样冒出
  "覆盖 `~/.claude/CLAUDE.md`"。那一排的字面意思就是全局的（方案 2.5：同步必须按
  agent 勾，不能是广播）。补 `memoPickable()`，取消勾一家就把跟着它走的那几份一起
  松开。
- 一家都没勾时，`bundleMcpEdits()` 恒为空，于是提示走到了"一步都排不出来"——
  那句话没告诉用户该做什么。补 `needsAgents()`，改说"至少勾一家，不然没地方可写"。
- `isAgent()` 一度当成"只有测试在用"删掉了。它有一处真实用途：包里的角色字符串是
  **别人机器上**写的，`AGENTS_META[role]` 是个普通对象字面量，`agentLabel('cursor')`
  会直接抛。恢复。

这一轮：

- **作用域**。导出跟着面板走，面板是带 cwd 扫的，于是项目级的配置也进了包；而导入端
  只往 user 级写 —— 一条只在一个项目里跑的 hook，到了那台机器上对所有项目跑，命令里
  还写着导出这台机器上的项目路径（本机 `sales-app` 那条
  `check-test-ids-post-write.sh` 就是）。改成只收 user 级，单测钉的是"给不给 cwd，
  导出的 MCP 和 hooks 逐字段相等"—— 这条不挑机器。
- **关掉的那份**。顺着上一条查出来的：`computer-use` 在 `~/.codex/config.toml` 里
  `enabled = false`，而 `McpServerInput` / `HookEdit` 都没有 `enabled` 字段 ——
  带过去等于替用户把它打开。
- **覆盖会清掉本机的值**。`Mutation::Put` 是整条替换，同名 server 一旦被覆盖，它原有的
  `env` / `headers` 整片消失，连包里没提到的键也一起没。而计划框的 `before` / `after`
  只画命令行（`mcp_write.rs::command_line`），命令行没变时那一行看上去和没动一样，
  用户是在 server 起不来之后才发现 token 没了。补 `clearedValues()`，在按"排一遍"
  **之前**就把要没的键逐条列出来。
- 片段落不了地的提示语把两种坏法并成了一句。本机实测那个包里的 `FROM-CURSOR.md`
  明明写着 `parent: cursor`，提示却说"包里没记它是被谁引用进来的"。拆成 `noParent`
  （真没记）和 `noHome`（记了，本机放不下那一家）。
- 三份文案里混进了 `**原文照搬**` 这样的 markdown 星号，而这些串是 `{{ t(...) }}`
  直接插值的，界面上显示的就是两个星号。整个 locales 目录里只有这三处有 `**`。

**四道闸**：`vue-tsc` 干净、vitest 1533 个全过、clippy `-D warnings` 干净、cargo 840 个全过。

---

#### 阶段 10 之后 · 一轮界面反馈

都是用户在真机上用出来的，按提出的顺序记：

- **开面板得先点一下才算真打开。** 四个面板都是主从两栏、扫描是异步的，挂上之后左边
  一列东西、右边一句"没选中任何东西"。抽 `selectFirstRow()` 到 `toolsPanel.ts`：等列表
  第一次有东西时选第一条，**只做一次** —— MCP / Hooks 的 `select()` 是开关，每次"没选中"
  都补一条的话用户就永远取消不掉了。全局指令那个面板多带一个 `pickable`：它的行点开
  会读文件，而"自己那份还不存在"的行点开是预填模板，等于开面板就凭空造一个未保存的改动。
- **文件树的图标小得没有存在感。** `▸` 那个字形换成 `IconChevronRight`，文件名前面按
  后缀给 `fileIconFor()`，箭头 14px / 图标 15px。skill 编辑器和 skill 详情页两处都改。
- **健康条上「36 处落点」折了行。** `.hook-defs` 补 `flex-shrink: 0` + `white-space: nowrap`。
- **「图标丢了？」不是代码问题。** 上一次低内存杀进程留了个僵尸占着 1420，新起的 Vite
  因为 `strictPort` 直接死掉（"Port 1420 is already in use"），webview 跑在缓存页上、
  几个 agent 的 PNG 全是 502。杀掉占位的重启就好了 —— 记在这儿是因为它看上去**非常像**
  一个图标资源的 bug，下次别再去翻 `icons.ts`。
- **点「来自 github」列表变空白。** 不是过滤器的问题：`.list-spotlight` 是
  `position: absolute` + 真实高度，会算进滚动容器的 `scrollHeight`；筛完之后内容只剩
  两行，而 `--spot-y` 还停在上一次 hover 的 736px，`scrollTop` 就被顶在了新内容下面。
  抽 `listScroll.ts` 的 `resetSpotlight()`，列表一变就把浮块归零。会话列表 / 回收站 /
  导出历史三处也是同一个浮块，一并接上。
- **取消筛选之后选中的那行要自己回到视野里。** 同一个文件里的 `revealSelected()`：
  只在选中行确实在可视区外时才 `scrollIntoView({ block: 'center' })`。
- **「搬进主 store」对着一条软链没反应。** `adopt()` 原来只有一条 `same_path(body, dest)`
  的跳过分支，而 `link::same_path` 两边都会 canonicalize —— 主目录里那条指回
  `.skills-manager` 的软链，比出来"已经在家了"。补一条**在跳过之前**的分支：拆链、把
  body 搬进来、再在原地回种一条链。不能走 `compare()`，那条路 `identical` 为真会
  `link_over()` 把 body 删掉，留下两条断链。
- **纯软链、没人引用的行缺删除。** 新增 `tools_unlink_ref`：只拆链、不碰它指向的东西，
  非链接的真内容一律拒绝。前端 `removableLink()` 判定"状态是 linked 且 `agents` 为空"，
  和已有的 `deletableBody()` 互斥。
- **健康条上的总数和列表对不上。** 用户数出 50 个 skill 的标题下只有十几行。当场那一次
  是自测脚本在搜索框里留了个词（`toolsQuery` 只在内存里，关面板就清），但根子在四个面板
  一律印**扫描到的总数**：搜索词、状态角标、agent 勾选任何一个生效，数字都和眼睛看到的
  对不上。补 `shownOfTotal(shown, total)` —— 筛过写成 `13/50`，没筛还是 `50`，另配一条
  只在筛过时出现的 tooltip。全局配置那个面板要特殊对待：它的行是**位置**不是文件
  （七家里有四家自己那份根本不存在，扫描一份都数不到），拿后端 `summary.files` 当分母
  会得出"9 行 / 8 个文件"，所以从 `memoRows` 里劈出一个 `allMemoRows()` 当分母。

- **「搜索后，列表没有被过滤」。** 列表其实过滤了（49 → 12），但两件事让它看上去没有：
  - 搜 `hyperframes`，**同名那一条排在最底下**。默认排序是「坏得最厉害的排最前」，
    而那条自己一个角标都没有，被 11 条「重复」压到了第一屏外面。默认那套是给**浏览**
    用的，搜索是**找一个具体的东西** —— 补 `queryRank()`：名字相等 > 名字前缀 >
    名字包含 > 描述 > 路径，插在置顶和角标之间。没搜索时它恒为 0，排序逐字不变。
  - 12 条里有 6 条名字里根本没这个词（`lottie` / `tailwind` / `three` / `waapi` …），
    它们命中在**描述**里，而描述那一行是省略号截断的 —— 屏幕上没有任何东西解释
    这些行为什么在结果里。接上会话列表早就在用的 `highlightSegments()` + `.kw-hit`，
    四个面板一起：skills 的名字和描述、MCP 的名字、hooks 的命令头和事件药丸、
    全局指令的名字和路径。
  - 还有一类命中连描述都不在：只落在**路径**上（搜 `cc-switch`）。补 `queryPath()`
    （skills）和 `queryCommand()`（MCP），这种行的第二行改成命中的那条路径 / 命令行。
- 顺带记一条：`.hook-tag.kw-hit` 必须在组件里再写一遍背景色。scoped 选择器多带一个
  属性，特异性比全局的 `.kw-hit` 高一档，不写的话药丸根本不变色。

- **详情的下半截要等两秒，那两秒里什么都没有。** 详情不是扫描给的，是按名字单独读
  一次：走一遍 skill 目录，每个文件读进来过一遍风险规则。本机实测 `hyperframes`
  47 个文件 **2.1 秒**（小的 13ms / 160ms）—— 而左边点一下是瞬时的，右半边的
  风险点 / Frontmatter / 文件三节就那么空着，看上去像点了没反应。补 `detailLoading`
  和一段骨架，照着那三节真实行的**分段**摆（风险行是「等级药丸 + 规则名 + 出处」，
  文件行是「图标 + 文件名 + 大小」）—— 一行画成一根通长的灰条不行，详情栏六百多像素宽，
  那看上去是几段正文。骨架只在**还没有内容**时出现：刷新时旧内容留在原地换掉。
- 顺手补了一个真的会中招的竞态：慢的那条回来得晚，2 秒的窗口足够用户点好几下，
  `loadDetail` 回来时不再无条件写 `detail`，先对一遍 `selectedName`。实测点
  `hyperframes` 后 200ms 改点 `git-push`，3.5 秒后右边仍然是 git-push 的 2 个文件。
- 骨架的灰条原来用 `--surface-hover`。那个 token 是给「悬停时比底色亮一点」用的，
  和底色差不到一档，摊在详情栏那片空白上几乎看不见。改成
  `color-mix(in srgb, var(--text-mute) 26%, transparent)`，深浅两套主题都分得开。

- **build 时 lightningcss 报 `'deep' is not recognized as a valid pseudo-class`。** 四条
  报出来的规则都在 `src/style.css` 里 —— 那是 `main.ts` 直接 import 的**全局**样式表，
  不是 SFC 的 `<style scoped>`。`:deep()` 是 Vue 编译期才认的写法，走不到全局 CSS 这儿；
  浏览器把它当未知伪类，**整条规则作废**。也就是说这四条从来没生效过：

  | 规则 | 想要的 | 实际（`1em` 跟着继承的字号走） |
  | --- | --- | --- |
  | `.tools-icon-btn svg` | 13×13 | 14×14 |
  | `.tools-reveal svg` | 13×13 | 继承字号 |
  | `.tools-act svg` | 12×12 | 11.5×11.5 |
  | `.tools-form-box svg` | 11×11 + `stroke-width: 3.2` | 约 12.5×12.5 塞在 14px 的框里 |

  去掉 `:deep(` 之后四条全部生效，实测 13/13、13/13、12/12、11/11（`stroke-width: 3.2px`）。
  勾选框那条最明显：勾在 14px 的框里终于是居中的，笔画也够粗了。
  全局样式表里没有别的 Vue 专用写法（`::v-deep` / `:slotted` / `:global` 一个都没有），
  SFC 里那 45 处 `:deep()` 全都在 `<style scoped>` 里，是对的。

- **「没有 agent 读它」把一份两条链都落在上面的内容标成了孤儿 —— 还配了个删除按钮。**
  `SkillRef.agents` 回答的一直是「**这条引用所在的那个目录**被哪几家扫」。对链接来说那
  就是答案；对实体目录不是：本机 `~/.skills-manager/skills/css-animations` 没有任何一家
  直接扫 `.skills-manager`，`agents` 是空的，可 `~/.claude/skills/css-animations` 和
  `~/.agents/skills/css-animations` 两条链最后都落在它身上。删除按钮点下去，后端会先把
  那两条链解掉再删内容 —— 两家 agent 的 skill 静悄悄没了。

  新增 `SkillRef::reached_by`：谁**真的够得到**这份内容。两种间接 ——「终点」（别人的
  `resolved` 落在我身上）和「中途」（别人的链路从我身上经过）。前端的
  `deletableBody()` / `removableLink()` 和那枚角标全部改看它。

  两个必须钉住的细节：
  - **坏掉的链一个 agent 都递不出去。** 拿死链当「有人在读」的证据，会把删除按钮永远
    锁死在一份其实没人用的内容上。
  - **身份不能用 `canonicalize` 直接算。** 链上每一环自己就是符号链接，`canonicalize`
    会一路跟到终点 —— 整条链算出来是同一个身份，彼此横向互递，`~/.claude/skills/X`
    会冒出 codex / kimicode / pi 这几家根本不读它的。改用 `self_identity()`：只规范化
    父目录，名字原样接上。这一条是写完之后在真机上看数才发现的，补了
    `an_entry_point_does_not_inherit_the_other_agents_that_share_its_body` 钉住。

  真机复核：115 条引用里 **28 份实体内容**原来会长出一个会断链的删除按钮，**12 条中间
  节点**同理；改完之后这 40 条全部收回，剩下 36 份「确实没人指着」的照旧可删。
  `~/.cc-switch/skills/css-animations` —— 用户在图里标出来「这个才是没被引用的」那条 ——
  仍然带着删除按钮。

- **全局配置里的同名同内容文件，要能像 skill 一样合并掉。** 用户指着 `RTK.md` 在
  `.claude` / `.codex` / `.grok` 下各一份 964 B 说「把它们都搬进主 store，然后链接过去」。
  本机实测这三份 md5 完全一样，另外 `.codex/AGENTS.md` 和 `.grok/AGENTS.md` 也是同一份
  889 B —— 两组。

  **做成了什么：** `detect_forks` 改成 `detect_groups`，一趟分出两堆 —— 内容分了家的还是
  分叉（只提示），内容一模一样的是新的 `MemoDup`（能动手）。合并 = 搬一份进主 store
  （默认 `~/.agents/memo`，和 `~/.agents/skills` 同一个父目录，**不塞进某一家的 home**，
  否则卸载那家会把另外六家的指令一起带走），原位全换成链接，走和 MCP / Hooks 同一个
  `ToolsPlanModal`（dry-run → 计划 → 确认 → 应用）。

  **三条差点埋进去的坑：**

  1. **`util::file_revision` 明确拒绝符号链接。** 那条拒绝本身是对的 —— 它挡的是
     「写入走 tmp + rename，会把用户链到 dotfiles 仓库的 `settings.json` 换成普通文件」。
     但合并之后每个位置都是链接，直接用它的话：列表里三行全变成红色错误，而且一保存就
     把刚建好的链接**换回普通文件**，合并当场作废且毫无提示。修法是 `memo::body_of()`
     —— 读、比指纹、写，一律先解析到链接背后那份，`.bak` 也跟着落在真身旁边。
     两条测试钉住：`saving_through_a_merged_link_writes_the_real_file_and_keeps_the_link`、
     `a_dangling_link_is_an_error_not_a_missing_file`。
  2. **「已经合并过」不能再报成重复。** 三条链接指向同一个物理文件时内容当然还是全相同，
     但那正是终态。判定改成「canonicalize 去重后还剩几份」，而不是「有几个位置」。
  3. **文件链接没有「实体复制」这一级降级。** 目录那边退到复制还能靠标记文件辨认，单个
     md 文件没地方放标记，复制出来的两份和合并前一模一样 —— 那是假装做成了。
     `link::link_file` 到硬链接为止（同样是「一份物理内容」），再不行就报错进 blocked。

  **真机跑通一遍再还原：** 两组五个位置全部变成指向 `~/.agents/memo` 的链接、md5 不变、
  通过链接保存一次链接仍在、`.bak` 落在 store 里、重扫后 `dups` 归零；随后按备份原样
  还原（拆链接、放回五个原件、删掉测试造出来的 `~/.agents/memo`），**机器留在合并前的
  状态，那一下由用户自己点**。

  **一条紧跟着的反馈：** 「合并全部重复」一次出十三步，平铺开来看不出哪几步是在处理
  同一个文件。`PlanRow` 加一个可空的 `group`，`ToolsPlanModal` 在组与组之间画一条带
  文件名的分割线 —— **只有一组时不画**（给一串本来就连贯的步骤加标题只是多占一行），
  所以 MCP / Hooks 那两个不给 `group` 的面板行为一个字都没变。建主 store 目录那一步
  是多组共用的，归到第一个用到它的那组，免得冒出一个不属于任何文件的孤儿行。

**四道闸**：`vue-tsc` 干净、vitest 1623 个全过、clippy `-D warnings` 干净、cargo 867 个全过。

---

#### 阶段 10 之后 · MCP 写入路径的一次对抗式复核

一轮针对 MCP 面板的对抗式评审，报了五条。核实下来**两条不成立、三条成立**，另外在核实
过程中撞见一条评审本身没报、但比它报的几条更该修的。按「先说不成立的」记，因为那两条
说明了这套设计里两个容易被误读的决定。

**不成立的两条：**

- **「条件来源被无条件当成生效」。** 报告说 `conditional` 的来源（codex 项目配置、grok 的
  `.mcp.json`）参与了覆盖链却没有任何不确定性提示。实际上 `ToolsMcpPanel.vue` 的来源行上
  就画着 `tools.conditional`（「可能未生效」），和 Hooks 面板同一个位置同一个串。至于
  「要不要把它从 `effective` 里排除」——那正是 3.3 里定过的取舍：**那个 import marker
  落在哪儿没有公开说明，判定不了就别假装判定得了**。把它排除出覆盖链同样是在假装知道，
  只是换了个方向假装。维持原样。
- **「编辑一条已停用的 server 会把它悄悄打开」。** 编辑框预勾的是 `syncState === 'checked'`
  的那几家，而一条 `enabled = false` 的定义算 `unchecked` —— 它压根不会被预勾，保存时
  既不 put 也不 drop。这条路径复现不出来。（勾选框那条路径上 `put` 清掉 `enabled` 是
  **有意的**，见 `syncEdit` 的注释：不清的话面板显示勾上了而 server 还是不跑。）

**成立并已修的三条：**

1. **`put` 会把用户自己写的键一起删掉。** `Mutation::Put` 原来是整条 `insert`，
   而 `toml_entry` / `json_entry` 只写我们认得的那几个键。结果是：编辑一条 codex server
   的参数，它的 `startup_timeout_sec` / `tool_timeout_sec` 跟着没 —— 而计划框上只写了
   一句「更新」。改成**只动归写入方管的键**（`TOML_OWNED_KEYS` / `JSON_OWNED_KEYS`），
   其余原样留着。`enabled` / `disabled` 仍归我们管（那是勾选框那条路径要的），
   `url` / `httpUrl` 也在列 —— 「本来是远端、这次改成本地命令」时旧地址必须跟着走。
   模块开头第 3 条原则（「只写确认过的键」）说的是**别凭空造键**，保留用户文件里已经
   有的键不违反它：那些键 codex 自己本来就收得下。
2. **用户批准的计划和真正执行的计划可能不是同一份。** dry-run 排完计划、确认框开着的
   时候，别的进程（编辑器、另一个 agent）改了同一个文件，`apply` 会**重新排一遍**再写：
   `write_file` 里那道指纹只覆盖「本次 apply 开始读取之后」，看不见看计划到点确认之间
   那段。修法照抄全局指令那边已经在用的规矩：dry-run 把每个目标文件的指纹
   (`McpFileStamp`) 一并交出去，点确认时原样回传，**所有文件先对一遍再动第一个字节**，
   任何一个对不上就整批拒绝（`McpFailKind::Stale`）。计划里没有的目标文件同样拒绝 ——
   重排之后多出一个目标，说明局面已经和用户看到的那份不一样了。
3. **多文件写到一半失败，用户不知道已经改了几家。** 七家 agent 的配置在七个文件里，
   没有哪个机制能把它们一起提交；原来是 `write_file(file)?` 一路抛出去，前面已经落盘的
   那几个文件连同整份报告一起丢掉，前端只拿到一个字符串。做不到原子就**如实交代做到
   哪儿了**：停在第一个失败上，已落盘的步骤 `done` 为真，失败原因进 `McpWriteReport.failed`，
   前端 `mcpApplyOutcome` 把三种结局分开说（全做完 / 一个字没写 / 做了一半停在哪儿）。

   > `tools_apply_hooks` 是同一段代码形状，同样会半途而废。这次只动了 MCP，那边留着。

4. **凭据识别漏了带分隔符的键名，URL 一栏压根没打码。** `looks_secret` 是照字面找子串的，
   `X-API-Key`（Anthropic 自家 header 的写法）既不含 `APIKEY` 也不含 `API_KEY` —— 一条
   明文密钥直接摆在详情页上。改成比对前把键名里的非字母数字抹掉。URL 那一栏更直接：
   原来是 `{{ def.url }}` 原样渲染，而托管 MCP 的接入地址常常自带密钥。加 `mask_url`，
   **只动能确认是凭据的两处**——`user:pass@` 那段 userinfo、名字像凭据的 query 参数；
   主机和路径留着（否则详情页上认不出这是哪个 server），路径里那一段**不猜**（判断
   一段路径是不是密钥只能看形状，和「只认键名」是同一条规矩）。列表行上没有「点一下
   看原文」这个动作，所以那儿永远只用打过码的那份。

**评审没报、但更该修的一条：** `ToolsMcpPanel.vue` 里有两个**字面 NUL 字节** —— 复合
map key 的分隔符被直接写成了原字符（`` `${server}\0${v.key}` ``）。后果是 `grep` / `rg` /
`file` 全把这个 795 行的 Vue 文件当二进制，`git diff` 报 `Binary files differ`：**这个
文件既搜不到也 diff 不了，更没法合并**。评审工具漏判第一条（「面板没画 conditional」）
正是因为它 grep 这个文件返回了空。换成 `\u0000` 转义，运行时一个字节都没变。

**真机跑通一遍再还原：** 用一条 `sv-selftest` 在 `~/.claude.json` + `~/.codex/config.toml`
上跑完整条链 —— 篡改指纹→拒绝且两文件 md5 不变、不给指纹→同样拒绝、给对指纹→两家都落盘、
手工塞进 `startup_timeout_sec` / `tool_timeout_sec` / `timeout` / `trustLevel` 再改一次参数→
**四个键全留着**而 args / env 照改、最后 drop 掉；随后按备份还原，两份配置 md5 与测试前
逐字节一致，app 自己写的 `.bak` 里的测试痕迹也一并清掉。URL 打码单独用一条临时的
`sv-urlcheck` 在界面上走了一遍：列表行不露地址，详情页默认
`https://bob:••••@mcp.example.com/sse?api_key=••••&mode=fast`，点一下给原文，再点收回。

**四道闸**：`vue-tsc` 干净、vitest 1630 个全过、clippy `-D warnings` 干净、cargo 883 个全过。

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
