//! Buddy's long-term memory: facts the user asked it to keep, in SQLite at %LOCALAPPDATA%\zor-ui\buddy.db. Recall uses FTS5
//! full-text search. Facts only, never whole conversations. It stays on this PC, duplicates update the fact they match,
//! and anything that looks like a secret is refused. A JSON store from the earlier build is imported once.

use crate::conf;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{collections::BTreeSet, path::{Path, PathBuf}, sync::Mutex, time::{SystemTime, UNIX_EPOCH}};

#[derive(Clone, Debug, PartialEq)]
pub struct Memory { pub id: i64, pub text: String, pub created: i64, pub updated: i64 }

/// What `add` did with a fact.
#[derive(Debug, PartialEq)]
pub enum Outcome { Added(i64), Updated(i64) }

/// Two facts with at least this share of their words (Jaccard) are the same fact, so the newer text replaces the old.
const SAME_FACT: f64 = 0.6;

/// Words that match almost every fact and say nothing about what it is about. They are left out of recall.
const STOP: &[&str] = &["the", "and", "for", "you", "your", "are", "was", "were", "what", "that", "this", "with", "have", "has",
    "did", "does", "from", "about", "tell", "know", "please", "them", "they", "there", "can", "could", "would", "should", "you're"];

/// The words of a text that count for matching: letters and digits, three or more characters, lowercased.
pub fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(|w| w.to_lowercase())
        .collect()
}

/// True for text that looks like a password, a key, a card or account number, or a PIN. Such text is never saved.
pub fn is_sensitive(text: &str) -> bool {
    let lower = text.to_lowercase();
    const WORDS: [&str; 12] = ["password", "passwd", "passcode", "api key", "apikey", "secret", "token", "pin code", "pin is", "credit card", "card number", "iban"];
    if WORDS.iter().any(|w| lower.contains(w)) { return true; }
    if ["sk-", "ghp_", "xox"].iter().any(|p| lower.contains(p)) { return true; }
    // a run of 12 or more digits (ignoring spaces and dashes) is a card, account or phone-like number
    let digits: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    let mut run = 0;
    for c in digits.chars() {
        run = if c.is_ascii_digit() { run + 1 } else { 0 };
        if run >= 12 { return true; }
    }
    false
}

/// Shared words over all words of both texts: 1.0 for the same words, 0.0 for none in common.
fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 { return 0.0; }
    a.intersection(b).count() as f64 / union as f64
}

/// Opens the memory database at `path` and makes sure its tables exist.
pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir).map_err(|e| format!("could not make the memory folder: {e}"))?; }
    let conn = Connection::open(path).map_err(|e| format!("could not open memory: {e}"))?;
    setup(&conn)?;
    Ok(conn)
}

fn setup(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memories (
             id INTEGER PRIMARY KEY,
             text TEXT NOT NULL,
             created INTEGER NOT NULL,
             updated INTEGER NOT NULL
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(text);",
    ).map_err(|e| format!("could not set up memory: {e}"))
}

/// Every stored fact, oldest first.
pub fn all(conn: &Connection) -> rusqlite::Result<Vec<Memory>> {
    let mut stmt = conn.prepare("SELECT id, text, created, updated FROM memories ORDER BY id")?;
    let rows = stmt.query_map([], |r| Ok(Memory { id: r.get(0)?, text: r.get(1)?, created: r.get(2)?, updated: r.get(3)? }))?;
    rows.collect()
}

/// Keeps a fact. Empty and sensitive text is refused; a fact that matches a stored one updates it.
pub fn add(conn: &Connection, text: &str, now: i64) -> Result<Outcome, String> {
    let text = text.trim();
    if text.is_empty() { return Err("Nothing to remember.".into()); }
    if is_sensitive(text) { return Err("I won't keep passwords, keys or account numbers.".into()); }
    let db = |e: rusqlite::Error| format!("could not save memory: {e}");
    let new = words(text);
    let same = all(conn).map_err(db)?.into_iter().find(|m| jaccard(&words(&m.text), &new) >= SAME_FACT);
    let tx = conn.unchecked_transaction().map_err(db)?;
    let outcome = match same {
        Some(m) => {
            tx.execute("UPDATE memories SET text = ?1, updated = ?2 WHERE id = ?3", params![text, now, m.id]).map_err(db)?;
            tx.execute("UPDATE memory_fts SET text = ?1 WHERE rowid = ?2", params![text, m.id]).map_err(db)?;
            Outcome::Updated(m.id)
        }
        None => {
            tx.execute("INSERT INTO memories (text, created, updated) VALUES (?1, ?2, ?2)", params![text, now]).map_err(db)?;
            let id = tx.last_insert_rowid();
            tx.execute("INSERT INTO memory_fts (rowid, text) VALUES (?1, ?2)", params![id, text]).map_err(db)?;
            Outcome::Added(id)
        }
    };
    tx.commit().map_err(db)?;
    Ok(outcome)
}

