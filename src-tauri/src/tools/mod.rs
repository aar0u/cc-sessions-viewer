//! 工具管理（MCP / Skills / Hooks / 全局配置）的 agent 抽象。
//!
//! 和 `agents::SessionSource` 是同一个套路，但管的是另一半事情：`SessionSource`
//! 负责「这个 agent 的会话记录长什么样」，`ToolSurface` 负责「这个 agent 的工具和
//! 配置放在哪、什么格式」。两者故意分开——一个 agent 可以有会话历史却没有 skills
//! （agy），也可以有 MCP 却没有全局指令约定。
//!
//! 接入新 agent 的步骤和 `agents/mod.rs` 一样：新建 `tools/<name>.rs` 实现
//! `ToolSurface`，在 [`surface`] 里加一个 match 分支。**不要把 per-agent 的路径或
//! 格式判断漏到 `lib.rs` 的命令体里**——命令里出现 `match agent` 就说明 trait 的
//! 形状不对，该改的是 trait。
//!
//! 各家的实测落点见 `docs/plan/tool-management.md` 的 1.2 / 1.3 两张表。

pub mod files;
pub mod hooks;
pub mod hooks_write;
pub mod link;
pub mod mcp;
pub mod mcp_write;
pub mod registry;
pub mod registry_git;
pub mod bundle;
pub mod memo;
pub mod memo_merge;
pub mod risk;
pub mod skills;
pub mod skills_git;
pub mod skills_write;
pub mod textdiff;

use std::path::{Path, PathBuf};

/// 一个配置来源的作用域。MCP 和 skills 共用同一套 —— 两边的分层规则是同一件事，
/// 各起一套名字只会让「哪一档覆盖哪一档」出现两个说法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigScope {
    /// 用户级（home 下），跟着人走。
    User,
    /// Claude 的 local scope：也存在 `~/.claude.json` 里，但只对某个项目生效，
    /// 优先级高于 project。和 `User` 同文件不同级，所以必须是独立的一档。
    Local,
    /// 项目级，跟着仓库走。需要知道 cwd 才能算出来。
    Project,
}

/// 这个文件是这个 agent「自己的」配置，还是它兼容读进来的别人家的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigOrigin {
    /// agent 自有格式的配置文件。
    Own,
    /// 跨 agent 通用的 `.mcp.json`（Claude 起的头，grok / kimi 都认）。
    Shared,
    /// 为兼容而扫描的别人家的配置 —— grok 默认读 `~/.claude.json` 和 Cursor 的。
    Compat,
}

/// 一个 MCP 配置文件的格式。
///
/// **格式是文件的属性，不是 agent 的属性。** `.mcp.json` 这一个文件 claude / grok /
/// kimi / pi 四家都读，而 codex 和 grok 的 user 级配置又都是 TOML —— 把格式挂在 agent
/// 上，就会出现「同一个文件被两家用两种解析器读」这种必错的形状。挂在来源上之后，
/// `mcp.rs` 里一行 agent 判断都不需要，正好是 CLAUDE.md 那条「放不进 trait 说明形状
/// 错了」的要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum McpFormat {
    /// `{ "mcpServers": { "<name>": { … } } }` —— claude / agy / kimi / cursor / pi，
    /// 以及跨 agent 通用的 `.mcp.json`。七家里最常见的一种。
    JsonServers,
    /// `~/.claude.json` 的 `projects.<cwd>.mcpServers`：同一个文件里按项目分区的
    /// local scope。和 [`JsonServers`](Self::JsonServers) 同格式不同位置，所以必须是
    /// 独立的一档 —— 否则 local scope 的 server 会被当成 user 级的报出来。
    JsonProjectServers,
    /// `[mcp_servers.<name>]` —— codex / grok。
    TomlServers,
    /// opencode 的 `mcp` 键。七家里形状最不一样的一个：`type: "local" | "remote"`、
    /// `command` 是命令和参数混在一起的数组、env 键叫 `environment`。
    OpencodeJson,
}

/// 一个 hook 配置文件的格式。
///
/// 和 [`McpFormat`] 同一个理由挂在**来源**上而不是 agent 上：codex 的 `hooks.json` 和
/// claude 的 `settings.json` 是同一种形状，而 codex 自己的 `config.toml` 又是另一种 ——
/// 挂在 agent 上就没法表达「同一家两个文件两种格式」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HookFormat {
    /// `hooks: { <Event>: [ { matcher?, hooks: [{type, command, timeout?}] } ] }`
    /// —— claude 的 `settings.json`、codex 的 `hooks.json`。
    GroupedJson,
    /// `[[hooks.<Event>]]` + `hooks = [{ type, command, timeout }]` —— grok、codex 的
    /// `config.toml`。
    TomlGrouped,
    /// `[[hooks]]` 每条自带 `event` —— kimi。
    TomlList,
    /// `{ <hook 名>: { <Event>: [ { type, command } ] } }` —— agy 的 `hooks.json`，
    /// 根上是一个个**有名字的** hook，名字底下才是事件。
    AgyJson,
}

/// 一个 agent 实际会读到的一个 hook 配置文件。
///
/// **没有 `precedence`，因为 hook 不是覆盖语义而是叠加语义**：user 级配了一条、项目里
/// 又配了一条，两条都会跑。照搬 MCP 那套优先级会画出一个根本不存在的「被覆盖」关系，
/// 让用户以为删掉高优先级那条、低优先级那条就会「顶上来」。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSource {
    pub path: String,
    pub scope: ConfigScope,
    pub origin: ConfigOrigin,
    pub format: HookFormat,
    /// 工具管理会不会往这里写。同 MCP：只写 agent 自有的 user 级文件。
    pub writable: bool,
    pub exists: bool,
    /// 受某个我们判定不了的开关影响（codex 的项目配置要过信任闸）。
    pub conditional: bool,
}

fn hook_source(
    path: PathBuf,
    scope: ConfigScope,
    origin: ConfigOrigin,
    format: HookFormat,
    writable: bool,
) -> HookSource {
    HookSource {
        exists: path.is_file(),
        path: path.to_string_lossy().to_string(),
        scope,
        origin,
        format,
        writable,
        conditional: false,
    }
}

/// 一个 agent 实际会读到的一个 MCP 配置文件。
///
/// 只报「自己的 user 级文件」是不够的：grok 会从 cwd 一路扫到 git root 再合并
/// Claude / Cursor 的配置，kimi 有三个作用域。用户在项目里配的 server 如果不显示
/// 出来，看上去就是「凭空多出来的工具」。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSource {
    pub path: String,
    pub scope: ConfigScope,
    pub origin: ConfigOrigin,
    /// 这个文件按哪种格式解析。见 [`McpFormat`]。
    pub format: McpFormat,
    /// 工具管理**会不会往这里写**。目前只写 agent 自有的 user 级文件：项目级配置
    /// 属于仓库、会被提交，兼容来源属于别的 agent，两者都只读展示。
    pub writable: bool,
    pub exists: bool,
    /// 合并时的优先级，**数字大的覆盖数字小的**。同名 server 出现在多个文件里时靠
    /// 它决定谁生效。
    ///
    /// 用显式数字而不是靠数组顺序：顺序只能表达全序，而实际情况里有并列
    /// （grok 的两个 Cursor 来源），而且「数组第几个」这种隐式约定在跨进程传到前端
    /// 之后没人守得住。各 agent 的取值依据写在各自的 `mcp_sources` 上。
    pub precedence: i32,
    /// 这个来源是不是**有条件**生效 —— 受某个我们没有可靠办法判定的开关影响。
    ///
    /// 目前只有一处：grok 的项目级 `.mcp.json`，它自带文档写的是「Loaded unless you
    /// have imported or dismissed the Claude import prompt (the import marker is set)」，
    /// 而那个 marker 落在哪儿没有公开说明。判定不了就**别假装判定得了**：标成条件
    /// 生效、UI 说明「可能未生效」，比直接画进覆盖链要诚实。
    pub conditional: bool,
}

fn mcp_source(
    path: PathBuf,
    scope: ConfigScope,
    origin: ConfigOrigin,
    format: McpFormat,
    writable: bool,
    precedence: i32,
) -> McpSource {
    McpSource {
        exists: path.is_file(),
        path: path.to_string_lossy().to_string(),
        scope,
        origin,
        format,
        writable,
        precedence,
        conditional: false,
    }
}

/// 从 `cwd` 往上走到 git root（含），逐层产出目录。非仓库时只产出 `cwd` 自己。
///
/// grok 的项目级配置就是这么加载的：「walks from the current directory up to the git
/// repo root, loading `.grok/config.toml` at each level」，越深优先级越高。
fn cwd_to_repo_root(cwd: &Path) -> Vec<PathBuf> {
    let root = crate::util::repo_root(cwd);
    let mut out = Vec::new();
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        out.push(current.to_path_buf());
        if root.as_deref() == Some(current) {
            break;
        }
        dir = current.parent();
        if root.is_none() {
            break; // 不在仓库里就只看 cwd 本身，不要一路爬到 /。
        }
    }
    out
}

/// 一个 agent 实际会扫描的一个 skills 目录。
///
/// 只报 `~/<agent>/skills` 是不够的：本仓库自己的 `.claude/skills/` 里就有 7 个 skill，
/// 而 grok 的 `08-skills.md:21-29` 更是列了六档来源（本目录 / 仓库根 / user，各自还要
/// 叠一份 `.agents/` 和兼容读的 `.claude/` `.cursor/`）。漏掉项目级等于对用户说
/// 「你正在用的这些 skill 不存在」。
#[derive(Debug, Clone)]
pub struct SkillSource {
    pub path: PathBuf,
    pub scope: ConfigScope,
    pub origin: ConfigOrigin,
}

fn skill_source(path: PathBuf, scope: ConfigScope, origin: ConfigOrigin) -> SkillSource {
    SkillSource {
        path,
        scope,
        origin,
    }
}

/// 一个 agent 支持四类工具里的哪几类。
///
/// 不支持不是错误，是常态：agy 没有 home 级全局指令约定，opencode 和 Pi 没有静态
/// hook 配置（它们用 JS/TS 插件）。UI 据此把对应面板置灰并说明原因，而不是给一个
/// 输入框让用户白写。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilities {
    pub mcp: bool,
    pub skills: bool,
    pub hooks: bool,
    pub global_memo: bool,
}

/// 一个 agent 的工具与配置落点。
///
/// 默认实现一律是「没有这个能力」，所以新增一家时只需要实现它真正有的部分。
pub trait ToolSurface: Send + Sync {
    /// 这个 agent 的全局配置目录（`~/.claude`、`$CODEX_HOME` …）。
    ///
    /// 判断「本机装没装」就看它存不存在。工具管理和会话列表不一样：会话那边跟着
    /// 用户在设置里勾的可见 agent 走，这边是**全机器的全景**，不受设置控制 ——
    /// 所以只能按装没装来筛，否则七家全列出来，其中五家点进去永远是空的。
    fn config_home(&self) -> Option<PathBuf> {
        None
    }

