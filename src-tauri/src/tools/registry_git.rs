//! 「发现」面板的详情预览：把远端仓库里的一个 skill 目录取到本地缓存里。
//!
//! **为什么是 git 而不是 GitHub API**（方案文档 2.1）：
//! `GET /repos/{o}/{r}/git/trees/HEAD?recursive=1` 一次就能拿到全部路径，看着比 clone 香，
//! 但未鉴权限流是 60 次/小时/IP —— 用户在面板里点十几行就见底，而失败的形态是
//! 「突然什么都点不开」。git 协议没有这个问题。`raw.githubusercontent.com` 同理:
//! 得先知道路径，而路径只能从 tree 拿。
//!
//! 三步（方案文档 2.2，括号里是 `emilkowalski/skills` 的实测耗时）：
//!
//! 1. `clone --depth 1 --filter=blob:none --no-checkout`（1900 ms，落盘 136 KB）——
//!    只有目录树，一个 blob 都不下。
//! 2. `ls-tree -r --name-only HEAD` 找 `*/SKILL.md`（38 ms）。
//! 3. `sparse-checkout set --cone <dir>` + `checkout`（1355 ms）——只把那一个目录铺开。
//!
//! 第 3 步之后 `<cache>/<dir>` 就是一个**普通的 skill 目录**，frontmatter、文件清单、
//! 风险扫描全部走 [`skills::describe_body`]，和本地面板同一份代码 —— 没有任何
//! 「网络版」的分支。装之前那一屏要回答的就是「这东西长什么样」，两处答案不一样
//! 就等于没答。
//!
//! **缓存是加速，不是数据。** 整个删掉不影响任何已装的 skill；同仓库的第二个 skill
//! 从 3.3 秒降到 ~1.4 秒（只多一次 sparse-checkout）。而 `mattpocock/skills`、
//! `anthropics/claude-code` 这种一个仓库出好几条结果的情况很常见。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::SystemTime;

use serde::Serialize;
use tauri::AppHandle;

use super::risk::{self, RiskFinding, RiskLevel};
use super::skills::{SkillFile, SkillFrontmatter};

/// 缓存目录的个数上限。一个 clone 通常 130 KB ~ 700 KB，30 个仓库是几十 MB 的量级。
pub const MAX_REPOS: usize = 30;
/// 缓存目录的总量上限。
pub const MAX_BYTES: u64 = 200 * 1024 * 1024;

/// 一次 git 子进程的墙钟上限。实测最慢的一步（大仓库 clone）3.3 秒，留足余量；
/// 真正要挡的是「对面不回包，子进程挂在那儿」——那时候界面上是个永远转的骨架屏。
const GIT_TIMEOUT_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// 对外形状
// ---------------------------------------------------------------------------

/// 装之前能看清楚的全部东西。
///
/// `frontmatter` / `files` / `findings` / `risk` / `truncated` 五个字段的类型
/// **全是本地 Skills 详情用的那几个**，所以详情三节的 Vue 片段能原样搬过来。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryPreview {
    pub source: String,
    pub skill_id: String,
    /// 仓库内路径，例如 `skills/prototype`。阶段 D 的更新要靠它再定位一次。
    pub repo_path: String,
    /// 同名目录出现在多处时，没被选中的那些。摊在详情里让用户自己看清楚装的是哪一个
    /// （方案文档 2.4）。只有一条命中时是空的。
    pub other_paths: Vec<String>,
    /// 安装时那一刻的 HEAD，短 sha。
    pub commit: String,
    /// 这个子目录的 tree sha。**比较它才知道要不要更新** —— 仓库里别处的提交不该
    /// 让一个没变过的 skill 显示「有更新」。
    pub tree_sha: String,
    pub frontmatter: Option<SkillFrontmatter>,
    pub files: Vec<SkillFile>,
    pub findings: Vec<RiskFinding>,
    pub risk: RiskLevel,
    pub truncated: bool,
    /// 这一次有没有省掉 clone。耗时差两个数量级，界面上不显示，但它是
    /// 「第二个 skill 明显更快」这条验收的唯一证据。
    pub cached: bool,
}

