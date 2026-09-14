//! Codex 账号额度（5 小时 / 周）—— 数据源是 `codex app-server` 的
//! `account/rateLimits/read` JSON-RPC，不是自己去打 OpenAI 后端接口。
//!
//! 为什么借 codex 的手：那个额度接口要 ChatGPT 订阅的 OAuth access_token，而这个
//! token 会过期、由 codex 自己在后台刷新（写回 `~/.codex/auth.json`）。让 codex 去读，
//! 就不用在这里复刻它的刷新逻辑，端点换了也跟着走。对照 Claude 那边（`usage_api.rs`
//! 直接 curl OAuth 接口）—— 那是因为 Claude CLI 没有这样一个可编程的读口子。
//!
//! 只对「官方订阅登录」成立：`auth.json` 的 `auth_mode == "chatgpt"`、没有
//! `OPENAI_API_KEY`，且 `config.toml` 里没接第三方 provider。用 API key 计费不共享
//! 这两个窗口，此时一律报错让前端保持现状（不显示徽标）。
//!
//! 取数是一个短命进程（实测 ~2.2s，其中 ~1s 是 login shell），所以前端只 60s 慢轮询 +
//! 每轮对话结束事件驱动，进程内再压一层 20s TTL 缓存兜住密集调用。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::agent_command::AgentCommand;
use crate::util;

/// 进程内缓存有效期：前端按 ~60s 轮询，这里 20s 兜住偶发的密集调用（每次都要起进程）。
const CACHE_TTL: Duration = Duration::from_secs(20);
/// 从起进程到拿到额度的总预算。实测 ~2.2s；给到 25s 是留给冷启动 / 网络慢的余量。
const DEADLINE: Duration = Duration::from_secs(25);

/// 单个额度窗口。`window_minutes` 决定前端标签（300 = 5h，10080 = 周）。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageWindow {
    /// 已用百分比 0–100。
    pub used_percent: f64,
    /// 窗口长度（分钟）。plus/pro 是 300 / 10080，其它套餐可能不同，故原样带出。
    pub window_minutes: u32,
    /// ISO8601 重置时刻（app-server 给的是 unix 秒，这里转成和 Claude 侧同形的字符串，
    /// 前端复用同一个倒计时格式化函数）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

/// 账号额度快照：`primary` = 短窗口（5 小时），`secondary` = 长窗口（7 天）。
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexAccountUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<CodexUsageWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<CodexUsageWindow>,
}

fn codex_home() -> std::path::PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| util::home().join(".codex"))
}

/// `~/.codex/auth.json` 是否是「官方 ChatGPT 订阅登录」。
///
/// 判据：没有 `OPENAI_API_KEY`（有就是 API key 计费）、`auth_mode` 若存在必须是
/// `chatgpt`、且确实存过 OAuth token。老版本 codex 不写 `auth_mode`，故缺省放行。
pub fn is_subscription_login(auth: &Value) -> bool {
    let api_key = auth
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !api_key.trim().is_empty() {
        return false;
    }
    if let Some(mode) = auth.get("auth_mode").and_then(Value::as_str) {
        if mode != "chatgpt" {
            return false;
        }
    }
    auth.pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.trim().is_empty())
}

/// `config.toml` 顶层写了 `model_provider` = 用户接了第三方端点 / API key
/// （官方默认根本不写这行）。此时额度窗口不属于这个账号，一律不显示。
pub fn uses_custom_provider(config_toml: &str) -> bool {
    config_toml.lines().any(|line| {
        let line = line.trim();
        line.starts_with("model_provider")
            && line.contains('=')
            && !line.starts_with('#')
            && !line.starts_with('[')
    })
}

