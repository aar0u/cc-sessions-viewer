//! 全局指令文件（`CLAUDE.md` / `AGENTS.md` / `MEMORY.md`）的读写、`@import` 展开、
//! 生效链路和分叉检测。
//!
//! 四块里最简单的一块 —— 文件都是 Markdown，所以**一套读写通吃七家**，`ToolSurface`
//! 上各家只报路径和回退规则。这里一个 agent 分支都没有。
//!
//! 三件不平凡的事：
//!
//! 1. **`@import` 要展开。** `~/.claude/CLAUDE.md` 全文可能只有一行 `@RTK.md`；不展开
//!    的话用户在面板上看到的「全局配置」等于什么都没看到。两种路径风格（相对文件自身
//!    目录 / 绝对）在同一个解析器里处理。**只展开第一层** —— 递归展开要防环，而全局
//!    指令套三层的场景没见过，为它引入一个环检测得不偿失；第二层往下只报数量。
//! 2. **一个文件影响好几家。** opencode 和 grok 在自己的 `AGENTS.md` 缺席时都回退去读
//!    `~/.claude/CLAUDE.md`。用户以为只在改 Claude —— 这是本功能最容易踩的坑，所以每个
//!    文件都带着「谁会读到它」一起报出来。
//! 3. **同名片段会漂，而且分两种。** 内容**已经不一样**的是分叉（`MemoFork`）——
//!    只提示，不自动合并，有意写成不一样完全合理（给 Codex 的那版故意更短）。内容
//!    **还一模一样**的是重复（`MemoDup`）—— 那是白存了好几份、改一处另外几家读不到，
//!    可以合并掉，见 [`super::memo_merge`]。本机 `RTK.md` 在 `.claude` / `.codex` /
//!    `.grok` 下各一份 964 B，md5 相同，正是后者。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use super::textdiff::{line_hunks, MAX_DIFF_LINES};
use super::{is_installed, surface, AGENTS};
use crate::types::DiffHunk;
use crate::util;

/// 单个文件最多读这么多字节。全局指令是人手写的 Markdown，几十 KB 顶天；真碰上一个
/// 几百 MB 的文件（有人把日志重定向进去过），整个面板会卡死在读文件上。
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// 一个文件在某个 agent 眼里的身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoRole {
    /// 这家自己的约定路径。
    Own,
    /// 自己那份缺席时才会读到的回退目标。
    Fallback,
    /// 配置里显式追加的（opencode 的 `instructions`）。
    Extra,
}

/// 谁会读到这个文件，以及怎么读到的。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoReader {
    pub agent: String,
    pub role: MemoRole,
    /// 这条链路**现在**是不是真的生效。回退目标在自家文件存在时就不生效。
    pub active: bool,
}

/// 一行 `@…`。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoImport {
    /// 原样的那一行（去掉首尾空白），UI 上显示它。
    pub raw: String,
    /// 行号，从 1 开始。
    pub line: usize,
    /// 解析出来的绝对路径。解析不出来（比如 `@` 后面是空的）为 `None`。
    pub path: Option<String>,
    pub exists: bool,
    pub bytes: u64,
    /// 这一层引用的文件自己还有几行 `@…`。**不展开**，只报数。
    pub nested: usize,
}

/// 文件内容的指纹，给「保存时外部改动检测」用。
///
/// 只到毫秒：`SystemTime` 原样传不过 JSON，而纳秒精度在前后端之间来回一趟必然掉精度，
/// 反过来让每次保存都误判成冲突。外部编辑器写一次文件，mtime 的变化远不止 1ms。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoRevision {
    pub exists: bool,
    pub size: u64,
    pub mtime_ms: Option<i64>,
}

/// 一条链接背后的真身。不是链接、或者解析不出来时原样返回。
///
/// **合并之后这条路径必经。** 合并把 `~/.claude/RTK.md` 换成了一条指向主 store 的
/// 链接，而写入走的是「写临时文件 + rename 盖上去」—— 直接对着链接路径 rename 会把
/// 链接本身**替换成一个普通文件**：合并当场被拆掉，另外两家又开始读各自的旧内容，
/// 而且全程没有任何报错。所以读、比指纹、写，一律先落到真身上。
pub fn body_of(path: &Path) -> PathBuf {
    if !super::link::is_link(path) {
        return path.to_path_buf();
    }
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn revision(path: &Path) -> Result<MemoRevision, String> {
    let real = body_of(path);
    // 断链：路径本身在（是条链接），但指过去什么都没有。报 `exists: false` 会让面板
    // 给个「新建」入口，点下去写的还是这条断链，照样失败。
    if real == path && super::link::is_link(path) {
        return Err(format!(
            "{} 是一条链接，但它指向的文件不存在",
            path.display()
        ));
    }
    let r = util::file_revision(&real)?;
    Ok(MemoRevision {
        exists: r.exists,
        size: r.size,
        mtime_ms: r.modified.and_then(|t| {
            t.duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as i64)
        }),
    })
}

