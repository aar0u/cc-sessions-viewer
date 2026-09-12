//! skills.sh 的搜索接口。
//!
//! 这是整个「发现」面板唯一能用的公开接口，方案文档 1.1 / 1.2 记了实测过程：
//! `/api/v1/*` 要 Vercel OIDC token、`/api/skill/...` 返回 401、详情页是 Next.js
//! RSC（build hash 一变就废）。**所以这儿只有搜索，描述和文件清单都得另走 git。**
//!
//! 两条要记住的约束：
//!
//! 1. **返回只有五个字段，没有描述。** 试过 `version` / `searchVersion` /
//!    `includeDescription` / `full` 等参数，返回形状一个字段都不变。列表第二行只能
//!    放 source，这是数据决定的，不是设计偏好。
//! 2. **`name` 可能不等于 `skillId`**（200 条样本里 5 条，`agent development` vs
//!    `agent-development`）。目录名、检出路径**一律用 `skill_id`**，`name` 只用来显示。

use std::time::Duration;

use serde::{Deserialize, Serialize};

const SEARCH_URL: &str = "https://www.skills.sh/api/search";
/// 接口自己的上限。传 201 / 500 都只回 200 条，照它的规矩夹住，别指望它报错。
const MAX_LIMIT: u32 = 200;
/// 接口拒绝更短的查询（`{"error":"Query must be at least 2 characters"}`，HTTP 400）。
const MIN_QUERY: usize = 2;

// ---------------------------------------------------------------------------
// 对外形状
// ---------------------------------------------------------------------------

/// 一条搜索结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryHit {
    /// `emilkowalski/skills`（GitHub 仓库）或 `code.deepline.com`（厂商自托管）。
    pub source: String,
    /// 目录名、安装名。**永远用它**，不要用 `name`。
    pub skill_id: String,
    /// 显示名，可能带空格。
    pub name: String,
    pub installs: u64,
    /// `source` 是不是 `owner/repo` 形态 —— 只有这种才装得了，见 [`installable_source`]。
    pub installable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrySearch {
    /// 接口回显的查询词。用它而不是我们发出去的那个，才能判断响应过不过期。
    pub query: String,
    /// `fuzzy` / `semantic`。多词查询会切到语义搜索，排序看着「不像」，原样显示出来
    /// 比让用户猜强。
    pub search_type: String,
    pub hits: Vec<RegistryHit>,
}

/// 搜不成的原因。
///
/// 分类而不是甩一句 `ureq` 的错误原文：面板有四种语言，而「断网」和「服务端 500」
/// 对用户意味着完全不同的下一步（一个是检查网络，一个是等一会儿再试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistryErrKind {
    /// 连不上 —— 断网、DNS、超时。
    Offline,
    /// 查询词太短，接口不收。前端本来就该拦住，这儿是第二道。
    TooShort,
    /// 连上了但对方不高兴（4xx / 5xx）。
    Http,
    /// 连上也回了，但不是我们认得的形状。
    BadJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryError {
    pub kind: RegistryErrKind,
    /// 底层原文，给「展开详情」用。界面上默认只显示按 `kind` 翻出来的那句话。
    pub detail: String,
}

