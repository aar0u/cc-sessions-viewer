//! 运行时自检：一条命令把「现在到底是谁在占内存 / 占磁盘」摊开。
//!
//! 用户反馈「越用越卡」「内存好高」时，最贵的一步是定位在哪一层。主进程和 WebContent
//! 是两个独立进程，各自的常驻缓存又散在七八个模块里，靠猜要来回好几轮。这里让 app
//! 自己报：设置页「诊断」区一屏截图就能定位。
//!
//! 只读，不改任何状态。取不到的项一律给 0 / None，绝不因为诊断本身失败而报错。

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::AppHandle;

/// 主进程 RSS 超过这个数就往 `diagnostics.log` 记一条，方便事后对时间线。
const RSS_WARN_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 两条 warn 之间的最小间隔 —— 诊断面板打开时是在轮询这条命令的。
const WARN_INTERVAL: Duration = Duration::from_secs(10 * 60);

static LAST_WARN_AT: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostics {
    /// Rust 主进程的常驻内存。
    pub main_rss_bytes: u64,
    /// 渲染进程（macOS 上是 `com.apple.WebKit.WebContent`）的常驻内存。取不到为 0。
    pub webview_rss_bytes: u64,
    /// 主进程线程数。轮询线程泄漏时这个数会一路往上爬。
    pub threads: usize,
    /// 全局搜索的正文缓存字节数（上限 64 MB）。
    pub user_text_cache_bytes: u64,
    /// 用量缓存条目数（上限 20000）。
    pub usage_cache_entries: usize,
    /// Claude 会话扫描缓存条目数（上限 20000）。
    pub scan_cache_entries: usize,
    /// 文件监听的 per-path map 条目数。会话关掉后应该回落，只增不减就是漏了。
    pub watch_map_entries: usize,
    /// 正在跑的 GUI chat 进程数。
    pub active_chats: usize,
    /// 记着 turn 状态的会话数（24 小时后自动剔除）。
    pub desktop_tasks: usize,
    /// 图片缓存目录字节数（上限 500 MB）。
    pub image_cache_bytes: u64,
    /// 附件目录字节数（上限 500 MB）。
    pub attachments_bytes: u64,
    /// 回收站字节数。
    pub trash_bytes: u64,
}

/// `ps -o rss= -p <pid>` 拿 KB 数。比 mach / procfs 那套省事，而且 macOS 与 Linux 通用。
#[cfg(unix)]
fn rss_bytes(pid: u32) -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(|kilobytes| kilobytes * 1024)
        .unwrap_or(0)
}

#[cfg(not(unix))]
fn rss_bytes(_pid: u32) -> u64 {
    0
}

/// 归属于本 app 的 WebKit 渲染进程 RSS 之和。
///
/// 这个数才是「内存越用越高」真正该看的那一个 —— transcript、DOM、解码后的图片全在
/// 渲染进程里，主进程通常只有几十 MB。
///
/// 认领方式是先列出机器上所有 `com.apple.WebKit.WebContent`，再用一次 `lsof` 看谁打开着
/// 本 app 的文件（WebKit 网络缓存目录 / 数据目录里的图片缓存）。听着绕，但没有更直接的
/// 办法：WebContent 是 WebKit 通过 XPC 起的，父进程是 launchd（ppid=1），命令行里也不带
/// 任何应用信息，从进程树上根本认不出来。
///
/// 两个已知的边界：认领不到时返回 0（UI 显示「—」）；dev 实例和已安装版本共用同一个
/// WebKit 缓存目录，两个一起开着的时候会算到一起。都不影响它作为趋势指标的用途。
#[cfg(target_os = "macos")]
fn webview_rss_bytes(data_dir: Option<&Path>) -> u64 {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,rss=,command="])
        .output()
    else {
        return 0;
    };
    let Ok(text) = String::from_utf8(output.stdout) else {
        return 0;
    };
    let mut candidates: Vec<(String, u64)> = Vec::new();
    for line in text.lines().filter(|line| line.contains("WebKit.WebContent")) {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(rss)) = (fields.next(), fields.next()) else {
            continue;
        };
        if let Ok(kilobytes) = rss.parse::<u64>() {
            candidates.push((pid.to_string(), kilobytes * 1024));
        }
    }
    if candidates.is_empty() {
        return 0;
    }

    let mut markers: Vec<String> = Vec::new();
    if let (Some(cache), Some(name)) = (dirs::cache_dir(), executable_name()) {
        markers.push(cache.join(name).to_string_lossy().into_owned());
    }
    if let Some(dir) = data_dir {
        markers.push(dir.to_string_lossy().into_owned());
    }
    if markers.is_empty() {
        return 0;
    }

    let pids = candidates
        .iter()
        .map(|(pid, _)| pid.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = std::process::Command::new("lsof")
        .args(["-p", &pids, "-Fpn"])
        .output()
    else {
        return 0;
    };
    let Ok(listing) = String::from_utf8(output.stdout) else {
        return 0;
    };
    // `-F` 是逐字段一行：`p<pid>` 起一个进程块，后面的 `n<path>` 都属于它。
    let mut ours: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut current = "";
    for line in listing.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            current = pid;
        } else if let Some(name) = line.strip_prefix('n') {
            if markers.iter().any(|marker| name.starts_with(marker.as_str())) {
                ours.insert(current);
            }
        }
    }

    candidates
        .iter()
        .filter(|(pid, _)| ours.contains(pid.as_str()))
        .map(|(_, bytes)| bytes)
        .sum()
}

