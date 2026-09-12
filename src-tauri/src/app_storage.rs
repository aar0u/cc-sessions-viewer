use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::types::StorageUsageEntry;

#[derive(Debug, Serialize, Deserialize)]
struct StorageConfig {
    path: PathBuf,
}

fn config_path() -> Result<PathBuf, String> {
    Ok(config_root()?.join("storage.json"))
}

fn config_root() -> Result<PathBuf, String> {
    let base = dirs::config_dir().ok_or_else(|| "Config directory is unavailable".to_string())?;
    Ok(base.join("cc-sessions-viewer"))
}

/// 回收站默认保留 30 天。选这个数而不是「永久」，是因为软删除永不清理会让回收站一路涨，
/// 而绝大多数用户不会主动进设置页。代价是老用户升级后会被删东西 —— 所以删之前必须先弹
/// 一次提示，见 `trash_retention_ack`。
const DEFAULT_TRASH_RETENTION_DAYS: u32 = 30;

fn default_trash_retention_days() -> u32 {
    DEFAULT_TRASH_RETENTION_DAYS
}

/// 磁盘治理相关的偏好。
///
/// 单独一个文件，不并进 `storage.json`：那个文件的语义是「数据目录被搬到哪了」，
/// 「重置数据目录」会把它整个删掉；保留期是独立的一项策略，不该跟着一起没。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Preferences {
    /// 回收站保留天数。`0` = 永久保留。字段缺失时取默认值 30。
    #[serde(default = "default_trash_retention_days")]
    trash_retention_days: u32,
    /// 用户是否已经被告知「回收站会自动清理了」。
    ///
    /// 没确认之前，维护线程**不碰回收站** —— 否则提示就成了事后通知，东西已经删完了。
    #[serde(default)]
    trash_retention_ack: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            trash_retention_days: DEFAULT_TRASH_RETENTION_DAYS,
            trash_retention_ack: false,
        }
    }
}

fn preferences_path() -> Result<PathBuf, String> {
    Ok(config_root()?.join("preferences.json"))
}

fn read_preferences() -> Preferences {
    preferences_path()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_preferences(preferences: &Preferences) -> Result<(), String> {
    let path = preferences_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| "Config directory is unavailable".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(preferences).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

/// 回收站保留天数；`0` = 不自动清理。后端启动时也读它，所以不能放在前端 localStorage。
pub fn trash_retention_days() -> u32 {
    read_preferences().trash_retention_days
}

/// 自动清理是否已经放行。用户还没被告知之前一律返回 false，维护线程据此跳过回收站。
pub fn trash_retention_acknowledged() -> bool {
    read_preferences().trash_retention_ack
}

fn default_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|error| error.to_string())
}

pub fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let default = default_dir(app)?;
    let path = config_path()?;
    if !path.is_file() {
        fs::create_dir_all(&default).map_err(|error| error.to_string())?;
        return Ok(default);
    }
    let config: StorageConfig =
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("Invalid storage configuration: {error}"))?;
    if config.path.as_os_str().is_empty() || !config.path.is_absolute() {
        return Err("Configured data path must be an absolute path".to_string());
    }
    fs::create_dir_all(&config.path).map_err(|error| error.to_string())?;
    Ok(config.path)
}

fn write_config(path: &Path) -> Result<(), String> {
    let config_path = config_path()?;
    let parent = config_path
        .parent()
        .ok_or_else(|| "Config directory is unavailable".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = config_path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&StorageConfig {
        path: path.to_path_buf(),
    })
    .map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, config_path).map_err(|error| error.to_string())
}

fn remove_config() -> Result<(), String> {
    let path = config_path()?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn move_entry(source: &Path, destination: &Path) -> io::Result<()> {
    if !destination.exists() {
        match fs::rename(source, destination) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                copy_entry(source, destination)?;
                if source.is_dir() {
                    fs::remove_dir_all(source)?;
                } else {
                    fs::remove_file(source)?;
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
    if source.is_dir() && destination.is_dir() {
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            move_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::remove_dir(source)
    } else {
        if source.is_file() && destination.is_file() && fs::read(source)? == fs::read(destination)? {
            fs::remove_file(source)
        } else {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("Data conflict at {}", destination.display()),
            ))
        }
    }
}

fn copy_entry(source: &Path, destination: &Path) -> io::Result<()> {
    if source.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(source, destination).map(|_| ())
    }
}

