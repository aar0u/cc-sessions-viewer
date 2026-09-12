//! MCP 配置的写入。
//!
//! 和 [`super::mcp`] 一样，**这里一行 agent 判断都没有**：写哪个文件由该 agent 的
//! `mcp_sources` 里 `writable` 的那一条决定，怎么写由那条来源的 [`McpFormat`] 决定。
//!
//! 三条贯穿始终的原则：
//!
//! 1. **只改该改的那几个字节。** `~/.claude.json` 有 187 KB，绝大部分是会话历史。
//!    整份反序列化再写回会把它全部重排（`serde_json` 默认的 Map 是 BTreeMap，键会被
//!    排序），一旦拼错就是把用户的会话历史一起搭进去。所以 JSON 走
//!    [`top_level_span`] 定位 `mcpServers` 那一段，只换这一段。
//! 2. **写不了就说写不了。** 做不到的请求进 `blocked` 并带上原因，绝不静默跳过 ——
//!    「显示成功但什么都没做」是这类面板最坏的失败方式。
//! 3. **只写确认过的键。** codex 的配置结构体是 `deny_unknown_fields` 的（二进制里
//!    有 `unknown field \`` 的报错串），往 `[mcp_servers.*]` 里塞一个它不认识的键，
//!    整份 config.toml 就废了。宁可少写一个键，也不猜。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::mcp::{read_source, McpServerDef, McpTransport};
use super::{surface, McpFormat, McpSource};
use crate::util;

// ---------------------------------------------------------------------------
// 对外形状
// ---------------------------------------------------------------------------

/// 一条写请求要做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpOp {
    /// 写进这家的可写文件（没有就新增，有就覆盖）。
    Put,
    /// 从这家的可写文件里摘掉。
    Drop,
    /// 只翻 `enabled` 位，定义留着。
    Enable,
    Disable,
}

/// 用户填的一份定义。**不收 `incomplete` / `transport` 这类派生字段的结论** ——
/// 那些由后端按形状重算，前端说了不算。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInput {
    pub transport: McpTransport,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub env: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEdit {
    pub agent: String,
    pub name: String,
    pub op: McpOp,
    /// `Put` 要写的定义；其余操作用不上。
    pub def: Option<McpServerInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum McpStepKind {
    /// 这家原来没有，新增一条。
    Add,
    /// 这家已经有了，覆盖。
    Update,
    Remove,
    Enable,
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum McpStepNote {
    /// 目标文件还不存在，会连它一起建出来。
    NewFile,
    /// 写进去了，但这家有优先级更高的来源也定义了同名 server —— 实际跑的仍是那一份。
    /// 不提示的话用户会以为「改了没反应」。
    Shadowed,
}

/// 计划里的一步。一步 = 对一个文件里的一条 server 做一件事。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpWriteStep {
    pub agent: String,
    pub name: String,
    pub kind: McpStepKind,
    pub path: String,
    /// 改之前那份的命令行（新增时为空）。
    pub before: Option<String>,
    /// 改之后的命令行（移除时为空）。
    pub after: Option<String>,
    pub note: Option<McpStepNote>,
    /// `Shadowed` 时是哪个文件盖住了它。
    pub shadowed_by: Option<String>,
    /// `dry_run` 恒为 false；真跑时表示这一步做成了。
    pub done: bool,
}

/// 做不了的请求，连同原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum McpBlockReason {
    /// 这家根本没有 MCP 支持。
    Unsupported,
    /// 这家没有可写的来源。
    NoWritableSource,
    /// 可写文件里没有这条 —— 要动的那份在别的文件里（项目配置、或别家的兼容来源），
    /// 改可写文件也动不到它。
    NotInWritableSource,
    /// 这个格式没有确认过的停用开关，只能移除。
    NoEnableSwitch,
    /// 既没 command 也没 url，写进去也起不来。
    Incomplete,
    /// 远端 server 暂不写：各家的 URL 键不一样（agy 用 `httpUrl`），没逐家实测前不猜。
    RemoteReadOnly,
    /// 这个格式写不了 headers。
    HeadersUnsupported,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpBlocked {
    pub agent: String,
    pub name: String,
    pub reason: McpBlockReason,
    /// 和原因相关的那个文件（有的话）。
    pub path: Option<String>,
}

/// 一个目标文件在**排这份计划的那一刻**的样子。
///
/// dry-run 把它交给前端，前端在用户点「确认」时原样递回来。对不上就拒绝执行 ——
/// 这条路径和全局指令那边的 `revision` 是同一套规矩，理由也一样：用户批准的是
/// 他当时看到的那份计划，不是执行时磁盘上碰巧是什么就写什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpFileStamp {
    pub path: String,
    /// 不透明字符串，前端只负责原样带回来，不解读。
    pub stamp: String,
}

