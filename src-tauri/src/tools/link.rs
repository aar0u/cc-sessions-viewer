//! 跨平台目录链接：建立、检查、删除。
//!
//! Skills 管理的整个模型是「一份实体目录放在主 store，各 agent 的 skills 目录里放
//! 一条指向它的链接」。这在 Unix 上就是 symlink，一行搞定；Windows 上不是：
//!
//! - `symlink_dir` 需要 `SeCreateSymbolicLinkPrivilege`。Win10 1703 起开了开发者
//!   模式的普通用户也能建，但**没开开发者模式的普通用户建不了**，这是多数用户的状态。
//! - junction（目录联接）不需要特权，但它是另一种 reparse point：Rust 的
//!   `is_symlink()` 对 junction 返回 **false**，而 Node/libuv 会把它映射成 S_IFLNK。
//!   也就是说不同 agent 的运行时对「这是不是个链接」判断都不一致。
//! - git 在 Windows 上默认 `core.symlinks=false`，仓库里的 symlink 会被检出成**文本
//!   文件**——文件夹直接变成文件，agent 扫 skills 目录时根本看不到它。
//!
//! 所以这里做三级降级：symlink → junction → copy。
//!
//! **复制那一级刻意写成平台无关的**，只有 junction 的创建是 `#[cfg(windows)]`。
//! 降级复制是这个模块里最危险的一段代码（它会真的搬运和删除用户的文件），如果把它
//! 整段藏在 `#[cfg(windows)]` 后面，macOS 上的单测一行都覆盖不到，等到阶段 9 上
//! Windows 实测才第一次跑——那时候出问题的成本高得多。
//!
//! 几处东西**只在 Windows 上有调用方**（junction 那一路、以及降级复制本身），它们照常
//! 编译进所有平台，所以 macOS 上的单测能覆盖到 —— 代价是非 Windows 编译时它们看着是
//! 死代码。`allow(dead_code)` 就定点挂在这些项上，不再整个文件放行：上一版整块放行的
//! 结果是，副本的健康判定写完、测完、然后整整两个阶段没有任何调用方，编译器一声不吭
//! （验收 #8 就卡在这儿）。

use std::fs;
use std::path::{Path, PathBuf};

/// 链接实际落地成了什么。降级是静默发生的，但**必须被记下来**——UI 要显示，
/// 健康检查要据此决定是比对 target 还是比对内容指纹。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    /// 真符号链接（Unix 一律是这个；Windows 上需要特权或开发者模式）。
    Symlink,
    /// Windows 目录联接。不需要特权，但只能指向本地绝对路径，且不能跨卷。
    Junction,
    /// 硬链接。**只用于文件**（junction 是目录专用的，文件这边没有对应物）。
    /// 不要特权，同卷即可，而且它和 symlink 一样是「同一份物理内容」——正是合并要的效果。
    HardLink,
    /// 兜底的实体复制。源改了不会自动同步，靠指纹比对检出。
    Copy,
}

/// 一条链接的健康状态。
///
/// 少任何一态都会把坏链报成好链——第 1.1 节那 22 个死链就是「只判断存不存在」的产物。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "detail")]
// 只有建链/修链之后的核对用得上，而那一路目前只在单测里走完整流程。
#[allow(dead_code)]
pub enum LinkHealth {
    /// 路径压根不存在。
    Missing,
    /// 是链接，且解析到了期望的目标。
    Valid,
    /// 是链接，但目标不存在（死链）。
    Broken,
    /// 是链接或受管副本，但指向/来自的不是我们期望的那个（被别的工具改过）。
    WrongTarget(PathBuf),
    /// copy 降级模式，副本与源一致。
    CopyInSync,
    /// copy 降级模式，**源**变了，副本是旧的 —— 需要把源同步下来。
    CopyStale,
    /// copy 降级模式，**副本**被就地改过 —— 不能直接从源重拷，那会吃掉用户的修改。
    CopyEdited,
    /// copy 降级模式，源和副本**都**变了。自动挑一边必然吃掉另一边。
    CopyDiverged,
    /// 是个普通实体目录，不是链接也不是我们建的副本。收编的对象就是它。
    NotALink,
}

/// copy 降级时写在副本目录里的标记文件名。
///
/// 没有它就无法区分「我们降级复制出来的副本」和「用户自己放在那儿的实体 skill」——
/// 后者是要收编的，前者不是。但**光看文件在不在是不够的**：删除是 `remove_dir_all`，
/// 一个普通目录里碰巧有个同名文件就会被整个删掉。所以一律走 [`read_copy_marker`]，
/// 解析 + 校验通过才算数。
const COPY_MARKER: &str = ".session-viewer-link.json";

/// 标记文件里认这个 kind，别的（或缺失）一律当作不是我们建的。
const MARKER_KIND: &str = "cc-sessions-viewer/skill-copy";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CopyMarker {
    /// 固定串，用来拒绝「碰巧同名」的文件。
    kind: String,
    /// 这份副本是从哪儿复制来的。
    original: String,
    /// 复制当时**源**的指纹。源变了 → `CopyStale`。
    source_fingerprint: String,
    /// 复制当时**副本**的指纹。副本变了 → `CopyEdited`。
    copy_fingerprint: String,
    /// 这份副本当初被放在哪儿。
    ///
    /// 挡的是「marker 被搬到别的目录」：用户把受管副本整个拷走一份、或者误把这个
    /// 文件复制进别的目录时，记录的位置和它现在所在的位置对不上，删除就会拒绝。
    /// 没有这条的话，一个目录里只要有个看着合法的 marker 就能授权 `remove_dir_all`。
    copy_path: String,
}

// ---------------------------------------------------------------------------
// 建链
// ---------------------------------------------------------------------------

/// 在 `link` 处建立一条指向 `original` 的**文件**链接，按 symlink → 硬链接降级。
///
/// 和 [`link_dir`] 差在最后一级：目录降级到实体复制还说得过去（副本有标记文件、
/// 有指纹核对），单个 Markdown 文件没地方放标记，复制出来的两份和合并前一模一样
/// —— 那是**假装做成了**。所以这里到硬链接为止，再不行就报错，让调用方把这条
/// 列进「做不了」而不是悄悄退化。
///
/// `link` 必须不存在；调用方负责先清掉旧的。
pub fn link_file(original: &Path, link: &Path) -> Result<LinkKind, String> {
    if !original.is_file() {
        return Err(format!("Link source is not a file: {}", original.display()));
    }
    if link.symlink_metadata().is_ok() {
        return Err(format!("Link target already exists: {}", link.display()));
    }
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create link parent directory: {e}"))?;
    }
    platform_link_file(original, link)
}

#[cfg(unix)]
fn platform_link_file(original: &Path, link: &Path) -> Result<LinkKind, String> {
    std::os::unix::fs::symlink(original, link)
        .map(|_| LinkKind::Symlink)
        .map_err(|e| format!("Failed to create symlink: {e}"))
}

#[cfg(windows)]
fn platform_link_file(original: &Path, link: &Path) -> Result<LinkKind, String> {
    // 一级：真 symlink。开发者模式或管理员才成功。
    if std::os::windows::fs::symlink_file(original, link).is_ok() {
        return Ok(LinkKind::Symlink);
    }
    // 二级：硬链接。同卷 NTFS 上不要任何特权，且指向同一份物理内容。
    fs::hard_link(original, link)
        .map(|_| LinkKind::HardLink)
        .map_err(|e| {
            format!(
                "Failed to link {}: symlink needs Developer Mode and the hard-link \
                 fallback failed ({e})",
                link.display()
            )
        })
}

/// 在 `link` 处建立一条指向 `original` 的目录链接，按 symlink → junction → copy 降级。
///
/// `link` 必须不存在；调用方负责先清掉旧的（用 [`remove_link`]）。
pub fn link_dir(original: &Path, link: &Path) -> Result<LinkKind, String> {
    if !original.is_dir() {
        return Err(format!(
            "Link source is not a directory: {}",
            original.display()
        ));
    }
    if link.symlink_metadata().is_ok() {
        return Err(format!("Link target already exists: {}", link.display()));
    }
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create link parent directory: {e}"))?;
    }
    platform_link_dir(original, link)
}

