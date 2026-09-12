# 从 skills.sh 搜索并安装 Skill（发现面板）调研与方案

## 目标、结论与边界

给「工具管理」加**第五个面板：发现**——在 skills.sh 上搜别人写好的 skill，看清楚它是什么、
里面有什么文件、有没有危险指令，然后装进本机主 store，再按现有的「启用于」开关分给各家 agent。

本文档只出方案，不含实现。下面所有数字都是 **2026-09-11 在本机实测**的，不是推测。

**已确认的结论：**

- **skills.sh 只有一个公开接口能用**：`GET /api/search?q=&limit=`。无鉴权、无特殊 header、
  `ureq` 直连返回 200（实测，见 1.1）。文档里那套 `/api/v1/*` 要 Vercel OIDC token，
  `/api/skill/...` 返回 401，详情页是 Next.js RSC——三条路全都不能用（1.2）。
- **搜索结果只有五个字段，没有描述**：`id / skillId / name / installs / source`。试过
  `version` / `searchVersion` / `includeDescription` / `full` 等参数，返回形状一个字段都不变（1.1）。
  所以**列表第二行放 source 而不是描述**，这是数据决定的，不是设计偏好。
- **详情和安装都只能走 git**，而且是同一条路：`clone --filter=blob:none --no-checkout`
  → `ls-tree` 定位 `SKILL.md` → `sparse-checkout` 检出那一个目录。全程 ~3.3 秒，
  落盘 196 KB（2.3 的实测表）。**不能靠猜路径**：七个仓库里只有 3 个用 `skills/<slug>/`（2.4）。
- **检出下来的目录就是一个普通的 skill body**，所以详情页的「文件清单 / frontmatter / 风险」
  三节可以**整块复用** `files::list` + `risk::scan_file` + `parse_frontmatter`——
  装之前先扫一遍风险，这是本方案相对 `npx skills add` 最大的价值（3.3、5.1）。
- **安装 = 把检出的目录搬进主 store**，然后走现有的 `toggle` 给各家 agent 建链。整条写入管线
  （dry-run → 计划 → 确认 → 应用 → `WriteReport`）和同名冲突三选一 `skills_write.rs` 已经有了，
  这里只新增「从缓存目录搬进来」这一种源（4.2）。
- **零新增依赖**：`ureq`（`json` feature）已在 `Cargo.toml:50`；`git` 已经由
  `skills_git.rs` 以子进程方式在用。

**边界：**

- **只支持 `owner/repo` 形态的来源。** skills.sh 的 `source` 有第二种形态是**域名**
  （`code.deepline.com`、`smithery.ai`、`open.feishu.cn` …，八次查询里占 0%–8%）。
  这些是厂商自托管的 skill，没有公开的取内容路径（`/skills.json`、`/.well-known/skills.json`、
  `/<slug>/SKILL.md` 全 404，详情页也没有 SSR 内容和安装命令）。**列出来，标成不可安装，
  只给「在浏览器打开」**——不假装能装（1.4）。
- **不执行 `npx skills add`。** 理由见第七章：要求本机有 node、会下载并执行任意 npm 包、
  没法 dry-run、也没法在安装前把风险摊给用户看。
- **只装进主 store**，不直接写 `~/.claude/skills` 这类 agent 目录。那儿该放的是链接，
  这是现有 Skills 面板已经确立的模型，发现面板不能另起一套。
- **不做榜单 / 分类 / 收藏 / 我的安装历史。** 搜索接口只给搜索，剩下的都得靠爬页面。

---

## 一、skills.sh 的接口实测

### 1.1 搜索接口（唯一可用的公开接口）

```
GET https://www.skills.sh/api/search?q=<关键词>&limit=<1..200>
```

无鉴权、无 Referer / UA 要求。**`ureq` 2.x 直连实测返回 200**（专门写了个最小 crate 验的，
因为本仓库有过前科：`~/.claude` 的 usage 接口对 rustls 指纹返回 403，`usage_api.rs` 才退回
系统 `curl`。skills.sh 没有这个问题，**不需要 curl 兜底**）。

