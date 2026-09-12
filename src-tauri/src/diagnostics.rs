//! 运行时自检：一条命令把「现在到底是谁在占内存 / 占磁盘」摊开。
//!
//! 用户反馈「越用越卡」「内存好高」时，最贵的一步是定位在哪一层。主进程和 WebContent
//! 是两个独立进程，各自的常驻缓存又散在七八个模块里，靠猜要来回好几轮。这里让 app
//! 自己报：设置页「诊断」区一屏截图就能定位。
//!
//! 只读，不改任何状态。取不到的项一律给 0 / None，绝不因为诊断本身失败而报错。

#[cfg(windows)]
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};
#[cfg(target_os = "macos")]
use std::{collections::HashSet, ffi::c_void, sync::OnceLock};

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

/// 读取进程 RSS。Unix 走 `ps`，Windows 走 PSAPI。
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

#[cfg(windows)]
fn rss_bytes(pid: u32) -> u64 {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    unsafe {
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if process.is_null() {
            return 0;
        }

        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        let ok = GetProcessMemoryInfo(process, &mut counters, counters.cb);
        CloseHandle(process);
        (ok != 0)
            .then_some(counters.WorkingSetSize as u64)
            .unwrap_or(0)
    }
}

#[cfg(not(any(unix, windows)))]
fn rss_bytes(_pid: u32) -> u64 {
    0
}

/// 归属于本 app 的 WebKit 渲染进程 RSS 之和。
///
/// 这个数才是「内存越用越高」真正该看的那一个 —— transcript、DOM、解码后的图片全在
/// 渲染进程里，主进程通常只有几十 MB。
///
/// 认领哪个 `com.apple.WebKit.WebContent` 是我们的 —— 两个信号取并集，缺一不可。
///
/// WebContent 是 WebKit 通过 XPC 起的：父进程是 launchd（ppid=1），命令行、`comm`、
/// `proc_name` 全都是 `com.apple.WebKit.WebContent`，从进程树上认不出来。能用的只有两条：
///
/// 1. **responsible pid**（Activity Monitor 用的就是它）：打包版由 LaunchServices 拉起，
///    responsible 就是 app 自己，认得准。dev 实例从终端起，responsible 会算到终端 app
///    头上，而终端自己往往也有 webview，分不开。
/// 2. **lsof 看 WebKit 网络缓存**：dev 加载 `http://localhost:1420` 走真实网络栈，
///    WebContent 会持有缓存目录里的 fd。打包版加载 `tauri://localhost` 这个自定义
///    scheme，资源由主进程直接喂，压根不碰网络缓存，一个应用 fd 都不开。
///
/// 两条正好互补，(1) 优先：它一旦认到就是确凿的，此时 (2) 只会添乱 —— dev 实例和已安装
/// 版本共用同一个 WebKit 缓存目录，并集会让打包版把 dev 的渲染进程一起算进来。只有 (1)
/// 一个都没认到（即跑在 dev 里）才退到 (2)。
///
/// 边界：两条都认不到时返回 0（UI 显示「—」）；dev 下同时开着两个 dev 实例仍会算到一起。
/// 不影响它作为趋势指标的用途。
#[cfg(target_os = "macos")]
fn webview_rss_bytes(data_dir: Option<&Path>) -> u64 {
    let candidates = webcontent_candidates();
    if candidates.is_empty() {
        return 0;
    }
    let self_pid = std::process::id() as i32;
    let mut ours: HashSet<i32> = candidates
        .iter()
        .filter(|(pid, _)| responsible_pid(*pid) == Some(self_pid))
        .map(|(pid, _)| *pid)
        .collect();
    if ours.is_empty() {
        let all: Vec<i32> = candidates.iter().map(|(pid, _)| *pid).collect();
        ours = webcontent_pids_holding_our_files(&all, data_dir);
    }

    candidates
        .iter()
        .filter(|(pid, _)| ours.contains(pid))
        .map(|(_, bytes)| bytes)
        .sum()
}

