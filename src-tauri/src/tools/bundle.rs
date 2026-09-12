//! 配置集导出（方案 阶段 10）。
//!
//! 把「一套 MCP + hooks + 全局配置 + 一份 skill 清单」打成一个可分享的 JSON。
//!
//! 这个模块只负责**导出**。导入那一半是"把包里的条目翻成 `McpEdit` / `HookEdit` /
//! 一次 memo 写入"，那三条写入路径已经各自有完整的校验、dry-run、备份和回读了 ——
//! 在这儿再实现一遍导入，等于把那些闸重新造一遍，而且是造第二份。翻译本身是纯函数，
//! 住在 `src/toolsBundle.ts`（能被单测覆盖）。
//!
//! # 值一律不导出
//!
//! MCP 的 `env` / `headers` **只导出键名，不导出值**，一个都不留。
//!
//! 不是"只抹看着像凭据的那几个"：`looks_secret()` 判的是键名，而键名是人随手起的
//! —— 一个叫 `GH` 的变量装的可能就是 token。配置集是拿去**发给别人**的东西，赌错一次
//! 的代价是一个凭据进了聊天记录。键名全留着，导入端一眼能看出要自己补哪几个。
//!
//! `args` 里也可能夹着凭据（`--api-key=…`），但 args 同时又是这个 server 的身份，
//! 整条抹掉包就没用了。所以对 args 做**按形状**的遮蔽：`--x=y` 这种带值的参数，
//! 键名像凭据就把值换掉，其余原样。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use super::mcp::McpTransport;
use super::{hooks, mcp, memo, skills, ConfigScope};

/// 包的类型标记。认不出来的一律拒绝 —— 这是要喂给写入路径的东西。
pub const BUNDLE_KIND: &str = "cc-sessions-viewer/tool-bundle";
pub const BUNDLE_VERSION: u32 = 1;

/// 被抹掉的值在界面上显示成什么。不是空串 —— 空串和"这个变量本来就是空的"分不开。
pub const REDACTED: &str = "<redacted>";

/// 导出哪几类。全关也允许：用户可能只想分享 hooks。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleInclude {
    pub mcp: bool,
    pub hooks: bool,
    pub memo: bool,
    pub skills: bool,
}

