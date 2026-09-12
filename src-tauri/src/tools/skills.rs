//! Skills 全盘扫描（只读）。
//!
//! 本机实测（方案文档 1.1）：三套 store 互不知情，22 条链接里 14 条是两跳的，
//! 26 个 skill 在两套 store 里各存一份，还漏着 2 条死链——而这些全是做调研时**量**出来
//! 的，机器上没有任何东西会主动告诉用户。这个模块就是那台量尺。
//!
//! 三件事：
//!
//! 1. **找 store**：七家 agent 的 skills 目录 + 三套第三方 store。
//! 2. **建反向索引**：把每个条目解析到底，按「真实目录 → 引用它的所有条目」归并。
//!    删除要靠它（先解链再删源），健康判定也要靠它——只看第一跳的话，两跳链里
//!    「第一跳在、第二跳断了」会被报成健康。
//! 3. **打角标**：断链 / 两跳 / 同名重复 / 成环，外加 [`super::risk`] 的风险等级。
//!
//! 全程不写盘。收编、修复、删除是阶段 4。

use rayon::prelude::*;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::link;
use super::risk::{self, RiskFinding, RiskLevel};
use super::{ConfigOrigin, ConfigScope, SkillSource};
use crate::util::{home, normalize_windows_path};

/// 解析链接时最多跟几跳。本机最长是两跳，16 是给成环留的余量。
const MAX_HOPS: usize = 16;
/// 单个 skill 目录最多统计 / 扫描多少个文件。
const MAX_BODY_FILES: usize = 400;
/// 目录递归深度上限。
const MAX_DEPTH: usize = 8;

/// 第三方 skill store。**不是 agent**，是几个 skill 管理器各自的仓库目录。
///
/// 依据：方案文档 1.1 的实测表。写死是因为它们就是本机上客观存在的三个目录；
/// 用户装了别的管理器，其内容照样会作为链接目标出现在链路里，只是不单列成 store。
/// 一个候选都没有时的兜底主 store。
///
/// 挑 `~/.agents/` 而不是 `~/.skills-manager/` 之类：`.agents/` 是**跨 agent 的公共
/// 约定**，grok / pi / opencode 三家都会主动去扫它（各自的加载器里写死的），
/// 内容放这儿不用再往每家目录建链就已经有三家读得到。另外两个是第三方管理器
/// 自己的目录，没装那个工具的人不该被塞一个它的目录。
fn default_main_store() -> PathBuf {
    home().join(".agents").join("skills")
}

const SHARED_STORES: &[&str] = &[
    ".agents/skills",
    ".skills-manager/skills",
    ".cc-switch/skills",
];

// ---------------------------------------------------------------------------
// 对外形状
// ---------------------------------------------------------------------------

/// 一个候选 store 的概况。首次进入要让用户从这里挑一个主 store。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreCandidate {
    pub path: String,
    /// 会读这个目录的**所有** agent。同一个目录常常不止一家读 —— grok 的
    /// `[compat.claude] skills = true` 让它照样扫 `~/.claude/skills`，只记一家
    /// 就等于告诉用户「这些 skill 在 grok 里用不了」。
    pub agents: Vec<String>,
    pub scope: ConfigScope,
    pub origin: ConfigOrigin,
    pub exists: bool,
    /// 条目总数（链接 + 实体目录 + 受管副本）。
    pub total: usize,
    pub links: usize,
    /// 实体目录数。**只有实体目录才是内容**，主 store 该从这一栏多的里面挑。
    pub real_dirs: usize,
    pub broken: usize,
    /// 够不够格当主 store。见 [`can_be_main`]。
    pub can_be_main: bool,
}

/// 这个目录能不能当主 store —— **只有用户级的跨 agent 共享目录**够格。
///
/// 挡掉的两类，两类都出现在真实扫描结果里：
///
/// - **项目级**（`<repo>/.agents/skills`）：跟着仓库走，换个项目就没了。主 store 是
///   全机器一份、所有 agent 链过去的地方，把它设成项目目录等于让别的项目全断链。
/// - **agent 自有目录**（`~/.claude/skills`、`~/.gemini/config/skills`、
///   `~/.config/opencode/skill` …）：那是给链接落脚的地方。把内容搬进去，就从
///   「一份内容所有 agent 共享」变成「绑死在某一家」—— 正好和这个面板要做的事相反。
///
/// 剩下的就是 `~/.agents/skills` 和几个第三方管理器的 store，它们才是内容该待的地方。
fn can_be_main(scope: ConfigScope, origin: ConfigOrigin) -> bool {
    scope == ConfigScope::User && origin == ConfigOrigin::Shared
}

/// 一条链接的一跳。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkHop {
    pub from: String,
    pub to: String,
    /// `to` 存不存在。断链就断在第一个 `false` 上。
    pub exists: bool,
}

/// 扫描时能判定的条目状态。
///
/// 和 [`link::LinkHealth`] 不是一回事：那个要先知道「应该指向哪」，而扫描发生在
/// 用户指定主 store **之前**，此时没有「应该」。硬凑一个期望值只会凭空造出
/// `WrongTarget`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "detail")]
pub enum RefHealth {
    /// 实体目录——内容本身。收编的对象就是它。
    RealDir,
    /// 链接，一路解析到了真实目录。
    Linked,
    /// 链接，但解析不到。`detail` 是链上第一个不存在的路径。
    Broken(String),
    /// 跟满 `MAX_HOPS` 还没到底：成环，或者链长得离谱。
    Cyclic,
    /// 我们在 Windows 上降级复制出来的副本，且**和源一致**。`detail` 是它记录的源。
    ManagedCopy(String),
    /// 受管副本，但**源**已经变了 —— 这个 agent 读到的是旧版本。`detail` 是源。
    ///
    /// 少了这一态就是 Skills-Manager 的那个坑（方案 2.1）：降级成拷贝之后用户在源上
    /// 改了 skill，agent 永远读旧的，而界面一路显示健康。
    CopyStale(String),
    /// 受管副本，**副本**被就地改过。`detail` 是源。重拷会把这些修改抹掉，所以不给一键。
    CopyEdited(String),
    /// 受管副本，源和副本**都**变了。`detail` 是源。自动挑一边必然吃掉另一边。
    CopyDiverged(String),
    /// 受管副本，但它记录的源已经没了。`detail` 是那个源。
    CopyOrphaned(String),
    /// 链接解析到了东西，但那不是目录。`detail` 是那个目标。
    ///
    /// 光判断 `exists()` 会把它报成健康，可它没有任何内容可用 —— 用户看到的是一个
    /// 「好端端却用不了」的 skill。CLAUDE.md 里记着 git 在 Windows 上默认
    /// `core.symlinks=false`，仓库里的 symlink 会被检出成**文本文件**，这条正是那个后果。
    NotADirectory(String),
}

/// 某个 store 里指向某个 skill 的一个条目。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRef {
    pub path: String,
    /// 所在 store 的路径。
    pub store: String,
    /// 会读到**这条引用所在 store** 的所有 agent。
    ///
    /// 对链接来说这就是「谁用得上这一条」。对实体目录**不是** —— 见 [`reached_by`]。
    pub agents: Vec<String>,
    /// 谁真的能读到这条引用背后的内容 —— 直接读，或者顺着别人的链走到这儿。
    ///
    /// `agents` 回答的是「这条引用所在的那个目录被哪几家读」。本机
    /// `~/.skills-manager/skills/css-animations` 没有任何 agent 直接读 `.skills-manager`，
    /// 于是 `agents` 是空的 —— 可它是 `~/.claude/skills/css-animations` 和
    /// `~/.agents/skills/css-animations` 两条链的终点。照 `agents` 标成「没有 agent 读它」
    /// 并给一个删除按钮，等于请用户亲手把那两条链一起打断。
    ///
    /// 中间节点同理：一条谁都不直接读的链接，可能正是别人那条链的中途一跳。
    pub reached_by: Vec<String>,
    pub health: RefHealth,
    /// 完整链路，实体目录为空数组。
    pub hops: Vec<LinkHop>,
    /// 最终落到的真实目录。
    pub resolved: Option<String>,
}

/// 一份实体内容。同名 skill 有多个 body 就是「同名重复」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillBody {
    pub path: String,
    /// 它所在的 store；不在任何已知 store 里则为 `None`。
    pub store: Option<String>,
    pub files: usize,
    pub bytes: u64,
    /// 最近修改时间，毫秒。
    pub modified: Option<u64>,
    pub risk: RiskLevel,
    /// 这份内容**没被完整扫完**：文件数或深度触了上限，或者有文件读不了/太大/是二进制。
    ///
    /// 为什么必须报出来：风险等级是「扫过的部分里最高的那个」。恶意脚本排在第 401 个
    /// 文件、或藏在第 9 层，`risk` 照样是 `None` —— 把部分结果当成「干净」呈现，比不扫
    /// 更糟，因为用户会据此放心。UI 必须据此把结论说成「至少」而不是「就是」。
    pub truncated: bool,
}

/// 列表角标。优先级由 [`worst_badge`] 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillBadge {
    /// 同名 skill 有多份实体内容，改哪份生效全看链接指向谁。
    Duplicate,
    /// 链路两跳及以上，任何一跳断掉整条就废。
    TwoHop,
    /// 链接成环。
    Cyclic,
    /// 有死链。
    Broken,
    /// 有受管副本和它的源对不上了 —— 那个 agent 读到的不是最新内容。
    ///
    /// 这是 Skills-Manager 缺的那一块（方案 2.1）：降级成拷贝之后源改了，副本还是旧的，
    /// 而界面一路显示健康。排在最后是因为它只可能出现在 Windows 的降级路径上。
    CopyStale,
}

