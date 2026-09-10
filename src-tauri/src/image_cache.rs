// 会话图片的内容寻址磁盘缓存。
//
// 问题：transcript 里的图片是内联的 `data:<mime>;base64,<...>`。一张 2000×1500 的截图
// 是 ~3 MB 字节、~4 MB base64 文本，而它要经过：JSONL 解析 → serde 序列化 → IPC
// 传输 → JS 字符串 → DOM 属性，最后 WebKit 还要为渲染中的每张图各留一份解码位图。
// 一份带十几张截图的会话，光图片就能吃掉几百 MB，而且只要 tab 还开着就一直占着。
//
// 修法：把 base64 解出来写进 `<data_dir>/image-cache/<sha256>.<ext>`，`image_src` 改成
// 那个文件的绝对路径。前端已有的 `imageSrcUrl` 会把非 `data:`/`http` 的值走
// `convertFileSrc` —— Codex / 剪贴板图片本来就是这么显示的，所以显示侧零改动。
// 之后图片字节只存在于磁盘和 WebKit 的图片缓存里，JS 堆里只剩一条路径字符串。
//
// 内容寻址顺带做了去重：同一张图在多个会话里重复出现（fork、continue、复制粘贴）
// 只占一份磁盘。
//
// 目录容量由 `prune` 按 500 MB 封顶（`storage_gc`，从最旧的开始删）。删掉是安全的：
// 文件名是内容哈希，下次读那个会话会照原样重新写出来 —— 这里存的东西全都可再生。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::Engine;
use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::types::Msg;

/// 小于这个大小的图片保持内联。落盘要付一次 open/write/rename 和一次 `asset://` 请求，
/// 对几 KB 的小图标不划算，而它们本来也不是内存问题的来源。
pub const INLINE_MAX_BYTES: usize = 32 * 1024;

static CACHE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 在 app setup 里调一次。拿不到数据目录时缓存整体停用，图片照旧走内联 —— 功能不受影响，
/// 只是没有这一层优化。
pub fn init(app: &AppHandle) {
    let dir = crate::app_storage::data_dir(app)
        .ok()
        .map(|root| root.join("image-cache"))
        .filter(|dir| fs::create_dir_all(dir).is_ok());
    let _ = CACHE_DIR.set(dir);
}

fn cache_dir() -> Option<&'static Path> {
    CACHE_DIR.get()?.as_deref()
}

/// 缓存目录的总量上限。
pub const MAX_BYTES: u64 = 500 * 1024 * 1024;

/// 把目录压回上限以内。不设年龄上限 —— 一张两年前的截图只要那个会话还在，
/// 下次打开还会用到；真正的判据只有「总量」。
pub fn prune() -> crate::storage_gc::Pruned {
    let Some(dir) = cache_dir() else {
        return crate::storage_gc::Pruned::default();
    };
    crate::storage_gc::prune(
        dir,
        crate::storage_gc::Policy {
            max_age: None,
            max_bytes: Some(MAX_BYTES),
        },
        std::time::SystemTime::now(),
    )
}

/// 整个清空（存储面板的「清理」按钮）。返回释放的字节数。
pub fn clear() -> u64 {
    let Some(dir) = cache_dir() else {
        return 0;
    };
    let freed = crate::storage_gc::total_bytes(dir);
    let _ = fs::remove_dir_all(dir);
    let _ = fs::create_dir_all(dir);
    freed
}

/// data URL 的 mime → 文件扩展名。认不出来的一律不落盘（扩展名决定前端能不能当图片
/// 加载，也决定 `util::is_image_file` 的判断），保持内联更安全。
fn extension_for(mime: &str) -> Option<&'static str> {
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/svg+xml" => Some("svg"),
        "image/avif" => Some("avif"),
        "image/heic" => Some("heic"),
        _ => None,
    }
}