/// 预览取不到的原因。
///
/// 分类而不是甩一句 git 的 stderr：面板有四种语言，而「这个仓库不存在」和
/// 「这个仓库里没有这个 skill」对用户意味着完全不同的下一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewErrKind {
    /// 域名源。没有任何公开的取内容路径（方案文档 1.4）。
    NotInstallable,
    /// 缓存目录不可用（拿不到数据目录、建不出来）。
    Cache,
    /// 克隆不下来：断网、仓库不存在、私有仓库。
    Clone,
    /// 克隆下来了，但里面没有这个 skill。
    NotFound,
    /// 找到了却检出不了。
    Checkout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewError {
    pub kind: PreviewErrKind,
    /// 底层原文，给「展开详情」用。界面默认只显示按 `kind` 翻出来的那句话。
    pub detail: String,
}

impl PreviewError {
    fn new(kind: PreviewErrKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// 缓存根目录
// ---------------------------------------------------------------------------

static CACHE_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 在 app setup 里调一次。拿不到数据目录时预览整体停用 —— 搜索照常，
/// 点开详情会说「缓存目录不可用」，而不是在某个中间步骤崩掉。
pub fn init(app: &AppHandle) {
    let dir = crate::app_storage::data_dir(app)
        .ok()
        .map(|root| root.join("skill-registry"))
        .filter(|dir| std::fs::create_dir_all(dir).is_ok());
    let _ = CACHE_ROOT.set(dir);
}

fn cache_root() -> Option<&'static Path> {
    CACHE_ROOT.get()?.as_deref()
}

/// `emilkowalski/skills` → `emilkowalski__skills`。
///
/// 不用斜杠建两层：一层目录才能按「一个仓库一个条目」做清理和计数。
/// 来源已经过了 [`super::registry::installable_source`] 的白名单，
/// 所以这里不会有 `..`、路径分隔符或其它要转义的东西。
pub fn cache_key(source: &str) -> String {
    source.replace('/', "__")
}

// ---------------------------------------------------------------------------
// 定位
// ---------------------------------------------------------------------------

/// 在 `ls-tree` 的输出里挑出这个 skill 的目录。
///
/// **必须真的去树里找，不能猜。** 早一轮对七个仓库做过路径探测（直接猜
/// `skills/<slug>`、`<slug>`、`.claude/skills/<slug>` …），只命中 3/7；实际见到的布局有
/// `skills/`、`.claude/skills/`、`.agents/skills/`、`.agent/skills/`、
/// `plugins/<x>/skills/`、`config/skills/`。而 `ls-tree -r` 只要 19–38 ms。
///
/// 规则：在所有 `*/SKILL.md` 里挑**父目录名 == `skill_id`** 的。多条命中时取路径最短
/// 的那条（顶层的那份通常才是正主），其余原样返回，在详情里摊给用户看。
///
/// 返回 `(选中的目录, 其余候选)`。一条都没有时返回 `None`。
pub fn locate(paths: &[String], skill_id: &str) -> Option<(String, Vec<String>)> {
    let mut hits: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let dir = p.strip_suffix("/SKILL.md")?;
            (leaf(dir) == skill_id).then(|| dir.to_string())
        })
        .collect();
    if hits.is_empty() {
        return None;
    }
    // 路径长度相同时按字典序，否则同一个仓库两次预览可能选中不同的目录 ——
    // 那会让「装的是哪一个」这件事变成掷骰子。
    hits.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    let chosen = hits.remove(0);
    Some((chosen, hits))
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 大小写不敏感地认 `SKILL.md`：`ls-tree` 给的是仓库里的原样拼写，而实际见过
/// `Skill.md`。统一成 `<dir>/SKILL.md` 再交给 [`locate`]。
pub fn skill_md_paths(ls_tree: &str) -> Vec<String> {
    ls_tree
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let name = leaf(line);
            name.eq_ignore_ascii_case("SKILL.md").then(|| {
                let dir = &line[..line.len() - name.len()];
                format!("{dir}SKILL.md")
            })
        })
        .filter(|p| p.contains('/'))
        .collect()
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// 每个 git 子进程都带上的一串 `-c`。
///
/// 三条都在挡同一类事：**这是个陌生人的仓库地址**。
/// - `credential.helper=`：不动用户的凭据。私有仓库就该失败，不该拿着用户的 token 去试。
/// - `protocol.ext.allow=never` / `protocol.file.allow=never`：`ext::` 是 git 的
///   「拿这行当命令跑」传输协议。URL 是我们自己拼的，这条是第二道。
/// - `core.askPass=` + 环境变量 `GIT_TERMINAL_PROMPT=0`：不弹密码框把子进程卡住。
fn hardening() -> Vec<&'static str> {
    vec![
        "-c",
        "credential.helper=",
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "protocol.file.allow=never",
        "-c",
        "core.askPass=",
    ]
}

