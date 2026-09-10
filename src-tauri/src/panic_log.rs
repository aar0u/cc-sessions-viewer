use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::panic;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn install() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = write_panic(info);
        default_hook(info);
    }));
}

/// panic.log 的大小上限。纯追加、从不清理，长期使用会一直长。超限就只保留尾部
/// 一半 —— 最近的崩溃才有诊断价值。
const MAX_LOG_BYTES: u64 = 256 * 1024;

fn rotate_if_large(path: &std::path::Path) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.len() <= MAX_LOG_BYTES {
        return;
    }
    // 失败不致命：轮转不了就照旧继续追加。
    if let Some(tail) = crate::util::read_tail_text(path, MAX_LOG_BYTES / 2) {
        let _ = fs::write(path, tail);
    }
}

fn write_panic(info: &panic::PanicHookInfo<'_>) -> io::Result<()> {
    let directory = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("cc-sessions-viewer");
    fs::create_dir_all(&directory)?;

    let path = directory.join("panic.log");
    rotate_if_large(&path);

    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    writeln!(
        file,
        "unix_ms={timestamp_ms}\n{info}\nbacktrace:\n{}\n",
        Backtrace::force_capture()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(name: &str, body: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("cssv-panic-{}-{name}.log", std::process::id()));
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn rotate_if_large_leaves_a_small_log_untouched() {
        let path = temp_log("small", b"line one\nline two\n");
        rotate_if_large(&path);
        assert_eq!(fs::read_to_string(&path).unwrap(), "line one\nline two\n");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rotate_if_large_keeps_the_newest_half_of_an_oversized_log() {
        // 每行都可辨认，轮转后应只剩尾部一半，且首行不是被截断的半行。
        let mut body = String::new();
        let mut line = 0u32;
        while body.len() as u64 <= MAX_LOG_BYTES * 2 {
            body.push_str(&format!("panic-{line:08}\n"));
            line += 1;
        }
        let path = temp_log("large", body.as_bytes());
        rotate_if_large(&path);

        let rotated = fs::read_to_string(&path).unwrap();
        assert!(rotated.len() as u64 <= MAX_LOG_BYTES / 2);
        assert!(rotated.ends_with(&format!("panic-{:08}\n", line - 1)));
        let first = rotated.lines().next().unwrap();
        assert!(
            first.len() == "panic-00000000".len(),
            "轮转后的首行不能是被截断的半行: {first:?}"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rotate_if_large_ignores_a_missing_file() {
        let missing = std::env::temp_dir().join("cssv-panic-does-not-exist.log");
        let _ = fs::remove_file(&missing);
        rotate_if_large(&missing);
        assert!(!missing.exists());
    }
}
