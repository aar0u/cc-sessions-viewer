//! Skills 写操作：收编 / 启停 / 删除 / 修复。
//!
//! 只读那半边在 [`super::skills`]，它量出了问题（三套 store、15 条两跳链、33 个同名
//! 重复、2 条死链）；这个模块负责动手改。四条命令共用三条铁律：
//!
//! 1. **先出计划，再动手。** 每条命令都带 `dry_run`：先返回 [`WriteReport`]，把「改哪条
//!    路径、变成什么、备份在哪」逐条列出来给确认框用；用户点了才用同样的参数真跑一遍。
//!    计划是**整批一次算完**的 —— 参数有问题（主 store 里有个同名文件、要收编的东西
//!    其实是条链接）在动手之前就报错，一个字节都不会被改。
//!
//! 2. **任一步失败就逆序撤销。** 搬走的搬回来、建的链拆掉、拆的链接回去。
//!
//! 3. **不可逆的那一步永远排最后。** 删实体目录撤不回来，所以它一律排在所有可撤销的
//!    操作之后；一旦开始删就是「已提交」，这之后出错不回滚（回滚只会把链接接回一个
//!    已经不存在的目标，比留着现场更糟），而是把「哪些删成了、哪些没删」原样报出来。
//!
//! 删除的顺序也是定死的：**先解链，再删源**。反过来的话链接瞬间全变死链，而这时候
//! 已经没有信息知道该去哪些目录清理 —— 方案文档 1.1 里那 22 条死链就是这么来的。

use super::textdiff::{lcs_len, line_hunks, MAX_DIFF_LINES};
use crate::types::DiffHunk;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::link;
use super::skills;

/// 内容比对时最多看多少个文件 / 递归多深。和 [`super::skills`] 的上限一致 ——
/// 两边用不同的上限，会出现「列表说没扫完、冲突框说一致」这种自相矛盾。
const MAX_FILES: usize = 400;
const MAX_DEPTH: usize = 8;

// ---------------------------------------------------------------------------
// 对外形状
// ---------------------------------------------------------------------------

/// 一步写操作。报告里的每一行就是实际要执行的一个原语，不是「概括」——
/// 确认框里给用户看的必须是真会发生的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    /// 建目录（含父目录）。
    EnsureDir,
    /// 移动目录。
    Move,
    /// 移到临时位置做备份 —— 全部做完才删，中途失败靠它搬回来。
    Backup,
    /// 建链接。
    Link,
    /// 解链（不碰它指向的内容）。
    Unlink,
    /// 删实体目录。**不可逆**，永远排最后。
    DeleteDir,
}

/// 步骤旁边那一句补充说明。
///
/// 走 code 而不是现成的句子：计划框是给用户看的，这里塞一句英文散文过去，
/// 中文 / 日文界面里就会蹦出一行英文 —— 而且是在「要不要执行删除」这种最需要看懂的地方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepNote {
    /// 这条链本来就指着不存在的地方，解链之后没有原样可恢复。
    DeadLink,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteStep {
    pub kind: StepKind,
    pub path: String,
    /// 链接指向谁 / 移动到哪。
    pub target: Option<String>,
    /// 这一步有什么要提醒的。
    pub note: Option<StepNote>,
    /// `dry_run` 恒为 false；真跑时表示这一步做成了。
    pub done: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteReport {
    pub dry_run: bool,
    pub steps: Vec<WriteStep>,
    /// 需要用户三选一的同名冲突。**带冲突的那些条目一步都没做** —— 静默跳过正是
    /// 上游 `import_to_hub` 那个「显示成功但什么都没做」的 bug。
    pub conflicts: Vec<AdoptConflict>,
}

impl WriteReport {
    fn plan(dry_run: bool, ops: &[Op], conflicts: Vec<AdoptConflict>) -> Self {
        WriteReport {
            dry_run,
            steps: ops.iter().map(Op::describe).collect(),
            conflicts,
        }
    }
}

/// 收编一个实体目录的请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptRequest {
    pub name: String,
    /// 要收编的实体目录。必须是真目录 —— 链接和受管副本收编没有意义。
    pub body: String,
    /// 同名冲突怎么办。`None` = 还没选，这一条会原样报回冲突列表。
    pub resolution: Option<Resolution>,
}