/// 一个物理文件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoFile {
    pub path: String,
    /// 文件名，分叉检测按它分组。
    pub name: String,
    pub exists: bool,
    pub bytes: u64,
    pub revision: MemoRevision,
    /// 谁会读到它。**空数组是可能的**：被 import 进来的片段没人直接读它，是靠引用它的
    /// 那个文件生效的。
    pub readers: Vec<MemoReader>,
    /// 这个文件是被谁 import 进来的。顶层文件为 `None`。
    pub imported_by: Option<String>,
    /// 它是条链接时，指向哪儿（原样的 target，不解析到底）。实体文件为 `None`。
    ///
    /// 合并之后 `~/.claude/RTK.md` 这类位置就是链接了，UI 得能把「这儿有一份内容」
    /// 和「这儿只是指过去」分开画 —— 否则合并完看上去和没合一样。
    pub link: Option<String>,
    pub imports: Vec<MemoImport>,
    /// 读不了（权限 / 太大 / 不是普通文件）。**不吞** —— 静悄悄少一个文件，用户会以为
    /// 那家 agent 没有全局指令。
    pub error: Option<String>,
}

/// 一个 agent 的全局指令情况。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoAgentInfo {
    pub agent: String,
    pub installed: bool,
    /// 有没有 home 级约定。false 的那几家（agy）在 UI 上是禁用态 —— 给个输入框让用户
    /// 白写是更糟的。
    pub supported: bool,
    pub path: Option<String>,
    pub exists: bool,
    pub fallback: Option<String>,
    /// **实际生效的那个文件**：自己那份在就是自己的，不在就是回退目标，都没有为 `None`。
    pub effective: Option<String>,
    /// 现在读的是不是回退来的。UI 上要标「实际生效 …（回退）」。
    pub fallen_back: bool,
    pub extra: Vec<String>,
}

/// 一处分叉：同名、内容不一样。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoFork {
    pub name: String,
    pub sides: Vec<MemoForkSide>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoForkSide {
    pub path: String,
    pub bytes: u64,
}

/// 一处重复：同名、内容**一模一样**、而且在磁盘上真的是好几份。
///
/// 和 [`MemoFork`] 正好是一枚硬币的两面 —— 分叉是「同名但内容分家了」，重复是
/// 「同名而且还没分家」。分叉只能提示（有意写成不一样完全合理），重复可以动手：
/// 搬一份进主 store，原位全改成链接，从此改一处处处生效。
///
/// **已经合并过的不算重复**：三条链接指向同一个物理文件时，内容当然还是全相同的，
/// 但那正是我们想要的终态，再报一次「重复」就是在劝用户做一件已经做完的事。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoDup {
    pub name: String,
    /// 每份都一样大（内容都相同了），所以只报一个数。
    pub bytes: u64,
    /// 涉及的位置，含已经是链接的那些。按路径排序，顺序稳定。
    pub paths: Vec<String>,
    /// 其中还是实体文件的那几个 —— 合并真正要动的就是它们。
    pub bodies: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoSummary {
    /// 列表上的文件数。
    pub files: usize,
    /// 其中真实存在的。
    pub present: usize,
    /// 断掉的 `@import`（目标不存在）。
    pub broken: usize,
    pub forks: usize,
    /// 有几组同名同内容的重复。
    pub dups: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoScan {
    pub home: String,
    pub agents: Vec<MemoAgentInfo>,
    pub files: Vec<MemoFile>,
    pub forks: Vec<MemoFork>,
    pub dups: Vec<MemoDup>,
    pub summary: MemoSummary,
}

// ---------------------------------------------------------------------------
// 读
// ---------------------------------------------------------------------------

/// 读一个 Markdown 文件。不存在不是错误 —— 返回空字符串加 `exists: false`，让「从无到
/// 有地建」和「改已有的」走同一条路径。
pub fn read(path: &Path) -> Result<(String, MemoRevision), String> {
    let rev = revision(path)?;
    if !rev.exists {
        return Ok((String::new(), rev));
    }
    if rev.size > MAX_BYTES {
        return Err(format!(
            "{} 有 {} 字节，超过 {MAX_BYTES} 的上限，不在这儿打开",
            path.display(),
            rev.size
        ));
    }
    let text = fs::read_to_string(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
    Ok((text, rev))
}

/// 一行 `@…` 解析成绝对路径。
///
/// - 相对路径按**文件自身所在目录**解析，不是进程 cwd —— 后者取决于 app 从哪儿启动，
///   同一个文件会在不同时候解析到不同地方。
/// - `~/` 开头按 home 解析。
/// - 绝对路径直接用。
fn resolve_import(raw: &str, base_dir: &Path) -> Option<PathBuf> {
    let target = raw.trim();
    if target.is_empty() {
        return None;
    }
    if let Some(rest) = target.strip_prefix("~/") {
        return Some(util::home().join(rest));
    }
    let p = Path::new(target);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    })
}