响应：

```json
{
  "query": "type",
  "searchType": "fuzzy",
  "searchVersion": "legacy",
  "skills": [
    {
      "id": "emilkowalski/skills/prototype",
      "skillId": "prototype",
      "name": "prototype",
      "installs": 82653,
      "source": "emilkowalski/skills"
    }
  ],
  "count": 3,
  "duration_ms": 557
}
```

实测到的约束（`q=code&limit=200`，200 条样本）：

| 事实 | 数据 | 对方案的影响 |
| --- | --- | --- |
| 字段只有五个 | `id / skillId / name / installs / source` | 列表只能显示这些；**描述要等选中后现取** |
| `id === source + 「/」 + skillId` | 200/200 命中 | `id` 是冗余的，内部只存 `source` + `skillId` |
| `name` 可能 ≠ `skillId` | 5/200（`agent development` vs `agent-development`） | **目录名、检出路径一律用 `skillId`**；`name` 只用来显示 |
| `limit` 上限 200 | 传 201 / 500 都返回 200 条 | 面板固定请求 `limit=100`，不做翻页（接口没有 offset / cursor） |
| `q` 至少 2 字符 | `{「error」:「Query must be at least 2 characters」}` | **0–1 字符不发请求**，列表区显示提示，不能让用户看到一句英文报错 |
| 多词会切到语义搜索 | `q=react native` → `「searchType」:「semantic」` | 只影响排序，不影响字段；健康条可以把 `searchType` 显示出来 |
| `installs` 跨度极大 | 122 ~ 914678 | 数字要千分位格式化 |
| 同名结果常见 | `code-review` 在一页里出现 8 次（不同 source） | **列表的唯一键必须是 `source/skillId`，不能是 name** |

试过的无效参数（返回形状完全不变）：`version=v2`、`searchVersion=v2|next`、
`type=semantic`、`searchType=semantic`、`includeDescription=1`、`full=1`。
**没有隐藏的富返回。**

### 1.2 文档里的 `/api/v1` 用不了

`/api/v1/skills`、`/api/v1/skills/search`、`/api/v1/skills/curated`、
`/api/v1/skills/{source}/{skill}`（这个会返回 `files[]` 带全文，正是我们想要的）、
`/api/v1/skills/audit/...` ——全部要求 `Authorization: Bearer $VERCEL_OIDC_TOKEN`，
而这个 token 只能从 Vercel 项目里铸出来，限流 600 req/min per (team, project)。
**桌面应用拿不到，也不该拿。**

`/api/skill/[owner]/[repo]/[skillName]` 这条路由确实存在（`x-matched-path` 头能证实），
但返回 `401 {「error」:「Unauthorized」}`。

### 1.3 详情页是页面，不是接口

详情页地址是 `https://www.skills.sh/{source}/{skillId}`，两种 source 形态都 200。

- GitHub 源的 HTML 里**有** `<meta name=「description」>`，内容就是 SKILL.md frontmatter 的
  description（`prototype` 那条完全对得上），`<title>` 是 `prototype — emilkowalski/skills`。
- 页面主体是 Next.js RSC（`?_rsc=<buildhash>` 或 `RSC: 1` 头），payload 里能挖到描述和
  仓库内路径——但那是内部协议，**build hash 一变就废**，不能当契约。

所以：**描述不从页面拿，从 SKILL.md 的 frontmatter 拿**——反正为了文件清单和风险扫描，
那个目录本来就要检出（第二章）。skills.sh 的页面只用来做「在浏览器打开」那个按钮。

### 1.4 来源有两种形态

```
owner/repo   184/200   github.com/<owner>/<repo>        → 可安装
域名           16/200   code.deepline.com 之类           → 不可安装
```

