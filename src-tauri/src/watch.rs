// 实时 tail：监听打开会话所在 JSONL 文件的写入事件。
//
// 设计：
//   - 单订阅模型 —— 同一时刻只追一个文件（当前 ChatView 打开的那条）。
//     watch_session(agent, path) 替换上一个 watcher；unwatch_session() 清空。
//     对**同一** (agent, path) 重复调用是幂等的：不会重建 watcher、不会再起一条
//     轮询线程、也不会再整份重解析一遍（见下方「轮询线程只能有一条」）。
//   - 触发：notify 派来 Modify / Create 事件后，debounce 一小段（避免 IDE / agent
//     频繁追加 1 行就 emit 一次），再走一次"整文件 read_session"，把新增的 Msg
//     切片 emit 给前端。
//   - 整文件 re-parse 的代价：Claude 的解析器有跨行状态（queued user 消息缓冲、
//     工具结果配对等），增量解析需要重写解析器；MVP 选择"整文件再读一次 +
//     基于 Msg 数量取尾巴"，简单、可读、足够快（实测十几 MB 会话 < 50 ms）。
//     正因为单次重解析对大文件并不便宜（本机最大的 codex rollout 有 160 MB），
//     下面三道闸门缺一不可：debounce、文件指纹 CAS、最小处理间隔。
//   - 文件截断 / 删除：emit `session:reset`（前端整文件重拉）或 `session:gone`。
//
// 前端事件契约：
//   session:append   { path, messages: Msg[] }    新增的尾段
//   session:reset    { path }                      文件被截断/替换或标题变更 → 整文件重拉
//   session:gone     { path }                      文件不再存在
//
// 注意：这里不能直接盯单个 JSONL 文件。很多 CLI / 编辑器会用“先写临时文件，再 rename
// 覆盖”的原子替换模式落盘；如果只 watch 旧文件 inode，替换后 watcher 会失联，后续再有
// 新内容也收不到。这里统一 watch 父目录，每次事件 debounce 后回头检查目标文件当前状态，
// 这样 append / truncate / replace / recreate 都能兜住。
//
// 代价是同目录里**别的**会话文件被写入也会派事件过来 —— 所以 process_change 第一件事
// 就是比对目标文件自己的指纹，没变直接返回，绝不因为邻居文件的写入去重解析大文件。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::agents;
use crate::types::Msg;

/// 当前活跃 watcher 的内部状态。Drop 后 notify 回调会自然停。
struct WatchState {
    /// 让 watcher 活着 —— drop 后回调停。
    _watcher: RecommendedWatcher,
    /// 当前打开的目标文件。
    path: PathBuf,
    /// notify 实际监听的目录（各目标文件的父目录）。
    #[allow(dead_code)]
    watch_roots: Vec<PathBuf>,
    agent: String,
    /// 轮询兜底线程的停止旗标。替换 / unwatch 这个 state 时置位，线程下一轮自行退出。
    ///
    /// 这条旗标是必需的：轮询线程原本只靠「活跃 watch 的 (agent, path) 是不是我」
    /// 来决定退出，于是切走再切回同一个会话时，旧线程会重新认领自己 —— 每 watch 一次
    /// 就多一条 1.5 s 轮询线程，且它们都在对同一个大文件做整份重解析。
    poll_stop: Arc<AtomicBool>,
}

/// 单 watcher 槽：同一时刻只追一个文件，新订阅会替换旧 watcher。
static STATE: OnceLock<Mutex<Option<WatchState>>> = OnceLock::new();

/// 每个文件路径独立维护"上次 emit 的 Msg 数量"。
/// key = 绝对路径串；value = last_msg_count
static LAST_COUNT: OnceLock<Mutex<std::collections::HashMap<String, usize>>> = OnceLock::new();

/// 真正的 debounce：每次文件事件都 bump 一次序号并延后处理；只有睡眠结束后序号仍然
/// 是最新的那次事件才会触发整文件重读。这样既能合并 burst 写入，又不会把"唯一的一次"
/// 事件直接丢掉。
static DEBOUNCE_SEQ: OnceLock<Mutex<std::collections::HashMap<String, u64>>> = OnceLock::new();

fn state() -> &'static Mutex<Option<WatchState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