/// Deletes one fact from the memory and its search index. True when it existed.
pub fn forget(conn: &Connection, id: i64) -> Result<bool, String> {
    let db = |e: rusqlite::Error| format!("could not forget: {e}");
    let tx = conn.unchecked_transaction().map_err(db)?;
    let n = tx.execute("DELETE FROM memories WHERE id = ?1", params![id]).map_err(db)?;
    tx.execute("DELETE FROM memory_fts WHERE rowid = ?1", params![id]).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(n > 0)
}

/// Up to `k` facts that match `query`, best match first. Only the words of the query count, and the common ones don't.
pub fn search(conn: &Connection, query: &str, k: usize) -> rusqlite::Result<Vec<Memory>> {
    let terms: Vec<String> = words(query).into_iter().filter(|w| !STOP.contains(&w.as_str())).collect();
    if terms.is_empty() { return Ok(vec![]); }
    // each word is letters and digits only, so quoting it can't break the search syntax
    let expr = terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" OR ");
    let mut stmt = conn.prepare(
        "SELECT m.id, m.text, m.created, m.updated FROM memory_fts
         JOIN memories m ON m.id = memory_fts.rowid
         WHERE memory_fts MATCH ?1
         ORDER BY bm25(memory_fts), m.updated DESC LIMIT ?2")?;
    let rows = stmt.query_map(params![expr, k as i64], |r| Ok(Memory { id: r.get(0)?, text: r.get(1)?, created: r.get(2)?, updated: r.get(3)? }))?;
    rows.collect()
}

/// Imports the facts of the JSON store from the earlier build, once: when the database is empty and the JSON file exists.
/// The JSON file is renamed afterwards, so it is kept but never read twice.
pub fn import_json(conn: &Connection, json_path: &Path) -> Result<usize, String> {
    let empty = all(conn).map_err(|e| format!("could not read memory: {e}"))?.is_empty();
    let Ok(text) = std::fs::read_to_string(json_path) else { return Ok(0) };
    if !empty { return Ok(0); }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("could not read the old memory file: {e}"))?;
    let mut n = 0;
    for m in v["memories"].as_array().into_iter().flatten() {
        let (Some(text), Some(created), Some(updated)) = (m["text"].as_str(), m["created"].as_i64(), m["updated"].as_i64()) else { continue };
        if text.trim().is_empty() || is_sensitive(text) { continue; }
        conn.execute("INSERT INTO memories (text, created, updated) VALUES (?1, ?2, ?3)", params![text, created, updated])
            .map_err(|e| format!("could not import memory: {e}"))?;
        conn.execute("INSERT INTO memory_fts (rowid, text) VALUES (?1, ?2)", params![conn.last_insert_rowid(), text])
            .map_err(|e| format!("could not import memory: {e}"))?;
        n += 1;
    }
    let _ = std::fs::rename(json_path, json_path.with_extension("json.migrated"));
    Ok(n)
}

fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

/// The memory database for this PC.
pub fn path() -> PathBuf { conf::data_dir().join("buddy.db") }

/// The JSON store the earlier build wrote, imported on first open.
fn legacy_json() -> PathBuf { conf::data_dir().join("buddy-memory.json") }

/// One lock for every read and write from the app, so two requests can't overwrite each other's facts.
static LOCK: Mutex<()> = Mutex::new(());

fn open_app() -> Result<Connection, String> {
    let conn = open(&path())?;
    import_json(&conn, &legacy_json())?;
    Ok(conn)
}

/// Forgets the stored fact that best matches `query`. The answer is what the buddy says back.
pub fn forget_matching(query: &str) -> Result<String, String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_app()?;
    let best = search(&conn, query, 1).map_err(|e| format!("could not search memory: {e}"))?;
    let Some(m) = best.into_iter().next() else { return Ok("I don't remember anything like that.".into()) };
    forget(&conn, m.id)?;
    Ok(format!("Forgot: {}", m.text))
}

/// Keeps a fact in the app's memory. The answer is what the buddy says back.
pub fn remember(text: &str) -> Result<String, String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_app()?;
    Ok(match add(&conn, text, now())? {
        Outcome::Added(_) => format!("Remembered: {}", text.trim()),
        Outcome::Updated(_) => format!("Updated what I remember: {}", text.trim()),
    })
}

