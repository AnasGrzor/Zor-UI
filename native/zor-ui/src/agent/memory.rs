//! Buddy's long-term memory: facts the user asked it to keep, in SQLite at %LOCALAPPDATA%\zor-ui\buddy.db. Recall uses FTS5
//! full-text search. Facts only, never whole conversations. It stays on this PC, duplicates update the fact they match,
//! and anything that looks like a secret is refused. A JSON store from the earlier build is imported once.

use crate::conf;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{collections::BTreeSet, path::{Path, PathBuf}, sync::Mutex, time::{SystemTime, UNIX_EPOCH}};

#[derive(Clone, Debug, PartialEq)]
pub struct Memory { pub id: i64, pub text: String, pub created: i64, pub updated: i64, pub kind: Option<String>, pub project: Option<String> }

/// What a fact is for, stored in `kind` when it is saved. A row with no kind (saved before the column existed) is read by its text.
pub const KIND_PREFERENCE: &str = "preference";
pub const KIND_PROJECT: &str = "project";
pub const KIND_SEMANTIC: &str = "semantic";

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
    ).map_err(|e| format!("could not set up memory: {e}"))?;
    // nullable and additive: rows saved before these columns keep NULL and are classified by their text
    add_column(conn, "kind", "TEXT").and_then(|_| add_column(conn, "project", "TEXT")).map_err(|e| format!("could not set up memory: {e}"))
}

