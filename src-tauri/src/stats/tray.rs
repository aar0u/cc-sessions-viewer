// 托盘弹窗快速统计：一次扫描同时产出 today / 7d / month 三个时间窗口的 per-agent 汇总。
//
// 设计：复用 SessionSource::read_turns + pricing，但不经 Aggregator（那个太重，
// 带排行 / 分类 / 图表等我们不需要的东西）。直接按 turn 累加 token + cost。
// 三个时间窗口在单次遍历里同时判定，避免扫三遍。
//
// 性能：跟全局统计走同样的文件列表，但跳过了 activity 分类 / by_model / by_tool /
// daily timeline 等高开销维度。大约比 stream::run_worker 快 2–3×。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use chrono::{Datelike, Duration as CDuration, Local, TimeZone};

use crate::agents;
use crate::types::{TrayAgentSummary, TrayStats};

// ============================ 按 (path, mtime) 缓存的逐调用记录 ============================
//
// 托盘每 5 分钟重算一次，原实现每次都把「近 30 天内改过的所有会话」整份 read_turns
// 一遍。本机语料 2.5 GB，等于每 5 分钟烧 5 分钟 CPU + 海量短命分配（主进程 RSS 只升
// 不降的一大来源）。
//
// 绝大多数会话在两次刷新之间根本没动，所以按 (路径, mtime) 缓存解析结果：第二轮起
// 只有真正被写过的文件才重新解析。
//
// 缓存的是「托盘用得上的最小信息」而不是完整 `Turn` —— Turn 还带 tools /
// bash_commands / mcp_servers，托盘一个都用不到。

/// 一次模型调用在托盘口径下的最小投影。
#[derive(Clone)]
struct TrayCall {
    ts_ms: u64,
    /// 跨文件去重用（Claude 的 fork / continue 会把同一条 assistant 消息复制到多个
    /// JSONL）。去重必须逐调用进行，所以这里不能预先按时间窗口求和。
    message_id: Option<String>,
    tokens: u64,
    cost: f64,
    call_weight: u64,
    pricing_missing: bool,
    pricing_estimated: bool,
}

struct TrayFileEntry {
    mtime: u64,
    seq: u64,
    calls: Arc<[TrayCall]>,
}

#[derive(Default)]
struct TrayCallCache {
    entries: HashMap<String, TrayFileEntry>,
    calls: usize,
    next_seq: u64,
}

/// 缓存的调用条数上限（约 30 MB 量级）。修内存问题时引入的缓存自己不能变成新的
/// 内存问题；超限按插入序淘汰，被淘汰的文件下一轮重新解析即可。
const TRAY_CACHE_MAX_CALLS: usize = 300_000;

static TRAY_CALL_CACHE: Mutex<Option<TrayCallCache>> = Mutex::new(None);

fn cached_tray_calls(path: &str, mtime: u64) -> Option<Arc<[TrayCall]>> {
    let guard = TRAY_CALL_CACHE.lock().ok()?;
    let cache = guard.as_ref()?;
    let entry = cache.entries.get(path)?;
    (entry.mtime == mtime).then(|| entry.calls.clone())
}

fn store_tray_calls(path: &str, mtime: u64, calls: Arc<[TrayCall]>) {
    let Ok(mut guard) = TRAY_CALL_CACHE.lock() else {
        return;
    };
    let cache = guard.get_or_insert_with(TrayCallCache::default);
    let len = calls.len();
    let seq = cache.next_seq;
    cache.next_seq += 1;
    let replaced = cache
        .entries
        .insert(path.to_string(), TrayFileEntry { mtime, seq, calls });
    if let Some(previous) = replaced {
        cache.calls = cache.calls.saturating_sub(previous.calls.len());
    }
    cache.calls = cache.calls.saturating_add(len);
    evict_tray_calls(cache);
}

