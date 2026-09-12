// 工具管理 · skill 目录内的文件读写。
//
// 内置编辑器的后端。这个模块只做三件事：列、读、写，全部**锁死在一个 skill 目录内**。
//
// 作用域这件事必须在后端再校验一次，不能只靠前端：
//
// - `rel` 里逐段拒绝 `..` / 绝对路径 / 盘符，这是第一道。
// - 但光看字符串不够 —— skill 目录**里面**可能躺着一条指向外面的软链（`refs -> ~/`），
//   拼出来的路径一个 `..` 都没有，解析之后却在用户主目录里。所以还要把路径解析到真实
//   位置，再确认它仍在这个 skill 目录下面。
// - 写入走的是 `body` 的**真实路径**（`canonicalize` 跟着链接走到底），不经过引用写 ——
//   某些 agent 的 skills 目录可能是只读挂载，而且经链接写会把「改哪一份」变成一个
//   取决于链接拓扑的问题。

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::skills::SkillFile;
use crate::util;

/// 单个文件的读写上限。超了只给前一段并禁止保存 —— 截半截存回去就是数据丢失。
///
/// skill 里最大的东西是 `references/` 下的说明文档，1 MB 已经远超正常范围；真需要
/// 改那种文件的人应该点「用外部编辑器打开」。
const MAX_EDIT_BYTES: u64 = 1024 * 1024;

/// 判二进制时只看开头这么多字节。整文件扫一遍没必要 —— 二进制格式的魔数都在头部。
const SNIFF_BYTES: usize = 8 * 1024;

/// 文件的版本标识，用来挡住「外部改过了还盲写」。
///
/// 不直接用 `util::FileRevision`：它带 `SystemTime`，过不了 serde 这一趟。毫秒够用 ——
/// 我们挡的是用户在别的编辑器里改了这个文件，不是纳秒级的竞态（那一层由
/// `atomic_write_file` 自己的指纹校验兜底）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRev {
    pub exists: bool,
    pub bytes: u64,
    pub modified_ms: Option<i64>,
}

/// 一个文件的内容。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFileText {
    pub rel: String,
    /// 二进制或超限时是**部分内容 / 空串**，别拿它当完整内容存回去。
    pub text: String,
    pub bytes: u64,
    /// 含 NUL 字节。编辑器显示「这是二进制文件」，不给编辑。
    pub binary: bool,
    /// 超过 `MAX_EDIT_BYTES`，`text` 只是开头一段。保存会被拒。
    pub truncated: bool,
    pub rev: FileRev,
}

// ---------------------------------------------------------------------------
// 作用域
// ---------------------------------------------------------------------------

/// 把用户给的 body 路径解析成一个**真实存在的 skill 目录**。
///
/// 要求目录里有 `SKILL.md`：这个模块的每一条命令都以它为根算作用域，根一旦随便什么
/// 目录都能当，`rel` 那几道检查就形同虚设了。
fn skill_root(body: &str) -> Result<PathBuf, String> {
    let real = Path::new(body)
        .canonicalize()
        .map_err(|e| format!("Cannot open {body}: {e}"))?;
    if !real.is_dir() {
        return Err(format!("{body} is not a directory"));
    }
    if !real.join("SKILL.md").is_file() {
        return Err(format!("{body} is not a skill directory (no SKILL.md)"));
    }
    Ok(real)
}

/// `root` 下的相对路径 → 绝对路径，越界就报错。
fn scoped(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if rel.is_empty() || rel_path.is_absolute() {
        return Err(format!("Bad path: {rel}"));
    }
    // 逐段只允许普通名字。`..`、`.`、`/`、`C:` 一律拒 —— 后面的真实路径校验虽然也能
    // 挡住，但那一步要碰磁盘，先用纯字符串挡掉明显的越界更省事也更好读。
    for part in rel_path.components() {
        if !matches!(part, Component::Normal(_)) {
            return Err(format!("Bad path: {rel}"));
        }
    }
    let target = root.join(rel_path);
    ensure_inside(root, &target)?;
    Ok(target)
}