/// 诊断用：per-path map 里还记着多少条路径。会话关掉后应该回落到 0。
pub fn tracked_path_count() -> usize {
    last_count_map().lock().map(|map| map.len()).unwrap_or(0)
}

fn last_count_map() -> &'static Mutex<std::collections::HashMap<String, usize>> {
    LAST_COUNT.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn debounce_seq_map() -> &'static Mutex<std::collections::HashMap<String, u64>> {
    DEBOUNCE_SEQ.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 每个路径上次「整文件重解析」时的廉价指纹 (mtime, 字节数)。
/// 因为我们 watch 的是父目录（见文件头注释），同目录里**别的**会话文件被追加也会派事件；
/// 若每次都对目标大文件（可达数十 MB）做全量 read_session，就会被无关写入反复全量重读、CPU 打满。
/// 处理前先比指纹：目标文件没变就直接跳过昂贵的重解析。真正的追加会改 mtime/size,照常被捕获。
type FileFingerprint = Vec<(PathBuf, std::time::SystemTime, u64)>;
static LAST_STAT: OnceLock<Mutex<std::collections::HashMap<String, FileFingerprint>>> =
    OnceLock::new();

fn last_stat_map() -> &'static Mutex<std::collections::HashMap<String, FileFingerprint>> {
    LAST_STAT.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 每个路径上次看到的「标题指纹」（见 `SessionSource::metadata_fingerprint`）。
/// 只有实现了该钩子的 agent（Claude / Pi）才会有非 None 值。
type MetaFingerprint = Option<String>;
static LAST_META: OnceLock<Mutex<std::collections::HashMap<String, MetaFingerprint>>> =
    OnceLock::new();

fn last_meta_map() -> &'static Mutex<std::collections::HashMap<String, MetaFingerprint>> {
    LAST_META.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 每个路径上次真正走完 process_change 的时刻，用于最小处理间隔限流。
static LAST_PROCESS_AT: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> =
    OnceLock::new();

fn last_process_at_map() -> &'static Mutex<std::collections::HashMap<String, Instant>> {
    LAST_PROCESS_AT.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 目标文件的廉价指纹：(修改时间, 字节数)。取不到（文件不在 / 无权限）返回 None,
/// 此时调用方退回到「照常重读」，不因拿不到指纹而漏掉更新。
fn file_fingerprint(path: &str) -> Option<(std::time::SystemTime, u64)> {
    let md = std::fs::metadata(path).ok()?;
    Some((md.modified().ok()?, md.len()))
}

fn files_fingerprint(paths: &[PathBuf]) -> Option<FileFingerprint> {
    let mut fingerprint = Vec::with_capacity(paths.len());
    for path in paths {
        let (modified, size) = file_fingerprint(&path.to_string_lossy())?;
        fingerprint.push((path.clone(), modified, size));
    }
    Some(fingerprint)
}

fn watch_root_for(path: &Path) -> Result<PathBuf, String> {
    path.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "Cannot determine parent directory: {}",
            path.to_string_lossy()
        )
    })
}

/// 丢掉某条路径的全部 per-path 状态。unwatch / session:gone 时调用，
/// 否则这几张 map 只增不减，长时间使用后会攒下每一个打开过的会话。
fn forget_path(path: &str) {
    if let Ok(mut m) = last_count_map().lock() {
        m.remove(path);
    }
    if let Ok(mut m) = debounce_seq_map().lock() {
        m.remove(path);
    }
    if let Ok(mut m) = last_stat_map().lock() {
        m.remove(path);
    }
    if let Ok(mut m) = last_meta_map().lock() {
        m.remove(path);
    }
    if let Ok(mut m) = last_process_at_map().lock() {
        m.remove(path);
    }
}

/// debounce 窗口：notify 一次写入可能拆成多条事件，攒一拨再 emit。
/// 200ms 平衡：人类感知接近实时（<300ms 觉得是即时），又能压平 IDE / agent 的多次
/// 小写入。
const DEBOUNCE_MS: u64 = 200;
/// 文件系统事件在某些场景下会漏（例如 AppKit / CLI 写入模式差异）；轮询兜底能确保
/// 正在跑的会话最终还是会被补进来。频率保持低一些，避免空转。
const POLL_MS: u64 = 1500;
/// 同一路径两次「整文件重解析」之间的最小间隔。debounce 只能合并**一拨**事件；
/// agent 持续输出时事件是连绵不断的，没有这道闸门，一个 160 MB 的会话会被每 200 ms
/// 重解析一次。被限流跳过的变更由 POLL_MS 轮询在下一轮补上，最多迟 1.5 s。
const MIN_PROCESS_INTERVAL_MS: u64 = 400;