impl Default for BundleInclude {
    fn default() -> Self {
        Self {
            mcp: true,
            hooks: true,
            memo: true,
            skills: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BundleMcp {
    pub name: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    /// 只有键名。值一个都不带 —— 见模块头。
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    /// 导出那台机器上的工作目录。导入端多半要改，所以计划框里会把它摊出来。
    pub cwd: Option<String>,
    /// 导出时这几家在跑它。导入端可以改投给别家。
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BundleHook {
    pub command: String,
    pub events: Vec<String>,
    pub matcher: Option<String>,
    pub timeout: Option<i64>,
    pub agents: Vec<String>,
}

/// 一份全局指令文件。
///
/// 按**角色**记，不按路径：`~/.claude/CLAUDE.md` 在导入端该落到那台机器的 claude 约定
/// 位置上，而不是原样拿绝对路径去写（那台机器的用户名都不一样）。被 `@` 引用进来的
/// 片段没有角色，靠 `name` 落在同一个目录里。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BundleMemo {
    /// 这是哪家的约定文件。`None` = 被引用进来的片段。
    pub agent: Option<String>,
    /// 片段是被**哪家**的约定文件 `@` 进来的。导入端照这一条把它放进那家的目录里
    /// —— 只有 `name` 的话根本不知道该往哪写（七家的 home 各不相同）。
    ///
    /// 顶层文件为 `None`。`@import` 只展开一层（阶段 8），所以这条链最长就一跳。
    pub parent: Option<String>,
    pub name: String,
    pub text: String,
}

/// 一条 skill **清单**，不含内容。
///
/// 不打包 skill 的文件：一个 skill 就是一段会被 agent 执行的指令，从别人发来的包里
/// 一键铺开等于运行来路不明的代码（方案七里"一键安装任意 npm 包"不做，同一条理由）。
/// 有 git 来源的把来源记下来，导入端自己决定要不要 clone。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BundleSkill {
    pub name: String,
    pub remote: Option<String>,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bundle {
    /// 固定串。导入时第一件事就是核它。
    pub kind: String,
    pub version: u32,
    pub created_at: i64,
    /// 导出时的 app 版本。格式往后演进时这一条是唯一的线索。
    pub app: String,
    pub mcp: Vec<BundleMcp>,
    pub hooks: Vec<BundleHook>,
    pub memo: Vec<BundleMemo>,
    pub skills: Vec<BundleSkill>,
    /// 哪些位置的值被抹掉了，逐条列出来（`github.env.GITHUB_TOKEN` 这样）。
    ///
    /// 导入端照这张单子提示用户补。没有它的话，一个 server 起不来时用户只能从
    /// "为什么连不上"一路倒查回"哦原来 token 没了"。
    pub redacted: Vec<String>,
}

/// `--key=value` 里的 key 像凭据就把值换掉，其余原样返回。
///
/// 只认 `=` 形式：`--api-key abc` 那种分成两个词的，抹掉后一个词要靠猜（后一个词也
/// 可能是下一个 flag），猜错就把包改坏了。分开写的那种交给 `redacted` 那张单子提醒人。
fn redact_arg(arg: &str) -> Option<String> {
    let (key, _) = arg.split_once('=')?;
    // `looks_secret` 的针是按**环境变量**的写法排的（`API_KEY`），而命令行 flag 一律
    // 写成 `--api-key`。不换这一刀的话 `--api-key=sk-…` 一条都抹不掉 —— 单测先红的。
    let name = key.trim_start_matches('-').replace('-', "_");
    if name.is_empty() || !mcp::looks_secret(&name) {
        return None;
    }
    Some(format!("{key}={REDACTED}"))
}

fn export_mcp(cwd: Option<&Path>, redacted: &mut Vec<String>) -> Vec<BundleMcp> {
    let scan = mcp::scan(cwd);
    let mut out = Vec::new();
    for entry in &scan.servers {
        // 一个 server 可能在好几个文件里各有一份定义。导出**user 级那份**：
        //
        // - 别的定义在导出这台机器上都不算数，带出去只会让导入端多出几条互相矛盾的；
        // - 而 user 级这一档是唯一跟着**人**走的。项目级（含 claude 的 local 级，
        //   它也存在 `~/.claude.json` 里但只对某个项目生效）跟着**仓库**走，仓库自己
        //   会被 clone 过去。把它写进包里，导入端会把它装成 user 级 —— 一条本来只
        //   在一个项目里生效的配置，到了那台机器上对所有项目生效。
        //
        // 关掉的那份也不导出。`McpServerInput` 没有 `enabled` 字段（`mcp_write.rs`），
        // 导入端装出来的一律是开着的 —— 把一条用户明确关掉的 server 带过去再自动打开，
        // 比不带过去坏得多。
        //
        // `defs` 在同一家里是按优先级降序排的，所以 `find` 命中的就是该家 user 级里
        // 优先级最高的那份；没有东西盖它的时候，它同时也就是生效的那份。
        let mine: Vec<_> = entry
            .defs
            .iter()
            .filter(|d| d.source.scope == ConfigScope::User && d.def.enabled)
            .collect();
        let Some(at) = mine.first() else { continue };
        let def = &at.def;
        // 角标那排是「真的会加载它的 agent」，里面可能有几家是靠项目级定义才加载上的，
        // 也可能少了几家（user 级那份被项目级盖住了，但它照样跟着人走）。所以这里
        // 自己数一遍，不抄 `entry.agents`。
        let mut agents: Vec<String> = Vec::new();
        for d in &mine {
            if !agents.contains(&d.agent) {
                agents.push(d.agent.clone());
            }
        }
        let mut args = Vec::with_capacity(def.args.len());
        for (i, arg) in def.args.iter().enumerate() {
            match redact_arg(arg) {
                Some(masked) => {
                    redacted.push(format!("{}.args[{i}]", entry.name));
                    args.push(masked);
                }
                None => args.push(arg.clone()),
            }
        }
        for v in &def.env {
            redacted.push(format!("{}.env.{}", entry.name, v.key));
        }
        for v in &def.headers {
            redacted.push(format!("{}.headers.{}", entry.name, v.key));
        }
        out.push(BundleMcp {
            name: entry.name.clone(),
            transport: def.transport,
            command: def.command.clone(),
            args,
            url: def.url.clone(),
            env_keys: def.env.iter().map(|v| v.key.clone()).collect(),
            header_keys: def.headers.iter().map(|v| v.key.clone()).collect(),
            cwd: def.cwd.clone(),
            agents,
        });
    }
    out
}

/// 扫描按**命令**归并（`HookEntry`），而一条命令挂在不同事件上完全可以配着不同的
/// 匹配器和超时。所以导出时要按 `(matcher, timeout)` 再分一次组。
///
/// 本机就有反例：`codex-skill-usage.sh` 在 `PostToolUse` 上带着
/// `^(Bash|mcp__.*)$`，在 `Stop` 和 `UserPromptSubmit` 上不带匹配器。并成一条、
/// 只留第一个匹配器的话，导入端会把那个正则盖到 `Stop` 上去 —— 那是另一个 hook，
/// 而且它永远不会触发（`Stop` 根本没有工具名可匹配）。
fn export_hooks(cwd: Option<&Path>) -> Vec<BundleHook> {
    /// 一条命令在同一个 `(matcher, timeout)` 下挂到了哪些事件、哪几家。
    #[derive(Default)]
    struct Group {
        events: Vec<String>,
        agents: Vec<String>,
    }
    /// 分组的键：匹配器 + 超时。这两个一样的才能合成一条 `BundleHook`。
    type Shape = (Option<String>, Option<i64>);

    let scan = hooks::scan(cwd);
    let mut out = Vec::new();
    // 回合信号是本 app 自己装的，跟着安装走不跟着配置走。带进包里，导入端会在
    // 自己那份之外再装一条一模一样的，于是每个回合触发两次。
    for entry in scan.hooks.iter().filter(|h| !h.managed) {
        let mut groups: BTreeMap<Shape, Group> = BTreeMap::new();
        for at in &entry.hooks {
            // `prompt` / `url` 型的那串字不是命令，各家的字段形状也不一样，翻不成
            // 一条 `HookEdit`。写进包里而导入端装不上，比不写更难查。
            if at.def.kind != "command" {
                continue;
            }
            // 项目级的落点跟着**仓库**走，不跟着人走。导入端只会往 user 级写，于是
            // 一条本来只在一个项目里跑的 hook 会变成对所有项目都跑 —— 而它的命令里
            // 十有八九还写着导出那台机器上的项目路径（本机 sales-app 那条
            // `check-test-ids-post-write.sh` 就是）。
            if at.source.scope != ConfigScope::User {
                continue;
            }
            // 关掉的那条也不导出，同 MCP：`HookEdit` 没有 `enabled` 字段，导入端
            // 装出来的一律是开着的。
            if !at.def.enabled {
                continue;
            }
            let g = groups
                .entry((at.def.matcher.clone(), at.def.timeout))
                .or_default();
            if !g.events.contains(&at.def.event) {
                g.events.push(at.def.event.clone());
            }
            if !g.agents.contains(&at.agent) {
                g.agents.push(at.agent.clone());
            }
        }
        for ((matcher, timeout), g) in groups {
            out.push(BundleHook {
                command: entry.command.clone(),
                events: g.events,
                matcher,
                timeout,
                agents: g.agents,
            });
        }
    }
    out
}

fn export_memo() -> Vec<BundleMemo> {
    let scan = memo::scan();
    let mut out = Vec::new();
    for file in &scan.files {
        if !file.exists || file.error.is_some() {
            continue;
        }
        let Ok((text, _)) = memo::read(Path::new(&file.path)) else {
            continue;
        };
        // 这个文件是哪家的约定位置。是回退目标 / 被引用的片段时没有角色。
        let role_of = |path: &str| {
            scan.agents
                .iter()
                .find(|a| a.path.as_deref() == Some(path))
                .map(|a| a.agent.clone())
        };
        let agent = role_of(&file.path);
        let parent = file.imported_by.as_deref().and_then(role_of);
        out.push(BundleMemo {
            agent,
            parent,
            name: file.name.clone(),
            text,
        });
    }
    out
}

fn export_skills(cwd: Option<&Path>) -> Vec<BundleSkill> {
    let scan = skills::scan(cwd, &[]);
    scan.skills
        .iter()
        .map(|s| BundleSkill {
            name: s.name.clone(),
            remote: s.git.as_ref().map(|g| g.remote.clone()),
            agents: s
                .refs
                .iter()
                .flat_map(|r| r.agents.iter().cloned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        })
        .collect()
}

pub fn export(cwd: Option<&Path>, include: BundleInclude) -> Bundle {
    let mut redacted = Vec::new();
    let mcp = if include.mcp {
        export_mcp(cwd, &mut redacted)
    } else {
        Vec::new()
    };
    Bundle {
        kind: BUNDLE_KIND.to_string(),
        version: BUNDLE_VERSION,
        created_at: crate::util::now_millis() as i64,
        app: env!("CARGO_PKG_VERSION").to_string(),
        mcp,
        hooks: if include.hooks {
            export_hooks(cwd)
        } else {
            Vec::new()
        },
        memo: if include.memo { export_memo() } else { Vec::new() },
        skills: if include.skills {
            export_skills(cwd)
        } else {
            Vec::new()
        },
        redacted,
    }
}

#[tauri::command(async)]
pub fn tools_export_bundle(cwd: Option<String>, include: BundleInclude) -> Bundle {
    export(cwd.as_deref().map(Path::new), include)
}

/// 一个包最大多少。挑错文件（一个 500 MB 的日志）时要在读之前就停，而不是让
/// `JSON.parse` 在渲染进程里把内存吃光。
const MAX_BUNDLE_BYTES: u64 = 8 * 1024 * 1024;

/// 把用户在文件对话框里挑中的那个文件读成文本。
///
/// **只读原文，不解析。** 认不认、哪几条能装，全在 `src/toolsBundle.ts` 里判 ——
/// 那是这一阶段唯一会造成破坏的判断，必须住在测得到的地方。
#[tauri::command(async)]
pub fn tools_read_bundle(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    let meta = std::fs::metadata(p).map_err(|e| format!("Failed to open {path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path} is not a file"));
    }
    if meta.len() > MAX_BUNDLE_BYTES {
        return Err(format!(
            "{path} is {} bytes, larger than the {MAX_BUNDLE_BYTES} byte limit",
            meta.len()
        ));
    }
    std::fs::read_to_string(p).map_err(|e| format!("Failed to read {path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_shaped_arg_loses_its_value() {
        assert_eq!(
            redact_arg("--api-key=sk-abc123").as_deref(),
            Some("--api-key=<redacted>")
        );
        assert_eq!(
            redact_arg("--auth-token=xyz").as_deref(),
            Some("--auth-token=<redacted>")
        );
    }

    /// 不带 `=` 的不碰。`--api-key abc` 里该抹的是**下一个词**，而下一个词也可能是
    /// 下一个 flag —— 猜错就把包改坏了。这种交给 `redacted` 单子提醒人。
    #[test]
    fn a_split_argument_is_left_alone() {
        assert_eq!(redact_arg("--api-key"), None);
        assert_eq!(redact_arg("abc"), None);
    }

    #[test]
    fn ordinary_arguments_survive_untouched() {
        assert_eq!(redact_arg("-y"), None);
        assert_eq!(redact_arg("@modelcontextprotocol/server-github"), None);
        // 这条最要紧：args 是这个 server 的身份，整条抹掉包就没用了。
        assert_eq!(redact_arg("--port=8080"), None);
        assert_eq!(redact_arg("--transport=stdio"), None);
    }

    /// 一个裸的 `=` 开头不能被当成"键名为空"而抹掉。
    #[test]
    fn an_argument_with_an_empty_key_is_left_alone() {
        assert_eq!(redact_arg("=value"), None);
        assert_eq!(redact_arg("--=value"), None);
    }

    /// 导出的是真实机器状态，所以这儿只断言**不变量**：类型标记、版本、以及
    /// "凡是 env 键名都进了 redacted 单子"。
    #[test]
    fn an_export_stamps_its_kind_and_lists_every_redaction() {
        let b = export(None, BundleInclude::default());
        assert_eq!(b.kind, BUNDLE_KIND);
        assert_eq!(b.version, BUNDLE_VERSION);
        for s in &b.mcp {
            for key in &s.env_keys {
                assert!(
                    b.redacted.contains(&format!("{}.env.{key}", s.name)),
                    "{}.env.{key} is not listed as redacted",
                    s.name
                );
            }
            for key in &s.header_keys {
                assert!(b
                    .redacted
                    .contains(&format!("{}.headers.{key}", s.name)));
            }
        }
    }

    /// 关掉某一类就真的一条都不带。
    #[test]
    fn turning_a_category_off_leaves_it_empty() {
        let b = export(
            None,
            BundleInclude {
                mcp: false,
                hooks: false,
                memo: false,
                skills: false,
            },
        );
        assert!(b.mcp.is_empty());
        assert!(b.hooks.is_empty());
        assert!(b.memo.is_empty());
        assert!(b.skills.is_empty());
        // 没有 MCP 就没有要抹的东西。
        assert!(b.redacted.is_empty());
    }

    /// 把一段 JSON 里所有字符串叶子摊平。
    fn string_leaves(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| string_leaves(x, out)),
            serde_json::Value::Object(o) => o.values().for_each(|x| string_leaves(x, out)),
            _ => {}
        }
    }

    /// 包里**一个值都不能有**。
    ///
    /// 这条是这个模块存在的全部意义：配置集是拿去发给别人的，漏一个 token 的代价
    /// 是它进了聊天记录。所以不信任类型形状，直接拿本机真实的 env 值去搜序列化结果。
    ///
    /// 两处必须让开，否则这条在真机上是红的，而红得没有道理：
    ///
    /// 1. **命令本身**。本机的 `node_repl` 是 `…/cua_node/bin/node_repl`，而它的
    ///    `NODE_REPL_NODE_PATH` 是 `…/cua_node/bin/node` —— 前者**包含**后者。命令、
    ///    url、cwd、args 是这个 server 的身份，本来就要导出。所以放行的是"整个叶子
    ///    恰好就是这几个字段之一"，不是"包含"：真出了 `format!("{k}={v}")` 那种漏，
    ///    叶子和原字段不相等，照样抓得到。
    /// 2. **全局配置的正文**。`CODEX_HOME` 的值是 `~/.codex`，而 `~/.codex/AGENTS.md`
    ///    里就写着一行 `@/Users/…/.codex/RTK.md`。正文是照磁盘逐字抄的，env 值不可能
    ///    从那条路进去。所以搜的范围就是 MCP 那一段加 `redacted` 单子。
    #[test]
    fn no_environment_value_survives_serialisation() {
        let b = export(None, BundleInclude::default());
        let live = mcp::scan(None);

        let mut leaves = Vec::new();
        string_leaves(&serde_json::to_value(&b.mcp).unwrap(), &mut leaves);
        string_leaves(&serde_json::to_value(&b.redacted).unwrap(), &mut leaves);

        let carriers: std::collections::HashSet<&str> = b
            .mcp
            .iter()
            .flat_map(|s| {
                s.command
                    .iter()
                    .chain(s.url.iter())
                    .chain(s.cwd.iter())
                    .chain(s.args.iter())
            })
            .map(String::as_str)
            .collect();

        let mut checked = 0;
        for entry in &live.servers {
            for at in &entry.defs {
                for v in at.def.env.iter().chain(at.def.headers.iter()) {
                    // 空值和极短的值搜出来全是误报（"1"、"true" 在 JSON 里到处都是）。
                    if v.value.len() < 8 {
                        continue;
                    }
                    for leaf in &leaves {
                        assert!(
                            !leaf.contains(&v.value) || carriers.contains(leaf.as_str()),
                            "the value of {} leaked into the bundle",
                            v.key
                        );
                    }
                    checked += 1;
                }
            }
        }
        // 本机一个都没有时这条测试什么也没证明 —— 说出来，别让它假装通过了。
        println!("checked {checked} live env/header values");
    }

    /// 每一份全局指令都得说得出自己该落在导入端的哪儿。
    ///
    /// 两种身份互斥：要么它**是**某家的约定文件（`agent`），要么它是被某家 `@` 进来的
    /// 片段（`parent`）。两个都没有的那种落不了地 —— 前端会把它单列成"放不下"，
    /// 而不是猜一个目录写进去。
    #[test]
    fn every_memo_entry_says_whose_it_is() {
        let b = export(None, BundleInclude::default());
        let mut fragments = 0;
        for m in &b.memo {
            assert!(
                m.agent.is_none() || m.parent.is_none(),
                "{} claims to be both an agent file and a fragment",
                m.name
            );
            if let Some(a) = m.agent.as_deref().or(m.parent.as_deref()) {
                assert!(crate::tools::AGENTS.contains(&a), "unknown agent role {a}");
            }
            if m.parent.is_some() {
                fragments += 1;
            }
        }
        println!("{fragments} imported fragments carried a parent role");
    }

    /// 三个常量在 Rust 和 TS 两边各写了一份，得钉住它们一致。
    ///
    /// 漂了之后的坏法很安静：`BUNDLE_VERSION` 这边先加到 2，前端还认到 1，于是本机
    /// 导出的每一个包在本机都读不回来，报的是"这个包比本机新"——而两边其实是同一个
    /// 版本的 app。`hooks_write.rs` 那条 `include_str!` 自查是同一个套路。
    #[test]
    fn the_two_sides_agree_on_the_wire_constants() {
        let ts = include_str!("../../../src/toolsBundle.ts");
        for expect in [
            format!("export const BUNDLE_KIND = '{BUNDLE_KIND}'"),
            format!("export const BUNDLE_VERSION = {BUNDLE_VERSION}"),
            format!("export const BUNDLE_REDACTED = '{REDACTED}'"),
        ] {
            assert!(ts.contains(&expect), "src/toolsBundle.ts is missing `{expect}`");
        }
    }

    /// 包里的每一条 hook 都必须**在本机真的这么配着**。
    ///
    /// 这条钉的是"按命令归并"和"按事件配置"之间那道缝：扫描把同一条命令的所有落点
    /// 并成一个 `HookEntry`，而匹配器和超时是**每个事件各自**的。早先的实现取
    /// `hooks.first()` 的匹配器发给全部事件，本机的 `codex-skill-usage.sh` 当场
    /// 中招 —— `PostToolUse` 上那个 `^(Bash|mcp__.*)$` 被盖到了 `Stop` 头上。
    #[test]
    fn every_exported_hook_matches_a_real_landing() {
        let b = export(None, BundleInclude::default());
        let live = hooks::scan(None);
        let mut checked = 0;
        for h in &b.hooks {
            for event in &h.events {
                let real = live.hooks.iter().any(|entry| {
                    entry.command == h.command
                        && entry.hooks.iter().any(|at| {
                            at.def.event == *event
                                && at.def.matcher == h.matcher
                                && at.def.timeout == h.timeout
                        })
                });
                assert!(
                    real,
                    "{}/{event} was exported with matcher {:?} / timeout {:?}, \
                     but nothing on this machine is configured that way",
                    h.command, h.matcher, h.timeout
                );
                checked += 1;
            }
        }
        println!("checked {checked} exported (command, event, matcher, timeout) landings");
    }

    /// 包里只装**跟着人走**的那一档。
    ///
    /// 导入端只会往各家的 user 级文件里写。项目级（含 claude 的 local 级）的配置跟着
    /// 仓库走，而仓库自己会被 clone 过去；把它塞进包里，到那台机器上就变成了对所有
    /// 项目生效 —— 而它的命令里往往还写死着导出这台机器上的项目路径。
    ///
    /// 断言写成「给不给 cwd，导出的 MCP 和 hooks 一模一样」：user 级的来源全是
    /// home 下的固定路径，一条都不看 cwd（`hook_sources` / `mcp_sources` 里那几个
    /// `ConfigScope::User` 分支都是），所以过滤对了，这两次导出就必须逐字节相等。
    /// 这条不挑机器 —— 换台机器上有别的项目配置，它照样成立。
    #[test]
    fn only_the_tier_that_travels_with_the_person_gets_packed() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let without = export(None, BundleInclude::default());
        let with = export(Some(here), BundleInclude::default());
        assert_eq!(
            with.mcp, without.mcp,
            "a project-scoped MCP server leaked into the bundle"
        );
        assert_eq!(
            with.hooks, without.hooks,
            "a project-scoped hook leaked into the bundle"
        );
    }

    /// 关掉的那份不进包。
    ///
    /// `McpServerInput` / `HookEdit` 都没有 `enabled` 字段（看 `mcp_write.rs` 和
    /// `hooks_write.rs`），导入端装出来的一律是开着的。把一条用户明确关掉的东西带
    /// 过去、再在那台机器上自动打开，比不带过去坏得多 —— 尤其 hook 是会**跑命令**的。
    #[test]
    fn what_the_user_switched_off_stays_off() {
        let b = export(None, BundleInclude::default());
        let live_mcp = mcp::scan(None);
        let live_hooks = hooks::scan(None);

        for s in &b.mcp {
            let entry = live_mcp
                .servers
                .iter()
                .find(|e| e.name == s.name)
                .expect("exported a server that is not on this machine");
            for agent in &s.agents {
                assert!(
                    entry.defs.iter().any(|d| {
                        &d.agent == agent
                            && d.source.scope == ConfigScope::User
                            && d.def.enabled
                    }),
                    "{}/{agent} was exported, but its user-level definition is switched off",
                    s.name
                );
            }
        }

        for h in &b.hooks {
            for event in &h.events {
                assert!(
                    live_hooks.hooks.iter().any(|entry| {
                        entry.command == h.command
                            && entry.hooks.iter().any(|at| {
                                at.def.event == *event
                                    && at.source.scope == ConfigScope::User
                                    && at.def.enabled
                            })
                    }),
                    "{}/{event} was exported, but nothing user-level and switched on matches it",
                    h.command
                );
            }
        }
    }

    /// 回合信号不进包：导入端自己装了一条，再收一条一模一样的，每个回合会触发两次。
    #[test]
    fn the_turn_signal_hook_is_never_exported() {
        let b = export(None, BundleInclude::default());
        let live = hooks::scan(None);
        for managed in live.hooks.iter().filter(|h| h.managed) {
            assert!(
                !b.hooks.iter().any(|h| h.command == managed.command),
                "the managed turn-signal hook leaked into the bundle"
            );
        }
    }
}