/// Adds `col` to `memories` when the table lacks it. Existing rows are left as they are.
fn add_column(conn: &Connection, col: &str, decl: &str) -> rusqlite::Result<()> {
    let has = conn.prepare("PRAGMA table_info(memories)")?.query_map([], |r| r.get::<_, String>(1))?.any(|n| n.is_ok_and(|n| n == col));
    if !has { conn.execute_batch(&format!("ALTER TABLE memories ADD COLUMN {col} {decl};"))?; }
    Ok(())
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Memory> {
    Ok(Memory { id: r.get(0)?, text: r.get(1)?, created: r.get(2)?, updated: r.get(3)?, kind: r.get(4)?, project: r.get(5)? })
}

/// Every stored fact, oldest first.
pub fn all(conn: &Connection) -> rusqlite::Result<Vec<Memory>> {
    let mut stmt = conn.prepare("SELECT id, text, created, updated, kind, project FROM memories ORDER BY id")?;
    let rows = stmt.query_map([], from_row)?;
    rows.collect()
}

/// Keeps a fact with its kind and project (both optional). Empty and sensitive text is refused; a fact that matches a stored one
/// updates it, and a kind or project given now replaces the stored one.
pub fn add_as(conn: &Connection, text: &str, kind: Option<&str>, project: Option<&str>, now: i64) -> Result<Outcome, String> {
    let text = text.trim();
    if text.is_empty() { return Err("Nothing to remember.".into()); }
    if is_sensitive(text) { return Err("I won't keep passwords, keys or account numbers.".into()); }
    let db = |e: rusqlite::Error| format!("could not save memory: {e}");
    let new = words(text);
    let same = all(conn).map_err(db)?.into_iter().find(|m| jaccard(&words(&m.text), &new) >= SAME_FACT);
    let tx = conn.unchecked_transaction().map_err(db)?;
    let outcome = match same {
        Some(m) => {
            tx.execute("UPDATE memories SET text = ?1, updated = ?2, kind = COALESCE(?3, kind), project = COALESCE(?4, project) WHERE id = ?5",
                params![text, now, kind, project, m.id]).map_err(db)?;
            tx.execute("UPDATE memory_fts SET text = ?1 WHERE rowid = ?2", params![text, m.id]).map_err(db)?;
            Outcome::Updated(m.id)
        }
        None => {
            tx.execute("INSERT INTO memories (text, created, updated, kind, project) VALUES (?1, ?2, ?2, ?3, ?4)", params![text, now, kind, project]).map_err(db)?;
            let id = tx.last_insert_rowid();
            tx.execute("INSERT INTO memory_fts (rowid, text) VALUES (?1, ?2)", params![id, text]).map_err(db)?;
            Outcome::Added(id)
        }
    };
    tx.commit().map_err(db)?;
    Ok(outcome)
}

/// A fact saved with no kind and no project, as the tests and the earlier code saved them.
#[cfg(test)]
pub fn add(conn: &Connection, text: &str, now: i64) -> Result<Outcome, String> { add_as(conn, text, None, None, now) }

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
        "SELECT m.id, m.text, m.created, m.updated, m.kind, m.project FROM memory_fts
         JOIN memories m ON m.id = memory_fts.rowid
         WHERE memory_fts MATCH ?1
         ORDER BY bm25(memory_fts), m.updated DESC LIMIT ?2")?;
    let rows = stmt.query_map(params![expr, k as i64], from_row)?;
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

/// Keeps a fact in the app's memory, with the kind it is known to have (`fact_kind`) and the active project it names, if any.
/// The answer is what the buddy says back.
pub fn remember(text: &str, project: Option<&str>) -> Result<String, String> {
    let outcome = with_app(|c| add_as(c, text, kind_to_save(text, project), project, now()))??;
    changed();
    Ok(match outcome {
        Outcome::Added(_) => format!("Remembered: {}", text.trim()),
        Outcome::Updated(_) => format!("Updated what I remember: {}", text.trim()),
    })
}

/// The memory database for this PC.
#[cfg(not(test))]
pub fn path() -> PathBuf { conf::data_dir().join("buddy.db") }

/// Tests never touch the real buddy.db: the shared connection opens a file of this process in the temp folder.
#[cfg(test)]
pub fn path() -> PathBuf { std::env::temp_dir().join(format!("zorui-test-buddy-{}.db", std::process::id())) }

/// The JSON store the earlier build wrote, imported on first open.
fn legacy_json() -> PathBuf { conf::data_dir().join("buddy-memory.json") }

/// The app's one memory connection. It is opened on first use and kept for the process, and its lock serializes every read and
/// write, so two requests can't overwrite each other's facts. `opens` counts the opens, so a test can check there is one.
struct Db { conn: Option<Connection>, opens: usize }

static DB: Mutex<Db> = Mutex::new(Db { conn: None, opens: 0 });

/// Runs `f` on the connection at `path`. The first call opens it (and imports the old JSON store); later calls reuse it. A failed
/// open is not kept, so the next call tries again.
fn with_conn<T>(slot: &Mutex<Db>, path: &Path, legacy: &Path, f: impl FnOnce(&Connection) -> T) -> Result<T, String> {
    let mut db = slot.lock().unwrap_or_else(|e| e.into_inner());
    if db.conn.is_none() {
        let conn = open(path)?;
        import_json(&conn, legacy)?;
        db.opens += 1;
        db.conn = Some(conn);
    }
    Ok(f(db.conn.as_ref().unwrap_or_else(|| unreachable!("set just above"))))
}

/// Runs `f` on the app's memory database (`path()`), opened once per process.
pub fn with_app<T>(f: impl FnOnce(&Connection) -> T) -> Result<T, String> { with_conn(&DB, &path(), &legacy_json(), f) }

/// How many times the app's connection was opened in this process (tests only).
#[cfg(test)]
pub fn opens() -> usize { DB.lock().unwrap_or_else(|e| e.into_inner()).opens }

/// Tests that write the shared database (`with_app`) hold this, so their task rows never mix with another test's (tests only).
#[cfg(test)]
pub static SHARED_DB_TESTS: Mutex<()> = Mutex::new(());

/// The facts as the Settings page lists them, oldest first. Cached, so a repaint never opens the database; every write clears it.
static CACHE: Mutex<Option<Vec<Memory>>> = Mutex::new(None);

fn changed() { *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None; }

pub fn listing() -> Vec<Memory> {
    if let Some(v) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() { return v.clone(); }
    let rows = with_app(|c| all(c).unwrap_or_default()).unwrap_or_default();
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(rows.clone());
    rows
}

/// Forgets one fact by id, from the Settings list. True when it existed.
pub fn forget_id(id: i64) -> Result<bool, String> {
    let gone = with_app(|c| forget(c, id))??;
    changed();
    Ok(gone)
}

/// Forgets the stored fact that best matches `query`. The answer is what the buddy says back.
pub fn forget_matching(query: &str) -> Result<String, String> {
    // search and delete in one step under the lock, so the fact found is the fact removed
    let found = with_app(|c| -> Result<Option<Memory>, String> {
        let Some(m) = search(c, query, 1).map_err(|e| format!("could not search memory: {e}"))?.into_iter().next() else { return Ok(None) };
        forget(c, m.id)?;
        Ok(Some(m))
    })??;
    changed();
    Ok(match found { Some(m) => format!("Forgot: {}", m.text), None => "I don't remember anything like that.".into() })
}

/// How facts are found for a request. Keyword search (FTS5) is the only ranker today. A semantic ranker can implement this
/// trait later, with its own storage, and `recall` will use it without any other change.
pub trait Recall {
    fn find(&self, conn: &Connection, query: &str, k: usize) -> rusqlite::Result<Vec<Memory>>;
}

/// Keyword ranking: words shared with the query, best match first.
pub struct Keyword;

impl Recall for Keyword {
    fn find(&self, conn: &Connection, query: &str, k: usize) -> rusqlite::Result<Vec<Memory>> { search(conn, query, k) }
}

/// Up to `k` facts that match `query`, for the system prompt. A memory that can't be read recalls nothing, and the request goes on.
/// A sensitive-looking fact is never recalled, even one stored before the save check existed.
pub fn recall(query: &str, k: usize) -> Vec<String> {
    with_app(|c| recall_in(c, query, k)).unwrap_or_default()
}

/// `recall` on one connection: the matching facts, sensitive ones left out.
fn recall_in(conn: &Connection, query: &str, k: usize) -> Vec<String> {
    Keyword.find(conn, query, k).unwrap_or_default().into_iter().filter(|m| !is_sensitive(&m.text)).map(|m| m.text).collect()
}

/// Most memory items the context carries, preferences and project memories together, and most characters they take.
pub const CONTEXT_ITEMS: usize = 6;
pub const CONTEXT_MEMORY_CHARS: usize = 600;

/// Words that say how the user wants things done. A fact saved with no kind is read by its text: a preference is a fact with one of
/// these words.
const PREFERENCE_WORDS: &[&str] = &["prefer", "prefers", "preferred", "always", "never", "avoid", "instead", "rather", "style", "don't", "dont"];

/// The words of a text as they are compared with a project's folder name: letters, digits, apostrophes and dashes, lowercased.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '-')).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

/// True when the fact says how the user wants something done.
pub fn is_preference(text: &str) -> bool { tokens(text).iter().any(|t| PREFERENCE_WORDS.contains(&t.as_str())) }

/// True when the fact names the project: one of its words is the project's folder name (lowercase).
pub fn names_project(text: &str, project: &str) -> bool { tokens(text).iter().any(|t| t == project) }

/// The kind a fact gets when it is saved: a preference if its text says how to do things, else a project fact when it names the
/// active project, else a plain fact.
pub fn fact_kind(text: &str, project: Option<&str>) -> &'static str {
    if is_preference(text) { KIND_PREFERENCE } else if project.is_some() { KIND_PROJECT } else { KIND_SEMANTIC }
}