#[cfg(target_os = "macos")]
fn executable_name() -> Option<String> {
    std::env::current_exe()
        .ok()?
        .file_name()?
        .to_str()
        .map(str::to_owned)
}

#[cfg(not(target_os = "macos"))]
fn webview_rss_bytes(_data_dir: Option<&Path>) -> u64 {
    0
}

#[cfg(target_os = "macos")]
fn thread_count(pid: u32) -> usize {
    std::process::Command::new("ps")
        .args(["-M", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        // 第一行是表头，其余每行一个线程。
        .map(|text| text.lines().skip(1).filter(|line| !line.is_empty()).count())
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn thread_count(pid: u32) -> usize {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn thread_count(_pid: u32) -> usize {
    0
}

/// 限流：抢到写 warn 的名额返回 true。诊断面板打开时是在轮询这条命令的，
/// 不限流会把 `diagnostics.log` 刷成一片。
fn take_warn_slot(now: Instant) -> bool {
    let Ok(mut last) = LAST_WARN_AT.lock() else {
        return false;
    };
    if last.is_some_and(|at| now.duration_since(at) < WARN_INTERVAL) {
        return false;
    }
    *last = Some(now);
    true
}

fn warning_line(diagnostics: &RuntimeDiagnostics, at: &str) -> String {
    format!(
        "{at} warn rss main={}MB webview={}MB threads={} chats={} watch={}\n",
        diagnostics.main_rss_bytes / 1024 / 1024,
        diagnostics.webview_rss_bytes / 1024 / 1024,
        diagnostics.threads,
        diagnostics.active_chats,
        diagnostics.watch_map_entries,
    )
}

fn append_warning(diagnostics: &RuntimeDiagnostics) {
    if !take_warn_slot(Instant::now()) {
        return;
    }
    let Some(directory) = dirs::data_local_dir().map(|base| base.join("cc-sessions-viewer")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&directory);
    let line = warning_line(
        diagnostics,
        &chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    );
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("diagnostics.log"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

#[tauri::command(async)]
pub fn runtime_diagnostics(app: AppHandle) -> RuntimeDiagnostics {
    let pid = std::process::id();
    let data = crate::app_storage::data_dir(&app).ok();
    let size_of = |name: &str| {
        data.as_ref()
            .map(|root| crate::storage_gc::total_bytes(&root.join(name)))
            .unwrap_or(0)
    };

    let diagnostics = RuntimeDiagnostics {
        main_rss_bytes: rss_bytes(pid),
        webview_rss_bytes: webview_rss_bytes(data.as_deref()),
        threads: thread_count(pid),
        user_text_cache_bytes: crate::agents::user_text_cache_bytes() as u64,
        usage_cache_entries: crate::agents::usage_cache_entries(),
        scan_cache_entries: crate::agents::claude::scan_cache_entries()
            + crate::agents::codex::scan_cache_entries(),
        watch_map_entries: crate::watch::tracked_path_count(),
        active_chats: crate::agent_chat::active_chat_count(),
        desktop_tasks: crate::turn::desktop_task_count(),
        image_cache_bytes: size_of("image-cache"),
        attachments_bytes: size_of("attachments"),
        trash_bytes: crate::storage_gc::total_bytes(Path::new(&crate::trash::trash_dir())),
    };

    if diagnostics.main_rss_bytes > RSS_WARN_BYTES
        || diagnostics.webview_rss_bytes > RSS_WARN_BYTES
    {
        append_warning(&diagnostics);
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn reads_this_process_rss_and_threads() {
        let pid = std::process::id();
        // 自己一定活着，这两个值必须是正数 —— 拿到 0 说明 `ps` 的取法在这个平台上不成立。
        assert!(rss_bytes(pid) > 0, "RSS should be readable for our own pid");
        assert!(thread_count(pid) > 0, "we always have at least one thread");
        // 不存在的 pid 不能 panic，只能得 0。
        assert_eq!(rss_bytes(u32::MAX), 0);
    }

    #[test]
    fn warnings_are_rate_limited() {
        let now = Instant::now();
        assert!(take_warn_slot(now), "first warning goes through");
        assert!(!take_warn_slot(now), "a second one right after is dropped");
        assert!(
            take_warn_slot(now + WARN_INTERVAL + Duration::from_secs(1)),
            "one interval later it opens up again"
        );
    }

    #[test]
    fn warning_line_reports_megabytes() {
        let line = warning_line(
            &RuntimeDiagnostics {
                main_rss_bytes: 5 * 1024 * 1024 * 1024,
                webview_rss_bytes: 512 * 1024 * 1024,
                threads: 42,
                active_chats: 2,
                watch_map_entries: 3,
                ..RuntimeDiagnostics::default()
            },
            "2026-09-09 19:00:00",
        );
        assert_eq!(
            line,
            "2026-09-09 19:00:00 warn rss main=5120MB webview=512MB threads=42 chats=2 watch=3\n"
        );
    }
}