/// 拆 `data:<mime>[;param]*;base64,<payload>`。只认 base64 变体 —— 非 base64 的 data URL
/// （百分号编码的 SVG 之类）本来就不大，留着内联。
fn split_data_url(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("data:")?;
    let comma = rest.find(',')?;
    let (meta, payload) = rest.split_at(comma);
    let payload = &payload[1..];
    let mut parts = meta.split(';');
    let mime = parts.next()?;
    if !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return None;
    }
    Some((mime, payload))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 把一个 data URL 落盘到指定目录，返回缓存文件的绝对路径。任何一步失败都返回 None，
/// 调用方保持原来的内联值 —— 这一层永远只做优化，不改变可见行为。
fn store_in(dir: &Path, src: &str) -> Option<String> {
    let (mime, payload) = split_data_url(src)?;
    let ext = extension_for(mime)?;
    // 先按 base64 文本长度粗筛，避免为小图白解一次码。base64 比原字节大约 4/3。
    if payload.len() / 4 * 3 < INLINE_MAX_BYTES {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .ok()?;
    if bytes.len() < INLINE_MAX_BYTES {
        return None;
    }

    let digest = Sha256::digest(&bytes);
    let target = dir.join(format!("{}.{ext}", hex(&digest)));
    // 内容寻址：同名即同内容，已经在了就直接用，连写都省了。
    if fs::metadata(&target).is_ok_and(|meta| meta.len() == bytes.len() as u64) {
        return Some(target.to_string_lossy().into_owned());
    }
    write_atomically(&target, &bytes)?;
    Some(target.to_string_lossy().into_owned())
}

/// 先写同目录临时文件再 rename。rename 在同一文件系统上是原子的，所以 webview 永远
/// 不会读到一个写了一半的图。
fn write_atomically(target: &Path, bytes: &[u8]) -> Option<()> {
    let temp = target.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut file = fs::File::create(&temp).ok()?;
        if file.write_all(bytes).is_err() || file.flush().is_err() {
            let _ = fs::remove_file(&temp);
            return None;
        }
    }
    if fs::rename(&temp, target).is_err() {
        let _ = fs::remove_file(&temp);
        return None;
    }
    Some(())
}

/// 把一整份会话里够大的内联图片换成缓存文件路径。就地改写，认不出 / 写不了的原样留着。
/// 缓存不可用（拿不到数据目录）时整体是个空操作。
pub fn externalize(msgs: &mut [Msg]) {
    let Some(dir) = cache_dir() else { return };
    externalize_in(dir, msgs);
}