/// 一个 skill（按名字归并后）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    pub name: String,
    pub refs: Vec<SkillRef>,
    pub bodies: Vec<SkillBody>,
    pub badges: Vec<SkillBadge>,
    pub risk: RiskLevel,
    /// 任一 body 没扫完。`risk` 只是下限，不是结论。
    pub truncated: bool,
    /// 取自主 body 的 SKILL.md `description`。
    pub description: Option<String>,
    /// 主 body 是个带 remote 的 clone 时给出它的来源。列表用它标「来自 github」，
    /// 详情页的「更新」按钮也是按同一份 body 判的 —— 两处对不上是最难查的一类 bug。
    pub git: Option<super::skills_git::SkillGit>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub total: usize,
    pub broken: usize,
    pub two_hop: usize,
    pub duplicate: usize,
    pub cyclic: usize,
    /// 受管副本和源对不上的有几个。非 Windows 上恒为 0（那儿根本不会降级到复制）。
    pub copy_stale: usize,
    /// 从远端 clone 来的有几个。这一条不是「毛病」，是来源 —— 但和那四个并排最有用：
    /// 它回答的是「这些东西我还能不能拉到新版」。
    pub from_git: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillScan {
    /// 用户 home，前端拿它把绝对路径缩写成 `~/…`。
    pub home: String,
    /// 一个候选都挑不出来时的兜底主 store（`~/.agents/skills`）。前端拿它决定
    /// 「这个目录哪怕还不存在也要出现在下拉里」—— 否则新机器上下拉是空的，
    /// 用户没有任何办法开始。
    pub default_main: String,
    pub stores: Vec<StoreCandidate>,
    /// 建议的主 store：够格的候选里实体目录最多的那个；一个都没有就是
    /// [`default_main`](Self::default_main)。只是建议，不落盘。
    pub suggested_main: String,
    pub skills: Vec<SkillEntry>,
    pub summary: ScanSummary,
}

/// SKILL.md 的 frontmatter。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub allowed_tools: Vec<String>,
    /// 其余字段原样保留——各家 skill 规范还在变，认不出来的不该丢掉。
    pub extra: Vec<FrontmatterField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontmatterField {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFile {
    /// 相对 skill 目录。
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub name: String,
    pub refs: Vec<SkillRef>,
    pub bodies: Vec<SkillBody>,
    /// 主 body：被引用最多的那份。详情页的文件清单和 frontmatter 都取自它。
    pub primary: Option<String>,
    pub frontmatter: Option<SkillFrontmatter>,
    pub files: Vec<SkillFile>,
    pub findings: Vec<RiskFinding>,
    pub risk: RiskLevel,
    /// 主 body 没扫完，`risk` / `files` 都不完整。
    pub truncated: bool,
    /// 主 body 是个能从远端更新的 clone 时，它的 remote 和分支；否则 `None`。
    /// 详情页据此决定要不要显示「更新」。
    pub git: Option<super::skills_git::SkillGit>,
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

/// `cwd` 是当前项目目录。给了才能算出项目级 skills 目录；没给就只报 user 级 ——
/// 猜一个出来只会显示一堆根本不存在的路径。
///
/// `extra` 是用户自己加的目录。前端每次调用都带上，后端不落盘 —— 理由和主 store
/// 一样：存两份就有两个真相，用户在别处把目录挪走之后后端那份还是错的。
#[tauri::command(async)]
pub fn tools_scan_skills(cwd: Option<String>, extra: Vec<String>) -> SkillScan {
    scan(cwd.as_deref().map(Path::new), &extra)
}

#[tauri::command(async)]
pub fn tools_skill_detail(
    name: String,
    cwd: Option<String>,
    extra: Vec<String>,
) -> Result<SkillDetail, String> {
    detail(&name, cwd.as_deref().map(Path::new), &extra)
}

// ---------------------------------------------------------------------------
// 扫描
// ---------------------------------------------------------------------------

/// 一个候选 store 的身份：路径 + 谁会读它 + 它是哪一档。
struct StoreIdentity {
    path: PathBuf,
    agents: Vec<String>,
    scope: ConfigScope,
    origin: ConfigOrigin,
}

/// 所有候选 store。
///
/// 同一个目录会被好几家读到（grok 开着 `[compat.claude] skills` 就也扫 `~/.claude/skills`），
/// 所以这里按路径聚合，agent 是个列表而不是一个。
fn store_paths(cwd: Option<&Path>, extra: &[String]) -> Vec<StoreIdentity> {
    let mut out: Vec<StoreIdentity> = Vec::new();
    let mut push = |src: SkillSource, agent: Option<&str>| {
        let path = normalize_windows_path(&src.path);
        if let Some(hit) = out.iter_mut().find(|s| s.path == path) {
            if let Some(agent) = agent {
                if !hit.agents.iter().any(|a| a == agent) {
                    hit.agents.push(agent.to_string());
                }
            }
            // 同一个目录被两家以不同 origin 读到时，记「自有」那一档 —— 它才是这个
            // 目录的本来身份，compat 只是别人也顺手读了。
            if src.origin == ConfigOrigin::Own {
                hit.origin = ConfigOrigin::Own;
            }
            return;
        }
        out.push(StoreIdentity {
            path,
            agents: agent.into_iter().map(str::to_string).collect(),
            scope: src.scope,
            origin: src.origin,
        });
    };

    for agent in super::AGENTS {
        let Ok(surface) = super::surface(agent) else {
            continue;
        };
        for src in surface.skills_sources(cwd) {
            push(src, Some(agent));
        }
    }
    // 第三方 store：没有哪个 agent 直接读它们，但链路会穿过去，收编的对象也在这儿。
    // 依据：方案文档 1.1 的实测表。
    for rel in SHARED_STORES {
        push(
            SkillSource {
                path: home().join(rel),
                scope: ConfigScope::User,
                origin: ConfigOrigin::Shared,
            },
            None,
        );
    }
    // 用户自己加的目录。放在最后一档：撞上已知目录时 `push` 保留原来的身份，所以
    // 挑中 `~/.claude/skills` 这种不会被「用户说了算」洗成够格的主 store —— 它装的
    // 是链接，不是内容，这一点不因为谁挑了它而改变。
    for raw in extra {
        let Some(path) = absolute_dir(raw) else {
            continue;
        };
        push(
            SkillSource {
                path,
                scope: ConfigScope::User,
                origin: ConfigOrigin::Shared,
            },
            None,
        );
    }
    out
}

/// 用户加的目录规整成绝对路径。`~/` 展开，相对路径直接丢掉 —— 相对谁？
fn absolute_dir(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return Some(home().join(rest));
    }
    let path = PathBuf::from(raw);
    path.is_absolute().then(|| normalize_windows_path(&path))
}

/// 走一遍所有 store，只做目录读取和链接解析，**不碰文件内容**。
fn collect(cwd: Option<&Path>, extra: &[String]) -> (Vec<StoreCandidate>, Vec<SkillRef>) {
    let mut stores = Vec::new();
    let mut refs: Vec<SkillRef> = Vec::new();
    for id in store_paths(cwd, extra) {
        let (candidate, entries) = scan_store(&id);
        stores.push(candidate);
        refs.extend(entries);
    }
    (stores, refs)
}

pub fn scan(cwd: Option<&Path>, extra: &[String]) -> SkillScan {
    let (stores, refs) = collect(cwd, extra);

    let store_lookup: Vec<String> = stores.iter().map(|s| s.path.clone()).collect();
    let skills = group(refs, &store_lookup);

    let summary = ScanSummary {
        total: skills.len(),
        broken: count_badge(&skills, SkillBadge::Broken),
        two_hop: count_badge(&skills, SkillBadge::TwoHop),
        duplicate: count_badge(&skills, SkillBadge::Duplicate),
        cyclic: count_badge(&skills, SkillBadge::Cyclic),
        copy_stale: count_badge(&skills, SkillBadge::CopyStale),
        from_git: skills.iter().filter(|s| s.git.is_some()).count(),
    };

    // 只在够格的候选里挑，而且要有实体内容 —— 一个全是链接的目录不是内容所在地。
    // 一个都挑不出来时退到 `~/.agents/skills`：本机全新、或者用户从没用过任何
    // skill 管理器时就是这种情况，总得有个地方让第一次收编往里搬。这个目录不存在
    // 也没关系，写操作的第一步就是 `EnsureDir`。
    let suggested_main = stores
        .iter()
        .filter(|s| s.can_be_main && s.exists && s.real_dirs > 0)
        .max_by_key(|s| (s.real_dirs, std::cmp::Reverse(s.path.clone())))
        .map(|s| s.path.clone())
        .unwrap_or_else(|| default_main_store().to_string_lossy().to_string());

    SkillScan {
        home: home().to_string_lossy().to_string(),
        default_main: default_main_store().to_string_lossy().to_string(),
        stores,
        suggested_main,
        skills,
        summary,
    }
}

pub fn detail(name: &str, cwd: Option<&Path>, extra: &[String]) -> Result<SkillDetail, String> {
    // 只归并这一个名字。走 `scan()` 的话会把全机器每一份内容都重新读一遍加扫一遍
    // 风险（本机 738 个文件），而详情页只需要其中一份。
    let (stores, refs) = collect(cwd, extra);
    let mine: Vec<SkillRef> = refs
        .into_iter()
        .filter(|r| leaf_name(&r.path) == name)
        .collect();
    if mine.is_empty() {
        return Err(format!("No skill named {name}"));
    }
    let store_lookup: Vec<String> = stores.into_iter().map(|s| s.path).collect();
    let entry = group(mine, &store_lookup)
        .into_iter()
        .next()
        .ok_or_else(|| format!("No skill named {name}"))?;

    Ok(build_detail(entry))
}

/// 详情页只展开**主 body** 的文件清单和风险明细。别的 body 的存在本身就是「重复」
/// 角标要说的事，把几份内容混在一个清单里反而看不出谁是谁。
/// 一个 skill 目录长什么样：文件清单、风险发现、frontmatter、有没有列全。
///
/// 抽出来是因为**「发现」面板从陌生仓库检出的目录也要问同一个问题**
/// （`registry_git::preview`）。复制一份的话，两边对「什么算这个 skill 的文件」
/// 「哪些算风险」的定义会各自漂移 —— 而这正是装之前那一屏要回答的东西，
/// 两处答案不一样就等于没答。
pub(super) fn describe_body(
    dir: &Path,
) -> (Vec<SkillFile>, Vec<RiskFinding>, Option<SkillFrontmatter>, bool) {
    let (listed, mut truncated) = list_files(dir);
    let mut findings = Vec::new();
    for f in &listed {
        let scan = risk::scan_file(&f.path, &dir.join(&f.path));
        // 扫不动的文件（太大、非文本）也算「没看全」：一份只扫了一半的目录和一份
        // 真的干净的目录在界面上长得一模一样，不报出来就是在骗人。
        truncated |= scan.skipped;
        findings.extend(scan.findings);
    }
    (listed, findings, read_frontmatter(dir), truncated)
}

fn build_detail(entry: SkillEntry) -> SkillDetail {
    let primary = primary_body(&entry.refs, &entry.bodies);
    let (files, findings, frontmatter, truncated) = match primary.as_deref() {
        Some(dir) => describe_body(Path::new(dir)),
        None => (Vec::new(), Vec::new(), None, false),
    };

    // 只看主 body：详情页的文件清单、frontmatter、风险都取自它，更新按钮动的
    // 也该是同一份。别的 body 是「重复」角标要说的事，收编才是处理它们的入口。
    //
    // 这里重算而不是搬 `entry.git`：`detail()` 走的是单名字的快路，`group()` 那边
    // 算出来的 entry 也在手边，但两条路各自算一次比留一个「谁负责填」的暗坑好。
    let git = primary
        .as_deref()
        .and_then(|dir| super::skills_git::detect(Path::new(dir)));

    SkillDetail {
        name: entry.name,
        refs: entry.refs,
        bodies: entry.bodies,
        primary,
        frontmatter,
        files,
        risk: risk::highest(&findings),
        findings,
        truncated,
        git,
    }
}

/// 把散在各 store 的条目按名字归并成 skill。
///
/// 这就是反向索引：同一个名字下所有引用它的条目挨在一起，删除时按这张表解链，
/// 健康判定也按它——两跳链的中间节点自己也是一条引用，所以它既出现在「引用者」里
/// 也出现在「被引用者所在 store」里，这是对的，不是重复计数。
fn group(refs: Vec<SkillRef>, stores: &[String]) -> Vec<SkillEntry> {
    let mut by_name: BTreeMap<String, Vec<SkillRef>> = BTreeMap::new();
    for r in refs {
        by_name.entry(leaf_name(&r.path)).or_default().push(r);
    }

    // 同一份实体内容会被好几个名字下的条目指到（两套 store 各一份的重复条目尤其
    // 常见），先去重再统一算，每份只走一遍磁盘。
    //
    // 这一步是整次扫描的全部开销：本机 738 个文件 / 2.5 MB，逐个读 + 正则扫。各份
    // 之间互不相干，rayon 一把并行掉——面板是「进店即扫」的，串行 1.3 秒太肉。
    let mut wanted: Vec<String> = by_name.values().flat_map(|refs| body_paths(refs)).collect();
    wanted.sort();
    wanted.dedup();
    let body_cache: BTreeMap<String, SkillBody> = wanted
        .par_iter()
        .map(|p| (p.clone(), build_body(Path::new(p), stores)))
        .collect();

    let mut out = Vec::new();
    for (name, mut refs) in by_name {
        fill_reached_by(&mut refs);
        let bodies: Vec<SkillBody> = body_paths(&refs)
            .into_iter()
            .filter_map(|p| body_cache.get(&p).cloned())
            .collect();
        let badges = badges_for(&refs, &bodies);
        let risk = bodies
            .iter()
            .map(|b| b.risk)
            .max()
            .unwrap_or(RiskLevel::None);
        let truncated = bodies.iter().any(|b| b.truncated);
        let primary = primary_body(&refs, &bodies);
        let description = primary
            .as_deref()
            .and_then(|p| read_frontmatter(Path::new(p)))
            .and_then(|fm| fm.description);
        // 只看一眼 `<body>/.git` 在不在，不在就到此为止 —— 全盘扫描要跑几十遍。
        let git = primary
            .as_deref()
            .and_then(|p| super::skills_git::detect(Path::new(p)));
        out.push(SkillEntry {
            name,
            refs,
            bodies,
            badges,
            risk,
            truncated,
            description,
            git,
        });
    }
    out
}

/// 补上每条引用的 [`SkillRef::reached_by`]。
///
/// 两种「间接被读到」：
///
/// 1. **终点**：另一条健康引用最后落在我身上（`resolved` 和我的 `resolved` 是同一份）。
/// 2. **中途**：另一条引用的链路从我身上经过。
///
/// 都得按**身份**比，不能按字面。链路上每一跳记的是链接里原样写着的东西 ——
/// 本机 `~/.claude/skills` 里那批相对链接逐跳 join 出来是
/// `~/.claude/skills/../../.agents/skills/X`，和实体目录 `~/.agents/skills/X` 是同一份
/// 内容，字符串却完全不同（同 [`identity`] 的理由）。
fn fill_reached_by(refs: &mut [SkillRef]) {
    let ids: Vec<String> = refs
        .iter()
        .map(|r| self_identity(Path::new(&r.path)))
        .collect();
    // 谁能把 agent 递出去，以及递给哪些身份。坏掉的链一个都递不出去 —— 它本来就
    // 读不到东西，拿它当「有人在读」的证据会把删除按钮永远锁死。
    let donors: Vec<(&[String], Vec<String>)> = refs
        .iter()
        .filter(|r| matches!(r.health, RefHealth::Linked | RefHealth::RealDir))
        .map(|r| {
            let mut touched: Vec<String> = r
                .hops
                .iter()
                .map(|h| self_identity(Path::new(&h.to)))
                .collect();
            if let Some(res) = &r.resolved {
                touched.push(res.clone());
            }
            (r.agents.as_slice(), touched)
        })
        .collect();

    let mut filled: Vec<Vec<String>> = Vec::with_capacity(refs.len());
    for (i, r) in refs.iter().enumerate() {
        let mut out = r.agents.clone();
        for (agents, touched) in &donors {
            if !touched.contains(&ids[i]) {
                continue;
            }
            for a in *agents {
                if !out.contains(a) {
                    out.push(a.clone());
                }
            }
        }
        filled.push(out);
    }
    for (r, got) in refs.iter_mut().zip(filled) {
        r.reached_by = got;
    }
}

fn count_badge(skills: &[SkillEntry], badge: SkillBadge) -> usize {
    skills.iter().filter(|s| s.badges.contains(&badge)).count()
}

fn scan_store(id: &StoreIdentity) -> (StoreCandidate, Vec<SkillRef>) {
    let dir = id.path.as_path();
    let store = dir.to_string_lossy().to_string();
    let mut candidate = StoreCandidate {
        path: normalize_windows_path(Path::new(&store))
            .to_string_lossy()
            .to_string(),
        agents: id.agents.clone(),
        scope: id.scope,
        origin: id.origin,
        exists: dir.is_dir(),
        total: 0,
        links: 0,
        real_dirs: 0,
        broken: 0,
        can_be_main: can_be_main(id.scope, id.origin),
    };
    let mut refs = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (candidate, refs);
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if leaf_name(&path.to_string_lossy()).starts_with('.') {
            continue; // `.DS_Store` 之类，不是 skill。
        }
        let Some(r) = classify(&path, &store, &id.agents) else {
            continue;
        };
        candidate.total += 1;
        match &r.health {
            RefHealth::RealDir => candidate.real_dirs += 1,
            RefHealth::Broken(_) | RefHealth::Cyclic | RefHealth::NotADirectory(_) => {
                candidate.links += 1;
                candidate.broken += 1;
            }
            RefHealth::Linked => candidate.links += 1,
            // 受管副本不算链接也不算实体内容：它是别处内容的一份影子，挑主 store 时
            // 两边都不该把它算进去。新旧与否是**角标**的事（见 `badges_for`），不是
            // 这张「哪个目录适合当主 store」的表该回答的。
            RefHealth::ManagedCopy(_)
            | RefHealth::CopyStale(_)
            | RefHealth::CopyEdited(_)
            | RefHealth::CopyDiverged(_)
            | RefHealth::CopyOrphaned(_) => {}
        }
        refs.push(r);
    }
    refs.sort_by(|a, b| a.path.cmp(&b.path));
    (candidate, refs)
}