    /// 能力位一律**从下面那些路径推导**，不由各家自己声明。
    ///
    /// 让每家手写一遍 `ToolCapabilities` 会立刻变成两份真相：能力位说有 skills、
    /// `skills_dir()` 却返回 `None`，UI 就打开一个永远空的面板。派生出来之后这种
    /// 不一致在结构上就不可能发生，不需要靠测试去守。
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            mcp: self.mcp_config_path().is_some(),
            skills: self.skills_dir().is_some(),
            hooks: self.hooks_config_path().is_some(),
            global_memo: self.memo_path().is_some(),
        }
    }

    // ---- MCP ----

    /// 用户级 MCP 配置文件。注意各家格式不同：claude / agy / kimi 是 JSON 的
    /// `mcpServers`，codex / grok 是 TOML 的 `[mcp_servers.*]`，opencode 是 JSON 的
    /// `mcp` 键且条目形状也不一样（`command` 是数组、env 键叫 `environment`）。
    fn mcp_config_path(&self) -> Option<PathBuf> {
        None
    }

    /// 自有 user 级配置文件的格式。默认 `mcpServers` 那一种 —— 七家里五家是它。
    ///
    /// 单独开一个方法而不是让每家都覆写 `mcp_sources`：只有格式不同的那家（opencode）
    /// 需要说一句话，剩下的继续吃默认实现，不用把整段来源列表抄一遍。
    fn mcp_format(&self) -> McpFormat {
        McpFormat::JsonServers
    }

    /// 这个 agent 实际会读到的**全部** MCP 配置来源，按优先级从高到低。
    ///
    /// 默认只有自己的 user 级文件。需要更多的自己覆写：grok 会合并 Claude 与
    /// Cursor 的配置，kimi 有三个作用域。`cwd` 为 `None` 时只返回 user 级的那些
    /// —— 没有工作目录就算不出项目级路径，与其猜一个不如不报。
    fn mcp_sources(&self, cwd: Option<&Path>) -> Vec<McpSource> {
        let _ = cwd;
        self.mcp_config_path()
            .map(|p| {
                vec![mcp_source(
                    p,
                    ConfigScope::User,
                    ConfigOrigin::Own,
                    self.mcp_format(),
                    true,
                    100,
                )]
            })
            .unwrap_or_default()
    }

    // ---- Skills ----

    /// 用户级 skills 目录。里面的条目应该是指向主 store 的链接。
    fn skills_dir(&self) -> Option<PathBuf> {
        None
    }

    /// 这个 agent 实际会扫到的**全部** skills 目录，含项目级。
    ///
    /// 默认只有 user 那一档。各家的项目级落点差别很大，而且**没实证过的一律不填** ——
    /// 凭空报一个 `<project>/.foo/skills` 出来，用户会以为往那儿放东西就能生效。
    fn skills_sources(&self, cwd: Option<&Path>) -> Vec<SkillSource> {
        let _ = cwd;
        self.skills_dir()
            .map(|p| vec![skill_source(p, ConfigScope::User, ConfigOrigin::Own)])
            .unwrap_or_default()
    }

    // ---- Hooks ----

    fn hooks_config_path(&self) -> Option<PathBuf> {
        None
    }

    /// 自有 user 级 hook 文件的格式。默认 claude / codex 那种分组 JSON。
    fn hook_format(&self) -> HookFormat {
        HookFormat::GroupedJson
    }

    /// 这个 agent 实际会读到的**全部** hook 文件。默认只有自己的 user 级那一个。
    fn hook_sources(&self, cwd: Option<&Path>) -> Vec<HookSource> {
        let _ = cwd;
        self.hooks_config_path()
            .map(|p| {
                vec![hook_source(
                    p,
                    ConfigScope::User,
                    ConfigOrigin::Own,
                    self.hook_format(),
                    true,
                )]
            })
            .unwrap_or_default()
    }

    /// 这个 agent **已实证**支持的 hook 事件。
    ///
    /// 各家差别很大（grok 有 `StopCancelled`，agy 是 `PreInvocation`），所以逐家列，
    /// 不写一份公共的。**没实证过的一律不填**：凭空多报一个事件，用户照着配一条永远
    /// 不会触发的 hook，然后去查自己的命令哪里写错了。每家的依据写在各自的实现上。
    fn hook_events(&self) -> &'static [&'static str] {
        &[]
    }

    // ---- 全局指令 ----

    /// 这个 agent 约定的全局指令文件。
    fn memo_path(&self) -> Option<PathBuf> {
        None
    }

    /// 除约定路径外，这个 agent 还会读进来的指令文件。
    ///
    /// opencode 的 `instructions: string[]` 就是干这个的。不报出来的话，全局配置面板
    /// 显示的「生效内容」和 agent 实际读到的对不上。
    fn memo_extra_sources(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// 约定路径不存在时，实际会被读到的回退文件。
    ///
    /// opencode 在自己的 `AGENTS.md` 缺席时回退读 `~/.claude/CLAUDE.md`，grok 也额外
    /// 兼容读 `CLAUDE.md`。返回 `None` 表示没有回退——缺了就是没有全局指令。
    fn memo_fallback(&self) -> Option<PathBuf> {
        None
    }
}

/// 一个 agent 的工具面快照，给前端渲染用。
///
/// 前端**不再自己存一份**各 agent 的能力位和路径：这些都是外部世界的事实（某家把
/// 配置从 `config.toml` 挪到 `mcp.json` 就变了），存两份必然漂移。浮层打开时问一次
/// 后端即可。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSurfaceInfo {
    pub agent: String,
    /// 本机装没装 —— 全局配置目录在不在（见 `ToolSurface::config_home`）。
    pub installed: bool,
    pub capabilities: ToolCapabilities,
    /// 工具管理会写入的那个 user 级文件。`mcp_sources` 里 `writable` 的那一条。
    pub mcp_config_path: Option<String>,
    /// 这个 agent 实际会读到的全部 MCP 来源，按优先级从高到低。
    pub mcp_sources: Vec<McpSource>,
    pub skills_dir: Option<String>,
    pub hooks_config_path: Option<String>,
    pub memo_path: Option<String>,
    /// 约定路径之外还会被读进来的指令文件（目前只有 opencode 的 `instructions`）。
    pub memo_extra_sources: Vec<String>,
    pub memo_fallback: Option<String>,
}

fn path_string(p: Option<PathBuf>) -> Option<String> {
    p.map(|p| p.to_string_lossy().to_string())
}

/// 本机装了这个 agent 没有 —— 全局配置目录在不在。
///
/// 用目录而不是可执行文件：CLI 可能装在 nvm / homebrew / 自编译的任意位置，而配置
/// 目录是它第一次跑完就一定会建的，且正是这个面板要读写的东西。
pub(crate) fn is_installed(s: &dyn ToolSurface) -> bool {
    s.config_home().is_some_and(|p| p.is_dir())
}

/// 七家的工具面快照，顺序固定。浮层打开时拉一次。
///
/// `cwd` 是当前活动项目的目录。给了才能算出项目级的 MCP 来源（grok 的
/// `.grok/config.toml`、各家通用的 `.mcp.json`、kimi 的 `.kimi-code/mcp.json`）；
/// 不给就只报 user 级的。
#[tauri::command(async)]
pub fn tool_surfaces(cwd: Option<String>) -> Vec<ToolSurfaceInfo> {
    let cwd = cwd.filter(|c| !c.trim().is_empty()).map(PathBuf::from);
    AGENTS
        .iter()
        .filter_map(|agent| {
            let s = surface(agent).ok()?;
            Some(ToolSurfaceInfo {
                agent: (*agent).to_string(),
                installed: is_installed(s.as_ref()),
                capabilities: s.capabilities(),
                mcp_config_path: path_string(s.mcp_config_path()),
                mcp_sources: s.mcp_sources(cwd.as_deref()),
                skills_dir: path_string(s.skills_dir()),
                hooks_config_path: path_string(s.hooks_config_path()),
                memo_path: path_string(s.memo_path()),
                memo_extra_sources: s
                    .memo_extra_sources()
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
                memo_fallback: path_string(s.memo_fallback()),
            })
        })
        .collect()
}

/// 规范的 agent 名单，与前端 `types.ts` 的 `Agent` 联合类型一一对应。
pub const AGENTS: &[&str] = &[
    "claude", "codex", "grok", "agy", "opencode", "kimicode", "pi",
];

/// agent 名 → 工具面。分支集合与 `agents::source()` 保持一致。
pub fn surface(agent: &str) -> Result<Box<dyn ToolSurface>, String> {
    match agent {
        "agy" => Ok(Box::new(AgySurface)),
        "claude" => Ok(Box::new(ClaudeSurface)),
        "codex" => Ok(Box::new(CodexSurface)),
        "grok" => Ok(Box::new(GrokSurface)),
        "kimicode" | "kimi" => Ok(Box::new(KimiSurface)),
        "opencode" => Ok(Box::new(OpencodeSurface)),
        "pi" => Ok(Box::new(PiSurface)),
        other => Err(format!("Unknown agent: {other}")),
    }
}

// ---------------------------------------------------------------------------
// 七家的落点。全部来自本机实测，依据见方案文档 1.2 / 1.3。
//
// 目前每家都短到几行，所以先放在一个文件里；等哪家开始长出自己的解析逻辑
// （比如 opencode 的 `command` 数组拆分）再拆成 `tools/<name>.rs`。
// ---------------------------------------------------------------------------

use crate::util::home;

