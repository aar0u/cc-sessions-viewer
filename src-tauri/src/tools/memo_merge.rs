//! 全局指令文件的**合并重复**：同名同内容的好几份，搬一份进主 store，原位全换成链接。
//!
//! 这是 Skills 那边「收编」的同一套路子（`skills_write::adopt`），只是对象从目录变成
//! 单个 Markdown 文件。本机的样子：`~/.claude/RTK.md`、`~/.codex/RTK.md`、
//! `~/.grok/RTK.md` 三份 964 字节、md5 一模一样 —— 改其中任何一份，另外两家还读着旧的，
//! 而且**没有任何地方会告诉你这件事**。
//!
//! 三条和 Skills 那边不同的地方：
//!
//! 1. **只对「内容完全相同」的下手。** 内容分了家的是 [`super::memo::MemoFork`]，
//!    那种只能摆出 diff 让用户自己决定 —— 给 Codex 的那份故意写得更短是合理的。
//!    这里不做「合并时顺手挑一份当赢家」，因为那等于替用户扔掉另外两份的内容。
//! 2. **删之前逐字节复核。** 计划是扫描时算出来的，用户看完计划再点确认，中间隔着
//!    好几秒；真动手时文件可能已经被别的编辑器改过。所以每一条 `Drop` 落地前都要
//!    重读一次并和主 store 那份比对，对不上就整个停下来。
//! 3. **降级只到硬链接。** 目录链接失败还能退到「实体复制 + 标记文件」，单个 md
//!    文件没地方放标记，复制出来的两份和合并前一模一样 —— 那是假装做成了。
//!    见 [`super::link::link_file`]。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::link;
use super::memo;

