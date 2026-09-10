//! 聊天里贴进来的图片的落盘目录。
//!
//! 这些图片原来直接写在 `$TMPDIR` 下（`clipboard-*.png`、
//! `cc-sessions-viewer-images/chat-img-*`），有两个问题：
//!
//!   1. **会被系统删掉**。macOS 会清理 `$TMPDIR` 里三天没动过的文件，而 transcript 里存的是
//!      这条路径 —— 三天后回头看那条消息，图片就没了。这是个一直存在的 bug。
//!   2. **永远不清理**。没被系统清掉的那些（比如 `$TMPDIR` 直接被设成别的位置）只增不减，
//!      而且散在临时目录里，用户根本不知道它们的存在。
//!
//! 现在统一写到 `<data_dir>/attachments/<YYYY-MM>/`：位置稳定、按月分目录方便人工翻看，
//! 容量由 [`crate::storage_gc`] 按「30 天 + 500 MB」两条上限治理。比系统的三天宽得多，
//! 且过期后界面上是已有的「图片不可用」占位，不会崩。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tauri::AppHandle;

use crate::storage_gc::{self, Policy};

/// 超过这个年龄的附件会被清掉。系统清 `$TMPDIR` 是 3 天，这里宽到 30 天。
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// 附件目录的总量上限。超了从最旧的开始删。
pub const MAX_BYTES: u64 = 500 * 1024 * 1024;

/// 旧版本留在 `$TMPDIR` 里的附件，超过这个年龄就顺手清掉。
const LEGACY_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub fn root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::app_storage::data_dir(app)?.join("attachments"))
}

/// 本月的附件目录，必要时创建。
fn month_dir(root: &Path, now: SystemTime) -> Result<PathBuf, String> {
    let month = chrono::DateTime::<chrono::Local>::from(now)
        .format("%Y-%m")
        .to_string();
    let dir = root.join(month);
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn save_in(root: &Path, bytes: &[u8], ext: &str, name_hint: &str, now: SystemTime) -> Result<PathBuf, String> {
    let dir = month_dir(root, now)?;
    let stamp = chrono::DateTime::<chrono::Local>::from(now).format("%d-%H%M%S%.3f");
    let path = dir.join(format!("{name_hint}-{stamp}.{ext}"));
    fs::write(&path, bytes).map_err(|error| format!("write failed: {error}"))?;
    Ok(path)
}

/// 写一张附件图片，返回绝对路径。`name_hint` 只影响文件名，方便人工辨认来源。
pub fn save(app: &AppHandle, bytes: &[u8], ext: &str, name_hint: &str) -> Result<PathBuf, String> {
    save_in(&root(app)?, bytes, ext, name_hint, SystemTime::now())
}

/// media type → 扩展名。认不出来的按 png 处理（贴图绝大多数是 png）。
pub fn extension_for(media_type: &str) -> &'static str {
    let media_type = media_type.to_ascii_lowercase();
    if media_type.contains("jpeg") || media_type.contains("jpg") {
        "jpg"
    } else if media_type.contains("gif") {
        "gif"
    } else if media_type.contains("webp") {
        "webp"
    } else if media_type.contains("bmp") {
        "bmp"
    } else {
        "png"
    }
}

pub fn policy() -> Policy {
    Policy {
        max_age: Some(MAX_AGE),
        max_bytes: Some(MAX_BYTES),
    }
}

/// 清理附件目录。启动时与每 24 小时各跑一次。
pub fn prune(app: &AppHandle) -> storage_gc::Pruned {
    let Ok(root) = root(app) else {
        return storage_gc::Pruned::default();
    };
    storage_gc::prune(&root, policy(), SystemTime::now())
}

/// 收拾旧版本散落在 `$TMPDIR` 的附件。
///
/// 只删本 app 自己写的那两种命名，且只删七天以上没动过的 —— 刚贴进输入框还没发出去的那张
/// 不能删。这个函数在下一个大版本之后可以整个删掉。
pub fn prune_legacy_temp(now: SystemTime) -> storage_gc::Pruned {
    let temp = std::env::temp_dir();
    let mut pruned = storage_gc::Pruned::default();

    if let Ok(entries) = fs::read_dir(&temp) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("clipboard-") {
                continue;
            }
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let stale = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > LEGACY_MAX_AGE);
            if stale && fs::remove_file(entry.path()).is_ok() {
                pruned.files += 1;
                pruned.bytes += metadata.len();
            }
        }
    }

    let images = temp.join("cc-sessions-viewer-images");
    let legacy = storage_gc::prune(
        &images,
        Policy {
            max_age: Some(LEGACY_MAX_AGE),
            max_bytes: None,
        },
        now,
    );
    pruned.files += legacy.files;
    pruned.bytes += legacy.bytes;
    let _ = fs::remove_dir(&images); // 空了就一并收掉

    pruned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("cc-attachments-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn saves_into_a_month_folder() {
        let root = scratch("save");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let month = chrono::DateTime::<chrono::Local>::from(now)
            .format("%Y-%m")
            .to_string();

        let path = save_in(&root, b"png-bytes", "png", "clipboard", now).unwrap();

        assert_eq!(path.parent().unwrap(), root.join(&month));
        assert_eq!(path.extension().unwrap(), "png");
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("clipboard-"));
        assert_eq!(fs::read(&path).unwrap(), b"png-bytes");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn two_saves_in_the_same_second_do_not_collide() {
        let root = scratch("collide");
        let now = SystemTime::now();
        let first = save_in(&root, b"a", "png", "chat-img", now).unwrap();
        let second = save_in(&root, b"bb", "png", "chat-img", now + Duration::from_millis(1)).unwrap();

        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"a");
        assert_eq!(fs::read(&second).unwrap(), b"bb");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extension_for_maps_the_media_types_the_composer_sends() {
        assert_eq!(extension_for("image/png"), "png");
        assert_eq!(extension_for("image/jpeg"), "jpg");
        assert_eq!(extension_for("image/JPG"), "jpg");
        assert_eq!(extension_for("image/gif"), "gif");
        assert_eq!(extension_for("image/webp"), "webp");
        assert_eq!(extension_for("application/octet-stream"), "png");
    }

    #[test]
    fn legacy_cleanup_only_touches_our_own_stale_files() {
        // `prune_legacy_temp` 扫的是真实的 `$TMPDIR`，这里就在那里造样本。
        let now = SystemTime::now();
        let temp = std::env::temp_dir();
        let tag = uuid::Uuid::new_v4();
        let ours = temp.join(format!("clipboard-{tag}.png"));
        let theirs = temp.join(format!("someone-else-{tag}.png"));
        fs::write(&ours, b"old").unwrap();
        fs::write(&theirs, b"old").unwrap();
        let stale = filetime::FileTime::from_system_time(now - Duration::from_secs(30 * 86_400));
        filetime::set_file_mtime(&ours, stale).unwrap();
        filetime::set_file_mtime(&theirs, stale).unwrap();

        let fresh = temp.join(format!("clipboard-{tag}-fresh.png"));
        fs::write(&fresh, b"new").unwrap();

        prune_legacy_temp(now);

        assert!(!ours.exists(), "过期的 clipboard-* 应被清掉");
        assert!(theirs.exists(), "别人的临时文件不能动");
        assert!(fresh.exists(), "刚写的贴图不能动");

        let _ = fs::remove_file(theirs);
        let _ = fs::remove_file(fresh);
    }
}
