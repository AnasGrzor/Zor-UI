//! Self-update. Once a day (and on `zorui.exe --update`) asks GitHub for the latest release, and when
//! its tag is newer than this build it downloads the release's zorui-app.zip and runs install.ps1 from
//! it. install.ps1 stops zorui, copies the new files over the old ones, keeps config.json and restarts.
//! Turn it off with "autoUpdate": false in config.json.

use super::common::{get_bytes, get_json, hidden, load_json, root, save_json};
use crate::conf::Config;
use serde_json::{json, Value};
use std::{fs, path::PathBuf, time::Duration};

const LATEST: &str = "https://api.github.com/repos/AnasGrzor/Zor-UI/releases/latest";
/// The release asset that holds the app.
const ASSETS: [&str; 1] = ["zorui-app.zip"];
/// Where the download is saved.
const SAVED: &str = "update.zip";

/// Where the installer's output and errors go.
const LOG: &str = "update.log";
/// CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW. Never add DETACHED_PROCESS (0x8): powershell.exe has no console
/// then and exits at once without running the script, which is why updates up to 0.1.9 never installed.
const SPAWN_FLAGS: u32 = 0x0000_0200 | 0x0800_0000;

/// The PowerShell command that unpacks the downloaded zip in `dir` and runs its install.ps1, logging to `log`.
fn install_script(dir: &std::path::Path, mod_arg: &str, log: &std::path::Path) -> String {
    format!(
        "& {{ try {{ Expand-Archive -Force '{d}\\{SAVED}' '{d}\\x'; & '{d}\\x\\install.ps1' -SkipDeps -Mod {mod_arg} }} catch {{ $_ }} }} *>> '{l}'",
        d = dir.display(), l = log.display())
}

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

/// The app zip's download URL out of a GitHub release object (the first of ASSETS the release has).
fn asset_url(release: &Value) -> Option<&str> {
    let assets = release.get("assets")?.as_array()?;
    ASSETS.iter().find_map(|name| assets.iter().find(|a| a.get("name").and_then(Value::as_str) == Some(name)))?
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
    let url = asset_url(&release).ok_or_else(|| format!("release {tag} has no {}", ASSETS[0]))?;
    let zip = get_bytes(url, &[], 300).ok_or("download failed")?;
    let dir = std::env::temp_dir().join("zorui-update");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(SAVED), zip).map_err(|e| e.to_string())?;
    // Own process group and no window, so install.ps1 killing zorui.exe does not take it down.
    let mut c = hidden("powershell.exe");
    c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &install_script(&dir, mod_arg(cfg), &root().join(LOG))]);
    std::os::windows::process::CommandExt::creation_flags(&mut c, SPAWN_FLAGS);
    c.spawn().map_err(|e| e.to_string())?;
    // Saved only once the installer is running, so a failed start is retried tomorrow.
    save_json(&state_path(), &json!({ "tried": tag }));
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
    fn finds_the_app_asset() {
        let app = json!({ "assets": [
            { "name": "ZorUI-Setup.exe", "browser_download_url": "https://x/setup" },
            { "name": "zorui-app.zip", "browser_download_url": "https://x/app" } ] });
        assert_eq!(asset_url(&app), Some("https://x/app"));
        let old = json!({ "assets": [
            { "name": "ZorUI-Setup.exe", "browser_download_url": "https://x/setup" },
            { "name": "payload.zip", "browser_download_url": "https://x/payload" } ] });
        assert_eq!(asset_url(&old), None, "payload.zip is no longer an app asset");
        assert_eq!(asset_url(&json!({ "assets": [] })), None);
    }

    #[test]
    fn spawn_flags_never_detach_powershell() {
        assert_eq!(SPAWN_FLAGS & 0x8, 0, "DETACHED_PROCESS makes powershell.exe exit without running");
        assert_ne!(SPAWN_FLAGS & 0x0800_0000, 0, "needs CREATE_NO_WINDOW");
    }

    #[test]
    fn script_unpacks_installs_and_logs() {
        let s = install_script(std::path::Path::new(r"C:\t\zorui-update"), "win", std::path::Path::new(r"C:\r\update.log"));
        assert!(s.contains(r"Expand-Archive -Force 'C:\t\zorui-update\update.zip' 'C:\t\zorui-update\x'"));
        assert!(s.contains(r"& 'C:\t\zorui-update\x\install.ps1' -SkipDeps -Mod win"));
        assert!(s.ends_with(r"*>> 'C:\r\update.log'"));
    }

    #[test]
    fn keeps_the_win_modifier() {
        assert_eq!(mod_arg(&Config::parse(r#"{"modKey":"lwin"}"#)), "win");
        assert_eq!(mod_arg(&Config::parse(r#"{"modKey":"alt"}"#)), "alt");
    }
}