八次不同查询（code / review / test / python / web / agent / design / data）里域名源占
**0%–8%**，出现过的有 `code.deepline.com`、`evlog.dev`、`apifox.com`、`smithery.ai`、
`developer.paddle.com`、`skills.volces.com`、`agent.qq.com`、`open.feishu.cn`。

对这些源做过的探测，**全部无路可走**：

```
https://code.deepline.com/skills.json                404
https://code.deepline.com/.well-known/skills.json    404
https://code.deepline.com/build-tam/SKILL.md         404
https://code.deepline.com/skills/build-tam/SKILL.md  404
skills.sh 的详情页                                    200 但无 SSR 内容、无安装命令、无仓库链接
```

**结论：列表照常显示（它们是真实存在的 skill，而且安装量不低——deepline 那批 8k~26k），
但详情只显示「这个来源不是 git 仓库」+ 一个「在浏览器打开」。** 不隐藏，也不给假的安装按钮。

判定规则：`source.split('/').length == 2` 且两段都匹配 `^[A-Za-z0-9._-]+$` → 可安装；
否则不可安装。

---

## 二、详情与安装只能走 git

### 2.1 为什么不用 GitHub API

`GET /repos/{o}/{r}/git/trees/HEAD?recursive=1` 一次请求就能拿到全部路径，看起来比 clone 香。
但**未鉴权限流是 60 次/小时/IP**——用户在发现面板里点十几行就见底了，而且失败的形态是
「突然什么都点不开」。git 协议没有这个问题。

`raw.githubusercontent.com` 同理：必须先知道路径，而路径只能从 tree 拿。

### 2.2 三步

```bash
# ① 拉一个无 blob、无检出的浅克隆（只有目录树）
git clone --depth 1 --filter=blob:none --no-checkout https://github.com/<source>.git <cache>/<owner>__<repo>

# ② 在目录树里找这个 skill 的 SKILL.md
git -C <cache> ls-tree -r --name-only HEAD | grep -i '/SKILL\.md$'   # 再按 /<skillId>/ 过滤

# ③ 只检出它所在的那一个目录
git -C <cache> sparse-checkout set --cone <dir>
git -C <cache> checkout
```

第 ③ 步之后 `<cache>/<dir>` 就是一个**普通的 skill 目录**，后面的一切（frontmatter、
文件清单、风险扫描、复制进 store）都按本地 body 处理，没有任何「网络版」的分支。

想在检出前先看一眼正文（比如列表上做预览）也可以——部分克隆支持惰性取 blob：
`git cat-file -p HEAD:<path>` 会只把那一个 blob 拉下来（实测 1196 ms）。**但既然详情页
本来就要文件清单和风险扫描，直接走 ③ 更划算**，`cat-file` 这条留给「只要描述」的场景。

### 2.3 实测耗时（emilkowalski/skills，2026-09-11）

| 步骤 | 耗时 | 落盘 |
| --- | --- | --- |
| `clone --depth 1 --filter=blob:none --no-checkout` | **1900 ms** | 136 KB |
| `ls-tree -r --name-only HEAD` | **38 ms** | — |
| `cat-file -p HEAD:skills/prototype/SKILL.md`（惰性取 blob） | 1196 ms | 7.5 KB |
| `sparse-checkout set --cone` + `checkout` | **1355 ms** | 累计 196 KB |

别的仓库的克隆一步（更早一轮测的）：

```
emilkowalski/skills        1410 ms   124 KB     ls-tree  19 ms（12 个 SKILL.md）
github/awesome-copilot     2098 ms   288 KB     ls-tree  29 ms（435 个）
pytorch/pytorch            3252 ms   692 KB     ls-tree  —（大仓库也扛得住）
```

**首次打开详情 ≈ 3.3 秒，装 ≈ 再 1.4 秒。** 这个量级必须有 loading——好在 Skills 详情的
骨架屏（`DETAIL_SKEL` / `.skill-skel-bar`）刚做完，直接复用。**同一个仓库的第二个 skill
是 40 ms 级**（缓存命中，只多一次 sparse-checkout），而 `mattpocock/skills`、
`anthropics/claude-code` 这种一个仓库出好几条结果的情况很常见，缓存的收益是实打实的。