/// 浅克隆的 argv（不含 `git` 本身）。抽出来是为了能钉住那几个开关 ——
/// 少一个 `--filter=blob:none`，一个 pytorch 级别的仓库就是几百 MB 而不是 692 KB。
pub fn clone_args(url: &str, dest: &str) -> Vec<String> {
    let mut args: Vec<String> = hardening().iter().map(|a| a.to_string()).collect();
    args.extend(
        [
            "clone",
            "--depth",
            "1",
            "--filter=blob:none",
            "--no-checkout",
            "--single-branch",
            "--quiet",
        ]
        .iter()
        .map(|a| a.to_string()),
    );
    args.push(url.to_string());
    args.push(dest.to_string());
    args
}

/// 仓库地址**由我们拼**，不接受任何用户给的完整 URL（方案文档 4.3）。
pub fn repo_url(source: &str) -> String {
    format!("https://github.com/{source}.git")
}

fn git(cwd: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        // 交互式的一切都不要：子进程卡在一个没人看得见的提示上，界面表现是骨架屏永远转。
        .env("GIT_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null());
    run_with_timeout(cmd, args)
}

/// 起子进程并等它，超时就杀掉。
///
/// `Command::output()` 没有超时；对面不回包时它会一直等下去，而这条路径是用户点一下
/// 就走一次的。
fn run_with_timeout(mut cmd: Command, args: &[&str]) -> Result<String, String> {
    // 报错里只提子命令，不回显整条 argv：那里面有仓库地址和一串 `-c`，对用户没用。
    let what = || {
        args.iter()
            .filter(|a| !a.starts_with('-') && !a.contains('='))
            .take(1)
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("git {}: {e}", what()))?;
    let deadline = SystemTime::now() + std::time::Duration::from_secs(GIT_TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if SystemTime::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("git {} timed out", what()));
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            Err(e) => return Err(format!("git {}: {e}", what())),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git {}: {e}", what()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() {
            format!("git {} failed", what())
        } else {
            err.to_string()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ---------------------------------------------------------------------------
// 预览
// ---------------------------------------------------------------------------

/// 把 `source` 里的 `skill_id` 取到 `root` 下面，并描述它。
///
/// `root` 作参数而不是直接读 `cache_root()`：测试要能指到一个临时目录。
/// `refresh` = 忘掉缓存里那份，重新克隆。缓存是浅克隆，`ls-tree` 只看得见克隆那一刻
/// 的 HEAD —— 仓库更新了之后，不重新克隆是看不到的。
pub fn preview_in(
    root: &Path,
    source: &str,
    skill_id: &str,
    refresh: bool,
) -> Result<RegistryPreview, PreviewError> {
    if !super::registry::installable_source(source) {
        return Err(PreviewError::new(
            PreviewErrKind::NotInstallable,
            source.to_string(),
        ));
    }
    let repo = root.join(cache_key(source));
    if refresh {
        let _ = std::fs::remove_dir_all(&repo);
    }
    let cached = repo.join(".git").is_dir();
    if !cached {
        // 上一次失败可能留下半个目录，clone 会拒绝往非空目录里写。
        let _ = std::fs::remove_dir_all(&repo);
        let args = clone_args(&repo_url(source), &repo.to_string_lossy());
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        git(None, &argv).map_err(|e| {
            let _ = std::fs::remove_dir_all(&repo);
            PreviewError::new(PreviewErrKind::Clone, e)
        })?;
    }
    // 命中缓存时顺手把 mtime 顶上去 —— 清理按 mtime 做 LRU，不碰的话
    // 一个天天在用的仓库会因为「克隆得早」被删掉。
    touch(&repo);

    let tree = git(Some(&repo), &["ls-tree", "-r", "--name-only", "HEAD"])
        .map_err(|e| PreviewError::new(PreviewErrKind::Clone, e))?;

    let (repo_path, other_paths) = locate(&skill_md_paths(&tree), skill_id).ok_or_else(|| {
        PreviewError::new(
            PreviewErrKind::NotFound,
            format!("{source} has no {skill_id}/SKILL.md"),
        )
    })?;

    checkout(&repo, &repo_path).map_err(|e| PreviewError::new(PreviewErrKind::Checkout, e))?;

    let dir = repo.join(&repo_path);
    if !dir.is_dir() {
        return Err(PreviewError::new(
            PreviewErrKind::Checkout,
            format!("{repo_path} did not appear after checkout"),
        ));
    }
    let (files, findings, frontmatter, truncated) = super::skills::describe_body(&dir);

    Ok(RegistryPreview {
        commit: short_sha(&rev_parse(&repo)),
        tree_sha: tree_sha(&repo, &repo_path),
        risk: risk::highest(&findings),
        source: source.to_string(),
        skill_id: skill_id.to_string(),
        repo_path,
        other_paths,
        frontmatter,
        files,
        findings,
        truncated,
        cached,
    })
}

/// 只铺开一个目录。`--cone` 模式下 `set` 是**替换**不是追加 —— 同一个仓库看第二个
/// skill 时上一个会被收回去，缓存目录不会随着点开的条数无限长大。
fn checkout(repo: &Path, repo_path: &str) -> Result<(), String> {
    git(Some(repo), &["sparse-checkout", "set", "--cone", repo_path])?;
    git(Some(repo), &["checkout", "--quiet"])?;
    Ok(())
}

fn rev_parse(repo: &Path) -> String {
    git(Some(repo), &["rev-parse", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 这个子目录的 tree sha。`ls-tree HEAD -- <path>` 的输出形如
/// `040000 tree 8c4d…\tskills/prototype`。
fn tree_sha(repo: &Path, repo_path: &str) -> String {
    let out = git(Some(repo), &["ls-tree", "HEAD", "--", repo_path]).unwrap_or_default();
    parse_tree_sha(&out)
}

pub fn parse_tree_sha(ls_tree_line: &str) -> String {
    ls_tree_line
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let _mode = parts.next()?;
            (parts.next()? == "tree").then(|| parts.next().unwrap_or_default().to_string())
        })
        .unwrap_or_default()
}

/// 短 sha 只用来显示。**不拿它做判据** —— 更新与否比的是 `tree_sha`。
pub fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn touch(repo: &Path) {
    let stamp = repo.join(".session-viewer-touched");
    let _ = std::fs::write(&stamp, b"");
}

// ---------------------------------------------------------------------------
// 缓存清理
// ---------------------------------------------------------------------------

/// 把缓存压回上限以内，按整个仓库目录删。
///
/// **`storage_gc::prune` 用不了**：它 `remove_file`，只删文件，删不掉一个 clone
/// 目录（删完只剩一棵空目录树，而 `.git` 里少几个 object 的仓库比没有更糟 ——
/// 后面每次 `ls-tree` 都会失败，且不会自愈）。所以这里整个目录一起删。
///
/// 判据是 mtime 的 LRU：最近用过的留下。缓存丢了不影响任何已装的 skill。
pub fn prune_in(root: &Path, max_repos: usize, max_bytes: u64) -> crate::storage_gc::Pruned {
    let mut repos: Vec<(PathBuf, SystemTime, u64)> = match std::fs::read_dir(root) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| {
                let path = e.path();
                let modified = newest_mtime(&path);
                let bytes = crate::storage_gc::total_bytes(&path);
                (path, modified, bytes)
            })
            .collect(),
        Err(_) => return crate::storage_gc::Pruned::default(),
    };
    // 最近用过的排前面，从尾巴上删。
    repos.sort_by_key(|repo| std::cmp::Reverse(repo.1));

    let mut total: u64 = repos.iter().map(|r| r.2).sum();
    let mut pruned = crate::storage_gc::Pruned::default();
    while repos.len() > max_repos || (total > max_bytes && !repos.is_empty()) {
        let (path, _, bytes) = repos.pop().expect("non-empty");
        if std::fs::remove_dir_all(&path).is_ok() {
            pruned.files += 1;
            pruned.bytes += bytes;
        }
        total = total.saturating_sub(bytes);
    }
    pruned
}

/// 一个仓库目录「最后一次被用到」是什么时候。
///
/// 取目录树里最新的那个 mtime，而不是目录自身的：在 macOS 上往子目录里写东西
/// 不会更新祖先目录的 mtime，光看顶层会把一个刚 sparse-checkout 过的仓库
/// 判成「克隆那天之后就没动过」。[`touch`] 写的那个戳也是走这条路被看见的。
fn newest_mtime(dir: &Path) -> SystemTime {
    let mut newest = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return newest;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let m = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if m > newest {
            newest = m;
        }
    }
    newest
}

