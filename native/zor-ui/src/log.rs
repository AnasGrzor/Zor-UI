//! zorui.log in the data folder: one timestamped line per user action and per preset / wallpaper step, so a
//! silent failure can be traced afterwards. Past `MAX` the file moves to zorui.log.1, the older copies move up one
//! (zorui.log.2 ...), and only the one past `KEEP` is dropped. Frame-trace lines go to their own file, so their rate can't
//! push the real lines out.

use std::{io::Write, path::Path};

/// The size at which a log file moves to its first old copy.
const MAX: u64 = 5_000_000;
/// Old copies kept per log file, zorui.log.1 being the newest.
const KEEP: u32 = 10;

/// Appends `msg` to zorui.log.
pub fn line(msg: &str) { write_to(&path(), msg); }

/// Appends a TEMP frametrace line to zorui.frametrace.log, which rotates on its own.
pub fn trace(msg: &str) { write_to(&path().with_extension("frametrace.log"), msg); }

/// The real zorui.log in the data folder.
fn real_path() -> std::path::PathBuf { crate::conf::data_dir().join("zorui.log") }

#[cfg(not(test))]
fn path() -> std::path::PathBuf { real_path() }

/// Tests never write the real zorui.log: their lines go to a file of this process in the temp folder.
#[cfg(test)]
fn path() -> std::path::PathBuf { std::env::temp_dir().join(format!("zorui-test-{}.log", std::process::id())) }

/// Routes every `tracing` event at INFO or above (libraries use it) into zorui.log, one line each.
/// Call once at start; a second call is ignored.
pub fn init() {
    let _ = tracing_subscriber::fmt().with_ansi(false).without_time().with_max_level(tracing_subscriber::filter::LevelFilter::INFO).with_writer(|| Sink).try_init();
}

struct Sink;

impl std::io::Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { line(&event_text(b)); Ok(b.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// A formatted event without its trailing newline (`line` adds its own).
fn event_text(b: &[u8]) -> String { String::from_utf8_lossy(b).trim_end().to_string() }

/// The `n`th old copy of `path`, as in zorui.log.3.
fn old(path: &Path, n: u32) -> std::path::PathBuf { path.with_extension(format!("log.{n}")) }

/// Shifts the old copies up one and moves `path` to the first; the copy past `KEEP` is the only one dropped.
fn rotate(path: &Path) {
    let _ = std::fs::remove_file(old(path, KEEP));
    // from the top down, since a rename can't replace a file on Windows
    for n in (1..KEEP).rev() { let _ = std::fs::rename(old(path, n), old(path, n + 1)); }
    let _ = std::fs::rename(path, old(path, 1));
}

fn write_to(path: &Path, msg: &str) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX) { rotate(path); }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // one line per entry, however many the message has
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) { let _ = writeln!(f, "[unix {secs}] {}", msg.replace('\n', " ")); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_text_drops_the_newline_and_bad_bytes() {
        assert_eq!(event_text(b" INFO systray_util: Found 3 buttons\n"), " INFO systray_util: Found 3 buttons");
        assert_eq!(event_text(&[b'a', 0xFF, b'\n']), format!("a{}", char::REPLACEMENT_CHARACTER));
    }

    #[test]
    fn a_test_line_never_reaches_the_real_log() {
        assert_ne!(path(), real_path(), "tests write to a temp file, not the real zorui.log");
        assert!(path().starts_with(std::env::temp_dir()));
        line("test marker for the redirect check");
        assert!(std::fs::read_to_string(path()).unwrap().contains("test marker for the redirect check"));
    }

    #[test]
    fn lines_append_and_the_file_rotates() {
        let d = std::env::temp_dir().join(format!("zor-ui-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("zorui.log");
        write_to(&p, "one\ntwo");
        write_to(&p, "three");
        let t = std::fs::read_to_string(&p).unwrap();
        assert_eq!(t.lines().count(), 2);
        assert!(t.lines().next().unwrap().starts_with("[unix ") && t.contains("one two"));
        std::fs::write(&p, vec![b'x'; MAX as usize + 1]).unwrap();
        write_to(&p, "fresh");
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 1);
        assert!(d.join("zorui.log.1").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rotation_keeps_the_newest_copies_in_order() {
        let d = std::env::temp_dir().join(format!("zor-ui-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("zorui.log");
        for n in 0..KEEP + 3 { std::fs::write(&p, format!("gen {n}")).unwrap(); rotate(&p); }
        let newest = KEEP + 2;
        assert_eq!(std::fs::read_to_string(old(&p, 1)).unwrap(), format!("gen {newest}"), "the newest copy is .1");
        assert_eq!(std::fs::read_to_string(old(&p, KEEP)).unwrap(), format!("gen {}", newest + 1 - KEEP), "KEEP copies survive");
        assert!(!old(&p, KEEP + 1).exists(), "nothing past KEEP");
        assert_eq!(old(&p.with_extension("frametrace.log"), 2).file_name().unwrap(), "zorui.frametrace.log.2");
        let _ = std::fs::remove_dir_all(&d);
    }
}