### 2.4 仓库内布局千奇百怪，必须 `ls-tree`

早一轮对七个仓库做过路径探测（直接猜 `skills/<slug>`、`<slug>`、`.claude/skills/<slug>`、
`.github/skills/<slug>`、`agents/<slug>`），**只命中 3/7**。实际见到的布局：

```
skills/<slug>/                 .claude/skills/<slug>/      .agents/skills/<slug>/
.agent/skills/<slug>/          plugins/<x>/skills/<slug>/  config/skills/<slug>/
```

`ls-tree -r` 只要 19–38 ms，**没有任何理由去猜**。

定位规则：在所有 `*/SKILL.md` 里挑**父目录名 == `skillId`** 的；

- 一条都没有 → 报「这个仓库里找不到 `<skillId>`」，附上仓库链接；
- 多条命中（同名目录出现在多处）→ 取路径最短的那条，并在详情里把完整路径显示出来，
  让用户自己看清楚装的是哪一个。

---

## 三、界面

用户给的参考图就是**现在的 Skills 面板**，箭头指着顶栏搜索框写「搜索网络 skills」，
外面框住整个工具管理区——即**沿用同一套壳**：顶栏搜索 + 健康条 + 左列表 + 右详情。

### 3.1 入口：导航第五项

`TOOL_TABS` 从 `['mcp', 'skills', 'hooks', 'memo']` 变成
`['mcp', 'skills', 'discover', 'hooks', 'memo']`（放在 Skills 后面，两者相邻）。

这样一来白拿三件事：

- `ToolsTopbar.vue` 的 placeholder 是 `t('tools.search.' + toolsTab)`，加一条 i18n 就有了；
- ⌘F 聚焦搜索框、切 tab 清搜索词（`switchToolsTab`）也都现成；
- **网络结果不会污染本地 Skills 面板的计数、角标、过滤器**——`49 个 skill / 重复 30 / 绕远路 12`
  这些数字问的是「我这台机器上有什么」，混进搜索结果就全废了。

`TAB_CAPABILITY` 里 discover 没有对应的 `ToolCapabilities` 字段（它不属于任何一家 agent）——
需要把那张表的取值放宽成 `keyof ToolCapabilities | null`，`null` 表示「跟 agent 无关，永远可用」。
`test/toolsPanel.test.ts:98` 现在断言「每个 tab 都有 truthy 的能力位」，要一并改成「要么是
`ToolCapabilities` 的合法键，要么显式为 `null`」——否则这条断言会挡住新 tab，而它挡的是个假问题。
左侧 agent 过滤器在这个面板里**整排压暗不可点**：搜的是 skills.sh，跟勾了哪几家没关系。

### 3.2 搜索框：网络请求不是本地过滤

这是和另外四个面板**唯一的行为差异**，必须说清楚：

| | 本地四个面板 | 发现面板 |
| --- | --- | --- |
| 输入后发生什么 | 过滤内存里的数组 | 发一次 HTTP |
| 防抖 | 200 ms（`useDebouncedSearch`） | 200 ms **之上再叠 500 ms**，或按 Enter 立即发 |
| 空查询 | 显示全部 | 显示引导页（不发请求） |
| 1 个字符 | 正常过滤 | 不发请求，提示「至少 2 个字」 |
| 过期响应 | 不存在 | **必须丢弃**（同 `loadDetail` 的 stale guard） |

**不改 `ToolsTopbar.vue`**（除了多一条 placeholder）：发现面板自己 `watch(toolsQuery)`，
在面板内部做第二层防抖和过期丢弃。顶栏不该知道某个面板要发网络请求。

### 3.3 三块区域

**健康条**（`.list-head tools-health`，和 Skills 同一个类）：

```
找到 128 个 · 模糊匹配     [按相关度] [按安装量]     已装 3      主 store ~/.agents/skills ▾    ⟳
```

