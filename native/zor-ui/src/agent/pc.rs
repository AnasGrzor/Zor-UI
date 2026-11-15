//! Full PC access (`buddies.fullAccess`, off by default). When on, the buddy's file tools take a full path on any drive, coding
//! agents start in any folder, and four more tools work: run_command, write_file, delete_file and move_file. Each of those four
//! asks the user before it runs (`policy::approval_for`); a chat asks in the chat. Secret files stay unreadable (`developer`).
//! Deletes and replaced files go to the Recycle Bin, so a yes given by mistake can be undone.

use super::developer;
use crate::conf::Config;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const KEY: &str = "buddies.fullAccess";
pub const OFF: &str = "Full PC access is off. Turn on buddies.fullAccess in config.json to allow it.";
/// How long one command may run before it is stopped.
const COMMAND_LIMIT: Duration = Duration::from_secs(120);

pub fn on(cfg: &Config) -> bool { cfg.get(KEY).as_bool().unwrap_or(false) }

/// Every drive root on the PC (`C:\`, `D:\`, ...).
pub fn drives() -> Vec<PathBuf> { ('A'..='Z').map(|d| PathBuf::from(format!("{d}:\\"))).filter(|p| p.is_dir()).collect() }

/// Where a path given to a file tool lives: (root, path inside it). With full access a full path is taken on its own drive; any
/// other path is inside the developer workspace, as before.
pub fn base(cfg: &Config, p: &str) -> Result<(PathBuf, String), String> {
    let path = Path::new(p.trim());
    if on(cfg) && path.is_absolute() {
        let mut parts = path.components();
        let root: PathBuf = parts.by_ref().take(2).collect(); // the drive and its root folder
        let root = std::fs::canonicalize(&root).map_err(|_| format!("There is no drive {}.", root.display()))?;
        return Ok((root, parts.as_path().to_string_lossy().into_owned()));
    }
    match developer::workspace(cfg) {
        Err(_) if on(cfg) => Err(format!("\"{}\" isn't a full path. Give one such as C:\\Users\\you\\notes.txt.", p.trim())),
        ws => Ok((ws?, p.to_string())),
    }
}

/// The real path of something that exists.
pub fn existing(cfg: &Config, p: &str) -> Result<PathBuf, String> {
    let (root, rel) = base(cfg, p)?;
    developer::resolve(&root, &rel)
}

/// The real path a new file or a move lands on: its folder must exist, the name itself may not.
pub fn target(cfg: &Config, p: &str) -> Result<PathBuf, String> {
    let (root, rel) = base(cfg, p)?;
    let rel = Path::new(&rel);
    let name = rel.file_name().ok_or_else(|| format!("\"{}\" has no file name.", p.trim()))?;
    let parent = rel.parent().filter(|d| !d.as_os_str().is_empty());
    let dir = match parent { Some(d) => developer::resolve(&root, &d.to_string_lossy())?, None => root };
    if !dir.is_dir() { return Err(format!("The folder for \"{}\" isn't there.", p.trim())); }
    Ok(dir.join(name))
}

/// A drive root or a folder right under it (`C:\`, `C:\Windows`, `C:\Users`): never deleted or moved.
fn top_level(p: &Path) -> bool { p.components().filter(|c| matches!(c, Component::Normal(_))).count() <= 1 }

/// The path as the user knows it, without the `\\?\` prefix `canonicalize` adds.
pub fn shown(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") { format!(r"\\{unc}") } else { s.trim_start_matches(r"\\?\").to_string() }
}

/// Checks a call can run before its question is asked, so a refused call asks nothing.
pub fn precheck(cfg: &Config, call: &super::Call) -> Result<(), String> {
    use super::Call::*;
    match call {
        RunCommand(_) | WriteFile { .. } | DeleteFile(_) | MoveFile { .. } if !on(cfg) => Err(OFF.into()),
        WriteFile { path, .. } => target(cfg, path).map(|_| ()),
        DeleteFile(p) => removable(cfg, p).map(|_| ()),
        MoveFile { from, to } => { removable(cfg, from)?; let t = target(cfg, to)?; if t.exists() { Err(format!("\"{}\" is already there.", shown(&t))) } else { Ok(()) } }
        _ => Ok(()),
    }
}

fn removable(cfg: &Config, p: &str) -> Result<PathBuf, String> {
    let real = existing(cfg, p)?;
    if top_level(&real) { return Err(format!("\"{}\" is a drive or a top folder of one, so I won't move or delete it.", shown(&real))); }
    Ok(real)
}

/// Runs one PowerShell command, hidden, in the workspace (or the home folder), and returns its exit code and output.
pub fn run_command(_permit: &super::policy::Cleared, cfg: &Config, command: &str) -> Result<String, String> {
    let cwd = developer::workspace(cfg).unwrap_or_else(|_| crate::conf::home());
    developer::run_program(&cwd, &crate::assist::short(command, 60), "powershell", &["-NoProfile", "-NonInteractive", "-Command", command], COMMAND_LIMIT)
}