/// 这台机器上的 Codex 现在是不是「官方订阅登录」—— 决定额度窗口**适不适用**。
///
/// 和「这次没取到」是两回事：不适用是个确定的结论（用户切了第三方 API key / provider，
/// 或者退登后 auth.json 根本不在了），调用方要据此把徽标连同缓存一起抹掉，而不是
/// 回退到上一次成功值。纯文件读，不起进程，~1ms。
fn subscription_applies() -> bool {
    let home = codex_home();
    let auth: Value = std::fs::read_to_string(home.join("auth.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null);
    if !is_subscription_login(&auth) {
        return false;
    }
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
    !uses_custom_provider(&config)
}

/// 把 `account/rateLimits/read` 的 result 整理成快照。两个窗口都缺 → None（当没读到）。
fn parse_rate_limits(result: &Value) -> Option<CodexAccountUsage> {
    let limits = result.get("rateLimits")?;
    let usage = CodexAccountUsage {
        primary: parse_window(limits.get("primary")),
        secondary: parse_window(limits.get("secondary")),
    };
    (usage.primary.is_some() || usage.secondary.is_some()).then_some(usage)
}

fn parse_window(window: Option<&Value>) -> Option<CodexUsageWindow> {
    let window = window?;
    let used_percent = window.get("usedPercent").and_then(Value::as_f64)?;
    Some(CodexUsageWindow {
        used_percent,
        window_minutes: window
            .get("windowDurationMins")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        resets_at: window
            .get("resetsAt")
            .and_then(Value::as_i64)
            .map(|secs| util::format_iso8601_utc(secs, 0)),
    })
}

/// 起一个短命 `codex app-server`，握手后读一次额度，然后连同它的后代一起收掉。
fn fetch_blocking() -> Result<CodexAccountUsage, String> {
    let cwd = util::home().to_string_lossy().to_string();
    let command = AgentCommand::new("codex").arg("app-server");
    let mut cmd = crate::agent_chat::build_piped_command(&cwd, &command, false, false);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // 这是个全局闸门（关停时用），只圈住「创建 + 登记」这一小段；后面 ~2s 的 RPC
    // 等待绝不能占着它，否则整个 app 的子进程创建都排在这后面。
    let permit = crate::runtime::spawn_permit()?;
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn codex app-server: {e}"))?;
    let pid = child.id();
    let _ = crate::process_tree::register(pid);
    drop(permit);
    let result = read_rate_limits(&mut child);
    // 外层是 login shell，真正的 codex 是它的孩子：按进程树收，别留孤儿。
    crate::process_tree::terminate(pid);
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// initialize → initialized → account/rateLimits/read，全程受 DEADLINE 约束。
fn read_rate_limits(child: &mut Child) -> Result<CodexAccountUsage, String> {
    let mut stdin = child.stdin.take().ok_or("codex app-server stdin 不可用")?;
    let stdout = child.stdout.take().ok_or("codex app-server stdout 不可用")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if tx.send(value).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + DEADLINE;

    write_line(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "cc-sessions-viewer",
                    "title": "Claude Session Viewer",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            },
        }),
    )?;
    wait_response(&rx, 1, deadline)?;

    write_line(
        &mut stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
    )?;
    write_line(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "account/rateLimits/read",
            "params": {},
        }),
    )?;
    let result = wait_response(&rx, 2, deadline)?;
    parse_rate_limits(&result).ok_or_else(|| "额度响应里没有窗口数据".to_string())
}

fn write_line(stdin: &mut impl Write, value: &Value) -> Result<(), String> {
    let mut line = serde_json::to_string(value).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .and_then(|()| stdin.flush())
        .map_err(|e| format!("write app-server: {e}"))
}

/// 等指定 id 的响应；中途的通知（无 id）直接丢掉。超时 / 进程提前退出都报错。
fn wait_response(
    rx: &mpsc::Receiver<Value>,
    id: u64,
    deadline: Instant,
) -> Result<Value, String> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| format!("codex app-server 响应超时（{}）", id))?;
        let value = match rx.recv_timeout(remaining) {
            Ok(value) => value,
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!("codex app-server 响应超时（{}）", id))
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err("codex app-server 提前退出".into())
            }
        };
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("codex app-server error");
            return Err(message.to_string());
        }
        return value
            .get("result")
            .cloned()
            .ok_or_else(|| "codex app-server 响应缺少 result".to_string());
    }
}

