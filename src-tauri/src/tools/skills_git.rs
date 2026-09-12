// 工具管理 · 从远端仓库拉来的 skill 的更新。
//
// 本机的实测形状（写这一版时）：`~/.skills-manager/skills/humanizer`、
// `~/.cc-switch/skills/humanizer`、`~/.agents/skills/humanizer` 是**三份各自独立的
// clone**（`git rev-parse --show-toplevel` 就是它自己），origin 指向
// `https://github.com/blader/humanizer.git`；`dm-watch` 也是个 git 仓库，但**没有
// remote**，而且工作区里有三个文件被改过。
//
// 所以两条判定必须分开：
//
// - **顶层就是这个目录**。skill 目录只是某个大仓库（dotfiles 之类）里的一个子目录时，
//   `reset --hard` 会把整个仓库里其它无关的改动一起冲掉。这种一律不认。
// - **有 remote**。`dm-watch` 那种本地仓库没地方可拉，给个更新按钮是在骗人。
//
// 判定只读两个文件（`.git/config`、`.git/HEAD`），不起子进程 —— 每开一次详情页都要
// 算一次。真要动手（fetch / reset）时才调 git 命令。

use std::path::Path;
use std::process::Command;

use serde::Serialize;

/// 一份能从远端更新的内容。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillGit {
    /// origin 的 URL，原样给用户看 —— 弹框里要说清楚「从哪儿拉」。
    pub remote: String,
    pub branch: String,
}

/// fetch 之后的对比结果，只用来填那个二次确认框。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUpdateCheck {
    pub remote: String,
    pub branch: String,
    /// 本地 HEAD 的短 sha。
    pub local: String,
    /// 远端最新的短 sha。
    pub latest: String,
    /// 落后几个提交。0 表示已经是最新的。
    pub behind: usize,
    /// 被改过 / 删过的**已跟踪**文件。这些正是 `reset --hard` 会冲掉的东西。
    pub changed: Vec<String>,
    /// 没被 git 跟踪的文件个数。`reset --hard` 不动它们，弹框里要讲明白，
    /// 否则用户会以为自己加的笔记也没了。
    pub untracked: usize,
}

/// 这份内容是不是一个能更新的 clone。
///
/// `.git` 是文件（worktree / submodule 的 gitdir 指针）时不认：那种的顶层在别处，
/// 在这儿 `reset --hard` 动的是另一个目录。
pub fn detect(body: &Path) -> Option<SkillGit> {
    let git_dir = body.join(".git");
    if !git_dir.is_dir() {
        return None;
    }
    let branch = head_branch(&git_dir)?;
    let remote = origin_url(&git_dir)?;
    Some(SkillGit { remote, branch })
}

/// `.git/HEAD` → 当前分支名。游离 HEAD 认不出分支，也就没有「拉哪一支」可言。
fn head_branch(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let name = head.trim().strip_prefix("ref: refs/heads/")?;
    (!name.is_empty()).then(|| name.to_string())
}