/// 一个 store 条目是什么。不是目录也不是链接（比如散落的 `README`）就返回 `None`。
fn classify(path: &Path, store: &str, agents: &[String]) -> Option<SkillRef> {
    let mk = |health, hops, resolved| {
        Some(SkillRef {
            path: normalize_windows_path(path).to_string_lossy().to_string(),
            store: normalize_windows_path(Path::new(store))
                .to_string_lossy()
                .to_string(),
            agents: agents.to_vec(),
            // 要看齐同一个名字下的**所有**引用才算得出来，`group()` 里补。
            reached_by: agents.to_vec(),
            health,
            hops,
            resolved,
        })
    };

    // 受管副本要在「是不是链接」之前判：它在磁盘上是实打实的目录，按实体目录处理
    // 就会被当成收编对象，而它其实是别处内容的一份影子。
    if let Some(state) = link::copy_state(path) {
        let original = identity(&state.original);
        let health = match state.sync {
            link::CopySync::InSync => RefHealth::ManagedCopy(original.clone()),
            link::CopySync::SourceChanged => RefHealth::CopyStale(original.clone()),
            link::CopySync::CopyChanged => RefHealth::CopyEdited(original.clone()),
            link::CopySync::Diverged => RefHealth::CopyDiverged(original.clone()),
            link::CopySync::SourceMissing => RefHealth::CopyOrphaned(original.clone()),
        };
        // 源没了的副本解析不到任何「真实内容」——它自己就是仅存的那份，但把它当成
        // 已解析会让详情页画出一条指向不存在目录的链路。
        let resolved = match state.sync {
            link::CopySync::SourceMissing => None,
            _ => Some(original),
        };
        return mk(health, Vec::new(), resolved);
    }

    if link::is_link(path) {
        let (targets, resolved) = link::resolve_chain(path, MAX_HOPS);
        let hops = build_hops(path, &targets);
        if targets.len() >= MAX_HOPS {
            return mk(RefHealth::Cyclic, hops, None);
        }
        return match resolved {
            // 解析到了东西，但不是目录 —— 没有任何内容可用。只判断 `exists()` 会把它
            // 报成健康，用户看到一个「好端端却用不了」的 skill。
            Some(target) if !target.is_dir() => {
                let shown = identity(&target);
                mk(RefHealth::NotADirectory(shown), hops, None)
            }
            Some(dir) => {
                let dir = identity(&dir);
                mk(RefHealth::Linked, hops, Some(dir))
            }
            None => {
                let dead = hops
                    .iter()
                    .find(|h| !h.exists)
                    .map(|h| normalize_windows_path(Path::new(&h.to)).to_string_lossy().to_string())
                    .unwrap_or_else(|| {
                        normalize_windows_path(path).to_string_lossy().to_string()
                    });
                mk(RefHealth::Broken(dead), hops, None)
            }
        };
    }

    if path.is_dir() {
        let me = identity(path);
        return mk(RefHealth::RealDir, Vec::new(), Some(me));
    }
    None
}