/// 超出条数上限时按插入序淘汰到 75%。留出余量是为了避免在上限附近来回抖动、
/// 每次插入都触发一轮淘汰。
fn evict_tray_calls(cache: &mut TrayCallCache) {
    if cache.calls <= TRAY_CACHE_MAX_CALLS {
        return;
    }
    let target = TRAY_CACHE_MAX_CALLS / 4 * 3;
    let mut by_seq: Vec<(u64, String)> = cache
        .entries
        .iter()
        .map(|(key, entry)| (entry.seq, key.clone()))
        .collect();
    by_seq.sort_unstable_by_key(|(seq, _)| *seq);
    for (_, key) in by_seq {
        if cache.calls <= target {
            break;
        }
        if let Some(entry) = cache.entries.remove(&key) {
            cache.calls = cache.calls.saturating_sub(entry.calls.len());
        }
    }
}

/// 拿到一个会话文件的逐调用投影：命中缓存直接返回，否则解析一次再存。
/// `modified` 既是缓存键的一部分，也是 turn 自身没带时间戳时的回退时刻。
fn tray_calls(
    src: &(dyn agents::SessionSource + Sync),
    path: &str,
    modified: u64,
) -> Arc<[TrayCall]> {
    if let Some(cached) = cached_tray_calls(path, modified) {
        return cached;
    }
    let turns = src.read_turns(path).unwrap_or_default();
    let mut calls: Vec<TrayCall> = Vec::new();
    for turn in &turns {
        let ts_ms = if turn.timestamp_ms > 0 {
            turn.timestamp_ms as u64
        } else {
            modified
        };
        for call in &turn.calls {
            if call.call_count == 0 {
                continue;
            }
            calls.push(TrayCall {
                ts_ms,
                message_id: call.message_id.clone(),
                tokens: call.usage.total,
                cost: call.cost_usd,
                call_weight: call.call_count,
                pricing_missing: call.pricing_missing,
                pricing_estimated: call.pricing_estimated,
            });
        }
    }
    let calls: Arc<[TrayCall]> = calls.into();
    store_tray_calls(path, modified, calls.clone());
    calls
}

struct Boundaries {
    today_ms: u64,
    week_ms: u64,
    month_ms: u64,
}

const TRAY_AGENT_NAMES: [&str; 6] = ["claude", "codex", "grok", "kimicode", "opencode", "pi"];
static ENABLED_TRAY_AGENTS: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

fn enabled_tray_agents() -> &'static RwLock<HashSet<String>> {
    ENABLED_TRAY_AGENTS
        .get_or_init(|| RwLock::new(TRAY_AGENT_NAMES.into_iter().map(str::to_owned).collect()))
}

/// Sync the user-facing agent visibility setting into the native tray worker.
/// Grok/Claude/Codex/Kimi/opencode are supported by tray stats; agy is intentionally
/// ignored because it has no usage statistics source.
pub fn set_enabled_agents(agents: &[String]) {
    let allowed: HashSet<&str> = TRAY_AGENT_NAMES.into_iter().collect();
    let next: HashSet<String> = agents
        .iter()
        .map(String::as_str)
        .filter(|agent| allowed.contains(agent))
        .map(str::to_owned)
        .collect();
    if let Ok(mut current) = enabled_tray_agents().write() {
        *current = next;
    }
}

fn is_enabled(agent: &str) -> bool {
    enabled_tray_agents()
        .read()
        .map(|agents| agents.contains(agent))
        .unwrap_or(true)
}

fn compute_boundaries() -> Result<Boundaries, String> {
    let now = Local::now();
    let midnight = Local
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .ok_or_else(|| "failed to resolve local midnight".to_string())?;
    let to_ms = |t: chrono::DateTime<Local>| -> u64 {
        let ts = t.timestamp_millis();
        if ts < 0 {
            0
        } else {
            ts as u64
        }
    };
    // 30d = 过去 30 个日历日（含今天），和 Statistics 页面的 days30 口径一致
    Ok(Boundaries {
        today_ms: to_ms(midnight),
        week_ms: to_ms(midnight - CDuration::days(6)),
        month_ms: to_ms(midnight - CDuration::days(29)),
    })
}

struct AgentAcc {
    today_tokens: u64,
    today_cost: f64,
    today_unpriced_calls: u64,
    today_estimated_calls: u64,
    week_tokens: u64,
    week_cost: f64,
    week_unpriced_calls: u64,
    week_estimated_calls: u64,
    month_tokens: u64,
    month_cost: f64,
    month_unpriced_calls: u64,
    month_estimated_calls: u64,
    session_count: usize,
    seen_ids: HashSet<String>,
}

