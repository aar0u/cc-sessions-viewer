//! MCP 配置的归一化模型 + 全机器扫描。
//!
//! 三种文件格式、七家 agent、最多六档来源，但这个模块里**一行 agent 判断都没有**：
//! 格式挂在 [`McpSource::format`](super::McpSource) 上，这里只按格式分派。想知道某家
//! 读哪些文件，去看 `mod.rs` 里它自己的 `mcp_sources`，那儿每一条都写了依据。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{surface, McpFormat, McpSource, AGENTS};

/// 传输方式。
///
/// `Unknown` 是给「认不出来」留的，不是给「没写」留的：没写 `type` 但有 `command` 的
/// 就是 stdio（七家都这么默认），而写了个我们没见过的值必须如实说不认识 —— 猜成
/// stdio 会让面板显示一条根本起不来的命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpTransport {
    Stdio,
    Http,
    Sse,
    Ws,
    Unknown,
}

/// 一条环境变量 / HTTP header。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpVar {
    pub key: String,
    pub value: String,
    /// 看着像凭据。UI 默认打码，要看得点一下。
    ///
    /// 本机 `~/.claude.json` 里就躺着一个明文 access token —— 面板一打开就把它摊在
    /// 屏幕上，用户截个图发出去就泄了。宁可对几个无害的变量多打一次码。
    pub secret: bool,
}

/// 键名看着像不像凭据。
///
/// 只看键名不看值：值的形状没有可靠特征（一个 32 位十六进制串可能是 token 也可能是
/// 一个 commit id），而键名是人写给人看的，反而稳定。
pub(super) fn looks_secret(key: &str) -> bool {
    // 针都不带分隔符，比对前把键上的 `-` / `_` / 空格一并抹掉。照字面比的话
    // `X-API-Key`（Anthropic 自己的 header 名）既不含 `APIKEY` 也不含 `API_KEY`，
    // 一条明文密钥就直接摊在详情页上了。
    const NEEDLES: &[&str] = &[
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "APIKEY",
        "ACCESSKEY",
        "PRIVATEKEY",
        "CREDENTIAL",
        "AUTH",
        "COOKIE",
        "SESSIONID",
        "BEARER",
    ];
    let squashed: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    NEEDLES.iter().any(|n| squashed.contains(n))
}

/// URL 里能确认是凭据的那几处，打码后的样子。
///
/// 和 [`looks_secret`] 同一条规矩：**只认键名，不猜值**。所以只动两处 ——
/// `user:pass@` 那段 userinfo，和名字像凭据的 query 参数。主机和路径原样留着，
/// 否则详情页上认不出这是哪个 server。
///
/// 路径里的 token（`https://mcp.example.com/<token>/sse` 这种托管服务的写法）**不动**：
/// 判断一段路径是不是密钥只能靠猜形状，猜错就是把正常路径打成一片点。
pub(super) fn mask_url(url: &str) -> String {
    let (scheme, rest) = match url.find("://") {
        Some(i) => url.split_at(i + 3),
        None => ("", url),
    };
    let (authority, tail) = match rest.find(['/', '?', '#']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let authority = match authority.rfind('@') {
        Some(i) => {
            let user = &authority[..i];
            let name = user.split_once(':').map(|(n, _)| n).unwrap_or(user);
            format!("{name}:{MASK}@{}", &authority[i + 1..])
        }
        None => authority.to_string(),
    };
    let (path, query) = match tail.find('?') {
        Some(i) => (&tail[..i], &tail[i + 1..]),
        None => (tail, ""),
    };
    if query.is_empty() {
        return format!("{scheme}{authority}{path}");
    }
    let masked: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if looks_secret(k) => format!("{k}={MASK}"),
            _ => pair.to_string(),
        })
        .collect();
    format!("{scheme}{authority}{path}?{}", masked.join("&"))
}

/// 打码后填进去的那串。和前端 `maskValue` 一样固定长度 —— 点的个数会泄漏长度。
const MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}";

/// 一个 server 在**某一个文件里**的定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerDef {
    pub name: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    /// `url` 里能确认是凭据的那几处打了码的样子。详情页默认显示这一份。
    ///
    /// 托管 MCP 的接入地址常常自带密钥（`https://user:tok@host/…`、`?api_key=…`），
    /// 而 URL 这一栏原来是原样渲染的 —— env 打了码、URL 没打，等于留了个后门。
    pub url_masked: Option<String>,
    pub env: Vec<McpVar>,
    pub headers: Vec<McpVar>,
    pub cwd: Option<String>,
    /// 显式关掉的（codex / opencode 的 `enabled = false`、cursor 的 `disabled = true`）。
    pub enabled: bool,
    /// 认不出形状：既没有 `command` 也没有 `url`。
    ///
    /// 本机 `~/.claude.json` 里的一条就是这样（只有 `env`），它在 Claude 那边根本起
    /// 不来。面板要如实标出来，不能画成一条正常的 server —— 那等于告诉用户「它在跑」。
    pub incomplete: bool,
}