- `找到 N 个`：`count`。搜索接口没有总数，所以**不用 `shownOfTotal`**——它的前提是
  「分子分母同源」，这里没有分母。
- `模糊匹配 / 语义匹配`：直接把 `searchType` 显示出来，解释为什么多词搜的结果看着「不像」。
- 排序两选一：默认按接口给的相关度；「按安装量」是纯本地重排。
- `已装 M`：结果里在本机主 store 已经存在同名目录的条数（可点，只看这些）。
- 主 store 下拉：**和 Skills 面板共用 `toolsSkillsActions.ts` 里那个 ref**，装到哪儿要一致。

**列表行**（复用 `.tools-list` 的行样式）：

```
prototype                                    emilkowalski/skills        82,653  ↓
Build multiple genuinely different …         ← 选中过一次之后才有（描述来自详情）
```

- 第一行：名称（命中词用 `highlightSegments` + `.kw-hit` 高亮，和另外四个面板一致）
  + 右侧安装量（千分位，带下载图标）。
- 第二行：**source**。本地 Skills 面板那儿是描述，这里只能是 source——接口不给描述（1.1）。
  已经预览过的条目把描述缓存下来，第二行换成描述、source 挪到名称右边，这样翻回来还看得见。
- 角标：`已装`（本机同名）、`不可安装`（域名源）。
- 唯一键 `source/skillId`（同名结果一页能出现 8 次）。

**详情**（和本地 Skills 详情同构，所以三节完全复用）：

```
prototype                    [安装]  [在 GitHub 打开]  [在 skills.sh 打开]  [复制安装命令]
Build multiple genuinely different versions of a UI piece you describe, …

来源
  emilkowalski/skills  ·  skills/prototype  ·  HEAD @ 3f2a1bc

风险点 (0)                     ← risk.rs 原样跑，装之前就能看见
Frontmatter                    ← parse_frontmatter
  name         prototype
  description  Build multiple …
文件 (2)                       ← files::list
  SKILL.md    7.5 KB
  PICKER.md   3.1 KB
```

- 「安装」在已装时变成「已安装」（禁用）或「更新」（来源标记里的 tree sha 和远端不一样时）。
- 「复制安装命令」抄 skills.sh 页面上的那条：`npx skills add https://github.com/<source> --skill <skillId>`，
  给想自己在终端装的人。**我们自己不执行它**。
- 域名源：只剩标题 + 「在 skills.sh 打开」，正文位置写清楚为什么装不了。

### 3.4 安装的交互

点「安装」→ 走现有的 dry-run → 计划弹框 → 确认 → 应用（和收编 / 删除同一套 `WriteReport` 渲染）。
计划里会出现的步骤：

```
EnsureDir  ~/.agents/skills
Move       <cache>/emilkowalski__skills/skills/prototype  →  ~/.agents/skills/prototype
Link       ~/.claude/skills/prototype  →  ../../.agents/skills/prototype     （勾了的 agent 各一条）
```

**同名冲突**复用 `AdoptConflict` 的三选一（`skills_write.rs:116`）：`KeepMain` 保留本机已有那份、
`UseExternal` 用刚下载的覆盖（本机那份先备份）、`KeepBoth(新名字)` 两份都留。内容一致时按现有规则不追问。
语义要对齐：这里的「外部」是缓存里刚检出的目录，「主」是主 store 里已有的同名 skill。

计划弹框底部加一行「装完启用于：[七个 agent 图标]」，默认勾上本机已装且已有 skills 目录的那几家；
不勾也行，装完在 Skills 面板照样能开。

---

## 四、安装管线

### 4.1 装到哪

主 store（默认 `~/.agents/skills`，用户可在健康条里改）。**实体目录进主 store，
agent 目录里只放链接**——这是 Skills 面板已经确立的模型（方案文档 3.4），发现面板必须遵守，
否则刚装的 skill 在本地面板里会立刻显示成「绕远路」或「重复」。

