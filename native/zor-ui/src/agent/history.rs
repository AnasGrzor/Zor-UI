//! The buddy's recent turns, kept across restarts in the memory database (table `turns`). Only answered turns are kept, and
//! turns that look like they hold a secret are not. A turn older than `TURN_SECS` is dropped, and at most `KEEP_TURNS` stay
//! in each conversation. A conversation is where the request came from (`DESKTOP`, or a remote chat such as
//! "discord:<user>:<channel>"), so turns from one never show up as context in another.

use super::memory;
use super::{Turn, KEEP_TURNS, TURN_SECS};
use rusqlite::{params, Connection};
use std::{collections::BTreeSet, sync::Mutex, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

pub fn setup(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS turns (id INTEGER PRIMARY KEY, said TEXT NOT NULL, answer TEXT NOT NULL, at INTEGER NOT NULL, conversation TEXT NOT NULL DEFAULT 'desktop');")?;
    // databases made before conversations: their turns were all said at the desktop
    super::tasks::add_column(conn, "turns", "conversation", "TEXT NOT NULL DEFAULT 'desktop'")
}

/// Saves one answered turn of `conversation` at unix time `at`, then trims that conversation to its newest `KEEP_TURNS`.
/// False when the turn was not saved because it looks like a secret.
pub fn save_on(conn: &Connection, conversation: &str, said: &str, answer: &str, at: i64) -> rusqlite::Result<bool> {
    if memory::is_sensitive(said) || memory::is_sensitive(answer) { return Ok(false); }
    conn.execute("INSERT INTO turns (said, answer, at, conversation) VALUES (?1, ?2, ?3, ?4)", params![said, answer, at, conversation])?;
    conn.execute(
        "DELETE FROM turns WHERE conversation = ?1 AND id NOT IN (SELECT id FROM turns WHERE conversation = ?1 ORDER BY id DESC LIMIT ?2)",
        params![conversation, KEEP_TURNS as i64],
    )?;
    Ok(true)
}

/// The saved turns of `conversation` no older than `TURN_SECS` before `now`, oldest first, as (said, answer, unix time).
pub fn load_on(conn: &Connection, conversation: &str, now: i64) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare("SELECT said, answer, at FROM turns WHERE conversation = ?1 AND at >= ?2 ORDER BY id ASC")?;
    let rows = stmt.query_map(params![conversation, now - TURN_SECS as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

/// Saves an answered turn of `conversation` to the app's database. A failure is logged and the request goes on.
pub fn save(conversation: &str, said: &str, answer: &str) {
    let saved = memory::with_app(|c| setup(c).and_then(|_| save_on(c, conversation, said, answer, now())).map_err(|e| e.to_string())).and_then(|r| r);
    if let Err(e) = saved { crate::log::line(&format!("buddy could not save the turn: {e}")); }
}

/// Conversations whose saved turns were already read this run.
static RESTORED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The saved turns of `conversation`, read from the database the first time this run asks for it and empty after that. Each
/// turn gets an `Instant` as old as it was when saved, so the normal expiry still applies. Empty too when the database fails.
pub fn restore(conversation: &str) -> Vec<Turn> {
    let first = RESTORED.lock().map(|mut r| r.insert(conversation.to_string())).unwrap_or(false);
    if !first { return vec![]; }
    let now_s = now();
    let Ok(Ok(rows)) = memory::with_app(|c| setup(c).and_then(|_| load_on(c, conversation, now_s))) else { return vec![] };
    rows.into_iter()
        .filter_map(|(said, answer, at)| {
            let age = Duration::from_secs((now_s - at).max(0) as u64);
            Instant::now().checked_sub(age).map(|when| (said, answer, when))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::DESKTOP;

    fn mem() -> Connection { let c = Connection::open_in_memory().unwrap(); setup(&c).unwrap(); c }

    #[test]
    fn a_saved_turn_comes_back_while_it_is_recent() {
        let c = mem();
        assert!(save_on(&c, DESKTOP, "play lofi", "Playing lofi.", 1_000).unwrap());
        let back = load_on(&c, DESKTOP, 1_000 + 60).unwrap();
        assert_eq!(back, vec![("play lofi".to_string(), "Playing lofi.".to_string(), 1_000)]);
        assert!(load_on(&c, DESKTOP, 1_000 + TURN_SECS as i64 + 1).unwrap().is_empty(), "an old turn is dropped");
    }

    #[test]
    fn only_the_newest_turns_are_kept() {
        let c = mem();
        for i in 0..(KEEP_TURNS as i64 + 3) { save_on(&c, DESKTOP, &format!("q{i}"), "a", 500 + i).unwrap(); }
        let back = load_on(&c, DESKTOP, 500).unwrap();
        assert_eq!(back.len(), KEEP_TURNS);
        assert_eq!(back.last().unwrap().0, format!("q{}", KEEP_TURNS as i64 + 2), "the newest one is kept");
    }

    #[test]
    fn conversations_keep_their_own_turns() {
        let c = mem();
        let remote = "discord:1:2";
        save_on(&c, DESKTOP, "desk", "a", 500).unwrap();
        // the remote chat fills up past the limit; the desktop turn is not trimmed by it
        for i in 0..(KEEP_TURNS as i64 + 2) { save_on(&c, remote, &format!("r{i}"), "a", 501 + i).unwrap(); }
        let desk = load_on(&c, DESKTOP, 500).unwrap();
        assert_eq!(desk.len(), 1);
        assert_eq!(desk[0].0, "desk");
        let far = load_on(&c, remote, 500).unwrap();
        assert_eq!(far.len(), KEEP_TURNS);
        assert!(far.iter().all(|t| t.0.starts_with('r')), "no desktop turn in the remote chat");
        assert!(load_on(&c, "discord:9:9", 500).unwrap().is_empty());
    }

    #[test]
    fn an_old_table_gets_the_column_and_its_turns_are_desktop_turns() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE turns (id INTEGER PRIMARY KEY, said TEXT NOT NULL, answer TEXT NOT NULL, at INTEGER NOT NULL);
                         INSERT INTO turns (said, answer, at) VALUES ('old', 'kept', 700);").unwrap();
        setup(&c).unwrap();
        setup(&c).unwrap(); // a second setup finds the column and changes nothing
        assert_eq!(load_on(&c, DESKTOP, 700).unwrap(), vec![("old".to_string(), "kept".to_string(), 700)]);
        assert!(load_on(&c, "discord:1:2", 700).unwrap().is_empty());
    }

    #[test]
    fn a_turn_with_a_secret_is_not_saved() {
        let c = mem();
        assert!(!save_on(&c, DESKTOP, "my password is hunter2", "Noted.", 1).unwrap());
        assert!(!save_on(&c, DESKTOP, "what is my card", "Your card is 4111 1111 1111 1111.", 1).unwrap());
        assert!(load_on(&c, DESKTOP, 0).unwrap().is_empty());
    }

    #[test]
    fn turns_survive_a_reopen() {
        let dir = std::env::temp_dir().join(format!("zorui-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("buddy.db");
        {
            let c = memory::open(&path).unwrap();
            setup(&c).unwrap();
            save_on(&c, DESKTOP, "what time is it", "It is nine.", now()).unwrap();
        }
        let c = memory::open(&path).unwrap();
        setup(&c).unwrap();
        assert_eq!(load_on(&c, DESKTOP, now() - 10).unwrap().len(), 1);
    }
}