pub struct ClaudeSurface;
impl ToolSurface for ClaudeSurface {
    fn config_home(&self) -> Option<PathBuf> {
        Some(home().join(".claude"))
    }
    fn mcp_config_path(&self) -> Option<PathBuf> {
        Some(home().join(".claude.json"))
    }
    /// Claude 的 scope 次序是 **local > project > user**，而 local 和 user 装在
    /// **同一个** `~/.claude.json` 里。
    ///
    /// 所以这个文件报成两条：同路径、不同 scope、不同优先级。用一条记的话必须二选一
    /// ——按 local 记会让 user scope 的 server 显示成高于项目配置，按 user 记又会让
    /// local scope 的显示成被 `.mcp.json` 覆盖掉，两种都是错的覆盖关系。同名两条看着
    /// 别扭，但它就是这个文件的真实形状；前端按 path 分组即可。
    ///
    /// 写入只落在 user 那一档 —— local scope 是按项目分区的，phase 6 单独处理。
    fn mcp_sources(&self, cwd: Option<&Path>) -> Vec<McpSource> {
        let config = home().join(".claude.json");
        let mut out = vec![mcp_source(
            config.clone(),
            ConfigScope::Local,
            ConfigOrigin::Own,
            McpFormat::JsonProjectServers,
            false,
            300,
        )];
        if let Some(root) = cwd.map(crate::util::project_root) {
            out.push(mcp_source(
                root.join(".mcp.json"),
                ConfigScope::Project,
                ConfigOrigin::Shared,
                McpFormat::JsonServers,
                false,
                200,
            ));
        }
        out.push(mcp_source(
            config,
            ConfigScope::User,
            ConfigOrigin::Own,
            McpFormat::JsonServers,
            true,
            100,
        ));
        out
    }
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(home().join(".claude").join("skills"))
    }
    /// 实测：本仓库 `.claude/skills/` 下就有 7 个 skill（`git-push`、`openspec-*` …）。
    fn skills_sources(&self, cwd: Option<&Path>) -> Vec<SkillSource> {
        let mut out = vec![skill_source(
            home().join(".claude").join("skills"),
            ConfigScope::User,
            ConfigOrigin::Own,
        )];
        if let Some(root) = cwd.map(crate::util::project_root) {
            out.push(skill_source(
                root.join(".claude").join("skills"),
                ConfigScope::Project,
                ConfigOrigin::Own,
            ));
        }
        out
    }
    fn hooks_config_path(&self) -> Option<PathBuf> {
        Some(home().join(".claude").join("settings.json"))
    }
    /// 依据：claude 2.1.268 二进制里自带的事件表（`| Event | Matcher | Purpose |` 那张）
    /// 加上二进制里单独出现的 `StopFailure` / `SubagentStart` / `SubagentStop` /
    /// `SessionEnd`。顺序按「会话 → 回合 → 工具 → 压缩」排，添加框里照这个分组。
    fn hook_events(&self) -> &'static [&'static str] {
        &[
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "Stop",
            "StopFailure",
            "SubagentStart",
            "SubagentStop",
            "Notification",
            "PermissionRequest",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PreCompact",
            "PostCompact",
        ]
    }
    /// claude 的 hook 是**叠加**的：user 级和项目级都会跑，不存在谁覆盖谁。项目里那两
    /// 份只读 —— 它们跟着仓库走，会被提交。
    fn hook_sources(&self, cwd: Option<&Path>) -> Vec<HookSource> {
        let mut out = Vec::new();
        if let Some(root) = cwd.map(crate::util::project_root) {
            out.push(hook_source(
                root.join(".claude").join("settings.local.json"),
                ConfigScope::Local,
                ConfigOrigin::Own,
                HookFormat::GroupedJson,
                false,
            ));
            out.push(hook_source(
                root.join(".claude").join("settings.json"),
                ConfigScope::Project,
                ConfigOrigin::Own,
                HookFormat::GroupedJson,
                false,
            ));
        }
        out.push(hook_source(
            home().join(".claude").join("settings.json"),
            ConfigScope::User,
            ConfigOrigin::Own,
            HookFormat::GroupedJson,
            true,
        ));
        out
    }
    fn memo_path(&self) -> Option<PathBuf> {
        Some(home().join(".claude").join("CLAUDE.md"))
    }
}

pub struct CodexSurface;
impl ToolSurface for CodexSurface {
    fn config_home(&self) -> Option<PathBuf> {
        Some(codex_home())
    }
    fn mcp_config_path(&self) -> Option<PathBuf> {
        Some(codex_home().join("config.toml"))
    }
    fn mcp_format(&self) -> McpFormat {
        McpFormat::TomlServers
    }
    /// codex 的 MCP 不止 user 一档：**项目级 `<repo>/.codex/config.toml` 同样能写
    /// `[mcp_servers.*]`**，而且优先级更高。
    ///
    /// 依据是 codex 0.154.0 自带的那段文档串：「Project `.codex/config.toml`: settings
    /// for a trusted repository, including sandbox, **MCP**, hooks, model, and reasoning
    /// defaults.」，配上二进制里那条 `Overridden by project config:` —— 同名时项目那份
    /// 赢。本机 10 个仓库有 `.codex/config.toml`，其中 9 个写了 `[mcp_servers.*]`，
    /// **本仓库自己就是一个**。漏掉这一档，codex 在项目里实际加载的 server 全部报不出来。
    ///
    /// 标成**条件生效**：它只对「受信任的仓库」加载，而信任有两道闸 ——
    /// `~/.codex/config.toml` 里的 `[projects."<路径>"] trust_level`（这个读得到）和一份
    /// `trusted_hash`（配置改过要重新确认，这个我们复现不了）。只读得到一道闸就当全知道，
    /// 会把一个 codex 其实没加载的 server 画成正在跑的 —— 和 grok 的项目级 `.mcp.json`
    /// 同样的处理：判定不了就别假装判定得了。
    fn mcp_sources(&self, cwd: Option<&Path>) -> Vec<McpSource> {
        let mut out = Vec::new();
        if let Some(root) = cwd.map(crate::util::project_root) {
            out.push(McpSource {
                conditional: true,
                ..mcp_source(
                    root.join(".codex").join("config.toml"),
                    ConfigScope::Project,
                    ConfigOrigin::Own,
                    McpFormat::TomlServers,
                    false,
                    200,
                )
            });
        }
        out.push(mcp_source(
            codex_home().join("config.toml"),
            ConfigScope::User,
            ConfigOrigin::Own,
            McpFormat::TomlServers,
            true,
            100,
        ));
        out
    }
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(codex_home().join("skills"))
    }
    /// 实测：本机 `~/apps/work` 下四个仓库各有一个 `.codex/skills/`。
    ///
    /// **`~/.agents/skills` 那一档是漏不得的**：codex 也读跨 agent 的公共目录（用户实测，
    /// 且 codex 0.154.0 的二进制里 `.agents/skills` 就挨着 `.codex/agents`、`.codex/hooks`
    /// 一起躺在同一张目录表里）。漏了它，公共目录里的 skill 会被报成「codex 读不到」。
    ///
    /// 只加 user 一档：项目级的 `<repo>/.agents/skills` codex 认不认没有实证，
    /// 宁可少报一个也不要让面板承诺一件没验过的事。
    fn skills_sources(&self, cwd: Option<&Path>) -> Vec<SkillSource> {
        let mut out = vec![
            skill_source(
                codex_home().join("skills"),
                ConfigScope::User,
                ConfigOrigin::Own,
            ),
            skill_source(
                home().join(".agents").join("skills"),
                ConfigScope::User,
                ConfigOrigin::Shared,
            ),
        ];
        if let Some(root) = cwd.map(crate::util::project_root) {
            out.push(skill_source(
                root.join(".codex").join("skills"),
                ConfigScope::Project,
                ConfigOrigin::Own,
            ));
        }
        out
    }
    fn hooks_config_path(&self) -> Option<PathBuf> {
        Some(codex_home().join("hooks.json"))
    }
    /// 依据：codex 0.154.0 二进制里的事件枚举，一个连着一个躺在同一段字符串里 ——
    /// `PreToolUsePermissionRequestPostToolUsePreCompactPostCompactSessionStartSessionEnd`
    /// `UserPromptSubmitSubagentStartSubagentStopStopInterrupt`。它没有 `Notification`，
    /// 也没有 `StopFailure`，别照着 claude 那份抄。
    fn hook_events(&self) -> &'static [&'static str] {
        &[
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "Stop",
            "Interrupt",
            "SubagentStart",
            "SubagentStop",
            "PermissionRequest",
            "PreToolUse",
            "PostToolUse",
            "PreCompact",
            "PostCompact",
        ]
    }
    /// 三档，**都会跑**（hook 是叠加的）：
    /// - `$CODEX_HOME/hooks.json` —— 专用文件，唯一可写的一档
    /// - `$CODEX_HOME/config.toml` —— `ConfigToml` 里有 `HookEventsToml`，同样被读
    /// - `<git root>/.codex/config.toml` —— 和 MCP 同一份依据（自带文档串写着项目配置
    ///   包含 hooks），同样标条件生效：信任的第二道闸 `trusted_hash` 复现不了
    fn hook_sources(&self, cwd: Option<&Path>) -> Vec<HookSource> {
        let mut out = Vec::new();
        if let Some(root) = cwd.map(crate::util::project_root) {
            out.push(HookSource {
                conditional: true,
                ..hook_source(
                    root.join(".codex").join("config.toml"),
                    ConfigScope::Project,
                    ConfigOrigin::Own,
                    HookFormat::TomlGrouped,
                    false,
                )
            });
        }
        out.push(hook_source(
            codex_home().join("hooks.json"),
            ConfigScope::User,
            ConfigOrigin::Own,
            HookFormat::GroupedJson,
            true,
        ));
        out.push(hook_source(
            codex_home().join("config.toml"),
            ConfigScope::User,
            ConfigOrigin::Own,
            HookFormat::TomlGrouped,
            false,
        ));
        out
    }
    fn memo_path(&self) -> Option<PathBuf> {
        Some(codex_home().join("AGENTS.md"))
    }
}