#[derive(Serialize, Clone)]
struct AppendPayload {
    path: String,
    messages: Vec<Msg>,
}

#[derive(Serialize, Clone)]
struct PathPayload {
    path: String,
}

/// 事件只发给主窗口。`Emitter::emit` 会广播给**所有** webview，包括桌宠窗口 ——
/// 那边没有任何监听者，却仍要为每条事件付一次 JSON 序列化 + evaluateJavaScript。
/// session:append 的 payload 是整段新增消息，广播的浪费很实在。
fn emit_main<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    let _ = app.emit_to(crate::MAIN_WINDOW_LABEL, event, payload);
}

/// 订阅一条会话的 file watch。
///
/// 对同一个 (agent, path) 重复调用是**幂等**的 —— 只在提供了 `known_count` 时顺手
/// 校准一下 baseline，不会重建 watcher、不会再起轮询线程、也不会再整份解析一遍。
/// 这一条很关键：前端在「打开会话」「窗口重新聚焦」「切回已打开的 tab」等多个入口
/// 都会调它，原实现每一次都会新起一条常驻轮询线程并整份重解析。
///
/// `known_count`：前端刚刚 `read_session` 拿到的消息条数。给了就直接当 baseline，
/// 省掉这里再解析一遍同一个文件（打开一个大会话原本要解析两遍）。前端只在「确实
/// 刚读完同一个 path」时才传，其余情况传 None 由后端自己建立 baseline。
/// 不存在的路径返回错误；前端可以选择降级到不 tail。
pub fn watch_session(
    app: AppHandle,
    agent: String,
    path: String,
    known_count: Option<usize>,
) -> Result<(), String> {
    // 幂等短路：已经在追同一个会话了，什么都不用重建。
    {
        let slot = state().lock().map_err(|e| e.to_string())?;
        if let Some(active) = slot.as_ref() {
            if active.agent == agent && active.path == Path::new(&path) {
                if let Some(count) = known_count {
                    // 前端手上的条数才是 append 该从哪里切的权威值。
                    if let Ok(mut m) = last_count_map().lock() {
                        m.insert(path.clone(), count);
                    }
                }
                return Ok(());
            }
        }
    }

    let src = agents::source(&agent)?;
    // 实际盯的磁盘文件由 agent 决定：文件型 = 会话文件自身；agy = transcript_full 优先；
    // opencode（虚拟路径）= 库的 -wal 文件。notify 挂在目标文件的父目录上（原子替换兜底）。
    let targets = src.watch_targets(&path);
    if targets.is_empty() {
        return Err(format!("No watchable file for: {path}"));
    }
    if let Some(missing) = targets.iter().find(|target| !target.exists()) {
        return Err(format!("File does not exist: {}", missing.display()));
    }
    let mut watch_roots = Vec::new();
    for target in &targets {
        let root = watch_root_for(target)?;
        if !watch_roots.contains(&root) {
            watch_roots.push(root);
        }
    }
    let p = PathBuf::from(&path);

    // 先把 baseline 写好，避免 watcher 起来后回调先到 process_change 时拿不到 count。
    let baseline = match known_count {
        Some(count) => count,
        None => src.read_session(&path).unwrap_or_default().len(),
    };
    {
        let mut m = last_count_map().lock().map_err(|e| e.to_string())?;
        m.insert(path.clone(), baseline);
    }
    // 记下初始指纹（使用对应目标真实落盘文件的指纹）,后续无关目录事件才能被廉价短路掉
    if let Some(fp) = files_fingerprint(&targets) {
        if let Ok(mut m) = last_stat_map().lock() {
            m.insert(path.clone(), fp);
        }
    }
    // 标题 baseline：没有它的话，第一次「消息数没变」的事件会被误判成标题变更。
    {
        let meta = src.metadata_fingerprint(&path);
        if let Ok(mut m) = last_meta_map().lock() {
            m.insert(path.clone(), meta);
        }
    }

    let app_handle = app.clone();
    let agent_for_cb = agent.clone();
    let path_for_cb = path.clone();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(ev) = res else { return };
            if !matches!(
                ev.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                return;
            }
            let seq = {
                let mut m = match debounce_seq_map().lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let next = m.get(&path_for_cb).copied().unwrap_or(0) + 1;
                m.insert(path_for_cb.clone(), next);
                next
            };
            let app_for_job = app_handle.clone();
            let agent_for_job = agent_for_cb.clone();
            let path_for_job = path_for_cb.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(DEBOUNCE_MS));
                let latest = debounce_seq_map()
                    .lock()
                    .ok()
                    .and_then(|m| m.get(&path_for_job).copied());
                if latest != Some(seq) {
                    return;
                }
                process_change(&app_for_job, &agent_for_job, &path_for_job);
            });
        })
        .map_err(|e| format!("notify init failed: {e}"))?;

    for watch_root in &watch_roots {
        watcher
            .watch(watch_root, RecursiveMode::NonRecursive)
            .map_err(|e| format!("watch failed: {e}"))?;
    }

    // notify 事件偶发漏报时，轮询兜底仍能把新消息补进来。这条线程与本次 WatchState
    // 一一绑定：state 被替换 / unwatch 时 poll_stop 置位，线程下一轮立刻退出。
    let poll_stop = Arc::new(AtomicBool::new(false));
    {
        let app_for_poll = app.clone();
        let agent_for_poll = agent.clone();
        let path_for_poll = path.clone();
        let stop_for_poll = poll_stop.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(POLL_MS));
            if stop_for_poll.load(Ordering::Relaxed) {
                return;
            }
            process_change(&app_for_poll, &agent_for_poll, &path_for_poll);
        });
    }

    // 替换上一个 watcher（如果有）—— 旧 RecommendedWatcher 随 WatchState drop，
    // 旧轮询线程由它的 poll_stop 收尾。
    let previous = {
        let mut slot = state().lock().map_err(|e| e.to_string())?;
        slot.replace(WatchState {
            _watcher: watcher,
            path: p,
            watch_roots,
            agent,
            poll_stop,
        })
    };
    if let Some(previous) = previous {
        retire(previous);
    }
    Ok(())
}