/// 计划里一步的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoStepKind {
    /// 建主 store 目录。
    EnsureDir,
    /// 把第一份实体搬进主 store。
    Move,
    /// 原位建一条指向主 store 的链接。
    Link,
    /// 拆掉一条指向别处的旧链接（拆链接不丢内容，所以不需要复核）。
    Unlink,
    /// 删掉多余的那几份实体（内容已复核和主 store 那份一致）。
    Drop,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoStep {
    pub kind: MemoStepKind,
    /// 动的是哪个路径。
    pub path: String,
    /// `Move` / `Link` 的另一头。
    pub target: Option<String>,
    /// 已经做完了没有。dry-run 时全是 false。
    pub done: bool,
    /// 这一步属于哪一组重复（文件名）。
    ///
    /// 「合并全部重复」一次能出二十来步，平铺开来是一堵墙 —— 前端按这个字段在组
    /// 之间画分割线。建主 store 目录那一步是多组共用的，归到**第一个用到它的那组**，
    /// 免得冒出一个不属于任何文件的孤儿行。
    pub group: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoMergeReport {
    pub dry_run: bool,
    pub steps: Vec<MemoStep>,
    /// 做不了的那几组，每条一句完整的话。**带原因的那几组一步都没做。**
    pub blocked: Vec<String>,
}

/// 计划里的一步。和 [`MemoStep`] 的区别是这边带着真路径，那边是给前端看的字符串。
#[derive(Debug, Clone)]
enum Op {
    EnsureDir(PathBuf),
    Move { from: PathBuf, to: PathBuf },
    Link { at: PathBuf, target: PathBuf },
    Unlink { path: PathBuf },
    Drop { path: PathBuf, same_as: PathBuf },
}

impl Op {
    fn describe(&self, group: &str) -> MemoStep {
        let (kind, path, target) = match self {
            Op::EnsureDir(p) => (MemoStepKind::EnsureDir, p.clone(), None),
            Op::Move { from, to } => (MemoStepKind::Move, from.clone(), Some(to.clone())),
            Op::Link { at, target } => (MemoStepKind::Link, at.clone(), Some(target.clone())),
            Op::Unlink { path } => (MemoStepKind::Unlink, path.clone(), None),
            Op::Drop { path, .. } => (MemoStepKind::Drop, path.clone(), None),
        };
        MemoStep {
            kind,
            path: path.to_string_lossy().to_string(),
            target: target.map(|t| t.to_string_lossy().to_string()),
            done: false,
            group: group.to_string(),
        }
    }
}

/// 给一组重复算出计划。
///
/// `store` 是主 store 目录（默认 `~/.agents/memo`）。赢家的挑法是**按路径排序取第一个
/// 实体文件** —— 内容都一样，挑谁都行，重要的是这个选择稳定可复现，不能这次是
/// `.claude` 下次是 `.codex`。
fn plan_one(dup: &memo::MemoDup, store: &Path) -> Result<Vec<Op>, String> {
    let dest = store.join(&dup.name);
    let mut bodies: Vec<PathBuf> = dup.bodies.iter().map(PathBuf::from).collect();
    bodies.sort();
    // 一份实体都没有（几个位置全是指向别处的链接），主 store 里也还没有内容 ——
    // 那就没有东西可搬。这时候硬建一圈链接会得到一堆死链。
    if bodies.is_empty() && !dest.is_file() {
        return Err(format!(
            "{}：这几个位置全是指向别处的链接，{} 里也还没有这份内容 —— 没有东西可搬",
            dup.name,
            store.display()
        ));
    }

    let mut ops = Vec::new();
    if !store.is_dir() {
        if store.symlink_metadata().is_ok() {
            return Err(format!("{} 已经存在，但它不是个目录", store.display()));
        }
        ops.push(Op::EnsureDir(store.to_path_buf()));
    }

    // 主 store 里已经有同名的：内容对得上就直接拿它当赢家，对不上就整组不动 ——
    // 覆盖一份来历不明的同名文件是在吃掉用户的东西。
    let winner = if dest.symlink_metadata().is_ok() {
        if !dest.is_file() {
            return Err(format!("{} 已经存在，但它不是个普通文件", dest.display()));
        }
        let there = fs::read(&dest).map_err(|e| format!("读不了 {}：{e}", dest.display()))?;
        // 组里随便一份都行（它们内容相同，这正是「重复」的定义）；实体优先，
        // 一份实体都没有时拿第一个位置读 —— 链接读出来的也是同一份内容。
        let sample = bodies.first().cloned().unwrap_or_else(|| {
            let mut ps: Vec<&String> = dup.paths.iter().collect();
            ps.sort();
            PathBuf::from(ps[0])
        });
        let here = fs::read(&sample).map_err(|e| format!("读不了 {}：{e}", sample.display()))?;
        if there != here {
            return Err(format!(
                "{} 里已经有一份 {}，内容和要合并的这几份不一样",
                store.display(),
                dup.name
            ));
        }
        None
    } else {
        let first = bodies[0].clone();
        ops.push(Op::Move {
            from: first.clone(),
            to: dest.clone(),
        });
        Some(first)
    };

    // 每个位置都要指向主 store 那一份。分三种：
    //
    // - 搬走的那个：原位现在是空的，只建链。
    // - 别的实体：先删（删之前复核内容），再建链。
    // - 已经是链接的：指对了就什么都不做，指别处就拆了重建 —— 拆一条链接不会
    //   丢任何内容，所以不需要复核。
    let mut positions: Vec<PathBuf> = dup.paths.iter().map(PathBuf::from).collect();
    positions.sort();
    for at in &positions {
        if link::is_link(at) {
            if fs::canonicalize(at).ok() == fs::canonicalize(&dest).ok() {
                continue;
            }
            ops.push(Op::Unlink { path: at.clone() });
        } else if winner.as_deref() != Some(at.as_path()) {
            ops.push(Op::Drop {
                path: at.clone(),
                same_as: dest.clone(),
            });
        }
        ops.push(Op::Link {
            at: at.clone(),
            target: dest.clone(),
        });
    }
    Ok(ops)
}

/// 执行一步。
fn run(op: &Op) -> Result<(), String> {
    match op {
        Op::EnsureDir(p) => {
            fs::create_dir_all(p).map_err(|e| format!("建不了目录 {}：{e}", p.display()))
        }
        Op::Move { from, to } => {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("建不了目录：{e}"))?;
            }
            fs::rename(from, to).map_err(|e| {
                format!("搬不动 {} → {}：{e}", from.display(), to.display())
            })
        }
        Op::Link { at, target } => link::link_file(target, at).map(|_| ()),
        Op::Unlink { path } => {
            if !link::is_link(path) {
                return Err(format!(
                    "{} 刚才还是一条链接，现在不是了 —— 这一步没做。",
                    path.display()
                ));
            }
            fs::remove_file(path).map_err(|e| format!("拆不掉 {}：{e}", path.display()))
        }
        Op::Drop { path, same_as } => {
            // 复核：计划是几秒钟前算的，这中间文件可能被别的编辑器改过。
            let mine = fs::read(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
            let theirs =
                fs::read(same_as).map_err(|e| format!("读不了 {}：{e}", same_as.display()))?;
            if mine != theirs {
                return Err(format!(
                    "{} 的内容和 {} 已经不一样了 —— 刚才那一刻还是一样的，说明有别的东西正在改它。这一步没做。",
                    path.display(),
                    same_as.display()
                ));
            }
            fs::remove_file(path).map_err(|e| format!("删不掉 {}：{e}", path.display()))
        }
    }
}