pub struct GrokSurface;
impl ToolSurface for GrokSurface {
    fn config_home(&self) -> Option<PathBuf> {
        Some(crate::agents::grok::grok_home())
    }
    fn mcp_config_path(&self) -> Option<PathBuf> {
        Some(crate::agents::grok::grok_home().join("config.toml"))
    }
    /// grok 是七家里来源最多的。两条规则叠在一起（依据都是它自带的
    /// `~/.grok/docs/user-guide/07-mcp-servers.md`）：
    ///
    /// - **`.grok/config.toml` 逐层加载**：「walks from the current directory up to the
    ///   git repo root, loading `.grok/config.toml` at each level」，`<cwd>` 最高、
    ///   `<repo-root>` 次之、`~/.grok` 最低（同文档的 Priority 表）。
    /// - **跨来源优先级**：`config.toml > Claude > Cursor > .mcp.json`（同文档 :233），
    ///   而 Cursor 自己有 home 和 project 两份。
    ///
    /// 所以整个 `config.toml` 家族都排在 Claude 之上，逐层的深度只在家族内部分高低。
    fn mcp_sources(&self, cwd: Option<&Path>) -> Vec<McpSource> {
        let mut out = Vec::new();
        if let Some(cwd) = cwd {
            // 越深越优先：给每层一个随深度递增的分数，全部落在 400 这一档里。
            let levels = cwd_to_repo_root(cwd);
            for (i, dir) in levels.iter().enumerate() {
                out.push(mcp_source(
                    dir.join(".grok").join("config.toml"),
                    ConfigScope::Project,
                    ConfigOrigin::Own,
                    McpFormat::TomlServers,
                    false,
                    410 + (levels.len() - i) as i32,
                ));
            }
        }
        out.push(mcp_source(
            crate::agents::grok::grok_home().join("config.toml"),
            ConfigScope::User,
            ConfigOrigin::Own,
            McpFormat::TomlServers,
            true,
            400,
        ));
        // 兼容来源是**可以关掉的**，关了还报出来就是在说谎：用户会以为 Claude 那边
        // 加的 server 在 grok 里生效，其实没有。
        if grok_compat_enabled("claude", "mcps") {
            out.push(mcp_source(
                home().join(".claude.json"),
                ConfigScope::User,
                ConfigOrigin::Compat,
                McpFormat::JsonServers,
                false,
                300,
            ));
        }
        if grok_compat_enabled("cursor", "mcps") {
            if let Some(root) = cwd.map(crate::util::project_root) {
                out.push(mcp_source(
                    root.join(".cursor").join("mcp.json"),
                    ConfigScope::Project,
                    ConfigOrigin::Compat,
                    McpFormat::JsonServers,
                    false,
                    201,
                ));
            }
            out.push(mcp_source(
                home().join(".cursor").join("mcp.json"),
                ConfigScope::User,
                ConfigOrigin::Compat,
                McpFormat::JsonServers,
                false,
                200,
            ));
        }
        if let Some(root) = cwd.map(crate::util::project_root) {
            // 条件生效：文档说它在 Claude import prompt 被导入或忽略（import marker
            // 已置位）之后就不再加载，而那个 marker 在哪儿没有公开说明。
            out.push(McpSource {
                conditional: true,
                ..mcp_source(
                    root.join(".mcp.json"),
                    ConfigScope::Project,
                    ConfigOrigin::Shared,
                    McpFormat::JsonServers,
                    false,
                    100,
                )
            });
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.precedence));
        out
    }
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(crate::agents::grok::grok_home().join("skills"))
    }
    /// 依据：`08-skills.md:21-29` 的来源表。它逐层从 cwd 走到 repo root，每层都同时看
    /// `.grok/` 和 `.agents/`，再按 `[compat.<vendor>] skills` 决定要不要加上 Claude /
    /// Cursor 的目录。**`.agents/` 是每一层都扫的**，不是只有 home 那份 —— 漏掉的话
    /// 用户在仓库里放的共享 skill 会被报成不存在。
    fn skills_sources(&self, cwd: Option<&Path>) -> Vec<SkillSource> {
        let claude = grok_compat_enabled("claude", "skills");
        let cursor = grok_compat_enabled("cursor", "skills");
        let mut out = Vec::new();
        if let Some(cwd) = cwd {
            for dir in cwd_to_repo_root(cwd) {
                out.push(skill_source(
                    dir.join(".grok").join("skills"),
                    ConfigScope::Project,
                    ConfigOrigin::Own,
                ));
                out.push(skill_source(
                    dir.join(".agents").join("skills"),
                    ConfigScope::Project,
                    ConfigOrigin::Shared,
                ));
                if claude {
                    out.push(skill_source(
                        dir.join(".claude").join("skills"),
                        ConfigScope::Project,
                        ConfigOrigin::Compat,
                    ));
                }
                if cursor {
                    out.push(skill_source(
                        dir.join(".cursor").join("skills"),
                        ConfigScope::Project,
                        ConfigOrigin::Compat,
                    ));
                }
            }
        }
        out.push(skill_source(
            crate::agents::grok::grok_home().join("skills"),
            ConfigScope::User,
            ConfigOrigin::Own,
        ));
        out.push(skill_source(
            home().join(".agents").join("skills"),
            ConfigScope::User,
            ConfigOrigin::Shared,
        ));
        if claude {
            out.push(skill_source(
                home().join(".claude").join("skills"),
                ConfigScope::User,
                ConfigOrigin::Compat,
            ));
        }
        if cursor {
            out.push(skill_source(
                home().join(".cursor").join("skills"),
                ConfigScope::User,
                ConfigOrigin::Compat,
            ));
        }
        out
    }
    fn hooks_config_path(&self) -> Option<PathBuf> {
        Some(crate::agents::grok::grok_home().join("config.toml"))
    }
    fn hook_format(&self) -> HookFormat {
        HookFormat::TomlGrouped
    }
    /// grok 的二进制是打包过的，事件名不在明文字符串里，所以这里**只列本仓库实测装得
    /// 上、且真的会触发的那几个**（`turn.rs` 的 `GROK_TURN_HOOKS` 就是它们）。宁可少列
    /// 几个，也不凭 claude 那份去推 —— 推错一个，用户配的 hook 永远不会响。
    fn hook_events(&self) -> &'static [&'static str] {
        &[
            "UserPromptSubmit",
            "Stop",
            "StopFailure",
            "StopCancelled",
            "Notification",
        ]
    }
    fn memo_path(&self) -> Option<PathBuf> {
        Some(crate::agents::grok::grok_home().join("AGENTS.md"))
    }
    fn memo_fallback(&self) -> Option<PathBuf> {
        Some(home().join(".claude").join("CLAUDE.md"))
    }
}

pub struct KimiSurface;
impl ToolSurface for KimiSurface {
    fn config_home(&self) -> Option<PathBuf> {
        Some(crate::agents::kimi::kimi_home())
    }
    /// kimi 的 MCP 单独一个文件，不在 `config.toml` 里。它自带的 `/mcp-config` skill
    /// 特意写了 "never assume `~/.kimi-code`"，所以必须走 `kimi_home()` 解析
    /// `KIMI_CODE_HOME`。
    fn mcp_config_path(&self) -> Option<PathBuf> {
        Some(crate::agents::kimi::kimi_home().join("mcp.json"))
    }
    /// 三个作用域，来自它自带的 `/mcp-config` skill 正文：user
    /// `<KIMI_CODE_HOME>/mcp.json`、project-root `<root>/.mcp.json`、project-local
    /// `<cwd>/.kimi-code/mcp.json`，三者都是 `{ "mcpServers": { … } }`。
    ///
    /// 优先级按同一段的原话定：「Config lives in three files; on key collision, later
    /// entries in this precedence order override earlier ones」，而列举顺序正是
    /// user → project-root → project-local —— 也就是 **project-local 最高、user 最低**。
    fn mcp_sources(&self, cwd: Option<&Path>) -> Vec<McpSource> {
        let mut out = Vec::new();
        if let Some(cwd) = cwd {
            out.push(mcp_source(
                cwd.join(".kimi-code").join("mcp.json"),
                ConfigScope::Project,
                ConfigOrigin::Own,
                McpFormat::JsonServers,
                false,
                300,
            ));
            out.push(mcp_source(
                crate::util::project_root(cwd).join(".mcp.json"),
                ConfigScope::Project,
                ConfigOrigin::Shared,
                McpFormat::JsonServers,
                false,
                200,
            ));
        }
        out.push(mcp_source(
            crate::agents::kimi::kimi_home().join("mcp.json"),
            ConfigScope::User,
            ConfigOrigin::Own,
            McpFormat::JsonServers,
            true,
            100,
        ));
        out
    }
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(crate::agents::kimi::kimi_home().join("skills"))
    }
    /// 依据是 kimi 二进制里那四张目录表，原样抄出来：
    ///
    /// ```text
    /// USER_BRAND_DIRS      = ["skills"]            // join(osHomeDir 下的 kimi 配置目录)
    /// USER_GENERIC_DIRS    = [".agents/skills"]    // join(osHomeDir)  → ~/.agents/skills
    /// PROJECT_BRAND_DIRS   = [".kimi-code/skills"]
    /// PROJECT_GENERIC_DIRS = [".agents/skills"]
    /// ```
    ///
    /// 也就是说 **kimi 是读 `~/.agents/skills` 的**（走 `pushFirstExisting(roots, …, "user")`）。
    /// 之前这里没写 `skills_sources`，落到默认实现上只报了自己那个 `~/.kimi-code/skills`，
    /// 于是公共目录里的 skill 会被报成「kimi 读不到」——而用户那边明明能用。
    fn skills_sources(&self, cwd: Option<&Path>) -> Vec<SkillSource> {
        let mut out = Vec::new();
        if let Some(cwd) = cwd {
            out.push(skill_source(
                cwd.join(".kimi-code").join("skills"),
                ConfigScope::Project,
                ConfigOrigin::Own,
            ));
            out.push(skill_source(
                cwd.join(".agents").join("skills"),
                ConfigScope::Project,
                ConfigOrigin::Shared,
            ));
        }
        out.push(skill_source(
            crate::agents::kimi::kimi_home().join("skills"),
            ConfigScope::User,
            ConfigOrigin::Own,
        ));
        out.push(skill_source(
            home().join(".agents").join("skills"),
            ConfigScope::User,
            ConfigOrigin::Shared,
        ));
        out
    }
    fn hooks_config_path(&self) -> Option<PathBuf> {
        Some(crate::agents::kimi::config_path())
    }
    fn hook_format(&self) -> HookFormat {
        HookFormat::TomlList
    }
    /// 同 grok：二进制打包过，只列 `turn.rs` 的 `KIMI_TURN_HOOKS` 里实测过的那几个。
    fn hook_events(&self) -> &'static [&'static str] {
        &[
            "TurnStarted",
            "Stop",
            "StopFailure",
            "PermissionRequest",
            "Interrupt",
        ]
    }
    fn memo_path(&self) -> Option<PathBuf> {
        Some(crate::agents::kimi::kimi_home().join("AGENTS.md"))
    }
}

/// opencode 没有静态 hook 配置：`Config` 顶层只有 `plugin: string[]`，靠 JS 插件扩展，
/// 不给路径，能力位自然是 false。
pub struct OpencodeSurface;
impl ToolSurface for OpencodeSurface {
    fn config_home(&self) -> Option<PathBuf> {
        Some(opencode_config_dir())
    }
    fn mcp_config_path(&self) -> Option<PathBuf> {
        Some(opencode_config_dir().join("opencode.json"))
    }
    fn mcp_format(&self) -> McpFormat {
        McpFormat::OpencodeJson
    }
    /// 单数 `skill/`。它的 glob 是 `{skill,skills}/**/SKILL.md`，两个名字都认，但它自己
    /// 的目录表一律把单数写在前面（`agent(s)` / `command(s)` / `skill(s)`），要新建就建
    /// 单数那个；已经存在的复数目录由 `skills_sources` 一并报出来。
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(opencode_config_dir().join("skill"))
    }
    /// 依据是 opencode 1.18.30 二进制里那段扫描：先按 `OPENCODE_DISABLE_EXTERNAL_SKILLS` /
    /// `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` 决定要不要扫 `~/.claude` 和 `~/.agents`
    /// （glob `skills/**/SKILL.md`），再从 cwd 往上找同名目录，最后才是它自己的配置目录
    /// （glob `{skill,skills}/**/SKILL.md`）。同一段文档表原话：
    /// 「External skills (auto-loaded) | `~/.claude/skills/<name>/SKILL.md`,
    /// `~/.agents/skills/<name>/SKILL.md`」。
    ///
    /// **这两个外部目录默认是开着的。** 漏掉它俩，本机 opencode 实际读到的那几十个
    /// skill 会被报成「opencode 读不到」—— 而用户在它的 Skills 面板里明明看得见。
    fn skills_sources(&self, cwd: Option<&Path>) -> Vec<SkillSource> {
        let external = !opencode_flag("OPENCODE_DISABLE_EXTERNAL_SKILLS");
        let claude = external
            && !opencode_flag("OPENCODE_DISABLE_CLAUDE_CODE")
            && !opencode_flag("OPENCODE_DISABLE_CLAUDE_CODE_SKILLS");
        let mut out = Vec::new();
        if let Some(cwd) = cwd {
            for dir in cwd_to_repo_root(cwd) {
                for name in ["skill", "skills"] {
                    out.push(skill_source(
                        dir.join(".opencode").join(name),
                        ConfigScope::Project,
                        ConfigOrigin::Own,
                    ));
                }
                if external {
                    out.push(skill_source(
                        dir.join(".agents").join("skills"),
                        ConfigScope::Project,
                        ConfigOrigin::Shared,
                    ));
                }
                if claude {
                    out.push(skill_source(
                        dir.join(".claude").join("skills"),
                        ConfigScope::Project,
                        ConfigOrigin::Compat,
                    ));
                }
            }
        }
        for name in ["skill", "skills"] {
            out.push(skill_source(
                opencode_config_dir().join(name),
                ConfigScope::User,
                ConfigOrigin::Own,
            ));
        }
        if external {
            out.push(skill_source(
                home().join(".agents").join("skills"),
                ConfigScope::User,
                ConfigOrigin::Shared,
            ));
        }
        if claude {
            out.push(skill_source(
                home().join(".claude").join("skills"),
                ConfigScope::User,
                ConfigOrigin::Compat,
            ));
        }
        // `skills.paths` 里的显式目录。glob 条目跳过，理由同 `opencode_extra_instructions`。
        out.extend(opencode_declared_skill_dirs());
        out
    }
    fn memo_path(&self) -> Option<PathBuf> {
        Some(opencode_config_dir().join("AGENTS.md"))
    }
    fn memo_extra_sources(&self) -> Vec<PathBuf> {
        opencode_extra_instructions()
    }
    fn memo_fallback(&self) -> Option<PathBuf> {
        Some(home().join(".claude").join("CLAUDE.md"))
    }
}