### 4.2 复用现有写入管线

`skills_write.rs` 已经有：`Op` 枚举（`EnsureDir` / `MoveDir` / `Link` / `Unlink` / `DeleteDir`，`skills_write.rs:816`）、
`WriteReport { dry_run, steps, conflicts }`、`AdoptConflict` 三选一、备份与回滚。

安装需要的只是**一种新的源**：从缓存里的检出目录搬进主 store。用 `MoveDir` 而不是拷贝——
缓存目录反正要清理，搬走还省一次 I/O；同一个仓库的别的 skill 不受影响（sparse-checkout 是按目录的）。
搬走之后把该仓库的 sparse 集合复位即可。

### 4.3 拿到手的东西不可信

从陌生仓库检出的目录要当**不可信输入**处理：

- **来源字符串先过白名单**：`^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$`，两段都不能是 `.` / `..`、
  不能以 `-` 开头（挡 `git` 把它当参数）。URL 由我们拼，不接受任何用户给的完整 URL，
  从根上挡掉 `ext::` / `file://` 这类 git 传输协议注入。
- **clone 的环境**：`GIT_TERMINAL_PROMPT=0`（不弹密码框卡住子进程）、
  `-c credential.helper=`（不动用户的凭据）、`-c protocol.ext.allow=never`。
- **搬进 store 前逐项检查**：只接受普通文件和目录，**拒绝符号链接 / 硬链接 / FIFO 等特殊文件**
  （一条指向 `~/.ssh/id_rsa` 的链接混进 skill 目录，agent 读 skill 时就读到了它）；
  拒绝任何解析后逃出 skill 目录的路径。
- **体积与数量上限**：沿用 `files::list` 的 `truncated` 语义，超限就截断显示并拒绝安装，
  在详情里说清楚「这个目录有 900 个文件，不像是个 skill」。
- **风险扫描是安装前的必经一屏**：`risk.rs` 已有的规则（危险命令、外发地址等）原样跑，
  `high` 级别的发现要求二次确认。**这是本方案相对 `npx skills add` 最大的价值**——
  那条命令是先装再说。

### 4.4 来源标记与更新

装完在 skill 目录里写一个标记文件（形制抄 `link.rs` 的 `.session-viewer-link.json`）：

```jsonc
// ~/.agents/skills/prototype/.session-viewer-skill.json
{
  "kind": "cc-sessions-viewer/skill-install",   // 固定串，挡"碰巧同名"
  "registry": "skills.sh",
  "source": "emilkowalski/skills",
  "skillId": "prototype",
  "repoPath": "skills/prototype",               // 仓库内路径，更新时再定位一次
  "commit": "3f2a1bc…",                          // 安装时的 HEAD
  "treeSha": "8c4d…",                            // 该子目录的 tree sha —— 比较它才知道要不要更新
  "installedAt": "2026-09-11T19:40:00Z"
}
```

**为什么不能直接用 `skills_git.rs`：** 它的 `detect()` 要求 body 自己就是 git 仓库顶层且有 remote
（注释里写得很清楚，防的是「reset --hard 把 dotfiles 仓库里别的改动一起冲掉」）。
我们装的是仓库里的**一个子目录**，`.git` 不在里面，`detect()` 判 false——这是对的，不该改它。

更新检查 = 按标记里的 `source` 重新（或从缓存）拿到 HEAD，`ls-tree HEAD -- <repoPath>` 取 tree sha，
和 `treeSha` 比。不一样就提示更新；更新动作就是**重跑一次安装**（同一个计划弹框，
`Replace` 分支），用户本地改过的文件会在计划里以备份步骤出现。

本地 Skills 面板的「来自 github」角标（`summary.fromGit`）现在只认 `skills_git::detect`；
这一阶段要把标记文件也算进去，否则刚装的 skill 在本地面板里看不出来源。

### 4.5 缓存

克隆缓存放 `app_storage::data_dir(app)/skill-registry/<owner>__<repo>/`
（和 `image_cache` 同一个根，`init()` 的写法照抄）。