fn cache() -> &'static Mutex<Option<(Instant, CodexAccountUsage)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, CodexAccountUsage)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 取额度快照。三种结局刻意分开，别混成一个 Err：
/// - `Ok(Some(_))` 拿到了（命中 20s TTL 缓存直接返回；`force=true` 跳过缓存读取但仍回写）；
/// - `Ok(None)` 额度窗口**不适用**（切了第三方 API key / provider、或已退登）。这是确定结论，
///   顺手把缓存清掉，调用方据此抹掉徽标 —— 绝不能回退旧值，那会让切走之后还挂着上一个
///   账号的百分比；
/// - `Err(_)` 这次**没取到**（进程起不来 / 超时）。回退「上一次成功值」，徽标宁可陈旧也不闪空。
pub fn codex_account_usage_blocking(force: bool) -> Result<Option<CodexAccountUsage>, String> {
    if !subscription_applies() {
        if let Ok(mut guard) = cache().lock() {
            *guard = None;
        }
        return Ok(None);
    }
    if !force {
        if let Ok(guard) = cache().lock() {
            if let Some((at, ref usage)) = *guard {
                if at.elapsed() < CACHE_TTL {
                    return Ok(Some(usage.clone()));
                }
            }
        }
    }
    match fetch_blocking() {
        Ok(usage) => {
            if let Ok(mut guard) = cache().lock() {
                *guard = Some((Instant::now(), usage.clone()));
            }
            Ok(Some(usage))
        }
        Err(e) => {
            if let Ok(guard) = cache().lock() {
                if let Some((_, ref usage)) = *guard {
                    return Ok(Some(usage.clone()));
                }
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chatgpt_auth() -> Value {
        serde_json::json!({
            "OPENAI_API_KEY": Value::Null,
            "auth_mode": "chatgpt",
            "tokens": { "access_token": "eyJhb", "account_id": "acc" },
        })
    }

    #[test]
    fn chatgpt_login_without_api_key_counts_as_subscription() {
        assert!(is_subscription_login(&chatgpt_auth()));
    }

    #[test]
    fn an_api_key_in_auth_json_is_not_a_subscription() {
        let mut auth = chatgpt_auth();
        auth["OPENAI_API_KEY"] = serde_json::json!("sk-live");
        assert!(!is_subscription_login(&auth));
    }

    #[test]
    fn a_non_chatgpt_auth_mode_is_not_a_subscription() {
        let mut auth = chatgpt_auth();
        auth["auth_mode"] = serde_json::json!("apikey");
        assert!(!is_subscription_login(&auth));
    }

    #[test]
    fn an_older_auth_json_without_auth_mode_still_counts() {
        let auth = serde_json::json!({ "tokens": { "access_token": "eyJhb" } });
        assert!(is_subscription_login(&auth));
    }

    #[test]
    fn missing_or_blank_tokens_are_not_a_subscription() {
        assert!(!is_subscription_login(&Value::Null));
        assert!(!is_subscription_login(
            &serde_json::json!({ "auth_mode": "chatgpt", "tokens": { "access_token": "  " } })
        ));
    }

    #[test]
    fn a_top_level_model_provider_means_a_custom_endpoint() {
        assert!(uses_custom_provider("model_provider = \"custom\"\n"));
        assert!(!uses_custom_provider("model = \"gpt-5.4\"\n"));
        assert!(!uses_custom_provider("# model_provider = \"custom\"\n"));
        assert!(!uses_custom_provider(
            "[model_providers.custom]\nbase_url = \"x\"\n"
        ));
    }

    #[test]
    fn rate_limits_map_both_windows_with_iso_reset_times() {
        let result = serde_json::json!({
            "rateLimits": {
                "primary": { "usedPercent": 3.0, "windowDurationMins": 300, "resetsAt": 1_788_779_373i64 },
                "secondary": { "usedPercent": 1.5, "windowDurationMins": 10080, "resetsAt": 1_789_366_173i64 },
            },
        });
        let usage = parse_rate_limits(&result).expect("windows");
        let primary = usage.primary.expect("primary");
        assert_eq!(primary.used_percent, 3.0);
        assert_eq!(primary.window_minutes, 300);
        assert_eq!(
            primary.resets_at.as_deref(),
            Some("2026-09-07T11:09:33.000Z")
        );
        let secondary = usage.secondary.expect("secondary");
        assert_eq!(secondary.used_percent, 1.5);
        assert_eq!(secondary.window_minutes, 10080);
    }

    #[test]
    fn a_window_without_a_reset_time_still_reports_its_percentage() {
        let result = serde_json::json!({
            "rateLimits": { "primary": { "usedPercent": 42.0, "windowDurationMins": 300 } },
        });
        let usage = parse_rate_limits(&result).expect("windows");
        let primary = usage.primary.expect("primary");
        assert_eq!(primary.used_percent, 42.0);
        assert!(primary.resets_at.is_none());
        assert!(usage.secondary.is_none());
    }

    #[test]
    fn a_response_without_any_window_is_not_a_snapshot() {
        assert!(parse_rate_limits(&serde_json::json!({ "rateLimits": {} })).is_none());
        assert!(parse_rate_limits(&serde_json::json!({ "ordinaryUsageAllowed": true })).is_none());
    }
}
