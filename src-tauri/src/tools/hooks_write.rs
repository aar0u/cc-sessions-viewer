//! Hook 配置的写入，外加「拿一条假事件试跑」。
//!
//! 和 [`super::mcp_write`] 同一套规矩（先 dry-run 出计划、写前备份、写后回读校验、
//! 做不了的进 `blocked` 不静默跳过），差别只在两处：
//!
//! - **回合信号受保护。** 本 app 自己装的那十几条一律不许在这儿动 —— 删掉之后 GUI
//!   聊天界面收不到回合结束事件，而用户完全看不出因果。要撤走设置页那个重置入口。
//! - **事件要先验支持。** 各家的事件集合不一样（codex 没有 `Notification`，agy 只有
//!   `PreInvocation` 那一套）。往一个这家根本不认的事件上装 hook，配置写得进去、
//!   永远不会触发，然后用户去查自己的命令哪儿写错了。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::hooks::read_source;
use super::{surface, HookFormat};
use crate::util;

/// agy 的 `hooks.json` 根上是一个个**有名字的** hook。这个面板写进去的都归到这一个
/// 名字底下：agy 那一层只是个命名空间，逼用户给每条 hook 起个名字纯属多余，而固定一个
/// 名字之后删除也只要认命令。
const AGY_GROUP: &str = "sessions-viewer-tools";

/// 试跑的墙钟上限。hook 本来就该是「几十毫秒就返回」的东西，跑到 10 秒还没完的
/// 基本就是挂住了 —— 真让它跑下去会把面板卡死。
const TEST_TIMEOUT_MS: u64 = 10_000;