/// The kind to store for a save, or None when the save can't say it. With no project in front, a plain fact may still name a
/// project: None lets `for_context` judge it by its text later, so it reaches that project's context. Stored as semantic it never would.
/// shortcut: a project fact saved with its window away is found by its text only, not by a stored project name; upgrade if that misses.
pub fn kind_to_save(text: &str, project: Option<&str>) -> Option<&'static str> {
    match fact_kind(text, project) { KIND_SEMANTIC => None, kind => Some(kind) }
}

/// The memory for a request: preferences whose words share something with the request, and project facts for the active project.
/// A row with a kind is judged by it. A row with no kind (saved before the column) is judged by its text: it is a project fact when
/// its text names the active project, a preference when its words are a preference and share something with the request.
/// Best match first (shared words, then newest), at most `CONTEXT_ITEMS` items and `CONTEXT_MEMORY_CHARS` characters. Anything
/// else in the table stays out, and so does anything sensitive.
pub fn for_context(conn: &Connection, request: &str, project: Option<&str>) -> rusqlite::Result<Vec<String>> {
    let terms: BTreeSet<String> = words(request).into_iter().filter(|w| !STOP.contains(&w.as_str())).collect();
    let project = project.map(str::to_lowercase).filter(|p| !p.is_empty());
    let mut picked: Vec<(usize, i64, String)> = all(conn)?.into_iter().filter_map(|m| {
        if is_sensitive(&m.text) { return None; }
        let shared = words(&m.text).intersection(&terms).count();
        let for_here = project.is_some() && m.project.as_deref().map_or(false, |p| Some(p.to_lowercase()) == project);
        let wanted = match m.kind.as_deref() {
            None => project.as_deref().map_or(false, |p| names_project(&m.text, p)) || (shared > 0 && is_preference(&m.text)),
            // a preference with a project is for that project only; one without is for every request it matches
            Some(KIND_PREFERENCE) => shared > 0 && (m.project.is_none() || for_here),
            Some(KIND_PROJECT) => for_here,
            Some(_) => false,
        };
        wanted.then_some((shared, m.updated, m.text))
    }).collect();
    picked.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    let mut out = vec![];
    let mut chars = 0;
    for (_, _, text) in picked {
        if out.len() == CONTEXT_ITEMS { break; }
        // the joined line has " | " between items, so each item after the first costs three more characters
        let n = text.chars().count() + if out.is_empty() { 0 } else { 3 };
        // a long fact that does not fit is skipped, and a shorter one after it may still go in
        if chars + n > CONTEXT_MEMORY_CHARS { continue; }
        chars += n;
        out.push(text);
    }
    Ok(out)
}