fn append_agent_summary(result: &mut TrayStats, agent_name: &str, acc: AgentAcc) {
    // Visibility is controlled by the Settings agent toggles. Keep an enabled
    // agent in the tray even when it has no activity in the current 30-day
    // window, otherwise an installed but idle agent is indistinguishable
    // from a disabled one. agy is excluded by TRAY_AGENT_NAMES.
    result.total_today_tokens += acc.today_tokens;
    result.total_today_cost += acc.today_cost;
    result.total_today_unpriced_calls += acc.today_unpriced_calls;
    result.total_today_estimated_calls += acc.today_estimated_calls;
    result.total_week_tokens += acc.week_tokens;
    result.total_week_cost += acc.week_cost;
    result.total_week_unpriced_calls += acc.week_unpriced_calls;
    result.total_week_estimated_calls += acc.week_estimated_calls;
    result.total_month_tokens += acc.month_tokens;
    result.total_month_cost += acc.month_cost;
    result.total_month_unpriced_calls += acc.month_unpriced_calls;
    result.total_month_estimated_calls += acc.month_estimated_calls;
    result.agents.push(TrayAgentSummary {
        agent: agent_name.to_string(),
        today_tokens: acc.today_tokens,
        today_cost: acc.today_cost,
        today_unpriced_calls: acc.today_unpriced_calls,
        today_estimated_calls: acc.today_estimated_calls,
        week_tokens: acc.week_tokens,
        week_cost: acc.week_cost,
        week_unpriced_calls: acc.week_unpriced_calls,
        week_estimated_calls: acc.week_estimated_calls,
        month_tokens: acc.month_tokens,
        month_cost: acc.month_cost,
        month_unpriced_calls: acc.month_unpriced_calls,
        month_estimated_calls: acc.month_estimated_calls,
        session_count: acc.session_count,
    });
}

impl Default for AgentAcc {
    fn default() -> Self {
        Self {
            today_tokens: 0,
            today_cost: 0.0,
            today_unpriced_calls: 0,
            today_estimated_calls: 0,
            week_tokens: 0,
            week_cost: 0.0,
            week_unpriced_calls: 0,
            week_estimated_calls: 0,
            month_tokens: 0,
            month_cost: 0.0,
            month_unpriced_calls: 0,
            month_estimated_calls: 0,
            session_count: 0,
            seen_ids: HashSet::new(),
        }
    }
}