/// 停掉一个已经不再活跃的 WatchState：结束它的轮询线程并清掉它的 per-path 状态。
fn retire(previous: WatchState) {
    previous.poll_stop.store(true, Ordering::Relaxed);
    forget_path(&previous.path.to_string_lossy());
}

/// 停止当前 watcher；没有活跃 watcher 时为空操作。前端 unmount / 切会话时调用。
pub fn unwatch_session() -> Result<(), String> {
    let previous = {
        let mut slot = state().lock().map_err(|e| e.to_string())?;
        slot.take()
    };
    if let Some(previous) = previous {
        retire(previous);
    }
    Ok(())
}

/// 单次文件变更处理：整文件重解析 → 跟上次 emit 的数量比 → emit 尾段或 reset。
fn process_change(app: &AppHandle, agent: &str, path: &str) {
    process_change_inner(app, agent, path, true);
}

/// `throttle = false` 用于用户主动触发的检查（窗口重新聚焦），此时不该被最小间隔挡掉。
fn process_change_inner(app: &AppHandle, agent: &str, path: &str, throttle: bool) {
    // 旧 watcher / 已切走的会话不再处理，避免延迟任务把过期 append 打到前端。
    {
        let slot = match state().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(active) = slot.as_ref() else {
            return;
        };
        if active.agent != agent || active.path != Path::new(path) {
            return;
        }
    }

    // 限流闸门放在指纹认领之前：被挡掉的这次不能算「已处理」，否则真变更会被吞掉。
    if throttle {
        let mut m = match last_process_at_map().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(previous) = m.get(path) {
            if previous.elapsed() < Duration::from_millis(MIN_PROCESS_INTERVAL_MS) {
                return;
            }
        }
        m.insert(path.to_string(), Instant::now());
    }

    let src = match agents::source(agent) {
        Ok(s) => s,
        Err(_) => return,
    };
    let targets = src.watch_targets(path);
    if targets.is_empty() {
        return;
    }

    if targets.iter().any(|target| !target.exists()) {
        emit_main(
            app,
            "session:gone",
            PathPayload {
                path: path.to_string(),
            },
        );
        forget_path(path);
        return;
    }

    // 廉价短路：目标文件指纹（mtime+size）与上次处理时相同 → 这次事件是同目录里**别的**
    // 文件在写,直接返回,别对大文件做全量 read_session。
    //
    // 「读-比-写」必须在同一把锁里完成：debounce 线程与轮询线程可能同时进到这里，
    // 分别读到同一个旧指纹、于是**各自**对同一个大文件跑一遍整份解析。先认领再解析，
    // 后来者看到新指纹直接退出。
    let claimed = files_fingerprint(&targets);
    if let Some(ref fingerprint) = claimed {
        let mut m = match last_stat_map().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if m.get(path) == Some(fingerprint) {
            return;
        }
        m.insert(path.to_string(), fingerprint.clone());
    }

    let msgs = match src.read_session(path) {
        Ok(m) => m,
        Err(_) => {
            // 解析失败：撤销刚才的指纹认领，让下一次事件 / 轮询还能重试这次变更。
            if claimed.is_some() {
                if let Ok(mut m) = last_stat_map().lock() {
                    m.remove(path);
                }
            }
            return;
        }
    };

    let prev_count = {
        let m = match last_count_map().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        m.get(path).copied().unwrap_or(0)
    };

    if msgs.len() < prev_count {
        // 文件被截断 / 替换 → 让前端整段重拉。
        emit_main(
            app,
            "session:reset",
            PathPayload {
                path: path.to_string(),
            },
        );
        let mut m = match last_count_map().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        m.insert(path.to_string(), msgs.len());
        return;
    }

    if msgs.len() == prev_count {
        // 消息数没变。对绝大多数 agent 这意味着这次写入是不产生消息的内部记录
        // （progress / token_count / file-history-snapshot / queue-operation …），
        // 实时会话里它们每秒能来好几条 —— 无条件 emit reset 会让前端对着一个
        // 上百 MB 的会话反复整份重拉，这是内存与卡顿的主要放大器。
        //
        // 只有真正改变了展示的元数据（目前只有标题：Claude 的 custom-title /
        // Pi 的 session_info）才让前端重拉。没有实现该钩子的 agent 恒为 None，
        // 于是自然地静默。
        let meta_now = src.metadata_fingerprint(path);
        let changed = {
            let mut m = match last_meta_map().lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match m.get(path) {
                Some(previous) if *previous == meta_now => false,
                _ => {
                    m.insert(path.to_string(), meta_now);
                    true
                }
            }
        };
        if changed {
            emit_main(
                app,
                "session:reset",
                PathPayload {
                    path: path.to_string(),
                },
            );
        }
        return;
    }

    // 真有新增 —— 切尾 emit。
    let tail = msgs[prev_count..].to_vec();
    emit_main(
        app,
        "session:append",
        AppendPayload {
            path: path.to_string(),
            messages: tail,
        },
    );
    if let Ok(mut m) = last_count_map().lock() {
        m.insert(path.to_string(), msgs.len());
    }
}