impl McpServerDef {
    /// 去重用的指纹：**跑起来的到底是什么**。
    ///
    /// 只看命令行 / URL，不看名字 —— 同一个 server 在不同 agent 里叫不同名字是常事
    /// （本机 `@hypothesi/tauri-mcp-server` 和 `tauri-mcp-server` 就是同一个）。反过来
    /// 同名不同指纹就是冲突，面板要让用户选一个。
    ///
    /// 不做「聪明」的归一化（比如把 `npx -y foo` 和 `npx foo` 当成一个）：那是在猜
    /// 语义，猜错了就是把两个真不一样的 server 合并掉。只做去空白。
    pub fn fingerprint(&self) -> String {
        if let Some(url) = &self.url {
            return format!("url\u{1f}{}", url.trim());
        }
        let mut parts = vec![self.command.clone().unwrap_or_default()];
        parts.extend(self.args.iter().cloned());
        format!(
            "cmd\u{1f}{}",
            parts
                .iter()
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("\u{1f}")
        )
    }
}

/// 一个来源文件的解析结果。
///
/// 解析失败**不吞**：本机随便哪个配置文件手改坏一个逗号，这个 agent 就会整片少掉
/// server。报成 `error` 让面板说「这个文件读不了」，比静悄悄少几行诚实得多。
#[derive(Debug, Clone, Default)]
pub struct SourceRead {
    pub servers: Vec<McpServerDef>,
    pub error: Option<String>,
}

/// 读一个来源里的全部 server。
///
/// `cwd` 只有 [`McpFormat::JsonProjectServers`] 用得上 —— `~/.claude.json` 的
/// `projects` 是按**启动目录的绝对路径**分区的，没有 cwd 就不知道该看哪一格。
pub fn read_source(source: &McpSource, cwd: Option<&Path>) -> SourceRead {
    let path = PathBuf::from(&source.path);
    if !path.is_file() {
        return SourceRead::default();
    }
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            return SourceRead {
                servers: Vec::new(),
                error: Some(format!("读不了：{e}")),
            }
        }
    };
    match source.format {
        McpFormat::JsonServers => json_servers(&raw, &["mcpServers"]),
        McpFormat::JsonProjectServers => {
            let Some(cwd) = cwd else {
                return SourceRead::default();
            };
            json_servers(
                &raw,
                &["projects", &cwd.to_string_lossy(), "mcpServers"],
            )
        }
        McpFormat::OpencodeJson => opencode_servers(&raw),
        McpFormat::TomlServers => toml_servers(&raw),
    }
}

// ---------------------------------------------------------------------------
// 三种格式的解析
// ---------------------------------------------------------------------------

fn json_root(raw: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str::<serde_json::Value>(raw).map_err(|e| format!("不是合法的 JSON：{e}"))
}

/// 顺着 `at` 一路往下取一个对象，任何一层缺席都当「这个文件里没有 MCP」。
fn json_servers(raw: &str, at: &[&str]) -> SourceRead {
    let root = match json_root(raw) {
        Ok(v) => v,
        Err(e) => {
            return SourceRead {
                servers: Vec::new(),
                error: Some(e),
            }
        }
    };
    let mut node = &root;
    for key in at {
        match node.get(*key) {
            Some(next) => node = next,
            None => return SourceRead::default(),
        }
    }
    let Some(map) = node.as_object() else {
        return SourceRead::default();
    };
    SourceRead {
        servers: map
            .iter()
            .map(|(name, entry)| json_entry(name, entry))
            .collect(),
        error: None,
    }
}