// ---------------------------------------------------------------------------
// 对外形状
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HookOp {
    Add,
    Remove,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookEdit {
    pub agent: String,
    pub op: HookOp,
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    /// 秒。各家单位不统一，所以**原样写**，不替用户换算。
    pub timeout: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HookStepKind {
    Add,
    Remove,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookWriteStep {
    pub agent: String,
    pub kind: HookStepKind,
    pub path: String,
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    /// 目标文件还不存在，会连它一起建出来。
    pub new_file: bool,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HookBlockReason {
    /// 这家 agent 没有 hook 机制。
    Unsupported,
    /// 这家没有我们能写的 hook 文件。
    NoWritableSource,
    /// 回合信号，不许在这儿动。
    Protected,
    /// 这家不认这个事件 —— 写得进去，永远不会触发。
    UnknownEvent,
    /// 可写文件里没有这一条（它在项目配置里，或者根本不存在）。
    NotInWritableSource,
    /// 命令是空的。
    EmptyCommand,
    /// 可写文件里已经有一条一模一样的（同事件、同匹配器、同命令）。
    ///
    /// 写入那一层是无脑 `push`，再来一条就是**同一个 hook 挂两遍**，每次事件触发两次。
    /// 从配置集导入时最容易撞上：「刚才成了没有」再点一次，就多出一条。
    AlreadyThere,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookBlocked {
    pub agent: String,
    pub event: String,
    pub reason: HookBlockReason,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookWriteReport {
    pub dry_run: bool,
    pub steps: Vec<HookWriteStep>,
    pub blocked: Vec<HookBlocked>,
}

// ---------------------------------------------------------------------------
// 计划
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Mutation {
    op: HookOp,
    event: String,
    matcher: Option<String>,
    command: String,
    timeout: Option<i64>,
}

struct FileEdits {
    path: PathBuf,
    format: HookFormat,
    ops: Vec<Mutation>,
}

/// 这一条动不动得了 —— 只看「可写文件里有没有它」。
///
/// `Add` 和 `Remove` 问的是同一件事，只是期望相反：删要求它**在**，加要求它**不在**。
/// 早先只写了删那一半，于是同一条 hook 加两遍就真的挂两遍（写入那层是无脑 `push`），
/// 每次事件触发两次；而扫描是**按命令归并**的，列表上一条都看不出来。从配置集导入
/// 时最容易撞上：「刚才成了没有」再点一次，就多出一条。
///
/// 只看可写的那一个文件，不看项目级和别家的来源：那些我们本来就写不到，拿它们拦
/// 用户等于拦掉一件他做得成的事。
fn presence_block(op: HookOp, here: bool) -> Option<HookBlockReason> {
    match (op, here) {
        (HookOp::Remove, false) => Some(HookBlockReason::NotInWritableSource),
        (HookOp::Add, true) => Some(HookBlockReason::AlreadyThere),
        _ => None,
    }
}

pub fn apply(
    edits: &[HookEdit],
    cwd: Option<&Path>,
    dry_run: bool,
) -> Result<HookWriteReport, String> {
    let mut steps: Vec<HookWriteStep> = Vec::new();
    let mut blocked: Vec<HookBlocked> = Vec::new();
    let mut files: BTreeMap<PathBuf, FileEdits> = BTreeMap::new();

    for edit in edits {
        let block = |reason, path| HookBlocked {
            agent: edit.agent.clone(),
            event: edit.event.clone(),
            reason,
            path,
        };
        // 受保护判定放在最前面，任何一条路径都绕不过去。
        if crate::turn::is_turn_hook_command(&edit.command) {
            blocked.push(block(HookBlockReason::Protected, None));
            continue;
        }
        if edit.command.trim().is_empty() {
            blocked.push(block(HookBlockReason::EmptyCommand, None));
            continue;
        }
        let Ok(s) = surface(&edit.agent) else {
            blocked.push(block(HookBlockReason::Unsupported, None));
            continue;
        };
        if !s.capabilities().hooks {
            blocked.push(block(HookBlockReason::Unsupported, None));
            continue;
        }
        if edit.op == HookOp::Add && !s.hook_events().contains(&edit.event.as_str()) {
            blocked.push(block(HookBlockReason::UnknownEvent, None));
            continue;
        }
        let sources = s.hook_sources(cwd);
        let Some(target) = sources.iter().find(|src| src.writable) else {
            blocked.push(block(HookBlockReason::NoWritableSource, None));
            continue;
        };
        let path = PathBuf::from(&target.path);

        let here = read_source(target).hooks.into_iter().any(|d| {
            d.event == edit.event && d.command == edit.command && d.matcher == edit.matcher
        });
        if let Some(reason) = presence_block(edit.op, here) {
            blocked.push(block(reason, Some(target.path.clone())));
            continue;
        }

        steps.push(HookWriteStep {
            agent: edit.agent.clone(),
            kind: match edit.op {
                HookOp::Add => HookStepKind::Add,
                HookOp::Remove => HookStepKind::Remove,
            },
            path: target.path.clone(),
            event: edit.event.clone(),
            matcher: edit.matcher.clone(),
            command: edit.command.clone(),
            new_file: !path.is_file(),
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
            .push(Mutation {
                op: edit.op,
                event: edit.event.clone(),
                matcher: edit.matcher.clone(),
                command: edit.command.clone(),
                timeout: edit.timeout,
            });
    }

    if dry_run {
        return Ok(HookWriteReport {
            dry_run: true,
            steps,
            blocked,
        });
    }

    for file in files.values() {
        write_file(file)?;
        for step in steps.iter_mut() {
            if step.path == file.path.to_string_lossy() {
                step.done = true;
            }
        }
    }
    Ok(HookWriteReport {
        dry_run: false,
        steps,
        blocked,
    })
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
    parse_check(&text, file.format).map_err(|e| format!("{label} 写出来的内容不合法：{e}"))?;

    let backup = file.path.with_file_name(format!(
        "{}.bak",
        file.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config")
    ));
    util::atomic_write_backed_up(&file.path, text.as_bytes(), &before, &backup, &label)?;

    let back = fs::read_to_string(&file.path).map_err(|e| format!("回读 {label} 失败：{e}"))?;
    if let Err(e) = parse_check(&back, file.format) {
        if before.exists {
            let _ = fs::copy(&backup, &file.path);
        }
        return Err(format!("{label} 写完校验不通过，已从备份还原：{e}"));
    }
    Ok(())
}

fn parse_check(text: &str, format: HookFormat) -> Result<(), String> {
    match format {
        HookFormat::TomlGrouped | HookFormat::TomlList => text
            .parse::<toml_edit::Document>()
            .map(|_| ())
            .map_err(|e| e.to_string()),
        _ => serde_json::from_str::<serde_json::Value>(text)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    }
}

fn edit_text(raw: &str, format: HookFormat, ops: &[Mutation]) -> Result<String, String> {
    match format {
        HookFormat::GroupedJson => edit_grouped_json(raw, ops),
        HookFormat::AgyJson => edit_agy_json(raw, ops),
        HookFormat::TomlGrouped => edit_toml_grouped(raw, ops),
        HookFormat::TomlList => edit_toml_list(raw, ops),
    }
}

// ---- JSON ----

/// 整份读进来再写回去，**不做 MCP 那种只换一段的拼接**。
///
/// 理由是这两份文件本身就小：`~/.claude/settings.json` 几 KB，`hooks.json` 更小，
/// 里面没有会话历史那种「重排了就很难看」的东西。MCP 那边费劲只换一段，是因为
/// `~/.claude.json` 有 187 KB。
fn json_root(raw: &str) -> Result<serde_json::Value, String> {
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(raw).map_err(|e| format!("不是合法的 JSON：{e}"))
}

fn json_out(root: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string_pretty(root).map_err(|e| e.to_string())
}

fn handler(op: &Mutation) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("type".into(), "command".into());
    map.insert("command".into(), op.command.clone().into());
    if let Some(timeout) = op.timeout {
        map.insert("timeout".into(), timeout.into());
    }
    serde_json::Value::Object(map)
}

fn edit_grouped_json(raw: &str, ops: &[Mutation]) -> Result<String, String> {
    let mut root = json_root(raw)?;
    if !root.get("hooks").is_some_and(|v| v.is_object()) {
        root["hooks"] = serde_json::json!({});
    }
    let events = root["hooks"]
        .as_object_mut()
        .ok_or_else(|| "hooks 不是一个对象".to_string())?;
    for op in ops {
        match op.op {
            HookOp::Add => {
                let mut group = serde_json::Map::new();
                if let Some(matcher) = &op.matcher {
                    group.insert("matcher".into(), matcher.clone().into());
                }
                group.insert("hooks".into(), serde_json::json!([handler(op)]));
                events
                    .entry(op.event.clone())
                    .or_insert_with(|| serde_json::json!([]))
                    .as_array_mut()
                    .ok_or_else(|| format!("hooks.{} 不是一个数组", op.event))?
                    .push(serde_json::Value::Object(group));
            }
            HookOp::Remove => {
                let Some(groups) = events.get_mut(&op.event).and_then(|v| v.as_array_mut()) else {
                    continue;
                };
                for group in groups.iter_mut() {
                    if group.get("matcher").and_then(|v| v.as_str())
                        != op.matcher.as_deref()
                    {
                        continue;
                    }
                    if let Some(items) = group.get_mut("hooks").and_then(|v| v.as_array_mut()) {
                        items.retain(|item| {
                            item.get("command").and_then(|v| v.as_str()) != Some(&op.command)
                        });
                    }
                }
                // 空壳不留：一个 `{"hooks": []}` 会在面板上显示成「这个事件配着东西」。
                groups.retain(|group| {
                    group
                        .get("hooks")
                        .and_then(|v| v.as_array())
                        .is_none_or(|items| !items.is_empty())
                });
                if groups.is_empty() {
                    events.remove(&op.event);
                }
            }
        }
    }
    if events.is_empty() {
        root.as_object_mut().map(|root| root.remove("hooks"));
    }
    json_out(&root)
}

fn edit_agy_json(raw: &str, ops: &[Mutation]) -> Result<String, String> {
    let mut root = json_root(raw)?;
    for op in ops {
        match op.op {
            HookOp::Add => {
                if !root.get(AGY_GROUP).is_some_and(|v| v.is_object()) {
                    root[AGY_GROUP] = serde_json::json!({});
                }
                root[AGY_GROUP][&op.event]
                    .as_array_mut()
                    .map(|items| items.push(handler(op)))
                    .unwrap_or_else(|| {
                        root[AGY_GROUP][&op.event] = serde_json::json!([handler(op)]);
                    });
            }
            HookOp::Remove => {
                // 名字那一层是别人起的也要能删：按命令扫全部名字，不只扫我们自己那个。
                let Some(named) = root.as_object_mut() else { continue };
                for (_, events) in named.iter_mut() {
                    let Some(events) = events.as_object_mut() else { continue };
                    if let Some(items) = events.get_mut(&op.event).and_then(|v| v.as_array_mut()) {
                        items.retain(|item| {
                            item.get("command").and_then(|v| v.as_str()) != Some(&op.command)
                        });
                    }
                    events.retain(|_, items| {
                        items.as_array().is_none_or(|items| !items.is_empty())
                    });
                }
                named.retain(|_, events| {
                    events.as_object().is_none_or(|events| !events.is_empty())
                });
            }
        }
    }
    json_out(&root)
}

// ---- TOML ----

fn toml_handler(op: &Mutation) -> toml_edit::InlineTable {
    let mut table = toml_edit::InlineTable::new();
    table.insert("type", "command".into());
    table.insert("command", op.command.clone().into());
    if let Some(timeout) = op.timeout {
        table.insert("timeout", timeout.into());
    }
    table
}

/// `[[hooks.<Event>]]` —— grok。
fn edit_toml_grouped(raw: &str, ops: &[Mutation]) -> Result<String, String> {
    let mut doc = raw
        .parse::<toml_edit::Document>()
        .map_err(|e| format!("不是合法的 TOML：{e}"))?;
    for op in ops {
        match op.op {
            HookOp::Add => {
                let hooks = doc
                    .as_table_mut()
                    .entry("hooks")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                let events = hooks
                    .as_table_mut()
                    .ok_or_else(|| "hooks 不是一张表".to_string())?;
                events.set_implicit(true);
                let slot = events
                    .entry(&op.event)
                    .or_insert(toml_edit::Item::ArrayOfTables(
                        toml_edit::ArrayOfTables::new(),
                    ));
                let groups = slot
                    .as_array_of_tables_mut()
                    .ok_or_else(|| format!("hooks.{} 不是一组表", op.event))?;
                let mut group = toml_edit::Table::new();
                if let Some(matcher) = &op.matcher {
                    group.insert("matcher", toml_edit::value(matcher.clone()));
                }
                let mut handlers = toml_edit::Array::new();
                handlers.push(toml_edit::Value::InlineTable(toml_handler(op)));
                group.insert("hooks", toml_edit::value(handlers));
                groups.push(group);
            }
            HookOp::Remove => {
                let Some(events) = doc
                    .as_table_mut()
                    .get_mut("hooks")
                    .and_then(|i| i.as_table_mut())
                else {
                    continue;
                };
                let Some(groups) = events
                    .get_mut(&op.event)
                    .and_then(|i| i.as_array_of_tables_mut())
                else {
                    continue;
                };
                for group in groups.iter_mut() {
                    if group.get("matcher").and_then(|i| i.as_str()) != op.matcher.as_deref() {
                        continue;
                    }
                    if let Some(handlers) = group.get_mut("hooks").and_then(|i| i.as_array_mut()) {
                        let kept: toml_edit::Array = handlers
                            .iter()
                            .filter(|value| {
                                value
                                    .as_inline_table()
                                    .and_then(|t| t.get("command"))
                                    .and_then(|v| v.as_str())
                                    != Some(&op.command)
                            })
                            .cloned()
                            .collect();
                        *handlers = kept;
                    }
                }
                groups.retain(|group| {
                    group
                        .get("hooks")
                        .and_then(|i| i.as_array())
                        .is_none_or(|handlers| !handlers.is_empty())
                });
                if groups.is_empty() {
                    events.remove(&op.event);
                }
                if events.is_empty() {
                    doc.as_table_mut().remove("hooks");
                }
            }
        }
    }
    Ok(doc.to_string())
}

/// `[[hooks]]`，每条自带 `event` —— kimi。
fn edit_toml_list(raw: &str, ops: &[Mutation]) -> Result<String, String> {
    let mut doc = raw
        .parse::<toml_edit::Document>()
        .map_err(|e| format!("不是合法的 TOML：{e}"))?;
    for op in ops {
        match op.op {
            HookOp::Add => {
                let slot = doc
                    .as_table_mut()
                    .entry("hooks")
                    .or_insert(toml_edit::Item::ArrayOfTables(
                        toml_edit::ArrayOfTables::new(),
                    ));
                let list = slot
                    .as_array_of_tables_mut()
                    .ok_or_else(|| "hooks 不是一组表".to_string())?;
                let mut hook = toml_edit::Table::new();
                hook.insert("event", toml_edit::value(op.event.clone()));
                if let Some(matcher) = &op.matcher {
                    hook.insert("matcher", toml_edit::value(matcher.clone()));
                }
                hook.insert("command", toml_edit::value(op.command.clone()));
                if let Some(timeout) = op.timeout {
                    hook.insert("timeout", toml_edit::value(timeout));
                }
                list.push(hook);
            }
            HookOp::Remove => {
                let Some(list) = doc
                    .as_table_mut()
                    .get_mut("hooks")
                    .and_then(|i| i.as_array_of_tables_mut())
                else {
                    continue;
                };
                list.retain(|hook| {
                    !(hook.get("event").and_then(|i| i.as_str()) == Some(&op.event)
                        && hook.get("command").and_then(|i| i.as_str()) == Some(&op.command))
                });
                if list.is_empty() {
                    doc.as_table_mut().remove("hooks");
                }
            }
        }
    }
    Ok(doc.to_string())
}

// ---------------------------------------------------------------------------
// 试跑
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookTestResult {
    /// 实际喂给它的那份 JSON。**一定要给用户看** —— hook 写不对十有八九是因为
    /// 以为收到的字段长别的样。
    pub payload: String,
    pub stdout: String,
    pub stderr: String,
    /// 进程退出码。被信号打断（含超时杀掉）时为 `None`。
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// 撞上墙钟上限被杀掉的。
    pub timed_out: bool,
    /// 退出码 2：各家都拿它当「拦住这一步」。
    pub blocking: bool,
}

/// 造一条假事件喂给 hook 命令。
///
/// 字段抄的是 claude 2.1.268 自带文档里那份 payload 的形状，各家大同小异。标成
/// `session_id: "tools-dry-run"` 是为了让 hook 自己也认得出这是试跑。
fn payload(event: &str, cwd: &str) -> String {
    serde_json::json!({
        "session_id": "tools-dry-run",
        "transcript_path": "",
        "cwd": cwd,
        "hook_event_name": event,
        "tool_name": "Bash",
        "tool_input": { "command": "echo sessions-viewer-dry-run" },
    })
    .to_string()
}

/// Windows 上喂给 `-Command` 的那串脚本。
///
/// 前面必须接上 `powershell_refresh_path()` —— CLAUDE.md 那五条不变量的第 4 条。GUI 拉
/// 起来的进程继承到的 PATH 可能缺 nvm/npm 那几个目录，而 hook 命令十有八九就是在调
/// 一个 node CLI（本机那几条 `cmux …` 正是）。不刷 PATH 的后果特别坏：试跑报
/// "command not found"，用户照着去改一条**本来是好的** hook。
///
/// 编译进测试（不只是 Windows）是为了能在 macOS 上断言这条前缀真的在。
#[cfg(any(windows, test))]
fn windows_hook_script(command: &str) -> String {
    format!(
        "{}; {command}",
        crate::agent_command::powershell_refresh_path()
    )
}

/// 用系统 shell 跑一条 hook 命令。
///
/// **必须过 shell**：本机真实的 hook 命令里有 `&&`、`||`、`>/dev/null`、`$VAR`
/// （cmux 那几条就是），直接 spawn 会把整串当成一个可执行文件名。
fn shell_command(command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = util::silent_command("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &windows_hook_script(command),
        ]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = util::silent_command("/bin/sh");
        cmd.args(["-c", command]);
        cmd
    }
}

pub fn test(command: &str, event: &str, cwd: Option<&Path>) -> Result<HookTestResult, String> {
    if command.trim().is_empty() {
        return Err("命令是空的".into());
    }
    let dir = cwd
        .map(|p| p.to_path_buf())
        .unwrap_or_else(crate::util::home);
    let body = payload(event, &dir.to_string_lossy());
    let started = std::time::Instant::now();

    let mut child = shell_command(command)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("起不来：{e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // 写不进去不算错：有的 hook 根本不读 stdin，直接就退了。
        let _ = stdin.write_all(body.as_bytes());
    }

    let mut timed_out = false;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None if started.elapsed().as_millis() as u64 >= TEST_TIMEOUT_MS => {
                let _ = child.kill();
                timed_out = true;
                break;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let exit_code = out.status.code();
    Ok(HookTestResult {
        payload: body,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        blocking: exit_code == Some(2),
        exit_code,
        duration_ms: started.elapsed().as_millis() as u64,
        timed_out,
    })
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn tools_apply_hooks(
    edits: Vec<HookEdit>,
    cwd: Option<String>,
    dry_run: bool,
) -> Result<HookWriteReport, String> {
    apply(&edits, cwd.as_deref().map(Path::new), dry_run)
}

#[tauri::command(async)]
pub fn tools_test_hook(
    command: String,
    event: String,
    cwd: Option<String>,
) -> Result<HookTestResult, String> {
    test(&command, &event, cwd.as_deref().map(Path::new))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLAUDE.md 的第 4 条不变量：每条要跑的 CLI 命令前面都得先刷 PATH。
    ///
    /// 试跑这条路最容易漏 —— 它跑的是**用户写的**命令，看着不像"我们在调 CLI"。但那串
    /// 命令十有八九就是在调一个 node CLI，PATH 不全时试跑报 command not found，用户会
    /// 照着去改一条本来是好的 hook。
    #[test]
    fn a_windows_dry_run_refreshes_path_before_the_command() {
        let script = windows_hook_script("cmux hooks codex stop");
        assert!(script.starts_with("$machinePath ="), "{script}");
        assert!(script.ends_with("; cmux hooks codex stop"), "{script}");
        // 顺带把第 5 条钉住：`-ExecutionPolicy Bypass` 少了，npm/nvm 那些 .ps1 shim 会被挡。
        let flags = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"];
        let source = include_str!("hooks_write.rs");
        for flag in flags {
            assert!(source.contains(&format!("\"{flag}\"")), "missing {flag}");
        }
    }

    fn add(event: &str, matcher: Option<&str>, command: &str) -> Mutation {
        Mutation {
            op: HookOp::Add,
            event: event.to_string(),
            matcher: matcher.map(str::to_string),
            command: command.to_string(),
            timeout: Some(5),
        }
    }

    fn remove(event: &str, matcher: Option<&str>, command: &str) -> Mutation {
        Mutation {
            op: HookOp::Remove,
            ..add(event, matcher, command)
        }
    }

    // -----------------------------------------------------------------------
    // 分组 JSON（claude / codex）
    // -----------------------------------------------------------------------

    #[test]
    fn grouped_json_add_keeps_the_rest_of_settings_json() {
        let raw = r#"{"editorMode":"vim","hooks":{"Stop":[{"hooks":[{"type":"command","command":"keep"}]}]}}"#;
        let out = edit_grouped_json(raw, &[add("PreToolUse", Some("Bash"), "mine")]).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        // settings.json 里绝大部分跟 hook 无关，动一条 hook 不能把别的吃掉。
        assert_eq!(back["editorMode"], "vim");
        assert_eq!(back["hooks"]["Stop"][0]["hooks"][0]["command"], "keep");
        assert_eq!(back["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(back["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "mine");
        assert_eq!(back["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 5);
    }

    #[test]
    fn grouped_json_remove_takes_only_the_named_handler() {
        let raw = r#"{"hooks":{"Stop":[{"hooks":[
            {"type":"command","command":"mine"},
            {"type":"command","command":"keep"}]}]}}"#;
        let out = edit_grouped_json(raw, &[remove("Stop", None, "mine")]).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        let items = back["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["command"], "keep");
    }

    /// 删空之后不留 `{"hooks": []}` 这种空壳：面板照着渲染就是「这个事件配着东西」。
    #[test]
    fn grouped_json_remove_leaves_no_empty_shell() {
        let raw = r#"{"editorMode":"vim","hooks":{"Stop":[{"hooks":[{"type":"command","command":"mine"}]}]}}"#;
        let out = edit_grouped_json(raw, &[remove("Stop", None, "mine")]).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(back.get("hooks").is_none(), "{out}");
        assert_eq!(back["editorMode"], "vim");
    }

    /// matcher 不同就是两条不同的 hook。不比 matcher 的话，删 `Bash` 那条会把
    /// `Write` 那条一起带走。
    #[test]
    fn grouped_json_remove_respects_the_matcher() {
        let raw = r#"{"hooks":{"PreToolUse":[
            {"matcher":"Bash","hooks":[{"command":"x"}]},
            {"matcher":"Write","hooks":[{"command":"x"}]}]}}"#;
        let out = edit_grouped_json(raw, &[remove("PreToolUse", Some("Bash"), "x")]).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        let groups = back["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["matcher"], "Write");
    }

    #[test]
    fn a_file_without_a_hooks_key_grows_one() {
        for raw in ["{}", "", r#"{"a":1}"#] {
            let out = edit_grouped_json(raw, &[add("Stop", None, "x")]).unwrap();
            let back: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(back["hooks"]["Stop"][0]["hooks"][0]["command"], "x", "{raw:?}");
        }
    }

    // -----------------------------------------------------------------------
    // agy
    // -----------------------------------------------------------------------

    #[test]
    fn agy_writes_under_one_name_and_leaves_other_names_alone() {
        let raw = r#"{"someone-else":{"Stop":[{"type":"command","command":"keep"}]}}"#;
        let out = edit_agy_json(raw, &[add("PreInvocation", None, "mine")]).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back["someone-else"]["Stop"][0]["command"], "keep");
        assert_eq!(back[AGY_GROUP]["PreInvocation"][0]["command"], "mine");
    }

    /// 删除按**命令**扫全部名字：面板上那条 hook 可能是别人放在别的名字底下的，
    /// 只扫我们自己那个名字就会「点了删除什么都没发生」。
    #[test]
    fn agy_remove_reaches_hooks_under_someone_elses_name() {
        let raw = r#"{"someone-else":{"Stop":[{"command":"gone"},{"command":"keep"}]}}"#;
        let out = edit_agy_json(raw, &[remove("Stop", None, "gone")]).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        let items = back["someone-else"]["Stop"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["command"], "keep");
    }

    // -----------------------------------------------------------------------
    // TOML
    // -----------------------------------------------------------------------

    #[test]
    fn toml_grouped_add_keeps_comments_and_other_settings() {
        let raw = "# 我的配置\nmodel = \"grok-4\"\n\n[hooks]\n\n[[hooks.Stop]]\nhooks = [{ type = \"command\", command = \"keep\" }]\n";
        let out = edit_toml_grouped(raw, &[add("UserPromptSubmit", None, "mine")]).unwrap();
        assert!(out.contains("# 我的配置"), "{out}");
        assert!(out.contains("model = \"grok-4\""), "{out}");
        let doc = out.parse::<toml_edit::Document>().unwrap();
        assert!(doc["hooks"]["Stop"].as_array_of_tables().is_some());
        let added = doc["hooks"]["UserPromptSubmit"].as_array_of_tables().unwrap();
        assert_eq!(added.len(), 1);
    }

    #[test]
    fn toml_grouped_remove_drops_the_event_when_it_empties() {
        let raw = "[hooks]\n\n[[hooks.Stop]]\nhooks = [{ type = \"command\", command = \"mine\" }]\n";
        let out = edit_toml_grouped(raw, &[remove("Stop", None, "mine")]).unwrap();
        let doc = out.parse::<toml_edit::Document>().unwrap();
        assert!(doc.get("hooks").is_none(), "{out}");
    }

    #[test]
    fn toml_list_add_and_remove_match_on_event_plus_command() {
        let raw = "[[hooks]]\nevent = \"Stop\"\ncommand = \"keep\"\n";
        let added = edit_toml_list(raw, &[add("TurnStarted", None, "mine")]).unwrap();
        let doc = added.parse::<toml_edit::Document>().unwrap();
        assert_eq!(doc["hooks"].as_array_of_tables().unwrap().len(), 2);

        let back = edit_toml_list(&added, &[remove("TurnStarted", None, "mine")]).unwrap();
        let doc = back.parse::<toml_edit::Document>().unwrap();
        let list = doc["hooks"].as_array_of_tables().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list.iter().next().unwrap()["command"].as_str(), Some("keep"));
    }

    // -----------------------------------------------------------------------
    // 保护与拦截
    // -----------------------------------------------------------------------

    /// 回合信号一律不许在这儿动 —— 删掉之后 GUI 聊天界面收不到回合结束事件，
    /// 而用户完全看不出因果。这条判定必须**在别的检查之前**，任何路径都绕不过去。
    #[test]
    fn the_turn_signal_hook_is_refused_before_anything_else_is_checked() {
        let script = crate::turn::signal_file_path()
            .unwrap()
            .with_file_name("turn-signal-hook.cjs");
        let command = format!("node \"{}\" claude started x", script.display());
        assert!(crate::turn::is_turn_hook_command(&command));

        let report = apply(
            &[HookEdit {
                // agent 故意写一个不存在的：受保护要比「这家不支持」先命中，
                // 否则报出来的原因就把真正的理由盖掉了。
                agent: "nope".into(),
                op: HookOp::Remove,
                event: "Stop".into(),
                matcher: None,
                command,
                timeout: None,
            }],
            None,
            true,
        )
        .unwrap();
        assert!(report.steps.is_empty());
        assert_eq!(report.blocked.len(), 1);
        assert_eq!(report.blocked[0].reason, HookBlockReason::Protected);
    }

    /// 往这家不认的事件上装 hook：写得进去，永远不会触发，用户会去查自己的命令。
    #[test]
    fn an_event_the_agent_does_not_know_is_refused() {
        let report = apply(
            &[HookEdit {
                agent: "codex".into(),
                op: HookOp::Add,
                // codex 的事件枚举里没有 Notification（claude 才有）。
                event: "Notification".into(),
                matcher: None,
                command: "echo hi".into(),
                timeout: None,
            }],
            None,
            true,
        )
        .unwrap();
        assert!(report.steps.is_empty());
        assert_eq!(report.blocked[0].reason, HookBlockReason::UnknownEvent);
    }

    #[test]
    fn an_empty_command_is_refused() {
        let report = apply(
            &[HookEdit {
                agent: "claude".into(),
                op: HookOp::Add,
                event: "Stop".into(),
                matcher: None,
                command: "   ".into(),
                timeout: None,
            }],
            None,
            true,
        )
        .unwrap();
        assert_eq!(report.blocked[0].reason, HookBlockReason::EmptyCommand);
    }

    #[test]
    fn an_agent_without_hooks_is_refused() {
        for agent in ["opencode", "pi"] {
            let report = apply(
                &[HookEdit {
                    agent: agent.into(),
                    op: HookOp::Add,
                    event: "Stop".into(),
                    matcher: None,
                    command: "echo hi".into(),
                    timeout: None,
                }],
                None,
                true,
            )
            .unwrap();
            assert_eq!(
                report.blocked[0].reason,
                HookBlockReason::Unsupported,
                "{agent}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 试跑
    // -----------------------------------------------------------------------

    #[test]
    fn a_dry_run_feeds_the_payload_on_stdin_and_reports_the_exit_code() {
        let out = test("cat; exit 0", "Stop", None).unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(!out.timed_out);
        // hook 收到的就是 stdin 上那份 JSON。看得见它，用户才查得出字段名写错在哪。
        assert!(out.stdout.contains("\"hook_event_name\":\"Stop\""), "{out:?}");
        assert!(out.payload.contains("tools-dry-run"));
    }

    /// 退出码 2 是各家约定的「拦住这一步」。不单独标出来的话，用户看到的是
    /// 「exit 2」这么一个数字。
    #[test]
    fn exit_code_two_is_reported_as_blocking() {
        let out = test("echo nope >&2; exit 2", "PreToolUse", None).unwrap();
        assert_eq!(out.exit_code, Some(2));
        assert!(out.blocking);
        assert!(out.stderr.contains("nope"));
    }

    /// 命令串里有 `&&` / `||` / `$VAR`，必须过 shell —— 直接 spawn 会把整串当成一个
    /// 可执行文件名。本机真实的 hook 就长这样。
    #[test]
    fn the_command_goes_through_a_shell() {
        let out = test("true && echo yes || echo no", "Stop", None).unwrap();
        assert_eq!(out.stdout.trim(), "yes");
    }

    #[test]
    fn an_empty_command_cannot_be_tested() {
        assert!(test("  ", "Stop", None).is_err());
    }

    /// 「在不在」这道闸两个方向都要有。
    ///
    /// 少了 `Add` 那一半，重复导入一份配置集就会把同一个 hook 挂两遍，每次事件触发
    /// 两次 —— 而扫描按命令归并，列表上根本看不出来，用户只会觉得"怎么跑了两回"。
    #[test]
    fn adding_something_that_is_already_there_is_refused() {
        assert_eq!(
            presence_block(HookOp::Add, true),
            Some(HookBlockReason::AlreadyThere)
        );
        assert_eq!(presence_block(HookOp::Add, false), None);
    }

    #[test]
    fn removing_something_that_is_not_there_is_refused() {
        assert_eq!(
            presence_block(HookOp::Remove, false),
            Some(HookBlockReason::NotInWritableSource)
        );
        assert_eq!(presence_block(HookOp::Remove, true), None);
    }
}