/// 一个路径的**身份**：能 canonicalize 就 canonicalize，不能就退回字面。
///
/// 去重必须按身份，不能按字面字符串。本机 `~/.claude/skills` 里 15 条链接是相对的
/// （`../../.agents/skills/X`），逐跳 join 出来的是
/// `~/.claude/skills/../../.agents/skills/X` —— 和实体目录 `~/.agents/skills/X` 是同一份
/// 内容，字符串却完全不同。按字面去重的话，同一个 skill 会被拆成两个 `SkillBody`，
/// 凭空长出一个「重复」角标，主 body 也会选错。
///
/// 展示用的逐跳路径保持原样（见 [`build_hops`]）——用户看到的应该是链接真正写的东西。
fn identity(path: &Path) -> String {
    normalize_windows_path(
        &std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
    )
        .to_string_lossy()
        .to_string()
}

/// 一个路径**自己**的身份 —— 规范化父目录，名字原样接上。
///
/// 不能直接 [`identity`]：链上每一环自己就是个符号链接，`canonicalize` 会一路跟到
/// 终点去。那样整条链算出来的身份全是同一个（终点），彼此互相把 agent 递给对方 ——
/// `~/.claude/skills/X` 会莫名其妙多出 codex / kimicode / pi 这几家根本不读它的。
///
/// 终点那份是实体目录、不是链接，所以对它来说两个函数给的是同一个答案。
fn self_identity(path: &Path) -> String {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return path.to_string_lossy().to_string();
    };
    let path = Path::new(&identity(dir)).join(name);
    normalize_windows_path(&path)
        .to_string_lossy()
        .to_string()
}

/// `resolve_chain` 只给出每一跳的目标，把「从哪来」补回去才画得出链路。
fn build_hops(start: &Path, targets: &[PathBuf]) -> Vec<LinkHop> {
    let mut from = start.to_path_buf();
    let mut out = Vec::with_capacity(targets.len());
    for to in targets {
        out.push(LinkHop {
            from: normalize_windows_path(&from).to_string_lossy().to_string(),
            to: normalize_windows_path(to).to_string_lossy().to_string(),
            exists: to.symlink_metadata().is_ok(),
        });
        from = to.clone();
    }
    out
}

/// 一组同名条目最终落到哪几份实体内容上。多于一份就是「同名重复」。
fn body_paths(refs: &[SkillRef]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in refs {
        // 中间节点（自己是链接、指向下一跳）不是内容，跳过。
        if matches!(r.health, RefHealth::Linked | RefHealth::RealDir) {
            if let Some(resolved) = &r.resolved {
                if Path::new(resolved).is_dir() && !out.contains(resolved) {
                    out.push(resolved.clone());
                }
            }
        }
    }
    out.sort();
    out
}

/// 主 body：被最多条目指向的那份；并列时取文件最多的，再并列按路径定序。
fn primary_body(refs: &[SkillRef], bodies: &[SkillBody]) -> Option<String> {
    bodies
        .iter()
        .max_by_key(|b| {
            let hits = refs
                .iter()
                .filter(|r| r.resolved.as_deref() == Some(b.path.as_str()))
                .count();
            (hits, b.files, std::cmp::Reverse(b.path.clone()))
        })
        .map(|b| b.path.clone())
}

fn badges_for(refs: &[SkillRef], bodies: &[SkillBody]) -> Vec<SkillBadge> {
    let mut out = Vec::new();
    if bodies.len() > 1 {
        out.push(SkillBadge::Duplicate);
    }
    if refs.iter().any(|r| r.hops.len() >= 2) {
        out.push(SkillBadge::TwoHop);
    }
    if refs.iter().any(|r| r.health == RefHealth::Cyclic) {
        out.push(SkillBadge::Cyclic);
    }
    if refs
        .iter()
        .any(|r| matches!(r.health, RefHealth::Broken(_)))
    {
        out.push(SkillBadge::Broken);
    }
    // 四种副本毛病共用一个角标：列表上要回答的是「这条要不要管」，具体是哪一种
    // （源变了 / 副本被改了 / 两边都改了 / 源没了）在详情里逐条说。
    if refs.iter().any(|r| {
        matches!(
            r.health,
            RefHealth::CopyStale(_)
                | RefHealth::CopyEdited(_)
                | RefHealth::CopyDiverged(_)
                | RefHealth::CopyOrphaned(_)
        )
    }) {
        out.push(SkillBadge::CopyStale);
    }
    out.sort();
    out
}

fn build_body(dir: &Path, stores: &[String]) -> SkillBody {
    let (files, mut truncated) = list_files(dir);
    let bytes = files.iter().map(|f| f.bytes).sum();
    let modified = files
        .iter()
        .map(|f| crate::util::mtime_millis(&dir.join(&f.path)))
        .max()
        .filter(|ms| *ms > 0);
    let mut findings: Vec<RiskFinding> = Vec::new();
    for f in &files {
        let scan = risk::scan_file(&f.path, &dir.join(&f.path));
        truncated |= scan.skipped;
        findings.extend(scan.findings);
    }
    let path = normalize_windows_path(dir).to_string_lossy().to_string();
    let store = dir
        .parent()
        .map(|p| normalize_windows_path(p).to_string_lossy().to_string())
        .filter(|p| stores.contains(p));
    SkillBody {
        path,
        store,
        files: files.len(),
        bytes,
        modified,
        risk: risk::highest(&findings),
        truncated,
    }
}

/// 列出 skill 目录里的文件（相对路径）。**不跟随链接**——跟进去就走出这个 skill 了。
///
/// 第二个返回值是「**没列全**」：文件数或深度触了上限，或者某个子目录读不了。上限本身
/// 是必要的（避免有人往 skill 里塞了 node_modules），但**触没触到必须报出来**——
/// 不报的话，一份只扫了前 400 个文件的目录和一份真的干净的目录在 UI 上长得一模一样。
/// 给内置编辑器复用：保存 / 新建之后重列一个 skill 的文件，不必重扫全机器。
pub(super) fn list_body_files(dir: &Path) -> (Vec<SkillFile>, bool) {
    list_files(dir)
}

fn list_files(dir: &Path) -> (Vec<SkillFile>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    walk(dir, dir, 0, &mut out, &mut truncated);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, truncated)
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<SkillFile>, truncated: &mut bool) {
    if depth > MAX_DEPTH || out.len() >= MAX_BODY_FILES {
        *truncated = true;
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        *truncated = true;
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_BODY_FILES {
            *truncated = true;
            return;
        }
        let path = entry.path();
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            // `.git` 不是 skill 的内容，是版本控制的账本。走进去会把成百上千个 object
            // 算进文件数，还会让每个 git 仓库形态的 skill 永远显示「没扫完」。
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            walk(root, &path, depth + 1, out, truncated);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(SkillFile {
                path: rel,
                bytes: meta.len(),
            });
        }
    }
}

fn leaf_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

// ---------------------------------------------------------------------------
// SKILL.md frontmatter
// ---------------------------------------------------------------------------

fn read_frontmatter(dir: &Path) -> Option<SkillFrontmatter> {
    let text = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
    parse_frontmatter(&text)
}

/// 解析 SKILL.md 顶部的 frontmatter。
///
/// 只认这里真正会出现的几种写法：`key: value`、带引号的值、`key:` 后跟缩进的 `- item`
/// 列表、以及 `>` / `|` 块标量。**故意不引第三方 YAML 库**——为一个「读三个字段」的
/// 需求加一条依赖不划算，而全量 YAML 的语义（锚点、别名、多文档）在这儿一条都用不上。
/// 认不出来的字段原样进 `extra`，不猜、不丢。
pub fn parse_frontmatter(text: &str) -> Option<SkillFrontmatter> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let body: Vec<&str> = lines.take_while(|l| l.trim() != "---").collect();

    let mut fm = SkillFrontmatter::default();
    let mut i = 0;
    while i < body.len() {
        let line = body[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // 缩进行属于上一个 key，上面已经吃掉了；这里再遇到就是孤儿，跳过。
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, rest)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let rest = rest.trim();

        let value: String;
        let mut list: Vec<String> = Vec::new();
        if rest == ">" || rest == "|" || rest == ">-" || rest == "|-" {
            let (block, next) = take_indented(&body, i);
            i = next;
            let joined = if rest.starts_with('>') {
                block.join(" ")
            } else {
                block.join("\n")
            };
            value = joined.trim().to_string();
        } else if rest.is_empty() {
            let (block, next) = take_indented(&body, i);
            i = next;
            for item in &block {
                if let Some(v) = item.trim().strip_prefix("- ") {
                    list.push(unquote(v.trim()));
                }
            }
            value = list.join(", ");
        } else {
            value = unquote(rest);
            // `Read, Write, Bash` 这种行内列表。
            list = value
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|s| unquote(s.trim()))
                .filter(|s| !s.is_empty())
                .collect();
        }

        match key.as_str() {
            "name" => fm.name = Some(value),
            "description" => fm.description = Some(value),
            "allowed-tools" | "allowed_tools" => fm.allowed_tools = list,
            _ => fm.extra.push(FrontmatterField { key, value }),
        }
    }
    Some(fm)
}

