//! 目录容量治理：按「最后修改时间 + 总字节」两条上限清理一个目录。
//!
//! 服务两个目录：`<data_dir>/attachments`（聊天里贴进来的图片）与
//! `<data_dir>/image-cache`（会话图片的内容寻址缓存）。两者的共同点是**可再生或可丢弃**：
//! 缓存丢了下次读会话时会重新写一份；附件丢了界面上显示「图片不可用」，跟今天 macOS 把
//! `$TMPDIR` 清掉之后的表现一样。所以这里可以放心删。
//!
//! **不要**把它用在回收站上 —— 那里放的是用户的原始数据，删除有自己的一套（保留期 +
//! 明确的确认），走 `trash::purge_expired`。
//!
//! 判据用 mtime 而不是 atime：很多文件系统默认 `noatime`，atime 根本不更新，拿它做 LRU
//! 会得到一个随机顺序。mtime 对这两个目录等价于「写进来的时间」，先进先出。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 一次清理的两条上限。任一条为 `None` 表示这条不限制。
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub max_age: Option<Duration>,
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub files: usize,
    pub bytes: u64,
}

struct Entry {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

fn collect(root: &Path, out: &mut Vec<Entry>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // 符号链接不跟进也不统计：跟进去可能走出这个目录，删到别人的东西。
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect(&entry.path(), out);
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        out.push(Entry {
            path: entry.path(),
            bytes: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
}

/// 目录里所有普通文件的字节数之和（不跟符号链接）。存储面板与清理都用它。
pub fn total_bytes(root: &Path) -> u64 {
    let mut entries = Vec::new();
    collect(root, &mut entries);
    entries.iter().map(|entry| entry.bytes).sum()
}

/// 删掉 `root` 下面已经空了的子目录（`root` 自身保留）。
fn remove_empty_dirs(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let path = entry.path();
            remove_empty_dirs(&path);
            let _ = fs::remove_dir(&path); // 非空会失败，正是想要的
        }
    }
}

/// 先按年龄删过期的，再按总量从最旧的开始删到限额以内。返回实际删掉的量。
pub fn prune(root: &Path, policy: Policy, now: SystemTime) -> Pruned {
    let mut entries = Vec::new();
    collect(root, &mut entries);
    // 最旧的排在前面 —— 两轮删除都从这一头开始。
    entries.sort_by_key(|entry| entry.modified);

    let mut pruned = Pruned::default();
    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries {
        let expired = policy.max_age.is_some_and(|max_age| {
            now.duration_since(entry.modified)
                .is_ok_and(|age| age > max_age)
        });
        if expired && fs::remove_file(&entry.path).is_ok() {
            pruned.files += 1;
            pruned.bytes += entry.bytes;
        } else {
            kept.push(entry);
        }
    }

    if let Some(max_bytes) = policy.max_bytes {
        let mut total: u64 = kept.iter().map(|entry| entry.bytes).sum();
        for entry in kept {
            if total <= max_bytes {
                break;
            }
            if fs::remove_file(&entry.path).is_ok() {
                total -= entry.bytes;
                pruned.files += 1;
                pruned.bytes += entry.bytes;
            }
        }
    }

    if pruned.files > 0 {
        remove_empty_dirs(root);
    }
    pruned
}

/// 磁盘治理的唯一调度点：启动跑一次，之后每 24 小时一次。
///
/// 三件事都在同一个后台线程里串行做完，谁也不阻塞 setup：
///   1. 附件目录按 30 天 / 500 MB 收口；
///   2. 图片缓存按 500 MB 收口；
///   3. 回收站按用户设置的保留期清理（默认 0 = 不清）。
///
/// 另外在启动那一轮顺手收拾旧版本散在 `$TMPDIR` 里的附件，只做一次。
pub fn spawn_maintenance(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut first_round = true;
        loop {
            if first_round {
                let pruned = crate::attachments::prune_legacy_temp(SystemTime::now());
                if pruned.files > 0 {
                    eprintln!(
                        "storage: removed {} stale temp attachment(s), {} bytes",
                        pruned.files, pruned.bytes
                    );
                }
                first_round = false;
            }

            crate::attachments::prune(&app);
            crate::image_cache::prune();

            let days = crate::app_storage::trash_retention_days();
            // 用户还没被告知「回收站会自动清理了」之前，一条都不许删 —— 否则那次提示
            // 就成了事后通知。确认之后 `ack_trash_retention` 会立刻补清一次。
            if days > 0 && crate::app_storage::trash_retention_acknowledged() {
                match crate::trash::purge_expired(days) {
                    Ok(purged) if !purged.is_empty() => {
                        eprintln!("storage: purged {} expired trash item(s)", purged.len());
                        let _ = tauri::Emitter::emit(&app, "trash:purged", purged.len());
                    }
                    Err(error) => eprintln!("storage: trash purge failed: {error}"),
                    _ => {}
                }
            }

            std::thread::sleep(Duration::from_secs(24 * 60 * 60));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("cc-storage-gc-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// 写一个文件并把 mtime 设成「`age_secs` 秒之前」。
    fn write_aged(path: &Path, bytes: &[u8], now: SystemTime, age_secs: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
        let when = now - Duration::from_secs(age_secs);
        let time = filetime::FileTime::from_system_time(when);
        filetime::set_file_mtime(path, time).unwrap();
    }

    #[test]
    fn total_bytes_sums_nested_files() {
        let root = scratch("total");
        fs::write(root.join("a"), b"12345").unwrap();
        fs::create_dir_all(root.join("2026-01")).unwrap();
        fs::write(root.join("2026-01").join("b"), b"123").unwrap();

        assert_eq!(total_bytes(&root), 8);
        assert_eq!(total_bytes(&root.join("nope")), 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prune_removes_only_expired_files() {
        let root = scratch("age");
        let now = UNIX_EPOCH + Duration::from_secs(10_000_000);
        write_aged(&root.join("old"), b"aaaa", now, 40 * 86_400);
        write_aged(&root.join("fresh"), b"bbbb", now, 86_400);

        let pruned = prune(
            &root,
            Policy {
                max_age: Some(Duration::from_secs(30 * 86_400)),
                max_bytes: None,
            },
            now,
        );

        assert_eq!(pruned, Pruned { files: 1, bytes: 4 });
        assert!(!root.join("old").exists());
        assert!(root.join("fresh").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prune_drops_the_oldest_until_it_fits_the_budget() {
        let root = scratch("size");
        let now = UNIX_EPOCH + Duration::from_secs(10_000_000);
        write_aged(&root.join("oldest"), &[0; 100], now, 300);
        write_aged(&root.join("middle"), &[0; 100], now, 200);
        write_aged(&root.join("newest"), &[0; 100], now, 100);

        let pruned = prune(
            &root,
            Policy {
                max_age: None,
                max_bytes: Some(150),
            },
            now,
        );

        // 300 > 150：删掉最旧的两个才降到 100。
        assert_eq!(pruned, Pruned { files: 2, bytes: 200 });
        assert!(!root.join("oldest").exists());
        assert!(!root.join("middle").exists());
        assert!(root.join("newest").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prune_is_a_no_op_inside_both_limits() {
        let root = scratch("noop");
        let now = UNIX_EPOCH + Duration::from_secs(10_000_000);
        write_aged(&root.join("a"), &[0; 10], now, 100);

        let pruned = prune(
            &root,
            Policy {
                max_age: Some(Duration::from_secs(86_400)),
                max_bytes: Some(1024),
            },
            now,
        );

        assert_eq!(pruned, Pruned::default());
        assert!(root.join("a").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prune_clears_month_folders_it_emptied() {
        let root = scratch("empty-dirs");
        let now = UNIX_EPOCH + Duration::from_secs(10_000_000);
        write_aged(&root.join("2026-01").join("a"), b"a", now, 40 * 86_400);
        write_aged(&root.join("2026-02").join("b"), b"b", now, 86_400);

        prune(
            &root,
            Policy {
                max_age: Some(Duration::from_secs(30 * 86_400)),
                max_bytes: None,
            },
            now,
        );

        assert!(!root.join("2026-01").exists());
        assert!(root.join("2026-02").join("b").exists());
        assert!(root.is_dir());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prune_ignores_symlinked_entries() {
        let root = scratch("symlink");
        let outside = scratch("symlink-target");
        let now = UNIX_EPOCH + Duration::from_secs(10_000_000);
        write_aged(&outside.join("precious"), &[0; 500], now, 400 * 86_400);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let pruned = prune(
            &root,
            Policy {
                max_age: Some(Duration::from_secs(86_400)),
                max_bytes: Some(0),
            },
            now,
        );

        assert_eq!(pruned, Pruned::default());
        assert!(outside.join("precious").exists());

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