/// 机器上所有 WebContent 进程及其 RSS。
///
/// 只认 argv[0] 而不是「命令行里出现过 WebKit.WebContent」：本进程会 spawn 一堆 PTY
/// 子进程，它们的 responsible 也是我们，宽松匹配会把它们的内存一起算进来。
#[cfg(target_os = "macos")]
fn webcontent_candidates() -> Vec<(i32, u64)> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,rss=,command="])
        .output()
    else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(output.stdout) else {
        return Vec::new();
    };
    parse_webcontent_candidates(&text)
}

/// `ps -axo pid=,rss=,command=` 的输出里挑出 WebContent 行。
#[cfg(target_os = "macos")]
fn parse_webcontent_candidates(text: &str) -> Vec<(i32, u64)> {
    let mut candidates = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(rss), Some(argv0)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if !argv0.ends_with("/com.apple.WebKit.WebContent") {
            continue;
        }
        if let (Ok(pid), Ok(kilobytes)) = (pid.parse::<i32>(), rss.parse::<u64>()) {
            candidates.push((pid, kilobytes * 1024));
        }
    }
    candidates
}

/// 这些 WebContent 里，哪些打开着本 app 的文件（WebKit 网络缓存目录 / 数据目录）。
#[cfg(target_os = "macos")]
fn webcontent_pids_holding_our_files(pids: &[i32], data_dir: Option<&Path>) -> HashSet<i32> {
    if pids.is_empty() {
        return HashSet::new();
    }
    let mut markers: Vec<String> = Vec::new();
    if let (Some(cache), Some(name)) = (dirs::cache_dir(), executable_name()) {
        markers.push(cache.join(name).to_string_lossy().into_owned());
    }
    if let Some(dir) = data_dir {
        markers.push(dir.to_string_lossy().into_owned());
    }
    if markers.is_empty() {
        return HashSet::new();
    }
    let joined = pids
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = std::process::Command::new("lsof")
        .args(["-p", &joined, "-Fpn"])
        .output()
    else {
        return HashSet::new();
    };
    let Ok(listing) = String::from_utf8(output.stdout) else {
        return HashSet::new();
    };
    parse_lsof_owners(&listing, &markers)
}

/// `lsof -Fpn` 是逐字段一行：`p<pid>` 起一个进程块，后面的 `n<path>` 都属于它。
#[cfg(target_os = "macos")]
fn parse_lsof_owners(listing: &str, markers: &[String]) -> HashSet<i32> {
    let mut ours = HashSet::new();
    let mut current: Option<i32> = None;
    for line in listing.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            current = pid.parse().ok();
        } else if let Some(name) = line.strip_prefix('n') {
            if let (Some(pid), true) = (
                current,
                markers
                    .iter()
                    .any(|marker| name.starts_with(marker.as_str())),
            ) {
                ours.insert(pid);
            }
        }
    }
    ours
}

/// macOS 的「responsible process」：Activity Monitor 就是靠它把 WebContent 归到宿主 app
/// 名下。符号在 libSystem 里但没有公开头文件 —— 直接 extern 链接的话，哪天系统抽掉它
/// 就是启动即崩，比少显示一个数字严重得多。所以 dlsym 运行时查，查不到当没有。
#[cfg(target_os = "macos")]
fn responsible_pid(pid: i32) -> Option<i32> {
    type ResponsibleFor = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
    static SYMBOL: OnceLock<Option<ResponsibleFor>> = OnceLock::new();

    let resolved = (*SYMBOL.get_or_init(|| {
        // macOS 的 RTLD_DEFAULT（`(void *)-2`）—— libc crate 没导出这个常量。
        const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
        const NAME: &[u8] = b"responsibility_get_pid_responsible_for_pid\0";
        let symbol = unsafe { libc::dlsym(RTLD_DEFAULT, NAME.as_ptr().cast()) };
        (!symbol.is_null())
            .then(|| unsafe { std::mem::transmute::<*mut c_void, ResponsibleFor>(symbol) })
    }))?;
    let found = unsafe { resolved(pid) };
    (found > 0).then_some(found)
}

