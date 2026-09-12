//! 逐行文本 diff：LCS 编辑脚本 → 带上下文的 hunk。
//!
//! 从 `skills_write.rs` 提出来的，因为全局配置面板的「分叉并排 diff」要的是同一件事：
//! 两份文本、哪几行不一样。留在那边的话第二个调用方只能照抄一份 LCS —— 两份 DP 回溯
//! 代码迟早在边界条件上分家（空文件、全删、只差末尾换行）。
//!
//! 输出是 `types::DiffHunk`，也就是 `components/DiffBlock.vue` 已经会画的那个形状。

use crate::types::{DiffHunk, DiffLine};

/// 超过这个行数就不 diff 了 —— 全局指令和 SKILL.md 都是人手写的，上千行的多半是别的
/// 东西（日志、压缩过的数据）被放错了地方。
pub const MAX_DIFF_LINES: usize = 800;
/// hunk 前后各留几行上下文。
pub const DIFF_CONTEXT: usize = 3;
/// 所有 hunk 加起来最多这么多行，超出的砍掉并置 `clipped`。
pub const MAX_DIFF_HUNK_LINES: usize = 400;

pub fn diff_line(kind: &str, text: &str, old_no: Option<usize>, new_no: Option<usize>) -> DiffLine {
    DiffLine {
        kind: kind.to_string(),
        old_no: old_no.map(|n| n as u32 + 1),
        new_no: new_no.map(|n| n as u32 + 1),
        text: text.to_string(),
    }
}

/// 逐行 diff：LCS 回溯出完整编辑脚本，再按 [`DIFF_CONTEXT`] 行上下文切成 hunk。
///
/// 和 [`lcs_len`] 分开写，不是重复：那个只要长度，两行滚动 DP 就够；这里要知道**哪几行**
/// 变了，必须把整张表留下来回溯。调用方已经把两边都卡在 [`MAX_DIFF_LINES`] 以内，
/// 最坏情况是 800×800 的 `u32`，2.5MB 左右，够用。
pub fn line_hunks(a: &[&str], b: &[&str]) -> (Vec<DiffHunk>, bool) {
    let (n, m) = (a.len(), b.len());
    let w = m + 1;
    // 从后往前填，回溯时就能从头往后走 —— 编辑脚本天然是正序，不用最后再 reverse。
    let mut dp = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * w + j] = if a[i] == b[j] {
                dp[(i + 1) * w + j + 1] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + j + 1])
            };
        }
    }

    let mut lines: Vec<DiffLine> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            lines.push(diff_line("ctx", a[i], Some(i), Some(j)));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
            lines.push(diff_line("del", a[i], Some(i), None));
            i += 1;
        } else {
            lines.push(diff_line("add", b[j], None, Some(j)));
            j += 1;
        }
    }
    while i < n {
        lines.push(diff_line("del", a[i], Some(i), None));
        i += 1;
    }
    while j < m {
        lines.push(diff_line("add", b[j], None, Some(j)));
        j += 1;
    }
    group_hunks(lines)
}

/// 把编辑脚本按「改动行 ± 上下文」切块。全是 ctx（两边一样）就返回空。
pub fn group_hunks(lines: Vec<DiffLine>) -> (Vec<DiffHunk>, bool) {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (idx, _) in lines.iter().enumerate().filter(|(_, l)| l.kind != "ctx") {
        let lo = idx.saturating_sub(DIFF_CONTEXT);
        let hi = (idx + DIFF_CONTEXT).min(lines.len() - 1);
        match ranges.last_mut() {
            // 上一块的上下文和这一块贴上了就并起来，免得中间夹一条只隔一行的分隔符。
            Some(last) if lo <= last.1 + 1 => last.1 = last.1.max(hi),
            _ => ranges.push((lo, hi)),
        }
    }

    let mut out = Vec::new();
    let mut budget = MAX_DIFF_HUNK_LINES;
    let mut clipped = false;
    for (lo, hi) in ranges {
        if budget == 0 {
            clipped = true;
            break;
        }
        let take = (hi - lo + 1).min(budget);
        if take < hi - lo + 1 {
            clipped = true;
        }
        let slice = &lines[lo..lo + take];
        budget -= take;
        out.push(DiffHunk {
            // 一块里第一条带行号的记作起点；整块全是新增时旧侧就没有行号。
            old_start: slice.iter().find_map(|l| l.old_no).unwrap_or(0),
            new_start: slice.iter().find_map(|l| l.new_no).unwrap_or(0),
            lines: slice.to_vec(),
        });
    }
    (out, clipped)
}

/// 最长公共子序列的**长度**（不需要还原序列，所以只留两行 DP）。
pub fn lcs_len(a: &[&str], b: &[&str]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            cur[j] = if a[i - 1] == b[j - 1] {
                prev[j - 1] + 1
            } else {
                prev[j].max(cur[j - 1])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
        cur.iter_mut().for_each(|v| *v = 0);
    }
    prev[b.len()]
}

