//! Self-update. Once a day (and on `zorui.exe --update`) asks GitHub for the latest release, and when
//! its tag is newer than this build it downloads the release's payload.zip and runs install.ps1 from
//! it. install.ps1 stops zorui, copies the new files over the old ones, keeps config.json and restarts.
//! Turn it off with "autoUpdate": false in config.json.

use super::common::{get_bytes, get_json, hidden, load_json, root, save_json};
use crate::conf::Config;
use serde_json::{json, Value};
use std::{fs, path::PathBuf, time::Duration};

const LATEST: &str = "https://api.github.com/repos/AnasGrzor/Zor-UI/releases/latest";
const ASSET: &str = "payload.zip";

/// "v0.2.1" -> [0, 2, 1]; None when any part is not a number.
fn parse_version(s: &str) -> Option<Vec<u64>> {
    s.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().ok()).collect()
}

/// True when `tag` is a higher version than `current` (a tag that does not parse never is).
fn newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(t), Some(c)) => t > c,
        _ => false,
    }
}

/// The payload.zip download URL out of a GitHub release object.
fn asset_url(release: &Value) -> Option<&str> {
    release.get("assets")?.as_array()?.iter()
        .find(|a| a.get("name").and_then(Value::as_str) == Some(ASSET))?
        .get("browser_download_url")?.as_str()
}

/// install.ps1 -Mod value matching the key the user already runs with.
fn mod_arg(cfg: &Config) -> &'static str { if cfg.str("modKey") == "lwin" { "win" } else { "alt" } }

fn state_path() -> PathBuf { root().join("update.json") }

/// Whether release `tag` should be installed. `force` (the Settings button) ignores the one-attempt-per-tag rule.
fn should_install(tag: &str, current: &str, tried: Option<&str>, force: bool) -> bool {
    newer(tag, current) && (force || tried != Some(tag))
}

/// One check. Ok(Some(tag)) when an install was started, Ok(None) when up to date or already tried.
pub fn check(cfg: &Config, force: bool) -> Result<Option<String>, String> {
    let release = get_json(LATEST, &[("Accept", "application/vnd.github+json")]).ok_or("could not reach GitHub")?;
    let tag = release.get("tag_name").and_then(Value::as_str).ok_or("no release found")?;
    // One attempt per tag: a release built without a version bump would otherwise reinstall every day.
    let state = load_json(&state_path(), json!({}));
    if !should_install(tag, env!("CARGO_PKG_VERSION"), state.get("tried").and_then(Value::as_str), force) { return Ok(None); }
    let url = asset_url(&release).ok_or_else(|| format!("release {tag} has no {ASSET}"))?;
    let zip = get_bytes(url, &[], 300).ok_or("download failed")?;
    let dir = std::env::temp_dir().join("zorui-update");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(ASSET), zip).map_err(|e| e.to_string())?;
    save_json(&state_path(), &json!({ "tried": tag }));
    // Detached from this process: install.ps1 kills zorui.exe, so it must not be our child's child.
    let script = format!(
        "Expand-Archive -Force '{d}\\{ASSET}' '{d}\\x'; & '{d}\\x\\install.ps1' -SkipDeps -Mod {m}",
        d = dir.display(), m = mod_arg(cfg));
    let mut c = hidden("powershell.exe");
    c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script]);
    std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    c.spawn().map_err(|e| e.to_string())?;
    Ok(Some(tag.to_string()))
}

/// Background loop for the running app: first check a minute after start, then daily.
pub fn spawn() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(60));
        loop {
            let cfg = Config::load();
            if cfg.get("autoUpdate").as_bool().unwrap_or(true) { let _ = check(&cfg, false); }
            std::thread::sleep(Duration::from_secs(24 * 3600));
        }
    });
}

/// `zorui.exe --update`: checks now, prints what happened.
pub fn cli() -> i32 {
    match check(&Config::load(), false) {
        Ok(Some(tag)) => { println!("updating to {tag}"); 0 }
        Ok(None) => { println!("up to date ({})", env!("CARGO_PKG_VERSION")); 0 }
        Err(e) => { eprintln!("update failed: {e}"); 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("v0.10.0", "0.9.5"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("v0.0.9", "0.1.0"));
        assert!(!newer("nightly", "0.1.0"));
    }

    #[test]
    fn force_ignores_the_tried_tag() {
        assert!(should_install("v0.2.0", "0.1.0", None, false));
        assert!(!should_install("v0.2.0", "0.1.0", Some("v0.2.0"), false));
        assert!(should_install("v0.2.0", "0.1.0", Some("v0.2.0"), true));
        assert!(!should_install("v0.1.0", "0.1.0", None, true));
    }

    #[test]
    fn finds_the_payload_asset() {
        let r = json!({ "assets": [
            { "name": "ZorUI-Setup.exe", "browser_download_url": "https://x/setup" },
            { "name": "payload.zip", "browser_download_url": "https://x/payload" } ] });
        assert_eq!(asset_url(&r), Some("https://x/payload"));
        assert_eq!(asset_url(&json!({ "assets": [] })), None);
    }

    #[test]
    fn keeps_the_win_modifier() {
        assert_eq!(mod_arg(&Config::parse(r#"{"modKey":"lwin"}"#)), "win");
        assert_eq!(mod_arg(&Config::parse(r#"{"modKey":"alt"}"#)), "alt");
    }
}