/// 合并若干组重复。
///
/// `names` 是要合并的文件名（`RTK.md` 这种），一次可以给多组 —— 健康条上那个
/// 「合并全部重复」就是把所有组一起交过来，共用一张计划、一次确认。
pub fn merge(names: &[String], store: &Path, dry_run: bool) -> Result<MemoMergeReport, String> {
    if names.is_empty() {
        return Err("没有指定要合并哪一组".into());
    }
    if !store.is_absolute() {
        return Err(format!("主 store 得是绝对路径：{}", store.display()));
    }
    let scan = memo::scan();

    // 每一步都带着它属于哪一组 —— 前端要靠它分段。
    let mut ops: Vec<(String, Op)> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    let mut made_dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    for name in names {
        let Some(dup) = scan.dups.iter().find(|d| &d.name == name) else {
            blocked.push(format!("{name}：现在已经不是重复了，跳过"));
            continue;
        };
        match plan_one(dup, store) {
            Ok(got) => {
                for op in got {
                    // 每组都会要求「主 store 目录得在」，多组一起合时那一步会重复
                    // 出现。做起来无害（`create_dir_all` 幂等），但计划里写着
                    // 「建目录 2」而磁盘上只有一个目录 —— 用户看不懂的计划等于没给。
                    if let Op::EnsureDir(p) = &op {
                        if !made_dirs.insert(p.clone()) {
                            continue;
                        }
                    }
                    ops.push((name.clone(), op));
                }
            }
            Err(why) => blocked.push(why),
        }
    }

    let mut steps: Vec<MemoStep> = ops.iter().map(|(g, op)| op.describe(g)).collect();
    if dry_run {
        return Ok(MemoMergeReport {
            dry_run: true,
            steps,
            blocked,
        });
    }

    for (i, (_, op)) in ops.iter().enumerate() {
        match run(op) {
            Ok(()) => steps[i].done = true,
            Err(why) => {
                // 停在这儿，把做到哪儿了原样报回去 —— 悄悄跳过一步继续往下走，
                // 会留下「链接建了但源还在」这种半拉子状态。
                blocked.push(why);
                break;
            }
        }
    }
    Ok(MemoMergeReport {
        dry_run: false,
        steps,
        blocked,
    })
}