/// Writes `text` to the file, making it or replacing it. A replaced file goes to the Recycle Bin first. The size is read back.
pub fn write_file(_permit: &super::policy::Cleared, cfg: &Config, path: &str, text: &str) -> Result<String, String> {
    let t = target(cfg, path)?;
    if t.is_dir() { return Err(format!("\"{}\" is a folder.", shown(&t))); }
    let replaced = t.exists();
    if replaced { recycle(&t)?; }
    std::fs::write(&t, text).map_err(|e| format!("Could not write \"{}\": {e}", shown(&t)))?;
    let len = std::fs::metadata(&t).map(|m| m.len()).unwrap_or(0);
    if len != text.len() as u64 { return Err(format!("Wrote \"{}\" but it holds {len} bytes, not {}.", shown(&t), text.len())); }
    Ok(format!("{} \"{}\" ({len} bytes){}.", if replaced { "Replaced" } else { "Made" }, shown(&t), if replaced { "; the old one is in the Recycle Bin" } else { "" }))
}

/// Moves a file or folder to the Recycle Bin, and checks it is gone.
pub fn delete_file(_permit: &super::policy::Cleared, cfg: &Config, path: &str) -> Result<String, String> {
    let real = removable(cfg, path)?;
    recycle(&real)?;
    Ok(format!("Moved \"{}\" to the Recycle Bin.", shown(&real)))
}

/// Moves or renames a file or folder. Never replaces what is at the destination. Checks the result.
pub fn move_file(_permit: &super::policy::Cleared, cfg: &Config, from: &str, to: &str) -> Result<String, String> {
    let src = removable(cfg, from)?;
    let dst = target(cfg, to)?;
    if dst.exists() { return Err(format!("\"{}\" is already there.", shown(&dst))); }
    std::fs::rename(&src, &dst).map_err(|e| format!("Could not move it ({e}). Moving to another drive needs run_command."))?;
    if dst.exists() && !src.exists() { Ok(format!("Moved \"{}\" to \"{}\".", shown(&src), shown(&dst))) } else { Err("The move could not be confirmed.".into()) }
}

/// Sends a file or folder to the Recycle Bin with no dialog, and checks it is gone.
fn recycle(p: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    // the shell can't take a \\?\ path, and the list ends with two NULs
    let from: Vec<u16> = std::ffi::OsStr::new(&shown(p)).encode_wide().chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW { wFunc: FO_DELETE, pFrom: PCWSTR(from.as_ptr()), fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16, ..Default::default() };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted.as_bool() || p.exists() { return Err(format!("Could not move \"{}\" to the Recycle Bin (code {code}).", shown(p))); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zorui-pc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }
    fn full() -> Config { Config::parse(&json!({ "buddies": { "fullAccess": true } }).to_string()) }

    #[test]
    fn full_paths_work_only_with_full_access() {
        let d = dir("paths");
        std::fs::write(d.join("a.txt"), "hi").unwrap();
        let p = shown(&d.join("a.txt"));
        assert_eq!(existing(&full(), &p).unwrap(), d.join("a.txt"));
        assert!(existing(&Config::parse("{}"), &p).is_err(), "off: only the workspace");
        assert_eq!(target(&full(), &shown(&d.join("sub").join("new.txt"))).unwrap(), d.join("sub").join("new.txt"));
        assert!(target(&full(), &shown(&d.join("missing").join("new.txt"))).is_err(), "its folder must exist");
        assert!(existing(&full(), "a.txt").unwrap_err().contains("full path"), "no workspace: a relative path is refused");
    }

    #[test]
    fn drives_and_their_top_folders_are_never_removed() {
        assert!(top_level(Path::new(r"C:\")));
        assert!(top_level(Path::new(r"\\?\C:\Windows")));
        assert!(!top_level(Path::new(r"C:\Users\me")));
        assert!(precheck(&full(), &super::super::Call::DeleteFile(r"C:\Windows".into())).is_err());
        assert_eq!(precheck(&Config::parse("{}"), &super::super::Call::RunCommand("dir".into())).unwrap_err(), OFF);
    }

    #[test]
    #[ignore = "sends two temp files to the user's Recycle Bin; run with --ignored"]
    fn write_replace_move_and_delete_round_trip() {
        let d = dir("rw");
        let call = super::super::Call::GitStatus;
        let ok = super::super::policy::Cleared::for_test(&call);
        let f = shown(&d.join("note.txt"));
        assert!(write_file(&ok, &full(), &f, "one").unwrap().starts_with("Made"));
        assert!(write_file(&ok, &full(), &f, "two!").unwrap().contains("Recycle Bin"), "the old file goes to the bin");
        assert_eq!(std::fs::read_to_string(d.join("note.txt")).unwrap(), "two!");
        let g = shown(&d.join("sub").join("moved.txt"));
        move_file(&ok, &full(), &f, &g).unwrap();
        assert!(move_file(&ok, &full(), &g, &g).is_err(), "never replaces the destination");
        delete_file(&ok, &full(), &g).unwrap();
        assert!(!d.join("sub").join("moved.txt").exists());
        assert_eq!(shown(Path::new(r"\\?\UNC\srv\share\x")), r"\\srv\share\x");
    }
}