/// The memory lines for a request, through the app's one connection. Empty when memory can't be read.
pub fn context_for(request: &str, project: Option<&str>) -> Vec<String> {
    with_app(|c| for_context(c, request, project).unwrap_or_default()).unwrap_or_default()
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
    fn a_write_clears_the_list_the_settings_page_reads() {
        *CACHE.lock().unwrap() = Some(vec![Memory { id: 1, text: "old".into(), created: 0, updated: 0, kind: None, project: None }]);
        changed();
        assert!(CACHE.lock().unwrap().is_none(), "the next listing reads the database again");
    }

    #[test]
    fn the_connection_is_opened_once_across_lookups() {
        let path = temp_path("one-open");
        let legacy = path.with_file_name("no-such-file.json");
        let slot = Mutex::new(Db { conn: None, opens: 0 });
        with_conn(&slot, &path, &legacy, |c| add(c, "I prefer short answers", 1).unwrap()).unwrap();
        for request in ["short answers please", "what about the answers"] {
            with_conn(&slot, &path, &legacy, |c| for_context(c, request, None).unwrap()).unwrap();
        }
        assert_eq!(slot.lock().unwrap().opens, 1, "two lookups and a write, one open");
    }

    #[test]
    fn a_project_memory_is_included_for_its_project_and_not_for_another() {
        let c = mem();
        add(&c, "zor-ui uses tiny-skia for drawing", 1).unwrap();
        add(&c, "other-app uses React for its pages", 2).unwrap();
        let mine = for_context(&c, "what is the status of the build", Some("zor-ui")).unwrap();
        assert_eq!(mine, vec!["zor-ui uses tiny-skia for drawing".to_string()]);
        assert!(for_context(&c, "what is the status of the build", Some("notes")).unwrap().is_empty(), "no memory names this project");
        assert!(for_context(&c, "what is the status of the build", None).unwrap().is_empty(), "no active project, no project memory");
    }

    #[test]
    fn a_preference_is_included_when_its_words_match_the_request() {
        let c = mem();
        add(&c, "I prefer short answers with no emojis", 1).unwrap();
        add(&c, "my sister is called Lina", 2).unwrap();
        assert_eq!(for_context(&c, "give me a short answer on the weather", None).unwrap(), vec!["I prefer short answers with no emojis".to_string()]);
        assert!(for_context(&c, "open spotify", None).unwrap().is_empty(), "no shared word, no preference");
        assert!(for_context(&c, "what is my sister called", None).unwrap().is_empty(), "a plain fact is not a preference");
    }

    #[test]
    fn the_memory_context_is_capped_in_items_and_characters() {
        let c = mem();
        // distinct words, so no two of these count as the same fact
        let kinds = ["apples", "bananas", "cherries", "dates", "elderberry", "figs", "grapes", "honeydew", "kiwis", "lemons"];
        for (i, k) in kinds.iter().enumerate() { add(&c, &format!("I prefer {k}"), 10 + i as i64).unwrap(); }
        let got = for_context(&c, "I prefer tea", None).unwrap();
        assert_eq!(got.len(), CONTEXT_ITEMS, "more matches than the cap return exactly the cap");
        assert_eq!(got[0], "I prefer lemons", "the newest match is first");
        let long = mem();
        for i in 0..5 { add(&long, &format!("zor-ui note{i} {}", "x".repeat(200)), i).unwrap(); }
        let got = for_context(&long, "anything", Some("zor-ui")).unwrap();
        let total: usize = got.iter().map(|s| s.chars().count()).sum();
        assert!(total <= CONTEXT_MEMORY_CHARS, "{total} characters");
        assert_eq!(got.len(), 2, "two 200-character facts fit in 600, the rest are left out");
    }

    /// Saves a fact with a kind and project, as `remember` does: the kind comes from `fact_kind`.
    fn save(c: &Connection, text: &str, project: Option<&str>, now: i64) {
        add_as(c, text, Some(fact_kind(text, project)), project, now).unwrap();
    }

    /// A row written straight into the table, the way a sensitive fact from an older build could be stored.
    fn insert_raw(c: &Connection, text: &str, kind: Option<&str>) {
        c.execute("INSERT INTO memories (text, created, updated, kind) VALUES (?1, 1, 1, ?2)", params![text, kind]).unwrap();
        c.execute("INSERT INTO memory_fts (rowid, text) VALUES (last_insert_rowid(), ?1)", params![text]).unwrap();
    }

    #[test]
    fn a_preference_row_is_retrieved_for_a_matching_request() {
        let c = mem();
        add_as(&c, "I prefer short answers in English", Some(KIND_PREFERENCE), None, 1).unwrap();
        add_as(&c, "my sister is called Lina", Some(KIND_SEMANTIC), None, 2).unwrap();
        assert_eq!(for_context(&c, "a short answer please", None).unwrap(), vec!["I prefer short answers in English".to_string()]);
        assert!(for_context(&c, "open spotify", None).unwrap().is_empty(), "no shared word, not retrieved");
        assert!(for_context(&c, "what is my sister called", None).unwrap().is_empty(), "a semantic row is never context");
    }

    #[test]
    fn a_project_row_is_retrieved_only_for_its_project() {
        let c = mem();
        add_as(&c, "Zor-UI uses Rust and tiny-skia for drawing", Some(KIND_PROJECT), Some("zor-ui"), 1).unwrap();
        assert_eq!(for_context(&c, "what is the build status", Some("zor-ui")).unwrap(), vec!["Zor-UI uses Rust and tiny-skia for drawing".to_string()]);
        assert!(for_context(&c, "what is the build status", Some("notes")).unwrap().is_empty(), "another project does not get it");
        assert!(for_context(&c, "what is the build status", None).unwrap().is_empty(), "no active project, no project row");
        insert_raw(&c, "Orca notes are in Markdown", Some(KIND_PROJECT));
        assert!(for_context(&c, "orca notes markdown", Some("orca")).unwrap().is_empty(), "a project row with no project column is for no project");
    }

    #[test]
    fn a_row_with_no_kind_is_classified_by_its_text() {
        let c = mem();
        insert_raw(&c, "I prefer short answers in English", None);
        insert_raw(&c, "zor-ui uses tiny-skia for drawing", None);
        insert_raw(&c, "my sister is called Lina", None);
        assert_eq!(for_context(&c, "short answers please", None).unwrap(), vec!["I prefer short answers in English".to_string()], "a preference by its words");
        assert_eq!(for_context(&c, "the build", Some("zor-ui")).unwrap(), vec!["zor-ui uses tiny-skia for drawing".to_string()], "a project fact by its folder name");
        assert!(for_context(&c, "what is my sister called", None).unwrap().is_empty(), "a plain fact by its text is not context");
    }

    #[test]
    fn a_saved_fact_gets_the_kind_it_is_known_to_have() {
        assert_eq!(fact_kind("Never use emojis in replies", None), KIND_PREFERENCE);
        assert_eq!(fact_kind("Zor-UI uses Rust", Some("zor-ui")), KIND_PROJECT);
        assert_eq!(fact_kind("My sister is called Lina", None), KIND_SEMANTIC);
    }

    #[test]
    fn the_migration_adds_the_columns_to_an_old_database_and_keeps_its_rows() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE memories (id INTEGER PRIMARY KEY, text TEXT NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL);
             CREATE VIRTUAL TABLE memory_fts USING fts5(text);
             INSERT INTO memories (text, created, updated) VALUES ('I prefer short answers', 3, 4);
             INSERT INTO memory_fts (rowid, text) VALUES (1, 'I prefer short answers');",
        ).unwrap();
        setup(&c).unwrap();
        setup(&c).unwrap();
        let cols: Vec<String> = c.prepare("PRAGMA table_info(memories)").unwrap().query_map([], |r| r.get::<_, String>(1)).unwrap().map(Result::unwrap).collect();
        assert!(cols.contains(&"kind".to_string()) && cols.contains(&"project".to_string()), "{cols:?}");
        let rows = all(&c).unwrap();
        assert_eq!(rows, vec![Memory { id: 1, text: "I prefer short answers".into(), created: 3, updated: 4, kind: None, project: None }]);
        assert_eq!(search(&c, "short", 5).unwrap().len(), 1, "the search index still finds it");
    }

    /// Realistic saved facts, saved through the save path (`fact_kind`). The project is the one the window named when it was saved.
    const FIXTURE: &[(&str, Option<&str>)] = &[
        ("I prefer short answers in English", None),
        ("Always answer with code first, then the explanation", None),
        ("Never use emojis in replies", None),
        ("Avoid long lists, use short paragraphs instead", None),
        ("Zor-UI uses Rust and tiny-skia for drawing", Some("zor-ui")),
        ("Zor-UI runs on Windows 11 with Win32 windows", Some("zor-ui")),
        ("Zor-UI tests run with cargo test", Some("zor-ui")),
        ("Orca-notes keeps meeting notes in Markdown", Some("orca-notes")),
        ("My sister is called Lina", None),
        ("I like green tea in the morning", None),
        ("My flat number is 12", None),
        ("My birthday is in March", None),
        ("My laptop is a ThinkPad with 16 GB of RAM", None),
    ];

    #[test]
    fn the_saved_facts_fixture_gives_the_expected_context_for_each_request() {
        let c = mem();
        for (i, (text, project)) in FIXTURE.iter().enumerate() { save(&c, text, *project, i as i64); }
        // a sensitive preference stored the old way, bypassing the save check: it shares words with a request and must not come back
        insert_raw(&c, "I prefer short answers, my PIN code is 4821", Some(KIND_PREFERENCE));
        assert_eq!(all(&c).unwrap().len(), FIXTURE.len() + 1, "no fixture fact merged into another");
        let set = |v: Vec<String>| { let mut v = v; v.sort(); v };
        let s = |t: &[&str]| set(t.iter().map(|x| x.to_string()).collect());
        // a short request in English: the preference that names both words, and the two that share one
        assert_eq!(set(for_context(&c, "give me a short answer in English", None).unwrap()),
            s(&["I prefer short answers in English", "Always answer with code first, then the explanation", "Avoid long lists, use short paragraphs instead"]));
        assert_eq!(set(for_context(&c, "any short tips for replies", None).unwrap()),
            s(&["I prefer short answers in English", "Never use emojis in replies", "Avoid long lists, use short paragraphs instead"]));
        // the active project's facts, and no preference shares a word with the request
        assert_eq!(set(for_context(&c, "what is the status of the zor-ui build", Some("zor-ui")).unwrap()),
            s(&["Zor-UI uses Rust and tiny-skia for drawing", "Zor-UI runs on Windows 11 with Win32 windows", "Zor-UI tests run with cargo test"]));
        assert_eq!(set(for_context(&c, "meeting notes format in orca-notes", Some("orca-notes")).unwrap()),
            s(&["Orca-notes keeps meeting notes in Markdown"]));
        // no active project: no project facts; a plain question matches no preference
        assert!(for_context(&c, "what is the zor-ui build status", None).unwrap().is_empty());
        assert!(for_context(&c, "what is my sister called", None).unwrap().is_empty());
        // the sensitive preference shares "short" and "answers" with this request, and is still left out
        assert_eq!(set(for_context(&c, "short answers please", None).unwrap()),
            s(&["I prefer short answers in English", "Avoid long lists, use short paragraphs instead"]));
        let pin = for_context(&c, "my pin code", None).unwrap();
        assert!(pin.iter().all(|t| !t.contains("PIN")), "a sensitive fact is never retrieved: {pin:?}");
    }

    #[test]
    fn a_damaged_database_is_an_error_not_a_crash() {
        let path = temp_path("damaged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not a database at all, just text that is longer than a page of bytes".repeat(100)).unwrap();
        assert!(open(&path).is_err());
    }

    #[test]
    fn a_project_fact_saved_with_no_project_in_front_is_retrieved_for_its_project() {
        let c = mem();
        let text = "Zor-UI uses Rust and tiny-skia for drawing";
        add_as(&c, text, kind_to_save(text, None), None, 1).unwrap();
        assert_eq!(for_context(&c, "what is the build status", Some("zor-ui")).unwrap(), vec![text.to_string()], "its text names the project");
        assert!(for_context(&c, "what is the build status", Some("notes")).unwrap().is_empty(), "another project does not get it");
        assert!(for_context(&c, "what is the build status", None).unwrap().is_empty(), "no active project, no project fact");
        assert_eq!(kind_to_save("My sister is called Lina", None), None, "a plain fact keeps no kind; the text rule decides");
    }

    #[test]
    fn a_restated_project_fact_keeps_its_project_kind() {
        let c = mem();
        let text = "Zor-UI uses Rust and tiny-skia for drawing";
        add_as(&c, text, Some(KIND_PROJECT), Some("zor-ui"), 1).unwrap();
        // restated with its window away: the save gives no kind and no project, so the stored ones stay
        assert!(matches!(add_as(&c, text, kind_to_save(text, None), None, 2).unwrap(), Outcome::Updated(_)));
        let row = all(&c).unwrap().remove(0);
        assert_eq!((row.kind.as_deref(), row.project.as_deref()), (Some(KIND_PROJECT), Some("zor-ui")));
        assert_eq!(for_context(&c, "what is the build status", Some("zor-ui")).unwrap(), vec![text.to_string()]);
    }

    #[test]
    fn a_restated_fact_with_a_new_explicit_kind_takes_it() {
        let c = mem();
        let text = "Zor-UI uses Rust and tiny-skia for drawing";
        // saved as a project fact, the way `remember` saves it with the window on zor-ui
        add_as(&c, text, kind_to_save(text, Some("zor-ui")), Some("zor-ui"), 1).unwrap();
        // restated with a preference in it: the save path (`kind_to_save`) gives the new kind, and the same fact is updated
        let restated = "Zor-UI uses Rust and tiny-skia for drawing, never use other libraries";
        assert_eq!(kind_to_save(restated, Some("zor-ui")), Some(KIND_PREFERENCE));
        assert!(matches!(add_as(&c, restated, kind_to_save(restated, Some("zor-ui")), Some("zor-ui"), 2).unwrap(), Outcome::Updated(_)));
        let row = all(&c).unwrap().remove(0);
        assert_eq!((row.kind.as_deref(), row.project.as_deref()), (Some(KIND_PREFERENCE), Some("zor-ui")), "new kind, project kept");
        // restated with its window away: no project in front, the kind still comes and the stored project stays
        let again = "Zor-UI uses Rust and tiny-skia for drawing, never use other libraries, ever";
        assert!(matches!(add_as(&c, again, kind_to_save(again, None), None, 3).unwrap(), Outcome::Updated(_)));
        let row = all(&c).unwrap().remove(0);
        assert_eq!((row.kind.as_deref(), row.project.as_deref()), (Some(KIND_PREFERENCE), Some("zor-ui")));
    }

    #[test]
    fn a_sensitive_fact_is_never_recalled() {
        let c = mem();
        // stored the old way, bypassing the save check: it shares words with the query and must not come back
        insert_raw(&c, "I prefer short answers, my PIN code is 4821", Some(KIND_PREFERENCE));
        add_as(&c, "I prefer short answers in English", Some(KIND_PREFERENCE), None, 2).unwrap();
        assert_eq!(recall_in(&c, "short answers", 5), vec!["I prefer short answers in English".to_string()], "only the safe fact");
        assert!(recall_in(&c, "pin code", 5).is_empty(), "a sensitive fact is never recalled");
    }

    #[test]
    fn a_sensitive_project_fact_is_refused_and_never_returned() {
        let c = mem();
        let text = "Zor-UI deploy password is hunter2";
        assert!(add_as(&c, text, kind_to_save(text, Some("zor-ui")), Some("zor-ui"), 1).is_err(), "a password is never saved");
        c.execute("INSERT INTO memories (text, created, updated, kind, project) VALUES (?1, 1, 1, ?2, 'zor-ui')", params![text, KIND_PROJECT]).unwrap();
        assert!(for_context(&c, "zor-ui deploy", Some("zor-ui")).unwrap().is_empty(), "a sensitive row is left out even when it matches");
    }

    #[test]
    fn project_facts_saved_with_no_window_still_cap_at_six_items() {
        let c = mem();
        // distinct wording, so no two of these count as the same fact
        let facts = ["Zor-UI uses Rust for the island", "Zor-UI runs on Windows with Win32", "Zor-UI tests run with cargo",
            "Zor-UI draws with tiny-skia", "Zor-UI stores settings in JSON", "Zor-UI builds a release exe",
            "Zor-UI has a voice module", "Zor-UI keeps a bar layout"];
        for (i, text) in facts.iter().enumerate() { add_as(&c, text, kind_to_save(text, None), None, i as i64).unwrap(); }
        assert_eq!(all(&c).unwrap().len(), facts.len(), "no fact merged into another");
        assert_eq!(for_context(&c, "zor-ui drawing", Some("zor-ui")).unwrap().len(), CONTEXT_ITEMS);
    }
}