#[tauri::command(async)]
pub fn tools_merge_memo(
    names: Vec<String>,
    store: String,
    dry_run: bool,
) -> Result<MemoMergeReport, String> {
    merge(&names, Path::new(&store), dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cssv_memo_merge_{}_{name}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn dup(name: &str, bodies: &[PathBuf]) -> memo::MemoDup {
        memo::MemoDup {
            name: name.to_string(),
            bytes: 3,
            paths: bodies.iter().map(|p| p.display().to_string()).collect(),
            bodies: bodies.iter().map(|p| p.display().to_string()).collect(),
        }
    }

    fn apply(ops: &[Op]) {
        for op in ops {
            run(op).unwrap();
        }
    }

    #[test]
    fn merging_moves_the_first_body_in_and_links_every_position_at_it() {
        let root = tmp("basic");
        let a = root.join("a/RTK.md");
        let b = root.join("b/RTK.md");
        for p in [&a, &b] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "hi\n").unwrap();
        }
        let store = root.join("store");

        let ops = plan_one(&dup("RTK.md", &[a.clone(), b.clone()]), &store).unwrap();
        apply(&ops);

        // 内容只剩主 store 那一份，两个原位置都是链接，而且读出来还是原文。
        assert!(store.join("RTK.md").is_file());
        assert!(link::is_link(&a) && link::is_link(&b));
        assert_eq!(fs::read_to_string(&a).unwrap(), "hi\n");
        assert_eq!(fs::read_to_string(&b).unwrap(), "hi\n");
    }

    #[test]
    fn a_write_through_any_position_now_reaches_all_of_them() {
        let root = tmp("writethrough");
        let a = root.join("a/RTK.md");
        let b = root.join("b/RTK.md");
        for p in [&a, &b] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "old\n").unwrap();
        }
        let store = root.join("store");
        apply(&plan_one(&dup("RTK.md", &[a.clone(), b.clone()]), &store).unwrap());

        // 这才是合并的意义：从任何一个入口改，另一头立刻就是新的。
        fs::write(&b, "new\n").unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "new\n");
        assert_eq!(
            fs::read_to_string(store.join("RTK.md")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn a_file_already_in_the_store_is_reused_instead_of_moved_over() {
        let root = tmp("reuse");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("RTK.md"), "hi\n").unwrap();
        let a = root.join("a/RTK.md");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::write(&a, "hi\n").unwrap();

        let ops = plan_one(&dup("RTK.md", std::slice::from_ref(&a)), &store).unwrap();
        assert!(!ops.iter().any(|o| matches!(o, Op::Move { .. })));
        apply(&ops);
        assert!(link::is_link(&a));
        assert!(store.join("RTK.md").is_file());
    }

    #[test]
    fn a_different_file_of_the_same_name_in_the_store_blocks_the_whole_group() {
        let root = tmp("occupied");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("RTK.md"), "someone else\n").unwrap();
        let a = root.join("a/RTK.md");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::write(&a, "hi\n").unwrap();

        let err = plan_one(&dup("RTK.md", std::slice::from_ref(&a)), &store).unwrap_err();
        assert!(err.contains("内容和要合并的这几份不一样"), "{err}");
        // 一步都没做：原文件原样还在。
        assert_eq!(fs::read_to_string(&a).unwrap(), "hi\n");
    }

    #[test]
    fn a_body_that_changed_between_planning_and_applying_is_never_deleted() {
        let root = tmp("raced");
        let a = root.join("a/RTK.md");
        let b = root.join("b/RTK.md");
        for p in [&a, &b] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "hi\n").unwrap();
        }
        let store = root.join("store");
        let ops = plan_one(&dup("RTK.md", &[a.clone(), b.clone()]), &store).unwrap();

        // 计划算完之后，别的编辑器改了 b。
        fs::write(&b, "edited behind our back\n").unwrap();

        let mut hit = None;
        for op in &ops {
            if let Err(why) = run(op) {
                hit = Some(why);
                break;
            }
        }
        let why = hit.expect("应该在 Drop 那一步停下来");
        assert!(why.contains("已经不一样了"), "{why}");
        assert_eq!(
            fs::read_to_string(&b).unwrap(),
            "edited behind our back\n",
            "改过的那份必须原封不动"
        );
    }

    #[test]
    fn the_winner_is_picked_by_sorted_path_so_replanning_gives_the_same_answer() {
        let root = tmp("stable");
        let z = root.join("z/RTK.md");
        let a = root.join("a/RTK.md");
        for p in [&z, &a] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "hi\n").unwrap();
        }
        let store = root.join("store");
        // 故意把顺序倒过来喂进去。
        let ops = plan_one(&dup("RTK.md", &[z.clone(), a.clone()]), &store).unwrap();
        let moved = ops
            .iter()
            .find_map(|o| match o {
                Op::Move { from, .. } => Some(from.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(moved, a, "排序后第一个才是赢家");
    }

    /// 多组一起合时，「建主 store 目录」只该出现一次 —— 计划里写「建目录 2」
    /// 而磁盘上只有一个目录，用户会以为自己看错了。
    #[test]
    fn the_store_directory_is_only_planned_once_for_the_whole_batch() {
        let root = tmp("dedupe");
        let store = root.join("store");
        let mut ops = Vec::new();
        let mut made: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
        for name in ["a.md", "b.md"] {
            let one = root.join("x").join(name);
            let two = root.join("y").join(name);
            for p in [&one, &two] {
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, "same\n").unwrap();
            }
            for op in plan_one(&dup(name, &[one, two]), &store).unwrap() {
                if let Op::EnsureDir(p) = &op {
                    if !made.insert(p.clone()) {
                        continue;
                    }
                }
                ops.push(op);
            }
        }
        let dirs = ops
            .iter()
            .filter(|o| matches!(o, Op::EnsureDir(_)))
            .count();
        assert_eq!(dirs, 1);
    }

    /// 多组一起合时，每一步都得说清自己属于哪个文件 —— 前端靠它画分割线，
    /// 少了就是二十行不分段的墙。建目录那一步归第一组，不留孤儿行。
    #[test]
    fn every_step_says_which_file_it_belongs_to() {
        let root = tmp("grouped");
        let store = root.join("store");
        let mut steps: Vec<MemoStep> = Vec::new();
        for name in ["a.md", "b.md"] {
            let one = root.join("x").join(name);
            let two = root.join("y").join(name);
            for p in [&one, &two] {
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, "same\n").unwrap();
            }
            for op in plan_one(&dup(name, &[one, two]), &store).unwrap() {
                steps.push(op.describe(name));
            }
        }
        assert!(steps.iter().all(|s| !s.group.is_empty()));
        let first = steps.first().unwrap();
        assert_eq!(first.kind, MemoStepKind::EnsureDir);
        assert_eq!(first.group, "a.md", "建目录归第一个用到它的那组");
        assert!(steps.iter().any(|s| s.group == "b.md"));
    }

    #[test]
    fn a_dry_run_touches_nothing() {
        let root = tmp("dry");
        let a = root.join("a/RTK.md");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::write(&a, "hi\n").unwrap();
        let store = root.join("store");

        let report = merge(&["definitely-not-there.md".into()], &store, true).unwrap();
        assert!(report.dry_run);
        assert!(report.steps.is_empty());
        assert!(!store.exists());
    }

    #[test]
    fn a_relative_store_path_is_refused() {
        let err = merge(&["RTK.md".into()], Path::new("relative/memo"), true).unwrap_err();
        assert!(err.contains("绝对路径"), "{err}");
    }

    #[test]
    fn asking_for_nothing_is_an_error_not_an_empty_success() {
        let err = merge(&[], Path::new("/tmp/whatever"), true).unwrap_err();
        assert!(err.contains("没有指定"), "{err}");
    }
}