impl RegistryError {
    fn new(kind: RegistryErrKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// 来源形态
// ---------------------------------------------------------------------------

/// 这个 source 是不是 `owner/repo`，也就是**装不装得了**。
///
/// 另一种形态是域名（`code.deepline.com`、`smithery.ai`、`open.feishu.cn` …，八次
/// 查询里占 0%–8%）。那些是厂商自托管的 skill，`/skills.json`、
/// `/.well-known/skills.json`、`/<slug>/SKILL.md` 全 404，详情页也没有 SSR 内容 ——
/// **没有任何公开的取内容路径**。列出来但标成不可安装，不假装能装。
///
/// 收紧到 `[A-Za-z0-9._-]`，并且两段都不能是 `.` / `..`、不能以 `-` 开头：
/// 这个字符串后面会被拼进 `https://github.com/<source>.git` 交给 `git`，
/// 以 `-` 开头的会被当成命令行参数。
pub fn installable_source(source: &str) -> bool {
    let mut parts = source.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, repo].iter().all(|seg| {
        !seg.is_empty()
            && *seg != "."
            && *seg != ".."
            && !seg.starts_with('-')
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    })
}

// ---------------------------------------------------------------------------
// 搜索
// ---------------------------------------------------------------------------

/// 接口原样的一条。只在本模块内部存在 —— 对外的是 [`RegistryHit`]。
///
/// `id` 恒等于 `source + "/" + skillId`（200/200 命中），所以不收：留着它只会多一个
/// 可能和另外两个字段对不上的真相来源。
#[derive(Deserialize)]
struct RawHit {
    #[serde(rename = "skillId")]
    skill_id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    installs: u64,
    source: Option<String>,
}

#[derive(Deserialize)]
struct RawSearch {
    query: Option<String>,
    #[serde(rename = "searchType")]
    search_type: Option<String>,
    #[serde(default)]
    skills: Vec<RawHit>,
}

/// 这个查询词够不够长发出去。按 **char** 数不是字节数 —— 两个汉字是六个字节，
/// 按字节判会把一个合法查询当成太短。
pub fn too_short(query: &str) -> bool {
    query.trim().chars().count() < MIN_QUERY
}

pub fn search(query: &str, limit: u32) -> Result<RegistrySearch, RegistryError> {
    let q = query.trim();
    if too_short(q) {
        return Err(RegistryError::new(RegistryErrKind::TooShort, q.to_string()));
    }
    let raw: RawSearch = ureq::get(SEARCH_URL)
        .query("q", q)
        .query("limit", &limit.clamp(1, MAX_LIMIT).to_string())
        .timeout(Duration::from_secs(15))
        .call()
        .map_err(|e| match e {
            // 连上了但状态码不对。对方的 body 常常是 `{"error":"…"}`，带上原文。
            ureq::Error::Status(code, resp) => RegistryError::new(
                RegistryErrKind::Http,
                format!("{code} {}", resp.into_string().unwrap_or_default().trim()),
            ),
            other => RegistryError::new(RegistryErrKind::Offline, other.to_string()),
        })?
        .into_json()
        .map_err(|e| RegistryError::new(RegistryErrKind::BadJson, e.to_string()))?;

    // 缺字段的条目直接丢掉，不用空串凑数：一条没有 source 的结果既点不开也装不了，
    // 摆在列表里只会让人以为是我们坏了。
    let hits = raw
        .skills
        .into_iter()
        .filter_map(|h| {
            let source = h.source?;
            let skill_id = h.skill_id?;
            Some(RegistryHit {
                installable: installable_source(&source),
                name: h.name.unwrap_or_else(|| skill_id.clone()),
                installs: h.installs,
                source,
                skill_id,
            })
        })
        .collect();

    Ok(RegistrySearch {
        query: raw.query.unwrap_or_else(|| q.to_string()),
        search_type: raw.search_type.unwrap_or_default(),
        hits,
    })
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn tools_registry_search(query: String, limit: u32) -> Result<RegistrySearch, RegistryError> {
    search(&query, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_two_segment_owner_repo_is_installable() {
        for s in [
            "emilkowalski/skills",
            "github/awesome-copilot",
            "a_b/c.d-e",
            "anthropics/claude-code",
        ] {
            assert!(installable_source(s), "{s}");
        }
    }

    /// 域名源是真实存在的（deepline 那批安装量 8k~26k），但没有任何公开的取内容
    /// 路径。判错一次就是给用户一个按下去必然失败的安装按钮。
    #[test]
    fn a_bare_domain_is_not_installable() {
        for s in [
            "code.deepline.com",
            "smithery.ai",
            "open.feishu.cn",
            "developer.paddle.com",
        ] {
            assert!(!installable_source(s), "{s}");
        }
    }

    /// 这个字符串会被拼进 `https://github.com/<source>.git` 交给 `git` 子进程。
    /// 放过一条就是把 git 的传输层交给陌生人。
    #[test]
    fn anything_that_could_reach_git_as_something_else_is_refused() {
        for s in [
            "a/b/c",                      // 三段：路径里能多插一层
            "../x/y",                     // 逃出去
            "a/..",                       //
            "-upload-pack/x",             // 以 `-` 开头会被 git 当参数
            "x/-c",                       //
            "ext::sh -c whoami/x",        // git 的 ext:: 传输协议
            "file:///etc/passwd",         //
            "https://github.com/a/b.git", // 完整 URL 一律不收，URL 由我们拼
            "a b/c",                      // 空格
            "a/",                         // 空段
            "/b",                         //
            "",                           //
        ] {
            assert!(!installable_source(s), "{s} 不该被放行");
        }
    }

    /// 接口的硬门槛是 2 个字符（HTTP 400）。本地先挡住，别拿一句英文报错糊用户脸上。
    #[test]
    fn a_query_shorter_than_two_characters_never_leaves_the_machine() {
        for q in ["", " ", "a", "  x  ", "\t"] {
            assert!(too_short(q), "{q:?} 不该发出去");
        }
        // 两个汉字是六个字节，按字节判会把一个合法查询当成太短。
        for q in ["中文", "ab", " ab "] {
            assert!(!too_short(q), "{q:?} 该发出去");
        }
    }

    /// `search` 走到网络之前先自己拦一道 —— 测的是这道拦截，不是接口。
    #[test]
    fn the_search_entry_point_refuses_a_short_query_without_touching_the_network() {
        assert_eq!(
            search("a", 100).unwrap_err().kind,
            RegistryErrKind::TooShort
        );
    }
}