/// `.git/config` 里 `[remote "origin"]` 那一段的 url。
///
/// 手写的小解析器而不是起一个 `git config` 进程：这函数在每次打开详情页时都会跑。
/// git 的 config 是 INI：节标题 `[remote "origin"]`，键值 `key = value`。
fn origin_url(git_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(git_dir.join("config")).ok()?;
    let mut in_origin = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line.starts_with("[remote \"origin\"]");
            continue;
        }
        if !in_origin {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "url" {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
    None
}

/// 拉一次远端，算出「更新会发生什么」。
///
/// 只 fetch，不动工作区 —— 这一步是给二次确认框攒信息的，用户还没点「确定」。
pub fn check(body: &Path) -> Result<SkillUpdateCheck, String> {
    let info = detect(body).ok_or_else(|| {
        format!(
            "{} is not a clone with a remote — nothing to update from",
            body.display()
        )
    })?;

    fetch(body, &info.branch)?;

    let local = git(body, &["rev-parse", "--short", "HEAD"])?;
    let latest = git(body, &["rev-parse", "--short", "FETCH_HEAD"])?;
    let behind = git(body, &["rev-list", "--count", "HEAD..FETCH_HEAD"])?
        .parse::<usize>()
        .unwrap_or(0);

    let (changed, untracked) = parse_status(&git_raw(body, &["status", "--porcelain"])?);

    Ok(SkillUpdateCheck {
        remote: info.remote,
        branch: info.branch,
        local,
        latest,
        behind,
        changed,
        untracked,
    })
}

/// 强制更新到远端最新：`fetch` + `reset --hard FETCH_HEAD`。
///
/// **不跑 `git clean`。** 用户改过的已跟踪文件被覆盖是这次操作说好的代价，但他自己
/// 新加的、git 根本没跟踪的文件（笔记、草稿）不在这个约定里，顺手删掉是越权。
pub fn update(body: &Path) -> Result<String, String> {
    let info = detect(body).ok_or_else(|| {
        format!(
            "{} is not a clone with a remote — nothing to update from",
            body.display()
        )
    })?;
    fetch(body, &info.branch)?;
    git(body, &["reset", "--hard", "--quiet", "FETCH_HEAD"])?;
    git(body, &["rev-parse", "--short", "HEAD"])
}

/// 拉一支分支。这是整个流程里**唯一一步联网**的。
///
/// 带上低速阈值：卡住的传输 20 秒没动静就放弃。不带的话 git 会一直等下去，而面板
/// 的 `busy` 是跟着这一步走的 —— 一次卡死的 fetch 等于整个面板再也点不动，连
/// 「取消」都没有。连不上远端本来就该是一条报错，不是无限转圈。
fn fetch(body: &Path, branch: &str) -> Result<(), String> {
    git(
        body,
        &[
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=20",
            "fetch",
            "--quiet",
            "origin",
            branch,
        ],
    )?;
    Ok(())
}

/// `git status --porcelain` → （会被冲掉的已跟踪文件，未跟踪文件个数）。
///
/// 这个格式是**定宽**的：前两列是状态码，第三列是空格，路径从第 4 个字节起。所以
/// 传进来的必须是**没 trim 过**的 stdout —— ` M SKILL.md` 被 trim 掉行首那个空格之后，
/// 第一行的路径会整体左移一位，切出来是 `KILL.md`。这是 live 实测踩到的，不是假想。
///
/// 两类分开数而不是合成一个「有 N 处改动」：`reset --hard` 只冲掉已跟踪的那些，
/// 未跟踪的（用户自己加的笔记）一个不动，弹框里要分别说清楚。
fn parse_status(raw: &str) -> (Vec<String>, usize) {
    let mut changed = Vec::new();
    let mut untracked = 0usize;
    for line in raw.lines() {
        let Some(path) = line.get(3..).map(str::trim_end) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        if line.starts_with("??") {
            untracked += 1;
        } else {
            changed.push(path.to_string());
        }
    }
    (changed, untracked)
}

/// 在 `body` 里跑一条 git，返回去掉首尾空白的 stdout。sha、计数这些用它。
fn git(body: &Path, args: &[&str]) -> Result<String, String> {
    Ok(git_raw(body, args)?.trim().to_string())
}

/// 同上，但 stdout 一个字节都不动。列对齐的输出（`status --porcelain`）只能用这个。
///
/// 参数是数组，不拼 shell 字符串 —— 路径里有空格、引号、`;` 都只是路径的一部分。
fn git_raw(body: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(body)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            err.to_string()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn tools_check_skill_update(body: String) -> Result<SkillUpdateCheck, String> {
    check(Path::new(&body))
}

#[tauri::command(async)]
pub fn tools_update_skill(body: String) -> Result<String, String> {
    update(Path::new(&body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 每个用例一个自己的临时根，跑完就删。同一次 `cargo test` 里用例是并行的，
    /// 只按 pid 命名会互相踩。
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "cc-sessions-viewer-skill-git-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// `.git` 目录 + config + HEAD，不起真的 git 进程。
    fn fake_clone(root: &Path, config: &str, head: &str) {
        write(&root.join(".git").join("config"), config);
        write(&root.join(".git").join("HEAD"), head);
    }

    const ORIGIN: &str = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/blader/humanizer.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[branch \"main\"]\n\tremote = origin\n";

    #[test]
    fn a_clone_with_an_origin_is_updatable() {
        let tmp = Tmp::new();
        let body = tmp.path().join("humanizer");
        fake_clone(&body, ORIGIN, "ref: refs/heads/main\n");

        let hit = detect(&body).expect("a clone with an origin must be detected");
        assert_eq!(hit.remote, "https://github.com/blader/humanizer.git");
        assert_eq!(hit.branch, "main");
    }

    #[test]
    fn a_local_repo_without_a_remote_is_not_updatable() {
        // 本机的 `dm-watch` 就是这样：是个 git 仓库，但没地方可拉。给它一个更新按钮
        // 等于骗用户点一下会有新版本。
        let tmp = Tmp::new();
        let body = tmp.path().join("dm-watch");
        fake_clone(&body, "[core]\n\tbare = false\n", "ref: refs/heads/main\n");
        assert!(detect(&body).is_none());
    }

    #[test]
    fn a_plain_directory_is_not_updatable() {
        let tmp = Tmp::new();
        let body = tmp.path().join("hand-written");
        fs::create_dir_all(&body).unwrap();
        write(&body.join("SKILL.md"), "---\nname: x\n---\n");
        assert!(detect(&body).is_none());
    }

    #[test]
    fn a_subdirectory_of_a_repo_is_not_updatable() {
        // dotfiles 仓库里的 `skills/foo`：它自己没有 `.git`，顶层在上面几层。
        // 在这儿 reset --hard 冲掉的是整个 dotfiles 仓库的改动。
        let tmp = Tmp::new();
        fake_clone(tmp.path(), ORIGIN, "ref: refs/heads/main\n");
        let body = tmp.path().join("skills").join("foo");
        fs::create_dir_all(&body).unwrap();
        assert!(detect(&body).is_none());
    }

    #[test]
    fn a_gitdir_pointer_file_is_not_updatable() {
        // submodule / worktree：`.git` 是文件不是目录，真正的仓库在别处。
        let tmp = Tmp::new();
        let body = tmp.path().join("as-submodule");
        write(&body.join(".git"), "gitdir: ../../.git/modules/skill\n");
        assert!(detect(&body).is_none());
    }

    #[test]
    fn a_detached_head_is_not_updatable() {
        // 游离 HEAD 认不出分支，也就没有「拉哪一支」可言。
        let tmp = Tmp::new();
        let body = tmp.path().join("detached");
        fake_clone(&body, ORIGIN, "9f1c0f4a1b2c3d4e5f60718293a4b5c6d7e8f901\n");
        assert!(detect(&body).is_none());
    }

    #[test]
    fn the_origin_url_is_read_from_the_right_section() {
        // `[remote "upstream"]` 在前面时不能把它的 url 当成 origin 的。
        let tmp = Tmp::new();
        let body = tmp.path().join("two-remotes");
        fake_clone(
            &body,
            "[remote \"upstream\"]\n\turl = https://github.com/someone-else/fork.git\n[remote \"origin\"]\n\turl = git@github.com:me/mine.git\n",
            "ref: refs/heads/main\n",
        );
        assert_eq!(detect(&body).unwrap().remote, "git@github.com:me/mine.git");
    }

    #[test]
    fn the_first_status_line_keeps_its_whole_filename() {
        // live 实测踩到的：`git()` 会 trim stdout，` M SKILL.md` 行首那个空格一没，
        // 第一行的路径就整体左移一位，弹框里写着「你改过的 KILL.md 会被覆盖」。
        let (changed, untracked) = parse_status(" M SKILL.md\n?? my-notes.txt\n");
        assert_eq!(changed, vec!["SKILL.md"]);
        assert_eq!(untracked, 1);
    }

    #[test]
    fn every_status_code_lands_on_the_right_column() {
        let (changed, untracked) = parse_status(
            "MM staged-and-dirty.md\n D deleted.md\nA  added.md\nR  old.md -> new.md\n?? a\n?? b\n",
        );
        assert_eq!(
            changed,
            vec![
                "staged-and-dirty.md",
                "deleted.md",
                "added.md",
                "old.md -> new.md"
            ]
        );
        assert_eq!(untracked, 2);
    }

    #[test]
    fn a_clean_worktree_has_nothing_to_lose() {
        let (changed, untracked) = parse_status("");
        assert!(changed.is_empty());
        assert_eq!(untracked, 0);
    }

    #[test]
    fn updating_something_that_is_not_a_clone_is_an_error_not_a_silent_no_op() {
        let tmp = Tmp::new();
        let body = tmp.path().join("hand-written");
        fs::create_dir_all(&body).unwrap();
        assert!(check(&body).is_err());
        assert!(update(&body).is_err());
    }
}