fn migrate(source: &Path, destination: &Path) -> Result<(), String> {
    if source == destination || !source.exists() {
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        return Ok(());
    }
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        move_entry(&entry.path(), &destination.join(entry.file_name()))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// 「日志」这一项统计 / 清理的两个文件。同目录下还有 hook 脚本
/// （`turn-signal-hook.cjs`），各 agent 的配置里按绝对路径引用它，**绝不能删**。
fn log_files() -> Vec<PathBuf> {
    let Some(base) = dirs::data_local_dir() else {
        return Vec::new();
    };
    let root = base.join("cc-sessions-viewer");
    vec![root.join("turn-signals.jsonl"), root.join("panic.log")]
}

fn file_bytes(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn entry(key: &str, path: PathBuf, bytes: u64, clearable: bool) -> StorageUsageEntry {
    StorageUsageEntry {
        key: key.to_string(),
        path: path.to_string_lossy().into_owned(),
        bytes,
        clearable,
    }
}

/// 设置页「存储占用」：逐项列出本 app 会写到磁盘的位置。
///
/// 只列会长大的位置。单文件覆盖写的那些（价格表缓存、书签、worktree 记录）加起来
/// 不到几百 KB，列出来只会让面板变吵。
#[tauri::command(async)]
pub fn storage_usage(app: AppHandle) -> Result<Vec<StorageUsageEntry>, String> {
    let data = data_dir(&app)?;
    let mut entries = vec![
        entry(
            "trash",
            crate::trash::trash_dir(),
            crate::storage_gc::total_bytes(&crate::trash::trash_dir()),
            true,
        ),
        entry(
            "attachments",
            data.join("attachments"),
            crate::storage_gc::total_bytes(&data.join("attachments")),
            true,
        ),
        entry(
            "imageCache",
            data.join("image-cache"),
            crate::storage_gc::total_bytes(&data.join("image-cache")),
            true,
        ),
        // 「发现」面板为了看详情拉下来的浅克隆。删了只是下次打开详情慢 3 秒，
        // 已装的 skill 一个都不受影响 —— 它是加速，不是数据。
        entry(
            "skillRegistry",
            data.join("skill-registry"),
            crate::storage_gc::total_bytes(&data.join("skill-registry")),
            true,
        ),
        entry(
            "desktopPets",
            data.join("desktop-pets"),
            crate::storage_gc::total_bytes(&data.join("desktop-pets")),
            true,
        ),
        // 用户自己放进来的素材 —— 只报大小，不给「清理」按钮。
        entry(
            "backgroundMedia",
            data.join("background-media"),
            crate::storage_gc::total_bytes(&data.join("background-media")),
            false,
        ),
        entry(
            "logs",
            log_files().first().cloned().unwrap_or_default(),
            log_files().iter().map(|path| file_bytes(path)).sum(),
            true,
        ),
    ];

    // Windows 的 WebView2 会在这里堆 Code Cache / GPUCache，是 Tauri 应用「越用越大」
    // 的常见来源。只报大小：这些文件在 webview 运行期间被占用，删一半会留下坏状态，
    // 要清得让用户在应用关闭后自己删。
    #[cfg(target_os = "windows")]
    if let Some(local) = dirs::data_local_dir() {
        let webview = local
            .join("com.wuchao.cc-sessions-viewer")
            .join("EBWebView");
        entries.push(entry(
            "webview",
            webview.clone(),
            crate::storage_gc::total_bytes(&webview),
            false,
        ));
    }

    entries.retain(|item| item.bytes > 0 || item.clearable);
    Ok(entries)
}

/// 清空面板里某一项，返回释放的字节数。只接受 `clearable` 的那几项。
#[tauri::command(async)]
pub fn clear_storage(app: AppHandle, key: String) -> Result<u64, String> {
    let data = data_dir(&app)?;
    match key.as_str() {
        "trash" => {
            let freed = crate::storage_gc::total_bytes(&crate::trash::trash_dir());
            crate::trash::empty()?;
            Ok(freed)
        }
        "attachments" => Ok(clear_dir(&data.join("attachments"))),
        // 内容寻址的缓存，删了下次读会话会重新写出来。
        "imageCache" => Ok(crate::image_cache::clear()),
        // 缓存是加速不是数据：删了下次打开详情重新 clone 一次。
        "skillRegistry" => Ok(crate::tools::registry_git::clear()),
        // 下次打开宠物设置会从内置资源 / Codex asar 重新装一遍。
        "desktopPets" => Ok(clear_dir(&data.join("desktop-pets"))),
        "logs" => {
            let mut freed = 0;
            for path in log_files() {
                freed += file_bytes(&path);
                // 截断而不是删除：turn 信号文件正被 watcher 按 offset 读，
                // 它认得「文件比 offset 短了」这种情况，但删掉会连带 inode 一起换。
                let _ = fs::write(&path, b"");
            }
            Ok(freed)
        }
        other => Err(format!("Storage item is not clearable: {other}")),
    }
}

fn clear_dir(path: &Path) -> u64 {
    let freed = crate::storage_gc::total_bytes(path);
    let _ = fs::remove_dir_all(path);
    let _ = fs::create_dir_all(path);
    freed
}

/// 允许的保留期档位。任意天数都放行会让「30」这种手误变成「3000 天」之类的静默无效值。
const RETENTION_CHOICES: [u32; 4] = [0, 7, 30, 90];

#[tauri::command]
pub fn trash_retention() -> u32 {
    trash_retention_days()
}

#[tauri::command]
pub fn set_trash_retention(days: u32) -> Result<u32, String> {
    if !RETENTION_CHOICES.contains(&days) {
        return Err(format!("Unsupported retention period: {days}"));
    }
    let mut preferences = read_preferences();
    preferences.trash_retention_days = days;
    write_preferences(&preferences)?;
    Ok(days)
}

/// 该不该弹「回收站现在会自动清理」这一次提示。
///
/// 三个条件缺一不可：还没确认过、保留期不是「永久」、并且**确实有条目会被删**。
/// 最后一条是为了别无谓打扰 —— 回收站空的时候这条提示没有任何信息量。
#[tauri::command(async)]
pub fn trash_retention_notice() -> u32 {
    let preferences = read_preferences();
    if preferences.trash_retention_ack || preferences.trash_retention_days == 0 {
        return 0;
    }
    crate::trash::expired_count(preferences.trash_retention_days) as u32
}

/// 用户看过提示了：放行自动清理，并立刻清一次（他刚被告知，不该再等到下次启动）。
#[tauri::command(async)]
pub fn ack_trash_retention(app: AppHandle) -> Result<(), String> {
    let mut preferences = read_preferences();
    preferences.trash_retention_ack = true;
    let days = preferences.trash_retention_days;
    write_preferences(&preferences)?;
    if days > 0 {
        if let Ok(purged) = crate::trash::purge_expired(days) {
            if !purged.is_empty() {
                // 载荷与维护线程那边保持一致：条数，不是文件名列表。
                let _ = tauri::Emitter::emit(&app, "trash:purged", purged.len());
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn data_directory(app: AppHandle) -> Result<String, String> {
    Ok(data_dir(&app)?.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn change_data_directory(app: AppHandle, new_path: String) -> Result<String, String> {
    let destination = PathBuf::from(new_path.trim());
    if destination.as_os_str().is_empty() || !destination.is_absolute() {
        return Err("Data path must be an absolute path".to_string());
    }
    let source = data_dir(&app)?;
    if source != destination {
        let source_cmp = fs::canonicalize(&source).unwrap_or(source.clone());
        let destination_cmp = fs::canonicalize(destination.parent().unwrap_or(&destination))
            .unwrap_or_else(|_| destination.parent().unwrap_or(&destination).to_path_buf())
            .join(destination.file_name().unwrap_or_default());
        if destination_cmp.starts_with(&source_cmp) {
            return Err("Data path cannot be inside the current data directory".to_string());
        }
        migrate(&source, &destination)?;
        write_config(&destination)?;
    }
    Ok(destination.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn reset_data_directory(app: AppHandle) -> Result<String, String> {
    let source = data_dir(&app)?;
    let destination = default_dir(&app)?;
    if source != destination {
        let source_cmp = fs::canonicalize(&source).unwrap_or(source.clone());
        let destination_cmp = fs::canonicalize(destination.parent().unwrap_or(&destination))
            .unwrap_or_else(|_| destination.parent().unwrap_or(&destination).to_path_buf())
            .join(destination.file_name().unwrap_or_default());
        if destination_cmp.starts_with(&source_cmp) {
            return Err("Default data path cannot be inside the current data directory".to_string());
        }
        migrate(&source, &destination)?;
    }
    remove_config()?;
    Ok(destination.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_merges_directories() {
        let root = std::env::temp_dir().join(format!("storage-test-{}", uuid::Uuid::new_v4()));
        let source = root.join("old");
        let destination = root.join("new");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(source.join("a"), b"a").unwrap();
        fs::write(source.join("nested/b"), b"b").unwrap();
        migrate(&source, &destination).unwrap();
        assert_eq!(fs::read(destination.join("a")).unwrap(), b"a");
        assert_eq!(fs::read(destination.join("nested/b")).unwrap(), b"b");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preferences_default_to_thirty_days_and_an_unacknowledged_notice() {
        let preferences: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(preferences.trash_retention_days, 30);
        // 关键：字段缺失时 ack 必须是 false —— 老用户升级后要先看到提示，
        // 在那之前维护线程不许碰回收站。
        assert!(!preferences.trash_retention_ack);

        // 旧文件里没有这些字段，也不能让整份偏好读失败。
        let legacy: Preferences = serde_json::from_str(r#"{"somethingElse":1}"#).unwrap();
        assert_eq!(legacy.trash_retention_days, 30);
        assert!(!legacy.trash_retention_ack);

        // 用户显式选了「永久保留」时，0 必须原样留住，不能被默认值顶掉。
        let explicit: Preferences =
            serde_json::from_str(r#"{"trashRetentionDays":0,"trashRetentionAck":true}"#).unwrap();
        assert_eq!(explicit.trash_retention_days, 0);
        assert!(explicit.trash_retention_ack);
    }

    #[test]
    fn retention_choices_are_the_only_accepted_values() {
        assert!(RETENTION_CHOICES.contains(&0));
        assert!(RETENTION_CHOICES.contains(&30));
        assert!(!RETENTION_CHOICES.contains(&3000));
    }

    #[test]
    fn rejects_file_conflicts() {
        let root = std::env::temp_dir().join(format!("storage-test-{}", uuid::Uuid::new_v4()));
        let source = root.join("old");
        let destination = root.join("new");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("a"), b"a").unwrap();
        fs::write(destination.join("a"), b"b").unwrap();
        assert!(migrate(&source, &destination).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