pub fn check_watched_session(app: AppHandle) -> Result<(), String> {
    let active_info = {
        let slot = state().lock().map_err(|e| e.to_string())?;
        slot.as_ref().map(|active| {
            (
                active.agent.clone(),
                active.path.to_string_lossy().to_string(),
            )
        })
    };
    if let Some((agent, path)) = active_info {
        // 用户主动触发（窗口重新聚焦）—— 绕过最小间隔限流，立刻给一次真实答复。
        process_change_inner(&app, &agent, &path, false);
    }
    Ok(())
}

/// 测试用：当前是否有活跃 watch。
#[cfg(test)]
pub fn is_watching() -> bool {
    state().lock().map(|g| g.is_some()).unwrap_or(false)
}

/// 测试用：当前 watch 的路径（如果有）。
#[cfg(test)]
pub fn current_path() -> Option<String> {
    state()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.path.to_string_lossy().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没起过 watcher 时 is_watching 必须是 false；unwatch 永远是 Ok。
    /// 注意：unit test 共用进程，OnceLock 状态跨测试持续，所以这条要先 unwatch 一次清场。
    #[test]
    fn unwatch_is_idempotent_and_state_starts_empty() {
        let _ = unwatch_session();
        assert!(!is_watching());
        assert!(current_path().is_none());
        // 再次 unwatch 仍 Ok，不会 panic
        assert!(unwatch_session().is_ok());
    }

    /// last_count_map 的 entry 是按 path 隔离的；不同 path 互不污染。
    /// 这条直接走内部 map，避开 notify watcher（需要真实文件 + AppHandle）。
    #[test]
    fn last_count_map_is_keyed_per_path() {
        let m = last_count_map();
        {
            let mut g = m.lock().unwrap();
            g.insert("/tmp/a.jsonl".into(), 3);
            g.insert("/tmp/b.jsonl".into(), 7);
        }
        let g = m.lock().unwrap();
        assert_eq!(g.get("/tmp/a.jsonl").copied(), Some(3));
        assert_eq!(g.get("/tmp/b.jsonl").copied(), Some(7));
    }

    /// debounce 序号同样按 path 隔离；新事件只覆盖自己的路径。
    #[test]
    fn debounce_seq_is_keyed_per_path() {
        let m = debounce_seq_map();
        {
            let mut g = m.lock().unwrap();
            g.insert("/tmp/a.jsonl".into(), 1);
            g.insert("/tmp/b.jsonl".into(), 4);
        }
        let g = m.lock().unwrap();
        assert_eq!(g.get("/tmp/a.jsonl").copied(), Some(1));
        assert_eq!(g.get("/tmp/b.jsonl").copied(), Some(4));
    }

    /// 原子替换场景下必须 watch 父目录而不是目标文件本身。
    #[test]
    fn watch_root_uses_parent_directory() {
        let p = PathBuf::from("/tmp/demo/rollout.jsonl");
        let root = watch_root_for(&p).unwrap();
        assert_eq!(root, PathBuf::from("/tmp/demo"));
    }

    /// 退订必须把该路径的全部 per-path 状态一起丢掉，否则这几张 map 会攒下
    /// 每一个打开过的会话，永不回收。
    #[test]
    fn forget_path_clears_every_per_path_map() {
        let path = "/tmp/forget-me.jsonl";
        last_count_map().lock().unwrap().insert(path.into(), 5);
        debounce_seq_map().lock().unwrap().insert(path.into(), 9);
        last_stat_map().lock().unwrap().insert(path.into(), vec![]);
        last_meta_map()
            .lock()
            .unwrap()
            .insert(path.into(), Some("t".into()));
        last_process_at_map()
            .lock()
            .unwrap()
            .insert(path.into(), Instant::now());

        forget_path(path);

        assert!(!last_count_map().lock().unwrap().contains_key(path));
        assert!(!debounce_seq_map().lock().unwrap().contains_key(path));
        assert!(!last_stat_map().lock().unwrap().contains_key(path));
        assert!(!last_meta_map().lock().unwrap().contains_key(path));
        assert!(!last_process_at_map().lock().unwrap().contains_key(path));
    }

    /// 指纹认领是「读-比-写」原子的：第一个调用者认领后，后来者看到的就是新指纹。
    /// 这正是并发的 debounce 线程与轮询线程不会各自整份重解析同一个大文件的原因。
    #[test]
    fn fingerprint_claim_is_visible_to_the_next_caller() {
        let path = "/tmp/claim.jsonl";
        forget_path(path);
        let fingerprint: FileFingerprint =
            vec![(PathBuf::from(path), std::time::SystemTime::UNIX_EPOCH, 42)];

        let first_claimed = {
            let mut m = last_stat_map().lock().unwrap();
            let claimed = m.get(path) != Some(&fingerprint);
            m.insert(path.to_string(), fingerprint.clone());
            claimed
        };
        let second_claimed = {
            let mut m = last_stat_map().lock().unwrap();
            let claimed = m.get(path) != Some(&fingerprint);
            m.insert(path.to_string(), fingerprint.clone());
            claimed
        };

        assert!(first_claimed, "第一个调用者应认领成功");
        assert!(!second_claimed, "第二个调用者应看到已认领的指纹并退出");
        forget_path(path);
    }
}