/// 找出正文里的 `@…` 行。
///
/// 只认**整行以 `@` 开头**的：Markdown 正文里 `@` 到处都是（邮箱、`@anthropic-ai/sdk`、
/// at 某人），把它们都当 import 会凭空多出一堆红色的断链。围栏代码块里的整行不算 ——
/// 文档里贴一段别人的 CLAUDE.md 是常事。
pub fn parse_imports(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fenced = false;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix('@') else {
            continue;
        };
        let target = rest.split_whitespace().next().unwrap_or("");
        if target.is_empty() {
            continue;
        }
        out.push((i + 1, target.to_string()));
    }
    out
}

/// 数一个文件里有几行 `@…`，读不了就当 0。给「还有 N 层未展开」那句话用。
fn count_imports(path: &Path) -> usize {
    let Ok(rev) = revision(path) else { return 0 };
    if !rev.exists || rev.size > MAX_BYTES {
        return 0;
    }
    fs::read_to_string(path)
        .map(|t| parse_imports(&t).len())
        .unwrap_or(0)
}

fn describe(path: &Path, imported_by: Option<String>) -> MemoFile {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let (rev, error) = match revision(path) {
        Ok(r) => (r, None),
        Err(e) => (
            MemoRevision {
                exists: false,
                size: 0,
                mtime_ms: None,
            },
            Some(e),
        ),
    };
    let mut file = MemoFile {
        path: path.to_string_lossy().to_string(),
        name,
        exists: rev.exists,
        bytes: rev.size,
        revision: rev,
        readers: Vec::new(),
        imported_by,
        link: super::link::read_link_target(path).map(|t| t.to_string_lossy().to_string()),
        imports: Vec::new(),
        error,
    };
    if !file.exists || file.error.is_some() || rev.size > MAX_BYTES {
        if rev.exists && rev.size > MAX_BYTES {
            file.error = Some(format!("{} 字节，超过 {MAX_BYTES} 的上限", rev.size));
        }
        return file;
    }
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            file.error = Some(format!("读不了：{e}"));
            return file;
        }
    };
    let base = path.parent().unwrap_or(Path::new("."));
    for (line, raw) in parse_imports(&text) {
        let resolved = resolve_import(&raw, base);
        let (path_s, exists, bytes, nested) = match resolved.as_deref() {
            Some(p) => {
                let r = revision(p).unwrap_or(MemoRevision {
                    exists: false,
                    size: 0,
                    mtime_ms: None,
                });
                (
                    Some(p.to_string_lossy().to_string()),
                    r.exists,
                    r.size,
                    if r.exists { count_imports(p) } else { 0 },
                )
            }
            None => (None, false, 0, 0),
        };
        file.imports.push(MemoImport {
            raw: format!("@{raw}"),
            line,
            path: path_s,
            exists,
            bytes,
            nested,
        });
    }
    file
}