#[cfg(unix)]
fn platform_link_dir(original: &Path, link: &Path) -> Result<LinkKind, String> {
    std::os::unix::fs::symlink(original, link)
        .map(|_| LinkKind::Symlink)
        .map_err(|e| format!("Failed to create symlink: {e}"))
}

#[cfg(windows)]
fn platform_link_dir(original: &Path, link: &Path) -> Result<LinkKind, String> {
    // 一级：真 symlink。开发者模式或管理员才成功。
    if std::os::windows::fs::symlink_dir(original, link).is_ok() {
        return Ok(LinkKind::Symlink);
    }
    // 二级：junction。不要特权，但只吃本地绝对路径，所以先 canonicalize。
    if create_junction(original, link).is_ok() {
        return Ok(LinkKind::Junction);
    }
    // 三级：实体复制 + 标记文件。
    copy_with_marker(original, link)?;
    Ok(LinkKind::Copy)
}

/// 路径交给 `cmd.exe` 是否安全。
///
/// `mklink` 是 cmd 的内建命令，没法直接 spawn，只能走 `cmd /C`。问题在于 cmd 会对
/// 命令行**再解析一遍**，而 `std::process::Command` 的转义走的是 C 运行时那套规则，
/// 挡不住 cmd 的元字符。`&` `^` `|` `<` `>` `(` `)` `%` `"` 在 Windows 文件名里全是
/// 合法字符 —— 一个叫 `foo & bar` 的 skill 目录就足以把命令行截断，最坏情况下让后半
/// 段以本应用的权限执行。
///
/// 这里**不做转义**：cmd 的引用规则角落太多，写对了也难以验证。检测到元字符就直接
/// 放弃 junction，让 [`platform_link_dir`] 落到复制降级——那条路径完全不经过 shell。
/// 代价只是这类路径下的 skill 在 Windows 上变成实体副本，而副本已经有完整的健康检查。
///
/// 编译进所有平台（而不是只在 Windows）是为了能在 macOS 上单测它。
#[cfg_attr(not(windows), allow(dead_code))]
fn is_cmd_safe(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false; // 非 UTF-8 路径，无法确认怎么被 cmd 看待。
    };
    !text.chars().any(|c| {
        matches!(
            c,
            '&' | '|' | '<' | '>' | '^' | '"' | '(' | ')' | '%' | '!' | '\r' | '\n' | '\0'
        )
    })
}

