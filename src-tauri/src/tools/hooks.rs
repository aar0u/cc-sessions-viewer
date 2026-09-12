//! Hook 配置的归一化模型 + 全机器扫描。
//!
//! 和 [`super::mcp`] 同一套骨架（格式挂在来源上、这里一行 agent 判断都没有），但有
//! **两处关键的形状差别**，照搬 MCP 那套会错：
//!
//! 1. **hook 是叠加的，不是覆盖的。** 同一个事件在 user 级和项目级各配一条，两条都会
//!    跑。所以 [`HookSource`](super::HookSource) 没有 `precedence`，这里也没有
//!    「生效 / 被盖住」这组概念。
//! 2. **一条 hook 没有名字。** MCP 的 server 有 `name` 可以归并，hook 只有一串命令。
//!    归并按**命令指纹**做，列表上显示的就是命令本身 —— 不去从命令里猜一个名字出来
//!    （`bash x.sh` 猜得出来，`[ -n "$X" ] && … || echo '{}'` 猜出来的是 `command`）。

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{surface, HookFormat, HookSource, AGENTS};

/// 一条 hook 在某一个文件里的定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookDef {
    pub event: String,
    /// 匹配器（工具名 / 通知类型）。`None` = 这个事件的全部都匹配。
    pub matcher: Option<String>,
    /// agy 比别家多的那一层：它的 `hooks.json` 根上是**一个个有名字的 hook**，名字
    /// 底下才是事件（本 app 装的回合信号就叫 `cc-sessions-viewer-turn-status`）。
    /// 别家没有这一层，恒为 `None`。
    pub group: Option<String>,
    /// `command` / `prompt` / …。认不出来的原样保留。
    pub kind: String,
    pub command: String,
    /// 秒。各家单位不统一（codex 的 `hooks.json` 里躺着 `120000`），所以**原样报**，
    /// 不换算 —— 猜单位比不显示更糟。
    pub timeout: Option<i64>,
    /// 显式关掉的（codex TOML 的 `enabled = false`）。
    pub enabled: bool,
}

impl HookDef {
    /// 归并用的指纹：**跑的是同一条命令**。
    ///
    /// 只看命令，不看事件也不看 agent —— 本 app 的回合信号在 5 家 agent × 最多 6 个
    /// 事件上装了十几条，它们是**一件事**，列成十几行就没法看了。只做去空白，不做
    /// 「聪明」归一化。
    pub fn fingerprint(&self) -> String {
        self.command.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// 一个来源文件的解析结果。解析失败**不吞**，理由同 MCP。
#[derive(Debug, Clone, Default)]
pub struct SourceRead {
    pub hooks: Vec<HookDef>,
    pub error: Option<String>,
}

pub fn read_source(source: &HookSource) -> SourceRead {
    let path = PathBuf::from(&source.path);
    if !path.is_file() {
        return SourceRead::default();
    }
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            return SourceRead {
                hooks: Vec::new(),
                error: Some(format!("读不了：{e}")),
            }
        }
    };
    match source.format {
        HookFormat::GroupedJson => grouped_json(&raw),
        HookFormat::AgyJson => agy_json(&raw),
        HookFormat::TomlGrouped => toml_grouped(&raw),
        HookFormat::TomlList => toml_list(&raw),
    }
}

// ---------------------------------------------------------------------------
// 四种格式
// ---------------------------------------------------------------------------

fn json_root(raw: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str::<serde_json::Value>(raw).map_err(|e| format!("不是合法的 JSON：{e}"))
}

fn json_err(e: String) -> SourceRead {
    SourceRead {
        hooks: Vec::new(),
        error: Some(e),
    }
}