/// 同名冲突的三选一。内容一致时不会问，直接按 `KeepMain` 合并。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Resolution {
    /// 保留主 store 的：外部那份删掉，原位改成指向主 store 的链接。
    KeepMain,
    /// 用外部的覆盖：主 store 那份先备份，外部那份移进去，原位留链。
    UseExternal,
    /// 都留着：外部那份以新名字进主 store。
    KeepBoth(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSide {
    pub path: String,
    pub files: usize,
    pub bytes: u64,
    /// 毫秒。
    pub modified: Option<u64>,
    /// 文件数或深度触顶 —— 这一侧没看全，所以**不能**判定两边一致。
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    OnlyMain,
    OnlyExternal,
    Same,
    Different,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    /// 相对 skill 目录。
    pub path: String,
    pub status: FileStatus,
    pub main_bytes: Option<u64>,
    pub external_bytes: Option<u64>,
}

/// 两份 SKILL.md 的逐行差异。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineDiff {
    pub plus: usize,
    pub minus: usize,
    /// 行数超限，没逐行比。`plus` / `minus` 是 0，但**不表示没差异**。
    pub truncated: bool,
    /// 逐行差异本身，带上下文，直接喂 `DiffBlock.vue`。空 = 两边一模一样。
    pub hunks: Vec<DiffHunk>,
    /// `hunks` 被砍到 [`MAX_DIFF_HUNK_LINES`] 行了，后面还有没显示的。
    pub clipped: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptConflict {
    pub name: String,
    pub main: ConflictSide,
    pub external: ConflictSide,
    /// 两边的文件清单合并后逐个比。用户要能看见到底差在哪，不能盲选。
    pub files: Vec<FileDiff>,
    /// 两边 SKILL.md 的增删行数。任一侧没有 SKILL.md 就是 `None`。
    pub skill_md: Option<LineDiff>,
    /// 「都留着」时建议的新名字，已经避开主 store 里已有的名字。
    pub suggested_rename: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOptions {
    /// 只解链，保留实体目录。确认框里那个勾。
    pub keep_bodies: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairRequest {
    /// 要修的那条引用。
    pub path: String,
    pub action: RepairAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum RepairAction {
    /// 改指向这个实体目录 —— 两跳压成一跳、死链重新接上都走它。
    Relink(String),
    /// 直接删掉这条死链。
    Unlink,
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn tools_adopt_skills(
    items: Vec<AdoptRequest>,
    main_store: String,
    dry_run: bool,
) -> Result<WriteReport, String> {
    adopt(&items, Path::new(&main_store), dry_run)
}

#[tauri::command(async)]
pub fn tools_toggle_skill(
    name: String,
    store: String,
    body: Option<String>,
    on: bool,
    dry_run: bool,
) -> Result<WriteReport, String> {
    toggle(
        &name,
        Path::new(&store),
        body.as_deref().map(Path::new),
        on,
        dry_run,
    )
}

#[tauri::command(async)]
pub fn tools_delete_body(
    name: String,
    body: String,
    cwd: Option<String>,
    extra: Vec<String>,
    dry_run: bool,
) -> Result<WriteReport, String> {
    delete_body(
        &name,
        Path::new(&body),
        cwd.as_deref().map(Path::new),
        &extra,
        dry_run,
    )
}

#[tauri::command(async)]
pub fn tools_delete_skill(
    name: String,
    opts: DeleteOptions,
    cwd: Option<String>,
    extra: Vec<String>,
    dry_run: bool,
) -> Result<WriteReport, String> {
    delete(
        &name,
        opts.keep_bodies,
        cwd.as_deref().map(Path::new),
        &extra,
        dry_run,
    )
}

/// 拆掉一条**没人读**的链接（方案 2.4）。
///
/// 和「修链」里的清理死链不是一回事：那条只碰解析不到东西的死链，而这里拆的是一条
/// 好端端指着实体的活链接 —— 它只是没有任何 agent 在扫它所在的目录，于是既不占用，
/// 也没有任何入口能把它拿掉（「停用」是按 agent 关的，而这条压根不属于任何一家）。
/// 本机换完主 store 之后，`~/.skills-manager/skills/X` 和 `~/.cc-switch/skills/X`
/// 就是这种：两条指向新家的链接，谁也不读，也删不掉。
#[tauri::command(async)]
pub fn tools_unlink_ref(path: String, dry_run: bool) -> Result<WriteReport, String> {
    unlink_ref(Path::new(&path), dry_run)
}

#[tauri::command(async)]
pub fn tools_repair_links(
    items: Vec<RepairRequest>,
    dry_run: bool,
) -> Result<WriteReport, String> {
    repair(&items, dry_run)
}

/// 把一份受管副本按它的源重拷一遍（方案 4.4 / 验收 #8）。
///
/// 没有 dry-run：这个操作的「计划」只有一步，而且它做什么完全由那份副本自己的状态决定
/// —— 前端在详情里已经把「源变了」和两边各在哪儿摊开了，再套一层计划框只是多点一次。
/// 会吃掉数据的那几种状态（副本被就地改过 / 两边都变了）在 [`link::resync_copy`] 里
/// 直接拒绝，不靠 UI 记得禁用按钮。
#[tauri::command(async)]
pub fn tools_resync_copy(path: String) -> Result<(), String> {
    link::resync_copy(Path::new(&path)).map(|_| ())
}

// ---------------------------------------------------------------------------
// 收编
// ---------------------------------------------------------------------------

/// 把散落的实体目录搬进主 store，原位留一条指向它的链接 —— 物理上只留一份。
pub fn adopt(
    items: &[AdoptRequest],
    main_store: &Path,
    dry_run: bool,
) -> Result<WriteReport, String> {
    if main_store.exists() && !main_store.is_dir() {
        return Err(format!(
            "Main store is not a directory: {}",
            main_store.display()
        ));
    }

    let mut ops: Vec<Op> = Vec::new();
    let mut conflicts: Vec<AdoptConflict> = Vec::new();
    // 计划阶段就要知道主 store 里将会有哪些名字：同一批里两条都选「都留着」时，
    // 建议的新名字不能撞在一起。
    let mut taken = existing_names(main_store);

    if !main_store.exists() {
        ops.push(Op::EnsureDir {
            path: main_store.to_path_buf(),
        });
    }

    for item in items {
        let body = PathBuf::from(&item.body);
        validate_body(&body)?;
        let dest = main_store.join(&item.name);

        // 主 store 的那个位置是一条**指向这份实体**的链接：把两边对调 —— 拆掉链接、
        // 把实体搬进来、原位留一条链接。物理上还是只有一份，但这一份终于落在主 store
        // 里了（验收 #7：结束后主 store 之外不再有实体 skill 目录）。
        //
        // 这一支必须排在下面那条「已经在主 store 里」之前。`same_path` 把两边都
        // canonicalize，而链接解析到底就是 `body` —— 不单独认出来的话，收编会当场
        // 静默跳过，UI 报成功而实体一步没挪。本机换主 store 之后就是这一幕：
        // `~/.agents/skills/hyperframes` → `~/.skills-manager/skills/hyperframes`，
        // 点「搬进主 store」什么都不发生。
        //
        // 也不能让它落到下面 `compare` 那条路上去：内容当然逐字节一样（本来就是同一个
        // 目录），`identical` 会成立，而 `link_over` 是「删掉外部那份、原位改成链接」
        // —— 实体没了，只剩两条指向空处的链接。
        if link::is_link(&dest) && link::same_path(&dest, &body) {
            ops.push(Op::Unlink {
                at: dest.clone(),
                expected: Some(body.clone()),
            });
            ops.push(Op::MoveDir {
                from: body.clone(),
                to: dest.clone(),
                backup: false,
            });
            ops.push(Op::Link {
                at: body,
                target: dest,
            });
            taken.push(item.name.clone());
            continue;
        }

        if link::same_path(&body, &dest) {
            // 已经在主 store 里了，不是收编的对象。（主 store 目录自己是条链接时也
            // 走这儿：`dest` 本身不是链接，实体确实已经在里面。）
            continue;
        }

        if !dest.exists() {
            ops.push(Op::MoveDir {
                from: body.clone(),
                to: dest.clone(),
                backup: false,
            });
            ops.push(Op::Link {
                at: body,
                target: dest,
            });
            taken.push(item.name.clone());
            continue;
        }

        if !dest.is_dir() {
            return Err(format!(
                "Main store already has a non-directory at {}",
                dest.display()
            ));
        }

        let (identical, conflict) = compare(&item.name, &dest, &body, &taken)?;
        if identical {
            // 内容一样，没什么好问的：删掉外部那份，原位改成链接。
            ops.extend(link_over(&body, &dest));
            continue;
        }

        let Some(resolution) = item.resolution.clone() else {
            conflicts.push(conflict);
            continue;
        };

        match resolution {
            Resolution::KeepMain => ops.extend(link_over(&body, &dest)),
            Resolution::UseExternal => {
                // 先把主 store 那份挪到备份位，成功之后才删 —— 中途失败能整个搬回来。
                let backup = backup_path(&dest);
                ops.push(Op::MoveDir {
                    from: dest.clone(),
                    to: backup.clone(),
                    backup: true,
                });
                ops.push(Op::MoveDir {
                    from: body.clone(),
                    to: dest.clone(),
                    backup: false,
                });
                ops.push(Op::Link {
                    at: body,
                    target: dest,
                });
                ops.push(Op::DeleteDir { path: backup });
            }
            Resolution::KeepBoth(new_name) => {
                let renamed = main_store.join(&new_name);
                if renamed.exists() {
                    return Err(format!("Main store already has {}", renamed.display()));
                }
                ops.push(Op::MoveDir {
                    from: body.clone(),
                    to: renamed.clone(),
                    backup: false,
                });
                ops.push(Op::Link {
                    at: body,
                    target: renamed,
                });
                taken.push(new_name);
            }
        }
    }

    run(ops, conflicts, dry_run)
}

/// 原位那份删掉、改成指向 `dest` 的链接。
///
/// 不是「先删再建链」：删完建链失败的话，外部那份内容就真没了（用户选「保留主 store」
/// 时两边内容是不一样的，那不是副本，是数据）。所以先搬到备份位，链建成了才删。
fn link_over(body: &Path, dest: &Path) -> Vec<Op> {
    let backup = backup_path(body);
    vec![
        Op::MoveDir {
            from: body.to_path_buf(),
            to: backup.clone(),
            backup: true,
        },
        Op::Link {
            at: body.to_path_buf(),
            target: dest.to_path_buf(),
        },
        Op::DeleteDir { path: backup },
    ]
}

fn validate_body(body: &Path) -> Result<(), String> {
    if !body.exists() {
        return Err(format!("No such directory: {}", body.display()));
    }
    if link::is_link(body) {
        return Err(format!(
            "{} is a link, not real content — nothing to adopt",
            body.display()
        ));
    }
    if link::copy_original(body).is_some() {
        return Err(format!(
            "{} is a managed copy, not real content — nothing to adopt",
            body.display()
        ));
    }
    if !body.is_dir() {
        return Err(format!("Not a directory: {}", body.display()));
    }
    Ok(())
}

fn existing_names(store: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(store) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

/// 备份位：同一个父目录下的隐藏名字，跟着被操作对象走。
///
/// 同父目录是刻意的 —— 跨卷的 rename 会退化成整棵树复制，备份就失去「瞬间、可撤销」
/// 的意义了。
fn backup_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "skill".to_string());
    let mut candidate = parent.join(format!(".{stem}.session-viewer-backup"));
    let mut n = 2;
    while candidate.exists() {
        candidate = parent.join(format!(".{stem}.session-viewer-backup-{n}"));
        n += 1;
    }
    candidate
}

// ---------------------------------------------------------------------------
// 启停
// ---------------------------------------------------------------------------

/// 在某个 store 里建 / 拆一条链接。**不动实体内容** —— 停用一个 skill 永远不该删东西。
pub fn toggle(
    name: &str,
    store: &Path,
    body: Option<&Path>,
    on: bool,
    dry_run: bool,
) -> Result<WriteReport, String> {
    let at = store.join(name);
    let mut ops: Vec<Op> = Vec::new();

    if on {
        let body = body.ok_or_else(|| "Enabling a skill needs the body to link at".to_string())?;
        validate_body(body)?;
        if at.exists() || link::is_link(&at) {
            // 已经有东西占着这个位置。指向同一份内容就是「已经启用了」。
            let (_, resolved) = link::resolve_chain(&at, 16);
            if resolved.is_some_and(|r| link::same_path(&r, body)) {
                return Ok(WriteReport::plan(dry_run, &[], Vec::new()));
            }
            return Err(format!(
                "{} already exists and points somewhere else",
                at.display()
            ));
        }
        if !store.exists() {
            ops.push(Op::EnsureDir {
                path: store.to_path_buf(),
            });
        }
        ops.push(Op::Link {
            at,
            target: body.to_path_buf(),
        });
    } else {
        if !at.exists() && !link::is_link(&at) {
            // 本来就没有 —— 停用一个没启用的 skill 不是错误。
            return Ok(WriteReport::plan(dry_run, &[], Vec::new()));
        }
        if !link::is_link(&at) && link::copy_original(&at).is_none() {
            return Err(format!(
                "{} is real content, not a link — disabling must never delete content",
                at.display()
            ));
        }
        let (_, resolved) = link::resolve_chain(&at, 16);
        ops.push(Op::Unlink {
            at,
            expected: resolved,
        });
    }

    run(ops, Vec::new(), dry_run)
}

// ---------------------------------------------------------------------------
// 删除
// ---------------------------------------------------------------------------

/// 按反向索引全量解链，再删实体目录。
///
/// 反向索引来自全盘扫描，不是我们自己的配置列表 —— 所以别的工具建的链接
/// （`~/.agents`、`~/.cc-switch` 那两条）也一起清掉。上游 Skills-Manager 只遍历自己
/// 认得的工具，那正是本机留下 22 条死链的原因。
pub fn delete(
    name: &str,
    keep_bodies: bool,
    cwd: Option<&Path>,
    extra: &[String],
    dry_run: bool,
) -> Result<WriteReport, String> {
    // 用户自己加的目录也要进这张反向索引 —— 漏了它，删除会在那儿留一条死链，
    // 而这个面板的存在理由之一就是清死链。
    let scan = skills::scan(cwd, extra);
    let entry = scan
        .skills
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("No such skill: {name}"))?;

    let refs: Vec<String> = entry.refs.iter().map(|r| r.path.clone()).collect();
    let bodies: Vec<String> = entry.bodies.iter().map(|b| b.path.clone()).collect();
    run(plan_delete(&refs, &bodies, keep_bodies)?, Vec::new(), dry_run)
}

/// 删掉**一份**实体内容，以及所有解析到它的引用。
///
/// 和 [`delete`] 的区别是范围：那个清掉整个 skill（所有 body + 所有 ref），这个只清指定
/// 的那一份。详情页「引用」里那几条标着「没有 agent 读它」的实体目录就是靠它收拾的 ——
/// 它们大多是别的管理器留下的孤儿副本。
///
/// **不能只删目录。** 「没有 agent 读它」说的是「没有哪家 agent 直接扫这个目录」，不是
/// 「没人指着它」—— 本机最常见的形状恰恰是 `~/.agents/skills/X` → `~/.skills-manager/skills/X`，
/// 那个 X 正是一条「没有 agent 读它」的实体目录。只删目录就是亲手造一条死链，而清死链
/// 正是这个面板存在的理由。所以指向它的每一条链接都要先解掉。
pub fn delete_body(
    name: &str,
    body: &Path,
    cwd: Option<&Path>,
    extra: &[String],
    dry_run: bool,
) -> Result<WriteReport, String> {
    let scan = skills::scan(cwd, extra);
    let entry = scan
        .skills
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("No such skill: {name}"))?;

    // 只删这个 skill 自己的内容。不校验的话，前端传什么路径就删什么路径。
    if !entry
        .bodies
        .iter()
        .any(|b| link::same_path(Path::new(&b.path), body))
    {
        return Err(format!(
            "{} is not content of {name} — refusing to delete it",
            body.display()
        ));
    }

    let refs: Vec<String> = entry.refs.iter().map(|r| r.path.clone()).collect();
    run(plan_delete_body(&refs, body)?, Vec::new(), dry_run)
}

/// 删一份内容的计划：先解掉落在它身上的每一条链接，再删目录。
///
/// 单独拎出来是为了能测「解了哪些、没解哪些」—— 指向**别处**的引用和这次删除无关，
/// 顺手解掉就等于把用户没要求删的东西也拆了。
fn plan_delete_body(refs: &[String], body: &Path) -> Result<Vec<Op>, String> {
    let mut ops: Vec<Op> = Vec::new();
    for r in refs {
        let path = PathBuf::from(r);
        // 和 [`plan_delete`] 一样：**先问「它是不是链接」**，绝不先用 `same_path` 把
        // 「body 自己」挑出去 —— `same_path` 会 canonicalize，于是每一条指向这份内容的
        // symlink 都跟 body「相等」而被跳过，一条都解不掉，直接删源，造出来的正是这个
        // 面板要清理的死链。
        //
        // 实体目录落在下面这个 `None` 分支里：别人家的内容不碰，这份自己留到最后那步删。
        let target = if link::is_link(&path) {
            // 解析**到底**，不是第一跳：两跳链的第一跳指着中间那条链接，可它最终
            // 落在这份内容上，删完照样是死链。
            link::resolve_chain(&path, 16).1
        } else {
            // 受管副本：marker 里记着它当初拷自哪份内容。
            link::copy_original(&path)
        };
        // 死链跟这次删除无关（它现在就指不到任何东西），交给健康修复。
        let Some(target) = target else { continue };
        if !link::same_path(&target, body) {
            continue;
        }
        ops.push(Op::Unlink {
            at: path,
            expected: Some(target),
        });
    }
    // 不可逆，永远最后。
    ops.push(Op::DeleteDir {
        path: body.to_path_buf(),
    });
    Ok(ops)
}

/// 删除的计划：先把每一条引用解链，再删实体目录。
///
/// 单独拎出来是为了能测 —— [`delete`] 那一半是全盘扫描，而这里才是「顺序对不对、
/// 会不会碰到不该碰的东西」的判断。
fn plan_delete(refs: &[String], bodies: &[String], keep_bodies: bool) -> Result<Vec<Op>, String> {
    let bodies: Vec<PathBuf> = bodies.iter().map(PathBuf::from).collect();
    let mut ops: Vec<Op> = Vec::new();

    // 先解链。实体目录不在这一轮 —— 它们是 refs 里 `RealDir` 的那些，等下一轮删。
    //
    // 判断顺序不能反：**先问「它是不是链接」，再问「它是不是 body」**。反过来用
    // `same_path` 去比的话，一条指向 body 的链接会被解析到 body 本身、当成「它就是
    // body」而跳过 —— 于是链一条都没解，直接删源，制造出正要清理的那种死链。
    for r in refs {
        let path = PathBuf::from(r);
        if link::is_link(&path) || link::copy_original(&path).is_some() {
            let (_, resolved) = link::resolve_chain(&path, 16);
            ops.push(Op::Unlink {
                at: path,
                expected: resolved,
            });
            continue;
        }
        if bodies.iter().any(|b| link::same_path(b, &path)) {
            continue;
        }
        // 既不是链接也不是受管副本、又不在 bodies 里：扫描之后被人改过。
        // 与其猜，不如停下来让用户看一眼。
        return Err(format!(
            "{} is neither a link nor known content — refusing to touch it",
            path.display()
        ));
    }

    // 再删源。不可逆，所以永远在最后。
    if !keep_bodies {
        for body in bodies {
            ops.push(Op::DeleteDir { path: body });
        }
    }

    Ok(ops)
}

// ---------------------------------------------------------------------------
// 拆一条没人读的链接
// ---------------------------------------------------------------------------

/// 唯一的闸是「它必须是链接或受管副本」。
///
/// 「没有 agent 读它」那一半判不了 —— 谁在读是全盘扫描的结论，而这儿只拿到一个路径。
/// 但那一半也不会吃掉数据：万一拆错了，实体一个字节没动，重新启用就接回来了。会吃掉
/// 数据的只有「把实体当链接拆了」，所以闸立在这儿。
fn unlink_ref(path: &Path, dry_run: bool) -> Result<WriteReport, String> {
    if !link::is_link(path) && link::copy_original(path).is_none() {
        return Err(format!(
            "{} is not a link — refusing to remove real content",
            path.display()
        ));
    }
    let (_, resolved) = link::resolve_chain(path, 16);
    run(
        vec![Op::Unlink {
            at: path.to_path_buf(),
            expected: resolved,
        }],
        Vec::new(),
        dry_run,
    )
}

// ---------------------------------------------------------------------------
// 修复
// ---------------------------------------------------------------------------

/// 批量修链：改指向 / 删死链。
pub fn repair(items: &[RepairRequest], dry_run: bool) -> Result<WriteReport, String> {
    let mut ops: Vec<Op> = Vec::new();

    for item in items {
        let path = PathBuf::from(&item.path);
        if !link::is_link(&path) && link::copy_original(&path).is_none() {
            return Err(format!(
                "{} is not a link — repair only touches links",
                path.display()
            ));
        }
        let (hops, resolved) = link::resolve_chain(&path, 16);
        match &item.action {
            RepairAction::Unlink => {
                if resolved.is_some() {
                    return Err(format!(
                        "{} still resolves to something — use relink instead of deleting it",
                        path.display()
                    ));
                }
                ops.push(Op::Unlink {
                    at: path,
                    expected: None,
                });
            }
            RepairAction::Relink(target) => {
                let target = PathBuf::from(target);
                validate_body(&target)?;
                // 只有「已经是一跳、且就指着它」才算不用改。**不能只看 resolved** ——
                // 两跳链解析到底也是这个目标，那正是要压平的对象，不是「已经好了」。
                if hops.len() <= 1 && resolved.as_ref().is_some_and(|r| link::same_path(r, &target))
                {
                    continue;
                }
                ops.push(Op::Unlink {
                    at: path.clone(),
                    expected: resolved,
                });
                ops.push(Op::Link { at: path, target });
            }
        }
    }

    run(ops, Vec::new(), dry_run)
}

// ---------------------------------------------------------------------------
// 原语与事务
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    EnsureDir {
        path: PathBuf,
    },
    MoveDir {
        from: PathBuf,
        to: PathBuf,
        /// 只影响报告里的措辞：备份是「临时挪开」，不是最终位置。
        backup: bool,
    },
    Link {
        at: PathBuf,
        target: PathBuf,
    },
    Unlink {
        at: PathBuf,
        /// 这条链现在解析到哪。`None` = 死链（解析不到东西），撤销时接不回来 ——
        /// 但删掉一条死链本来也不会失去任何东西。
        expected: Option<PathBuf>,
    },
    DeleteDir {
        path: PathBuf,
    },
}

impl Op {
    fn describe(&self) -> WriteStep {
        match self {
            Op::EnsureDir { path } => WriteStep {
                kind: StepKind::EnsureDir,
                path: path.to_string_lossy().to_string(),
                target: None,
                note: None,
                done: false,
            },
            Op::MoveDir { from, to, backup } => WriteStep {
                kind: if *backup {
                    StepKind::Backup
                } else {
                    StepKind::Move
                },
                path: from.to_string_lossy().to_string(),
                target: Some(to.to_string_lossy().to_string()),
                note: None,
                done: false,
            },
            Op::Link { at, target } => WriteStep {
                kind: StepKind::Link,
                path: at.to_string_lossy().to_string(),
                target: Some(target.to_string_lossy().to_string()),
                note: None,
                done: false,
            },
            Op::Unlink { at, expected } => WriteStep {
                kind: StepKind::Unlink,
                path: at.to_string_lossy().to_string(),
                target: expected.as_ref().map(|p| p.to_string_lossy().to_string()),
                note: expected.is_none().then_some(StepNote::DeadLink),
                done: false,
            },
            Op::DeleteDir { path } => WriteStep {
                kind: StepKind::DeleteDir,
                path: path.to_string_lossy().to_string(),
                target: None,
                note: None,
                done: false,
            },
        }
    }

    fn is_delete(&self) -> bool {
        matches!(self, Op::DeleteDir { .. })
    }
}

/// 撤销一步已经做完的操作。
#[derive(Debug)]
enum Undo {
    MoveBack { from: PathBuf, to: PathBuf },
    RemoveLink { at: PathBuf, target: PathBuf },
    Relink { at: PathBuf, target: PathBuf },
    RemoveDir { path: PathBuf },
}

/// 执行（或只是计划）一批操作。
///
/// 删目录那几步被拎到最后单独跑：它们撤不回来，所以必须等所有可撤销的操作都成功之后
/// 才开始；一旦开始就是已提交，这之后出错只报告，不回滚。
fn run(
    ops: Vec<Op>,
    conflicts: Vec<AdoptConflict>,
    dry_run: bool,
) -> Result<WriteReport, String> {
    let (deletes, reversible): (Vec<Op>, Vec<Op>) = ops.into_iter().partition(Op::is_delete);
    let ordered: Vec<Op> = reversible.into_iter().chain(deletes).collect();

    if dry_run {
        return Ok(WriteReport::plan(true, &ordered, conflicts));
    }

    let mut steps: Vec<WriteStep> = ordered.iter().map(Op::describe).collect();
    let mut undos: Vec<Undo> = Vec::new();

    for (i, op) in ordered.iter().enumerate() {
        let outcome = apply(op);
        match outcome {
            Ok(Some(undo)) => {
                undos.push(undo);
                steps[i].done = true;
            }
            Ok(None) => steps[i].done = true,
            Err(e) => {
                if op.is_delete() {
                    // 已提交区。前面的删除可能已经成功，回滚只会把链接接回一个不存在的
                    // 目标 —— 比留着现场更糟。原样报出来。
                    let done: Vec<String> = steps
                        .iter()
                        .filter(|s| s.done)
                        .map(|s| s.path.clone())
                        .collect();
                    return Err(format!(
                        "{e}\nAlready applied and NOT rolled back: {}",
                        done.join(", ")
                    ));
                }
                let failures = rollback(undos);
                if failures.is_empty() {
                    return Err(format!("{e}\nRolled back, nothing changed."));
                }
                return Err(format!(
                    "{e}\nRollback also failed — the tree is in a mixed state: {}",
                    failures.join("; ")
                ));
            }
        }
    }

    Ok(WriteReport {
        dry_run: false,
        steps,
        conflicts,
    })
}

fn apply(op: &Op) -> Result<Option<Undo>, String> {
    match op {
        Op::EnsureDir { path } => {
            if path.is_dir() {
                return Ok(None);
            }
            fs::create_dir_all(path)
                .map_err(|e| format!("Failed to create {}: {e}", path.display()))?;
            Ok(Some(Undo::RemoveDir { path: path.clone() }))
        }
        Op::MoveDir { from, to, .. } => {
            move_dir(from, to)?;
            Ok(Some(Undo::MoveBack {
                from: to.clone(),
                to: from.clone(),
            }))
        }
        Op::Link { at, target } => {
            link::link_dir(target, at)?;
            Ok(Some(Undo::RemoveLink {
                at: at.clone(),
                target: target.clone(),
            }))
        }
        Op::Unlink { at, expected } => {
            let intent = match expected {
                Some(t) => link::RemovalIntent::PointingAt(t),
                None => link::RemovalIntent::BrokenOnly,
            };
            link::remove_link(at, intent)?;
            Ok(expected.as_ref().map(|t| Undo::Relink {
                at: at.clone(),
                target: t.clone(),
            }))
        }
        Op::DeleteDir { path } => {
            if !path.exists() {
                return Ok(None);
            }
            // 只删实体目录。链接走 Unlink —— remove_dir_all 顺着 reparse point
            // 会把源目录里的东西删干净。
            if link::is_link(path) {
                return Err(format!(
                    "Refusing to delete {}: it is a link, not content",
                    path.display()
                ));
            }
            fs::remove_dir_all(path)
                .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
            Ok(None)
        }
    }
}

/// 逆序撤销。返回撤不回来的那些（正常情况是空的）。
fn rollback(undos: Vec<Undo>) -> Vec<String> {
    let mut failures = Vec::new();
    for undo in undos.into_iter().rev() {
        let result = match &undo {
            Undo::MoveBack { from, to } => move_dir(from, to),
            Undo::RemoveLink { at, target } => {
                link::remove_link(at, link::RemovalIntent::PointingAt(target))
            }
            Undo::Relink { at, target } => link::link_dir(target, at).map(|_| ()),
            Undo::RemoveDir { path } => {
                fs::remove_dir(path).map_err(|e| format!("Failed to remove {}: {e}", path.display()))
            }
        };
        if let Err(e) = result {
            failures.push(format!("{undo:?}: {e}"));
        }
    }
    failures
}

/// 移动一个目录，跨卷时退化成「整棵复制 + 删源」。
///
/// 复制成功但删源失败要把复制出来的那份清掉，否则同一份内容会在两个地方各留一份 ——
/// 收编要解决的正是这个问题。
fn move_dir(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() || link::is_link(to) {
        return Err(format!("Move target already exists: {}", to.display()));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    link::copy_tree_rejecting_links(from, to)?;
    if let Err(e) = fs::remove_dir_all(from) {
        let _ = fs::remove_dir_all(to);
        return Err(format!(
            "Copied {} but could not remove the source: {e}",
            from.display()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 内容比对
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSig {
    bytes: u64,
    hash: u64,
}

/// 走一遍目录，拿到「相对路径 → (大小, 内容哈希)」。
///
/// 用内容哈希而不是 [`link::fingerprint`]：那个指纹里带 mtime，而两份内容完全相同的
/// 副本只要不是同一时刻产生的，指纹就不一样 —— 拿它判「内容一致」会把 33 个重复条目
/// 全部打成冲突，用户得逐个手工看。
fn content_map(dir: &Path) -> (BTreeMap<String, FileSig>, bool) {
    let mut out = BTreeMap::new();
    let mut truncated = false;
    walk_content(dir, dir, 0, &mut out, &mut truncated);
    (out, truncated)
}

fn walk_content(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut BTreeMap<String, FileSig>,
    truncated: &mut bool,
) {
    if depth > MAX_DEPTH {
        *truncated = true;
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        *truncated = true;
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_FILES {
            *truncated = true;
            return;
        }
        let path = entry.path();
        let name = entry.file_name();
        // `.git` 不是 skill 的内容（本机好几个 skill 自己就是 git 仓库）。
        if name == ".git" {
            continue;
        }
        let Ok(meta) = path.symlink_metadata() else {
            *truncated = true;
            continue;
        };
        if meta.file_type().is_symlink() {
            // 嵌套链接不跟进 —— 跟进去就走出这个 skill 了。
            *truncated = true;
            continue;
        }
        if meta.is_dir() {
            walk_content(root, &path, depth + 1, out, truncated);
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        match fs::read(&path) {
            Ok(bytes) => {
                out.insert(
                    rel,
                    FileSig {
                        bytes: meta.len(),
                        hash: link::fnv1a64(&bytes),
                    },
                );
            }
            Err(_) => *truncated = true,
        }
    }
}

/// 两边一样吗，以及不一样的话差在哪。
fn compare(
    name: &str,
    main: &Path,
    external: &Path,
    taken: &[String],
) -> Result<(bool, AdoptConflict), String> {
    let (main_map, main_truncated) = content_map(main);
    let (ext_map, ext_truncated) = content_map(external);
    // 任一侧没看全就不能判「一致」—— 差异可能正好在没看到的那部分里。
    let identical = !main_truncated && !ext_truncated && main_map == ext_map;

    let mut files: Vec<FileDiff> = Vec::new();
    let mut names: Vec<&String> = main_map.keys().chain(ext_map.keys()).collect();
    names.sort();
    names.dedup();
    for rel in names {
        let a = main_map.get(rel);
        let b = ext_map.get(rel);
        let status = match (a, b) {
            (Some(_), None) => FileStatus::OnlyMain,
            (None, Some(_)) => FileStatus::OnlyExternal,
            (Some(x), Some(y)) if x == y => FileStatus::Same,
            _ => FileStatus::Different,
        };
        files.push(FileDiff {
            path: rel.clone(),
            status,
            main_bytes: a.map(|s| s.bytes),
            external_bytes: b.map(|s| s.bytes),
        });
    }

    Ok((
        identical,
        AdoptConflict {
            name: name.to_string(),
            main: side(main, main_map.len(), &main_map, main_truncated),
            external: side(external, ext_map.len(), &ext_map, ext_truncated),
            files,
            skill_md: skill_md_diff(main, external),
            suggested_rename: suggest_rename(name, external, taken),
        },
    ))
}

fn side(path: &Path, files: usize, map: &BTreeMap<String, FileSig>, truncated: bool) -> ConflictSide {
    ConflictSide {
        path: path.to_string_lossy().to_string(),
        files,
        bytes: map.values().map(|s| s.bytes).sum(),
        modified: fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64),
        truncated,
    }
}

/// 建议的新名字：`<name>-from-<外部 store 的名字>`，撞名了再加序号。
fn suggest_rename(name: &str, external: &Path, taken: &[String]) -> String {
    let store = external
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "external".to_string());
    // `~/.cc-switch/skills` → 取「cc-switch」这一层更像人话，而不是所有 store 都叫 skills。
    let tag = if store == "skills" {
        external
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().trim_start_matches('.').to_string())
            .unwrap_or(store)
    } else {
        store
    };
    let base = format!("{name}-from-{tag}");
    if !taken.iter().any(|t| t == &base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !taken.iter().any(|t| t == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// 两份 SKILL.md 的增删行数。逐行 LCS，超过 [`MAX_DIFF_LINES`] 就不算。
fn skill_md_diff(main: &Path, external: &Path) -> Option<LineDiff> {
    let a = fs::read_to_string(main.join("SKILL.md")).ok()?;
    let b = fs::read_to_string(external.join("SKILL.md")).ok()?;
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        return Some(LineDiff {
            plus: 0,
            minus: 0,
            truncated: true,
            hunks: Vec::new(),
            clipped: false,
        });
    }
    let common = lcs_len(&a, &b);
    let (hunks, clipped) = line_hunks(&a, &b);
    Some(LineDiff {
        plus: b.len() - common,
        minus: a.len() - common,
        truncated: false,
        hunks,
        clipped,
    })
}

#[cfg(test)]
mod tests {
    use super::super::textdiff::MAX_DIFF_HUNK_LINES;
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_root(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "csv-skills-write-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 造一个 skill 目录，SKILL.md 内容由调用方给。
    fn skill(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("SKILL.md"), body).unwrap();
        path
    }

    fn req(name: &str, body: &Path, resolution: Option<Resolution>) -> AdoptRequest {
        AdoptRequest {
            name: name.to_string(),
            body: body.to_string_lossy().to_string(),
            resolution,
        }
    }

    #[test]
    fn adopting_moves_the_body_into_the_main_store_and_leaves_a_link_behind() {
        let root = temp_root("adopt");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let external = root.join("external");
        fs::create_dir_all(&external).unwrap();
        let body = skill(&external, "pinme", "---\nname: pinme\n---\n");

        let report = adopt(&[req("pinme", &body, None)], &main, false).unwrap();

        assert!(report.conflicts.is_empty());
        assert!(report.steps.iter().all(|s| s.done));
        // 内容进了主 store，原位只剩一条指过来的链接 —— 物理上仍然只有一份。
        assert!(main.join("pinme").join("SKILL.md").is_file());
        assert!(link::is_link(&body));
        let (_, resolved) = link::resolve_chain(&body, 16);
        assert!(link::same_path(&resolved.unwrap(), &main.join("pinme")));
    }

    /// 换了主 store 之后的那一幕：新主 store 里已经有一条**指向这份实体**的链接。
    ///
    /// 本机实测（`~/.agents/skills` 换成主 store 之后点「搬进主 store」）：
    /// `~/.agents/skills/hyperframes` 是一条指向 `~/.skills-manager/skills/hyperframes`
    /// 的 symlink。`same_path` 把两边都 canonicalize，于是这条链接被判成「实体已经
    /// 在主 store 里了」—— 收编静默跳过，UI 报成功，而实体一步没挪，验收 #7 要求的
    /// 「结束后主 store 之外不再有实体 skill 目录」根本没达成。
    #[test]
    fn a_link_sitting_in_the_main_store_gets_swapped_with_the_body_it_points_at() {
        let root = temp_root("adopt-link");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let external = root.join("external");
        fs::create_dir_all(&external).unwrap();
        let body = skill(&external, "hyperframes", "---\nname: hyperframes\n---\n");
        // 主 store 里那个位置现在是一条指向实体的链接。
        link::link_dir(&body, &main.join("hyperframes")).unwrap();

        let report = adopt(&[req("hyperframes", &body, None)], &main, false).unwrap();

        assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
        assert!(report.steps.iter().all(|s| s.done));
        assert!(!report.steps.is_empty(), "adoption silently did nothing");

        // 实体落在主 store 里，而且是**真目录**不是链接。
        let dest = main.join("hyperframes");
        assert!(!link::is_link(&dest), "the main store still holds a link");
        assert!(dest.join("SKILL.md").is_file());
        assert_eq!(
            fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "---\nname: hyperframes\n---\n"
        );
        // 原位留一条指过来的链接 —— 物理上仍然只有一份。
        assert!(link::is_link(&body));
        let (_, resolved) = link::resolve_chain(&body, 16);
        assert!(link::same_path(&resolved.unwrap(), &dest));
    }

    /// 主 store 目录**自己**是条链接的时候，实体确实已经在里面了，别再搬一遍。
    ///
    /// 这一条钉的是上面那支的边界：`dest` 本身不是链接（是它的父目录），
    /// `link::is_link(&dest)` 为假，所以要落到「已经在主 store 里」那条上。
    #[test]
    fn a_body_reached_through_a_linked_main_store_is_already_home() {
        let root = temp_root("adopt-storelink");
        let real = root.join("real");
        fs::create_dir_all(&real).unwrap();
        let body = skill(&real, "pinme", "x");
        // 主 store 是一条指向 `real` 的链接，于是 `main/pinme` 就是 `real/pinme`。
        let main = root.join("main");
        link::link_dir(&real, &main).unwrap();

        let report = adopt(&[req("pinme", &body, None)], &main, false).unwrap();

        assert!(report.steps.is_empty(), "{:?}", report.steps);
        assert!(!link::is_link(&body), "the body was replaced by a link");
        assert!(body.join("SKILL.md").is_file());
    }

    #[test]
    fn a_dry_run_lists_every_step_and_touches_nothing() {
        let root = temp_root("dry");
        let main = root.join("main");
        let external = root.join("external");
        fs::create_dir_all(&external).unwrap();
        let body = skill(&external, "pinme", "x");

        let report = adopt(&[req("pinme", &body, None)], &main, true).unwrap();

        assert!(report.dry_run);
        assert!(report.steps.iter().all(|s| !s.done));
        assert_eq!(
            report
                .steps
                .iter()
                .map(|s| s.kind)
                .collect::<Vec<_>>(),
            vec![StepKind::EnsureDir, StepKind::Move, StepKind::Link],
        );
        // 一个字节都没动。
        assert!(!main.exists());
        assert!(!link::is_link(&body));
        assert!(body.join("SKILL.md").is_file());
    }

    #[test]
    fn identical_content_merges_silently_instead_of_asking() {
        // 本机 33 个重复条目里绝大多数是这种 —— 每个都弹一次三选一等于让用户点 33 次
        // 「随便哪个都行」。
        let root = temp_root("same");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let external = root.join("external");
        fs::create_dir_all(&external).unwrap();
        skill(&main, "doc", "same body\n");
        let body = skill(&external, "doc", "same body\n");

        let report = adopt(&[req("doc", &body, None)], &main, false).unwrap();

        assert!(report.conflicts.is_empty(), "内容一致不该弹冲突");
        assert!(link::is_link(&body));
        assert_eq!(
            fs::read_to_string(main.join("doc").join("SKILL.md")).unwrap(),
            "same body\n"
        );
    }

    #[test]
    fn differing_content_without_a_resolution_reports_a_conflict_and_changes_nothing() {
        let root = temp_root("conflict");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let external = root.join("external");
        fs::create_dir_all(&external).unwrap();
        skill(&main, "doc", "line1\nline2\n");
        let body = skill(&external, "doc", "line1\nline2\nline3\n");
        fs::write(body.join("run.sh"), "echo hi").unwrap();

        let report = adopt(&[req("doc", &body, None)], &main, false).unwrap();

        assert_eq!(report.conflicts.len(), 1);
        let c = &report.conflicts[0];
        assert_eq!(c.name, "doc");
        // 用户要能看见差在哪，不能盲选。
        let run = c.files.iter().find(|f| f.path == "run.sh").unwrap();
        assert_eq!(run.status, FileStatus::OnlyExternal);
        let md = c.files.iter().find(|f| f.path == "SKILL.md").unwrap();
        assert_eq!(md.status, FileStatus::Different);
        let diff = c.skill_md.as_ref().unwrap();
        assert_eq!((diff.plus, diff.minus), (1, 0));
        assert!(!diff.truncated);
        // 没选之前一步都不做。
        assert!(report.steps.is_empty());
        assert!(!link::is_link(&body));
    }

    #[test]
    fn keeping_the_main_copy_drops_the_external_body_and_links_the_spot() {
        let root = temp_root("keepmain");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let external = root.join("external");
        fs::create_dir_all(&external).unwrap();
        skill(&main, "doc", "main version\n");
        let body = skill(&external, "doc", "external version\n");

        adopt(
            &[req("doc", &body, Some(Resolution::KeepMain))],
            &main,
            false,
        )
        .unwrap();

        assert!(link::is_link(&body));
        assert_eq!(
            fs::read_to_string(body.join("SKILL.md")).unwrap(),
            "main version\n"
        );
        // 备份位不留垃圾。
        assert!(!backup_path(&body).exists());
    }

    #[test]
    fn using_the_external_copy_overwrites_the_main_one_and_keeps_old_links_working() {
        let root = temp_root("useext");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let external = root.join("external");
        fs::create_dir_all(&external).unwrap();
        let main_body = skill(&main, "doc", "main version\n");
        let body = skill(&external, "doc", "external version\n");
        // 别处一条早就指向主 store 的链接 —— 覆盖之后它必须照样能用。
        let other = root.join("claude");
        fs::create_dir_all(&other).unwrap();
        link::link_dir(&main_body, &other.join("doc")).unwrap();

        adopt(
            &[req("doc", &body, Some(Resolution::UseExternal))],
            &main,
            false,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(main.join("doc").join("SKILL.md")).unwrap(),
            "external version\n"
        );
        assert_eq!(
            fs::read_to_string(other.join("doc").join("SKILL.md")).unwrap(),
            "external version\n"
        );
        assert!(link::is_link(&body));
    }

    #[test]
    fn keeping_both_renames_the_external_one() {
        let root = temp_root("keepboth");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let store = root.join(".cc-switch").join("skills");
        fs::create_dir_all(&store).unwrap();
        skill(&main, "doc", "main\n");
        let body = skill(&store, "doc", "external\n");

        let report = adopt(&[req("doc", &body, None)], &main, true).unwrap();
        // 建议的名字带上外部 store 的来历，而不是所有 store 都叫 skills。
        let suggested = report.conflicts[0].suggested_rename.clone();
        assert_eq!(suggested, "doc-from-cc-switch");

        adopt(
            &[req("doc", &body, Some(Resolution::KeepBoth(suggested.clone())))],
            &main,
            false,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(main.join("doc").join("SKILL.md")).unwrap(),
            "main\n"
        );
        assert_eq!(
            fs::read_to_string(main.join(&suggested).join("SKILL.md")).unwrap(),
            "external\n"
        );
        assert!(link::is_link(&body));
    }

    #[test]
    fn adopting_refuses_links_and_managed_copies() {
        let root = temp_root("refuse");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let real = skill(&store, "real", "x");
        let alias = store.join("alias");
        link::link_dir(&real, &alias).unwrap();

        let err = adopt(&[req("alias", &alias, None)], &main, true).unwrap_err();
        assert!(err.contains("is a link"), "{err}");
        // 报错发生在计划阶段 —— 磁盘上什么都没动。
        assert!(link::is_link(&alias));
    }

    #[test]
    fn a_failure_midway_rolls_the_whole_batch_back() {
        // 第二条的原位是只读目录，建链会失败 —— 第一条已经搬走的内容必须搬回来。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = temp_root("rollback");
            let main = root.join("main");
            fs::create_dir_all(&main).unwrap();
            let a_store = root.join("a");
            let b_store = root.join("b");
            fs::create_dir_all(&a_store).unwrap();
            fs::create_dir_all(&b_store).unwrap();
            let a = skill(&a_store, "alpha", "a\n");
            let b = skill(&b_store, "beta", "b\n");

            fs::set_permissions(&b_store, fs::Permissions::from_mode(0o555)).unwrap();
            let err = adopt(&[req("alpha", &a, None), req("beta", &b, None)], &main, false)
                .unwrap_err();
            fs::set_permissions(&b_store, fs::Permissions::from_mode(0o755)).unwrap();

            assert!(err.contains("Rolled back"), "{err}");
            // 第一条原样退回：内容还在原地，主 store 里没有残留。
            assert_eq!(fs::read_to_string(a.join("SKILL.md")).unwrap(), "a\n");
            assert!(!link::is_link(&a));
            assert!(!main.join("alpha").exists());
        }
    }

    #[test]
    fn enabling_creates_the_link_and_disabling_takes_it_away_without_touching_content() {
        let root = temp_root("toggle");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let body = skill(&store, "git-push", "x");
        let agent = root.join("agent").join("skills");

        toggle("git-push", &agent, Some(&body), true, false).unwrap();
        assert!(link::is_link(&agent.join("git-push")));

        toggle("git-push", &agent, None, false, false).unwrap();
        assert!(!agent.join("git-push").exists());
        // 停用永远不该动实体内容。
        assert!(body.join("SKILL.md").is_file());
    }

    #[test]
    fn disabling_refuses_to_delete_real_content() {
        let root = temp_root("toggle-real");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        skill(&store, "git-push", "x");

        let err = toggle("git-push", &store, None, false, false).unwrap_err();
        assert!(err.contains("real content"), "{err}");
        assert!(store.join("git-push").join("SKILL.md").is_file());
    }

    #[test]
    fn enabling_something_already_enabled_is_a_no_op() {
        let root = temp_root("toggle-idem");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let body = skill(&store, "x", "x");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        link::link_dir(&body, &agent.join("x")).unwrap();

        let report = toggle("x", &agent, Some(&body), true, false).unwrap();
        assert!(report.steps.is_empty());
    }

    #[test]
    fn repair_flattens_a_two_hop_chain_into_one() {
        let root = temp_root("repair");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let body = skill(&store, "smux", "x");
        let middle = root.join("agents");
        fs::create_dir_all(&middle).unwrap();
        link::link_dir(&body, &middle.join("smux")).unwrap();
        let top = root.join("claude");
        fs::create_dir_all(&top).unwrap();
        link::link_dir(&middle.join("smux"), &top.join("smux")).unwrap();

        let (hops, _) = link::resolve_chain(&top.join("smux"), 16);
        assert_eq!(hops.len(), 2, "前提：这是一条两跳链");

        repair(
            &[RepairRequest {
                path: top.join("smux").to_string_lossy().to_string(),
                action: RepairAction::Relink(body.to_string_lossy().to_string()),
            }],
            false,
        )
        .unwrap();

        let (hops, resolved) = link::resolve_chain(&top.join("smux"), 16);
        assert_eq!(hops.len(), 1, "压成一跳");
        assert!(link::same_path(&resolved.unwrap(), &body));
    }

    /// 收编完成之后，旧 store 里各剩一条指向新家的活链接，没有任何 agent 在扫那些
    /// 目录。「停用」是按 agent 关的（碰不到不属于任何一家的这条），「清理死链」只
    /// 碰解析不到东西的 —— 所以这一条原来一个入口都没有。
    #[test]
    fn a_live_link_nobody_reads_can_be_taken_away_without_touching_what_it_points_at() {
        let root = temp_root("unlink-live");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let body = skill(&main, "hyperframes", "---\nname: hyperframes\n---\n");
        let stale = root.join("old-store");
        fs::create_dir_all(&stale).unwrap();
        let at = stale.join("hyperframes");
        link::link_dir(&body, &at).unwrap();

        let report = unlink_ref(&at, false).unwrap();

        assert!(report.steps.iter().all(|s| s.done));
        assert_eq!(
            report.steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![StepKind::Unlink],
        );
        // 链接没了，它指着的内容一个字节没动。
        assert!(!at.exists() && !link::is_link(&at));
        assert!(body.join("SKILL.md").is_file());
        assert_eq!(
            fs::read_to_string(body.join("SKILL.md")).unwrap(),
            "---\nname: hyperframes\n---\n"
        );
    }

    /// 唯一会吃掉数据的走法：把实体目录当链接拆了。闸立在后端，不靠 UI 记得禁用按钮。
    #[test]
    fn unlinking_refuses_anything_that_is_real_content() {
        let root = temp_root("unlink-body");
        let body = skill(&root, "hyperframes", "x");

        let err = unlink_ref(&body, false).unwrap_err();

        assert!(err.contains("not a link"), "{err}");
        assert!(body.join("SKILL.md").is_file());
    }

    /// dry-run 要把那一步摆出来（计划框里画的就是它），并且一个字节都不动。
    #[test]
    fn unlinking_has_a_dry_run_like_every_other_write() {
        let root = temp_root("unlink-dry");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        let body = skill(&main, "hyperframes", "x");
        let at = root.join("old");
        fs::create_dir_all(&at).unwrap();
        let link_at = at.join("hyperframes");
        link::link_dir(&body, &link_at).unwrap();

        let report = unlink_ref(&link_at, true).unwrap();

        assert!(report.dry_run);
        assert!(report.steps.iter().all(|s| !s.done));
        // 计划里要说清它现在指着谁 —— 用户是照着这一行决定拆不拆的。
        assert_eq!(report.steps.len(), 1);
        assert!(report.steps[0]
            .target
            .as_deref()
            .is_some_and(|t| Path::new(t) == body));
        assert!(link::is_link(&link_at));
    }

    #[test]
    fn repair_removes_a_dead_link_but_refuses_a_live_one() {
        let root = temp_root("repair-dead");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let body = skill(&store, "pinme", "x");
        let agent = root.join("claude");
        fs::create_dir_all(&agent).unwrap();
        link::link_dir(&body, &agent.join("pinme")).unwrap();
        let live = agent.join("pinme");

        // 活链不能当死链删 —— 那是「停用」，不是「清理」。
        let err = repair(
            &[RepairRequest {
                path: live.to_string_lossy().to_string(),
                action: RepairAction::Unlink,
            }],
            false,
        )
        .unwrap_err();
        assert!(err.contains("still resolves"), "{err}");

        fs::remove_dir_all(&body).unwrap();
        repair(
            &[RepairRequest {
                path: live.to_string_lossy().to_string(),
                action: RepairAction::Unlink,
            }],
            false,
        )
        .unwrap();
        assert!(!link::is_link(&live));
    }

    #[test]
    fn deleting_unlinks_every_reference_before_touching_the_body() {
        // 反过来的话链接瞬间全变死链，而这时候已经没有信息知道该去哪些目录清理 ——
        // 方案文档 1.1 那 22 条死链就是这么来的。
        let root = temp_root("delete");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let body = skill(&store, "pinme", "x");
        // 三条来自不同工具的引用：直连、两跳的中间节点、以及最上面那条。
        let agents = root.join("agents");
        let claude = root.join("claude");
        let codex = root.join("codex");
        for d in [&agents, &claude, &codex] {
            fs::create_dir_all(d).unwrap();
        }
        link::link_dir(&body, &agents.join("pinme")).unwrap();
        link::link_dir(&agents.join("pinme"), &claude.join("pinme")).unwrap();
        link::link_dir(&body, &codex.join("pinme")).unwrap();

        let refs: Vec<String> = [&claude, &codex, &agents, &store]
            .iter()
            .map(|d| d.join("pinme").to_string_lossy().to_string())
            .collect();
        let ops = plan_delete(&refs, &[body.to_string_lossy().to_string()], false).unwrap();

        // 顺序：三条解链在前，删源在最后。
        let kinds: Vec<StepKind> = ops.iter().map(|o| o.describe().kind).collect();
        assert_eq!(
            kinds,
            vec![
                StepKind::Unlink,
                StepKind::Unlink,
                StepKind::Unlink,
                StepKind::DeleteDir
            ],
        );

        run(ops, Vec::new(), false).unwrap();
        for d in [&agents, &claude, &codex] {
            assert!(!d.join("pinme").exists(), "{} 的引用没清掉", d.display());
            assert!(!link::is_link(&d.join("pinme")));
        }
        assert!(!body.exists());
    }

    #[test]
    fn keeping_the_body_only_unlinks() {
        let root = temp_root("delete-keep");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let body = skill(&store, "pinme", "x");
        let claude = root.join("claude");
        fs::create_dir_all(&claude).unwrap();
        link::link_dir(&body, &claude.join("pinme")).unwrap();

        let ops = plan_delete(
            &[claude.join("pinme").to_string_lossy().to_string()],
            &[body.to_string_lossy().to_string()],
            true,
        )
        .unwrap();
        run(ops, Vec::new(), false).unwrap();

        assert!(!claude.join("pinme").exists());
        assert!(body.join("SKILL.md").is_file());
    }

    #[test]
    fn deleting_refuses_a_reference_that_is_neither_a_link_nor_a_known_body() {
        // 扫描之后被人改过：那个位置现在是个实体目录，但它不在 bodies 里。
        // 与其猜，不如停下来。
        let root = temp_root("delete-stale");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let body = skill(&store, "pinme", "x");
        let other = root.join("claude");
        fs::create_dir_all(&other).unwrap();
        skill(&other, "pinme", "someone put real content here");

        let err = plan_delete(
            &[other.join("pinme").to_string_lossy().to_string()],
            &[body.to_string_lossy().to_string()],
            false,
        )
        .unwrap_err();
        assert!(err.contains("neither a link"), "{err}");
        assert!(other.join("pinme").join("SKILL.md").is_file());
    }

    #[test]
    fn the_plan_puts_every_deletion_after_every_reversible_step() {
        // 删目录撤不回来，所以它必须排在所有能撤销的操作之后 —— 否则中途失败时
        // 「已经删了、但链还在」就成了不可收拾的现场。
        let ops = vec![
            Op::DeleteDir {
                path: PathBuf::from("/x"),
            },
            Op::Unlink {
                at: PathBuf::from("/y"),
                expected: None,
            },
        ];
        let report = run(ops, Vec::new(), true).unwrap();
        assert_eq!(
            report.steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![StepKind::Unlink, StepKind::DeleteDir],
        );
    }

    /// 计划里被解掉的那些路径。
    fn unlinked(ops: &[Op]) -> Vec<String> {
        ops.iter()
            .filter_map(|o| match o {
                Op::Unlink { at, .. } => Some(at.to_string_lossy().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn deleting_one_body_unlinks_everything_that_pointed_at_it() {
        // 「没有 agent 读它」说的是没有 agent **直接扫**这个目录，不是没人指着它。
        // 只删目录就是亲手造一条死链 —— 正是这个面板要清理的东西。
        let root = temp_root("delbody");
        let body = skill(&root, "orphan", "x");
        let link_a = root.join("a").join("orphan");
        let link_b = root.join("b").join("orphan");
        link::link_dir(&body, &link_a).unwrap();
        link::link_dir(&body, &link_b).unwrap();

        let refs = vec![
            link_a.to_string_lossy().to_string(),
            link_b.to_string_lossy().to_string(),
            body.to_string_lossy().to_string(),
        ];
        let ops = plan_delete_body(&refs, &body).unwrap();
        let mut got = unlinked(&ops);
        got.sort();
        let mut want = vec![
            link_a.to_string_lossy().to_string(),
            link_b.to_string_lossy().to_string(),
        ];
        want.sort();
        assert_eq!(got, want);
        // 目录自己排在最后，而且只删这一个。
        assert!(matches!(ops.last(), Some(Op::DeleteDir { path }) if path == &body));
        assert_eq!(
            ops.iter()
                .filter(|o| matches!(o, Op::DeleteDir { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn a_two_hop_link_that_lands_on_the_body_is_unlinked_too() {
        // 第一跳指着中间那条链接，不是 body；只看第一跳会漏掉它，删完就是死链。
        let root = temp_root("delbody2");
        let body = skill(&root, "deep", "x");
        let mid = root.join("mid").join("deep");
        let outer = root.join("outer").join("deep");
        link::link_dir(&body, &mid).unwrap();
        link::link_dir(&mid, &outer).unwrap();

        let refs = vec![
            outer.to_string_lossy().to_string(),
            mid.to_string_lossy().to_string(),
            body.to_string_lossy().to_string(),
        ];
        assert_eq!(unlinked(&plan_delete_body(&refs, &body).unwrap()).len(), 2);
    }

    #[test]
    fn a_link_pointing_somewhere_else_is_left_alone() {
        // 用户要删的是这一份，不是顺手把指向别处的引用也拆了。
        let root = temp_root("delbody3");
        let doomed = skill(&root, "doomed", "x");
        let keeper = skill(&root, "keeper", "x");
        let to_keeper = root.join("a").join("keeper");
        link::link_dir(&keeper, &to_keeper).unwrap();

        let refs = vec![
            to_keeper.to_string_lossy().to_string(),
            doomed.to_string_lossy().to_string(),
            keeper.to_string_lossy().to_string(),
        ];
        let ops = plan_delete_body(&refs, &doomed).unwrap();
        assert!(unlinked(&ops).is_empty(), "不该碰指向别处的链接");
        // 另一份实体目录也不能跟着被删。
        assert_eq!(
            ops.iter()
                .filter(|o| matches!(o, Op::DeleteDir { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn deleting_a_body_that_is_not_this_skills_content_is_refused() {
        // 不校验的话，前端传什么路径就删什么路径。
        let root = temp_root("delbody4");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        skill(&store, "mine", "x");
        let stranger = skill(&root, "stranger", "x");

        let err = delete_body(
            "mine",
            &stranger,
            None,
            &[store.to_string_lossy().to_string()],
            true,
        )
        .unwrap_err();
        assert!(err.contains("is not content of"), "{err}");
        assert!(stranger.join("SKILL.md").is_file());
    }

    #[test]
    fn line_diff_counts_added_and_removed_lines() {
        let root = temp_root("diff");
        let a = skill(&root, "a", "one\ntwo\nthree\n");
        let b = skill(&root, "b", "one\nthree\nfour\n");
        let d = skill_md_diff(&a, &b).unwrap();
        assert_eq!((d.plus, d.minus), (1, 1));
    }

    /// 把 hunk 摊平成 `"±text"` 便于断言。
    fn flat(hunks: &[DiffHunk]) -> Vec<String> {
        hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .map(|l| {
                let sign = match l.kind.as_str() {
                    "add" => '+',
                    "del" => '-',
                    _ => ' ',
                };
                format!("{sign}{}", l.text)
            })
            .collect()
    }

    fn hunks_of(a: &str, b: &str) -> Vec<DiffHunk> {
        let a: Vec<&str> = a.lines().collect();
        let b: Vec<&str> = b.lines().collect();
        line_hunks(&a, &b).0
    }

    #[test]
    fn a_line_diff_shows_which_lines_changed_not_just_how_many() {
        // 阶段 5 之前这里只有 `+1 −1`，用户得先接受合并才知道改的是哪一行。
        let h = hunks_of("one\ntwo\nthree\n", "one\nthree\nfour\n");
        assert_eq!(flat(&h), vec![" one", "-two", " three", "+four"]);
    }

    #[test]
    fn two_identical_files_produce_no_hunks_at_all() {
        assert!(hunks_of("same\nlines\n", "same\nlines\n").is_empty());
    }

    #[test]
    fn line_numbers_count_from_one_on_the_side_that_has_the_line() {
        let h = hunks_of("a\nb\n", "a\nB\n");
        let lines = &h[0].lines;
        assert_eq!(lines[0].old_no, Some(1));
        assert_eq!(lines[0].new_no, Some(1));
        // 删除的行只有旧行号，新增的行只有新行号 —— DiffBlock 靠这个留空。
        let del = lines.iter().find(|l| l.kind == "del").unwrap();
        assert_eq!((del.old_no, del.new_no), (Some(2), None));
        let add = lines.iter().find(|l| l.kind == "add").unwrap();
        assert_eq!((add.old_no, add.new_no), (None, Some(2)));
    }

    #[test]
    fn only_the_neighbourhood_of_a_change_is_carried() {
        // 改一行不该把 200 行全搬进弹框；上下各三行足够看懂。
        let a: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 25\n", "CHANGED\n");
        let h = hunks_of(&a, &b);
        assert_eq!(h.len(), 1);
        assert_eq!(
            flat(&h),
            vec![
                " line 22", " line 23", " line 24", "-line 25", "+CHANGED", " line 26", " line 27",
                " line 28",
            ]
        );
        assert_eq!(h[0].old_start, 23);
    }

    #[test]
    fn far_apart_changes_become_separate_hunks() {
        let a: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 5\n", "FIVE\n").replace("line 40\n", "FORTY\n");
        assert_eq!(hunks_of(&a, &b).len(), 2);
    }

    #[test]
    fn changes_whose_context_touches_are_merged_into_one() {
        // 中间只隔几行的话，切成两块会在 UI 上多一条毫无信息量的分隔线。
        let a: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 5\n", "FIVE\n").replace("line 9\n", "NINE\n");
        assert_eq!(hunks_of(&a, &b).len(), 1);
    }

    #[test]
    fn a_diff_too_long_for_the_dialog_is_cut_and_says_so() {
        let a: String = (0..MAX_DIFF_HUNK_LINES + 100).map(|i| format!("line {i}\n")).collect();
        let b: String = (0..MAX_DIFF_HUNK_LINES + 100).map(|i| format!("other {i}\n")).collect();
        let av: Vec<&str> = a.lines().collect();
        let bv: Vec<&str> = b.lines().collect();
        let (hunks, clipped) = line_hunks(&av, &bv);
        assert!(clipped);
        let total: usize = hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(total, MAX_DIFF_HUNK_LINES);
    }

    #[test]
    fn a_file_over_the_compare_limit_still_reports_no_hunks() {
        // 超限时 plus/minus 都是 0 且 truncated —— 逐行差异同样不能瞎给。
        let root = temp_root("difflimit");
        let big: String = (0..MAX_DIFF_LINES + 10).map(|i| format!("l{i}\n")).collect();
        let a = skill(&root, "a", &big);
        let b = skill(&root, "b", "short\n");
        let d = skill_md_diff(&a, &b).unwrap();
        assert!(d.truncated);
        assert!(d.hunks.is_empty());
        assert!(!d.clipped);
    }

    #[test]
    fn content_comparison_ignores_mtime() {
        // 指纹里带 mtime，两份内容一样但不同时刻产生的副本指纹不同 —— 拿它判「一致」
        // 会把每个重复条目都打成冲突。
        let root = temp_root("mtime");
        let a = skill(&root, "a", "same\n");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = skill(&root, "b", "same\n");
        assert_ne!(
            link::fingerprint(&a).unwrap(),
            link::fingerprint(&b).unwrap(),
            "前提：指纹确实受 mtime 影响"
        );
        let (ma, _) = content_map(&a);
        let (mb, _) = content_map(&b);
        assert_eq!(ma, mb);
    }
}