/// agy 的全局指令在 `~/.gemini/config/` 下，见 [`AgySurface::memo_path`]。
///
/// 这里原先写的是「只有目录级的 `GEMINI.md` / `AGENTS.md`（从 cwd 往上找），没有 home
/// 级全局约定」——**错了**，那只看了「目录规则逐层往上找」那一档。`~/.gemini/config/`
/// 本身就是一个定制根，根下的 `GEMINI.md` / `AGENTS.md` 是全局规则。照旧结论走，面板
/// 对 agy 是禁用态，用户写在那儿的一整套全局规则一个字都看不到。
pub struct AgySurface;
impl ToolSurface for AgySurface {
    fn config_home(&self) -> Option<PathBuf> {
        Some(home().join(".gemini"))
    }
    fn mcp_config_path(&self) -> Option<PathBuf> {
        Some(home().join(".gemini").join("settings.json"))
    }
    /// **不是 `~/.gemini/skills`。** 它的全局发现位置是 `~/.gemini/config/`（和
    /// `hooks.json` / `mcp_config.json` 同级），skills 在它下面一层。按错一层的后果是
    /// 建出来的目录 agy 永远扫不到，而 UI 会显示成已启用。
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(agy_config_dir().join("skills"))
    }
    /// 依据是 agy 1.2.1 自带的定制文档：发现位置分三档 —— 工作区 `.agents/`（别名
    /// `.agent/` `_agents/` `_agent/`，从 cwd 逐层走到仓库根）、`skills.json` 声明、
    /// 全局 `~/.gemini/config/`。四个别名都要报：用户用哪个都生效，只认 `.agents` 会
    /// 把另外三个报成不存在。
    fn skills_sources(&self, cwd: Option<&Path>) -> Vec<SkillSource> {
        let mut out = Vec::new();
        if let Some(cwd) = cwd {
            for dir in cwd_to_repo_root(cwd) {
                for name in AGY_WORKSPACE_DIRS {
                    out.push(skill_source(
                        dir.join(name).join("skills"),
                        ConfigScope::Project,
                        // `.agents/` 是跨 agent 的公共约定，另外三个是它自己的别名。
                        if *name == ".agents" {
                            ConfigOrigin::Shared
                        } else {
                            ConfigOrigin::Own
                        },
                    ));
                }
            }
        }
        out.push(skill_source(
            agy_config_dir().join("skills"),
            ConfigScope::User,
            ConfigOrigin::Own,
        ));
        out
    }
    fn hooks_config_path(&self) -> Option<PathBuf> {
        Some(agy_config_dir().join("hooks.json"))
    }
    /// 全局规则：`~/.gemini/config/` 是它的**全局定制根**，根下的标准规则文件就是
    /// 「Rules … 或者独立的 `GEMINI.md` / `AGENTS.md` 文件」（agy 1.2.1 二进制里自带的
    /// 定制文档，`## Customization Elements` 那节）。
    ///
    /// 两个名字都认，所以取现存的那个（见 [`agy_global_memo`]）；两份都在时另一份进
    /// `memo_extra_sources` —— 它是**两份都读**（规则按解析后的路径去重，不是二选一），
    /// 少报一份，面板上的「生效内容」就比 agy 实际读到的少一截。
    ///
    /// 全局根下的 `rules/*.md` 不在这儿报：那些带 frontmatter，只有 `always_on` 的才
    /// 无条件加载，报成「全局指令」会把 `model_decision` 的那几份说成一直生效。
    fn memo_path(&self) -> Option<PathBuf> {
        Some(agy_global_memo())
    }
    fn memo_extra_sources(&self) -> Vec<PathBuf> {
        let dir = agy_config_dir();
        let own = agy_global_memo();
        AGY_MEMO_NAMES
            .iter()
            .map(|name| dir.join(name))
            .filter(|path| *path != own && path.is_file())
            .collect()
    }
    fn hook_format(&self) -> HookFormat {
        HookFormat::AgyJson
    }
    /// 依据：agy 二进制里明文出现的这几个事件名。`PreInvocation` 是它独有的，别家没有。
    fn hook_events(&self) -> &'static [&'static str] {
        &[
            "SessionStart",
            "PreInvocation",
            "Stop",
            "PreToolUse",
            "PostToolUse",
        ]
    }
}

/// agy 认的四个工作区目录名，优先级相同。顺序即文档里的列举顺序。
const AGY_WORKSPACE_DIRS: &[&str] = &[".agents", ".agent", "_agents", "_agent"];

fn agy_config_dir() -> PathBuf {
    home().join(".gemini").join("config")
}

/// agy 全局根下的规则文件名。两个都认，`GEMINI.md` 在前 —— 它自己的文档两处都把
/// `GEMINI.md` 写在前面，一份都没有时也该按这个名字新建。
const AGY_MEMO_NAMES: &[&str] = &["GEMINI.md", "AGENTS.md"];

/// agy 现在实际读的那份全局规则。两份都在时报第一个，另一份由 `memo_extra_sources`
/// 补上。
fn agy_global_memo() -> PathBuf {
    let dir = agy_config_dir();
    AGY_MEMO_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| dir.join(AGY_MEMO_NAMES[0]))
}

/// Pi 的 MCP 由 `npm:pi-mcp-adapter` 提供，是可装可不装的扩展，所以那个能力位要**看
/// 扩展装没装**，不能因为本机恰好装了就写死。
///
/// **skills 和全局指令都是内建的**，见 `skills_dir` / `memo_path`。`npm:pi-memory` 的
/// `~/.pi/agent/memory/MEMORY.md` 是它**额外**喂进去的一份，不是 Pi 的约定文件——这两件
/// 事一度被记成同一件，于是面板把没装扩展的机器整片报成「Pi 没有全局指令」。
pub struct PiSurface;
impl ToolSurface for PiSurface {
    fn config_home(&self) -> Option<PathBuf> {
        Some(home().join(".pi"))
    }
    /// 装了适配器才有 MCP。文件是 `$PI_AGENT_DIR/mcp.json` —— 适配器自己的
    /// `getPiGlobalConfigPath()` 就是 `getAgentPath("mcp.json")`，而且它是六个来源里
    /// 唯一的 `writePath`（`config.ts` 里那张 `getConfigSources` 表，每个来源都写明了
    /// 读哪儿、写哪儿）。
    ///
    /// 这里原先写的是「server 声明不落在一个我们能读写的固定文件里，所以一律不报」。
    /// 那是在没读适配器源码时下的结论，**错了**：`mcp-cache.json` 确实是缓存，但它
    /// 旁边就有一份正经配置。照旧结论走的话，本机 pi 实际加载的 server 会被整片报成
    /// 「pi 没有 MCP」。
    fn mcp_config_path(&self) -> Option<PathBuf> {
        pi_has_package("npm:pi-mcp-adapter").then(|| pi_agent_dir().join("mcp.json"))
    }
    /// 六档，原样抄自适配器 `config.ts:450-530` 的 `getConfigSources()`，**顺序即合并
    /// 顺序，后面的覆盖前面的**（同文件 `mergeConfigs` 依次叠加，冲突报告里
    /// `winner: sources[sources.length - 1]`）：
    ///
    /// ```text
    /// shared-global        ~/.config/mcp/mcp.json        import
    /// agents-global        ~/.agents/mcp.json            import
    /// agents-nested-global ~/.agents/mcp/mcp.json        import
    /// pi-global            $PI_AGENT_DIR/mcp.json        user   ← 唯一可写
    /// shared-project       <cwd>/.mcp.json               project
    /// pi-project           <cwd>/.pi/mcp.json            project
    /// ```
    ///
    /// 注意 `<cwd>` 是**当前目录本身**，不是 git root：适配器用的是
    /// `resolve(cwd, ".mcp.json")`，没有往上找。
    fn mcp_sources(&self, cwd: Option<&Path>) -> Vec<McpSource> {
        if self.mcp_config_path().is_none() {
            return Vec::new();
        }
        let json = |path: PathBuf, scope, origin, writable, precedence| {
            mcp_source(path, scope, origin, McpFormat::JsonServers, writable, precedence)
        };
        let mut out = Vec::new();
        if let Some(cwd) = cwd {
            out.push(json(
                cwd.join(".pi").join("mcp.json"),
                ConfigScope::Project,
                ConfigOrigin::Own,
                false,
                600,
            ));
            out.push(json(
                cwd.join(".mcp.json"),
                ConfigScope::Project,
                ConfigOrigin::Shared,
                false,
                500,
            ));
        }
        out.push(json(
            pi_agent_dir().join("mcp.json"),
            ConfigScope::User,
            ConfigOrigin::Own,
            true,
            400,
        ));
        out.push(json(
            home().join(".agents").join("mcp").join("mcp.json"),
            ConfigScope::User,
            ConfigOrigin::Shared,
            false,
            300,
        ));
        out.push(json(
            home().join(".agents").join("mcp.json"),
            ConfigScope::User,
            ConfigOrigin::Shared,
            false,
            200,
        ));
        out.push(json(
            home().join(".config").join("mcp").join("mcp.json"),
            ConfigScope::User,
            ConfigOrigin::Shared,
            false,
            100,
        ));
        out
    }
    /// 全局指令是 pi **内建**的，不看扩展：`loadProjectContextFiles()` 无条件先从
    /// `agentDir` 读一份（0.85.1 bundle `chunk-JVUZSMYM.js`），候选名和项目级那套一样，
    /// 见 [`pi_global_memo`]。
    fn memo_path(&self) -> Option<PathBuf> {
        Some(pi_global_memo())
    }
    /// `npm:pi-memory` 会把 `~/.pi/agent/memory/MEMORY.md` 也喂进上下文。它不是约定文件
    /// （约定文件是上面那份 `AGENTS.md`），但装了扩展的机器上确实有人读——不报出来的话，
    /// 面板显示的「生效内容」比 pi 实际读到的少一份。
    fn memo_extra_sources(&self) -> Vec<PathBuf> {
        if pi_has_package("npm:pi-memory") {
            vec![pi_agent_dir().join("memory").join("MEMORY.md")]
        } else {
            Vec::new()
        }
    }
    /// skills 是 pi **原生**的，不像 MCP / 全局指令那样要装扩展 —— 它的加载器把
    /// `<agentDir>/skills` 和 `<cwd>/.pi/skills` 写成了默认档（`includeDefaults`），不看
    /// 任何 `packages` 声明。所以这里无条件给路径。
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(pi_agent_dir().join("skills"))
    }
    /// 依据是 pi 0.85.1 的 bundle：默认档是 `<agentDir>/skills`（source `user`）和
    /// `<cwd>/.pi/skills`（source `project`）；此外还单独加了一档 source `agents` 的
    /// `~/.agents/skills`，以及从 cwd 逐层走到 git root 的 `<dir>/.agents/skills`
    /// （`collectAncestorAgentsSkillDirs`，仅在项目被信任时启用）。
    ///
    /// **`~/.agents/skills` 那一档是漏不得的**：本机 18 个 skill 全在那儿，pi 读得到。
    fn skills_sources(&self, cwd: Option<&Path>) -> Vec<SkillSource> {
        let mut out = Vec::new();
        if let Some(cwd) = cwd {
            out.push(skill_source(
                cwd.join(".pi").join("skills"),
                ConfigScope::Project,
                ConfigOrigin::Own,
            ));
            for dir in cwd_to_repo_root(cwd) {
                out.push(skill_source(
                    dir.join(".agents").join("skills"),
                    ConfigScope::Project,
                    ConfigOrigin::Shared,
                ));
            }
        }
        out.push(skill_source(
            pi_agent_dir().join("skills"),
            ConfigScope::User,
            ConfigOrigin::Own,
        ));
        out.push(skill_source(
            home().join(".agents").join("skills"),
            ConfigScope::User,
            ConfigOrigin::Shared,
        ));
        out
    }
}

