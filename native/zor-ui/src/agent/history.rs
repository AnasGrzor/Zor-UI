//! The buddy's recent turns, kept across restarts in the memory database (table `turns`). Only answered turns are kept, and
//! turns that look like they hold a secret are not. A turn older than `TURN_SECS` is dropped, and at most `KEEP_TURNS` stay.

use super::memory;
use super::{Turn, KEEP_TURNS, TURN_SECS};
use rusqlite::{params, Connection};
use std::{collections::VecDeque, sync::{Mutex, Once}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

pub fn setup(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS turns (id INTEGER PRIMARY KEY, said TEXT NOT NULL, answer TEXT NOT NULL, at INTEGER NOT NULL);")
}

/// Saves one answered turn at unix time `at`, then trims the table to the newest `KEEP_TURNS`. False when the turn was not
/// saved because it looks like a secret.
pub fn save_on(conn: &Connection, said: &str, answer: &str, at: i64) -> rusqlite::Result<bool> {
    if memory::is_sensitive(said) || memory::is_sensitive(answer) { return Ok(false); }
    conn.execute("INSERT INTO turns (said, answer, at) VALUES (?1, ?2, ?3)", params![said, answer, at])?;
    conn.execute("DELETE FROM turns WHERE id NOT IN (SELECT id FROM turns ORDER BY id DESC LIMIT ?1)", params![KEEP_TURNS as i64])?;
    Ok(true)
}

/// The saved turns no older than `TURN_SECS` before `now`, oldest first, as (said, answer, unix time).
pub fn load_on(conn: &Connection, now: i64) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare("SELECT said, answer, at FROM turns WHERE at >= ?1 ORDER BY id ASC")?;
    let rows = stmt.query_map(params![now - TURN_SECS as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

/// Saves an answered turn to the app's database. A failure is logged and the request goes on.
pub fn save(said: &str, answer: &str) {
    let saved = memory::with_app(|c| setup(c).and_then(|_| save_on(c, said, answer, now())).map_err(|e| e.to_string())).and_then(|r| r);
    if let Err(e) = saved { crate::log::line(&format!("buddy could not save the turn: {e}")); }
}

static RESTORED: Once = Once::new();

/// Fills the in-memory turns from the database, once per run. Each saved turn gets an `Instant` that is as old as it was
/// when saved, so the normal expiry still applies.
pub fn restore(turns: &Mutex<VecDeque<Turn>>) {
    RESTORED.call_once(|| {
        let now_s = now();
        let Ok(Ok(rows)) = memory::with_app(|c| setup(c).and_then(|_| load_on(c, now_s))) else { return };
        let Ok(mut t) = turns.lock() else { return };
        for (said, answer, at) in rows {
            let age = Duration::from_secs((now_s - at).max(0) as u64);
            if let Some(when) = Instant::now().checked_sub(age) { t.push_back((said, answer, when)); }
        }
        while t.len() > KEEP_TURNS { t.pop_front(); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection { let c = Connection::open_in_memory().unwrap(); setup(&c).unwrap(); c }

    #[test]
    fn a_saved_turn_comes_back_while_it_is_recent() {
        let c = mem();
        assert!(save_on(&c, "play lofi", "Playing lofi.", 1_000).unwrap());
        let back = load_on(&c, 1_000 + 60).unwrap();
        assert_eq!(back, vec![("play lofi".to_string(), "Playing lofi.".to_string(), 1_000)]);
        assert!(load_on(&c, 1_000 + TURN_SECS as i64 + 1).unwrap().is_empty(), "an old turn is dropped");
    }

    #[test]
    fn only_the_newest_turns_are_kept() {
        let c = mem();
        for i in 0..(KEEP_TURNS as i64 + 3) { save_on(&c, &format!("q{i}"), "a", 500 + i).unwrap(); }
        let back = load_on(&c, 500).unwrap();
        assert_eq!(back.len(), KEEP_TURNS);
        assert_eq!(back.last().unwrap().0, format!("q{}", KEEP_TURNS as i64 + 2), "the newest one is kept");
    }

    #[test]
    fn a_turn_with_a_secret_is_not_saved() {
        let c = mem();
        assert!(!save_on(&c, "my password is hunter2", "Noted.", 1).unwrap());
        assert!(!save_on(&c, "what is my card", "Your card is 4111 1111 1111 1111.", 1).unwrap());
        assert!(load_on(&c, 0).unwrap().is_empty());
    }

    #[test]
    fn turns_survive_a_reopen() {
        let dir = std::env::temp_dir().join(format!("zorui-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("buddy.db");
        {
            let c = memory::open(&path).unwrap();
            setup(&c).unwrap();
            save_on(&c, "what time is it", "It is nine.", now()).unwrap();
        }
        let c = memory::open(&path).unwrap();
        setup(&c).unwrap();
        assert_eq!(load_on(&c, now() - 10).unwrap().len(), 1);
    }
}