/// `hooks: { <Event>: [ { matcher?, hooks: [ … ] } ] }` —— claude / codex。
fn grouped_json(raw: &str) -> SourceRead {
    let root = match json_root(raw) {
        Ok(v) => v,
        Err(e) => return json_err(e),
    };
    let Some(events) = root.get("hooks").and_then(|v| v.as_object()) else {
        return SourceRead::default();
    };
    let mut hooks = Vec::new();
    for (event, groups) in events {
        let Some(groups) = groups.as_array() else { continue };
        for group in groups {
            let matcher = json_text(group.get("matcher"));
            let Some(items) = group.get("hooks").and_then(|v| v.as_array()) else {
                continue;
            };
            for item in items {
                hooks.push(json_hook(event, matcher.clone(), None, item));
            }
        }
    }
    SourceRead { hooks, error: None }
}

/// `{ <hook 名>: { <Event>: [ … ] } }` —— agy 的根上是一个个有名字的 hook。
fn agy_json(raw: &str) -> SourceRead {
    let root = match json_root(raw) {
        Ok(v) => v,
        Err(e) => return json_err(e),
    };
    let Some(named) = root.as_object() else {
        return SourceRead::default();
    };
    let mut hooks = Vec::new();
    for (group, events) in named {
        let Some(events) = events.as_object() else { continue };
        for (event, items) in events {
            let Some(items) = items.as_array() else { continue };
            for item in items {
                hooks.push(json_hook(
                    event,
                    json_text(item.get("matcher")),
                    Some(group.clone()),
                    item,
                ));
            }
        }
    }
    SourceRead { hooks, error: None }
}

fn json_hook(
    event: &str,
    matcher: Option<String>,
    group: Option<String>,
    item: &serde_json::Value,
) -> HookDef {
    HookDef {
        event: event.to_string(),
        matcher,
        group,
        kind: item
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("command")
            .to_string(),
        // `command` 之外还认 `prompt` / `url`：别家有这两种 handler，落到「空命令」上
        // 会被画成一条什么都不做的 hook。
        command: ["command", "prompt", "url"]
            .iter()
            .find_map(|key| json_text(item.get(*key)))
            .unwrap_or_default(),
        timeout: item.get("timeout").and_then(|v| v.as_i64()),
        enabled: item.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
    }
}

fn json_text(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(|v| v.as_str()).map(str::to_string)
}

/// `[[hooks.<Event>]]` + `hooks = [{ … }]` —— grok、codex 的 `config.toml`。
fn toml_grouped(raw: &str) -> SourceRead {
    let doc = match raw.parse::<toml_edit::Document>() {
        Ok(doc) => doc,
        Err(e) => return json_err(format!("不是合法的 TOML：{e}")),
    };
    let Some(events) = doc.get("hooks").and_then(|i| i.as_table_like()) else {
        return SourceRead::default();
    };
    let mut hooks = Vec::new();
    for (event, item) in events.iter() {
        // 同一个事件可以有多组（grok 的 `Notification` 就配了两组不同 matcher 的）。
        let groups: Vec<&dyn toml_edit::TableLike> = match item.as_array_of_tables() {
            Some(list) => list.iter().map(|t| t as &dyn toml_edit::TableLike).collect(),
            None => item.as_table_like().into_iter().collect(),
        };
        for group in groups {
            let matcher = group
                .get("matcher")
                .and_then(|i| i.as_str())
                .map(str::to_string);
            let enabled = group
                .get("enabled")
                .and_then(|i| i.as_bool())
                .unwrap_or(true);
            let Some(items) = group.get("hooks").and_then(|i| i.as_array()) else {
                continue;
            };
            for value in items.iter() {
                let Some(table) = value.as_inline_table() else { continue };
                hooks.push(HookDef {
                    event: event.to_string(),
                    matcher: matcher.clone(),
                    group: None,
                    kind: table
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("command")
                        .to_string(),
                    command: ["command", "prompt", "url"]
                        .iter()
                        .find_map(|key| table.get(key).and_then(|v| v.as_str()))
                        .unwrap_or_default()
                        .to_string(),
                    timeout: table.get("timeout").and_then(|v| v.as_integer()),
                    enabled: enabled
                        && table.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                });
            }
        }
    }
    SourceRead { hooks, error: None }
}