/// pi 的 agent 目录。`getAgentDir()` 的原话是 `$PI_AGENT_DIR` 优先，否则
/// `~/<CONFIG_DIR_NAME>/agent`，而 `CONFIG_DIR_NAME` 默认 `.pi`。
fn pi_agent_dir() -> PathBuf {
    std::env::var_os("PI_AGENT_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".pi").join("agent"))
}

/// Pi 读全局指令时认的文件名，**顺序即优先级**，取第一个存在的。原样抄自 0.85.1
/// bundle 的 `loadContextFileFromDir()`：
/// `["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"]`。
/// 同一个函数既用于项目目录也用于 `agentDir`，所以全局这份也吃这套候选。
const PI_MEMO_NAMES: &[&str] = &[
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];

/// Pi 现在实际读的那份全局指令。
///
/// 一个都不存在时报 `AGENTS.md` —— 面板拿这个路径当「新建」的落点，而 `AGENTS.md` 是
/// 这五个名字里唯一该由我们写出来的那个（`.override.` 是用来盖掉别的，大写那两个是
/// 兼容老写法）。
fn pi_global_memo() -> PathBuf {
    let dir = pi_agent_dir();
    PI_MEMO_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| dir.join("AGENTS.md"))
}

/// Pi 的扩展装在 `~/.pi/agent/settings.json` 的 `packages` 数组里。
fn pi_has_package(name: &str) -> bool {
    let path = home().join(".pi").join("agent").join("settings.json");
    let Ok(raw) = std::fs::read(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return false;
    };
    value
        .get("packages")
        .and_then(|p| p.as_array())
        .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(name)))
}

/// grok 还扫不扫某家的某个面。默认扫；`[compat.<vendor>] <surface> = false` 或环境变量
/// `GROK_<VENDOR>_<SURFACE>_ENABLED=0` 可以关掉。
///
/// `surface` 是 `mcps` / `skills` 这样的键 —— 它自己的 `05-configuration.md:385` 把每个面
/// 单独列了一行（`[compat.claude] skills = true` / `mcps = true`），**共用一个开关就是错的**：
/// 关掉 MCP 兼容不等于同时关掉 skills 兼容。
fn grok_compat_enabled(vendor: &str, surface: &str) -> bool {
    // 环境变量优先，和它文档里的表述一致。
    let env_key = format!(
        "GROK_{}_{}_ENABLED",
        vendor.to_uppercase(),
        surface.to_uppercase()
    );
    if let Some(raw) = std::env::var_os(&env_key) {
        let raw = raw.to_string_lossy().to_lowercase();
        if !raw.is_empty() {
            return !matches!(raw.as_str(), "0" | "false" | "no" | "off");
        }
    }
    let path = crate::agents::grok::grok_home().join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return true; // 没有配置文件就是默认值。
    };
    let Ok(doc) = text.parse::<toml_edit::Document>() else {
        return true; // 配置坏了不是我们该在这儿报的错，按默认走。
    };
    doc.get("compat")
        .and_then(|c| c.get(vendor))
        .and_then(|v| v.get(surface))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// opencode 的 `instructions` 允许追加额外的指令文件，条目是相对 config 目录的路径