/// 扫全机器的全局指令。
pub fn scan() -> MemoScan {
    let mut agents: Vec<MemoAgentInfo> = Vec::new();
    // 路径 → 文件。BTreeMap 让列表顺序稳定，也顺手去了重（好几家指向同一个文件正是
    // 这个面板要说的事）。
    let mut files: BTreeMap<String, MemoFile> = BTreeMap::new();
    let mut readers: BTreeMap<String, Vec<MemoReader>> = BTreeMap::new();

    let mut note = |path: &Path, agent: &str, role: MemoRole, active: bool| {
        let key = path.to_string_lossy().to_string();
        files
            .entry(key.clone())
            .or_insert_with(|| describe(path, None));
        readers.entry(key).or_default().push(MemoReader {
            agent: agent.to_string(),
            role,
            active,
        });
    };

    for agent in AGENTS {
        let Ok(s) = surface(agent) else { continue };
        let own = s.memo_path();
        let fallback = s.memo_fallback();
        let extra = s.memo_extra_sources();
        let own_exists = own.as_deref().map(|p| p.is_file()).unwrap_or(false);

        if let Some(p) = own.as_deref() {
            note(p, agent, MemoRole::Own, own_exists);
        }
        if let Some(p) = fallback.as_deref() {
            // 回退只在自家那份缺席时生效。两边都报出来，但只有一边是 active。
            note(p, agent, MemoRole::Fallback, !own_exists);
        }
        for p in &extra {
            note(p, agent, MemoRole::Extra, true);
        }

        let effective = if own_exists {
            own.clone()
        } else {
            fallback.clone().filter(|p| p.is_file())
        };
        agents.push(MemoAgentInfo {
            agent: agent.to_string(),
            installed: is_installed(s.as_ref()),
            supported: own.is_some(),
            path: own.as_ref().map(|p| p.to_string_lossy().to_string()),
            exists: own_exists,
            fallback: fallback.as_ref().map(|p| p.to_string_lossy().to_string()),
            effective: effective.as_ref().map(|p| p.to_string_lossy().to_string()),
            fallen_back: !own_exists && effective.is_some(),
            extra: extra
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
        });
    }

    // 第一层 import 也进列表：它们是能点开编辑的，而且分叉检测靠的就是它们。
    let tops: Vec<MemoFile> = files.values().cloned().collect();
    for top in &tops {
        for imp in &top.imports {
            let Some(p) = imp.path.as_deref() else { continue };
            files
                .entry(p.to_string())
                .or_insert_with(|| describe(Path::new(p), Some(top.path.clone())));
        }
    }

    let mut out: Vec<MemoFile> = files.into_values().collect();
    for f in out.iter_mut() {
        if let Some(r) = readers.remove(&f.path) {
            f.readers = r;
        }
    }

    let (forks, dups) = detect_groups(&out);
    let summary = MemoSummary {
        files: out.len(),
        present: out.iter().filter(|f| f.exists).count(),
        broken: out
            .iter()
            .flat_map(|f| f.imports.iter())
            .filter(|i| !i.exists)
            .count(),
        forks: forks.len(),
        dups: dups.len(),
    };
    MemoScan {
        home: util::home().to_string_lossy().to_string(),
        agents,
        files: out,
        forks,
        dups,
        summary,
    }
}

/// 把同名文件分成两堆：内容不一样的是**分叉**，内容一样还存着好几份的是**重复**。
///
/// 一趟做完两件事，是因为它们判的是同一件事的两面，分成两个函数就要把同一批文件
/// 读两遍、而且两边对「同名」的定义还得自己保持同步。
///
/// 判定一律用**内容**，不用大小：两份不同的内容凑巧一样长不是没可能，而「大小一样
/// 所以内容一样」这种判断错了是静默的。
fn detect_groups(files: &[MemoFile]) -> (Vec<MemoFork>, Vec<MemoDup>) {
    let mut groups: BTreeMap<String, Vec<&MemoFile>> = BTreeMap::new();
    for f in files.iter().filter(|f| f.exists && f.error.is_none()) {
        groups.entry(f.name.clone()).or_default().push(f);
    }
    let mut forks = Vec::new();
    let mut dups = Vec::new();
    for (name, group) in groups {
        if group.len() < 2 {
            continue;
        }
        let texts: Vec<String> = group
            .iter()
            .map(|f| fs::read_to_string(&f.path).unwrap_or_default())
            .collect();
        if !texts.windows(2).all(|w| w[0] == w[1]) {
            forks.push(MemoFork {
                name,
                sides: group
                    .iter()
                    .map(|f| MemoForkSide {
                        path: f.path.clone(),
                        bytes: f.bytes,
                    })
                    .collect(),
            });
            continue;
        }
        // 内容全相同。但如果它们本来就指着同一个物理文件（已经合并过了），
        // 那是终态不是毛病。
        let mut physical: Vec<PathBuf> = group
            .iter()
            .map(|f| {
                let p = Path::new(&f.path);
                fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
            })
            .collect();
        physical.sort();
        physical.dedup();
        if physical.len() < 2 {
            continue;
        }
        dups.push(MemoDup {
            name,
            bytes: group.first().map(|f| f.bytes).unwrap_or(0),
            paths: group.iter().map(|f| f.path.clone()).collect(),
            bodies: group
                .iter()
                .filter(|f| !super::link::is_link(Path::new(&f.path)))
                .map(|f| f.path.clone())
                .collect(),
        });
    }
    (forks, dups)
}

// ---------------------------------------------------------------------------
// 并排 diff
// ---------------------------------------------------------------------------

/// 两份同名文件的逐行差异。**只提示不自动合并** —— 有意分叉完全合理，动手由用户决定。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoDiff {
    pub left: String,
    pub right: String,
    pub hunks: Vec<DiffHunk>,
    /// 两边逐字节一样 —— 扫描之后有人把它们改成一样了。
    pub same: bool,
    /// 太大，没逐行比。不假装算过。
    pub truncated: bool,
    /// hunk 被砍到上限了，后面还有没显示的。
    pub clipped: bool,
}