/// `[[hooks]]`，每条自带 `event` —— kimi。
fn toml_list(raw: &str) -> SourceRead {
    let doc = match raw.parse::<toml_edit::Document>() {
        Ok(doc) => doc,
        Err(e) => return json_err(format!("不是合法的 TOML：{e}")),
    };
    let Some(list) = doc.get("hooks").and_then(|i| i.as_array_of_tables()) else {
        return SourceRead::default();
    };
    let hooks = list
        .iter()
        .filter_map(|hook| {
            let event = hook.get("event").and_then(|i| i.as_str())?;
            Some(HookDef {
                event: event.to_string(),
                matcher: hook.get("matcher").and_then(|i| i.as_str()).map(str::to_string),
                group: None,
                kind: hook
                    .get("type")
                    .and_then(|i| i.as_str())
                    .unwrap_or("command")
                    .to_string(),
                command: hook
                    .get("command")
                    .and_then(|i| i.as_str())
                    .unwrap_or_default()
                    .to_string(),
                timeout: hook.get("timeout").and_then(|i| i.as_integer()),
                enabled: hook.get("enabled").and_then(|i| i.as_bool()).unwrap_or(true),
            })
        })
        .collect();
    SourceRead { hooks, error: None }
}

// ---------------------------------------------------------------------------
// 扫描
// ---------------------------------------------------------------------------

/// 一条定义连同它来自哪儿。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookAt {
    pub agent: String,
    pub source: HookSource,
    pub def: HookDef,
}

/// 按命令归并之后的一条 hook。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookEntry {
    /// 命令指纹，同时当列表的 key。
    pub fingerprint: String,
    /// 原样的命令。列表标题就是它 —— 不从命令里猜名字。
    pub command: String,
    pub hooks: Vec<HookAt>,
    pub agents: Vec<String>,
    /// 它挂在哪些事件上，去重后按首次出现顺序。
    pub events: Vec<String>,
    /// 本 app 自己装的回合信号：**不可删不可改**，删掉之后 GUI 聊天界面收不到回合
    /// 结束事件。要撤走设置页那个已有的重置入口。
    pub managed: bool,
    /// 至少有一条没被关掉。
    pub enabled: bool,
}