/// 或 glob。这里只解析**字面路径**：glob 要展开得先定 cwd 和匹配语义，属于阶段 8，
/// 现在把带通配符的条目原样跳过，好过报一个不存在的路径。
fn opencode_extra_instructions() -> Vec<PathBuf> {
    let dir = opencode_config_dir();
    let Ok(raw) = std::fs::read(dir.join("opencode.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    value
        .get("instructions")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str())
                .filter(|entry| !entry.contains('*') && !entry.contains('?'))
                .map(|entry| {
                    let p = Path::new(entry);
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        dir.join(p)
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// opencode 的布尔开关环境变量。它用的是 Effect 的 `Config.boolean`，认
/// `true` / `yes` / `on` / `1`（大小写不敏感），其余一律当 false。
fn opencode_flag(key: &str) -> bool {
    std::env::var_os(key).is_some_and(|raw| {
        matches!(
            raw.to_string_lossy().trim().to_lowercase().as_str(),
            "true" | "yes" | "on" | "1"
        )
    })
}

/// `opencode.json` 的 `skills.paths`：显式登记的 skill 目录。相对路径相对 cwd 解析，
/// 但这里没有 cwd 语境，所以**只收绝对路径和 `~/` 开头的**——它自己的解析顺序也是
/// `~/` 先展开、再判 `isAbsolute`、最后才落到 cwd。`http(s)://` 条目是远端来源（走
/// `urls` 那条路），不是磁盘目录，跳过。
fn opencode_declared_skill_dirs() -> Vec<SkillSource> {
    let dir = opencode_config_dir();
    let Ok(raw) = std::fs::read(dir.join("opencode.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    value
        .get("skills")
        .and_then(|v| v.get("paths"))
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str())
                .filter(|entry| !entry.starts_with("http://") && !entry.starts_with("https://"))
                .filter_map(|entry| {
                    if let Some(rest) = entry.strip_prefix("~/") {
                        Some(home().join(rest))
                    } else if Path::new(entry).is_absolute() {
                        Some(PathBuf::from(entry))
                    } else {
                        None
                    }
                })
                .map(|path| skill_source(path, ConfigScope::User, ConfigOrigin::Own))
                .collect()
        })
        .unwrap_or_default()
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"))
}

fn opencode_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("opencode")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_a_surface() {
        for agent in AGENTS {
            assert!(surface(agent).is_ok(), "{agent} has no ToolSurface");
        }
        assert!(surface("nope").is_err());
    }

    #[test]
    fn kimi_answers_to_both_names() {
        // `agents::source()` 也接受这两个名字，两边必须一致。
        assert!(surface("kimi").is_ok());
        assert!(surface("kimicode").is_ok());
    }

    #[test]
    fn capabilities_match_the_measured_reality() {
        // 这张表是 1.2 / 1.3 实测结论的机器可读版本。哪天某家改了约定，
        // 这里会红 —— 比等 UI 上出现一个空面板要早得多。
        let expect = [
            //          agent        mcp   skills hooks  memo
            ("claude", true, true, true, true),
            ("codex", true, true, true, true),
            ("grok", true, true, true, true),
            ("kimicode", true, true, true, true),
            ("opencode", true, true, false, true),
            ("agy", true, true, true, true),
            // Pi 只有 MCP 那格跟着本机装没装扩展走 —— 写死的话，换一台没装
            // `pi-mcp-adapter` 的机器这条就会红。skills 和全局指令都是内建的。
            ("pi", pi_has_package("npm:pi-mcp-adapter"), true, false, true),
        ];
        for (agent, mcp, skills, hooks, memo) in expect {
            let caps = surface(agent).unwrap().capabilities();
            assert_eq!(caps.mcp, mcp, "{agent}.mcp");
            assert_eq!(caps.skills, skills, "{agent}.skills");
            assert_eq!(caps.hooks, hooks, "{agent}.hooks");
            assert_eq!(caps.global_memo, memo, "{agent}.globalMemo");
        }
    }

    /// 这三家的 skills 目录一度被记成「没有」，导致面板把它们的开关置灰。依据分别是：
    /// agy 1.2.1 自带定制文档的「Global Configuration: `~/.gemini/config/`」、opencode
    /// 1.18.30 文档表的「Global skills | `~/.config/opencode/skill(s)/`」、pi 0.85.1
    /// `getAgentDir()` + `includeDefaults` 那段默认档。写死在这儿，别再"研究"没了。
    #[test]
    fn every_agent_has_a_user_skills_dir() {
        for agent in AGENTS {
            let dir = surface(agent).unwrap().skills_dir();
            assert!(dir.is_some(), "{agent} 少了 skills_dir");
        }
        let ends_with = |agent: &str, tail: &str| {
            let dir = surface(agent).unwrap().skills_dir().unwrap();
            assert!(
                dir.ends_with(tail),
                "{agent}.skills_dir = {dir:?}，期望以 {tail} 结尾"
            );
        };
        // agy 的是 `.gemini/config/skills`，不是 `.gemini/skills` —— 差一层就扫不到。
        ends_with("agy", ".gemini/config/skills");
        ends_with("opencode", "opencode/skill");
        ends_with("pi", ".pi/agent/skills");
    }

    /// 格式必须跟着**文件**走，不能跟着 agent 走。
    ///
    /// 这条是补一个真出过的洞：codex 的 user 级配置是 `config.toml`，而它当时吃的是
    /// 默认实现（`mcpServers` 那种 JSON），于是整个 `[mcp_servers.*]` 被当成坏 JSON
    /// 报错，codex 的 server 一个都读不到。一家一家去记「谁是 TOML」迟早再漏一次，
    /// 所以按后缀整片校验。
    #[test]
    fn every_toml_source_is_parsed_as_toml_and_every_json_source_is_not() {
        let cwd = PathBuf::from("/tmp/some-project");
        for agent in AGENTS {
            for source in surface(agent).unwrap().mcp_sources(Some(&cwd)) {
                let path = source.path.to_ascii_lowercase();
                if path.ends_with(".toml") {
                    assert_eq!(
                        source.format,
                        McpFormat::TomlServers,
                        "{agent} 的 {} 是 TOML 文件，却按 {:?} 解析",
                        source.path,
                        source.format
                    );
                } else if path.ends_with(".json") {
                    assert_ne!(
                        source.format,
                        McpFormat::TomlServers,
                        "{agent} 的 {} 是 JSON 文件，却按 TOML 解析",
                        source.path
                    );
                }
            }
        }
    }

    /// codex 的项目级 `.codex/config.toml` 是漏不得的一档。
    ///
    /// 用户实测指出来的：本机 10 个仓库有这个文件，9 个写了 `[mcp_servers.*]`，本仓库
    /// 自己就是一个。只报 user 那档的话，codex 在项目里实际加载的 server 全部报不出来。
    #[test]
    fn codex_reads_the_project_level_config_toml_and_it_wins_over_the_user_one() {
        let cwd = PathBuf::from("/tmp/some-project");
        let sources = surface("codex").unwrap().mcp_sources(Some(&cwd));
        let project = sources
            .iter()
            .find(|s| s.scope == ConfigScope::Project)
            .expect("codex 少了项目级 MCP 来源");
        assert!(project.path.ends_with(".codex/config.toml"), "{}", project.path);
        assert_eq!(project.format, McpFormat::TomlServers);
        // 「Overridden by project config」—— 同名时项目那份赢。
        let user = sources
            .iter()
            .find(|s| s.scope == ConfigScope::User)
            .expect("codex 少了用户级 MCP 来源");
        assert!(project.precedence > user.precedence);
        // 只对受信任的仓库加载，而信任的第二道闸（trusted_hash）我们复现不了。
        assert!(project.conditional, "项目级那档必须标成条件生效");
        assert!(!project.writable, "项目级配置属于仓库，会被提交，不写它");
    }

    /// 可写的来源至多一条，而且必须是 user 级自有格式的那个。
    ///
    /// 多于一条的话「保存」就没有唯一落点；写到项目级会把用户的改动提交进仓库，
    /// 写到兼容来源则是改别家 agent 的配置。
    #[test]
    fn at_most_one_writable_source_per_agent_and_it_is_the_user_level_own_one() {
        let cwd = PathBuf::from("/tmp/some-project");
        for agent in AGENTS {
            let sources = surface(agent).unwrap().mcp_sources(Some(&cwd));
            let writable: Vec<_> = sources.iter().filter(|s| s.writable).collect();
            assert!(writable.len() <= 1, "{agent} 有多个可写的 MCP 来源");
            for s in writable {
                assert_eq!(s.scope, ConfigScope::User, "{agent} 的可写来源不是 user 级");
                assert_eq!(s.origin, ConfigOrigin::Own, "{agent} 的可写来源不是自有格式");
            }
        }
    }

    fn user_skill_paths(agent: &str) -> Vec<PathBuf> {
        surface(agent)
            .unwrap()
            .skills_sources(None)
            .into_iter()
            .map(|s| s.path)
            .collect()
    }

    /// 谁读 `~/.agents/skills`、谁不读，是这个面板最核心的一张表：面板拿它决定
    /// 「在公共目录放一份，等于给哪几家开了」。多报一家是骗用户，少报一家会把
    /// 用户明明能用的 skill 标成「读不到」。所以正反两边都钉死。
    ///
    /// 依据（2026-09 复核）：
    ///
    /// - **grok / opencode / pi**：各自的加载器里都有这一档，早前已实证。
    /// - **kimi**：二进制里 `USER_GENERIC_DIRS = [".agents/skills"]`，join `osHomeDir`。
    /// - **codex**：用户实测能识别；0.154.0 的二进制里也有 `.agents/skills` 这个字面量。
    /// - **claude**：只读自己的 `~/.claude/skills`（反过来是 opencode 去读它）。
    /// - **agy**：`.agents` 只出现在**工作区**四个别名里，user 级只有 `~/.gemini/config/skills`。
    #[test]
    fn exactly_the_right_agents_read_the_cross_agent_hub() {
        let agents_dir = home().join(".agents").join("skills");
        for agent in ["grok", "opencode", "pi", "kimicode", "codex"] {
            assert!(
                user_skill_paths(agent).contains(&agents_dir),
                "{agent} 漏了 ~/.agents/skills"
            );
        }
        for agent in ["claude", "agy"] {
            assert!(
                !user_skill_paths(agent).contains(&agents_dir),
                "{agent} 不读 ~/.agents/skills，不能报成读得到"
            );
        }
    }

    /// opencode 还默认读 `~/.claude/skills`（`OPENCODE_DISABLE_CLAUDE_CODE_SKILLS` 关掉）。
    #[test]
    fn opencode_also_reads_the_claude_store() {
        let claude_dir = home().join(".claude").join("skills");
        assert!(
            user_skill_paths("opencode").contains(&claude_dir),
            "opencode 漏了 ~/.claude/skills"
        );
    }

    /// kimi 的四张目录表里 project 级也有 `.agents/skills` 和 `.kimi-code/skills`。
    #[test]
    fn kimi_scans_both_project_level_skill_dirs() {
        let cwd = Path::new("/tmp/some-project");
        let paths: Vec<PathBuf> = surface("kimicode")
            .unwrap()
            .skills_sources(Some(cwd))
            .into_iter()
            .map(|s| s.path)
            .collect();
        for tail in [".kimi-code", ".agents"] {
            let want = cwd.join(tail).join("skills");
            assert!(paths.contains(&want), "kimi 漏了 {want:?}");
        }
    }

    /// agy 的工作区目录有四个别名，用户用哪个都生效。只认 `.agents` 会把另外三个
    /// 里的 skill 报成不存在。
    #[test]
    fn agy_scans_all_four_workspace_dir_aliases() {
        let cwd = Path::new("/tmp/some-project");
        let paths: Vec<PathBuf> = surface("agy")
            .unwrap()
            .skills_sources(Some(cwd))
            .into_iter()
            .map(|s| s.path)
            .collect();
        for name in AGY_WORKSPACE_DIRS {
            let want = cwd.join(name).join("skills");
            assert!(paths.contains(&want), "agy 漏了 {want:?}");
        }
    }

    #[test]
    fn grok_declares_that_it_also_reads_claudes_mcp_config() {
        // 漏了这条，用户会以为在 Claude 里加的 server 只影响 Claude。
        let sources = grok_sources(None);
        let claude = sources
            .iter()
            .find(|s| s.path.ends_with(".claude.json"))
            .expect("grok must declare that it reads ~/.claude.json");
        assert_eq!(claude.origin, ConfigOrigin::Compat);
        assert!(
            !claude.writable,
            "we must never write Claude's file as grok's config"
        );
    }

    #[test]
    fn project_scoped_sources_appear_only_when_a_cwd_is_known() {
        // 没有 cwd 就算不出项目级路径，与其猜一个不如不报。
        let (repo, cwd) = fake_repo("project-scope");
        for agent in ["claude", "grok", "kimicode"] {
            let s = surface(agent).unwrap();
            // 只能断言「没有项目级来源」—— Claude 的 local scope 也在 `~/.claude.json`
            // 里，跟 cwd 无关，照样该报出来。
            assert!(
                s.mcp_sources(None)
                    .iter()
                    .all(|x| x.scope != ConfigScope::Project),
                "{agent} leaked a project source without a cwd"
            );
            assert!(
                s.mcp_sources(Some(&cwd))
                    .iter()
                    .any(|x| x.scope == ConfigScope::Project),
                "{agent} must report its project-level MCP sources"
            );
        }
        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn a_project_that_is_not_a_git_repo_still_reports_its_project_mcp_sources() {
        // 用 git root 找项目配置的话，非 git 目录一条项目来源都报不出来，而 agent 照样
        // 会读那儿的 `.mcp.json`。项目根用 `project_root`：不是仓库就是 cwd 自己。
        let dir = std::env::temp_dir().join(format!(
            "csv-surface-norepo-mcp-{}-{}",
            std::process::id(),
            crate::util::now_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let expected = dir.join(".mcp.json").to_string_lossy().to_string();
        for agent in ["claude", "kimicode", "grok"] {
            assert!(
                surface(agent)
                    .unwrap()
                    .mcp_sources(Some(&dir))
                    .iter()
                    .any(|s| s.path == expected),
                "{agent} must report {expected} outside a git repo too"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `grok_compat_enabled` 读进程环境，而进程环境是全局的：改它的用例必须和所有
    /// 读 grok 来源的用例串起来跑，否则后者会偶发地看不到本该在的 compat 来源。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn grok_sources(cwd: Option<&Path>) -> Vec<McpSource> {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        surface("grok").unwrap().mcp_sources(cwd)
    }

    /// 造一个 `<root>/a/b` 形状的假仓库，返回 (root, 最深的那层)。
    fn fake_repo(tag: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "csv-surface-{}-{}-{}",
            tag,
            std::process::id(),
            crate::util::now_millis()
        ));
        let deep = root.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        (root, deep)
    }

    #[test]
    fn kimi_reports_all_three_of_its_mcp_scopes_in_the_documented_order() {
        // 依据：kimi 自带的 /mcp-config skill 正文 ——「later entries in this precedence
        // order override earlier ones」，列举顺序是 user → project-root → project-local，
        // 所以 project-local 最高、user 最低。
        let (root, deep) = fake_repo("kimi");
        let sources = surface("kimicode").unwrap().mcp_sources(Some(&deep));
        assert_eq!(sources.len(), 3, "user + project-local + project-root");

        // 用 scope + origin 定位，不能按路径子串 —— user 的 `~/.kimi-code/mcp.json`
        // 和 project-local 的 `<cwd>/.kimi-code/mcp.json` 后缀一模一样。
        let pick = |scope: ConfigScope, origin: ConfigOrigin| {
            sources
                .iter()
                .find(|s| s.scope == scope && s.origin == origin)
                .unwrap_or_else(|| panic!("missing {scope:?}/{origin:?} source"))
                .precedence
        };
        let local = pick(ConfigScope::Project, ConfigOrigin::Own);
        let repo_shared = pick(ConfigScope::Project, ConfigOrigin::Shared);
        let user = pick(ConfigScope::User, ConfigOrigin::Own);
        assert!(
            local > repo_shared,
            "project-local must outrank project-root"
        );
        assert!(
            repo_shared > user,
            "project-root must outrank the user file"
        );
        // project-root 的 .mcp.json 挂在 git root 上，不是 cwd 上。
        assert!(sources
            .iter()
            .any(|s| s.path == root.join(".mcp.json").to_string_lossy()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn grok_walks_every_level_from_cwd_up_to_the_repo_root() {
        // 依据：07-mcp-servers.md ——「walks from the current directory up to the git repo
        // root, loading .grok/config.toml at each level」，越深优先级越高。
        let (root, deep) = fake_repo("grok-walk");
        let sources = grok_sources(Some(&deep));

        let grok_levels: Vec<&McpSource> = sources
            .iter()
            .filter(|s| s.path.ends_with(".grok/config.toml") && s.scope == ConfigScope::Project)
            .collect();
        assert_eq!(
            grok_levels.len(),
            3,
            "root, root/a, root/a/b — one per level"
        );
        assert!(
            grok_levels[0].path.contains("/a/b/"),
            "the deepest level must come first: {:?}",
            grok_levels.iter().map(|s| &s.path).collect::<Vec<_>>()
        );
        assert!(
            grok_levels
                .windows(2)
                .all(|w| w[0].precedence > w[1].precedence),
            "deeper levels must outrank shallower ones"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn grok_ranks_its_own_config_above_the_vendors_it_merges() {
        // 同文档 :233 ——「config.toml > Claude > Cursor > .mcp.json」。
        let (root, deep) = fake_repo("grok-rank");
        let sources = grok_sources(Some(&deep));
        let p = |needle: &str| {
            sources
                .iter()
                .find(|s| s.path.contains(needle))
                .unwrap_or_else(|| panic!("missing {needle}"))
                .precedence
        };
        assert!(p(".grok/config.toml") > p(".claude.json"));
        assert!(p(".claude.json") > p(".cursor/mcp.json"));
        assert!(p(".cursor/mcp.json") > p("/.mcp.json"));

        // 项目级 Cursor 配置也要报，只报 home 那份会漏掉实际生效的 server。
        assert!(
            sources
                .iter()
                .any(|s| s.path == root.join(".cursor").join("mcp.json").to_string_lossy()),
            "the project-level Cursor config must be reported too"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn grok_stops_reporting_a_vendor_it_has_been_told_not_to_scan() {
        // 依据：07-mcp-servers.md:235 —— 环境变量可以关掉某家的兼容扫描。
        // 关了还报出来，用户就会以为 Claude 那边加的 server 在 grok 里也生效。
        let (root, deep) = fake_repo("grok-compat");
        let has_claude =
            |sources: &[McpSource]| sources.iter().any(|s| s.path.ends_with(".claude.json"));
        assert!(has_claude(&grok_sources(Some(&deep))));

        // 进程环境是全局的，改之前先拿锁，别的用例才不会读到这个临时值。
        let off = {
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: 单测里改进程环境；ENV_LOCK 保证此刻没有并发读者。
            unsafe { std::env::set_var("GROK_CLAUDE_MCPS_ENABLED", "0") };
            let off = surface("grok").unwrap().mcp_sources(Some(&deep));
            unsafe { std::env::remove_var("GROK_CLAUDE_MCPS_ENABLED") };
            off
        };

        assert!(
            !has_claude(&off),
            "a disabled vendor must not be reported as a live source"
        );
        // 自己的配置不受影响。
        assert!(off.iter().any(|s| s.path.ends_with(".grok/config.toml")));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn claude_reports_its_file_at_both_scopes_it_actually_spans() {
        // local 和 user 装在同一个 ~/.claude.json 里，而 local > project > user。
        // 只报一条的话，两种记法都会画出错误的覆盖关系。
        let (root, deep) = fake_repo("claude-scopes");
        let sources = surface("claude").unwrap().mcp_sources(Some(&deep));
        let config = home().join(".claude.json").to_string_lossy().to_string();

        let local = sources
            .iter()
            .find(|s| s.scope == ConfigScope::Local)
            .expect("the local scope must be reported");
        let user = sources
            .iter()
            .find(|s| s.scope == ConfigScope::User)
            .expect("the user scope must be reported");
        let project = sources
            .iter()
            .find(|s| s.scope == ConfigScope::Project)
            .expect("the project scope must be reported");

        assert_eq!(local.path, config);
        assert_eq!(user.path, config, "both scopes live in the same file");
        assert!(local.precedence > project.precedence, "local > project");
        assert!(project.precedence > user.precedence, "project > user");
        assert!(user.writable, "we write the user scope");
        assert!(
            !local.writable,
            "local scope is per-project, phase 6 handles it"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn opencode_reports_the_extra_instruction_files_it_was_configured_with() {
        // opencode 的 instructions 能追加额外指令文件；不报出来，全局配置面板显示的
        // 「生效内容」就和它实际读到的对不上。
        let extras = surface("opencode").unwrap().memo_extra_sources();
        // 本机没配 instructions，所以这里只能断言「解析得动、不炸、不编路径」。
        assert!(
            extras.iter().all(|p| p.is_absolute()),
            "relative entries must be resolved against the config dir: {extras:?}"
        );
        assert!(
            extras.iter().all(|p| !p.to_string_lossy().contains('*')),
            "glob entries must be skipped rather than reported as real paths"
        );
    }

    #[test]
    fn agys_global_memo_lives_under_its_global_customization_root() {
        // 这条一度写成「agy 没有 home 级全局约定」，面板对它整片禁用。它自带的定制文档
        // 把 `~/.gemini/config/` 列为全局定制根，根下的独立 `GEMINI.md` / `AGENTS.md`
        // 就是全局规则。
        let s = surface("agy").unwrap();
        assert!(s.capabilities().global_memo);
        let own = s.memo_path().expect("agy 的全局规则在全局定制根下");
        assert!(
            own.starts_with(agy_config_dir()),
            "{own:?} 不在 ~/.gemini/config 下"
        );
        assert!(
            AGY_MEMO_NAMES.contains(&own.file_name().unwrap().to_str().unwrap()),
            "{own:?} 不是 agy 认的规则文件名"
        );
        // 额外那份只会是另一个名字，且一定和约定路径不是同一个文件。
        for extra in s.memo_extra_sources() {
            assert_ne!(extra, own);
            assert!(AGY_MEMO_NAMES.contains(&extra.file_name().unwrap().to_str().unwrap()));
        }
    }

    #[test]
    fn only_opencode_pi_and_agy_declare_extra_instruction_sources() {
        // 「额外来源」是约定路径之外、这家确实还会读的那些：opencode 的 `instructions`
        // 数组、pi 的 `npm:pi-memory`、agy 全局根下的第二个规则文件名。别家凭空多报一个
        // 文件，面板就会说 agent 读了一份它根本没读的东西。
        for agent in AGENTS {
            // agy 只在 `GEMINI.md` 和 `AGENTS.md` 两份都在时才多报一份（另一份也会被
            // 读），所以它进不进这张表跟本机有什么文件有关，不能反过来断言它一定有。
            if matches!(*agent, "opencode" | "pi" | "agy") {
                continue;
            }
            assert!(
                surface(agent).unwrap().memo_extra_sources().is_empty(),
                "{agent} must not invent extra instruction files"
            );
        }
    }

    #[test]
    fn grok_marks_the_shared_mcp_json_as_conditional() {
        // 它取决于 Claude import marker，而那个 marker 在哪儿没有公开说明。判定不了
        // 就标成条件生效，别直接断言它在覆盖链里。
        let (root, deep) = fake_repo("grok-conditional");
        let sources = grok_sources(Some(&deep));
        let shared = sources
            .iter()
            .find(|s| s.origin == ConfigOrigin::Shared)
            .expect("the shared .mcp.json must be reported");
        assert!(
            shared.conditional,
            "grok's .mcp.json is not unconditionally loaded"
        );
        assert_eq!(
            sources.iter().filter(|s| s.conditional).count(),
            1,
            "only the shared .mcp.json is conditional — don't sprinkle the flag around"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sources_are_returned_highest_precedence_first() {
        let (root, deep) = fake_repo("order");
        for agent in AGENTS {
            let sources = surface(agent).unwrap().mcp_sources(Some(&deep));
            assert!(
                sources
                    .windows(2)
                    .all(|w| w[0].precedence >= w[1].precedence),
                "{agent} returned sources out of precedence order"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_cwd_outside_any_repo_does_not_walk_up_to_the_filesystem_root() {
        // 不在仓库里就只看 cwd 自己 —— 一路爬到 / 会报出一堆根本不存在的路径。
        let dir = std::env::temp_dir().join(format!(
            "csv-surface-norepo-{}-{}",
            std::process::id(),
            crate::util::now_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let levels = super::cwd_to_repo_root(&dir);
        assert_eq!(levels, vec![dir.clone()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exactly_one_writable_source_per_agent_that_supports_mcp() {
        // 「可写」是会真的落盘的那一个。多于一个就说明写入目标不确定。
        let (repo, cwd) = fake_repo("writable");
        for agent in AGENTS {
            let s = surface(agent).unwrap();
            let writable: Vec<_> = s
                .mcp_sources(Some(&cwd))
                .into_iter()
                .filter(|x| x.writable)
                .collect();
            assert_eq!(
                writable.len(),
                usize::from(s.capabilities().mcp),
                "{agent} writable-source count must match its mcp capability"
            );
            if let Some(w) = writable.first() {
                assert_eq!(
                    Some(w.path.clone()),
                    s.mcp_config_path().map(|p| p.to_string_lossy().to_string()),
                    "{agent}: the writable source must be mcp_config_path"
                );
            }
        }
        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn pis_global_memo_is_native_and_the_extension_only_adds_one() {
        // 这条一度写反：把 `npm:pi-memory` 的 MEMORY.md 当成了 Pi 的约定文件，于是没装
        // 扩展的机器被报成「Pi 没有全局指令」。Pi 自己无条件读 `<agentDir>/AGENTS.md`
        // （https://pi.dev/docs/latest/quickstart#give-pi-project-instructions）。
        let s = surface("pi").unwrap();
        assert!(s.capabilities().global_memo);
        let own = s.memo_path().expect("pi 的全局指令是内建的");
        assert!(own.starts_with(pi_agent_dir()), "{own:?} 不在 agent 目录下");
        assert!(
            PI_MEMO_NAMES.contains(&own.file_name().unwrap().to_str().unwrap()),
            "{own:?} 不是 pi 认的候选名"
        );
        // 扩展只往上加一份，加不加都不影响约定文件那份。
        let extra = s.memo_extra_sources();
        assert_eq!(extra.len(), usize::from(pi_has_package("npm:pi-memory")));
        assert!(extra
            .iter()
            .all(|p| p.ends_with("memory/MEMORY.md") && p != &own));
    }

    #[test]
    fn only_opencode_and_grok_fall_back_to_claudes_memo() {
        for agent in AGENTS {
            let has_fallback = surface(agent).unwrap().memo_fallback().is_some();
            let expected = matches!(*agent, "opencode" | "grok");
            assert_eq!(has_fallback, expected, "{agent} fallback mismatch");
        }
    }
}

#[cfg(test)]
mod installed_tests {
    use super::*;

    /// 七家都得报出配置目录，否则 `is_installed` 一律 false，面板上一个 agent 都不剩。
    #[test]
    fn every_agent_reports_a_config_home() {
        for agent in AGENTS {
            let s = surface(agent).expect("surface");
            assert!(
                s.config_home().is_some(),
                "{agent} 没有 config_home，装没装就判不出来"
            );
        }
    }

    /// 配置目录必须是**目录本身**，不是里面的某个文件 —— `is_installed` 用 `is_dir()` 判。
    #[test]
    fn config_home_is_a_directory_not_a_file() {
        for agent in AGENTS {
            let s = surface(agent).expect("surface");
            let home = s.config_home().unwrap();
            assert!(
                home.extension().is_none(),
                "{agent} 的 config_home 看着像个文件：{}",
                home.display()
            );
        }
    }

    /// 不存在的目录判成没装。
    #[test]
    fn missing_dir_counts_as_not_installed() {
        struct Nowhere;
        impl ToolSurface for Nowhere {
            fn config_home(&self) -> Option<PathBuf> {
                Some(PathBuf::from("/definitely/not/here/.nope"))
            }
        }
        assert!(!is_installed(&Nowhere));
    }

    /// 压根不报 config_home 的也判成没装，而不是 panic。
    #[test]
    fn no_config_home_counts_as_not_installed() {
        struct Silent;
        impl ToolSurface for Silent {}
        assert!(!is_installed(&Silent));
    }

    /// 存在的目录判成装了。
    #[test]
    fn existing_dir_counts_as_installed() {
        let dir = std::env::temp_dir().join("cv-installed-test");
        std::fs::create_dir_all(&dir).unwrap();
        struct Here(PathBuf);
        impl ToolSurface for Here {
            fn config_home(&self) -> Option<PathBuf> {
                Some(self.0.clone())
            }
        }
        assert!(is_installed(&Here(dir.clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