fn json_entry(name: &str, entry: &serde_json::Value) -> McpServerDef {
    let command = entry.get("command").and_then(|v| v.as_str()).map(str::to_string);
    // `httpUrl` 是 agy（Gemini CLI）给 streamable-http 用的键，`url` 在它那儿专指 SSE。
    // 不认的话，agy 里配的每一个 http server 都会被判成「既没 command 也没 url」的残缺
    // 条目。别家没有这个键，多认一个不会误伤。
    let url = entry
        .get("url")
        .or_else(|| entry.get("httpUrl"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let args = json_string_list(entry.get("args"));
    let env = json_vars(entry.get("env"));
    let headers = json_vars(entry.get("headers"));
    let declared = entry
        .get("type")
        .or_else(|| entry.get("transport"))
        .and_then(|v| v.as_str());
    // `disabled: true` 是 Cursor 那套的写法，`enabled: false` 是其它几家的。两个都认，
    // 因为通用的 `.mcp.json` 是同一个文件在几家之间传着用的。
    let enabled = entry.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true)
        && !entry.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false);
    McpServerDef {
        name: name.to_string(),
        transport: transport_of(declared, command.is_some(), url.is_some()),
        incomplete: command.is_none() && url.is_none(),
        command,
        args,
        url_masked: url.as_deref().map(mask_url),
        url,
        env,
        headers,
        cwd: entry.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
        enabled,
    }
}

/// opencode 的 `mcp` 键。
///
/// 依据是本机 `~/.config/opencode/node_modules/@opencode-ai/sdk` 的 `McpLocalConfig` /
/// `McpRemoteConfig`：`type: "local" | "remote"`、**`command` 是命令和参数混在一起的
/// 数组**、env 键叫 `environment`。七家里唯一需要拆数组的一家。
fn opencode_servers(raw: &str) -> SourceRead {
    let root = match json_root(raw) {
        Ok(v) => v,
        Err(e) => {
            return SourceRead {
                servers: Vec::new(),
                error: Some(e),
            }
        }
    };
    let Some(map) = root.get("mcp").and_then(|v| v.as_object()) else {
        return SourceRead::default();
    };
    let servers = map
        .iter()
        .map(|(name, entry)| {
            let argv = json_string_list(entry.get("command"));
            let (command, args) = match argv.split_first() {
                Some((head, rest)) => (Some(head.clone()), rest.to_vec()),
                None => (None, Vec::new()),
            };
            let url = entry.get("url").and_then(|v| v.as_str()).map(str::to_string);
            let declared = entry.get("type").and_then(|v| v.as_str());
            McpServerDef {
                name: name.clone(),
                // `local` / `remote` 不是传输方式，是「在不在本机」。本地就是 stdio，
                // 远端再看 url 判 http/sse。
                transport: match declared {
                    Some("local") => McpTransport::Stdio,
                    Some("remote") => transport_of(None, false, url.is_some()),
                    other => transport_of(other, command.is_some(), url.is_some()),
                },
                incomplete: command.is_none() && url.is_none(),
                command,
                args,
                url_masked: url.as_deref().map(mask_url),
                url,
                env: json_vars(entry.get("environment")),
                headers: json_vars(entry.get("headers")),
                cwd: entry.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
                enabled: entry.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
            }
        })
        .collect();
    SourceRead {
        servers,
        error: None,
    }
}

/// `[mcp_servers.<name>]` —— codex / grok。
fn toml_servers(raw: &str) -> SourceRead {
    let doc = match raw.parse::<toml_edit::Document>() {
        Ok(doc) => doc,
        Err(e) => {
            return SourceRead {
                servers: Vec::new(),
                error: Some(format!("不是合法的 TOML：{e}")),
            }
        }
    };
    let Some(table) = doc.get("mcp_servers").and_then(|i| i.as_table_like()) else {
        return SourceRead::default();
    };
    let servers = table
        .iter()
        .map(|(name, item)| {
            let command = toml_str(item.get("command"));
            let url = toml_str(item.get("url"));
            let declared = item
                .get("type")
                .or_else(|| item.get("transport"))
                .and_then(toml_as_str);
            McpServerDef {
                name: name.to_string(),
                transport: transport_of(declared, command.is_some(), url.is_some()),
                incomplete: command.is_none() && url.is_none(),
                command,
                args: toml_string_list(item.get("args")),
                url_masked: url.as_deref().map(mask_url),
                url,
                env: toml_vars(item.get("env")),
                headers: toml_vars(item.get("headers")),
                cwd: toml_str(item.get("cwd")),
                enabled: item
                    .get("enabled")
                    .and_then(|i| i.as_bool())
                    .unwrap_or(true),
            }
        })
        .collect();
    SourceRead {
        servers,
        error: None,
    }
}

fn transport_of(declared: Option<&str>, has_command: bool, has_url: bool) -> McpTransport {
    match declared.map(str::to_ascii_lowercase).as_deref() {
        Some("stdio") => McpTransport::Stdio,
        Some("http") | Some("streamable-http") | Some("streamable_http") => McpTransport::Http,
        Some("sse") => McpTransport::Sse,
        Some("ws") | Some("websocket") => McpTransport::Ws,
        Some(_) => McpTransport::Unknown,
        // 没写就按形状推：七家都是「有 command 即 stdio」。
        None if has_command => McpTransport::Stdio,
        None if has_url => McpTransport::Http,
        None => McpTransport::Unknown,
    }
}