/// 把 `target` 解析到真实位置，确认它仍在 `root`（已经是真实路径）下面。
///
/// 文件可以还不存在（新建），所以从 `target` 往上找到第一个存在的祖先来解析，再把剩下
/// 的段拼回去。挡的是**目录本身是软链**这一类：`root/refs` 指向 `~/`，`refs/a.md`
/// 字面上老实，解析完在主目录里。
fn ensure_inside(root: &Path, target: &Path) -> Result<(), String> {
    let mut probe = target.to_path_buf();
    let mut tail: Vec<OsString> = Vec::new();
    while !probe.exists() {
        let Some(name) = probe.file_name().map(OsString::from) else {
            break;
        };
        let Some(parent) = probe.parent().map(Path::to_path_buf) else {
            break;
        };
        tail.push(name);
        probe = parent;
    }
    let mut real = probe
        .canonicalize()
        .map_err(|e| format!("Cannot resolve {}: {e}", target.display()))?;
    while let Some(name) = tail.pop() {
        real.push(name);
    }
    if !real.starts_with(root) {
        return Err(format!("{} escapes the skill directory", target.display()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 读写
// ---------------------------------------------------------------------------

fn revision(path: &Path) -> FileRev {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return FileRev {
            exists: false,
            bytes: 0,
            modified_ms: None,
        };
    };
    FileRev {
        exists: true,
        bytes: meta.len(),
        modified_ms: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64),
    }
}

pub fn read(body: &str, rel: &str) -> Result<SkillFileText, String> {
    let root = skill_root(body)?;
    let path = scoped(&root, rel)?;
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|e| format!("Cannot read {rel}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{rel} is not a file"));
    }
    let bytes = meta.len();
    let rev = revision(&path);

    let raw = std::fs::read(&path).map_err(|e| format!("Cannot read {rel}: {e}"))?;
    let head = &raw[..raw.len().min(SNIFF_BYTES)];
    if head.contains(&0) {
        return Ok(SkillFileText {
            rel: rel.to_string(),
            text: String::new(),
            bytes,
            binary: true,
            truncated: false,
            rev,
        });
    }

    let truncated = bytes > MAX_EDIT_BYTES;
    let slice = if truncated {
        // 从字节数截断可能落在一个多字节字符中间，`from_utf8_lossy` 会把它变成 U+FFFD。
        // 反正超限的内容本来就禁止保存，这里只求能显示。
        &raw[..MAX_EDIT_BYTES as usize]
    } else {
        &raw[..]
    };
    Ok(SkillFileText {
        rel: rel.to_string(),
        text: String::from_utf8_lossy(slice).into_owned(),
        bytes,
        binary: false,
        truncated,
        rev,
    })
}

/// 写回。`expected` 是读的时候拿到的那份 `rev`，对不上就拒绝，让用户先看清外面改了什么。
pub fn write(body: &str, rel: &str, text: &str, expected: &FileRev) -> Result<FileRev, String> {
    let root = skill_root(body)?;
    let path = scoped(&root, rel)?;

    let current = revision(&path);
    if current != *expected {
        return Err(format!(
            "{rel} changed outside the editor — reload it before saving"
        ));
    }
    if current.exists && current.bytes > MAX_EDIT_BYTES {
        return Err(format!("{rel} is too large to edit in place"));
    }

    // `atomic_write_file` 自己再校验一次指纹（这里到落盘之间还有一小段），并且会拒绝
    // 目标是软链的情况 —— tmp + rename 会把那条软链替换成普通文件。
    let guard = util::file_revision(&path)?;
    util::atomic_write_file(&path, text.as_bytes(), &guard, rel)?;
    Ok(revision(&path))
}