/// 一个事件在各家的支持情况。「添加 hook」那个列表照它渲染。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookEventInfo {
    pub name: String,
    /// 支持它的 agent，按 [`AGENTS`] 顺序。
    pub agents: Vec<String>,
    /// 已经配了几条。默认列表只显示这个数不为 0 的。
    pub configured: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookAgentInfo {
    pub agent: String,
    pub installed: bool,
    pub supported: bool,
    pub write_path: Option<String>,
    pub events: Vec<String>,
    pub sources: Vec<HookSourceInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSourceInfo {
    #[serde(flatten)]
    pub source: HookSource,
    pub hooks: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSummary {
    /// 列表上的行数（归并之后）。
    pub hooks: usize,
    /// 配置文件里的条数（归并之前）。两个数差很多是正常的 —— 一条命令常常挂在
    /// 好几家的好几个事件上。
    pub defs: usize,
    pub managed: usize,
    pub disabled: usize,
    /// 配了 hook 的事件数。
    pub events: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookScan {
    pub home: String,
    pub hooks: Vec<HookEntry>,
    pub agents: Vec<HookAgentInfo>,
    /// 全部已实证事件的并集，附各家支持情况。
    pub events: Vec<HookEventInfo>,
    pub summary: HookSummary,
}

pub fn scan(cwd: Option<&Path>) -> HookScan {
    let mut agents: Vec<HookAgentInfo> = Vec::new();
    // 指纹 → 这条命令的全部落点。BTreeMap 让列表顺序稳定。
    let mut grouped: BTreeMap<String, Vec<HookAt>> = BTreeMap::new();
    // 事件 → 支持它的 agent。按 AGENTS 的顺序插入，所以不用再排。
    let mut events: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for agent in AGENTS {
        let Ok(s) = surface(agent) else { continue };
        let installed = s.config_home().is_some_and(|p| p.is_dir());
        let sources = s.hook_sources(cwd);
        let supported = s.capabilities().hooks;
        let write_path = sources
            .iter()
            .find(|src| src.writable)
            .map(|src| src.path.clone());
        for event in s.hook_events() {
            events
                .entry((*event).to_string())
                .or_default()
                .push((*agent).to_string());
        }

        let mut infos = Vec::new();
        for source in &sources {
            let read = read_source(source);
            infos.push(HookSourceInfo {
                source: source.clone(),
                hooks: read.hooks.len(),
                error: read.error,
            });
            for def in read.hooks {
                grouped.entry(def.fingerprint()).or_default().push(HookAt {
                    agent: (*agent).to_string(),
                    source: source.clone(),
                    def,
                });
            }
        }
        agents.push(HookAgentInfo {
            agent: (*agent).to_string(),
            installed,
            supported,
            write_path,
            events: s.hook_events().iter().map(|e| (*e).to_string()).collect(),
            sources: infos,
        });
    }

    let mut hooks: Vec<HookEntry> = Vec::new();
    // 回合信号并成**一行**。它在 5 家 agent × 最多 6 个事件上装了十几条，而且每条的
    // 命令行都不一样（参数里带着 agent 名和状态），按指纹归并会摊成十几行几乎一模一样
    // 的东西。它们是一件事，用户对它只有一个动作（去设置页重置），所以并成一行。
    let mut managed: Vec<HookAt> = Vec::new();
    for (fp, at) in grouped {
        if at
            .first()
            .is_some_and(|a| crate::turn::is_turn_hook_command(&a.def.command))
        {
            managed.extend(at);
        } else {
            hooks.push(entry(fp, at));
        }
    }
    hooks.sort_by_key(|h| h.command.clone());
    if !managed.is_empty() {
        // 受管的排最前：它是唯一一条用户不能动的，摆在最上面比藏在字母序中间诚实。
        hooks.insert(0, entry(MANAGED_KEY.to_string(), managed));
    }

    let mut configured: BTreeMap<&str, usize> = BTreeMap::new();
    for h in &hooks {
        for e in &h.events {
            *configured.entry(e.as_str()).or_default() += 1;
        }
    }
    let event_infos = events
        .iter()
        .map(|(name, agents)| HookEventInfo {
            name: name.clone(),
            agents: agents.clone(),
            configured: configured.get(name.as_str()).copied().unwrap_or(0),
        })
        .collect();

    let summary = HookSummary {
        hooks: hooks.len(),
        defs: hooks.iter().map(|h| h.hooks.len()).sum(),
        managed: hooks.iter().filter(|h| h.managed).map(|h| h.hooks.len()).sum(),
        disabled: hooks.iter().filter(|h| !h.enabled).count(),
        events: configured.len(),
    };

    HookScan {
        home: crate::util::home().to_string_lossy().to_string(),
        hooks,
        agents,
        events: event_infos,
        summary,
    }
}

/// 回合信号那一行的 key。它不是指纹 —— 那一行本来就不对应单一命令。
pub const MANAGED_KEY: &str = "\u{1f}managed";

fn entry(fingerprint: String, hooks: Vec<HookAt>) -> HookEntry {
    let mut agents: Vec<String> = Vec::new();
    let mut events: Vec<String> = Vec::new();
    for at in &hooks {
        if !agents.contains(&at.agent) {
            agents.push(at.agent.clone());
        }
        if !events.contains(&at.def.event) {
            events.push(at.def.event.clone());
        }
    }
    let command = hooks
        .first()
        .map(|at| at.def.command.clone())
        .unwrap_or_default();
    HookEntry {
        managed: fingerprint == MANAGED_KEY,
        enabled: hooks.iter().any(|at| at.def.enabled),
        fingerprint,
        command,
        agents,
        events,
        hooks,
    }
}

#[tauri::command(async)]
pub fn tools_scan_hooks(cwd: Option<String>) -> HookScan {
    scan(cwd.as_deref().map(Path::new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ConfigOrigin, ConfigScope};

    fn scratch(name: &str, file: &str, body: &str) -> (PathBuf, HookSource) {
        let dir = std::env::temp_dir().join(format!(
            "cssv-hooks-{}-{}-{name}",
            std::process::id(),
            crate::util::now_millis()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        fs::write(&path, body).unwrap();
        let format = if file.ends_with(".toml") {
            HookFormat::TomlGrouped
        } else {
            HookFormat::GroupedJson
        };
        let source = HookSource {
            path: path.to_string_lossy().to_string(),
            scope: ConfigScope::User,
            origin: ConfigOrigin::Own,
            format,
            writable: true,
            exists: true,
            conditional: false,
        };
        (dir, source)
    }

    #[test]
    fn grouped_json_reads_matcher_and_every_handler_in_a_group() {
        let (dir, src) = scratch(
            "grouped",
            "settings.json",
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[
                {"type":"command","command":"a","timeout":5},
                {"type":"command","command":"b"}]}]}}"#,
        );
        let read = read_source(&src);
        assert!(read.error.is_none());
        assert_eq!(read.hooks.len(), 2);
        assert_eq!(read.hooks[0].event, "PreToolUse");
        assert_eq!(read.hooks[0].matcher.as_deref(), Some("Bash"));
        assert_eq!(read.hooks[0].timeout, Some(5));
        assert_eq!(read.hooks[1].command, "b");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `prompt` / `url` 型的 handler 不能落成空命令 —— 那会画出一条「什么都不做」的
    /// hook，而它其实在跑。
    #[test]
    fn a_prompt_handler_is_not_read_as_an_empty_command() {
        let (dir, src) = scratch(
            "prompt",
            "settings.json",
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"prompt","prompt":"check it"}]}]}}"#,
        );
        let read = read_source(&src);
        assert_eq!(read.hooks[0].kind, "prompt");
        assert_eq!(read.hooks[0].command, "check it");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn toml_grouped_keeps_the_two_notification_groups_apart() {
        // grok 的 `Notification` 就是配两组不同 matcher 的，合成一组等于丢掉一半语义。
        let (dir, mut src) = scratch(
            "grok",
            "config.toml",
            "[hooks]\n\n[[hooks.Notification]]\nmatcher = \"idle_prompt\"\nhooks = [{ type = \"command\", command = \"a\" }]\n\n[[hooks.Notification]]\nmatcher = \"permission_prompt\"\nhooks = [{ type = \"command\", command = \"b\" }]\n",
        );
        src.format = HookFormat::TomlGrouped;
        let read = read_source(&src);
        assert_eq!(read.hooks.len(), 2);
        let matchers: Vec<_> = read.hooks.iter().filter_map(|h| h.matcher.clone()).collect();
        assert_eq!(matchers, vec!["idle_prompt", "permission_prompt"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn toml_list_takes_the_event_off_each_entry() {
        let (dir, mut src) = scratch(
            "kimi",
            "config.toml",
            "[[hooks]]\nevent = \"TurnStarted\"\ncommand = \"a\"\ntimeout = 5\n\n[[hooks]]\nevent = \"Stop\"\ncommand = \"b\"\n",
        );
        src.format = HookFormat::TomlList;
        let read = read_source(&src);
        assert_eq!(
            read.hooks.iter().map(|h| h.event.as_str()).collect::<Vec<_>>(),
            vec!["TurnStarted", "Stop"]
        );
        assert_eq!(read.hooks[0].timeout, Some(5));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_json_keeps_the_hook_name_layer() {
        let (dir, mut src) = scratch(
            "agy",
            "hooks.json",
            r#"{"my-hook":{"PreInvocation":[{"type":"command","command":"a"}]}}"#,
        );
        src.format = HookFormat::AgyJson;
        let read = read_source(&src);
        assert_eq!(read.hooks.len(), 1);
        assert_eq!(read.hooks[0].group.as_deref(), Some("my-hook"));
        assert_eq!(read.hooks[0].event, "PreInvocation");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 一个坏逗号让一家 agent 的 hook 整片消失，而「静悄悄少几行」是最坏的失败方式。
    #[test]
    fn a_broken_file_is_reported_not_swallowed() {
        let (dir, src) = scratch("broken", "settings.json", r#"{"hooks":{,}}"#);
        let read = read_source(&src);
        assert!(read.error.is_some(), "坏文件必须报错");
        assert!(read.hooks.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_simply_empty() {
        let (dir, mut src) = scratch("missing", "settings.json", "{}");
        src.path = dir.join("nope.json").to_string_lossy().to_string();
        let read = read_source(&src);
        assert!(read.error.is_none());
        assert!(read.hooks.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_fingerprint_only_looks_at_the_command() {
        let a = HookDef {
            event: "Stop".into(),
            matcher: None,
            group: None,
            kind: "command".into(),
            command: "bash  x.sh".into(),
            timeout: Some(5),
            enabled: true,
        };
        let b = HookDef {
            event: "PreToolUse".into(),
            matcher: Some("Bash".into()),
            timeout: None,
            ..a.clone()
        };
        // 同一条命令挂在不同事件上是**一件事**，列成两行没法看。
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.fingerprint(), "bash x.sh");
    }

    /// 每家的事件目录都必须非空，且 `hooks_config_path` 有就该有目录 —— 能力位说
    /// 支持 hooks、事件列表却是空的，「添加」框里就一个选项都没有。
    #[test]
    fn every_agent_with_hooks_has_an_evidenced_event_catalog() {
        for agent in super::super::AGENTS {
            let s = super::super::surface(agent).unwrap();
            assert_eq!(
                s.capabilities().hooks,
                !s.hook_events().is_empty(),
                "{agent}：能力位和事件目录对不上"
            );
        }
    }

    /// 可写的 hook 来源每家至多一个，且必须是 user 级自有的那个。项目级的跟着仓库走、
    /// 会被提交，绝不能被这个面板改。
    #[test]
    fn at_most_one_writable_hook_source_per_agent() {
        let cwd = std::env::temp_dir();
        for agent in super::super::AGENTS {
            let s = super::super::surface(agent).unwrap();
            let sources = s.hook_sources(Some(&cwd));
            let writable: Vec<_> = sources.iter().filter(|src| src.writable).collect();
            assert!(writable.len() <= 1, "{agent}：有 {} 个可写来源", writable.len());
            for src in writable {
                assert_eq!(src.scope, ConfigScope::User, "{agent}");
                assert_eq!(src.origin, ConfigOrigin::Own, "{agent}");
            }
        }
    }

    /// 后缀和格式必须对得上。挂错一次就是整片读不出来（MCP 那边 codex 就是这么栽的）。
    #[test]
    fn every_toml_hook_source_is_parsed_as_toml() {
        let cwd = std::env::temp_dir();
        for agent in super::super::AGENTS {
            let s = super::super::surface(agent).unwrap();
            for src in s.hook_sources(Some(&cwd)) {
                let is_toml = src.path.ends_with(".toml");
                let parsed_as_toml =
                    matches!(src.format, HookFormat::TomlGrouped | HookFormat::TomlList);
                assert_eq!(is_toml, parsed_as_toml, "{agent}：{}", src.path);
            }
        }
    }
}