pub fn quick_stats() -> Result<TrayStats, String> {
    let bounds = compute_boundaries()?;
    let earliest = bounds.month_ms.min(bounds.week_ms);

    let agent_names: Vec<&str> = TRAY_AGENT_NAMES
        .into_iter()
        .filter(|agent| is_enabled(agent))
        .collect();
    let mut accs: HashMap<&str, AgentAcc> = HashMap::new();

    for &agent_name in &agent_names {
        let src = match agents::source(agent_name) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let projects = match src.list_projects(false, false) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let acc = accs.entry(agent_name).or_default();
        for p in &projects {
            let sessions = match src.discover_stats_sessions(&p.dir_name) {
                Ok(s) => s,
                Err(_) => continue,
            };
            for s in sessions {
                if s.modified < earliest {
                    continue;
                }
                let mut has_data = false;
                for call in tray_calls(src.as_ref(), &s.path, s.modified).iter() {
                    let ts = call.ts_ms;
                    if ts < earliest {
                        continue;
                    }
                    if let Some(id) = &call.message_id {
                        if !acc.seen_ids.insert(id.clone()) {
                            continue;
                        }
                    }
                    has_data = true;
                    let tokens = call.tokens;
                    let cost = call.cost;
                    let call_weight = call.call_weight;
                    if ts >= bounds.month_ms {
                        acc.month_tokens += tokens;
                        acc.month_cost += cost;
                        if call.pricing_missing {
                            acc.month_unpriced_calls += call_weight;
                        }
                        if call.pricing_estimated {
                            acc.month_estimated_calls += call_weight;
                        }
                    }
                    if ts >= bounds.week_ms {
                        acc.week_tokens += tokens;
                        acc.week_cost += cost;
                        if call.pricing_missing {
                            acc.week_unpriced_calls += call_weight;
                        }
                        if call.pricing_estimated {
                            acc.week_estimated_calls += call_weight;
                        }
                    }
                    if ts >= bounds.today_ms {
                        acc.today_tokens += tokens;
                        acc.today_cost += cost;
                        if call.pricing_missing {
                            acc.today_unpriced_calls += call_weight;
                        }
                        if call.pricing_estimated {
                            acc.today_estimated_calls += call_weight;
                        }
                    }
                }
                if has_data {
                    acc.session_count += 1;
                }
            }
        }
    }

    let mut result = TrayStats::default();
    for &agent_name in &agent_names {
        let acc = accs.remove(agent_name).unwrap_or_default();
        append_agent_summary(&mut result, agent_name, acc);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tray_entry(seq: u64, calls: usize) -> TrayFileEntry {
        let call = TrayCall {
            ts_ms: 1,
            message_id: None,
            tokens: 1,
            cost: 0.0,
            call_weight: 1,
            pricing_missing: false,
            pricing_estimated: false,
        };
        TrayFileEntry {
            mtime: 1,
            seq,
            calls: vec![call; calls].into(),
        }
    }

    fn seed_tray_cache(calls_per_entry: usize, count: u64) -> TrayCallCache {
        let mut cache = TrayCallCache::default();
        for seq in 0..count {
            cache.entries.insert(
                format!("/tmp/tray-{seq}.jsonl"),
                tray_entry(seq, calls_per_entry),
            );
            cache.calls += calls_per_entry;
        }
        cache.next_seq = count;
        cache
    }

    #[test]
    fn evict_tray_calls_is_a_no_op_below_the_cap() {
        let mut cache = seed_tray_cache(10, 4);
        evict_tray_calls(&mut cache);
        assert_eq!(cache.entries.len(), 4);
        assert_eq!(cache.calls, 40);
    }

    #[test]
    fn evict_tray_calls_drops_the_oldest_entries_down_to_three_quarters() {
        // 每条 1/8 上限 => 10 条超限；淘汰到 <= 75% 需要掉到 6 条。
        let per_entry = TRAY_CACHE_MAX_CALLS / 8;
        let mut cache = seed_tray_cache(per_entry, 10);
        evict_tray_calls(&mut cache);

        assert!(cache.calls <= TRAY_CACHE_MAX_CALLS / 4 * 3);
        assert_eq!(cache.entries.len(), 6);
        assert_eq!(cache.calls, 6 * per_entry, "calls 必须与留下的条目对得上");
        for seq in 0..4 {
            assert!(!cache
                .entries
                .contains_key(&format!("/tmp/tray-{seq}.jsonl")));
        }
        for seq in 4..10 {
            assert!(cache
                .entries
                .contains_key(&format!("/tmp/tray-{seq}.jsonl")));
        }
    }

    #[test]
    fn tray_visibility_accepts_supported_agents_and_excludes_agy() {
        set_enabled_agents(&[
            "claude".to_string(),
            "grok".to_string(),
            "agy".to_string(),
            "unknown".to_string(),
        ]);
        assert!(is_enabled("claude"));
        assert!(is_enabled("grok"));
        assert!(!is_enabled("kimicode"));
        assert!(!is_enabled("codex"));
        assert!(!is_enabled("opencode"));
        // agy is not a tray-stat agent even if the frontend sends it.
        assert!(!is_enabled("agy"));

        // Restore the default for other tests and for in-process test runners.
        set_enabled_agents(
            &TRAY_AGENT_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn enabled_opencode_is_retained_when_the_current_window_is_empty() {
        let mut result = TrayStats::default();
        append_agent_summary(&mut result, "opencode", AgentAcc::default());

        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].agent, "opencode");
        assert_eq!(result.agents[0].session_count, 0);
    }

    #[test]
    fn enabled_kimi_is_retained_when_the_current_window_is_empty() {
        let mut result = TrayStats::default();
        append_agent_summary(&mut result, "kimicode", AgentAcc::default());

        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].agent, "kimicode");
        assert_eq!(result.agents[0].session_count, 0);
    }
}