fn externalize_in(dir: &Path, msgs: &mut [Msg]) {
    for msg in msgs {
        for block in &mut msg.blocks {
            let Some(src) = block.image_src.as_deref() else {
                continue;
            };
            if !src.starts_with("data:") {
                continue;
            }
            if let Some(path) = store_in(dir, src) {
                block.image_src = Some(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Block;

    fn data_url(mime: &str, bytes: &[u8]) -> String {
        format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }

    fn image_msg(src: &str) -> Msg {
        Msg {
            role: "user".into(),
            blocks: vec![Block {
                kind: "image".into(),
                image_src: Some(src.to_string()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// 每个测试用自己的目录。`externalize_in` 显式收目录，正是为了不必去碰那个
    /// 只能设置一次的全局 OnceLock —— 并行跑的测试会互相抢它。
    fn temp_cache_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cssv-image-cache-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn split_data_url_requires_the_base64_marker() {
        assert_eq!(
            split_data_url("data:image/png;base64,QUJD"),
            Some(("image/png", "QUJD"))
        );
        assert_eq!(
            split_data_url("data:image/png;charset=utf-8;base64,QUJD"),
            Some(("image/png", "QUJD"))
        );
        // 百分号编码的 data URL 不认 —— 它们本来也不大。
        assert!(split_data_url("data:image/svg+xml,%3Csvg%2F%3E").is_none());
        assert!(split_data_url("https://example.com/a.png").is_none());
        assert!(split_data_url("data:image/png;base64").is_none());
    }

    #[test]
    fn extension_for_only_accepts_known_image_types() {
        assert_eq!(extension_for("image/PNG"), Some("png"));
        assert_eq!(extension_for("image/jpeg"), Some("jpg"));
        // 认不出来就不落盘：扩展名决定前端能不能把它当图片加载。
        assert!(extension_for("application/octet-stream").is_none());
        assert!(extension_for("").is_none());
    }

    #[test]
    fn hex_pads_every_byte_to_two_digits() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn externalize_leaves_small_images_inline() {
        let dir = temp_cache_dir("small");
        let src = data_url("image/png", &vec![7u8; INLINE_MAX_BYTES - 1]);
        let mut msgs = vec![image_msg(&src)];
        externalize_in(&dir, &mut msgs);
        assert_eq!(msgs[0].blocks[0].image_src.as_deref(), Some(src.as_str()));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "小图不该落盘");
    }

    #[test]
    fn externalize_writes_a_big_image_and_swaps_in_its_path() {
        let dir = temp_cache_dir("big");
        let bytes = vec![3u8; INLINE_MAX_BYTES * 2];
        let mut msgs = vec![image_msg(&data_url("image/png", &bytes))];
        externalize_in(&dir, &mut msgs);

        let path = msgs[0].blocks[0].image_src.clone().unwrap();
        assert!(!path.starts_with("data:"), "应替换成文件路径: {path}");
        assert!(path.starts_with(dir.to_string_lossy().as_ref()));
        assert!(path.ends_with(".png"));
        assert_eq!(fs::read(&path).unwrap(), bytes, "落盘内容必须逐字节一致");
    }

    #[test]
    fn the_same_image_lands_on_one_file_no_matter_how_often_it_appears() {
        let dir = temp_cache_dir("dedupe");
        let bytes = vec![9u8; INLINE_MAX_BYTES * 2];
        let src = data_url("image/png", &bytes);
        let mut msgs = vec![image_msg(&src), image_msg(&src), image_msg(&src)];
        externalize_in(&dir, &mut msgs);

        let paths: Vec<String> = msgs
            .iter()
            .map(|m| m.blocks[0].image_src.clone().unwrap())
            .collect();
        assert_eq!(paths[0], paths[1]);
        assert_eq!(paths[1], paths[2]);
    }

    #[test]
    fn different_bytes_land_on_different_files() {
        let dir = temp_cache_dir("distinct");
        let a = data_url("image/png", &vec![1u8; INLINE_MAX_BYTES * 2]);
        let b = data_url("image/png", &vec![2u8; INLINE_MAX_BYTES * 2]);
        let mut msgs = vec![image_msg(&a), image_msg(&b)];
        externalize_in(&dir, &mut msgs);
        assert_ne!(
            msgs[0].blocks[0].image_src,
            msgs[1].blocks[0].image_src,
            "内容不同必须落到不同文件，否则会串图"
        );
    }

    #[test]
    fn externalize_leaves_remote_and_local_sources_untouched() {
        let dir = temp_cache_dir("passthrough");
        let mut msgs = vec![
            image_msg("https://example.com/a.png"),
            image_msg("/Users/someone/clipboard-1.png"),
        ];
        externalize_in(&dir, &mut msgs);
        assert_eq!(
            msgs[0].blocks[0].image_src.as_deref(),
            Some("https://example.com/a.png")
        );
        assert_eq!(
            msgs[1].blocks[0].image_src.as_deref(),
            Some("/Users/someone/clipboard-1.png")
        );
    }

    #[test]
    fn an_unknown_media_type_stays_inline() {
        let dir = temp_cache_dir("unknown-mime");
        let src = data_url("application/pdf", &vec![5u8; INLINE_MAX_BYTES * 2]);
        let mut msgs = vec![image_msg(&src)];
        externalize_in(&dir, &mut msgs);
        assert_eq!(msgs[0].blocks[0].image_src.as_deref(), Some(src.as_str()));
    }
}