/// 重新列一遍这个 skill 的文件。保存 / 新建之后刷新用，不必为一个文件重扫全机器。
pub fn list(body: &str) -> Result<(Vec<SkillFile>, bool), String> {
    let root = skill_root(body)?;
    Ok(super::skills::list_body_files(&root))
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFileList {
    pub files: Vec<SkillFile>,
    pub truncated: bool,
}

#[tauri::command(async)]
pub fn tools_list_skill_files(body: String) -> Result<SkillFileList, String> {
    let (files, truncated) = list(&body)?;
    Ok(SkillFileList { files, truncated })
}

#[tauri::command(async)]
pub fn tools_read_skill_file(body: String, rel: String) -> Result<SkillFileText, String> {
    read(&body, &rel)
}

#[tauri::command(async)]
pub fn tools_write_skill_file(
    body: String,
    rel: String,
    text: String,
    rev: FileRev,
) -> Result<FileRev, String> {
    write(&body, &rel, &text, &rev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "cc-sessions-viewer-skill-files-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            // 临时目录在 macOS 上是 `/var` → `/private/var` 的软链，作用域校验里到处
            // 都是 canonicalize，装置这边也得先解析，否则 `starts_with` 一路对不上。
            Self(root.canonicalize().unwrap())
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

    /// 一个最小的 skill：`SKILL.md` + `scripts/run.sh`。
    fn skill(root: &Path) -> String {
        let dir = root.join("doc-writer");
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: doc-writer\n---\nhi\n").unwrap();
        fs::write(dir.join("scripts").join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn reads_a_file_inside_the_skill() {
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        let got = read(&body, "scripts/run.sh").unwrap();
        assert_eq!(got.text, "#!/bin/sh\necho hi\n");
        assert!(!got.binary);
        assert!(!got.truncated);
        assert!(got.rev.exists);
    }

    #[test]
    fn writes_and_round_trips_the_revision() {
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        let before = read(&body, "SKILL.md").unwrap();
        let after = write(&body, "SKILL.md", "---\nname: x\n---\nbye\n", &before.rev).unwrap();
        assert_ne!(after, before.rev);
        assert_eq!(read(&body, "SKILL.md").unwrap().text, "---\nname: x\n---\nbye\n");
    }

    #[test]
    fn creates_a_file_that_does_not_exist_yet() {
        // 「新建文件」走的就是这条路：读不到就拿一个 `exists: false` 的 rev 去写。
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        let fresh = FileRev {
            exists: false,
            bytes: 0,
            modified_ms: None,
        };
        write(&body, "references/notes.md", "# notes\n", &fresh).unwrap();
        assert_eq!(read(&body, "references/notes.md").unwrap().text, "# notes\n");
    }

    #[test]
    fn refuses_to_overwrite_something_changed_outside() {
        // 用户在别的编辑器里改了同一个文件。盲写会把那边的改动吃掉。
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        let stale = read(&body, "SKILL.md").unwrap().rev;
        // 直接改磁盘，模拟外部编辑；顺手把 size 改掉，免得毫秒级 mtime 撞上。
        fs::write(
            Path::new(&body).join("SKILL.md"),
            "---\nname: someone-else\n---\nchanged outside\n",
        )
        .unwrap();
        let err = write(&body, "SKILL.md", "mine\n", &stale).unwrap_err();
        assert!(err.contains("changed outside"), "{err}");
    }

    #[test]
    fn a_parent_traversal_is_refused() {
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        fs::write(tmp.path().join("secret.txt"), "nope").unwrap();
        for rel in ["../secret.txt", "scripts/../../secret.txt", "a/../../b"] {
            assert!(read(&body, rel).is_err(), "{rel} must be refused");
            assert!(
                write(
                    &body,
                    rel,
                    "x",
                    &FileRev {
                        exists: false,
                        bytes: 0,
                        modified_ms: None
                    }
                )
                .is_err(),
                "{rel} must be refused"
            );
        }
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        let outside = tmp.path().join("secret.txt");
        fs::write(&outside, "nope").unwrap();
        assert!(read(&body, &outside.to_string_lossy()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_points_out_of_the_skill_is_refused() {
        // 这一条是 `..` 检查抓不到的：路径里一个 `..` 都没有，解析完却在外面。
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        let outside = tmp.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "nope").unwrap();
        std::os::unix::fs::symlink(&outside, Path::new(&body).join("refs")).unwrap();

        let err = read(&body, "refs/secret.txt").unwrap_err();
        assert!(err.contains("escapes"), "{err}");
        assert!(write(
            &body,
            "refs/new.txt",
            "x",
            &FileRev {
                exists: false,
                bytes: 0,
                modified_ms: None
            }
        )
        .is_err());
        // 外面那份一个字节都没动。
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "nope"
        );
    }

    #[test]
    fn a_directory_without_a_skill_md_is_not_a_valid_root() {
        // 根随便什么目录都能当的话，`rel` 那几道检查就只是在限制「从哪儿开始逃」。
        let tmp = Tmp::new();
        fs::create_dir_all(tmp.path().join("just-a-folder")).unwrap();
        let body = tmp.path().join("just-a-folder").to_string_lossy().to_string();
        assert!(read(&body, "anything.md").is_err());
        assert!(list(&body).is_err());
    }

    #[test]
    fn a_binary_file_comes_back_flagged_and_empty() {
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        fs::write(Path::new(&body).join("logo.png"), [0x89, b'P', 0x00, b'G']).unwrap();
        let got = read(&body, "logo.png").unwrap();
        assert!(got.binary);
        assert!(got.text.is_empty(), "binary content must not be handed to the editor");
    }

    #[test]
    fn listing_only_sees_the_skills_own_files() {
        let tmp = Tmp::new();
        let body = skill(tmp.path());
        fs::write(tmp.path().join("secret.txt"), "nope").unwrap();
        let (files, _) = list(&body).unwrap();
        let rels: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(rels.contains(&"SKILL.md"));
        assert!(rels.contains(&"scripts/run.sh"));
        assert!(!rels.iter().any(|r| r.contains("secret")));
    }
}