/// 挂在启动 gc 旁边。
pub fn prune() -> crate::storage_gc::Pruned {
    match cache_root() {
        Some(root) => prune_in(root, MAX_REPOS, MAX_BYTES),
        None => crate::storage_gc::Pruned::default(),
    }
}

/// 整个清空（存储面板的「清理」按钮）。返回释放的字节数。
pub fn clear() -> u64 {
    let Some(root) = cache_root() else { return 0 };
    let freed = crate::storage_gc::total_bytes(root);
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::create_dir_all(root);
    freed
}


// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn tools_registry_preview(
    source: String,
    skill_id: String,
    refresh: bool,
) -> Result<RegistryPreview, PreviewError> {
    let root = cache_root().ok_or_else(|| {
        PreviewError::new(PreviewErrKind::Cache, "no data directory".to_string())
    })?;
    preview_in(root, &source, &skill_id, refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("cc-reg-git-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // -----------------------------------------------------------------------
    // 定位
    // -----------------------------------------------------------------------

    /// 早一轮对七个仓库猜过路径（`skills/<slug>`、`<slug>`、`.claude/skills/<slug>` …），
    /// 只命中 3/7。这个测试就是那 7 个仓库的真实布局。
    #[test]
    fn the_skill_directory_is_found_wherever_the_repo_decided_to_put_it() {
        for layout in [
            "skills/prototype/SKILL.md",
            "prototype/SKILL.md",
            ".claude/skills/prototype/SKILL.md",
            ".agents/skills/prototype/SKILL.md",
            ".agent/skills/prototype/SKILL.md",
            "plugins/web/skills/prototype/SKILL.md",
            "config/skills/prototype/SKILL.md",
        ] {
            let got = locate(&paths(&[layout, "README.md"]), "prototype");
            assert_eq!(
                got.map(|(dir, _)| dir),
                Some(layout.trim_end_matches("/SKILL.md").to_string()),
                "{layout}"
            );
        }
    }

    /// 挑的是**父目录名**等于 skillId 的那条，不是「路径里出现过这个词」的那条。
    /// 放松成 `contains` 的话 `typescript` 会命中 `typescript-advanced-types`，
    /// 装下来是另一个 skill，而且看不出错。
    #[test]
    fn a_directory_that_merely_contains_the_name_is_not_a_match() {
        let list = paths(&[
            "skills/typescript-advanced-types/SKILL.md",
            "skills/my-typescript/SKILL.md",
            "docs/typescript/README.md",
        ]);
        assert!(locate(&list, "typescript").is_none());
    }

    /// 同名目录出现在多处时取路径最短的那条，其余原样带出来 —— 详情里要摊给用户看
    /// 「装的是哪一个」。
    #[test]
    fn the_shortest_path_wins_and_the_others_are_still_reported() {
        let list = paths(&[
            "plugins/legacy/skills/prototype/SKILL.md",
            "skills/prototype/SKILL.md",
            "vendor/x/skills/prototype/SKILL.md",
        ]);
        let (chosen, others) = locate(&list, "prototype").unwrap();
        assert_eq!(chosen, "skills/prototype");
        assert_eq!(others.len(), 2);
    }

    /// 长度打平时按字典序。不定序的话同一个仓库两次预览可能选中不同的目录 ——
    /// 「装的是哪一个」会变成掷骰子，而且两次都不会报错。
    #[test]
    fn two_candidates_of_equal_length_still_resolve_the_same_way_every_time() {
        let a = paths(&["b/x/skills/prototype/SKILL.md", "a/y/skills/prototype/SKILL.md"]);
        let b = paths(&["a/y/skills/prototype/SKILL.md", "b/x/skills/prototype/SKILL.md"]);
        assert_eq!(locate(&a, "prototype").unwrap().0, locate(&b, "prototype").unwrap().0);
        assert_eq!(locate(&a, "prototype").unwrap().0, "a/y/skills/prototype");
    }

    #[test]
    fn a_repo_without_that_skill_reports_nothing_rather_than_guessing() {
        assert!(locate(&paths(&["skills/other/SKILL.md"]), "prototype").is_none());
        assert!(locate(&[], "prototype").is_none());
    }

    /// 仓库里的原样拼写见过 `Skill.md`。大小写敏感地比会让那些仓库整个「找不到」。
    #[test]
    fn the_file_name_is_matched_without_caring_about_case() {
        let tree = "skills/prototype/Skill.md\nskills/other/SKILL.md\nREADME.md\n";
        let found = skill_md_paths(tree);
        assert_eq!(found, vec!["skills/prototype/SKILL.md", "skills/other/SKILL.md"]);
    }

    /// 仓库根上的 `SKILL.md` 没有父目录可比，认它就等于「随便哪个 skillId 都命中」。
    #[test]
    fn a_skill_md_at_the_repo_root_is_not_a_candidate() {
        assert!(skill_md_paths("SKILL.md\n").is_empty());
    }

    // -----------------------------------------------------------------------
    // git 的调用形状
    // -----------------------------------------------------------------------

    /// 少一个 `--filter=blob:none`，pytorch 级别的仓库就是几百 MB 而不是 692 KB；
    /// 少一个 `--no-checkout`，clone 完就把整个工作区铺在磁盘上了。
    #[test]
    fn the_clone_stays_shallow_blobless_and_unchecked_out() {
        let args = clone_args("https://github.com/a/b.git", "/tmp/x");
        for needed in [
            "--depth",
            "1",
            "--filter=blob:none",
            "--no-checkout",
            "--single-branch",
        ] {
            assert!(args.iter().any(|a| a == needed), "缺 {needed}: {args:?}");
        }
    }

    /// 这是在陌生人的地址上起子进程。三条 `-c` 一条都不能少。
    #[test]
    fn the_clone_never_touches_the_users_credentials_or_gits_command_transports() {
        let args = clone_args("https://github.com/a/b.git", "/tmp/x");
        for needed in [
            "credential.helper=",
            "protocol.ext.allow=never",
            "protocol.file.allow=never",
        ] {
            assert!(args.iter().any(|a| a == needed), "缺 {needed}: {args:?}");
        }
    }

    /// URL 由我们拼。接受用户给的完整 URL 就等于把 `ext::` / `file://` 放进来。
    #[test]
    fn the_repository_url_is_built_here_not_taken_from_anyone() {
        assert_eq!(repo_url("a/b"), "https://github.com/a/b.git");
    }

    #[test]
    fn the_cache_directory_is_one_level_per_repository() {
        assert_eq!(cache_key("emilkowalski/skills"), "emilkowalski__skills");
    }

    // -----------------------------------------------------------------------
    // 输出解析
    // -----------------------------------------------------------------------

    #[test]
    fn the_subdirectory_tree_sha_is_read_out_of_ls_tree() {
        let line = "040000 tree 8c4dabc1234567890abcdef1234567890abcdef12\tskills/prototype\n";
        assert_eq!(
            parse_tree_sha(line),
            "8c4dabc1234567890abcdef1234567890abcdef12"
        );
    }

    /// 路径是文件时 `ls-tree` 给的是 `blob`。拿 blob 的 sha 当 tree sha 会让更新判断
    /// 在一个不存在的目录上比来比去。
    #[test]
    fn a_blob_is_not_mistaken_for_a_tree() {
        let line = "100644 blob 8c4dabc…\tREADME.md\n";
        assert_eq!(parse_tree_sha(line), "");
        assert_eq!(parse_tree_sha(""), "");
    }

    #[test]
    fn the_commit_is_shortened_only_for_display() {
        assert_eq!(short_sha("3f2a1bcdeadbeef"), "3f2a1bc");
        assert_eq!(short_sha(""), "");
    }

    // -----------------------------------------------------------------------
    // 缓存清理
    // -----------------------------------------------------------------------

    fn fake_repo(root: &Path, name: &str, bytes: usize, age: std::time::Duration) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("pack"), vec![0u8; bytes]).unwrap();
        let when = filetime::FileTime::from_system_time(SystemTime::now() - age);
        filetime::set_file_mtime(dir.join(".git").join("pack"), when).unwrap();
        filetime::set_file_mtime(&dir, when).unwrap();
    }

    /// `storage_gc::prune` 只 `remove_file`。用它清这儿会留下一棵空目录树 + 一个
    /// 缺了 object 的 `.git` —— 那比没有缓存更糟：之后每次 `ls-tree` 都失败，
    /// 而且不会自愈。所以这里整个目录一起删。
    #[test]
    fn a_repository_is_removed_whole_never_file_by_file() {
        let root = scratch("whole");
        fake_repo(&root, "a__b", 10, std::time::Duration::from_secs(0));
        prune_in(&root, 0, 0);
        assert!(!root.join("a__b").exists(), "整个目录该消失");
        std::fs::remove_dir_all(&root).ok();
    }

    /// LRU 按 mtime：最近用过的留下。
    #[test]
    fn the_least_recently_used_repository_is_the_one_that_goes() {
        let root = scratch("lru");
        fake_repo(&root, "old__one", 10, std::time::Duration::from_secs(86_400));
        fake_repo(&root, "new__one", 10, std::time::Duration::from_secs(1));
        let pruned = prune_in(&root, 1, u64::MAX);
        assert_eq!(pruned.files, 1);
        assert!(root.join("new__one").exists());
        assert!(!root.join("old__one").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    /// 个数没超但总量超了，也要删到上限以内。
    #[test]
    fn the_byte_ceiling_bites_even_when_the_count_is_fine() {
        let root = scratch("bytes");
        fake_repo(&root, "a__a", 4096, std::time::Duration::from_secs(86_400));
        fake_repo(&root, "b__b", 4096, std::time::Duration::from_secs(1));
        let pruned = prune_in(&root, 30, 5000);
        assert_eq!(pruned.files, 1);
        assert!(root.join("b__b").exists(), "最近用过的留下");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nothing_is_removed_while_both_ceilings_hold() {
        let root = scratch("ok");
        fake_repo(&root, "a__a", 10, std::time::Duration::from_secs(1));
        assert_eq!(prune_in(&root, 30, MAX_BYTES), crate::storage_gc::Pruned::default());
        assert!(root.join("a__a").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    /// 缓存根不存在时（第一次启动、用户手动删了）不能 panic —— 它挂在启动 gc 上。
    #[test]
    fn a_missing_cache_root_is_not_an_error() {
        let root = std::env::temp_dir().join(format!("cc-reg-git-none-{}", uuid::Uuid::new_v4()));
        assert_eq!(prune_in(&root, 30, MAX_BYTES), crate::storage_gc::Pruned::default());
    }

    // -----------------------------------------------------------------------
    // 预览的前置拦截
    // -----------------------------------------------------------------------

    /// 域名源没有任何公开的取内容路径。走到 `git clone` 才失败的话，用户要等 3 秒
    /// 才看到一句 git 的英文报错。
    #[test]
    fn a_domain_source_is_refused_before_any_subprocess_starts() {
        let root = scratch("domain");
        let err = preview_in(&root, "code.deepline.com", "x", false).unwrap_err();
        assert_eq!(err.kind, PreviewErrKind::NotInstallable);
        assert!(!root.join("code.deepline.com").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    /// 白名单挡在起子进程之前，所以这些字符串一次都到不了 `git`。
    #[test]
    fn nothing_that_could_reach_git_as_something_else_starts_a_subprocess() {
        let root = scratch("inject");
        for source in [
            "ext::sh -c whoami/x",
            "-upload-pack/x",
            "../etc/passwd",
            "https://github.com/a/b.git",
        ] {
            assert_eq!(
                preview_in(&root, source, "x", false).unwrap_err().kind,
                PreviewErrKind::NotInstallable,
                "{source}"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
