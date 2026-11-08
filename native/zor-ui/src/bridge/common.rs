//! Helpers the ported commands share (end4_bridge.py's out / load_json / save_json / config / http).
#![allow(dead_code)]

use serde_json::{json, Value};
use std::{os::windows::process::CommandExt, path::{Path, PathBuf}, process::Command};

pub const NO_WINDOW: u32 = 0x0800_0000;

/// %LOCALAPPDATA%\zor-ui (ROOT in end4_bridge.py): state.json, secrets, caches.
pub fn root() -> PathBuf { crate::conf::data_dir() }
/// The pack folder: config.json, theme.json, wallpaper.jpg.
pub fn pack() -> PathBuf { crate::conf::pack_dir() }
/// The installed scripts folder (football.py, terminal templates...).
pub fn scripts(cfg: &crate::conf::Config) -> PathBuf {
    let d = cfg.str("scriptsDir");
    if d.is_empty() { root().join("scripts") } else { PathBuf::from(d) }
}

/// A JSON file, or `default` if it is missing or unreadable (a UTF-8 BOM is fine).
pub fn load_json(path: &Path, default: Value) -> Value {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or(default)
}

/// Writes JSON (pretty, UTF-8) through a temp file, so a crash never leaves half a file.
pub fn save_json(path: &Path, v: &Value) -> bool {
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default()).is_ok() && std::fs::rename(&tmp, path).is_ok()
}

/// {"ok": false, "error": msg}.
pub fn err(msg: impl std::fmt::Display) -> Value { json!({ "ok": false, "error": msg.to_string() }) }

/// A hidden child process (no console window flashing up).
pub fn hidden(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(NO_WINDOW);
    c
}

/// GET through curl.exe (this process has no TLS stack): body bytes on a 2xx, else None.
pub fn get_bytes(url: &str, headers: &[(&str, &str)], timeout_s: u32) -> Option<Vec<u8>> {
    let mut c = hidden("curl.exe");
    c.args(["-s", "-f", "-L", "--max-time", &timeout_s.to_string(), "-A", "zor-ui"]);
    for (k, v) in headers { c.args(["-H", &format!("{k}: {v}")]); }
    let out = c.arg(url).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// GET JSON through curl.exe.
pub fn get_json(url: &str, headers: &[(&str, &str)]) -> Option<Value> {
    serde_json::from_slice(&get_bytes(url, headers, 20)?).ok()
}