/// 用 `cmd /C mklink /J` 建 junction。
///
/// 走 `cmd` 而不是直接调 `DeviceIoControl` 写 reparse point：后者要手工拼
/// `REPARSE_DATA_BUFFER`，为了省一次进程启动引入一段 unsafe，不划算。代价是要自己
/// 守住 shell 元字符那道口子，见 [`is_cmd_safe`]。
#[cfg(windows)]
fn create_junction(original: &Path, link: &Path) -> Result<(), String> {
    // junction 只认本地绝对路径，相对路径会建出一条指向错误位置的链。
    let original = fs::canonicalize(original)
        .map_err(|e| format!("Failed to resolve junction source: {e}"))?;
    // canonicalize 会带上 \\?\ 前缀，mklink 不认，去掉。
    let original = original.to_string_lossy().replace(r"\\?\", "");
    if !is_cmd_safe(Path::new(&original)) || !is_cmd_safe(link) {
        return Err(format!(
            "Refusing to build a junction for {}: the path contains characters that cmd.exe \
             would reinterpret. Falling back to a copy.",
            link.display()
        ));
    }
    let status = crate::util::silent_command("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(&original)
        .status()
        .map_err(|e| format!("Failed to run mklink: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("mklink /J failed with status {status}"))
    }
}

/// 实体复制并留下标记文件。**先复制到临时目录，全部成功后再整体改名到位。**
///
/// 直接往 `link` 里边复制边写的话，中途失败（磁盘满、权限、嵌套链接）会留下一个
/// 半成品目录：agent 会读到残缺的 skill，而重试又会被 `link_dir` 的「目标已存在」
/// 挡住，用户只能手工去删。改成 tmp + rename 之后，失败路径上 `link` 始终不存在。
pub fn copy_with_marker(original: &Path, link: &Path) -> Result<(), String> {
    let parent = link
        .parent()
        .ok_or_else(|| format!("Link path has no parent: {}", link.display()))?;

    // 源目录里已经有个同名文件的话，复制过去之后会被我们的 marker 覆盖掉，而且
    // 覆盖得悄无声息。概率很低，但代价是用户丢一个文件，所以直接拒绝。
    if original.join(COPY_MARKER).symlink_metadata().is_ok() {
        return Err(format!(
            "Cannot copy {}: it already contains a file named {COPY_MARKER}, \
             which is the name this app uses to mark managed copies.",
            original.display()
        ));
    }

    let stem = link.file_name().and_then(|n| n.to_str()).unwrap_or("skill");
    // 复制期间源被改了就重来。只试两轮：一直在变说明有别的东西正在写这个目录，
    // 与其反复复制不如把话说清楚让调用方去处理。
    for attempt in 0..2 {
        let temp = create_exclusive_temp_dir(parent, stem, "copy")?;
        let built = (|| -> Result<Option<CopyMarker>, String> {
            // 先记源的指纹，复制完再记一次。两次不一样说明副本是「半新半旧」的混合体
            // —— 而事后无论比对哪一个指纹都看不出来，健康检查会把它报成 CopyInSync。
            //
            // 挡不住的情况：指纹的 mtime 只到毫秒，源在同一毫秒内改完又改回去，两次
            // 取样看着一模一样。真实的编辑不会这样，不值得为它把指纹换成全文 hash
            // （skill 目录里可能有几 MB 的 assets，每次健康扫描都要重算）。
            let before = fingerprint(original)?;
            copy_tree_rejecting_links(original, &temp)?;
            let after = fingerprint(original)?;
            if before != after {
                return Ok(None);
            }
            Ok(Some(CopyMarker {
                kind: MARKER_KIND.to_string(),
                original: original.to_string_lossy().to_string(),
                source_fingerprint: after,
                // 复制会刷新 mtime，所以副本的指纹必须在副本上单独算，不能沿用源的。
                copy_fingerprint: fingerprint(&temp)?,
                // 记最终位置，不是临时目录 —— 临时目录马上就 rename 掉了。
                copy_path: link.to_string_lossy().to_string(),
            }))
        })();

        let marker = match built {
            Err(error) => {
                let _ = fs::remove_dir_all(&temp);
                return Err(error);
            }
            Ok(None) => {
                let _ = fs::remove_dir_all(&temp);
                if attempt == 0 {
                    continue;
                }
                return Err(format!(
                    "Cannot copy {}: it kept changing while being copied",
                    original.display()
                ));
            }
            Ok(Some(marker)) => marker,
        };

        let written = serde_json::to_vec_pretty(&marker)
            .map_err(|e| format!("Failed to encode copy marker: {e}"))
            .and_then(|bytes| {
                fs::write(temp.join(COPY_MARKER), bytes)
                    .map_err(|e| format!("Failed to write copy marker: {e}"))
            })
            .and_then(|_| {
                fs::rename(&temp, link).map_err(|e| format!("Failed to install copied link: {e}"))
            });
        if let Err(error) = written {
            let _ = fs::remove_dir_all(&temp);
            return Err(error);
        }
        return Ok(());
    }
    unreachable!("the loop either returns or continues exactly once")
}

/// 建一个**由本次调用独占创建**的临时目录。
///
/// 必须用 `create_dir` 而不是 `create_dir_all`：后者在路径已经存在时直接返回成功，
/// 而且会跟随 symlink。临时目录名只由「链接名 + pid + 毫秒」组成，同一用户下的另一个
/// 进程完全可以预测出来并抢先占位（放一个指向别处的 symlink），于是复制会写进它指定
/// 的地方，失败清理时的 `remove_dir_all` 也会删到那儿去。
///
/// `create_dir` 在路径已存在时报 `AlreadyExists`，抢占就只是一次失败的重试，不会变成
/// 一次越界写入。名字里再加一个递增计数，保证重试不会撞回同一个名字。
fn create_exclusive_temp_dir(parent: &Path, stem: &str, tag: &str) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);

    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    for _ in 0..8 {
        let candidate = parent.join(format!(
            ".{stem}.{tag}-{}-{}-{}.tmp",
            std::process::id(),
            crate::util::now_millis(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Failed to create a temporary directory: {e}")),
        }
    }
    Err(format!(
        "Failed to create a temporary directory under {}: the name kept being taken",
        parent.display()
    ))
}

/// 递归复制目录，**遇到任何嵌套链接就整体失败**。
///
/// 不能用 `path.is_dir()` 判断要不要递归：它会跟随 symlink，于是
/// 一条指向 `~` 的目录链接会让复制走出源目录、甚至撞上环。而「悄悄把链接展开成
/// 实体内容」也不对——用户看到的副本会和源不一样。
///
/// skill 目录里出现嵌套链接本来就罕见，所以这里选择**明确报错**而不是猜：错误信息
/// 带上具体路径，UI 可以直接告诉用户哪个文件挡住了。
pub(super) fn copy_tree_rejecting_links(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("Failed to create {}: {e}", to.display()))?;
    for entry in
        fs::read_dir(from).map_err(|e| format!("Failed to read {}: {e}", from.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let src = entry.path();
        // symlink_metadata 不跟随链接 —— 这是和 `is_dir()` 的关键区别。
        let meta = src
            .symlink_metadata()
            .map_err(|e| format!("Failed to stat {}: {e}", src.display()))?;
        if meta.file_type().is_symlink() || is_link(&src) {
            return Err(format!(
                "Cannot copy {}: it contains a nested link ({}). \
                 Resolve or remove the nested link first.",
                from.display(),
                src.display()
            ));
        }
        let dst = to.join(entry.file_name());
        if meta.is_dir() {
            copy_tree_rejecting_links(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| format!("Failed to copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 检查
// ---------------------------------------------------------------------------

/// 这个路径是不是一条链接。
///
/// **不能只用 `is_symlink()`**：Windows 上 junction 的 `is_symlink()` 返回 false，
/// 只能去看 `FILE_ATTRIBUTE_REPARSE_POINT` 属性位。漏了这条，所有 junction 都会被
/// 当成实体目录，删除时会 `remove_dir_all` 掉**源目录的内容**。
pub fn is_link(path: &Path) -> bool {
    let Ok(meta) = path.symlink_metadata() else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        return meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[allow(unreachable_code)]
    false
}

/// 读并**校验**副本标记。任何一项不对都返回 `None`，调用方一律当作「不是我们建的」。
///
/// 校验 `kind` 而不是只看文件在不在，是为了挡住「用户的普通目录里碰巧有个同名文件」
/// 这种情况——那条路径通向 `remove_dir_all`，猜错的代价是删掉用户的数据。
fn read_copy_marker(path: &Path) -> Option<CopyMarker> {
    let marker_path = path.join(COPY_MARKER);
    // 必须是普通文件：`fs::read` 会跟随 symlink，一条指向别处某个合法 marker 的链接
    // 就能让任意目录看起来「受管」，而那条路径通向 `remove_dir_all`。
    let meta = marker_path.symlink_metadata().ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return None;
    }
    let raw = fs::read(&marker_path).ok()?;
    let marker: CopyMarker = serde_json::from_slice(&raw).ok()?;
    if marker.kind != MARKER_KIND || marker.original.is_empty() {
        return None;
    }
    // 记录的位置必须就是它现在所在的位置。
    if !same_path(&PathBuf::from(&marker.copy_path), path) {
        return None;
    }
    Some(marker)
}

/// 受管副本记录的源路径。不是受管副本就返回 `None`。
///
/// 扫描要用它：副本在磁盘上是实打实的目录，不问一句就会被当成「实体内容」报出来，
/// 于是同一个 skill 凭空多出一份「重复」。
pub fn copy_original(path: &Path) -> Option<PathBuf> {
    read_copy_marker(path).map(|m| PathBuf::from(m.original))
}

/// 把一份受管副本按源重拷一遍。
///
/// **只处理「只有源变了」这一种**。副本被就地改过（`CopyChanged` / `Diverged`）时重拷
/// 会把用户在副本上的修改无声地抹掉，所以这儿直接拒绝，由 UI 说清两边各在哪儿、让人
/// 自己决定 —— 这和 Hooks 面板「删不动的提前禁掉」是同一条规矩：宁可不给按钮，也不给
/// 一个会吃掉数据的按钮。
///
/// 换装是「先在旁边建好，再两次 rename」：新副本先建在临时目录里，旧的挪开，新的就位，
/// 最后才删旧的。任何一步失败，`copy` 这个位置上要么是旧副本要么是新副本，不会是一个
/// 半拉子目录 —— agent 随时可能在读它。
pub fn resync_copy(copy: &Path) -> Result<CopyState, String> {
    let state = copy_state(copy)
        .ok_or_else(|| format!("{} is not a managed copy", copy.display()))?;
    match state.sync {
        CopySync::SourceChanged => {}
        CopySync::InSync => return Ok(state),
        CopySync::CopyChanged | CopySync::Diverged => {
            return Err(format!(
                "Refusing to re-copy {}: it has local edits that a re-copy would discard.",
                copy.display()
            ))
        }
        CopySync::SourceMissing => {
            return Err(format!(
                "Cannot re-copy {}: its source {} is gone.",
                copy.display(),
                state.original.display()
            ))
        }
    }

    let parent = copy
        .parent()
        .ok_or_else(|| format!("Copy path has no parent: {}", copy.display()))?;
    let stem = copy.file_name().and_then(|n| n.to_str()).unwrap_or("skill");
    let staged = create_exclusive_temp_dir(parent, stem, "resync")?;
    // `copy_with_marker` 要求目标不存在，所以先建到一个不存在的名字上，再换过去。
    let fresh = staged.join("new");
    if let Err(e) = copy_with_marker(&state.original, &fresh) {
        let _ = fs::remove_dir_all(&staged);
        return Err(e);
    }
    // marker 里记的是「这份副本当初被放在哪儿」，而它最终要落在 `copy` 上，不是暂存位置。
    if let Err(e) = rewrite_copy_path(&fresh, copy) {
        let _ = fs::remove_dir_all(&staged);
        return Err(e);
    }

    let old = staged.join("old");
    if let Err(e) = fs::rename(copy, &old) {
        let _ = fs::remove_dir_all(&staged);
        return Err(format!("Failed to move the old copy aside: {e}"));
    }
    if let Err(e) = fs::rename(&fresh, copy) {
        // 新的没能就位 —— 把旧的放回去，别留下一个空位置。
        let _ = fs::rename(&old, copy);
        let _ = fs::remove_dir_all(&staged);
        return Err(format!("Failed to install the re-copied directory: {e}"));
    }
    let _ = fs::remove_dir_all(&staged);
    copy_state(copy).ok_or_else(|| format!("{} lost its marker while re-copying", copy.display()))
}

/// 把 marker 里的 `copy_path` 改写成它最终要落的位置，并按新位置重算副本指纹。
fn rewrite_copy_path(built: &Path, final_path: &Path) -> Result<(), String> {
    let marker_path = built.join(COPY_MARKER);
    let raw = fs::read(&marker_path).map_err(|e| format!("Failed to read the copy marker: {e}"))?;
    let mut marker: CopyMarker = serde_json::from_slice(&raw)
        .map_err(|e| format!("Failed to parse the copy marker: {e}"))?;
    marker.copy_path = final_path.to_string_lossy().to_string();
    let bytes = serde_json::to_vec_pretty(&marker)
        .map_err(|e| format!("Failed to encode the copy marker: {e}"))?;
    fs::write(&marker_path, bytes).map_err(|e| format!("Failed to write the copy marker: {e}"))?;
    // marker 本身不算进指纹（`fingerprint` 跳过树根那一个），所以重写它不会让刚记下的
    // `copy_fingerprint` 失效。这一行只是把「记录位置」对上，不动内容。
    Ok(())
}

/// 读一条链接指向哪里。junction 和 symlink 都能读到。
pub fn read_link_target(path: &Path) -> Option<PathBuf> {
    fs::read_link(path).ok()
}

/// 解析一条链接最终落到哪个真实路径，**沿着链一路跟到底**。
///
/// 本机实测 22 条链接里有 14 条是两跳的（`~/.claude/skills/x` → `~/.agents/skills/x`
/// → 主 store）。只跟一跳的话，「第一跳存在但第二跳断了」会被报成健康。
///
/// `max_hops` 防环——链接成环时 `fs::read_link` 会一直有返回值。
pub fn resolve_chain(path: &Path, max_hops: usize) -> (Vec<PathBuf>, Option<PathBuf>) {
    let mut hops = Vec::new();
    let mut current = path.to_path_buf();
    for _ in 0..max_hops {
        if !is_link(&current) {
            return (hops, current.exists().then_some(current));
        }
        let Some(target) = read_link_target(&current) else {
            return (hops, None);
        };
        // 相对链接要按链接自身所在目录解析，不是按 cwd。
        let target = if target.is_absolute() {
            target
        } else {
            current.parent().map(|p| p.join(&target)).unwrap_or(target)
        };
        hops.push(target.clone());
        current = target;
    }
    (hops, None)
}

/// 判断一条链接的健康状态。`expected` 是它**应该**指向的主 store 路径。
///
/// 和扫描用的 [`copy_state`] 分工不同：那个的期望值写在副本自己的 marker 里，扫描时
/// 就能算；这个要调用方先知道「应该指向哪」。
#[allow(dead_code)]
pub fn check_link(path: &Path, expected: &Path) -> Result<LinkHealth, String> {
    if path.symlink_metadata().is_err() {
        return Ok(LinkHealth::Missing);
    }
    if is_link(path) {
        let (_, resolved) = resolve_chain(path, 16);
        return Ok(match resolved {
            None => LinkHealth::Broken,
            Some(actual) if same_path(&actual, expected) => LinkHealth::Valid,
            Some(actual) => LinkHealth::WrongTarget(actual),
        });
    }
    if let Some(marker) = read_copy_marker(path) {
        return copy_health(path, expected, &marker);
    }
    Ok(LinkHealth::NotALink)
}

/// copy 降级模式的健康判定。先看来源对不对，再把同步关系翻成 `LinkHealth`。
#[allow(dead_code)]
fn copy_health(copy: &Path, expected: &Path, marker: &CopyMarker) -> Result<LinkHealth, String> {
    // 来源对不对。内容一致但来自别处的副本，不能当成指向 expected 的健康链接。
    let recorded = PathBuf::from(&marker.original);
    if !same_path(&recorded, expected) {
        return Ok(LinkHealth::WrongTarget(recorded));
    }
    Ok(match compare_copy(copy, &recorded, marker)? {
        CopySync::InSync => LinkHealth::CopyInSync,
        CopySync::SourceChanged => LinkHealth::CopyStale,
        CopySync::CopyChanged => LinkHealth::CopyEdited,
        CopySync::Diverged => LinkHealth::CopyDiverged,
        CopySync::SourceMissing => LinkHealth::Broken,
    })
}

/// 一份受管副本和它的源现在的关系。
///
/// **两边各自会不会变是独立的**，所以分别比、再组合，不能「先判副本、再判源」然后返回
/// 先命中的那个：那样两边都改过时会报成「副本被改过」，而照那个状态去处理就把另一边的
/// 改动吃掉了 —— 恰恰是最该拦住的一种。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CopySync {
    /// 两边都没动过。
    InSync,
    /// 只有源变了 —— 可以直接从源重拷下来。
    SourceChanged,
    /// 只有副本变了。重拷会吃掉这些修改，所以这一态**不给一键**。
    CopyChanged,
    /// 两边都变了。自动挑一边必然吃掉另一边，同样不给一键。
    Diverged,
    /// 源目录没了。没有东西可比，也没有东西可同步。
    SourceMissing,
}

/// 扫描时能算出来的副本状态 —— 期望值写在副本自己的 marker 里，不需要外部告诉它
/// 「应该指向哪」。这正是 [`check_link`] 在扫描阶段用不上的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyState {
    /// marker 里记的源。
    pub original: PathBuf,
    pub sync: CopySync,
}

/// 这个路径是不是受管副本，是的话它和源现在什么关系。
///
/// 比 [`copy_original`] 贵：要把副本和源各遍历一遍算指纹。扫描里一条条目只问一次，
/// 而 `copy_original` 在收编/删除的循环里每个候选都要问一次，所以两个都留着。
pub fn copy_state(path: &Path) -> Option<CopyState> {
    let marker = read_copy_marker(path)?;
    let original = PathBuf::from(&marker.original);
    // 算不出指纹（权限没了、目录被换成文件）时报 `SourceMissing`，而不是把整条判定丢掉：
    // 返回 `None` 会让调用方把这份副本当成实体目录，于是它变成一个「重复的 skill」。
    let sync = compare_copy(path, &original, &marker).unwrap_or(CopySync::SourceMissing);
    Some(CopyState { original, sync })
}

fn compare_copy(copy: &Path, source: &Path, marker: &CopyMarker) -> Result<CopySync, String> {
    let copy_changed = fingerprint(copy)? != marker.copy_fingerprint;
    if !source.is_dir() {
        return Ok(CopySync::SourceMissing);
    }
    let source_changed = fingerprint(source)? != marker.source_fingerprint;
    Ok(match (source_changed, copy_changed) {
        (false, false) => CopySync::InSync,
        (true, false) => CopySync::SourceChanged,
        (false, true) => CopySync::CopyChanged,
        (true, true) => CopySync::Diverged,
    })
}

/// 目录内容指纹：相对路径 + 大小 + mtime 毫秒，排序后拼接再 hash。
///
/// 不读文件内容——skill 目录里可能有几 MB 的 assets，每次健康扫描都全文 hash 太慢。
/// 大小 + mtime 足够发现「被改过」，这是它唯一的用途。
///
/// **只**跳过树根那一个标记文件：它是复制完成之后才写进去的，算进来的话副本的指纹
/// 永远对不上记录值。嵌套目录里碰巧同名的用户文件必须照常计入，否则改动它不会被
/// 任何一次健康检查发现。
pub fn fingerprint(dir: &Path) -> Result<String, String> {
    let mut rows: Vec<String> = Vec::new();
    collect_fingerprint_rows(dir, dir, &mut rows)?;
    rows.sort();
    Ok(format!("{:016x}", fnv1a64(rows.join("\n").as_bytes())))
}

fn collect_fingerprint_rows(root: &Path, dir: &Path, rows: &mut Vec<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("Failed to read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let path = entry.path();
        if path == root.join(COPY_MARKER) {
            continue;
        }
        // 和复制一样用 symlink_metadata：跟随链接会把指纹算到源目录外面去。
        let meta = path
            .symlink_metadata()
            .map_err(|e| format!("Failed to stat {}: {e}", path.display()))?;
        if meta.is_dir() {
            collect_fingerprint_rows(root, &path, rows)?;
        } else {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            rows.push(format!(
                "{}|{}|{}",
                rel.to_string_lossy().replace('\\', "/"),
                meta.len(),
                crate::util::mtime_millis(&path)
            ));
        }
    }
    Ok(())
}

pub(super) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 两个路径是不是同一个。先试 canonicalize（能穿透 symlink 和大小写差异），
/// 失败（目标不存在）再退到字面比较。
pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

// ---------------------------------------------------------------------------
// 删除
// ---------------------------------------------------------------------------

/// 为什么要删这条链接。删除是这个模块唯一会毁掉东西的操作，所以**意图必须显式**，
/// 不能用一个 `Option<&Path>` 含糊过去。
#[derive(Debug, Clone, Copy)]
pub enum RemovalIntent<'a> {
    /// 只删指向 `expected` 的链接或副本。指向别处的一律拒绝 —— 用户自己建的、指向
    /// 他自己某个目录的 symlink，不该因为「刚好占着这个位置」就被清掉。
    PointingAt(&'a Path),
    /// 只删死链（解析不到任何东西的）。健康修复里「清理死链」走这条：死链的目标已经
    /// 没了，没法用 `PointingAt` 校验，但删掉它不会失去任何东西。
    BrokenOnly,
}

/// 删除一条链接，**不碰它指向的内容**。
///
/// 三种东西三种删法，用错就是删掉用户的数据：
/// - Unix 目录 symlink：`remove_file`。
/// - Windows 目录 symlink / junction：`remove_dir`（不是 `remove_file`，也**绝不是**
///   `remove_dir_all`——后者会顺着 reparse point 把源目录里的东西删干净）。
/// - copy 降级出来的副本：它是实打实的目录，只能 `remove_dir_all`。
///
/// 最后一种唯一通向 `remove_dir_all`，所以门槛最高：marker 要是普通文件（不能是
/// symlink）、`kind` 要对、记录的位置要就是当前位置、记录的源要和意图一致，全过才删；
/// 而且删之前先原子搬走再复查一遍，见下面的注释。
///
/// **残留风险（真链接）**：校验和删除之间有个窗口，同一用户下的另一个进程可以在这中间
/// 把链接换成指向别处的链接。副本那一档用「先原子搬走再复查」堵掉了它，真链接这档没堵：
/// 副本走的是 `remove_dir_all`，被骗一次就是递归删一棵树；真链接无论怎么被换，
/// `remove_dir` / `remove_file` 拿掉的都只是一个目录项，源内容一个字节都不会少。代价
/// 差着数量级，不值得为它把每次删除都变成一次搬移。
///
/// **残留风险（副本）**：受管副本的凭据终究是目录里那个 JSON，有人能往用户 home 里写就能伪造。
/// 彻底的做法是在应用私有目录里维护一份「我们建过哪些副本」的登记，但那份登记会和磁盘
/// 漂移（用户挪走目录之后登记就是错的），需要连同主 store 的配置一起设计 —— 放在阶段 3。
pub fn remove_link(path: &Path, intent: RemovalIntent<'_>) -> Result<(), String> {
    let path = crate::util::normalize_windows_path(path);
    if is_link(&path) {
        match intent {
            RemovalIntent::BrokenOnly => {
                let (_, resolved) = resolve_chain(&path, 16);
                if resolved.is_some() {
                    return Err(format!(
                        "Refusing to remove {}: it still resolves to something. \
                         Use PointingAt when you know what it should point to.",
                        path.display()
                    ));
                }
            }
            RemovalIntent::PointingAt(expected) => {
                // 解析到底再比。只比第一跳的话，两跳链会被误判。
                let (_, resolved) = resolve_chain(&path, 16);
                let matches = match resolved {
                    Some(actual) => same_path(&actual, expected),
                    // 死链没法解析，退一步比字面目标：删除流程是「先解链再删源」，
                    // 所以正常情况下源还在，走不到这儿。
                    None => read_link_target(&path).is_some_and(|t| same_path(&t, expected)),
                };
                if !matches {
                    return Err(format!(
                        "Refusing to remove {}: it does not point at {}",
                        path.display(),
                        expected.display()
                    ));
                }
            }
        }
        #[cfg(windows)]
        {
            // 按**实际类型**选删法，不能「先 remove_dir 失败再 remove_file」：那个
            // 兜底等于说「只要 remove_dir 不行就当文件删」，有人在上面校验之后把一个
            // 普通文件换到这个位置，就会被直接删掉。查一次类型，只删对得上的那种。
            // Windows 的 junction 在 `symlink_metadata` 上可能既不是 file 也不是
            // directory，但技能链接只创建目录 reparse point；统一走目录删除，避免
            // 把 junction 误判成文件链接。
            return remove_windows_directory_link(&path);
        }
        #[cfg(unix)]
        {
            return fs::remove_file(&path).map_err(|e| format!("Failed to remove link: {e}"));
        }
    }

    let RemovalIntent::PointingAt(expected) = intent else {
        return Err(format!(
            "Refusing to remove {}: it is not a link, so there is no broken link to clean up",
            path.display()
        ));
    };
    let Some(marker) = read_copy_marker(&path) else {
        return Err(format!(
            "Refusing to remove {}: it is not a link or a managed copy",
            path.display()
        ));
    };
    if !same_path(&PathBuf::from(&marker.original), expected) {
        return Err(format!(
            "Refusing to remove {}: it is a copy of {}, not of {}",
            path.display(),
            marker.original,
            expected.display()
        ));
    }

    // 校验和 `remove_dir_all` 之间有个时间窗：别的进程（agent、编辑器、另一个我们
    // 自己的窗口）可能正好在这中间把这条路径换成一个普通目录，而我们手里那份已经
    // 读过的 marker 还是「通过」的，于是递归删掉的是用户的新数据。
    //
    // 所以先**原子地搬走**再删：rename 之后重新从搬到的位置读一遍 marker，确认搬走的
    // 确实是那份受管副本，才真正删除；对不上就原样搬回去并报错。这样 `remove_dir_all`
    // 作用的永远是一个我们刚刚亲自校验过、且别人已经拿不到路径的目录。
    let parent = path
        .parent()
        .ok_or_else(|| format!("Managed copy has no parent: {}", path.display()))?;
    let staged = parent.join(format!(
        ".{}.removing-{}-{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("skill"),
        std::process::id(),
        crate::util::now_millis()
    ));
    fs::rename(&path, &staged)
        .map_err(|e| format!("Failed to stage {} for removal: {e}", path.display()))?;

    // 搬走之后 marker 记的位置就对不上了，所以这里直接读原始内容比对，而不是再走
    // `read_copy_marker`（它会因为位置不符而拒绝）。
    let staged_ok = fs::read(staged.join(COPY_MARKER))
        .ok()
        .and_then(|raw| serde_json::from_slice::<CopyMarker>(&raw).ok())
        .is_some_and(|m| {
            m.kind == MARKER_KIND
                && same_path(&PathBuf::from(&m.original), expected)
                && same_path(&PathBuf::from(&m.copy_path), &path)
        });
    if staged_ok {
        fs::remove_dir_all(&staged).map_err(|e| format!("Failed to remove copied link: {e}"))
    } else {
        // 搬走的不是我们以为的东西 —— 原样放回去，一个字节都不动。
        let _ = fs::rename(&staged, &path);
        Err(format!(
            "Refusing to remove {}: it changed while we were verifying it",
            path.display()
        ))
    }
}

#[cfg(windows)]
fn remove_windows_directory_link(path: &Path) -> Result<(), String> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Some Windows reparse points are rejected by RemoveDirectory even
            // though they are safe to remove as links. Ask fsutil to remove
            // only the reparse metadata, then remove the now-empty directory
            // entry. The path was already checked as a link immediately before
            // this helper, and no recursive delete is involved.
            let status = crate::util::silent_command("fsutil")
                .args(["reparsepoint", "delete"])
                .arg(path)
                .status();
            if let Ok(status) = status {
                if status.success() {
                    return fs::remove_dir(path).map_err(|second| {
                        format!("Failed to remove link after clearing reparse point: {second}")
                    });
                }
                return Err(format!(
                    "Failed to remove link: {first} (fsutil exited with {status})"
                ));
            }
            Err(format!("Failed to remove link: {first} (fsutil could not run)"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write as _;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "csv-link-{}-{}-{}",
            tag,
            std::process::id(),
            crate::util::now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn skill_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        File::create(dir.join("SKILL.md")).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    // ---- 建链 / 解析 ----

    #[test]
    fn link_dir_creates_a_link_that_resolves_to_the_source() {
        let root = temp_root("create");
        let source = skill_dir(&root, "source");
        let link = root.join("link");

        let kind = link_dir(&source, &link).unwrap();
        assert!(matches!(kind, LinkKind::Symlink | LinkKind::Junction));
        assert!(is_link(&link), "a freshly created link must read as a link");
        assert_eq!(check_link(&link, &source).unwrap(), LinkHealth::Valid);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn link_dir_refuses_to_clobber_an_existing_path() {
        let root = temp_root("clobber");
        let source = skill_dir(&root, "source");
        let link = skill_dir(&root, "occupied");

        assert!(link_dir(&source, &link).is_err());
        assert!(
            link.join("SKILL.md").is_file(),
            "the existing directory must survive"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_whose_target_disappeared_reads_as_broken() {
        let root = temp_root("broken");
        let source = skill_dir(&root, "source");
        let link = root.join("link");
        link_dir(&source, &link).unwrap();

        fs::remove_dir_all(&source).unwrap();

        assert!(is_link(&link), "a dead link is still a link");
        assert_eq!(check_link(&link, &source).unwrap(), LinkHealth::Broken);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_pointing_somewhere_else_is_not_reported_as_valid() {
        let root = temp_root("wrong");
        let expected = skill_dir(&root, "expected");
        let other = skill_dir(&root, "other");
        let link = root.join("link");
        link_dir(&other, &link).unwrap();

        match check_link(&link, &expected).unwrap() {
            LinkHealth::WrongTarget(actual) => assert!(same_path(&actual, &other)),
            state => panic!("expected WrongTarget, got {state:?}"),
        }

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_two_hop_chain_resolves_to_the_real_source() {
        // 本机 22 条链接里 14 条是这个形状：agent 目录 → 中转目录 → 主 store。
        let root = temp_root("twohop");
        let source = skill_dir(&root, "source");
        let middle = root.join("middle");
        let front = root.join("front");
        link_dir(&source, &middle).unwrap();
        link_dir(&middle, &front).unwrap();

        let (hops, resolved) = resolve_chain(&front, 16);
        assert_eq!(hops.len(), 2, "both hops must be recorded for the UI");
        assert!(same_path(&resolved.unwrap(), &source));
        assert_eq!(check_link(&front, &source).unwrap(), LinkHealth::Valid);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_two_hop_chain_broken_at_the_second_hop_is_not_healthy() {
        // 这条是第 1.1 节那 22 个死链的成因：只跟一跳就会把它报成健康。
        let root = temp_root("twohop-broken");
        let source = skill_dir(&root, "source");
        let middle = root.join("middle");
        let front = root.join("front");
        link_dir(&source, &middle).unwrap();
        link_dir(&middle, &front).unwrap();

        fs::remove_dir_all(&source).unwrap();

        assert!(is_link(&front), "the first hop still exists");
        assert_eq!(check_link(&front, &source).unwrap(), LinkHealth::Broken);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_chain_gives_up_instead_of_looping_forever() {
        let root = temp_root("cycle");
        let a = root.join("a");
        let b = root.join("b");
        // 先建一条指向尚不存在的 b 的链，再让 b 指回 a，成环。
        #[cfg(unix)]
        std::os::unix::fs::symlink(&b, &a).unwrap();
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_dir(&b, &a);
        if !is_link(&a) {
            fs::remove_dir_all(&root).ok();
            return; // Windows 无特权时建不出悬空链接，这条不适用。
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&a, &b).unwrap();
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_dir(&a, &b);

        let (hops, resolved) = resolve_chain(&a, 16);
        assert!(resolved.is_none(), "a cycle must not resolve to anything");
        assert_eq!(hops.len(), 16, "it must stop at max_hops, not spin");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_plain_directory_reads_as_not_a_link() {
        let root = temp_root("plain");
        let source = skill_dir(&root, "source");
        let plain = skill_dir(&root, "plain");

        assert!(!is_link(&plain));
        assert_eq!(check_link(&plain, &source).unwrap(), LinkHealth::NotALink);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_path_reads_as_missing() {
        let root = temp_root("missing");
        let source = skill_dir(&root, "source");

        assert_eq!(
            check_link(&root.join("nope"), &source).unwrap(),
            LinkHealth::Missing
        );

        fs::remove_dir_all(&root).ok();
    }

    // ---- 删除 ----

    #[test]
    fn remove_link_deletes_the_link_and_leaves_the_source_alone() {
        let root = temp_root("remove");
        let source = skill_dir(&root, "source");
        let link = root.join("link");
        link_dir(&source, &link).unwrap();

        remove_link(&link, RemovalIntent::PointingAt(&source)).unwrap();

        assert!(link.symlink_metadata().is_err(), "the link must be gone");
        assert!(
            source.join("SKILL.md").is_file(),
            "the source must survive — this is the whole point"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn remove_link_refuses_to_delete_a_real_directory() {
        let root = temp_root("remove-real");
        let real = skill_dir(&root, "real");

        assert!(remove_link(&real, RemovalIntent::PointingAt(&real)).is_err());
        assert!(real.join("SKILL.md").is_file());

        fs::remove_dir_all(&root).ok();
    }

    // ---- 降级复制：这一段在 macOS 上也要跑得到 ----

    #[test]
    fn copy_fallback_produces_a_managed_copy_that_reads_as_in_sync() {
        let root = temp_root("copy-basic");
        let source = skill_dir(&root, "source");
        write(&source.join("SKILL.md"), "hello");
        fs::create_dir_all(source.join("scripts")).unwrap();
        write(&source.join("scripts").join("run.sh"), "echo hi");
        let copy = root.join("copy");

        copy_with_marker(&source, &copy).unwrap();

        assert!(!is_link(&copy), "a copy is a real directory, not a link");
        assert!(copy_original(&copy).is_some());
        assert_eq!(
            fs::read_to_string(copy.join("scripts").join("run.sh")).unwrap(),
            "echo hi",
            "nested files must come across"
        );
        assert_eq!(check_link(&copy, &source).unwrap(), LinkHealth::CopyInSync);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_copy_whose_source_changed_reads_as_stale() {
        let root = temp_root("copy-stale");
        let source = skill_dir(&root, "source");
        write(&source.join("SKILL.md"), "v1");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();

        write(&source.join("SKILL.md"), "v2 is longer");

        assert_eq!(check_link(&copy, &source).unwrap(), LinkHealth::CopyStale);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_copy_edited_in_place_is_not_reported_as_stale() {
        // 只有副本变了。报成 CopyStale 的话，「立即同步」会把用户的修改抹掉。
        let root = temp_root("copy-edited");
        let source = skill_dir(&root, "source");
        write(&source.join("SKILL.md"), "v1");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();

        write(&copy.join("SKILL.md"), "edited by the user");

        assert_eq!(copy_state(&copy).unwrap().sync, CopySync::CopyChanged);
        assert_eq!(check_link(&copy, &source).unwrap(), LinkHealth::CopyEdited);

        fs::remove_dir_all(&root).ok();
    }

    /// 两边都变了要单独成一态。
    ///
    /// 旧实现是「先判副本、再判源」，返回先命中的那个 —— 于是这种情况报成 CopyEdited。
    /// 光看状态名没问题，但照它去处理（把副本回写到源）就把源上的改动吃掉了。两边各自
    /// 变没变是独立的两件事，必须分别比再组合。
    #[test]
    fn both_sides_changing_is_its_own_state() {
        let root = temp_root("copy-diverged");
        let source = skill_dir(&root, "source");
        write(&source.join("SKILL.md"), "v1");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();

        write(&copy.join("SKILL.md"), "edited by the user");
        write(&source.join("SKILL.md"), "source moved on too");

        assert_eq!(copy_state(&copy).unwrap().sync, CopySync::Diverged);
        assert_eq!(check_link(&copy, &source).unwrap(), LinkHealth::CopyDiverged);

        fs::remove_dir_all(&root).ok();
    }

    /// 源没了：不是「健康」，也不是「不是受管副本」。
    ///
    /// 返回 `None` 的话扫描会把这份副本当成实体目录，于是它凭空变成一个「重复的 skill」。
    #[test]
    fn a_copy_whose_source_is_gone_still_reports_as_a_copy() {
        let root = temp_root("copy-orphan");
        let source = skill_dir(&root, "source");
        write(&source.join("SKILL.md"), "v1");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();
        fs::remove_dir_all(&source).unwrap();

        let state = copy_state(&copy).unwrap();
        assert_eq!(state.sync, CopySync::SourceMissing);
        assert_eq!(state.original, source);

        fs::remove_dir_all(&root).ok();
    }

    /// 扫描期的判定不需要外部告诉它「应该指向哪」—— 期望值就写在副本自己的 marker 里。
    #[test]
    fn a_copy_state_is_computable_without_an_expected_target() {
        let root = temp_root("copy-state");
        let source = skill_dir(&root, "source");
        write(&source.join("SKILL.md"), "v1");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();

        assert_eq!(copy_state(&copy).unwrap().sync, CopySync::InSync);
        write(&source.join("SKILL.md"), "v2 is longer");
        assert_eq!(copy_state(&copy).unwrap().sync, CopySync::SourceChanged);

        // 普通目录不是受管副本。
        assert!(copy_state(&skill_dir(&root, "plain")).is_none());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn re_copying_brings_a_stale_copy_back_in_sync() {
        let root = temp_root("copy-resync");
        let source = skill_dir(&root, "source");
        write(&source.join("SKILL.md"), "v1");
        fs::create_dir_all(source.join("nested")).unwrap();
        write(&source.join("nested/deep.txt"), "a");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();

        write(&source.join("SKILL.md"), "v2 is longer");
        fs::remove_file(source.join("nested/deep.txt")).unwrap();
        write(&source.join("added.txt"), "new");
        assert_eq!(copy_state(&copy).unwrap().sync, CopySync::SourceChanged);

        let after = resync_copy(&copy).unwrap();
        assert_eq!(after.sync, CopySync::InSync);
        assert_eq!(after.original, source);
        // 内容真的换过去了：新增的进来了，删掉的没留下。
        assert_eq!(fs::read_to_string(copy.join("SKILL.md")).unwrap(), "v2 is longer");
        assert_eq!(fs::read_to_string(copy.join("added.txt")).unwrap(), "new");
        assert!(!copy.join("nested/deep.txt").exists());
        // marker 记的位置仍然是它自己，不是重拷时用的暂存目录。
        assert_eq!(copy_original(&copy).unwrap(), source);
        // 暂存目录清干净了。
        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        fs::remove_dir_all(&root).ok();
    }

    /// 重拷会把副本上的本地修改抹掉，所以有本地修改时**后端**就拒绝。
    ///
    /// 前端当然也会禁按钮，但那是第二道；靠 UI 记得禁用的规则迟早会被下一个入口绕过去。
    #[test]
    fn re_copying_refuses_to_discard_local_edits() {
        let root = temp_root("copy-resync-refuse");
        let source = skill_dir(&root, "source");
        write(&source.join("SKILL.md"), "v1");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();

        write(&copy.join("SKILL.md"), "edited by the user");
        let err = resync_copy(&copy).unwrap_err();
        assert!(err.contains("local edits"), "{err}");
        // 一个字都没动。
        assert_eq!(
            fs::read_to_string(copy.join("SKILL.md")).unwrap(),
            "edited by the user"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn re_copying_something_that_is_not_a_managed_copy_is_an_error() {
        let root = temp_root("copy-resync-plain");
        let plain = skill_dir(&root, "plain");
        write(&plain.join("SKILL.md"), "mine");
        let err = resync_copy(&plain).unwrap_err();
        assert!(err.contains("not a managed copy"), "{err}");
        assert!(plain.join("SKILL.md").exists());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_copy_of_something_else_is_not_reported_as_in_sync() {
        // 内容相同但来源不同，不能算作指向 expected 的健康链接。
        let root = temp_root("copy-wrong");
        let expected = skill_dir(&root, "expected");
        let other = skill_dir(&root, "other");
        let copy = root.join("copy");
        copy_with_marker(&other, &copy).unwrap();

        match check_link(&copy, &expected).unwrap() {
            LinkHealth::WrongTarget(actual) => assert!(same_path(&actual, &other)),
            state => panic!("expected WrongTarget, got {state:?}"),
        }

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn copying_a_tree_that_contains_a_nested_link_fails_and_leaves_nothing_behind() {
        let root = temp_root("copy-nested");
        let source = skill_dir(&root, "source");
        let outside = skill_dir(&root, "outside");
        let nested = source.join("refs");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &nested).unwrap();
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_dir(&outside, &nested);
        if !is_link(&nested) {
            fs::remove_dir_all(&root).ok();
            return;
        }
        let copy = root.join("copy");

        let error = copy_with_marker(&source, &copy).unwrap_err();
        assert!(error.contains("nested link"), "unexpected error: {error}");
        assert!(
            copy.symlink_metadata().is_err(),
            "a failed copy must not leave a half-built directory behind"
        );
        // 临时目录也要清干净，否则父目录里会攒垃圾。
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".copy-"))
            .collect();
        assert!(leftovers.is_empty(), "temp dir was not cleaned up");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_failed_copy_can_simply_be_retried() {
        // 半成品目录会把重试卡在「目标已存在」上，用户只能手工删。
        let root = temp_root("copy-retry");
        let source = skill_dir(&root, "source");
        let outside = skill_dir(&root, "outside");
        let nested = source.join("refs");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &nested).unwrap();
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_dir(&outside, &nested);
        if !is_link(&nested) {
            fs::remove_dir_all(&root).ok();
            return;
        }
        let copy = root.join("copy");
        assert!(copy_with_marker(&source, &copy).is_err());

        // 把挡路的嵌套链接去掉之后，同一个目标路径必须能直接重试成功。
        remove_link(&nested, RemovalIntent::PointingAt(&outside)).unwrap();
        copy_with_marker(&source, &copy).unwrap();
        assert_eq!(check_link(&copy, &source).unwrap(), LinkHealth::CopyInSync);

        fs::remove_dir_all(&root).ok();
    }

    // ---- 标记文件的信任边界 ----

    #[test]
    fn a_plain_directory_that_happens_to_contain_the_marker_name_is_not_managed() {
        // 这条守的是 remove_dir_all：只看文件在不在，就会整目录删掉用户的数据。
        let root = temp_root("marker-fake");
        let source = skill_dir(&root, "source");
        let impostor = skill_dir(&root, "impostor");
        write(&impostor.join(COPY_MARKER), "not even json");

        assert!(copy_original(&impostor).is_none());
        assert_eq!(
            check_link(&impostor, &source).unwrap(),
            LinkHealth::NotALink
        );
        assert!(remove_link(&impostor, RemovalIntent::PointingAt(&source)).is_err());
        assert!(
            impostor.join("SKILL.md").is_file(),
            "the impostor directory must survive"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_marker_with_the_wrong_kind_is_not_trusted() {
        let root = temp_root("marker-kind");
        let source = skill_dir(&root, "source");
        let impostor = skill_dir(&root, "impostor");
        // 字段名要和 CopyMarker 对得上 —— 否则这条测的是「JSON 解析失败」，
        // 而不是它声称要测的「kind 不对就不认」。
        write(
            &impostor.join(COPY_MARKER),
            &format!(
                r#"{{"kind":"something-else","original":{:?},"source_fingerprint":"a","copy_fingerprint":"b","copy_path":{:?}}}"#,
                source.to_string_lossy(),
                impostor.to_string_lossy()
            ),
        );
        assert!(
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(impostor.join(COPY_MARKER)).unwrap()
            )
            .is_ok(),
            "the fixture must be valid JSON, otherwise this test proves nothing"
        );

        assert!(copy_original(&impostor).is_none());
        assert!(remove_link(&impostor, RemovalIntent::PointingAt(&source)).is_err());
        assert!(impostor.join("SKILL.md").is_file());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn removing_a_managed_copy_requires_the_source_to_match() {
        let root = temp_root("remove-copy");
        let source = skill_dir(&root, "source");
        let other = skill_dir(&root, "other");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();

        // 意图是「只清死链」→ 拒绝，它不是死链。
        assert!(remove_link(&copy, RemovalIntent::BrokenOnly).is_err());
        // 给了错的源 → 拒绝。
        assert!(remove_link(&copy, RemovalIntent::PointingAt(&other)).is_err());
        assert!(
            copy.join("SKILL.md").is_file(),
            "the copy must still be there"
        );

        // 给对了才删，而且不能动到源。
        remove_link(&copy, RemovalIntent::PointingAt(&source)).unwrap();
        assert!(copy.symlink_metadata().is_err());
        assert!(source.join("SKILL.md").is_file());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cmd_metacharacters_in_a_path_disqualify_the_junction_path() {
        // Windows 文件名里 & ^ | ( ) 全是合法字符，而 cmd 会把它们当控制符再解析一遍。
        // 检测到就放弃 junction、落到复制——那条路不经过 shell。
        assert!(is_cmd_safe(Path::new(r"C:\Users\me\.claude\skills\pinme")));
        assert!(is_cmd_safe(Path::new("/Users/me/.claude/skills/pinme")));
        for bad in [
            r"C:\skills\foo & calc",
            r"C:\skills\foo | whoami",
            r"C:\skills\a^b",
            r"C:\skills\%PATH%",
            r"C:\skills\(x)",
            "C:\\skills\\a\nb",
        ] {
            assert!(!is_cmd_safe(Path::new(bad)), "{bad} must be rejected");
        }
    }

    #[test]
    fn copying_refuses_when_the_source_already_has_a_file_named_like_the_marker() {
        // 否则我们的 marker 会把用户那个同名文件悄无声息地盖掉。
        let root = temp_root("marker-collide");
        let source = skill_dir(&root, "source");
        write(&source.join(COPY_MARKER), "user's own file");
        let copy = root.join("copy");

        let error = copy_with_marker(&source, &copy).unwrap_err();
        assert!(error.contains(COPY_MARKER), "unexpected error: {error}");
        assert!(copy.symlink_metadata().is_err());
        assert_eq!(
            fs::read_to_string(source.join(COPY_MARKER)).unwrap(),
            "user's own file",
            "the user's file must be untouched"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_nested_file_named_like_the_marker_still_counts_towards_the_fingerprint() {
        // 只有树根那一个 marker 该被跳过。按文件名跳过的话，嵌套目录里同名文件
        // 怎么改都不会被任何一次健康检查发现。
        let root = temp_root("marker-nested");
        let dir = skill_dir(&root, "skill");
        fs::create_dir_all(dir.join("refs")).unwrap();
        write(&dir.join("refs").join(COPY_MARKER), "v1");

        let before = fingerprint(&dir).unwrap();
        write(&dir.join("refs").join(COPY_MARKER), "v2 is longer");
        assert_ne!(
            fingerprint(&dir).unwrap(),
            before,
            "a nested same-named file must not be invisible"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_copy_whose_nested_marker_named_file_changed_reads_as_edited() {
        let root = temp_root("marker-nested-copy");
        let source = skill_dir(&root, "source");
        fs::create_dir_all(source.join("refs")).unwrap();
        write(&source.join("refs").join(COPY_MARKER), "v1");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();
        assert_eq!(check_link(&copy, &source).unwrap(), LinkHealth::CopyInSync);

        write(&copy.join("refs").join(COPY_MARKER), "edited by the user");
        assert_eq!(check_link(&copy, &source).unwrap(), LinkHealth::CopyEdited);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn removal_re_verifies_after_staging_and_puts_things_back_when_they_do_not_match() {
        // 模拟校验后、删除前被掉包：搬走之后重新读 marker 对不上，必须原样放回。
        let root = temp_root("remove-toctou");
        let source = skill_dir(&root, "source");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();
        // 把 marker 换成一个指向别处的，模拟「搬走的不是我们以为的东西」。
        write(
            &copy.join(COPY_MARKER),
            &format!(
                r#"{{"kind":{MARKER_KIND:?},"original":"/somewhere/else","source_fingerprint":"a","copy_fingerprint":"b","copy_path":{:?}}}"#,
                copy.to_string_lossy()
            ),
        );

        assert!(remove_link(&copy, RemovalIntent::PointingAt(&source)).is_err());
        assert!(
            copy.join("SKILL.md").is_file(),
            "a failed removal must leave the directory exactly where it was"
        );
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".removing-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the staging directory was not put back"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_pointing_elsewhere_is_not_removed() {
        // 用户自己建的、指向他自己某个目录的 symlink，不该因为刚好占着这个位置就被清掉。
        let root = temp_root("remove-wrong-target");
        let expected = skill_dir(&root, "expected");
        let mine = skill_dir(&root, "mine");
        let link = root.join("link");
        link_dir(&mine, &link).unwrap();

        assert!(remove_link(&link, RemovalIntent::PointingAt(&expected)).is_err());
        assert!(is_link(&link), "the user's own link must survive");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn removing_a_two_hop_link_matches_on_the_final_target() {
        // 只比第一跳的话，两跳链会被误判成「指向别处」而删不掉。
        let root = temp_root("remove-twohop");
        let source = skill_dir(&root, "source");
        let middle = root.join("middle");
        let front = root.join("front");
        link_dir(&source, &middle).unwrap();
        link_dir(&middle, &front).unwrap();

        remove_link(&front, RemovalIntent::PointingAt(&source)).unwrap();
        assert!(front.symlink_metadata().is_err());
        assert!(
            middle.symlink_metadata().is_ok(),
            "the middle hop is a separate entry"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn broken_only_removes_dead_links_and_refuses_live_ones() {
        let root = temp_root("remove-broken");
        let source = skill_dir(&root, "source");
        let live = root.join("live");
        let dead = root.join("dead");
        link_dir(&source, &live).unwrap();
        link_dir(&source, &dead).unwrap();

        // 活链不能用「清死链」的意图删掉。
        assert!(remove_link(&live, RemovalIntent::BrokenOnly).is_err());
        assert!(is_link(&live));

        fs::remove_dir_all(&source).unwrap();
        remove_link(&dead, RemovalIntent::BrokenOnly).unwrap();
        assert!(dead.symlink_metadata().is_err());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_marker_pointing_at_a_different_location_is_not_trusted() {
        // 把受管副本整个拷走一份之后，那份拷贝的 marker 记的还是原位置 —— 不能凭它
        // 授权 remove_dir_all 删掉这个新目录。
        let root = temp_root("marker-moved");
        let source = skill_dir(&root, "source");
        let copy = root.join("copy");
        copy_with_marker(&source, &copy).unwrap();

        let elsewhere = root.join("elsewhere");
        copy_tree_rejecting_links(&copy, &elsewhere).unwrap();
        fs::copy(copy.join(COPY_MARKER), elsewhere.join(COPY_MARKER)).unwrap();

        assert!(
            copy_original(&elsewhere).is_none(),
            "a relocated marker must not vouch for its new home"
        );
        assert!(remove_link(&elsewhere, RemovalIntent::PointingAt(&source)).is_err());
        assert!(elsewhere.join("SKILL.md").is_file());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_marker_that_is_a_symlink_is_not_trusted() {
        // fs::read 会跟随 symlink：一条指向别处某个合法 marker 的链接就能让任意目录
        // 看起来「受管」。
        let root = temp_root("marker-symlink");
        let source = skill_dir(&root, "source");
        let real = root.join("real");
        copy_with_marker(&source, &real).unwrap();

        let impostor = skill_dir(&root, "impostor");
        #[cfg(unix)]
        std::os::unix::fs::symlink(real.join(COPY_MARKER), impostor.join(COPY_MARKER)).unwrap();
        #[cfg(windows)]
        let _ =
            std::os::windows::fs::symlink_file(real.join(COPY_MARKER), impostor.join(COPY_MARKER));
        if impostor.join(COPY_MARKER).symlink_metadata().is_err() {
            fs::remove_dir_all(&root).ok();
            return;
        }

        assert!(copy_original(&impostor).is_none());
        assert!(remove_link(&impostor, RemovalIntent::PointingAt(&source)).is_err());
        assert!(impostor.join("SKILL.md").is_file());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn temp_directories_are_created_by_us_and_never_handed_out_twice() {
        // 临时目录名如果可以被预测，同一用户下的另一个进程就能抢先在那儿放一个指向别处
        // 的 symlink；`create_dir_all` 会跟着它写出去，失败清理的 `remove_dir_all`
        // 也会删到那儿。改用独占创建之后，被抢占只会变成一次重试。
        let root = temp_root("exclusive");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let dir = create_exclusive_temp_dir(&root, "skill", "copy").unwrap();
            assert!(seen.insert(dir.clone()), "handed out {dir:?} twice");
            assert!(dir.is_dir());
            assert_eq!(
                fs::read_dir(&dir).unwrap().count(),
                0,
                "a temp dir must be freshly created, not adopted"
            );
        }
        // 同一毫秒里连开 50 个都没重名，说明名字不是只靠时间戳区分的。
        assert_eq!(seen.len(), 50);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fingerprint_changes_when_a_file_changes_and_ignores_the_marker() {
        let root = temp_root("fingerprint");
        let dir = skill_dir(&root, "skill");

        let before = fingerprint(&dir).unwrap();
        // 标记文件不参与指纹，否则副本的指纹永远对不上记录值。
        write(&dir.join(COPY_MARKER), "{}");
        assert_eq!(
            fingerprint(&dir).unwrap(),
            before,
            "the marker must not count"
        );

        write(&dir.join("SKILL.md"), "changed");
        assert_ne!(
            fingerprint(&dir).unwrap(),
            before,
            "a size change must show up"
        );

        fs::remove_dir_all(&root).ok();
    }
}