fn json_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 值不是字符串的（数字、布尔）也收：进程环境里它们最终都是字符串，丢掉等于假装
/// 用户没配过。
fn json_vars(value: Option<&serde_json::Value>) -> Vec<McpVar> {
    value
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .map(|(key, value)| McpVar {
                    secret: looks_secret(key),
                    key: key.clone(),
                    value: match value {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

fn toml_as_str(item: &toml_edit::Item) -> Option<&str> {
    item.as_str()
}

fn toml_str(item: Option<&toml_edit::Item>) -> Option<String> {
    item.and_then(toml_as_str).map(str::to_string)
}

fn toml_string_list(item: Option<&toml_edit::Item>) -> Vec<String> {
    item.and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn toml_vars(item: Option<&toml_edit::Item>) -> Vec<McpVar> {
    item.and_then(|i| i.as_table_like())
        .map(|table| {
            table
                .iter()
                .map(|(key, value)| McpVar {
                    secret: looks_secret(key),
                    key: key.to_string(),
                    value: value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string().trim().to_string()),
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 扫描
// ---------------------------------------------------------------------------

/// 一份定义，连同它来自哪儿。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDefAt {
    pub agent: String,
    pub source: McpSource,
    pub def: McpServerDef,
    /// 这一份是不是这家 agent 实际生效的那一份。
    ///
    /// 同名 server 出现在同一家的多个来源里时只有优先级最高的那份算数，其余是被盖住
    /// 的。不标出来的话，面板会把一条早就被项目配置覆盖掉的 user 级定义画成「正在
    /// 用的」—— 用户改了它却什么都没变。
    pub effective: bool,
}

/// 一个 server（按名字归并）在全机器的样子。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEntry {
    pub name: String,
    pub defs: Vec<McpDefAt>,
    /// 真的会加载它的 agent：生效的那份且没被关掉。列表右侧那排角标就是它。
    pub agents: Vec<String>,
    /// 配置里有它、但被关掉或被盖住的 agent。
    pub inactive_agents: Vec<String>,
    /// 同名但指纹不止一种 —— 各家跑的根本不是同一个东西。
    pub conflict: bool,
    /// 生效的那些定义里出现过的指纹，去重后按出现顺序。冲突时 UI 拿它列「哪几种」。
    pub fingerprints: Vec<String>,
    /// 缓存里量到的工具数。`None` = 没量过，不是 0 —— 两者在预算条上的含义完全不同。
    pub tools: Option<usize>,
    /// 工具定义的 token 估算。见 [`estimate_tokens`]。
    pub tokens: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSummary {
    pub servers: usize,
    /// 至少在一家 agent 里生效的。
    pub active: usize,
    pub conflicts: usize,
    /// 生效集合的工具数与 token 估算合计；量不到的条目不计入（会另外标出来）。
    pub tools: usize,
    pub tokens: usize,
    /// 有工具数缓存的条目数。用户据此知道上面那两个数字覆盖了多少条。
    pub measured: usize,
}

/// 一家 agent 的来源清单 + 每个文件的解析结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAgentInfo {
    pub agent: String,
    pub installed: bool,
    pub supported: bool,
    /// 可写的那一条（`mcp_sources` 里 `writable` 为真的），没有就是这家没法改。
    pub write_path: Option<String>,
    pub sources: Vec<McpSourceInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSourceInfo {
    #[serde(flatten)]
    pub source: McpSource,
    pub servers: usize,
    /// 解析失败的原因。有值就说明这个文件整片没读进来。
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpScan {
    pub home: String,
    pub servers: Vec<McpEntry>,
    pub agents: Vec<McpAgentInfo>,
    pub summary: McpSummary,
}

/// 扫全机器的 MCP 配置。
///
/// 只读，不碰任何文件，也**不启动任何 server** —— 打开面板不该在用户机器上拉起十几个
/// `npx` 进程。工具数走缓存（见 [`tool_counts`]），要现场探测是另一个显式动作。
pub fn scan(cwd: Option<&Path>) -> McpScan {
    let counts = tool_counts();
    let mut agents: Vec<McpAgentInfo> = Vec::new();
    // 名字 → 这个名字下的全部定义。BTreeMap 保证列表顺序稳定（按名字），不然每次
    // 扫描顺序都在跳。
    let mut grouped: BTreeMap<String, Vec<McpDefAt>> = BTreeMap::new();

    for agent in AGENTS {
        let Ok(s) = surface(agent) else { continue };
        let installed = s.config_home().is_some_and(|p| p.is_dir());
        let sources = s.mcp_sources(cwd);
        let supported = s.capabilities().mcp;
        let write_path = sources
            .iter()
            .find(|src| src.writable)
            .map(|src| src.path.clone());

        let mut infos = Vec::new();
        // 同一家里，优先级高的先来，第一次见到的名字就是生效的那份。
        let mut ordered: Vec<&McpSource> = sources.iter().collect();
        ordered.sort_by_key(|s| std::cmp::Reverse(s.precedence));
        let mut seen: Vec<String> = Vec::new();
        for source in ordered {
            let read = read_source(source, cwd);
            infos.push(McpSourceInfo {
                source: source.clone(),
                servers: read.servers.len(),
                error: read.error,
            });
            for def in read.servers {
                let effective = !seen.contains(&def.name);
                if effective {
                    seen.push(def.name.clone());
                }
                grouped.entry(def.name.clone()).or_default().push(McpDefAt {
                    agent: (*agent).to_string(),
                    source: source.clone(),
                    def,
                    effective,
                });
            }
        }
        // 展示顺序按 `mcp_sources` 自己报的顺序，不按上面排序后的 —— 各家的文档就是
        // 那个顺序写的，改了反而对不上。
        infos.sort_by_key(|i| std::cmp::Reverse(i.source.precedence));
        agents.push(McpAgentInfo {
            agent: (*agent).to_string(),
            installed,
            supported,
            write_path,
            sources: infos,
        });
    }

    let servers: Vec<McpEntry> = grouped
        .into_iter()
        .map(|(name, defs)| entry(name, defs, &counts))
        .collect();

    let summary = McpSummary {
        servers: servers.len(),
        active: servers.iter().filter(|s| !s.agents.is_empty()).count(),
        conflicts: servers.iter().filter(|s| s.conflict).count(),
        tools: servers
            .iter()
            .filter(|s| !s.agents.is_empty())
            .filter_map(|s| s.tools)
            .sum(),
        tokens: servers
            .iter()
            .filter(|s| !s.agents.is_empty())
            .filter_map(|s| s.tokens)
            .sum(),
        measured: servers
            .iter()
            .filter(|s| !s.agents.is_empty() && s.tools.is_some())
            .count(),
    };

    McpScan {
        home: crate::util::home().to_string_lossy().to_string(),
        servers,
        agents,
        summary,
    }
}

fn entry(name: String, defs: Vec<McpDefAt>, counts: &BTreeMap<String, ToolCount>) -> McpEntry {
    let mut agents = Vec::new();
    let mut inactive = Vec::new();
    for d in &defs {
        let list = if d.effective && d.def.enabled {
            &mut agents
        } else {
            &mut inactive
        };
        if !list.contains(&d.agent) {
            list.push(d.agent.clone());
        }
    }
    // 一家 agent 只要有一份生效的就算「开着」，别的来源里那份被盖住的不该再把它算进
    // 「关着」那一栏。
    inactive.retain(|a| !agents.contains(a));

    let mut fingerprints: Vec<String> = Vec::new();
    for d in defs.iter().filter(|d| d.effective) {
        let fp = d.def.fingerprint();
        if !fingerprints.contains(&fp) {
            fingerprints.push(fp);
        }
    }

    let count = counts.get(&name);
    McpEntry {
        conflict: fingerprints.len() > 1,
        fingerprints,
        agents,
        inactive_agents: inactive,
        tools: count.map(|c| c.tools),
        tokens: count.map(|c| c.tokens),
        name,
        defs,
    }
}

// ---------------------------------------------------------------------------
// 工具数与 token 估算
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct ToolCount {
    pub tools: usize,
    pub tokens: usize,
}

/// 一个工具定义大约占多少 token。
///
/// 按 **4 字符 ≈ 1 token** 估。这是英文散文加 JSON 的常见比值，不是精确分词 —— 精确
/// 值要跑一遍各家自己的 tokenizer，而它们互不相同，为一个「大概多少」的条子引三个
/// 分词器不值得。面板上这个数字永远带「约」字。
fn estimate_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

/// 从各家已有的缓存里读工具数，**不启动任何进程**。
///
/// 目前两个来源：
/// - pi 的 `~/.pi/agent/mcp-cache.json`：`{ servers: { <name>: { tools: [...] } } }`，
///   工具的 `name` / `description` / `inputSchema` 全都在，是七家里唯一现成能算准的。
/// - agy 的 `~/.gemini/antigravity-cli/mcp/<server>/<tool>.json`：一个工具一个文件。
///
/// 同名以 pi 那份为准（它带完整 schema）。都没有就是 `None` —— 面板显示「未测量」，
/// 不显示 0。
pub fn tool_counts() -> BTreeMap<String, ToolCount> {
    let mut out = BTreeMap::new();
    for (name, count) in agy_tool_counts() {
        out.insert(name, count);
    }
    for (name, count) in pi_tool_counts() {
        out.insert(name, count);
    }
    out
}

fn pi_tool_counts() -> Vec<(String, ToolCount)> {
    let path = super::pi_agent_dir().join("mcp-cache.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(servers) = root.get("servers").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    servers
        .iter()
        .filter_map(|(name, entry)| {
            let tools = entry.get("tools")?.as_array()?;
            let chars: usize = tools.iter().map(|t| t.to_string().chars().count()).sum();
            Some((
                name.clone(),
                ToolCount {
                    tools: tools.len(),
                    tokens: estimate_tokens(chars),
                },
            ))
        })
        .collect()
}

fn agy_tool_counts() -> Vec<(String, ToolCount)> {
    let root = crate::util::home()
        .join(".gemini")
        .join("antigravity-cli")
        .join("mcp");
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|dir| {
            let name = dir.file_name().to_string_lossy().to_string();
            let files = fs::read_dir(dir.path()).ok()?;
            let mut tools = 0usize;
            let mut chars = 0usize;
            for file in files.filter_map(|f| f.ok()) {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(raw) = fs::read_to_string(&path) {
                    tools += 1;
                    chars += raw.chars().count();
                }
            }
            (tools > 0).then_some((
                name,
                ToolCount {
                    tools,
                    tokens: estimate_tokens(chars),
                },
            ))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

/// 扫全机器的 MCP 配置。`cwd` 给了才算得出项目级来源。
#[tauri::command(async)]
pub fn tools_scan_mcp(cwd: Option<String>) -> McpScan {
    let cwd = cwd
        .filter(|c| !c.trim().is_empty())
        .map(std::path::PathBuf::from);
    scan(cwd.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ConfigOrigin, ConfigScope};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // -----------------------------------------------------------------------
    // 凭据识别
    // -----------------------------------------------------------------------

    /// 键名里的 `-` / `_` 不能挡住比对。`X-API-Key` 是 Anthropic 自家 header 的写法，
    /// 照字面比既不含 `APIKEY` 也不含 `API_KEY` —— 漏掉它等于详情页上直接摆一把钥匙。
    #[test]
    fn separators_in_a_key_name_do_not_hide_a_credential() {
        for key in [
            "X-API-Key",
            "x-api-key",
            "apiKey",
            "API_KEY",
            "AWS_ACCESS_KEY_ID",
            "Authorization",
            "Cookie",
            "my-private-key",
            "SESSION-ID",
            "BearerToken",
        ] {
            assert!(looks_secret(key), "{key} 该被当成凭据");
        }
    }

    /// 反过来也得管住：什么都打码等于什么都没打码，用户会直接学会无视那几个点。
    #[test]
    fn ordinary_variables_are_left_alone() {
        for key in ["PATH", "HOME", "NODE_ENV", "PORT", "DEBUG", "LANG"] {
            assert!(!looks_secret(key), "{key} 不该被打码");
        }
    }

    #[test]
    fn a_url_password_is_masked_but_the_host_still_reads() {
        let out = mask_url("https://bob:s3cr3t@mcp.example.com/sse");
        assert!(out.starts_with("https://bob:"), "{out}");
        assert!(!out.contains("s3cr3t"), "{out}");
        // 主机和路径留着 —— 全打码的话详情页上认不出这是哪个 server。
        assert!(out.ends_with("@mcp.example.com/sse"), "{out}");
    }

    #[test]
    fn a_credential_query_parameter_is_masked_and_the_rest_is_not() {
        let out = mask_url("https://mcp.example.com/x?api_key=abc123&mode=fast");
        assert!(!out.contains("abc123"), "{out}");
        assert!(out.contains("mode=fast"), "{out}");
        assert!(out.starts_with("https://mcp.example.com/x?api_key="), "{out}");
    }

    /// 路径里的那一段**不猜**。判断一段路径是不是密钥只能看形状，猜错就是把正常
    /// 路径打成一片点 —— 和 `looks_secret` 只认键名是同一条规矩。
    #[test]
    fn a_url_without_anything_recognisable_comes_back_untouched() {
        for url in [
            "https://mcp.example.com/9f8e7d/sse",
            "http://127.0.0.1:3000/mcp",
            "not-a-url-at-all",
        ] {
            assert_eq!(mask_url(url), url);
        }
    }

    /// 读出来的每一条都得带上打码版本：漏填的那条在详情页上就是原文。
    #[test]
    fn every_parsed_shape_carries_the_masked_url() {
        let json = json_servers(
            r#"{"mcpServers":{"a":{"url":"https://u:p@h/x"}}}"#,
            &["mcpServers"],
        );
        let oc = opencode_servers(r#"{"mcp":{"a":{"type":"remote","url":"https://u:p@h/x"}}}"#);
        let toml = toml_servers("[mcp_servers.a]\nurl = \"https://u:p@h/x\"\n");
        for read in [json, oc, toml] {
            let def = &read.servers[0];
            let masked = def.url_masked.as_deref().unwrap_or("");
            assert!(!masked.contains(":p@"), "{masked}");
            assert_eq!(def.url.as_deref(), Some("https://u:p@h/x"), "原文得留着");
        }
    }

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_file(tag: &str, name: &str, body: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "csv-mcp-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    fn source(path: &Path, format: McpFormat) -> McpSource {
        McpSource {
            path: path.to_string_lossy().to_string(),
            scope: ConfigScope::User,
            origin: ConfigOrigin::Own,
            format,
            writable: true,
            exists: path.is_file(),
            precedence: 100,
            conditional: false,
        }
    }

    fn read(path: &Path, format: McpFormat) -> SourceRead {
        read_source(&source(path, format), None)
    }

    #[test]
    fn a_json_stdio_server_comes_through_whole() {
        let path = temp_file(
            "json",
            "mcp.json",
            r#"{"mcpServers":{"chrome":{"type":"stdio","command":"npx","args":["chrome-devtools-mcp@latest"],"env":{"CHROME_PATH":"/Applications/Chrome"}}}}"#,
        );
        let got = read(&path, McpFormat::JsonServers);
        assert!(got.error.is_none());
        assert_eq!(got.servers.len(), 1);
        let s = &got.servers[0];
        assert_eq!(s.name, "chrome");
        assert_eq!(s.transport, McpTransport::Stdio);
        assert_eq!(s.command.as_deref(), Some("npx"));
        assert_eq!(s.args, vec!["chrome-devtools-mcp@latest"]);
        assert_eq!(s.env.len(), 1);
        assert!(!s.env[0].secret);
        assert!(s.enabled && !s.incomplete);
    }

    #[test]
    fn an_entry_with_neither_command_nor_url_is_marked_incomplete() {
        // 本机 `~/.claude.json` 里真有这么一条（只有 env）。画成正常的就是告诉用户
        // 「它在跑」，而它根本起不来。
        let path = temp_file(
            "broken",
            "mcp.json",
            r#"{"mcpServers":{"tapd":{"env":{"TAPD_ACCESS_TOKEN":"x"}}}}"#,
        );
        let got = read(&path, McpFormat::JsonServers);
        assert!(got.servers[0].incomplete);
        assert_eq!(got.servers[0].transport, McpTransport::Unknown);
    }

    #[test]
    fn a_credential_looking_key_is_flagged_so_the_ui_can_mask_it() {
        let path = temp_file(
            "secret",
            "mcp.json",
            r#"{"mcpServers":{"x":{"command":"a","env":{"TAPD_ACCESS_TOKEN":"s3cret","CHROME_PATH":"/x"}}}}"#,
        );
        let env = &read(&path, McpFormat::JsonServers).servers[0].env;
        let secret: Vec<&McpVar> = env.iter().filter(|v| v.secret).collect();
        assert_eq!(secret.len(), 1);
        assert_eq!(secret[0].key, "TAPD_ACCESS_TOKEN");
    }

    #[test]
    fn claude_local_scope_reads_the_project_partition_not_the_top_level() {
        // `~/.claude.json` 一个文件里装着 user 和 local 两档。按顶层读 local 这一条，
        // 项目里配的 server 就整片消失。
        let path = temp_file(
            "local",
            "claude.json",
            r#"{"mcpServers":{"user-one":{"command":"a"}},
                "projects":{"/tmp/p":{"mcpServers":{"local-one":{"command":"b"}}}}}"#,
        );
        let mut src = source(&path, McpFormat::JsonProjectServers);
        src.scope = ConfigScope::Local;
        let got = read_source(&src, Some(Path::new("/tmp/p")));
        assert_eq!(
            got.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["local-one"]
        );
        // 换一个项目就该什么都读不到，而不是退回顶层那份。
        assert!(read_source(&src, Some(Path::new("/tmp/other")))
            .servers
            .is_empty());
    }

    #[test]
    fn opencode_splits_its_command_array_into_command_and_args() {
        // 七家里唯一 `command` 是数组的一家，env 键还叫 `environment`。
        let path = temp_file(
            "opencode",
            "opencode.json",
            r#"{"mcp":{"x":{"type":"local","command":["npx","-y","foo"],"environment":{"A":"1"},"enabled":false}}}"#,
        );
        let s = &read(&path, McpFormat::OpencodeJson).servers[0];
        assert_eq!(s.command.as_deref(), Some("npx"));
        assert_eq!(s.args, vec!["-y", "foo"]);
        assert_eq!(s.env.len(), 1);
        assert_eq!(s.transport, McpTransport::Stdio);
        assert!(!s.enabled);
    }

    #[test]
    fn a_remote_opencode_server_is_http_not_stdio() {
        let path = temp_file(
            "remote",
            "opencode.json",
            r#"{"mcp":{"x":{"type":"remote","url":"https://example.com/mcp","headers":{"Authorization":"Bearer x"}}}}"#,
        );
        let s = &read(&path, McpFormat::OpencodeJson).servers[0];
        assert_eq!(s.transport, McpTransport::Http);
        assert!(s.command.is_none() && !s.incomplete);
        assert!(s.headers[0].secret, "Authorization 该按凭据打码");
    }

    #[test]
    fn a_toml_server_comes_through_with_its_env_table() {
        let path = temp_file(
            "toml",
            "config.toml",
            "[mcp_servers.node_repl]\ncommand = \"node\"\nargs = [\"x.js\"]\nenabled = false\n\n[mcp_servers.node_repl.env]\nA = \"1\"\nB = 2\n",
        );
        let s = &read(&path, McpFormat::TomlServers).servers[0];
        assert_eq!(s.name, "node_repl");
        assert_eq!(s.command.as_deref(), Some("node"));
        assert_eq!(s.args, vec!["x.js"]);
        assert!(!s.enabled);
        // 数字值也要收：进程环境里它最终就是字符串，丢掉等于假装没配过。
        let b = s.env.iter().find(|v| v.key == "B").unwrap();
        assert_eq!(b.value, "2");
    }

    #[test]
    fn a_broken_file_reports_the_error_instead_of_silently_reading_nothing() {
        // 静悄悄少几行是这整个面板最坏的失败方式：用户看到的是「我配的 server 没了」。
        let path = temp_file("bad", "mcp.json", "{ not json");
        let got = read(&path, McpFormat::JsonServers);
        assert!(got.servers.is_empty());
        assert!(got.error.is_some());
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let got = read(Path::new("/nonexistent/nope.json"), McpFormat::JsonServers);
        assert!(got.servers.is_empty() && got.error.is_none());
    }

    #[test]
    fn the_fingerprint_looks_at_the_command_line_not_the_name() {
        let a = temp_file("fp-a", "a.json", r#"{"mcpServers":{"one":{"command":"npx","args":["foo"]}}}"#);
        let b = temp_file("fp-b", "b.json", r#"{"mcpServers":{"two":{"command":"npx","args":["foo"]}}}"#);
        let fa = read(&a, McpFormat::JsonServers).servers[0].fingerprint();
        let fb = read(&b, McpFormat::JsonServers).servers[0].fingerprint();
        assert_eq!(fa, fb, "名字不同但跑的是同一条命令");

        let c = temp_file("fp-c", "c.json", r#"{"mcpServers":{"one":{"command":"npx","args":["bar"]}}}"#);
        assert_ne!(fa, read(&c, McpFormat::JsonServers).servers[0].fingerprint());
    }

    #[test]
    fn tokens_are_estimated_at_four_characters_each() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(8), 2);
        // 向上取整：9 个字符不能报成 2。
        assert_eq!(estimate_tokens(9), 3);
    }

    #[test]
    fn scanning_this_machine_never_panics_and_reports_every_agent() {
        let scan = scan(None);
        assert_eq!(scan.agents.len(), AGENTS.len());
        // 不支持 MCP 的那几家也要在名单里，UI 才好说明「为什么这儿是灰的」。
        assert!(scan.agents.iter().all(|a| !a.agent.is_empty()));
        assert_eq!(
            scan.summary.servers,
            scan.servers.len(),
            "汇总和列表必须是同一份数据"
        );
    }
}