- 命中即复用：同仓库的第二个 skill 从 3.3 秒降到 ~1.4 秒（只多一次 sparse-checkout），
  再点回已经检出过的那个是 40 ms 级。
- **`storage_gc::prune` 用不了**：它 `remove_file`，只删文件，删不掉整个 clone 目录。
  需要一个小的目录级清理（按 mtime 删整个 `<owner>__<repo>`，上限按个数 + 总字节，
  默认 30 个 / 200 MB），挂在现有的启动 gc 旁边。
- 缓存丢了不影响任何已装的 skill——它只是加速，不是数据。

---

## 五、后端结构

### 5.1 三个新模块

```
src-tauri/src/tools/
├── registry.rs          // 搜索：ureq 请求 + 结果类型 + source 形态判定
├── registry_git.rs      // 缓存 clone / ls-tree 定位 / sparse-checkout / 缓存清理
└── registry_install.rs  // 校验 + 搬进主 store + 写标记 + WriteReport（复用 skills_write 的 Op）
```

一个**必要的小重构**：`skills.rs::build_detail` 里那段
「`list_files` + 逐文件 `risk::scan_file` + `read_frontmatter` + `truncated`」抽成
`describe_body(dir) -> (Vec<SkillFile>, Vec<RiskFinding>, Option<SkillFrontmatter>, bool)`，
本地详情和网络预览共用。**不是复制一份**——两边对「一个 skill 目录长什么样」的定义必须是同一份代码。

### 5.2 命令

```rust
#[tauri::command(async)] tools_registry_search(query: String, limit: u32) -> RegistrySearch
#[tauri::command(async)] tools_registry_preview(source: String, skill_id: String) -> RegistryPreview
#[tauri::command(async)] tools_registry_install(
    source: String, skill_id: String, main_store: String,
    resolution: Option<Resolution>, enable_for: Vec<String>, dry_run: bool,
) -> WriteReport
#[tauri::command(async)] tools_registry_update_check(body: String) -> Option<RegistryUpdate>
```

四个都要进 `lib.rs` 的 `generate_handler!`，并在 `api.ts` 配上同名包装 + `types.ts` 里的类型。
`enable_for` 传 agent 名，装完在同一个计划里出 `Link` 步骤——不另起一轮确认。

### 5.3 类型（`types.rs` + `types.ts`）

```ts
interface RegistryHit {
  source: string        // "emilkowalski/skills" 或 "code.deepline.com"
  skillId: string       // 目录名、安装名，永远用它
  name: string          // 显示名，可能带空格
  installs: number
  installable: boolean  // source 是不是 owner/repo（1.4）
}
interface RegistrySearch {
  query: string
  searchType: string    // "fuzzy" | "semantic"，原样显示
  hits: RegistryHit[]
}
interface RegistryPreview {
  hit: RegistryHit
  repoPath: string      // 仓库内路径
  commit: string
  frontmatter: SkillFrontmatter | null   // 复用本地类型
  files: SkillFile[]                      // 复用
  findings: RiskFinding[]                 // 复用
  risk: RiskLevel                         // 复用
  truncated: boolean
  installedAt: string | null              // 本机已装的那份的路径，没装就是 null
}
```

`SkillFrontmatter` / `SkillFile` / `RiskFinding` / `RiskLevel` 全是现有类型，
所以详情三节的 Vue 片段能原样搬。

### 5.4 前端

```
src/toolsRegistry.ts             // 纯逻辑：结果排序、已装匹配、source 形态判定、安装量格式化
src/views/ToolsDiscoverPanel.vue // 面板壳（在 views/，不计覆盖率 —— 所以逻辑都放上面那个文件）
test/toolsRegistry.test.ts       // 上面那个文件的单测
```