/// 逐行比两段正文。`left` / `right` 只是给 UI 认人的标签，不参与比较。
///
/// 分叉比的是磁盘上的两个文件，冲突比的是「我打开时那份」和「现在磁盘上那份」——
/// 后者两边都只在内存里。两个入口共用这一份，两边各写一遍必然给出两种 hunk。
pub fn diff_text(a: &str, b: &str, left: String, right: String) -> MemoDiff {
    let la: Vec<&str> = a.lines().collect();
    let lb: Vec<&str> = b.lines().collect();
    let head = MemoDiff {
        left,
        right,
        hunks: Vec::new(),
        same: a == b,
        truncated: la.len() > MAX_DIFF_LINES || lb.len() > MAX_DIFF_LINES,
        clipped: false,
    };
    if head.same || head.truncated {
        return head;
    }
    let (hunks, clipped) = line_hunks(&la, &lb);
    MemoDiff {
        hunks,
        clipped,
        ..head
    }
}

pub fn diff(left: &Path, right: &Path) -> Result<MemoDiff, String> {
    let (a, _) = read(left)?;
    let (b, _) = read(right)?;
    Ok(diff_text(
        &a,
        &b,
        left.to_string_lossy().to_string(),
        right.to_string_lossy().to_string(),
    ))
}

// ---------------------------------------------------------------------------
// 写
// ---------------------------------------------------------------------------