#[cfg(target_os = "macos")]
fn executable_name() -> Option<String> {
    std::env::current_exe()
        .ok()?
        .file_name()?
        .to_str()
        .map(str::to_owned)
}

#[cfg(windows)]
fn webview_rss_bytes(_data_dir: Option<&Path>) -> u64 {
    let processes = windows_processes();
    if processes.is_empty() {
        return 0;
    }

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for process in &processes {
        children
            .entry(process.parent_pid)
            .or_default()
            .push(process.pid);
    }

    let mut queue = vec![std::process::id()];
    let mut descendants = HashSet::new();
    while let Some(parent_pid) = queue.pop() {
        if let Some(child_pids) = children.get(&parent_pid) {
            for &child_pid in child_pids {
                if descendants.insert(child_pid) {
                    queue.push(child_pid);
                }
            }
        }
    }

    processes
        .into_iter()
        .filter(|process| {
            descendants.contains(&process.pid)
                && process
                    .image_name
                    .eq_ignore_ascii_case("msedgewebview2.exe")
        })
        .map(|process| rss_bytes(process.pid))
        .sum()
}

#[cfg(windows)]
fn windows_processes() -> Vec<WindowsProcessInfo> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut out = Vec::new();
    unsafe {
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                out.push(WindowsProcessInfo {
                    pid: entry.th32ProcessID,
                    parent_pid: entry.th32ParentProcessID,
                    image_name: wide_string(&entry.szExeFile),
                });
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    out
}

#[cfg(windows)]
struct WindowsProcessInfo {
    pid: u32,
    parent_pid: u32,
    image_name: String,
}

#[cfg(windows)]
fn wide_string(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

#[cfg(not(any(target_os = "macos", windows)))]
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

#[cfg(windows)]
fn thread_count(pid: u32) -> usize {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return 0;
    }

    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut count = 0;
    unsafe {
        if Thread32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == pid {
                    count += 1;
                }
                if Thread32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    count
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
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

    if diagnostics.main_rss_bytes > RSS_WARN_BYTES || diagnostics.webview_rss_bytes > RSS_WARN_BYTES
    {
        append_warning(&diagnostics);
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(any(unix, windows))]
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

    /// 真实 `ps -axo pid=,rss=,command=` 的片段：WebContent 行认 argv[0]，别的都不算。
    #[cfg(target_os = "macos")]
    #[test]
    fn webcontent_candidates_match_on_argv0_only() {
        let text = concat!(
            " 4930  4096 /System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices",
            "/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent\n",
            "59496 74448 /System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices",
            "/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent\n",
            // 本进程 spawn 的 PTY 子进程：responsible 也是我们，命令行里带着这个字符串，
            // 但它不是渲染进程 —— 宽松匹配会把它的内存算进来。
            "25787  3392 /bin/zsh -c grep WebKit.WebContent /tmp/notes\n",
            "  501   128 /usr/sbin/cupsd -l\n",
        );
        assert_eq!(
            super::parse_webcontent_candidates(text),
            vec![(4930, 4096 * 1024), (59496, 74448 * 1024)]
        );
    }

    /// `lsof -Fpn` 的块结构：`p<pid>` 之后的 `n<path>` 都归那个 pid。
    #[cfg(target_os = "macos")]
    #[test]
    fn lsof_owners_attribute_paths_to_the_enclosing_pid_block() {
        let listing = concat!(
            "p33471\n",
            "n/System/Library/Fonts/Times.ttc\n",
            "n/Users/me/Library/Caches/cc-sessions-viewer/WebKit/NetworkCache/Version 17/Blobs/AB\n",
            "p34305\n",
            "n/System/Library/Fonts/SFNSMono.ttf\n",
            "n/Users/me/Library/Caches/some-other-app/WebKit/NetworkCache/Blobs/CD\n",
        );
        let markers = vec!["/Users/me/Library/Caches/cc-sessions-viewer".to_string()];
        let owners = super::parse_lsof_owners(listing, &markers);
        assert_eq!(owners.into_iter().collect::<Vec<_>>(), vec![33471]);
    }
}
