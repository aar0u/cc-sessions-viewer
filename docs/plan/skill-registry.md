# 发现 skills（skills.sh）

工具管理的第五个面板 —— 在 skills.sh 上搜、看清楚、再装。
本机已装的那一半在 [tool-management.md](./tool-management.md) 的 Skills 面板。

---

## 一、搜索

- [x] 顶栏搜索框接 `tools_registry_search`，**网络请求不是本地过滤**：500 ms 防抖，Enter 立即发
- [x] 少于 2 个字符不发请求（按 char 数算，两个汉字是两个字符）
- [x] 一次取 100 条 —— 接口上限 200 且没有 offset / cursor，翻不了页，这不是「第一页」是「全部」
- [x] 过期响应丢弃：比对接口回显的 `query`，打 `react` 途中 `re` 那一轮晚回来不会顶掉结果
- [x] 五种列表状态有固定优先级（`discoverState()`）：空 / 太短 / 加载 / 失败 / 没结果 / 有结果
- [x] 加载态是骨架（`ToolsListSkeleton.vue`），和右边详情区的 `SkillDetailSkeleton` 同一种语言
- [x] 本地重排：相关度（接口原序）/ 安装量
- [x] 行 key 是 `source/skillId` —— 一页里 `code-review` 出现过 8 次，拿名字当 key 会点错行
- [x] 断网 / HTTP 错 / 坏 JSON 都翻成中文一句话，不是 `[object Object]` 也不是 ureq 的英文
- [x] 已装的条目打「已安装」角标（和本地 Skills 扫描结果对得上）
- [x] 域名源（非 GitHub）列得出来、标「不可安装」、给「在浏览器打开」的出路

## 二、详情预览

- [x] 走 git 不走 GitHub API：浅克隆 → `ls-tree` 定位 SKILL.md → sparse-checkout 只检出那一个目录
- [x] 仓库缓存复用：同仓库第二个 skill 从秒级降到百毫秒级
- [x] 三节：frontmatter 描述 / 文件清单 / 风险点（复用本地 Skills 的风险引擎）
- [x] **接口不给描述**，所以这里是全应用唯一能拿到描述的地方；看过一次之后列表行第二行从 source 换成描述
- [x] 同名目录在仓库里出现多处时提示「还有 N 处」
- [x] 预览身份校验（`isPreviewFor`）：点第二条时第一条还没回来，不比对就会显示**另一个 skill 的文件清单**
- [x] 详情区状态同样抽成 `previewState()`，正在取时不挂着上一条的内容
- [x] 五种取不到的原因各有中文提示，且磁盘状态不变

## 三、复制并安装

方案原稿写死了「不执行 `npx skills add`」，按用户要求改掉了。保留的是那条理由里真正要紧的一半 —— **不背着用户跑**。

- [x] 点按钮在详情区拉起一个看得见的终端（`InstallTerminal.vue`），命令原样打进去回车，每一行输出都在眼前，Ctrl-C 随时停
- [x] 一次性终端：关掉连 PTY 一起收掉。和 `terminals.ts` 那套会话级 TUI tabs 是两回事，只共用调色板和 base64 编解码
- [x] 先挂 `pty://data` 监听再 spawn —— 那是个广播事件，反过来会丢开头几行
- [x] **CLI 停在 agent 多选那一步等用户勾**，我们不替他决定（见下）
- [x] 命令发出 3.5 秒后弹一个「我知道了」的确认框，关掉焦点立刻还给终端
- [x] `installCommand()` 只对 GitHub 源生成，域名源返回 null

### 事故：内嵌终端把安装确认吃掉了

点一次「复制并安装」，`npx skills add` **一句不问就装完**，往 7 个 agent 目录建了链接。

原因不是文案，是环境变量泄漏：`skills` CLI 用 `@vercel/detect-agent`，它读 18 个环境变量，任一个有值就打印
`Agent detected — installing non-interactively` 并**整段跳过多选**。链路是「用户在 Claude Code 里开的 shell（`CLAUDECODE=1`）→ `npm run tauri dev` → app 进程 → `pty_spawn_shell` → 每一个 PTY」。

- [x] `pty.rs` 加 `AGENT_ENV_MARKERS`（18 个）+ `strip_agent_markers()`，在 `build_interactive_shell` 里逐个 `env_remove`
- [x] **只剥人类终端这一条路**。`build_shell_command`（agent CLI 那条）一个字没动 —— 那边这些变量是身份，剥了会改 agent 行为
- [x] 3 条单测钉住：标记表覆盖 detect-agent 读的全集、交互 shell 不继承调用方身份、真有值时确实被删掉
- [x] 真机验收：`printf` 探针打出 `PROBE[][][]`，终端随后停在 `Which agents do you want to install to?`

## 四、缓存与存储

- [x] 仓库缓存删掉之后，所有已装的 skill 照常工作 —— 缓存只服务预览
- [x] 装出来的东西落在 `~/.agents/skills/<id>`，由本地 Skills 面板接管（链接、健康、删除、更新都在那边）

---

## 还在约束后续改动的事实

- **只有搜索接口是公开可用的。** 文档里的 `/api/v1` 要 Vercel OIDC token；详情页是 RSC 页面不是接口，build hash 一变就废
- **来源有两种形态**：GitHub 源可克隆可安装；域名源没有任何公开的取内容路径，只能跳浏览器
- **仓库内布局千奇百怪**，必须 `ls-tree` 找，猜路径的命中率实测 3/7
- **零新增 npm / cargo 依赖**

## 不做

- **不接 `/api/v1`**（要 token）、**不解析 RSC payload**（build hash 一变就废）
- **不做域名源的安装**（没有公开取内容路径）
- **不做榜单 / 分类 / 收藏 / 趋势**（公开接口只有搜索，其余全得爬页面）
- **不做「一键把搜到的都装上」** —— 每一个都是别人写的、会被 agent 当指令读的文本
- **不在本地 Skills 面板里混网络结果** —— 那些计数回答的是「我这台机器上有什么」
