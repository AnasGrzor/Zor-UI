//! The active project: the folder of the project the window in front is working on. The title names it ("main.rs - zor-ui - Visual
//! Studio Code" names zor-ui). A project is a folder directly under one of the project roots that has a project marker (.git,
//! Cargo.toml, package.json, pyproject.toml, go.mod). The roots are Documents and Desktop, or `buddies.projectRoots` in the config.
//! Only the title is read to find it. Its top-level names are read only when a request asks about the project.

use crate::conf::Config;
use std::path::{Path, PathBuf};
use windows::{core::GUID, Win32::System::Com::CoTaskMemFree, Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT}};

const MARKERS: [&str; 5] = [".git", "Cargo.toml", "package.json", "pyproject.toml", "go.mod"];

/// Where Windows keeps a known folder (Desktop, Documents), OneDrive included. None when Windows doesn't say.
fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let dir = PathBuf::from(p.to_string().unwrap_or_default());
        CoTaskMemFree(Some(p.0 as *const _));
        Some(dir)
    }
}

/// The folders a project is looked for in: `buddies.projectRoots` when it is set, else Documents and Desktop as Windows keeps them
/// (a OneDrive folder counts).
pub fn roots(cfg: &Config) -> Vec<PathBuf> {
    let set: Vec<PathBuf> = cfg.get("buddies.projectRoots").as_array().into_iter().flatten()
        .filter_map(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(PathBuf::from).collect();
    if !set.is_empty() { return set; }
    let home = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
    vec![known_folder(&FOLDERID_Documents).unwrap_or_else(|| home.join("Documents")), known_folder(&FOLDERID_Desktop).unwrap_or_else(|| home.join("Desktop"))]
}

fn is_project(dir: &Path) -> bool { MARKERS.iter().any(|m| dir.join(m).exists()) }

/// The project a window title names: a part of the title ("main.rs", "zor-ui", "Visual Studio Code") equal to the name of a
/// project folder under one of `roots`. The first match wins.
pub fn from_title(title: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let title = title.replace(" — ", " - ").replace(" | ", " - ");
    let parts: Vec<String> = title.split(" - ").map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if p.is_dir() && parts.contains(&name) && is_project(&p) { return Some(p); }
        }
    }
    None
}

/// The project as the AI reads it: its name and folder, whether it is a git repository, and its top-level names (at most 12).
pub fn describe(dir: &Path) -> String {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut top: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && n != "target" && n != "node_modules").collect();
    top.sort();
    top.truncate(12);
    let git = if dir.join(".git").exists() { "yes" } else { "no" };
    format!("Active project: {name} (folder {}). Git repository: {git}. Top level: {}.", dir.display(), top.join(", "))
}

/// Asks about the project or the code: words that only make sense about the folder being worked in.
pub fn wants_project(said: &str) -> bool {
    said.split(|c: char| !c.is_alphanumeric()).any(|w| ["project", "code", "repo", "repository", "build", "compile", "test", "tests", "bug", "bugs", "error", "errors", "workspace", "folder", "here"].contains(&w.to_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zorui-project-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zor-ui")).unwrap();
        std::fs::write(dir.join("zor-ui").join("Cargo.toml"), "x").unwrap();
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        dir
    }

    #[test]
    fn the_title_names_the_project_and_a_folder_without_a_marker_is_not_one() {
        let root = temp("title");
        let roots = [root.clone()];
        assert_eq!(from_title("main.rs - zor-ui - Visual Studio Code", &roots), Some(root.join("zor-ui")));
        assert_eq!(from_title("ZOR-UI — Orca", &roots), Some(root.join("zor-ui")), "case and dash do not matter");
        assert_eq!(from_title("notes - Visual Studio Code", &roots), None, "no project marker, no project");
        assert_eq!(from_title("Inbox - Chrome", &[]), None, "no roots, no project");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_description_names_the_folder_git_and_the_top_level() {
        let root = temp("describe");
        let project = root.join("zor-ui");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(project.join("target")).unwrap();
        std::fs::write(project.join("README.md"), "x").unwrap();
        let text = describe(&project);
        assert!(text.starts_with("Active project: zor-ui"), "{text}");
        assert!(text.contains("Git repository: yes"), "{text}");
        assert!(text.contains("Top level: Cargo.toml, README.md"), "hidden and build folders are left out: {text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_roots_come_from_settings_or_the_user_folders() {
        assert_eq!(roots(&Config(json!({ "buddies": { "projectRoots": ["C:\\work"] } }))), [PathBuf::from("C:\\work")]);
        let default = roots(&Config(json!({})));
        assert_eq!(default.len(), 2, "Documents and Desktop");
        assert!(default[0].ends_with("Documents") && default[1].ends_with("Desktop"));
    }

    #[test]
    fn only_project_words_ask_for_the_project() {
        assert!(wants_project("build the active project") && wants_project("why does the code fail"));
        assert!(wants_project("are there any errors here?"));
        assert!(!wants_project("what time is it") && !wants_project("turn the volume up"));
    }
}