/// 真跑时中途停下来的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum McpFailKind {
    /// 这个文件在「看计划」和「点确认」之间被别处改过了。
    Stale,
    /// 写的时候出错了。
    Write,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpFailure {
    pub kind: McpFailKind,
    pub path: String,
    /// `Write` 时的底层报错原文。
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpWriteReport {
    pub dry_run: bool,
    pub steps: Vec<McpWriteStep>,
    pub blocked: Vec<McpBlocked>,
    /// 这份计划要碰的每个文件的当时样子。dry-run 时填，真跑时回传。
    pub stamps: Vec<McpFileStamp>,
    /// 真跑中途停下来的原因。**`steps` 里 `done` 为真的那几步已经落盘**，其余一个
    /// 字节都没动 —— 只报一句「失败了」的话，用户根本不知道该从哪儿收拾。
    pub failed: Option<McpFailure>,
}

/// 读一个文件现在的指纹。文件不存在也有指纹（「它当时不存在」本身就是计划的前提）。
fn stamp_of(path: &Path) -> Result<String, String> {
    let r = util::file_revision(path)?;
    let millis = r
        .modified
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(format!("{}:{}:{millis}", r.exists, r.size))
}

// ---------------------------------------------------------------------------
// 计划
// ---------------------------------------------------------------------------

/// 对某个文件里某条 server 的一次改动。
#[derive(Debug, Clone)]
enum Mutation {
    Put(McpServerInput),
    Drop,
    SetEnabled(bool),
}

/// 攒到同一个文件上的全部改动。一个文件只读一次、只写一次 —— 一条一条地
/// 读-改-写，中间任何一步失败都会留下改了一半的文件。
struct FileEdits {
    path: PathBuf,
    format: McpFormat,
    ops: Vec<(String, Mutation)>,
}

/// 这个格式认不认 `enabled`。
///
/// - TOML：codex 的 `RawMcpServerConfig` 里有 `enabled`，本机 `~/.codex/config.toml`
///   里就躺着一条 `enabled = false` 的。
/// - opencode：`@opencode-ai/sdk` 的 `McpLocalConfig` / `McpRemoteConfig` 都有。
/// - 其余 JSON（claude / agy / kimi / pi）：**没有可确认的停用开关**。写一个它们不读的
///   `enabled: false` 进去，面板上显示「已停用」而 server 照跑，比不提供这个功能坏得多。
fn has_enable_switch(format: McpFormat) -> bool {
    matches!(format, McpFormat::TomlServers | McpFormat::OpencodeJson)
}

/// 这个格式写不写得了 headers。
///
/// TOML 那边 codex 的键叫 `http_headers` 而不是 `headers`，grok 的键名没能从它的二进制
/// 里确认；两家共用 `TomlServers` 这一个格式，所以只能取交集 —— 写不了就明说。
fn can_write_headers(format: McpFormat) -> bool {
    !matches!(format, McpFormat::TomlServers)
}

/// 排好一份计划。`dry_run` 只影响执不执行，计划本身是同一份。
///
/// `stamps` 是 dry-run 报告里那一份，真跑时必须原样回传：每个要碰的文件都得在里面、
/// 且指纹对得上，否则整批拒绝（**一个文件都不写**）。dry-run 时传空。
pub fn apply(
    edits: &[McpEdit],
    cwd: Option<&Path>,
    dry_run: bool,
    stamps: &[McpFileStamp],
) -> Result<McpWriteReport, String> {
    let mut steps: Vec<McpWriteStep> = Vec::new();
    let mut blocked: Vec<McpBlocked> = Vec::new();
    // 路径 → 攒在它上面的改动。BTreeMap 让执行顺序稳定（按路径），报告才复现得了。
    let mut files: BTreeMap<PathBuf, FileEdits> = BTreeMap::new();

    for edit in edits {
        let block = |reason, path| McpBlocked {
            agent: edit.agent.clone(),
            name: edit.name.clone(),
            reason,
            path,
        };
        let Ok(s) = surface(&edit.agent) else {
            blocked.push(block(McpBlockReason::Unsupported, None));
            continue;
        };
        if !s.capabilities().mcp {
            blocked.push(block(McpBlockReason::Unsupported, None));
            continue;
        }
        let sources = s.mcp_sources(cwd);
        let Some(target) = sources.iter().find(|src| src.writable) else {
            blocked.push(block(McpBlockReason::NoWritableSource, None));
            continue;
        };
        let path = PathBuf::from(&target.path);
        let existing = read_source(target, cwd)
            .servers
            .into_iter()
            .find(|d| d.name == edit.name);

        let mutation = match edit.op {
            McpOp::Put => {
                let Some(def) = edit.def.clone() else {
                    blocked.push(block(McpBlockReason::Incomplete, Some(target.path.clone())));
                    continue;
                };
                if def.command.is_none() && def.url.is_none() {
                    blocked.push(block(McpBlockReason::Incomplete, Some(target.path.clone())));
                    continue;
                }
                if def.url.is_some() {
                    blocked.push(block(
                        McpBlockReason::RemoteReadOnly,
                        Some(target.path.clone()),
                    ));
                    continue;
                }
                if !def.headers.is_empty() && !can_write_headers(target.format) {
                    blocked.push(block(
                        McpBlockReason::HeadersUnsupported,
                        Some(target.path.clone()),
                    ));
                    continue;
                }
                Mutation::Put(def)
            }
            McpOp::Drop => {
                if existing.is_none() {
                    blocked.push(block(
                        McpBlockReason::NotInWritableSource,
                        Some(target.path.clone()),
                    ));
                    continue;
                }
                Mutation::Drop
            }
            McpOp::Enable | McpOp::Disable => {
                if !has_enable_switch(target.format) {
                    blocked.push(block(
                        McpBlockReason::NoEnableSwitch,
                        Some(target.path.clone()),
                    ));
                    continue;
                }
                if existing.is_none() {
                    blocked.push(block(
                        McpBlockReason::NotInWritableSource,
                        Some(target.path.clone()),
                    ));
                    continue;
                }
                Mutation::SetEnabled(edit.op == McpOp::Enable)
            }
        };

        let shadow = shadowed_by(&sources, target, &edit.name, cwd);
        steps.push(McpWriteStep {
            agent: edit.agent.clone(),
            name: edit.name.clone(),
            kind: match (&mutation, existing.is_some()) {
                (Mutation::Put(_), true) => McpStepKind::Update,
                (Mutation::Put(_), false) => McpStepKind::Add,
                (Mutation::Drop, _) => McpStepKind::Remove,
                (Mutation::SetEnabled(true), _) => McpStepKind::Enable,
                (Mutation::SetEnabled(false), _) => McpStepKind::Disable,
            },
            path: target.path.clone(),
            before: existing.as_ref().map(command_line),
            after: match &mutation {
                Mutation::Put(def) => Some(input_command_line(def)),
                Mutation::Drop => None,
                Mutation::SetEnabled(_) => existing.as_ref().map(command_line),
            },
            note: match (&shadow, path.is_file()) {
                (Some(_), _) => Some(McpStepNote::Shadowed),
                (None, false) => Some(McpStepNote::NewFile),
                (None, true) => None,
            },
            shadowed_by: shadow,
            done: false,
        });
        files
            .entry(path.clone())
            .or_insert_with(|| FileEdits {
                path,
                format: target.format,
                ops: Vec::new(),
            })
            .ops
            .push((edit.name.clone(), mutation));
    }

    let mut taken: Vec<McpFileStamp> = Vec::new();
    for file in files.values() {
        taken.push(McpFileStamp {
            path: file.path.to_string_lossy().to_string(),
            stamp: stamp_of(&file.path)?,
        });
    }

    if dry_run {
        return Ok(McpWriteReport {
            dry_run: true,
            steps,
            blocked,
            stamps: taken,
            failed: None,
        });
    }

    let failed = match stale_target(&taken, stamps) {
        Some(path) => Some(McpFailure {
            kind: McpFailKind::Stale,
            path,
            detail: None,
        }),
        None => run(&files, &mut steps),
    };
    Ok(McpWriteReport {
        dry_run: false,
        steps,
        blocked,
        stamps: taken,
        failed,
    })
}

/// 第一个和计划对不上的文件。**所有文件都得先对一遍，再动第一个字节** —— 逐个边对
/// 边写的话，第三个文件对不上时前两个已经按一份过期的计划写下去了。
///
/// 计划里没有的文件同样算对不上：重排之后多出一个目标，说明磁盘上的局面已经和用户
/// 看到的那份不一样了。
fn stale_target(now: &[McpFileStamp], approved: &[McpFileStamp]) -> Option<String> {
    now.iter()
        .find(|n| {
            approved.iter().find(|a| a.path == n.path).map(|a| &a.stamp) != Some(&n.stamp)
        })
        .map(|n| n.path.clone())
}

/// 逐个文件落盘，把做成了的那几步标上 `done`。
///
/// 多文件不是一个原子操作 —— 七家 agent 的配置在七个文件里，没有哪个机制能把它们
/// 一起提交。做不到原子就**如实交代做到哪儿了**：停在第一个失败上，已落盘的那几步
/// `done` 为真，其余一个字节都没动。只抛一个字符串出去的话，用户只知道「失败了」，
/// 不知道自己现在有几份配置已经被改了。
fn run(files: &BTreeMap<PathBuf, FileEdits>, steps: &mut [McpWriteStep]) -> Option<McpFailure> {
    for file in files.values() {
        if let Err(e) = write_file(file) {
            return Some(McpFailure {
                kind: McpFailKind::Write,
                path: file.path.to_string_lossy().to_string(),
                detail: Some(e),
            });
        }
        for step in steps.iter_mut() {
            if step.path == file.path.to_string_lossy() {
                step.done = true;
            }
        }
    }
    None
}

/// 同一家 agent 里，有没有优先级比目标文件更高的来源也定义了这个名字。
fn shadowed_by(
    sources: &[McpSource],
    target: &McpSource,
    name: &str,
    cwd: Option<&Path>,
) -> Option<String> {
    sources
        .iter()
        .filter(|src| src.precedence > target.precedence)
        .find(|src| read_source(src, cwd).servers.iter().any(|d| d.name == name))
        .map(|src| src.path.clone())
}

fn command_line(def: &McpServerDef) -> String {
    // 计划里也用打过码的那份：确认框是截图最多的一屏。
    if let Some(url) = def.url_masked.as_ref().or(def.url.as_ref()) {
        return url.clone();
    }
    std::iter::once(def.command.clone().unwrap_or_default())
        .chain(def.args.iter().cloned())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn input_command_line(def: &McpServerInput) -> String {
    if let Some(url) = &def.url {
        return super::mcp::mask_url(url);
    }
    std::iter::once(def.command.clone().unwrap_or_default())
        .chain(def.args.iter().cloned())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// 落盘
// ---------------------------------------------------------------------------

fn write_file(file: &FileEdits) -> Result<(), String> {
    let label = file.path.to_string_lossy().to_string();
    let before = util::file_revision(&file.path)?;
    let raw = if before.exists {
        fs::read_to_string(&file.path).map_err(|e| format!("读不了 {label}：{e}"))?
    } else {
        String::new()
    };
    let text = edit_text(&raw, file.format, &file.ops)?;
    // 写之前先把**打算写出去的这段**解回来一遍：字符串拼坏了不该等到用户下次启动
    // agent 才发现。
    parse_check(&text, file.format).map_err(|e| format!("{label} 写出来的内容不合法：{e}"))?;

    let backup = backup_path(&file.path);
    util::atomic_write_backed_up(&file.path, text.as_bytes(), &before, &backup, &label)?;

    // 写完再读回来解一遍。落盘成功不等于内容对（截断、编码、别的进程插进来）。
    let back = fs::read_to_string(&file.path).map_err(|e| format!("回读 {label} 失败：{e}"))?;
    if let Err(e) = parse_check(&back, file.format) {
        if before.exists {
            let _ = fs::copy(&backup, &file.path);
        }
        return Err(format!("{label} 写完校验不通过，已从备份还原：{e}"));
    }
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config");
    path.with_file_name(format!("{name}.bak"))
}

/// 这段文本按这个格式解得开吗。只判「解不解得开」，不判内容。
fn parse_check(text: &str, format: McpFormat) -> Result<(), String> {
    match format {
        McpFormat::TomlServers => text
            .parse::<toml_edit::Document>()
            .map(|_| ())
            .map_err(|e| e.to_string()),
        _ => serde_json::from_str::<serde_json::Value>(text)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    }
}

fn edit_text(raw: &str, format: McpFormat, ops: &[(String, Mutation)]) -> Result<String, String> {
    match format {
        McpFormat::TomlServers => edit_toml(raw, ops),
        McpFormat::OpencodeJson => edit_json(raw, "mcp", ops, opencode_entry),
        // `JsonProjectServers` 永远不是可写来源（`~/.claude.json` 的 local scope 归
        // claude 自己管），走到这儿只可能是 `mcp_sources` 配错了，按 `mcpServers` 处理
        // 反而会把 server 写到错误的层级上。
        McpFormat::JsonProjectServers => Err("按项目分区的 JSON 不作为写入目标".into()),
        McpFormat::JsonServers => edit_json(raw, "mcpServers", ops, json_entry),
    }
}

// ---------------------------------------------------------------------------
// TOML
// ---------------------------------------------------------------------------

fn edit_toml(raw: &str, ops: &[(String, Mutation)]) -> Result<String, String> {
    let mut doc = raw
        .parse::<toml_edit::Document>()
        .map_err(|e| format!("不是合法的 TOML：{e}"))?;
    if doc.get("mcp_servers").is_none() {
        let mut table = toml_edit::Table::new();
        // 隐式：只出现 `[mcp_servers.foo]`，不额外冒一个空的 `[mcp_servers]` 头。
        table.set_implicit(true);
        doc["mcp_servers"] = toml_edit::Item::Table(table);
    }
    let table = doc["mcp_servers"]
        .as_table_like_mut()
        .ok_or_else(|| "mcp_servers 不是一张表".to_string())?;
    for (name, mutation) in ops {
        match mutation {
            Mutation::Put(def) => match table.get_mut(name).and_then(|i| i.as_table_like_mut()) {
                Some(entry) => merge_toml_entry(entry, def),
                None => {
                    table.insert(name, toml_edit::Item::Table(toml_entry(def)));
                }
            },
            Mutation::Drop => {
                table.remove(name);
            }
            Mutation::SetEnabled(on) => {
                let entry = table
                    .get_mut(name)
                    .ok_or_else(|| format!("{name} 不在这个文件里"))?;
                entry["enabled"] = toml_edit::value(*on);
            }
        }
    }
    Ok(doc.to_string())
}

/// 覆盖一条已经在的 `[mcp_servers.<name>]`。
///
/// **只动我们认得的键。** 整张表换掉的话，用户自己写的 `startup_timeout_sec` /
/// `tool_timeout_sec`（codex `RawMcpServerConfig` 里有、我们不写的那些）会跟着没，
/// 而面板上只说了一句「更新」—— 这是静默改坏用户配置，比写不进去坏得多。
///
/// `enabled` 是例外：它归我们管，`put` 要把它清掉。勾选框把一条「关着的」勾上时走的
/// 正是这条路径，留着 `enabled = false` 面板会显示勾上了而 server 还是不跑。
fn merge_toml_entry(entry: &mut dyn toml_edit::TableLike, def: &McpServerInput) {
    let fresh = toml_entry(def);
    for key in TOML_OWNED_KEYS {
        match fresh.get(key) {
            Some(item) => {
                entry.insert(key, item.clone());
            }
            None => {
                entry.remove(key);
            }
        }
    }
}

/// 归写入方管的 TOML 键：有新值就覆盖，没有就删掉。**不在这张表里的一律不碰。**
///
/// `url` 在列是因为「本来是远端、这次改成本地命令」时那个旧地址必须跟着走，留着
/// 它就是一条自相矛盾的配置。`type` 不在列 —— codex 的结构体里根本没有这个键，
/// 真在文件里见到它说明是别人写的，删不如留。
const TOML_OWNED_KEYS: &[&str] = &["command", "args", "cwd", "env", "url", "enabled"];

/// `[mcp_servers.<name>]`。
///
/// 键只取 codex `RawMcpServerConfig` 里确认存在的那几个（`command` / `args` / `env` /
/// `cwd` / `enabled`），**不写 `type`** —— 那个结构体里没有这个字段，而 codex 对未知
/// 字段是报错的，写进去等于把整份 config.toml 弄废。传输方式它自己按有没有 `command`
/// 推，和我们读的时候用的是同一条规则。
fn toml_entry(def: &McpServerInput) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    if let Some(command) = &def.command {
        table["command"] = toml_edit::value(command.clone());
    }
    if !def.args.is_empty() {
        table["args"] = toml_edit::value(def.args.iter().collect::<toml_edit::Array>());
    }
    if let Some(cwd) = &def.cwd {
        table["cwd"] = toml_edit::value(cwd.clone());
    }
    if !def.env.is_empty() {
        let mut env = toml_edit::Table::new();
        for v in &def.env {
            env[&v.key] = toml_edit::value(v.value.clone());
        }
        table["env"] = toml_edit::Item::Table(env);
    }
    table
}

// ---------------------------------------------------------------------------
// JSON：只换 `mcpServers` 那一段
// ---------------------------------------------------------------------------

/// 根对象里某个键的位置。
#[derive(Debug, PartialEq, Eq)]
enum Span {
    /// 键在，值占 `start..end`。
    Found { start: usize, end: usize },
    /// 键不在。`after` 是最后一个成员的结尾（空对象时为 `None`），`close` 是根对象
    /// 那个 `}` 的位置。
    Missing { after: Option<usize>, close: usize },
}

fn edit_json(
    raw: &str,
    key: &str,
    ops: &[(String, Mutation)],
    entry: fn(&McpServerInput) -> serde_json::Value,
) -> Result<String, String> {
    let raw = if raw.trim().is_empty() { "{}" } else { raw };
    let span = top_level_span(raw, key)?;
    let mut map = match span {
        Span::Found { start, end } => serde_json::from_str::<serde_json::Map<_, _>>(&raw[start..end])
            .map_err(|e| format!("{key} 不是一个对象：{e}"))?,
        Span::Missing { .. } => serde_json::Map::new(),
    };
    for (name, mutation) in ops {
        match mutation {
            Mutation::Put(def) => {
                let fresh = entry(def);
                match map.get_mut(name).and_then(|v| v.as_object_mut()) {
                    Some(slot) => merge_json_entry(slot, &fresh),
                    None => {
                        map.insert(name.clone(), fresh);
                    }
                }
            }
            Mutation::Drop => {
                map.remove(name);
            }
            Mutation::SetEnabled(on) => {
                let slot = map
                    .get_mut(name)
                    .and_then(|v| v.as_object_mut())
                    .ok_or_else(|| format!("{name} 不在这个文件里"))?;
                slot.insert("enabled".into(), serde_json::Value::Bool(*on));
            }
        }
    }
    let body = indent(&serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?);
    Ok(match span {
        Span::Found { start, end } => format!("{}{body}{}", &raw[..start], &raw[end..]),
        Span::Missing {
            after: Some(after),
            ..
        } => format!(
            "{},\n  {}: {body}{}",
            &raw[..after],
            serde_json::Value::String(key.to_string()),
            &raw[after..]
        ),
        Span::Missing { after: None, close } => format!(
            "{}\n  {}: {body}\n{}",
            &raw[..close],
            serde_json::Value::String(key.to_string()),
            &raw[close..]
        ),
    })
}

/// 覆盖一条已经在的 JSON entry。和 [`merge_toml_entry`] 同一条规矩：只动我们认得的键，
/// 其余（各家自己的超时、实验开关…）原样留着。
fn merge_json_entry(slot: &mut serde_json::Map<String, serde_json::Value>, fresh: &serde_json::Value) {
    let Some(fresh) = fresh.as_object() else { return };
    for key in JSON_OWNED_KEYS {
        match fresh.get(*key) {
            Some(v) => {
                slot.insert((*key).to_string(), v.clone());
            }
            None => {
                slot.remove(*key);
            }
        }
    }
}

/// 归写入方管的 JSON 键。
///
/// `url` / `httpUrl` 在列的理由和 TOML 那边一样。`enabled` / `disabled` 两个都在：
/// 停用位归我们管，`put` 要把它清干净 —— 只清一个的话，一条 `disabled: true` 的
/// claude server 勾上之后仍然是关的。
const JSON_OWNED_KEYS: &[&str] = &[
    "type",
    "command",
    "args",
    "cwd",
    "env",
    "environment",
    "headers",
    "url",
    "httpUrl",
    "enabled",
    "disabled",
];

/// 把 `to_string_pretty` 出来的块整体缩进一层 —— 它是按「自己是根」排的版，而我们要把
/// 它塞进根对象的一个成员位置上。
fn indent(pretty: &str) -> String {
    pretty.replace('\n', "\n  ")
}

/// 在一段 JSON 文本里定位顶层对象某个键的值占了哪几个字节。
///
/// 手写扫描而不是 `serde_json` 整份来回：`~/.claude.json` 有 187 KB 且装着会话历史，
/// 整份重写会把跟 MCP 毫无关系的内容全部重排。这里只定位 `mcpServers` 那一段，其余
/// 字节原封不动。
fn top_level_span(raw: &str, key: &str) -> Result<Span, String> {
    let b = raw.as_bytes();
    let mut i = skip_ws(b, 0);
    if b.get(i) != Some(&b'{') {
        return Err("JSON 根不是一个对象".into());
    }
    i += 1;
    let mut after: Option<usize> = None;
    loop {
        i = skip_ws(b, i);
        match b.get(i) {
            Some(b'}') => return Ok(Span::Missing { after, close: i }),
            Some(b'"') => {}
            _ => return Err("JSON 结构不认识：对象里等一个键".into()),
        }
        let key_end = scan_string(b, i)?;
        let this = serde_json::from_str::<String>(&raw[i..key_end])
            .map_err(|e| format!("键读不出来：{e}"))?;
        i = skip_ws(b, key_end);
        if b.get(i) != Some(&b':') {
            return Err("JSON 结构不认识：键后面等一个冒号".into());
        }
        let start = skip_ws(b, i + 1);
        let end = scan_value(b, start)?;
        if this == key {
            return Ok(Span::Found { start, end });
        }
        after = Some(end);
        i = skip_ws(b, end);
        match b.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => return Ok(Span::Missing { after, close: i }),
            _ => return Err("JSON 结构不认识：值后面等逗号或右括号".into()),
        }
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// 跳过一个字符串字面量，返回收尾引号之后的位置。
fn scan_string(b: &[u8], mut i: usize) -> Result<usize, String> {
    i += 1; // 开头那个引号
    while i < b.len() {
        match b[i] {
            // `\\` 后面跟什么都不算收尾引号。`ሴ` 那四位是普通字符，跳两个就够。
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err("JSON 里有没收尾的字符串".into())
}

/// 跳过一个值，返回它之后的位置。
fn scan_value(b: &[u8], i: usize) -> Result<usize, String> {
    match b.get(i) {
        Some(b'"') => scan_string(b, i),
        Some(open @ (b'{' | b'[')) => {
            let close = if *open == b'{' { b'}' } else { b']' };
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = scan_string(b, j)?;
                        continue;
                    }
                    c if c == *open => depth += 1,
                    c if c == close => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            Err("JSON 里有没收尾的对象或数组".into())
        }
        Some(_) => {
            // 数字 / true / false / null：一路走到分隔符。
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && !b[j].is_ascii_whitespace()
            {
                j += 1;
            }
            if j == i {
                return Err("JSON 结构不认识：等一个值".into());
            }
            Ok(j)
        }
        None => Err("JSON 提前结束了".into()),
    }
}

/// `{ "mcpServers": { "<name>": … } }` 里的一条。
fn json_entry(def: &McpServerInput) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    // `type` 这几家（claude / agy / kimi / pi）都是 TS 写的、对未知键宽容，写出来比
    // 让读的人去猜清楚。
    if let Some(kind) = transport_key(def.transport) {
        map.insert("type".into(), kind.into());
    }
    if let Some(command) = &def.command {
        map.insert("command".into(), command.clone().into());
    }
    if !def.args.is_empty() {
        map.insert("args".into(), def.args.clone().into());
    }
    if let Some(cwd) = &def.cwd {
        map.insert("cwd".into(), cwd.clone().into());
    }
    if !def.env.is_empty() {
        map.insert("env".into(), vars(&def.env));
    }
    if !def.headers.is_empty() {
        map.insert("headers".into(), vars(&def.headers));
    }
    serde_json::Value::Object(map)
}

/// opencode 的 `mcp` 键：`command` 是命令和参数混在一起的数组，env 键叫 `environment`。
fn opencode_entry(def: &McpServerInput) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("type".into(), "local".into());
    let argv: Vec<String> = std::iter::once(def.command.clone().unwrap_or_default())
        .chain(def.args.iter().cloned())
        .filter(|p| !p.is_empty())
        .collect();
    map.insert("command".into(), argv.into());
    if !def.env.is_empty() {
        map.insert("environment".into(), vars(&def.env));
    }
    // opencode 的 schema 里 `enabled` 没有默认值，不显式写的话它按 undefined 处理；
    // 写出来省得用户以为「加了但没开」。
    map.insert("enabled".into(), true.into());
    serde_json::Value::Object(map)
}

fn vars(list: &[KeyValue]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for v in list {
        map.insert(v.key.clone(), v.value.clone().into());
    }
    serde_json::Value::Object(map)
}

fn transport_key(transport: McpTransport) -> Option<&'static str> {
    match transport {
        McpTransport::Stdio => Some("stdio"),
        McpTransport::Http => Some("http"),
        McpTransport::Sse => Some("sse"),
        McpTransport::Ws => Some("ws"),
        // 认不出来的就别写 —— 写一个假的 `type` 比不写坏。
        McpTransport::Unknown => None,
    }
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn tools_apply_mcp(
    edits: Vec<McpEdit>,
    cwd: Option<String>,
    dry_run: bool,
    stamps: Vec<McpFileStamp>,
) -> Result<McpWriteReport, String> {
    apply(&edits, cwd.as_deref().map(Path::new), dry_run, &stamps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(command: &str, args: &[&str]) -> McpServerInput {
        McpServerInput {
            transport: McpTransport::Stdio,
            command: Some(command.to_string()),
            args: args.iter().map(|a| a.to_string()).collect(),
            url: None,
            env: Vec::new(),
            headers: Vec::new(),
            cwd: None,
        }
    }

    fn put(name: &str, def: McpServerInput) -> Vec<(String, Mutation)> {
        vec![(name.to_string(), Mutation::Put(def))]
    }

    // -----------------------------------------------------------------------
    // JSON：定位与拼接
    // -----------------------------------------------------------------------

    #[test]
    fn span_finds_the_value_of_a_top_level_key() {
        let raw = r#"{ "a": 1, "mcpServers": {"x": {}}, "b": 2 }"#;
        let Span::Found { start, end } = top_level_span(raw, "mcpServers").unwrap() else {
            panic!("没找到");
        };
        assert_eq!(&raw[start..end], r#"{"x": {}}"#);
    }

    /// 别的值里的花括号、方括号、转义引号都不能把扫描带偏 —— 带偏一次就是把
    /// `~/.claude.json` 从中间劈开。
    #[test]
    fn span_is_not_confused_by_braces_inside_other_values() {
        let raw = r#"{"note":"a \" } brace","list":[{"k":"}"},[1,2]],"mcpServers":{"x":1}}"#;
        let Span::Found { start, end } = top_level_span(raw, "mcpServers").unwrap() else {
            panic!("没找到");
        };
        assert_eq!(&raw[start..end], r#"{"x":1}"#);
    }

    #[test]
    fn span_reports_where_to_insert_when_the_key_is_absent() {
        assert_eq!(
            top_level_span(r#"{}"#, "mcpServers").unwrap(),
            Span::Missing {
                after: None,
                close: 1
            }
        );
        let raw = r#"{"a": 1}"#;
        assert_eq!(
            top_level_span(raw, "mcpServers").unwrap(),
            Span::Missing {
                after: Some(7),
                close: 7
            }
        );
    }

    #[test]
    fn span_refuses_what_it_cannot_read() {
        assert!(top_level_span("[]", "mcpServers").is_err());
        assert!(top_level_span(r#"{"a" 1}"#, "mcpServers").is_err());
        assert!(top_level_span(r#"{"a": "unterminated"#, "mcpServers").is_err());
    }

    /// 这条是整个写入路径的地基：`~/.claude.json` 里 187 KB 的会话历史必须**一个字节
    /// 都不动**。整份 `serde_json` 来回会把它全部重排，所以只换 `mcpServers` 那一段。
    #[test]
    fn editing_touches_nothing_outside_the_mcp_servers_block() {
        let raw = "{\n  \"numStartups\": 12,\n  \"mcpServers\": {\n    \"old\": {\n      \"command\": \"a\"\n    }\n  },\n  \"projects\": {\n    \"/x\": { \"history\": [1, 2, 3] }\n  }\n}";
        let out = edit_json(raw, "mcpServers", &put("new", input("npx", &["-y", "f"])), json_entry)
            .unwrap();
        assert!(out.starts_with("{\n  \"numStartups\": 12,\n"), "{out}");
        assert!(
            out.ends_with("  \"projects\": {\n    \"/x\": { \"history\": [1, 2, 3] }\n  }\n}"),
            "{out}"
        );
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back["numStartups"], 12);
        assert_eq!(back["mcpServers"]["new"]["command"], "npx");
        // 原来那条不能被顺手擦掉 —— 一次写入只动点名的那一条。
        assert_eq!(back["mcpServers"]["old"]["command"], "a");
    }

    #[test]
    fn a_missing_mcp_servers_key_is_inserted_and_the_file_stays_valid() {
        for raw in ["{}", "{\n  \"a\": 1\n}", ""] {
            let out =
                edit_json(raw, "mcpServers", &put("x", input("npx", &[])), json_entry).unwrap();
            let back: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(back["mcpServers"]["x"]["command"], "npx", "{raw:?} → {out}");
        }
    }

    #[test]
    fn dropping_the_last_server_leaves_an_empty_object_not_a_broken_file() {
        let raw = r#"{"mcpServers":{"x":{"command":"a"}}}"#;
        let out = edit_json(
            raw,
            "mcpServers",
            &[("x".to_string(), Mutation::Drop)],
            json_entry,
        )
        .unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(back["mcpServers"].as_object().unwrap().is_empty(), "{out}");
    }

    #[test]
    fn opencode_gets_its_own_shape_command_array_and_environment() {
        let mut def = input("npx", &["-y", "foo"]);
        def.env = vec![KeyValue {
            key: "K".into(),
            value: "V".into(),
        }];
        let out = edit_json("{}", "mcp", &put("x", def), opencode_entry).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        let entry = &back["mcp"]["x"];
        assert_eq!(entry["type"], "local");
        assert_eq!(entry["command"], serde_json::json!(["npx", "-y", "foo"]));
        // env 键叫 environment —— 写成 env 的话 opencode 一个变量都读不到。
        assert_eq!(entry["environment"]["K"], "V");
        assert!(entry.get("env").is_none());
    }

    // -----------------------------------------------------------------------
    // TOML
    // -----------------------------------------------------------------------

    /// codex 的配置结构体对未知字段是报错的，而它的 `RawMcpServerConfig` 里没有
    /// `type`。写进去 = 整份 config.toml 报废。
    #[test]
    fn toml_never_writes_a_type_key() {
        for transport in [
            McpTransport::Stdio,
            McpTransport::Http,
            McpTransport::Sse,
            McpTransport::Unknown,
        ] {
            let mut def = input("npx", &["foo"]);
            def.transport = transport;
            let out = edit_toml("", &put("x", def)).unwrap();
            assert!(!out.contains("type"), "{transport:?} → {out}");
            assert!(!out.contains("transport"), "{transport:?} → {out}");
        }
    }

    #[test]
    fn toml_keeps_the_rest_of_the_file_including_comments() {
        let raw = "# 我的配置\nmodel = \"gpt-5\"\n\n[mcp_servers.keep]\ncommand = \"a\"\n";
        let out = edit_toml(raw, &put("added", input("npx", &["-y", "f"]))).unwrap();
        assert!(out.contains("# 我的配置"), "{out}");
        assert!(out.contains("model = \"gpt-5\""), "{out}");
        let doc = out.parse::<toml_edit::Document>().unwrap();
        assert_eq!(doc["mcp_servers"]["keep"]["command"].as_str(), Some("a"));
        assert_eq!(doc["mcp_servers"]["added"]["command"].as_str(), Some("npx"));
        assert_eq!(
            doc["mcp_servers"]["added"]["args"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn toml_disable_keeps_the_definition() {
        let raw = "[mcp_servers.x]\ncommand = \"a\"\nargs = [\"m\"]\n";
        let out = edit_toml(raw, &[("x".to_string(), Mutation::SetEnabled(false))]).unwrap();
        let doc = out.parse::<toml_edit::Document>().unwrap();
        assert_eq!(doc["mcp_servers"]["x"]["enabled"].as_bool(), Some(false));
        // 停用不是删除：命令行必须原样留着。
        assert_eq!(doc["mcp_servers"]["x"]["command"].as_str(), Some("a"));
    }

    /// 覆盖一条已经在的：用户自己写的键得留着。
    ///
    /// codex 的 `RawMcpServerConfig` 比我们写的那几个键多，整张表换掉就是替用户把
    /// 超时设置删了 —— 而面板上只说了一句「更新」。
    #[test]
    fn updating_a_toml_server_keeps_keys_this_app_does_not_write() {
        let raw = "[mcp_servers.x]\ncommand = \"old\"\nstartup_timeout_sec = 60\ntool_timeout_sec = 30\n";
        let out = edit_toml(raw, &put("x", input("npx", &["-y", "f"]))).unwrap();
        let doc = out.parse::<toml_edit::Document>().unwrap();
        assert_eq!(doc["mcp_servers"]["x"]["command"].as_str(), Some("npx"));
        assert_eq!(
            doc["mcp_servers"]["x"]["startup_timeout_sec"].as_integer(),
            Some(60),
            "{out}"
        );
        assert_eq!(
            doc["mcp_servers"]["x"]["tool_timeout_sec"].as_integer(),
            Some(30),
            "{out}"
        );
    }

    /// 反过来：归我们管的键有旧值没新值时必须删掉，留着就是一条自相矛盾的配置
    /// （既有 `url` 又有 `command`，谁也说不清它会怎么起）。
    #[test]
    fn updating_a_toml_server_clears_the_keys_it_owns() {
        let raw = "[mcp_servers.x]\nurl = \"https://old\"\nargs = [\"a\"]\ncwd = \"/tmp\"\n";
        let out = edit_toml(raw, &put("x", input("npx", &[]))).unwrap();
        let doc = out.parse::<toml_edit::Document>().unwrap();
        let entry = doc["mcp_servers"]["x"].as_table_like().unwrap();
        assert!(entry.get("url").is_none(), "{out}");
        assert!(entry.get("args").is_none(), "{out}");
        assert!(entry.get("cwd").is_none(), "{out}");
    }

    /// 勾选框把一条「关着的」勾上时走的就是 `put`。停用位不清掉的话，面板显示勾上了
    /// 而 server 还是不跑 —— 「显示成功但什么都没做」。
    #[test]
    fn putting_over_a_disabled_server_turns_it_back_on() {
        let raw = "[mcp_servers.x]\ncommand = \"a\"\nenabled = false\n";
        let out = edit_toml(raw, &put("x", input("npx", &[]))).unwrap();
        let doc = out.parse::<toml_edit::Document>().unwrap();
        assert!(
            doc["mcp_servers"]["x"].as_table_like().unwrap().get("enabled").is_none(),
            "{out}"
        );
    }

    #[test]
    fn updating_a_json_server_keeps_keys_this_app_does_not_write() {
        let raw = r#"{"mcpServers":{"x":{"command":"old","timeout":120,"trust":true}}}"#;
        let out = edit_json(raw, "mcpServers", &put("x", input("npx", &[])), json_entry).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back["mcpServers"]["x"]["command"], "npx");
        assert_eq!(back["mcpServers"]["x"]["timeout"], 120, "{out}");
        assert_eq!(back["mcpServers"]["x"]["trust"], true, "{out}");
    }

    /// `disabled: true` 是 Cursor 那套的停用写法，扫描是认它的。只清 `enabled`
    /// 不清它的话，勾上之后面板显示开着、实际还是关的。
    #[test]
    fn putting_over_a_json_server_clears_both_spellings_of_off() {
        let raw = r#"{"mcpServers":{"x":{"command":"a","enabled":false,"disabled":true,"url":"https://old"}}}"#;
        let out = edit_json(raw, "mcpServers", &put("x", input("npx", &[])), json_entry).unwrap();
        let entry = serde_json::from_str::<serde_json::Value>(&out).unwrap()["mcpServers"]["x"]
            .as_object()
            .unwrap()
            .clone();
        assert!(entry.get("enabled").is_none(), "{out}");
        assert!(entry.get("disabled").is_none(), "{out}");
        assert!(entry.get("url").is_none(), "{out}");
    }

    #[test]
    fn a_brand_new_toml_gets_no_stray_empty_table_header() {
        let out = edit_toml("", &put("x", input("npx", &[]))).unwrap();
        assert!(out.starts_with("[mcp_servers.x]"), "{out}");
    }

    // -----------------------------------------------------------------------
    // 能力判定
    // -----------------------------------------------------------------------

    /// 停用开关只有确认认它的格式才开放。给 claude 写一个它不读的 `enabled: false`，
    /// 面板显示「已停用」而 server 照跑 —— 比没有这个功能坏得多。
    #[test]
    fn only_formats_with_a_confirmed_enabled_key_can_be_disabled() {
        assert!(has_enable_switch(McpFormat::TomlServers));
        assert!(has_enable_switch(McpFormat::OpencodeJson));
        assert!(!has_enable_switch(McpFormat::JsonServers));
        assert!(!has_enable_switch(McpFormat::JsonProjectServers));
    }

    #[test]
    fn headers_are_refused_on_toml_rather_than_written_under_a_guessed_key() {
        assert!(!can_write_headers(McpFormat::TomlServers));
        assert!(can_write_headers(McpFormat::JsonServers));
    }

    #[test]
    fn the_project_partitioned_json_is_never_a_write_target() {
        assert!(edit_text("{}", McpFormat::JsonProjectServers, &[]).is_err());
    }

    // -----------------------------------------------------------------------
    // 落盘
    // -----------------------------------------------------------------------

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cssv-mcpw-{}-{}-{name}",
            std::process::id(),
            util::now_millis()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writing_backs_the_file_up_first() {
        let dir = scratch("backup");
        let path = dir.join("claude.json");
        fs::write(&path, r#"{"numStartups":3}"#).unwrap();
        write_file(&FileEdits {
            path: path.clone(),
            format: McpFormat::JsonServers,
            ops: put("x", input("npx", &[])),
        })
        .unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["mcpServers"]["x"]["command"], "npx");
        // 备份是写之前那一份，原样。
        let backup = fs::read_to_string(dir.join("claude.json.bak")).unwrap();
        assert_eq!(backup, r#"{"numStartups":3}"#);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_does_not_exist_yet_is_created() {
        let dir = scratch("create");
        let path = dir.join("nested").join("mcp.json");
        write_file(&FileEdits {
            path: path.clone(),
            format: McpFormat::JsonServers,
            ops: put("x", input("npx", &[])),
        })
        .unwrap();
        let back: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["mcpServers"]["x"]["command"], "npx");
        // 没有原文件就没有备份，别凭空造一个空的出来。
        assert!(!dir.join("nested").join("mcp.json.bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 坏文件必须**整片拒绝**，不能「跳过读不出来的部分」然后把剩下的写回去 ——
    /// 那等于用户手抖一个逗号，我们就把他别的 server 全删了。
    #[test]
    fn a_broken_file_is_refused_instead_of_rewritten() {
        let dir = scratch("broken");
        let path = dir.join("mcp.json");
        let raw = r#"{"mcpServers":{"x":{"command":"a"},}}"#;
        fs::write(&path, raw).unwrap();
        let err = write_file(&FileEdits {
            path: path.clone(),
            format: McpFormat::JsonServers,
            ops: put("y", input("npx", &[])),
        })
        .unwrap_err();
        assert!(err.contains("mcpServers"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), raw);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 坏在 `mcpServers` **之外**的文件也得拦住。定位扫描找到键就收工，不会替我们
    /// 校验后面那一截 —— 拦住它的是写之前那道「把打算写出去的内容解回来」。
    #[test]
    fn a_file_broken_outside_the_block_is_caught_before_it_lands() {
        let dir = scratch("broken-tail");
        let path = dir.join("mcp.json");
        let raw = r#"{"mcpServers":{},"tail":[1,2,}"#;
        fs::write(&path, raw).unwrap();
        assert!(write_file(&FileEdits {
            path: path.clone(),
            format: McpFormat::JsonServers,
            ops: put("y", input("npx", &[])),
        })
        .is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), raw);
        let _ = fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // 确认期间被改 / 写到一半失败
    // -----------------------------------------------------------------------

    fn stamp(path: &str, s: &str) -> McpFileStamp {
        McpFileStamp {
            path: path.into(),
            stamp: s.into(),
        }
    }

    fn planned_step(path: &str) -> McpWriteStep {
        McpWriteStep {
            agent: "claude".into(),
            name: "x".into(),
            kind: McpStepKind::Add,
            path: path.into(),
            before: None,
            after: Some("npx".into()),
            note: None,
            shadowed_by: None,
            done: false,
        }
    }

    #[test]
    fn a_plan_whose_files_are_untouched_goes_ahead() {
        let now = vec![stamp("/a", "true:10:5"), stamp("/b", "false:0:0")];
        assert_eq!(stale_target(&now, &now.clone()), None);
    }

    /// 用户批准的是他**当时看到的**那份计划。确认框开着的时候别的进程改了同一个
    /// 文件，就不能再按旧结论往下写。
    #[test]
    fn a_file_changed_since_the_preview_stops_the_whole_batch() {
        let approved = vec![stamp("/a", "true:10:5"), stamp("/b", "true:20:7")];
        let now = vec![stamp("/a", "true:10:5"), stamp("/b", "true:31:9")];
        assert_eq!(stale_target(&now, &approved), Some("/b".into()));
    }

    /// 重排之后多出一个目标文件，说明局面已经变了 —— 用户没在计划里见过它。
    #[test]
    fn a_target_that_was_not_in_the_approved_plan_stops_the_batch() {
        let approved = vec![stamp("/a", "true:10:5")];
        let now = vec![stamp("/a", "true:10:5"), stamp("/b", "false:0:0")];
        assert_eq!(stale_target(&now, &approved), Some("/b".into()));
    }

    /// 指纹得真的跟着文件走，否则上面那几条全是摆设。
    #[test]
    fn the_stamp_moves_when_the_file_does() {
        let dir = scratch("stamp");
        let path = dir.join("mcp.json");
        let missing = stamp_of(&path).unwrap();
        fs::write(&path, "{}").unwrap();
        let created = stamp_of(&path).unwrap();
        fs::write(&path, r#"{"mcpServers":{}}"#).unwrap();
        let grown = stamp_of(&path).unwrap();
        assert_ne!(missing, created);
        assert_ne!(created, grown);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 七家配置在七个文件里，没有哪个机制能把它们一起提交。做不到原子就得**说清楚
    /// 做到哪儿了** —— 只报一句「失败了」的话，用户不知道自己现在有几份配置已经改了。
    #[test]
    fn a_failure_halfway_reports_which_files_already_landed() {
        let dir = scratch("partial");
        let good = dir.join("a.json");
        let bad = dir.join("b.json");
        let later = dir.join("c.json");
        fs::write(&good, "{}").unwrap();
        // 解不开的一份：写它必定失败，而它排在 a 后面、c 前面（BTreeMap 按路径）。
        fs::write(&bad, r#"{"mcpServers":{,}}"#).unwrap();
        fs::write(&later, "{}").unwrap();

        let mut files: BTreeMap<PathBuf, FileEdits> = BTreeMap::new();
        for path in [&good, &bad, &later] {
            files.insert(
                path.clone(),
                FileEdits {
                    path: path.clone(),
                    format: McpFormat::JsonServers,
                    ops: put("x", input("npx", &[])),
                },
            );
        }
        let mut steps: Vec<McpWriteStep> = [&good, &bad, &later]
            .iter()
            .map(|p| planned_step(&p.to_string_lossy()))
            .collect();

        let failure = run(&files, &mut steps).expect("中间那个必须失败");
        assert_eq!(failure.kind, McpFailKind::Write);
        assert_eq!(failure.path, bad.to_string_lossy());
        // 做成了的那一步标出来，没轮到的那一步不能冒充做过。
        assert!(steps[0].done, "a 已经落盘了，得说出来");
        assert!(!steps[1].done);
        assert!(!steps[2].done, "停在失败那一步，后面的不再动");
        // 而且后面那个文件是真的一个字节都没动。
        assert_eq!(fs::read_to_string(&later).unwrap(), "{}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_concurrent_change_stops_the_write() {
        let dir = scratch("race");
        let path = dir.join("mcp.json");
        fs::write(&path, "{}").unwrap();
        let before = util::file_revision(&path).unwrap();
        fs::write(&path, r#"{"a":1}"#).unwrap();
        let err = util::atomic_write_backed_up(
            &path,
            b"{}",
            &before,
            &dir.join("mcp.json.bak"),
            "mcp.json",
        )
        .unwrap_err();
        assert!(err.contains("changed while writing"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"a":1}"#);
        let _ = fs::remove_dir_all(&dir);
    }
}
