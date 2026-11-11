//! Tasks saved in the memory database (table `tasks`): one row per request, its state, its verdict and its tool calls.
//! A task still running when the app closes is marked Interrupted on the next start. A request whose text looks like a
//! secret is saved as "[private request]".

use super::memory;
use super::state::TaskStatus;
use rusqlite::{params, Connection};
use std::sync::Once;

#[derive(Clone, Debug, PartialEq)]
pub struct Row { pub id: i64, pub request: String, pub status: String, pub verdict: String, pub tool_calls: i64 }

pub fn setup(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tasks (
             id INTEGER PRIMARY KEY,
             request TEXT NOT NULL,
             status TEXT NOT NULL,
             verdict TEXT NOT NULL DEFAULT '',
             tool_calls INTEGER NOT NULL DEFAULT 0,
             started INTEGER NOT NULL,
             updated INTEGER NOT NULL
         );")
}

/// Saves a new task in its first state. Returns its id.
pub fn begin_on(conn: &Connection, request: &str, now: i64) -> rusqlite::Result<i64> {
    let saved = if memory::is_sensitive(request) { "[private request]" } else { request };
    conn.execute("INSERT INTO tasks (request, status, started, updated) VALUES (?1, ?2, ?3, ?3)",
        params![saved, format!("{:?}", TaskStatus::Planning), now])?;
    Ok(conn.last_insert_rowid())
}

pub fn set_on(conn: &Connection, id: i64, status: TaskStatus, verdict: &str, tool_calls: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE tasks SET status = ?1, verdict = ?2, tool_calls = ?3, updated = ?4 WHERE id = ?5",
        params![format!("{status:?}"), verdict, tool_calls, now, id])?;
    Ok(())
}

/// Marks every task that never reached a final state as Interrupted. Returns how many.
pub fn recover_on(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    let open = ["Planning", "Running", "WaitingApproval", "Verifying"];
    let mut n = 0;
    for s in open {
        n += conn.execute("UPDATE tasks SET status = 'Interrupted', updated = ?1 WHERE status = ?2", params![now, s])?;
    }
    Ok(n)
}

/// The newest tasks, newest first.
pub fn recent_on(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare("SELECT id, request, status, verdict, tool_calls FROM tasks ORDER BY id DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![limit], |r| Ok(Row { id: r.get(0)?, request: r.get(1)?, status: r.get(2)?, verdict: r.get(3)?, tool_calls: r.get(4)? }))?;
    rows.collect()
}

fn now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

static RECOVERED: Once = Once::new();

fn open_app() -> Option<Connection> {
    let conn = memory::open(&memory::path()).ok()?;
    setup(&conn).ok()?;
    Some(conn)
}

/// Marks tasks interrupted by an earlier run, once per start. A failure is logged and the request goes on.
pub fn recover_once() {
    RECOVERED.call_once(|| {
        let Some(conn) = open_app() else { return };
        match recover_on(&conn, now()) {
            Ok(0) => {}
            Ok(n) => crate::log::line(&format!("buddy tasks: {n} task(s) were interrupted by the last run")),
            Err(e) => crate::log::line(&format!("buddy tasks: could not check for interrupted tasks: {e}")),
        }
    });
}

/// Saves a new task for `request`. None when the database can't be used; the request still runs.
pub fn begin(request: &str) -> Option<i64> {
    let conn = open_app()?;
    let id = begin_on(&conn, request, now()).ok();
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    id
}

/// Records a task's state. A failure is logged and ignored. The Settings list is cleared, so it shows the change.
pub fn set(id: Option<i64>, status: TaskStatus, verdict: &str, tool_calls: i64) {
    let Some(id) = id else { return };
    let Some(conn) = open_app() else { return };
    if let Err(e) = set_on(&conn, id, status, verdict, tool_calls, now()) { crate::log::line(&format!("buddy tasks: could not save a state: {e}")); }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// One line of counts for the Settings list: how the recent requests ended.
pub fn summary_line(rows: &[Row]) -> String {
    let n = |s: &str| rows.iter().filter(|r| r.status == s).count();
    format!("Recent requests: {} completed, {} unverified, {} partial, {} failed, {} declined, {} cancelled.",
        n("Completed"), n("Unverified"), n("Partial"), n("Failed"), n("Declined"), n("Cancelled"))
}

/// The newest tasks, as the Settings page lists them. Cached, so a repaint never opens the database.
static CACHE: std::sync::Mutex<Option<Vec<Row>>> = std::sync::Mutex::new(None);

pub fn listing() -> Vec<Row> {
    if let Some(v) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() { return v.clone(); }
    let rows = open_app().and_then(|c| recent_on(&c, 10).ok()).unwrap_or_default();
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(rows.clone());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection { let c = Connection::open_in_memory().unwrap(); setup(&c).unwrap(); c }

    #[test]
    fn a_task_is_saved_and_its_state_follows() {
        let c = mem();
        let id = begin_on(&c, "open Spotify", 10).unwrap();
        set_on(&c, id, TaskStatus::Completed, "Completed", 1, 11).unwrap();
        let rows = recent_on(&c, 5).unwrap();
        assert_eq!(rows, vec![Row { id, request: "open Spotify".into(), status: "Completed".into(), verdict: "Completed".into(), tool_calls: 1 }]);
    }

    #[test]
    fn a_secret_request_is_saved_without_its_text() {
        let c = mem();
        begin_on(&c, "my bank password is hunter2", 1).unwrap();
        assert_eq!(recent_on(&c, 1).unwrap()[0].request, "[private request]");
    }

    #[test]
    fn a_task_left_running_is_interrupted_and_a_finished_one_is_not() {
        let c = mem();
        let running = begin_on(&c, "play lofi", 1).unwrap();
        set_on(&c, running, TaskStatus::Running, "", 0, 2).unwrap();
        let done = begin_on(&c, "what time is it", 1).unwrap();
        set_on(&c, done, TaskStatus::Completed, "Completed", 0, 2).unwrap();
        assert_eq!(recover_on(&c, 9).unwrap(), 1, "only the open one");
        let rows = recent_on(&c, 5).unwrap();
        let status = |id| rows.iter().find(|r| r.id == id).unwrap().status.clone();
        assert_eq!(status(running), "Interrupted");
        assert_eq!(status(done), "Completed");
        assert_eq!(recover_on(&c, 10).unwrap(), 0, "nothing left to mark");
    }

    #[test]
    fn the_summary_counts_how_the_recent_requests_ended() {
        let row = |s: &str| Row { id: 1, request: String::new(), status: s.into(), verdict: String::new(), tool_calls: 0 };
        let rows = vec![row("Completed"), row("Completed"), row("Unverified"), row("Partial"), row("Failed"), row("Declined"), row("Cancelled"), row("Running")];
        assert_eq!(summary_line(&rows), "Recent requests: 2 completed, 1 unverified, 1 partial, 1 failed, 1 declined, 1 cancelled.");
        assert_eq!(summary_line(&[]), "Recent requests: 0 completed, 0 unverified, 0 partial, 0 failed, 0 declined, 0 cancelled.");
    }

    #[test]
    fn the_newest_tasks_come_first() {
        let c = mem();
        let a = begin_on(&c, "first", 1).unwrap();
        let b = begin_on(&c, "second", 2).unwrap();
        let ids: Vec<i64> = recent_on(&c, 5).unwrap().iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![b, a]);
    }
}