/// 写回一个全局指令文件。
///
/// `expected` 是打开时拿到的指纹。对不上说明文件在外面被改过了 —— **拒绝写**，而不是
/// 拿旧内容盖掉别人的修改。这些文件用户随时会在别的编辑器里直接改，而 `watch.rs` 是
/// 单会话 watcher 套不上来，所以只能在这两个时刻比对：打开面板时、保存时。
pub fn write(path: &Path, text: &str, expected: &MemoRevision) -> Result<MemoRevision, String> {
    let current = revision(path)?;
    if current != *expected {
        return Err(if current.exists && !expected.exists {
            format!("{} 已经存在了 —— 是别的地方刚建的，先看一眼再决定要不要覆盖。", path.display())
        } else {
            format!("{} 在外面被改过了 —— 没有覆盖，先看看那边改了什么。", path.display())
        });
    }
    // 合并过的位置是条链接，真正要写的是它背后那份 —— 直接写链接路径会把链接
    // 换成普通文件（见 [`body_of`]）。备份也跟着落到真身旁边。
    let real = body_of(path);
    let label = real.to_string_lossy().to_string();
    let backup = real.with_file_name(format!(
        "{}.bak",
        real.file_name().and_then(|n| n.to_str()).unwrap_or("memo")
    ));
    // 已经存在的才备份：从无到有地建一个 `.bak` 出来只是留垃圾。
    if current.exists {
        util::atomic_write_backed_up(
            &real,
            text.as_bytes(),
            &util::file_revision(&real)?,
            &backup,
            &label,
        )?;
    } else {
        util::atomic_write_file(&real, text.as_bytes(), &util::file_revision(&real)?, &label)?;
    }
    revision(path)
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn tools_scan_memo() -> MemoScan {
    scan()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoDoc {
    pub path: String,
    pub text: String,
    pub revision: MemoRevision,
}

#[tauri::command(async)]
pub fn tools_read_memo(path: String) -> Result<MemoDoc, String> {
    let p = PathBuf::from(&path);
    let (text, revision) = read(&p)?;
    Ok(MemoDoc {
        path,
        text,
        revision,
    })
}

#[tauri::command(async)]
pub fn tools_write_memo(
    path: String,
    text: String,
    expected: MemoRevision,
) -> Result<MemoDoc, String> {
    let p = PathBuf::from(&path);
    let revision = write(&p, &text, &expected)?;
    Ok(MemoDoc {
        path,
        text,
        revision,
    })
}

#[tauri::command(async)]
pub fn tools_diff_memo(left: String, right: String) -> Result<MemoDiff, String> {
    diff(Path::new(&left), Path::new(&right))
}

/// 保存被拒之后那一问：「外面到底改了什么」。
///
/// 两边都不在磁盘上（一边是打开时读到的原文，一边是刚重新读回来的），所以不能走
/// `tools_diff_memo`。实现是同一个 `diff_text`。
#[tauri::command(async)]
pub fn tools_diff_memo_text(
    left: String,
    right: String,
    left_label: String,
    right_label: String,
) -> MemoDiff {
    diff_text(&left, &right, left_label, right_label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("memo-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_line_starting_with_at_is_an_import() {
        let found = parse_imports("@RTK.md\n\n## 正文\n");
        assert_eq!(found, vec![(1, "RTK.md".to_string())]);
    }

    #[test]
    fn an_absolute_at_line_is_an_import_too() {
        let found = parse_imports("@/Users/me/.codex/RTK.md\n");
        assert_eq!(found, vec![(1, "/Users/me/.codex/RTK.md".to_string())]);
    }

    /// 正文里的 `@` 不是 import。都当 import 的话面板上会凭空多出一堆红色断链。
    #[test]
    fn an_at_in_the_middle_of_a_line_is_not_an_import() {
        assert!(parse_imports("装 @anthropic-ai/sdk 就行\n").is_empty());
        assert!(parse_imports("发给 me@example.com\n").is_empty());
    }

    /// 文档里贴一段别人的 CLAUDE.md 是常事，那一段里的 `@` 行不该被当成本文件的引用。
    #[test]
    fn an_at_line_inside_a_fence_is_not_an_import() {
        let text = "@real.md\n\n```md\n@fake.md\n```\n\n@also-real.md\n";
        let found = parse_imports(text);
        assert_eq!(
            found,
            vec![(1, "real.md".to_string()), (7, "also-real.md".to_string())]
        );
    }

    #[test]
    fn a_bare_at_is_not_an_import() {
        assert!(parse_imports("@\n@   \n").is_empty());
    }

    /// 相对路径按**文件自身目录**解析，不是进程 cwd —— 后者取决于 app 从哪儿启动。
    #[test]
    fn a_relative_import_resolves_against_the_file_that_wrote_it() {
        let base = Path::new("/Users/me/.claude");
        assert_eq!(
            resolve_import("RTK.md", base),
            Some(PathBuf::from("/Users/me/.claude/RTK.md"))
        );
    }

    #[test]
    fn an_absolute_import_is_used_as_is() {
        let base = Path::new("/Users/me/.claude");
        assert_eq!(
            resolve_import("/etc/x.md", base),
            Some(PathBuf::from("/etc/x.md"))
        );
    }

    #[test]
    fn a_tilde_import_resolves_against_home() {
        let base = Path::new("/somewhere/else");
        assert_eq!(
            resolve_import("~/notes.md", base),
            Some(util::home().join("notes.md"))
        );
    }

    #[test]
    fn describe_lists_the_first_level_and_only_counts_the_next() {
        let dir = tmp("nested");
        fs::write(dir.join("top.md"), "@mid.md\n").unwrap();
        fs::write(dir.join("mid.md"), "@deep-a.md\n@deep-b.md\n").unwrap();
        fs::write(dir.join("deep-a.md"), "a").unwrap();
        fs::write(dir.join("deep-b.md"), "b").unwrap();

        let f = describe(&dir.join("top.md"), None);
        assert_eq!(f.imports.len(), 1);
        assert_eq!(f.imports[0].raw, "@mid.md");
        assert!(f.imports[0].exists);
        // 第二层只报数，不展开 —— 展开就要防环。
        assert_eq!(f.imports[0].nested, 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_import_is_reported_not_hidden() {
        let dir = tmp("broken");
        fs::write(dir.join("top.md"), "@gone.md\n").unwrap();
        let f = describe(&dir.join("top.md"), None);
        assert_eq!(f.imports.len(), 1);
        assert!(!f.imports[0].exists);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_there_is_not_an_error() {
        let dir = tmp("absent");
        let f = describe(&dir.join("nope.md"), None);
        assert!(!f.exists);
        assert!(f.error.is_none());
        assert!(f.imports.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_name_different_content_is_a_fork() {
        let dir = tmp("fork");
        fs::write(dir.join("a"), "长一点的内容").unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("a").with_extension("md"), "one").unwrap();
        fs::write(dir.join("b").join("a.md"), "two").unwrap();
        let files = vec![
            describe(&dir.join("a.md"), None),
            describe(&dir.join("b").join("a.md"), None),
        ];
        let (forks, dups) = detect_groups(&files);
        assert!(dups.is_empty(), "内容不一样的是分叉，不是重复");
        assert_eq!(forks.len(), 1);
        assert_eq!(forks[0].name, "a.md");
        assert_eq!(forks[0].sides.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 内容一样不是分叉，是**重复** —— 可以合并掉的那一种。
    ///
    /// **判定看内容不看大小** —— 「一样长所以一样」错了是静默的。
    #[test]
    fn same_name_same_content_is_a_dup_not_a_fork() {
        let dir = tmp("same");
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("a.md"), "same").unwrap();
        fs::write(dir.join("b").join("a.md"), "same").unwrap();
        let files = vec![
            describe(&dir.join("a.md"), None),
            describe(&dir.join("b").join("a.md"), None),
        ];
        let (forks, dups) = detect_groups(&files);
        assert!(forks.is_empty());
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].name, "a.md");
        assert_eq!(dups[0].bodies.len(), 2, "两份都还是实体");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 已经合并过的不该再报重复：三个位置指着同一个物理文件，内容当然还是全相同，
    /// 但那正是我们想要的终态。
    #[test]
    fn positions_that_already_point_at_one_file_are_not_a_dup() {
        let dir = tmp("merged");
        fs::create_dir_all(dir.join("b")).unwrap();
        let real = dir.join("a.md");
        fs::write(&real, "same").unwrap();
        let link = dir.join("b").join("a.md");
        super::super::link::link_file(&real, &link).unwrap();
        let files = vec![describe(&real, None), describe(&link, None)];
        let (forks, dups) = detect_groups(&files);
        assert!(forks.is_empty());
        assert!(dups.is_empty(), "合并完的状态不该再劝人合并一次");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 两份实体 + 一条指向其中一份的链接 = 还是重复（物理上仍然是两份）。
    #[test]
    fn a_link_alongside_two_bodies_still_counts_as_a_dup() {
        let dir = tmp("mixed");
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::create_dir_all(dir.join("c")).unwrap();
        let one = dir.join("a.md");
        let two = dir.join("b").join("a.md");
        fs::write(&one, "same").unwrap();
        fs::write(&two, "same").unwrap();
        let link = dir.join("c").join("a.md");
        super::super::link::link_file(&one, &link).unwrap();
        let files = vec![
            describe(&one, None),
            describe(&two, None),
            describe(&link, None),
        ];
        let (_, dups) = detect_groups(&files);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].paths.len(), 3);
        assert_eq!(dups[0].bodies.len(), 2, "链接不算一份内容");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 链接那一行要能被认出来是链接 —— 合并完 UI 得画得出区别。
    #[test]
    fn describe_reports_where_a_link_points() {
        let dir = tmp("linkfield");
        let real = dir.join("real.md");
        fs::write(&real, "x").unwrap();
        let link = dir.join("link.md");
        super::super::link::link_file(&real, &link).unwrap();
        assert!(describe(&real, None).link.is_none());
        assert_eq!(
            describe(&link, None).link.as_deref(),
            Some(real.to_string_lossy().as_ref())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 一样长但内容不同 —— 按大小判会漏掉，这条就是守它的。
    #[test]
    fn same_length_different_bytes_is_still_a_fork() {
        let dir = tmp("samelen");
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("a.md"), "abcd").unwrap();
        fs::write(dir.join("b").join("a.md"), "abce").unwrap();
        let files = vec![
            describe(&dir.join("a.md"), None),
            describe(&dir.join("b").join("a.md"), None),
        ];
        assert_eq!(detect_groups(&files).0.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// **合并之后最容易被静默拆掉的一条。**
    ///
    /// 写入走「临时文件 + rename」，对着链接路径 rename 会把链接换成普通文件 ——
    /// 合并当场作废，另外几家又开始各读各的，而且没有任何报错。
    #[test]
    fn saving_through_a_merged_link_writes_the_real_file_and_keeps_the_link() {
        let dir = tmp("writelink");
        fs::create_dir_all(dir.join("store")).unwrap();
        let real = dir.join("store").join("RTK.md");
        fs::write(&real, "old\n").unwrap();
        let at = dir.join("RTK.md");
        super::super::link::link_file(&real, &at).unwrap();

        let before = revision(&at).unwrap();
        assert!(before.exists, "链接背后的文件在，就该报存在");
        write(&at, "new\n", &before).unwrap();

        assert!(super::super::link::is_link(&at), "链接必须还在");
        assert_eq!(fs::read_to_string(&real).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(&at).unwrap(), "new\n");
        // 备份落在真身旁边，不是链接旁边。
        assert!(dir.join("store").join("RTK.md.bak").is_file());
        assert!(!dir.join("RTK.md.bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 断链不该被报成「还没建，去新建吧」—— 点下去写的还是那条断链。
    #[test]
    fn a_dangling_link_is_an_error_not_a_missing_file() {
        let dir = tmp("dangling");
        let at = dir.join("RTK.md");
        let gone = dir.join("gone.md");
        fs::write(&gone, "x").unwrap();
        super::super::link::link_file(&gone, &at).unwrap();
        fs::remove_file(&gone).unwrap();

        let err = revision(&at).unwrap_err();
        assert!(err.contains("指向的文件不存在"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_creates_the_file_and_returns_the_new_revision() {
        let dir = tmp("create");
        let p = dir.join("new.md");
        let before = revision(&p).unwrap();
        assert!(!before.exists);
        let after = write(&p, "hello", &before).unwrap();
        assert!(after.exists);
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello");
        // 从无到有不该留一个空 .bak
        assert!(!p.with_file_name("new.md.bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwriting_leaves_a_backup_of_what_was_there() {
        let dir = tmp("backup");
        let p = dir.join("x.md");
        fs::write(&p, "old").unwrap();
        let rev = revision(&p).unwrap();
        write(&p, "new", &rev).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(p.with_file_name("x.md.bak")).unwrap(),
            "old"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 外面改过了就拒绝写。拿旧内容盖掉用户在别的编辑器里的修改是这个面板最坏的失败。
    #[test]
    fn a_file_changed_behind_our_back_is_refused() {
        let dir = tmp("conflict");
        let p = dir.join("x.md");
        fs::write(&p, "old").unwrap();
        let stale = revision(&p).unwrap();
        // 模拟外部编辑器：内容和大小都变了
        fs::write(&p, "someone else wrote this").unwrap();
        let err = write(&p, "mine", &stale).unwrap_err();
        assert!(err.contains("外面被改过"), "{err}");
        assert_eq!(fs::read_to_string(&p).unwrap(), "someone else wrote this");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 「本来没有，写的时候发现有了」要给一句不一样的话 —— 那是别的地方刚建出来的。
    #[test]
    fn creating_over_something_that_appeared_is_refused_with_its_own_message() {
        let dir = tmp("appeared");
        let p = dir.join("x.md");
        let absent = revision(&p).unwrap();
        fs::write(&p, "appeared").unwrap();
        let err = write(&p, "mine", &absent).unwrap_err();
        assert!(err.contains("已经存在"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fork_diff_points_at_the_lines_that_differ() {
        let dir = tmp("diff");
        fs::write(dir.join("a.md"), "one\ntwo\nthree\n").unwrap();
        fs::write(dir.join("b.md"), "one\nTWO\nthree\n").unwrap();
        let d = diff(&dir.join("a.md"), &dir.join("b.md")).unwrap();
        assert!(!d.same);
        assert!(!d.truncated);
        let kinds: Vec<&str> = d.hunks[0]
            .lines
            .iter()
            .map(|l| l.kind.as_str())
            .collect();
        assert!(kinds.contains(&"del") && kinds.contains(&"add"), "{kinds:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 扫描之后有人把两边改成一样了 —— 要说「一样」，不是给一个空 diff 让人猜。
    #[test]
    fn two_identical_files_report_same_rather_than_an_empty_diff() {
        let dir = tmp("diffsame");
        fs::write(dir.join("a.md"), "x\n").unwrap();
        fs::write(dir.join("b.md"), "x\n").unwrap();
        let d = diff(&dir.join("a.md"), &dir.join("b.md")).unwrap();
        assert!(d.same);
        assert!(d.hunks.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 保存被拒之后要回答「外面改了什么」，而那两边都只在内存里 —— 没有第二个路径可传。
    #[test]
    fn a_conflict_diff_compares_two_in_memory_texts() {
        let d = diff_text("a\nb\n", "a\nB\n", "opened".into(), "on disk".into());
        assert_eq!((d.left.as_str(), d.right.as_str()), ("opened", "on disk"));
        assert!(!d.same);
        assert_eq!(d.hunks.len(), 1);
    }

    /// 太大就说太大，不假装算过。
    #[test]
    fn a_huge_file_is_reported_as_truncated_not_diffed() {
        let dir = tmp("diffbig");
        let big: String = (0..MAX_DIFF_LINES + 10).map(|i| format!("l{i}\n")).collect();
        fs::write(dir.join("a.md"), &big).unwrap();
        fs::write(dir.join("b.md"), format!("{big}tail\n")).unwrap();
        let d = diff(&dir.join("a.md"), &dir.join("b.md")).unwrap();
        assert!(d.truncated);
        assert!(d.hunks.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 每一家报的路径都得是绝对的 —— 相对路径落到 `resolve_import` 的 base 上会解析歪。
    #[test]
    fn every_agent_reports_absolute_memo_paths() {
        for agent in AGENTS {
            let s = surface(agent).unwrap();
            if let Some(p) = s.memo_path() {
                assert!(p.is_absolute(), "{agent} 的 memo_path 不是绝对路径：{p:?}");
            }
            if let Some(p) = s.memo_fallback() {
                assert!(p.is_absolute(), "{agent} 的 memo_fallback 不是绝对路径：{p:?}");
            }
        }
    }

    /// 扫描要能跑通，且每个 agent 都有一行。
    #[test]
    fn scan_reports_one_row_per_agent() {
        let s = scan();
        assert_eq!(s.agents.len(), AGENTS.len());
        // 支持的那几家路径必须给出来，不支持的必须不给 —— UI 靠这个决定禁不禁用。
        for a in &s.agents {
            assert_eq!(a.supported, a.path.is_some(), "{}", a.agent);
        }
    }

    /// 回退链路：自己那份在的时候，回退那条不能是 active。
    #[test]
    fn a_fallback_is_only_active_while_the_agents_own_file_is_missing() {
        let s = scan();
        for a in &s.agents {
            if a.exists {
                assert!(!a.fallen_back, "{} 自己有文件却报成回退", a.agent);
                assert_eq!(a.effective.as_deref(), a.path.as_deref(), "{}", a.agent);
            }
        }
    }
}