/// 从 `start` 开始吃掉连续的缩进行，返回（去掉公共缩进的内容，下一行下标）。
fn take_indented(body: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < body.len() {
        let line = body[i];
        if line.trim().is_empty() {
            out.push(String::new());
            i += 1;
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        out.push(line.trim_start().to_string());
        i += 1;
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    (out, i)
}

fn unquote(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() < 2 {
        return trimmed.to_string();
    }
    // 两种引号的转义规则不一样：双引号里是反斜杠，单引号里是把引号写两遍。面板里的
    // 编辑器写回去的就是这两种形状（`src/skillFrontmatter.ts`），读的时候得原样还原，
    // 否则 description 里带个引号，详情页就显示成 `say \"hi\"`。
    if trimmed.starts_with('"') && trimmed.ends_with('"') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some(next @ ('"' | '\\')) => out.push(next),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                },
                _ => out.push(c),
            }
        }
        return out;
    }
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return trimmed[1..trimmed.len() - 1].replace("''", "'");
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "csv-skills-{}-{}-{}",
            tag,
            std::process::id(),
            crate::util::now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 测试用的 store 身份。真实身份由 `store_paths()` 从各 agent 的 `skills_sources()`
    /// 聚出来，这里只要一个能喂给 `scan_store` 的壳。
    fn ident(path: &Path, agents: &[&str]) -> StoreIdentity {
        StoreIdentity {
            path: path.to_path_buf(),
            agents: agents.iter().map(|a| a.to_string()).collect(),
            scope: ConfigScope::User,
            origin: ConfigOrigin::Own,
        }
    }

    fn body(store: &Path, name: &str, skill_md: &str) -> PathBuf {
        let dir = store.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), skill_md).unwrap();
        dir
    }

    fn link(from: &Path, to: &Path) {
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(to, from).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(to, from).unwrap();
    }

    /// 造一棵和 1.1 实测同形状的树：主 store 有实体内容，中间 store 转一手，
    /// agent store 走两跳，另一套 store 里躺着同名的第二份内容，外加一条死链。
    fn fixture(tag: &str) -> (PathBuf, Vec<SkillRef>, Vec<String>) {
        let root = temp_root(tag);
        let main = root.join("skills-manager");
        let mid = root.join("agents");
        let agent = root.join("claude");
        let other = root.join("cc-switch");
        for d in [&main, &mid, &agent, &other] {
            fs::create_dir_all(d).unwrap();
        }

        body(
            &main,
            "alpha",
            "---\nname: alpha\ndescription: from main\n---\n",
        );
        body(&main, "dup", "---\nname: dup\n---\n");
        body(&other, "dup", "---\nname: dup\n---\nother copy\n");

        link(&mid.join("alpha"), &main.join("alpha"));
        link(&agent.join("alpha"), &mid.join("alpha")); // 两跳
        link(&agent.join("dup"), &main.join("dup"));
        link(&agent.join("ghost"), &main.join("ghost")); // 死链

        let stores: Vec<String> = [&main, &mid, &agent, &other]
            .iter()
            .map(|d| d.to_string_lossy().to_string())
            .collect();
        let mut refs = Vec::new();
        for (dir, name) in [
            (&main, &[][..]),
            (&mid, &[][..]),
            (&agent, &["claude"][..]),
            (&other, &[][..]),
        ] {
            refs.extend(scan_store(&ident(dir, name)).1);
        }
        (root, refs, stores)
    }

    fn find<'a>(skills: &'a [SkillEntry], name: &str) -> &'a SkillEntry {
        skills
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no skill named {name}"))
    }

    // ---- 链路判定 ----

    #[test]
    fn a_two_hop_chain_is_flagged_and_resolves_to_the_real_body() {
        // 1.1 ②：22 条链接里 14 条是两跳的。只跟一跳的话，「第一跳在、第二跳断了」
        // 会被报成健康，所以必须解析到底并把整条链留下来。
        let (root, refs, stores) = fixture("twohop");
        let skills = group(refs, &stores);
        let alpha = find(&skills, "alpha");

        assert!(alpha.badges.contains(&SkillBadge::TwoHop));
        let deep = alpha
            .refs
            .iter()
            .find(|r| r.path.contains("/claude/"))
            .unwrap();
        assert_eq!(deep.hops.len(), 2, "claude → agents → skills-manager");
        assert_eq!(deep.health, RefHealth::Linked);
        // 期望值也要走 identity()：macOS 上 `/var` 本身就是指向 `/private/var` 的
        // symlink，canonicalize 之后两边才对得上。
        assert_eq!(
            deep.resolved,
            Some(identity(&root.join("skills-manager").join("alpha")))
        );
        // 内容只有一份，不该报重复。
        assert_eq!(alpha.bodies.len(), 1);
        assert!(!alpha.badges.contains(&SkillBadge::Duplicate));

        fs::remove_dir_all(&root).ok();
    }

    // ---- 谁真的够得着这份内容 ----

    #[test]
    fn a_body_nobody_scans_is_still_read_through_the_links_that_land_on_it() {
        // 真机上报出来的那一幕：`~/.skills-manager/skills/css-animations` 没有任何
        // agent 直接扫 `.skills-manager`，可它是两条链的终点。按 `agents` 判会给它
        // 标上「没有 agent 读它」外加一个删除按钮 —— 点下去两条链一起断。
        let (root, refs, stores) = fixture("reach");
        let skills = group(refs, &stores);
        let alpha = find(&skills, "alpha");

        let body = alpha
            .refs
            .iter()
            .find(|r| r.path.contains("/skills-manager/"))
            .unwrap();
        assert_eq!(body.health, RefHealth::RealDir);
        assert!(body.agents.is_empty(), "nobody scans skills-manager itself");
        assert_eq!(body.reached_by, vec!["claude".to_string()]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_in_the_middle_of_someone_elses_chain_counts_as_read() {
        // `agents/alpha` 自己没有一家直接扫，但 `claude/alpha` 的链从它身上经过。
        // 拆了它，claude 那条整根断掉。
        let (root, refs, stores) = fixture("midhop");
        let skills = group(refs, &stores);
        let mid = find(&skills, "alpha")
            .refs
            .iter()
            .find(|r| r.path.contains("/agents/"))
            .unwrap()
            .clone();

        assert_eq!(mid.health, RefHealth::Linked);
        assert!(mid.agents.is_empty());
        assert_eq!(mid.reached_by, vec!["claude".to_string()]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_body_nothing_points_at_really_is_unread() {
        // 另一半同样重要：`cc-switch/dup` 是同名的第二份内容，没有任何链子落在它身上。
        // 这一条要是也被算成「有人读」，删除按钮就永远不会出现，收编完的残留清不掉。
        let (root, refs, stores) = fixture("orphanbody");
        let skills = group(refs, &stores);
        let dup = find(&skills, "dup");

        let stray = dup
            .refs
            .iter()
            .find(|r| r.path.contains("/cc-switch/"))
            .unwrap();
        assert!(stray.reached_by.is_empty(), "{:?}", stray.reached_by);
        // 而主 store 那份是 claude 那条链的终点。
        let kept = dup
            .refs
            .iter()
            .find(|r| r.path.contains("/skills-manager/"))
            .unwrap();
        assert_eq!(kept.reached_by, vec!["claude".to_string()]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_entry_point_does_not_inherit_the_other_agents_that_share_its_body() {
        // 这一条钉的是反向：`claude/alpha` 和 `codex/alpha` 指着同一份内容，但
        // `claude/alpha` 只有 claude 在读。传染是**沿着链往下**的（链接 → 它指向的东西），
        // 不是横向的。少了这条，`canonicalize` 会把链上每一环都算成终点的身份，
        // 整条链互相递 agent，界面上 `~/.claude/skills/X` 会冒出几家根本不读它的。
        let root = temp_root("noleak");
        let main = root.join("main");
        let claude = root.join("claude");
        let codex = root.join("codex");
        for d in [&claude, &codex] {
            fs::create_dir_all(d).unwrap();
        }
        body(&main, "alpha", "---\nname: alpha\n---\n");
        link(&claude.join("alpha"), &main.join("alpha"));
        link(&codex.join("alpha"), &main.join("alpha"));

        let mut refs = scan_store(&ident(&claude, &["claude"])).1;
        refs.extend(scan_store(&ident(&codex, &["codex"])).1);
        refs.extend(scan_store(&ident(&main, &[])).1);
        let alpha = find(&group(refs, &[]), "alpha").clone();

        let at = |frag: &str| {
            alpha
                .refs
                .iter()
                .find(|r| r.path.contains(frag))
                .unwrap()
                .clone()
        };
        assert_eq!(at("/claude/").reached_by, vec!["claude".to_string()]);
        assert_eq!(at("/codex/").reached_by, vec!["codex".to_string()]);
        // 两条链都落在同一份内容上，那一份才该两家都算上。
        let mut both = at("/main/").reached_by;
        both.sort();
        assert_eq!(both, vec!["claude".to_string(), "codex".to_string()]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_dead_link_hands_out_nothing() {
        // 坏掉的链读不到任何东西，不能拿它当「有人在读」的证据 —— 那会把删除按钮
        // 永远锁死在一份其实没人用的内容上。
        let root = temp_root("deadreach");
        let main = root.join("main");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        body(&main, "alpha", "---\nname: alpha\n---\n");
        // 这条链指向 alpha，但中途那一跳不存在。
        link(&agent.join("alpha"), &root.join("missing").join("alpha"));

        let mut refs = scan_store(&ident(&agent, &["claude"])).1;
        refs.extend(scan_store(&ident(&main, &[])).1);
        let alpha = find(&group(refs, &[]), "alpha").clone();

        let real = alpha
            .refs
            .iter()
            .find(|r| r.health == RefHealth::RealDir)
            .unwrap();
        assert!(real.reached_by.is_empty(), "{:?}", real.reached_by);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reach_is_matched_by_identity_not_by_the_string_the_link_writes() {
        // 相对链接逐跳 join 出来是 `…/agent/../main/alpha`，和实体目录字符串完全不同。
        // 按字面比的话，本机 `~/.claude/skills` 那 15 条相对链接一条都传不出去。
        let root = temp_root("relreach");
        let main = root.join("main");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        body(&main, "alpha", "---\nname: alpha\n---\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink("../main/alpha", agent.join("alpha")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir("..\\main\\alpha", agent.join("alpha")).unwrap();

        let mut refs = scan_store(&ident(&agent, &["claude"])).1;
        refs.extend(scan_store(&ident(&main, &[])).1);
        let alpha = find(&group(refs, &[]), "alpha").clone();

        let real = alpha
            .refs
            .iter()
            .find(|r| r.health == RefHealth::RealDir)
            .unwrap();
        assert_eq!(real.reached_by, vec!["claude".to_string()]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_dead_link_names_the_target_that_is_missing() {
        // 光说「断了」没用，用户要知道断在哪儿才能修。
        let (root, refs, stores) = fixture("dead");
        let skills = group(refs, &stores);
        let ghost = find(&skills, "ghost");

        assert!(ghost.badges.contains(&SkillBadge::Broken));
        assert!(ghost.bodies.is_empty(), "a dead link has no content");
        let missing = root.join("skills-manager").join("ghost");
        assert_eq!(
            ghost.refs[0].health,
            RefHealth::Broken(missing.to_string_lossy().to_string())
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn two_stores_holding_the_same_name_is_a_duplicate() {
        // 1.1 ③：同一个 skill 在磁盘上存两三份，改哪份生效全看链接指向谁。
        let (root, refs, stores) = fixture("dup");
        let skills = group(refs, &stores);
        let dup = find(&skills, "dup");

        assert!(dup.badges.contains(&SkillBadge::Duplicate));
        assert_eq!(dup.bodies.len(), 2);
        // 被链接指到的那份是主 body。
        assert_eq!(
            primary_body(&dup.refs, &dup.bodies),
            Some(identity(&root.join("skills-manager").join("dup")))
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_cycle_is_reported_instead_of_hanging() {
        let root = temp_root("cycle");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        link(&store.join("a"), &store.join("b"));
        link(&store.join("b"), &store.join("a"));

        let (candidate, refs) = scan_store(&ident(&store, &[]));
        assert!(refs.iter().all(|r| r.health == RefHealth::Cyclic));
        assert_eq!(candidate.broken, 2, "a cycle is not a healthy link");

        let skills = group(refs, &[store.to_string_lossy().to_string()]);
        assert!(find(&skills, "a").badges.contains(&SkillBadge::Cyclic));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_managed_copy_is_not_counted_as_a_second_body() {
        // Windows 降级复制出来的副本在磁盘上是实打实的目录。不问一句就会被当成
        // 「第二份内容」，于是每个降级过的 skill 都凭空多出一个「重复」角标。
        let root = temp_root("copy");
        let main = root.join("main");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        let source = body(&main, "alpha", "---\nname: alpha\n---\n");
        link::copy_with_marker(&source, &agent.join("alpha")).unwrap();

        let (_, refs) = scan_store(&ident(&agent, &["claude"]));
        assert!(matches!(refs[0].health, RefHealth::ManagedCopy(_)));

        let mut all = refs;
        all.extend(scan_store(&ident(&main, &[])).1);
        let skills = group(all, &[]);
        let alpha = find(&skills, "alpha");
        assert_eq!(alpha.bodies.len(), 1, "the copy is a shadow, not content");
        assert!(!alpha.badges.contains(&SkillBadge::Duplicate));

        fs::remove_dir_all(&root).ok();
    }

    /// 验收 #8：源改了之后副本**不能**还报健康。
    ///
    /// 这正是 Skills-Manager 的那个坑（方案 2.1）—— 降级成拷贝之后用户在管理器里改了
    /// skill，那个 agent 读到的永远是旧版本，而界面一路显示绿的。
    #[test]
    fn a_copy_whose_source_moved_on_is_not_reported_as_healthy() {
        let root = temp_root("copy-stale");
        let main = root.join("main");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        let source = body(&main, "alpha", "---\nname: alpha\n---\n");
        link::copy_with_marker(&source, &agent.join("alpha")).unwrap();

        // 扫出来先是健康的。
        let (_, refs) = scan_store(&ident(&agent, &["claude"]));
        assert!(matches!(refs[0].health, RefHealth::ManagedCopy(_)));

        // 源改了 —— agent 那边读到的还是旧的。
        fs::write(
            source.join("SKILL.md"),
            "---\nname: alpha\ndescription: now with more words\n---\n",
        )
        .unwrap();

        let (_, refs) = scan_store(&ident(&agent, &["claude"]));
        match &refs[0].health {
            RefHealth::CopyStale(original) => assert!(original.ends_with("alpha")),
            other => panic!("expected CopyStale, got {other:?}"),
        }

        // 角标要跟着出来，否则列表上仍然看不出这条要管。
        let mut all = refs;
        all.extend(scan_store(&ident(&main, &[])).1);
        let skills = group(all, &[]);
        assert!(find(&skills, "alpha").badges.contains(&SkillBadge::CopyStale));

        fs::remove_dir_all(&root).ok();
    }

    /// 副本被就地改过时报的是另一态：拿「源变了」那条路去重拷会把用户的修改抹掉。
    #[test]
    fn a_copy_edited_in_place_gets_its_own_state() {
        let root = temp_root("copy-edited-scan");
        let main = root.join("main");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        let source = body(&main, "alpha", "---\nname: alpha\n---\n");
        let copy = agent.join("alpha");
        link::copy_with_marker(&source, &copy).unwrap();

        fs::write(copy.join("SKILL.md"), "---\nname: alpha\nedited: here\n---\n").unwrap();
        let (_, refs) = scan_store(&ident(&agent, &["claude"]));
        assert!(matches!(refs[0].health, RefHealth::CopyEdited(_)));

        // 两边都改了是第三态，不是「副本被改过」—— 照后者处理会吃掉源上的改动。
        fs::write(source.join("SKILL.md"), "---\nname: alpha\nalso: moved\n---\n").unwrap();
        let (_, refs) = scan_store(&ident(&agent, &["claude"]));
        assert!(matches!(refs[0].health, RefHealth::CopyDiverged(_)));

        fs::remove_dir_all(&root).ok();
    }

    /// 源没了的副本仍然是副本，不能退回成「实体目录」—— 那会让它变成一份「重复内容」。
    #[test]
    fn a_copy_whose_source_vanished_is_still_a_copy() {
        let root = temp_root("copy-orphan-scan");
        let main = root.join("main");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        let source = body(&main, "alpha", "---\nname: alpha\n---\n");
        link::copy_with_marker(&source, &agent.join("alpha")).unwrap();
        fs::remove_dir_all(&main).unwrap();

        let (candidate, refs) = scan_store(&ident(&agent, &["claude"]));
        assert!(matches!(refs[0].health, RefHealth::CopyOrphaned(_)));
        assert_eq!(refs[0].resolved, None, "there is nothing to resolve to");
        assert_eq!(candidate.real_dirs, 0, "a copy is never main-store content");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn store_counts_separate_content_from_links() {
        // 主 store 该按「实体目录」多寡来建议，不是按条目总数 —— 一个全是链接的
        // 目录条目再多也不是内容所在地。
        let (root, _, _) = fixture("counts");
        let (main, _) = scan_store(&ident(&root.join("skills-manager"), &[]));
        let (agent, _) = scan_store(&ident(&root.join("claude"), &["claude"]));

        assert_eq!((main.total, main.real_dirs, main.links), (2, 2, 0));
        assert_eq!((agent.total, agent.real_dirs, agent.links), (3, 0, 3));
        assert_eq!(agent.broken, 1);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_store_is_reported_as_absent_not_empty() {
        let (candidate, refs) = scan_store(&ident(Path::new("/nope/does/not/exist"), &["claude"]));
        assert!(!candidate.exists);
        assert_eq!(candidate.total, 0);
        assert!(refs.is_empty());
    }

    #[test]
    fn loose_files_in_a_store_are_not_skills() {
        let root = temp_root("loose");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("README.md"), "hi").unwrap();
        fs::write(store.join(".DS_Store"), "").unwrap();
        body(&store, "real", "---\nname: real\n---\n");

        let (candidate, refs) = scan_store(&ident(&store, &[]));
        assert_eq!(candidate.total, 1);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].health, RefHealth::RealDir);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn badges_are_ordered_worst_last_so_the_ui_can_take_the_max() {
        let mut badges = vec![
            SkillBadge::Broken,
            SkillBadge::Duplicate,
            SkillBadge::TwoHop,
        ];
        badges.sort();
        assert_eq!(
            badges,
            vec![
                SkillBadge::Duplicate,
                SkillBadge::TwoHop,
                SkillBadge::Broken
            ]
        );
    }

    #[test]
    fn risk_rides_along_with_the_body_it_came_from() {
        let root = temp_root("risk");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let dir = body(&store, "danger", "---\nname: danger\n---\n");
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("scripts/go.sh"), "#!/bin/sh\nrm -rf $HOME\n").unwrap();

        let skills = group(scan_store(&ident(&store, &[])).1, &[]);
        assert_eq!(find(&skills, "danger").risk, RiskLevel::Critical);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn file_listing_stays_inside_the_skill_and_does_not_follow_links() {
        // 跟进去就走出这个 skill 了 —— 统计会把别处的文件算进来，风险扫描更会
        // 把别人的脚本报成这个 skill 的问题。
        let root = temp_root("listing");
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.sh"), "rm -rf /\n").unwrap();
        let dir = body(&root, "skill", "---\nname: skill\n---\n");
        link(&dir.join("escape"), &outside);

        let (files, truncated) = list_files(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "SKILL.md");
        assert!(!truncated, "nothing was skipped here");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn every_deliberately_killed_link_is_detected_none_missed() {
        // 阶段 2 的验收条件之一：手工造一批死链，检出率 100%。
        //
        // 1.1 那 22 个死链是人肉清的，清完还漏了 2 个 —— 漏检才是这个功能要解决的
        // 问题本身，所以这里不抽样，逐条对名字。
        let root = temp_root("killed");
        let main = root.join("main");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        let names: Vec<String> = (0..30).map(|i| format!("s{i:02}")).collect();
        for name in &names {
            body(&main, name, "---\nname: x\n---\n");
            link(&agent.join(name), &main.join(name));
        }

        // 每三个删一个源，链接留在原地 —— 这正是「三套 store 互不知情」的后果。
        let killed: Vec<&String> = names.iter().step_by(3).collect();
        for name in &killed {
            fs::remove_dir_all(main.join(name)).unwrap();
        }

        let mut refs = scan_store(&ident(&agent, &["claude"])).1;
        refs.extend(scan_store(&ident(&main, &[])).1);
        let skills = group(refs, &[]);

        let reported: Vec<&str> = skills
            .iter()
            .filter(|s| s.badges.contains(&SkillBadge::Broken))
            .map(|s| s.name.as_str())
            .collect();
        let expected: Vec<&str> = killed.iter().map(|n| n.as_str()).collect();
        assert_eq!(
            reported, expected,
            "detection must be exact, not approximate"
        );
        assert_eq!(reported.len(), 10);
        // 活着的一条都不能被误报。
        assert_eq!(skills.len() - reported.len(), 20);

        fs::remove_dir_all(&root).ok();
    }

    // ---- review 第一轮挑出来的四条 ----

    #[test]
    fn project_level_skills_are_scanned_when_a_cwd_is_known() {
        // 只扫 user 级的话，本仓库 `.claude/skills/` 里那 7 个 skill（git-push、openspec-*）
        // 会被报成不存在 —— 而它们正在被使用。
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let project_store = repo
            .join(".claude")
            .join("skills")
            .to_string_lossy()
            .to_string();

        let project = scan(Some(repo), &[]);
        // 按 store 断言而不是按名字：`git-push` 在 user 级的第三方 store 里也有一份，
        // 光看名字在不在区分不出它是从哪儿来的。
        let from_project: Vec<&str> = project
            .skills
            .iter()
            .filter(|s| s.refs.iter().any(|r| r.store == project_store))
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            from_project.contains(&"git-push"),
            "project-level skills missing: {from_project:?}"
        );

        // 没有 cwd 就算不出项目级路径，此时不该凭空报出来。
        let user_only = scan(None, &[]);
        assert!(user_only
            .skills
            .iter()
            .all(|s| s.refs.iter().all(|r| r.store != project_store)));
        assert!(user_only
            .stores
            .iter()
            .all(|s| s.scope != ConfigScope::Project));
        assert!(project
            .stores
            .iter()
            .any(|s| s.scope == ConfigScope::Project));
    }

    #[test]
    fn the_summary_counts_what_came_from_a_remote() {
        // 「来自 github」那个角标读的就是这一栏。它和那四个毛病角标不是一类：
        // 一个 clone 完全可以既干净又是 clone，所以不能混进 `count_badge`。
        let scan = scan(None, &[]);
        let by_entry = scan.skills.iter().filter(|s| s.git.is_some()).count();
        assert_eq!(scan.summary.from_git, by_entry);
        // 每一条都得有个能拉的 remote，否则「更新」按钮会点出个错。
        for s in scan.skills.iter().filter(|s| s.git.is_some()) {
            let git = s.git.as_ref().unwrap();
            assert!(!git.remote.is_empty(), "{} has an empty remote", s.name);
            assert!(!git.branch.is_empty(), "{} has an empty branch", s.name);
        }
    }

    #[test]
    fn a_user_added_directory_becomes_a_main_store_candidate() {
        // 「新增主目录」：本机三个内置的共享 store 都不是用户想要的时候，得让他自己指一个。
        let mine = home().join("some-place-i-picked").to_string_lossy().to_string();
        let stores = store_paths(None, std::slice::from_ref(&mine));
        let hit = stores
            .iter()
            .find(|s| s.path.to_string_lossy() == mine)
            .expect("user-added directory must show up as a candidate");
        assert_eq!(hit.scope, ConfigScope::User);
        assert_eq!(hit.origin, ConfigOrigin::Shared);
        assert!(can_be_main(hit.scope, hit.origin));
        // 没加的时候不该凭空出现。
        assert!(store_paths(None, &[])
            .iter()
            .all(|s| s.path.to_string_lossy() != mine));
    }

    #[test]
    fn picking_an_agents_own_directory_does_not_make_it_eligible() {
        // 用户在选目录的对话框里翻到了 `~/.claude/skills`。它装的是链接不是内容，
        // 这一点不因为谁挑了它而改变 —— 「用户说了算」在这儿会把内容搬进一个
        // 随时被 agent 自己重写的目录。
        let claude = home().join(".claude").join("skills");
        let stores = store_paths(None, &[claude.to_string_lossy().to_string()]);
        let hit = stores.iter().find(|s| s.path == claude).unwrap();
        assert_eq!(hit.origin, ConfigOrigin::Own);
        assert!(!can_be_main(hit.scope, hit.origin));
        assert_eq!(stores.iter().filter(|s| s.path == claude).count(), 1);
    }

    #[test]
    fn user_added_paths_are_normalised() {
        let expanded = store_paths(None, &["~/picked-with-tilde".to_string()]);
        assert!(
            expanded
                .iter()
                .any(|s| s.path == home().join("picked-with-tilde")),
            "`~/` must expand — the raw string would be a directory literally named `~`"
        );
        // 相对路径没有参照系，扔掉而不是拼到某个碰巧的 cwd 上。
        let junk = store_paths(None, &["../somewhere".to_string(), "  ".to_string()]);
        assert!(junk
            .iter()
            .all(|s| !s.path.to_string_lossy().contains("somewhere")));
    }

    #[test]
    fn one_directory_can_belong_to_several_agents() {
        // grok 的 `[compat.claude] skills = true` 让它也扫 `~/.claude/skills`。
        // 只记一家就等于告诉用户「这些 skill 在 grok 里用不了」。
        let stores = store_paths(None, &[]);
        let claude_skills = home().join(".claude").join("skills");
        let hit = stores
            .iter()
            .find(|s| s.path == claude_skills)
            .expect("~/.claude/skills must be a candidate store");
        assert!(hit.agents.iter().any(|a| a == "claude"));
        assert!(
            hit.agents.iter().any(|a| a == "grok"),
            "grok reads it through compat: {:?}",
            hit.agents
        );
        // 目录只出现一次，不是每家一条。
        assert_eq!(stores.iter().filter(|s| s.path == claude_skills).count(), 1);
    }

    #[test]
    fn a_truncated_body_never_reports_itself_as_clean() {
        // 上限本身是必要的，但触没触到必须报出来 —— 一份只扫了前 400 个文件的目录和
        // 一份真的干净的目录在 UI 上长得一模一样的话，用户会据此放心。
        let root = temp_root("truncated");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let dir = body(&store, "big", "---\nname: big\n---\n");
        for i in 0..MAX_BODY_FILES + 20 {
            fs::write(dir.join(format!("f{i:04}.txt")), "x").unwrap();
        }

        let skills = group(scan_store(&ident(&store, &[])).1, &[]);
        let big = find(&skills, "big");
        assert!(big.truncated, "hitting the file cap must be reported");
        assert!(big.bodies[0].truncated);
        assert_eq!(big.bodies[0].files, MAX_BODY_FILES);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_text_file_too_big_to_scan_counts_as_truncated() {
        // 超过单文件上限的**文本**：本该扫，没扫成。跳过 != 干净，那段危险脚本可能
        // 正好在里面。
        let root = temp_root("skipped");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let dir = body(&store, "big-text", "---\nname: big-text\n---\n");
        fs::write(dir.join("huge.sh"), "a".repeat(600 * 1024)).unwrap();

        let skills = group(scan_store(&ident(&store, &[])).1, &[]);
        let entry = find(&skills, "big-text");
        assert!(entry.truncated);
        assert_eq!(
            entry.risk,
            RiskLevel::None,
            "risk is a floor, not a verdict"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn binary_assets_and_git_metadata_do_not_cry_wolf() {
        // 「没扫完」这个标记存在的全部意义，是让人在**该**当真的时候当真。skill 目录里
        // 本来就带图片（3.4 的 references / assets），很多还是 git 仓库 —— 这些都算进去
        // 的话它永远亮着，就成了摆设。本机 dm-watch / humanizer 正是这么误报的。
        let root = temp_root("crywolf");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let dir = body(&store, "withassets", "---\nname: withassets\n---\n");
        fs::write(dir.join("logo.png"), [0u8, 1, 2, 3]).unwrap();
        fs::create_dir_all(dir.join(".git").join("objects")).unwrap();
        for i in 0..50 {
            fs::write(dir.join(".git").join("objects").join(format!("o{i}")), "x").unwrap();
        }

        let skills = group(scan_store(&ident(&store, &[])).1, &[]);
        let entry = find(&skills, "withassets");
        assert!(
            !entry.truncated,
            "a png and a .git dir are not a coverage gap"
        );
        assert_eq!(
            entry.bodies[0].files, 2,
            "SKILL.md + logo.png, no git objects"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_relative_link_and_its_target_are_the_same_body_not_a_duplicate() {
        // 本机 `~/.claude/skills` 里 15 条链接是相对的（`../../.agents/skills/X`）。
        // 逐跳 join 出来是 `…/.claude/skills/../../.agents/skills/X`，和实体目录字符串
        // 完全不同 —— 按字面去重就会把同一份内容拆成两个 body，凭空长出「重复」角标。
        let root = temp_root("relative");
        let main = root.join("main");
        let agent = root.join("agent");
        fs::create_dir_all(&agent).unwrap();
        body(&main, "alpha", "---\nname: alpha\n---\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink("../main/alpha", agent.join("alpha")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir("..\\main\\alpha", agent.join("alpha")).unwrap();

        let mut refs = scan_store(&ident(&agent, &["claude"])).1;
        refs.extend(scan_store(&ident(&main, &[])).1);
        let alpha = find(&group(refs, &[]), "alpha").clone();

        assert_eq!(alpha.bodies.len(), 1, "{:?}", alpha.bodies);
        assert!(!alpha.badges.contains(&SkillBadge::Duplicate));
        // 展示用的逐跳路径保持链接真正写的东西，不做规范化。
        let link_ref = alpha
            .refs
            .iter()
            .find(|r| r.path.contains("/agent/"))
            .unwrap();
        assert!(
            link_ref.hops[0].to.contains(".."),
            "hops must show what the link literally says: {}",
            link_ref.hops[0].to
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_pointing_at_a_plain_file_is_not_healthy() {
        // 只判断 `exists()` 会把它报成 Linked，可它没有任何内容可用 —— 用户看到一个
        // 「好端端却用不了」的 skill。git 在 Windows 上把仓库里的 symlink 检出成文本
        // 文件，这条正是那个后果。
        let root = temp_root("notdir");
        let store = root.join("store");
        fs::create_dir_all(&store).unwrap();
        let target = root.join("plain.txt");
        fs::write(&target, "not a skill").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, store.join("alpha")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, store.join("alpha")).unwrap();

        let (candidate, refs) = scan_store(&ident(&store, &["claude"]));
        assert_eq!(refs[0].health, RefHealth::NotADirectory(identity(&target)));
        assert!(refs[0].resolved.is_none(), "there is no body to show");
        assert_eq!(candidate.broken, 1, "it must count as a problem");

        fs::remove_dir_all(&root).ok();
    }

    // ---- 详情 ----

    #[test]
    fn detail_expands_the_primary_body_and_keeps_every_reference() {
        let (root, refs, stores) = fixture("detail");
        let entry = find(&group(refs, &stores), "alpha").clone();
        let detail = build_detail(entry);

        assert_eq!(
            detail.primary,
            Some(identity(&root.join("skills-manager").join("alpha")))
        );
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].path, "SKILL.md");
        assert_eq!(
            detail.frontmatter.unwrap().description.as_deref(),
            Some("from main")
        );
        // 三条引用：主 store 里的实体目录自己，加上中间节点和 agent 那两条链接。
        // 实体目录也算一条 —— 删除时它就是「源」，解链表少了它就没有东西可删。
        let kinds: Vec<&RefHealth> = detail.refs.iter().map(|r| &r.health).collect();
        assert_eq!(detail.refs.len(), 3, "{kinds:?}");
        assert_eq!(
            kinds.iter().filter(|h| ***h == RefHealth::RealDir).count(),
            1
        );
        assert_eq!(
            kinds.iter().filter(|h| ***h == RefHealth::Linked).count(),
            2
        );
        assert_eq!(detail.risk, RiskLevel::None);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn detail_of_a_dead_link_has_no_body_but_still_lists_the_broken_reference() {
        let (root, refs, stores) = fixture("detail-dead");
        let entry = find(&group(refs, &stores), "ghost").clone();
        let detail = build_detail(entry);

        assert!(detail.primary.is_none());
        assert!(detail.files.is_empty());
        assert!(detail.frontmatter.is_none());
        assert_eq!(detail.refs.len(), 1, "the dead link itself must still show");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn asking_for_a_skill_that_does_not_exist_is_an_error_not_an_empty_detail() {
        let err = detail("definitely-not-a-skill-on-this-machine", None, &[]).unwrap_err();
        assert!(
            err.contains("definitely-not-a-skill-on-this-machine"),
            "{err}"
        );
    }

    // ---- frontmatter ----

    #[test]
    fn frontmatter_reads_the_three_fields_the_panel_needs() {
        let fm = parse_frontmatter(
            "---\nname: doc-writer\ndescription: Write docs\nallowed-tools: Read, Write, Bash\n---\n\n# body\n",
        )
        .unwrap();
        assert_eq!(fm.name.as_deref(), Some("doc-writer"));
        assert_eq!(fm.description.as_deref(), Some("Write docs"));
        assert_eq!(fm.allowed_tools, ["Read", "Write", "Bash"]);
    }

    #[test]
    fn frontmatter_accepts_the_yaml_list_form_too() {
        let fm =
            parse_frontmatter("---\nname: x\nallowed-tools:\n  - Read\n  - \"Bash(git:*)\"\n---\n")
                .unwrap();
        assert_eq!(fm.allowed_tools, ["Read", "Bash(git:*)"]);
    }

    #[test]
    fn frontmatter_folds_a_block_scalar_description() {
        let fm = parse_frontmatter(
            "---\nname: x\ndescription: >\n  first line\n  second line\nother: 1\n---\n",
        )
        .unwrap();
        assert_eq!(fm.description.as_deref(), Some("first line second line"));
        assert_eq!(fm.extra[0].key, "other");
    }

    #[test]
    fn frontmatter_keeps_fields_it_does_not_recognise() {
        // 各家 skill 规范还在变，认不出来就丢掉的话，用户在编辑器里一保存就没了。
        let fm = parse_frontmatter("---\nname: x\nmodel: opus\nlicense: MIT\n---\n").unwrap();
        assert_eq!(
            fm.extra,
            vec![
                FrontmatterField {
                    key: "model".into(),
                    value: "opus".into()
                },
                FrontmatterField {
                    key: "license".into(),
                    value: "MIT".into()
                },
            ]
        );
    }

    #[test]
    fn a_quoted_value_comes_back_the_way_yaml_means_it() {
        // 面板里的编辑器写回去的就是这两种形状（`src/skillFrontmatter.ts` 的 `serialize`）：
        // 双引号用反斜杠转义，单引号把引号写两遍。读的时候不还原，详情页就显示成
        // `say \"hi\"`，而用户在表单里看到的又是干净的 —— 同一个值两种样子。
        let fm = parse_frontmatter("---\nname: \"say \\\"hi\\\": now\"\n---\n").unwrap();
        assert_eq!(fm.name.as_deref(), Some("say \"hi\": now"));

        let fm = parse_frontmatter("---\nname: 'it''s'\n---\n").unwrap();
        assert_eq!(fm.name.as_deref(), Some("it's"));
    }

    #[test]
    fn a_lone_backslash_in_a_quoted_value_is_left_alone() {
        // `\p` 不是 YAML 的转义，吞掉那个反斜杠就把 Windows 路径改坏了。
        let fm = parse_frontmatter("---\nname: \"C:\\\\path\"\n---\n").unwrap();
        assert_eq!(fm.name.as_deref(), Some("C:\\path"));
    }

    #[test]
    fn a_file_without_frontmatter_is_not_pretended_to_have_one() {
        assert!(parse_frontmatter("# just a heading\n").is_none());
        assert!(parse_frontmatter("").is_none());
    }

    #[test]
    fn quotes_around_a_value_are_stripped() {
        let fm = parse_frontmatter("---\nname: \"quoted\"\ndescription: 'single'\n---\n").unwrap();
        assert_eq!(fm.name.as_deref(), Some("quoted"));
        assert_eq!(fm.description.as_deref(), Some("single"));
    }

    /// 手工跑：`cargo test --lib report_this_machine -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn report_this_machine() {
        let t0 = std::time::Instant::now();
        let scan = scan(Some(Path::new(env!("CARGO_MANIFEST_DIR"))), &[]);
        let elapsed = t0.elapsed();
        println!("\nscan took {elapsed:?}");
        for s in &scan.stores {
            println!(
                "  {:<50} exists={} total={:<3} links={:<3} real={:<3} broken={}",
                s.path, s.exists, s.total, s.links, s.real_dirs, s.broken
            );
        }
        println!("  suggested main: {:?}", scan.suggested_main);
        println!("  summary: {:?}", scan.summary);
        let files: usize = scan
            .skills
            .iter()
            .flat_map(|k| k.bodies.iter())
            .map(|b| b.files)
            .sum();
        let bytes: u64 = scan
            .skills
            .iter()
            .flat_map(|k| k.bodies.iter())
            .map(|b| b.bytes)
            .sum();
        println!("  bodies scanned: files={files} bytes={bytes}");
        for k in scan.skills.iter().filter(|k| !k.badges.is_empty()) {
            println!(
                "  {:<28} {:?} risk={:?}{}",
                k.name,
                k.badges,
                k.risk,
                if k.truncated { " (partial scan)" } else { "" }
            );
            for r in k.refs.iter().filter(|r| r.hops.len() >= 2) {
                println!("      2-hop: {}", r.path);
            }
        }
    }

    #[test]
    fn the_real_machine_scan_does_not_panic_and_agrees_with_itself() {
        // 这台机器上就有 1.1 描述的那套结构。这条用例不断言具体数字（会变），
        // 只断言不变量：summary 必须和 skills 里的角标对得上。
        let scan = scan(None, &[]);
        assert_eq!(scan.summary.total, scan.skills.len());
        assert_eq!(
            scan.summary.broken,
            count_badge(&scan.skills, SkillBadge::Broken)
        );
        assert_eq!(
            scan.summary.duplicate,
            count_badge(&scan.skills, SkillBadge::Duplicate)
        );
        for skill in &scan.skills {
            for r in &skill.refs {
                assert!(
                    scan.stores.iter().any(|s| s.path == r.store),
                    "{} came from an unlisted store",
                    r.path
                );
            }
        }
        // 项目目录和 agent 自有目录一度混进了主 store 下拉 —— 前者跟着仓库走，
        // 后者是链接落脚的地方，两类都不能装内容。
        for store in &scan.stores {
            if store.scope == ConfigScope::Project {
                assert!(!store.can_be_main, "项目目录 {} 不该能当主 store", store.path);
            }
            if store.origin == ConfigOrigin::Own {
                assert!(
                    !store.can_be_main,
                    "agent 自有目录 {} 不该能当主 store",
                    store.path
                );
            }
        }
        // 兜底目录永远在列表里，哪怕它还不存在 —— 新机器上下拉不能是空的。
        assert!(
            scan.stores.iter().any(|s| s.path == scan.default_main && s.can_be_main),
            "兜底主 store 不在候选里"
        );

        // 建议值要么是一个够格、有内容的候选，要么就是兜底的 `~/.agents/skills`
        // （本机一个候选都没有时）。绝不能是项目目录或某家 agent 的自有目录。
        match scan.stores.iter().find(|s| s.path == scan.suggested_main) {
            Some(store) => {
                assert!(store.can_be_main, "建议了一个不够格当主 store 的目录");
                assert!(
                    store.real_dirs > 0 || scan.suggested_main == scan.default_main,
                    "a store with no content cannot be main"
                );
            }
            // 扫描结果里一定有 `~/.agents/skills`（SHARED_STORES 无条件推进去），
            // 所以走到这儿只可能是兜底值本身。
            None => assert_eq!(scan.suggested_main, scan.default_main),
        }
    }
}
