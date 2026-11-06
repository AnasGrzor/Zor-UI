//! zorui.log in the data folder: one timestamped line per user action and per preset / wallpaper step, so a
//! silent failure can be traced afterwards. The file is moved to zorui.log.old once it passes 1 MB.

use std::{io::Write, path::Path};

const MAX: u64 = 1_000_000;

/// Appends `msg` to zorui.log.
pub fn line(msg: &str) { write_to(&crate::conf::data_dir().join("zorui.log"), msg); }

fn write_to(path: &Path, msg: &str) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX) { let _ = std::fs::rename(path, path.with_extension("log.old")); }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // one line per entry, however many the message has
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) { let _ = writeln!(f, "[unix {secs}] {}", msg.replace('\n', " ")); }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(d.join("zorui.log.old").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