/// Up to `k` facts that match `query`, for the system prompt. A memory that can't be read recalls nothing, and the request goes on.
pub fn recall(query: &str, k: usize) -> Vec<String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(conn) = open_app() else { return vec![] };
    search(&conn, query, k).unwrap_or_default().into_iter().map(|m| m.text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zorui-memory-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("buddy.db")
    }

    fn mem() -> Connection { let c = Connection::open_in_memory().unwrap(); setup(&c).unwrap(); c }

    #[test]
    fn a_fact_is_added_then_a_restatement_updates_it() {
        let c = mem();
        assert_eq!(add(&c, "My sister is called Lina.", 1), Ok(Outcome::Added(1)));
        assert_eq!(add(&c, "My sister is called Lina", 2), Ok(Outcome::Updated(1)), "the same fact, not a second one");
        let rows = all(&c).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].updated, 2);
        assert_eq!(add(&c, "I like green tea in the morning", 3), Ok(Outcome::Added(2)));
        assert_eq!(all(&c).unwrap().len(), 2);
    }

    #[test]
    fn an_updated_fact_is_found_by_its_new_words() {
        let c = mem();
        add(&c, "My sister is called Lina", 1).unwrap();
        add(&c, "My sister is called Lina Berg", 2).unwrap();
        assert_eq!(search(&c, "Berg", 5).unwrap().len(), 1, "the search index follows the update");
    }

    #[test]
    fn empty_and_sensitive_text_is_refused() {
        let c = mem();
        assert!(add(&c, "   ", 1).is_err());
        assert!(add(&c, "My bank password is hunter2", 1).is_err());
        assert!(add(&c, "my card is 4111 1111 1111 1111", 1).is_err());
        assert!(add(&c, "api key sk-abc123", 1).is_err());
        assert!(all(&c).unwrap().is_empty(), "nothing sensitive was kept");
        assert!(add(&c, "My flat number is 12", 1).is_ok(), "short numbers are fine");
    }

    #[test]
    fn search_finds_facts_that_share_words_and_ignores_common_ones() {
        let c = mem();
        add(&c, "My sister is called Lina", 1).unwrap();
        add(&c, "I work on the Zor UI project", 2).unwrap();
        add(&c, "I like green tea", 3).unwrap();
        let hits = search(&c, "what is the status of the Zor project", 5).unwrap();
        assert_eq!(hits.len(), 1, "the common words alone match nothing");
        assert_eq!(hits[0].text, "I work on the Zor UI project");
        assert!(search(&c, "weather tomorrow", 5).unwrap().is_empty(), "no shared word, nothing recalled");
        assert!(search(&c, "what the and", 5).unwrap().is_empty(), "only common words, nothing recalled");
        assert_eq!(search(&c, "lina tea", 1).unwrap().len(), 1, "the limit holds");
    }

    #[test]
    fn a_forgotten_fact_is_gone_from_search_too() {
        let c = mem();
        let Ok(Outcome::Added(id)) = add(&c, "I like green tea", 1) else { panic!() };
        assert_eq!(search(&c, "tea", 5).unwrap().len(), 1);
        assert!(forget(&c, id).unwrap());
        assert!(!forget(&c, id).unwrap(), "gone already");
        assert!(search(&c, "tea", 5).unwrap().is_empty());
    }

    #[test]
    fn facts_survive_closing_and_reopening_the_database() {
        let path = temp_path("reopen");
        {
            let c = open(&path).unwrap();
            add(&c, "My sister is called Lina", 5).unwrap();
        }
        let c = open(&path).unwrap();
        assert_eq!(search(&c, "Lina", 1).unwrap()[0].text, "My sister is called Lina");
    }

    #[test]
    fn the_json_store_from_the_earlier_build_is_imported_once() {
        let path = temp_path("import");
        let json_path = path.with_file_name("buddy-memory.json");
        std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
        std::fs::write(&json_path, r#"{"next_id":2,"memories":[{"id":0,"text":"I like green tea","created":1,"updated":1},{"id":1,"text":"no time stamps"}]}"#).unwrap();
        let c = open(&path).unwrap();
        assert_eq!(import_json(&c, &json_path).unwrap(), 1, "the entry without its time stamps is skipped");
        assert_eq!(search(&c, "tea", 5).unwrap().len(), 1);
        assert!(!json_path.exists(), "the old file is renamed, not read again");
        assert_eq!(import_json(&c, &json_path).unwrap(), 0);
    }

    #[test]
    fn a_damaged_database_is_an_error_not_a_crash() {
        let path = temp_path("damaged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not a database at all, just text that is longer than a page of bytes".repeat(100)).unwrap();
        assert!(open(&path).is_err());
    }
}
