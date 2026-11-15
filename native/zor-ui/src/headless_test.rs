//! CLAUDE.md, "Everything runs headless": every child process this repo starts must set the
//! no-window flag, or it opens a console on the user's PC. This test reads the source of both
//! crates and fails on any `Command::new(` whose statement does not set the flag.
//!
//! The check is textual. It looks at the spawn line and the lines after it, up to the next
//! `Command::new(`, for a flag name. A flag set on a later line that belongs to a different
//! statement can hide a missing one, so keep the flag next to its spawn.
use std::path::{Path, PathBuf};

/// Tokens that count as the no-window flag: the constant names and the raw value.
const FLAGS: [&str; 3] = ["NO_WINDOW", "CREATE_NO_WINDOW", "0x0800_0000"];

/// How many lines after a spawn line are searched for its flag.
const WINDOW: usize = 12;

/// Spawns that are visible on purpose. Matched by file name and a substring of the spawn line,
/// so the list survives edits above them. Each entry says why the window is allowed.
const VISIBLE_ON_PURPOSE: [(&str, &str, &str); 6] = [
    ("bridge\\claude_profile.rs", "wt.exe", "opens a terminal window that runs Claude, when the user asks for one"),
    ("bridge\\claude_profile.rs", "cmd.exe", "fallback for the same terminal window when wt.exe is missing"),
    ("bridge\\claude_profile.rs", "powershell.exe", "PowerShell console fallback for the same Claude login window when Windows Terminal fails to start"),
    // the user chose visible terminals for coding agents (2026-11-15): they watch the agent and type to it there
    ("bridge\\agent_restart.rs", "cmd.exe", "opens a visible console to resume an agent session"),
    ("bridge\\agent_restart.rs", "agent_exe", "opens a visible console for a coding agent the buddy starts (start_agent)"),
    ("settings.rs", "explorer.exe", "opens a folder in Explorer, when the user asks to see it"),
];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("source dir") {
        let p = entry.expect("dir entry").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// (1-based line, line text) of every spawn in `src` whose window has no no-window flag.
fn unflagged_spawns(src: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut bad = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("//") || !line.contains("Command::new(") {
            continue;
        }
        let mut window = String::new();
        for (j, next) in lines[i..].iter().take(WINDOW).enumerate() {
            if j > 0 && next.contains("Command::new(") {
                break;
            }
            window.push_str(next);
            window.push('\n');
        }
        if !FLAGS.iter().any(|f| window.contains(f)) {
            bad.push((i + 1, line.trim().to_string()));
        }
    }
    bad
}

fn allowed(file: &Path, line: &str) -> bool {
    let name = file.to_string_lossy();
    VISIBLE_ON_PURPOSE.iter().any(|(f, prog, _)| name.ends_with(f) && line.contains(prog))
}

#[test]
fn every_spawn_sets_the_no_window_flag() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rs_files(&manifest.join("src"), &mut files);
    rs_files(&manifest.join("..").join("zor-setup").join("src"), &mut files);

    let mut spawns = 0;
    let mut failures = Vec::new();
    for file in &files {
        // This file names `Command::new(` in its own text and in the test data.
        if file.file_name().and_then(|n| n.to_str()) == Some("headless_test.rs") {
            continue;
        }
        let src = std::fs::read_to_string(file).expect("read source");
        spawns += src.lines().filter(|l| !l.trim_start().starts_with("//") && l.contains("Command::new(")).count();
        for (line_no, text) in unflagged_spawns(&src) {
            if !allowed(file, &text) {
                failures.push(format!("{}:{line_no}: {text}", file.display()));
            }
        }
    }

    // Guard against the walk finding nothing (a moved folder would otherwise pass silently).
    assert!(spawns >= 20, "found only {spawns} spawns; the scan is looking in the wrong place");
    assert!(
        failures.is_empty(),
        "these spawns open a console window. Add `.creation_flags(NO_WINDOW)` (or use bridge::common::hidden):\n{}",
        failures.join("\n")
    );
}

#[test]
fn flags_on_a_later_line_of_the_same_spawn_count() {
    let src = "let mut c = Command::new(program);\nc.creation_flags(NO_WINDOW);\n";
    assert!(unflagged_spawns(src).is_empty());
}

#[test]
fn a_spawn_without_the_flag_is_reported() {
    let src = "let out = Command::new(\"curl.exe\").output();\n";
    assert_eq!(unflagged_spawns(src), vec![(1, "let out = Command::new(\"curl.exe\").output();".to_string())]);
}

#[test]
fn a_commented_out_spawn_is_ignored() {
    assert!(unflagged_spawns("// Command::new(\"curl.exe\")\n").is_empty());
}

#[test]
fn a_flag_on_the_next_spawn_does_not_hide_this_one() {
    let src = "let a = Command::new(\"x\").output();\nlet b = Command::new(\"y\").creation_flags(NO_WINDOW);\n";
    let bad = unflagged_spawns(src);
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].0, 1);
}