`src/toolsPanel.ts` 改两处：`TOOL_TABS` 加一项、`TAB_CAPABILITY` 的值放宽成可空。
`ToolsNav.vue` 给 discover 配个图标（`icons.ts` 里加一个下载 / 罗盘型的 inline SVG，不用 emoji）。
四个 locale 各加一组 `tools.discover.*` 和 `tools.search.discover`。

---

## 六、分阶段

每一阶段自己能跑、能验、能停。

**阶段 A · 只读搜索**
导航第五项 + 顶栏搜索接进来 + `tools_registry_search` + 列表 + 空态 / 短查询提示 / 失败态。
详情区显示「选中后加载」。不碰磁盘，不装任何东西。
_验收：_ 搜 `type` 出结果；1 个字不发请求；断网时显示可读的失败提示而不是一串 Rust 错误；
切到别的 tab 再回来不残留上一次的结果。

**阶段 B · 详情预览**
`registry_git.rs` 全套（clone 缓存 / ls-tree 定位 / sparse-checkout）+ `describe_body` 重构
+ 详情三节 + 骨架屏 + 过期响应丢弃 + 缓存清理。
_验收：_ `emilkowalski/skills / prototype` 三节都出得来且和 GitHub 上一致；
同仓库第二个 skill 明显更快；找不到 SKILL.md 的情况有明确报错；域名源显示「不可安装」。

**阶段 C · 安装**
`registry_install.rs` + 校验（符号链接 / 越界路径 / 体积）+ dry-run 计划弹框 + 冲突三选一（KeepMain / UseExternal / KeepBoth）
+ 「装完启用于」+ 装完刷新本地 Skills 面板。
_验收：_ 装一个新 skill，主 store 里出现实体目录、勾中的 agent 目录里出现链接，
本地 Skills 面板能看到它且「启用于」状态正确；同名冲突三个分支都走通；
dry-run 的步骤和实际应用后的磁盘状态一致；风险 `high` 的包会要求二次确认。

**阶段 D · 来源与更新**
标记文件 + `tools_registry_update_check` + 本地 Skills 面板的「来自 github」角标认标记文件
+ 详情里的「更新」按钮。
_验收：_ 装完标记文件内容正确；把 `treeSha` 改坏后显示「有更新」；更新走的是同一个计划弹框；
手工改过的文件在计划里出现为备份步骤。

---

## 七、不做什么，以及为什么

- **不执行 `npx skills add`。** 要求本机有 node；会下载并执行任意 npm 包；没法 dry-run；
  装完才知道装了什么。本方案的全部价值就在「装之前先看清楚」，套一层它等于把这个价值扔掉。
  但**「复制安装命令」按钮要给**——想自己在终端装的人不该被挡住。
- **不接 `/api/v1`。** 要 Vercel OIDC token（1.2）。
- **不解析 RSC payload。** build hash 一变就废（1.3）。
- **不做域名源的安装。** 没有公开的取内容路径（1.4）。真要做，得一家一家适配厂商的私有协议。
- **不做榜单 / 分类 / 收藏 / 趋势。** 公开接口只有搜索，其余全得爬页面。
- **不做「一键把搜到的都装上」。** 每一个都是别人写的、会被 agent 当指令读的文本，
  批量安装和批量执行陌生脚本是一回事。
- **不在本地 Skills 面板里混网络结果。** 那些计数回答的是「我这台机器上有什么」。

---

## 八、总验收

1. 搜索 → 选中 → 看清楚（描述 / 文件 / 风险）→ 安装 → 在本地 Skills 面板里出现且能启用给各家 agent，
   全程不出终端。
2. 全程零新增 npm / cargo 依赖。
3. 域名源在列表里可见、不可安装、有出路（浏览器）。
4. 断网 / 仓库不存在 / 找不到 SKILL.md / 同名冲突 / 目录里有符号链接——五种异常都有明确的中文提示，
   且**磁盘状态不变**。
5. 缓存删掉之后，所有已装的 skill 照常工作。
6. `npx vue-tsc --noEmit`、`npm run test:run`、`cargo clippy -- -D warnings`、`cargo test` 四关全绿。
