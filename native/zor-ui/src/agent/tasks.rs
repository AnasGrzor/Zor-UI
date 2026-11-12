//! Tasks saved in the memory database (table `tasks`): one row per request, its state, its verdict and its tool calls.
//! A task still running when the app closes is marked Interrupted on the next start. A request whose text looks like a
//! secret is saved as "[private request]".

use super::{memory, plan, provider, DESKTOP};
use super::state::TaskStatus;
use rusqlite::{params, Connection};
use std::sync::{atomic::{AtomicBool, Ordering}, Once};

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
             updated INTEGER NOT NULL,
             steps TEXT NOT NULL DEFAULT '[]',
             tokens_in INTEGER NOT NULL DEFAULT 0,
             tokens_out INTEGER NOT NULL DEFAULT 0
         );")?;
    // databases made before a column was added get it; existing rows keep their values
    add_missing(conn, "steps", "TEXT NOT NULL DEFAULT '[]'")?;
    add_missing(conn, "tokens_in", "INTEGER NOT NULL DEFAULT 0")?;
    add_missing(conn, "tokens_out", "INTEGER NOT NULL DEFAULT 0")?;
    // the plan (plan.rs JSON) is nullable: a task with no plan is NULL and resumes as before
    add_missing(conn, "plan", "TEXT")?;
    add_missing(conn, "resumes", "INTEGER NOT NULL DEFAULT 0")?;
    // the conversation the request came from (agent.rs DESKTOP, or a remote chat's id); tasks made before it are desktop tasks
    add_missing(conn, "conversation", "TEXT NOT NULL DEFAULT 'desktop'")
}

fn add_missing(conn: &Connection, col: &str, decl: &str) -> rusqlite::Result<()> { add_column(conn, "tasks", col, decl) }

/// Adds column `col` to `table` when a database made before the column existed lacks it. `table`, `col` and `decl` are fixed
/// names from this crate, never user text.
pub(super) fn add_column(conn: &Connection, table: &str, col: &str, decl: &str) -> rusqlite::Result<()> {
    let has = conn.prepare(&format!("PRAGMA table_info({table})"))?.query_map([], |r| r.get::<_, String>(1))?.any(|n| n.is_ok_and(|n| n == col));
    if !has { conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {decl};"))?; }
    Ok(())
}

/// Saves the tokens a task used in total (the provider's counts, summed over its model calls).
pub fn usage_on(conn: &Connection, id: i64, u: provider::Usage) -> rusqlite::Result<()> {
    conn.execute("UPDATE tasks SET tokens_in = ?1, tokens_out = ?2 WHERE id = ?3", params![u.input as i64, u.output as i64, id])?;
    Ok(())
}

/// Saves a task's token total. A failure is logged and the request is done anyway.
pub fn save_usage(id: Option<i64>, u: provider::Usage) {
    let Some(id) = id else { return };
    if let Some(Err(e)) = with_db(|c| usage_on(c, id, u)) { crate::log::line(&format!("buddy tasks: could not save tokens: {e}")); }
}

/// Saves a task's step record (the JSON text `plan` writes) as it runs.
pub fn steps_on(conn: &Connection, id: i64, steps: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE tasks SET steps = ?1 WHERE id = ?2", params![steps, id])?;
    Ok(())
}

/// The step records of the newest tasks the app interrupted, newest first.
pub fn interrupted_steps_on(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT steps FROM tasks WHERE status = 'Interrupted' ORDER BY id DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
    rows.collect()
}

/// Saves a task's step record. A failure is logged and the request goes on.
pub fn save_steps(id: Option<i64>, steps: &str) {
    let Some(id) = id else { return };
    if let Some(Err(e)) = with_db(|c| steps_on(c, id, steps)) { crate::log::line(&format!("buddy tasks: could not save steps: {e}")); }
}

/// The step records of the newest interrupted tasks, for the side-effect guard. Empty when the database can't be read.
pub fn interrupted_steps() -> Vec<String> {
    with_db(|c| interrupted_steps_on(c, plan::RECENT).ok()).flatten().unwrap_or_default()
}

/// Saves a task's plan (the JSON `plan::Plan::to_json` writes), at the start and after each re-plan.
pub fn plan_on(conn: &Connection, id: i64, plan: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE tasks SET plan = ?1 WHERE id = ?2", params![plan, id])?;
    Ok(())
}

/// An interrupted task with a plan that was not resumed yet, and the conversation it belongs to. Private requests are never resumed.
pub struct Candidate { pub id: i64, pub request: String, pub steps: String, pub plan: String, pub conversation: String }

/// The newest such task, if any.
pub fn resume_candidate_on(conn: &Connection) -> rusqlite::Result<Option<Candidate>> {
    let mut stmt = conn.prepare("SELECT id, request, steps, plan, conversation FROM tasks WHERE status = 'Interrupted' AND plan IS NOT NULL
        AND resumes = 0 AND request <> '[private request]' ORDER BY id DESC LIMIT 1")?;
    let mut rows = stmt.query_map([], |r| Ok(Candidate { id: r.get(0)?, request: r.get(1)?, steps: r.get(2)?, plan: r.get(3)?, conversation: r.get(4)? }))?;
    rows.next().transpose()
}

/// Puts a task back to Running on its own row, and counts the resume so it happens once.
pub fn resume_mark_on(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE tasks SET status = 'Running', resumes = resumes + 1, verdict = 'resumed', updated = ?1 WHERE id = ?2", params![now, id])?;
    Ok(())
}

/// Saves a new task of `conversation` in its first state. Returns its id.
pub fn begin_in_on(conn: &Connection, request: &str, conversation: &str, now: i64) -> rusqlite::Result<i64> {
    let saved = if memory::is_sensitive(request) { "[private request]" } else { request };
    conn.execute("INSERT INTO tasks (request, status, started, updated, conversation) VALUES (?1, ?2, ?3, ?3, ?4)",
        params![saved, format!("{:?}", TaskStatus::Planning), now, conversation])?;
    Ok(conn.last_insert_rowid())
}

/// `begin_in_on` for a request at the desktop.
pub fn begin_on(conn: &Connection, request: &str, now: i64) -> rusqlite::Result<i64> {
    begin_in_on(conn, request, DESKTOP, now)
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

/// The newest task of `conversation`, if it has one. Other conversations' tasks are never returned.
pub fn latest_on(conn: &Connection, conversation: &str) -> rusqlite::Result<Option<Row>> {
    let mut stmt = conn.prepare("SELECT id, request, status, verdict, tool_calls FROM tasks WHERE conversation = ?1 ORDER BY id DESC LIMIT 1")?;
    let mut rows = stmt.query_map(params![conversation], |r| Ok(Row { id: r.get(0)?, request: r.get(1)?, status: r.get(2)?, verdict: r.get(3)?, tool_calls: r.get(4)? }))?;
    rows.next().transpose()
}

/// The newest tasks, newest first.
pub fn recent_on(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare("SELECT id, request, status, verdict, tool_calls FROM tasks ORDER BY id DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![limit], |r| Ok(Row { id: r.get(0)?, request: r.get(1)?, status: r.get(2)?, verdict: r.get(3)?, tool_calls: r.get(4)? }))?;
    rows.collect()
}

fn now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

static RECOVERED: Once = Once::new();

/// Set once the tasks tables exist on the shared connection, so later calls skip the schema checks.
static READY: AtomicBool = AtomicBool::new(false);

/// Runs `setup` on `conn` unless `ready` is set. `ready` is set only after setup succeeds, so a failed setup runs again on the next call.
fn ensure_setup(ready: &AtomicBool, conn: &Connection, setup: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> rusqlite::Result<()> {
    if ready.load(Ordering::Acquire) { return Ok(()); }
    setup(conn)?;
    ready.store(true, Ordering::Release);
    Ok(())
}

/// Runs `f` on the app's shared connection (`memory::with_app`). None when the database can't be opened or set up.
/// `f` must not call back into `tasks::` or `memory::with_app`: that lock is not reentrant.
fn with_db<T>(f: impl FnOnce(&Connection) -> T) -> Option<T> {
    memory::with_app(|c| match ensure_setup(&READY, c, setup) {
        Ok(()) => Some(f(c)),
        Err(e) => { crate::log::line(&format!("buddy tasks: could not set up the table: {e}")); None }
    }).ok().flatten()
}

/// Marks tasks interrupted by an earlier run, once per start. A failure is logged and the request goes on.
pub fn recover_once() {
    RECOVERED.call_once(|| {
        let Some(r) = with_db(|c| recover_on(c, now())) else { return };
        match r {
            Ok(0) => {}
            Ok(n) => crate::log::line(&format!("buddy tasks: {n} task(s) were interrupted by the last run")),
            Err(e) => crate::log::line(&format!("buddy tasks: could not check for interrupted tasks: {e}")),
        }
    });
}

/// Saves a task's plan. A failure is logged and the request goes on.
pub fn save_plan(id: Option<i64>, plan: &str) {
    let Some(id) = id else { return };
    if let Some(Err(e)) = with_db(|c| plan_on(c, id, plan)) { crate::log::line(&format!("buddy tasks: could not save the plan: {e}")); }
}

/// The interrupted task to resume at start, if any. None when the database can't be read.
pub fn resume_candidate() -> Option<Candidate> { with_db(|c| resume_candidate_on(c).ok().flatten()).flatten() }

/// Puts an interrupted task back to Running on its row. A failure is logged and the resume is not done.
pub fn resume_mark(id: i64) -> bool {
    let Some(r) = with_db(|c| resume_mark_on(c, id, now())) else { return false };
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Err(e) = &r { crate::log::line(&format!("buddy tasks: could not resume: {e}")); }
    r.is_ok()
}

/// Records why an interrupted task waits for the user (its verdict). The status stays Interrupted.
pub fn wait(id: i64) {
    let Some(r) = with_db(|c| c.execute("UPDATE tasks SET verdict = ?1 WHERE id = ?2", params![plan::WAITING, id])) else { return };
    if let Err(e) = r { crate::log::line(&format!("buddy tasks: could not save the wait: {e}")); }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Saves a new task for `request` from `conversation`. None when the database can't be used; the request still runs.
pub fn begin_in(request: &str, conversation: &str) -> Option<i64> {
    let id = with_db(|c| begin_in_on(c, request, conversation, now()).ok())?;
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    id
}

/// `begin_in` for a request at the desktop (take-overs started from a fixed phrase, and the tests).
pub fn begin(request: &str) -> Option<i64> { begin_in(request, DESKTOP) }

/// Records a task's state. A failure is logged and ignored. The Settings list is cleared, so it shows the change.
pub fn set(id: Option<i64>, status: TaskStatus, verdict: &str, tool_calls: i64) {
    let Some(id) = id else { return };
    let Some(r) = with_db(|c| set_on(c, id, status, verdict, tool_calls, now())) else { return };
    if let Err(e) = r { crate::log::line(&format!("buddy tasks: could not save a state: {e}")); }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// One line of counts for the Settings list: how the recent requests ended.
pub fn summary_line(rows: &[Row]) -> String {
    let n = |s: &str| rows.iter().filter(|r| r.status == s).count();
    format!("Recent requests: {} completed, {} unverified, {} partial, {} failed, {} declined, {} cancelled.",
        n("Completed"), n("Unverified"), n("Partial"), n("Failed"), n("Declined"), n("Cancelled"))
}

/// Records how a take-over that a request started ended. Only the state and verdict change: the request's tool count stays.
pub fn take_over_ended(id: i64, status: TaskStatus) {
    let Some(r) = with_db(|c| c.execute("UPDATE tasks SET status = ?1, verdict = ?2, updated = ?3 WHERE id = ?4",
        params![format!("{status:?}"), format!("take-over {status:?}"), now(), id])) else { return };
    if let Err(e) = r { crate::log::line(&format!("buddy tasks: could not save the take-over's end: {e}")); }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The newest request of `conversation` and how it ended. None when there is none, or the database can't be read.
pub fn latest_in(conversation: &str) -> Option<Row> { with_db(|c| latest_on(c, conversation).ok().flatten()).flatten() }

/// The newest tasks, as the Settings page lists them. Cached, so a repaint never opens the database.
static CACHE: std::sync::Mutex<Option<Vec<Row>>> = std::sync::Mutex::new(None);

pub fn listing() -> Vec<Row> {
    if let Some(v) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() { return v.clone(); }
    let rows = with_db(|c| recent_on(c, 10).ok()).flatten().unwrap_or_default();
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
    fn a_task_records_its_conversation_and_an_old_one_is_a_desktop_task() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE tasks (id INTEGER PRIMARY KEY, request TEXT NOT NULL, status TEXT NOT NULL,
            verdict TEXT NOT NULL DEFAULT '', tool_calls INTEGER NOT NULL DEFAULT 0, started INTEGER NOT NULL, updated INTEGER NOT NULL);
            INSERT INTO tasks (request, status, started, updated) VALUES ('old', 'Completed', 1, 1);").unwrap();
        setup(&c).unwrap();
        setup(&c).unwrap(); // a second setup finds the column and changes nothing
        let remote = begin_in_on(&c, "text sam", "discord:1:2", 2).unwrap();
        let desk = begin_on(&c, "what time is it", 3).unwrap();
        let conv = |id: i64| -> String { c.query_row("SELECT conversation FROM tasks WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
        assert_eq!(conv(remote), "discord:1:2");
        assert_eq!(conv(desk), DESKTOP);
        let old: String = c.query_row("SELECT conversation FROM tasks WHERE request = 'old'", [], |r| r.get(0)).unwrap();
        assert_eq!(old, DESKTOP, "a task saved before the column is a desktop task");
    }

    #[test]
    fn latest_is_the_newest_task_of_its_own_conversation() {
        let c = mem();
        let desk = begin_on(&c, "open Spotify", 1).unwrap();
        let remote = begin_in_on(&c, "text sam", "discord:1:2", 2).unwrap();
        assert_eq!(latest_on(&c, DESKTOP).unwrap().map(|r| r.id), Some(desk), "the remote task is not the desktop's latest");
        assert_eq!(latest_on(&c, "discord:1:2").unwrap().map(|r| r.id), Some(remote));
        assert!(latest_on(&c, "discord:9:9").unwrap().is_none(), "a chat with no tasks has no latest");
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

    #[test]
    fn steps_saved_before_an_interruption_are_still_there_after_it() {
        let c = mem();
        let id = begin_on(&c, "text sam", 1).unwrap();
        steps_on(&c, id, "[{\"key\":\"k\"}]").unwrap();
        set_on(&c, id, TaskStatus::Running, "", 1, 2).unwrap();
        recover_on(&c, 3).unwrap();
        assert_eq!(interrupted_steps_on(&c, 5).unwrap(), vec!["[{\"key\":\"k\"}]".to_string()]);
    }

    #[test]
    fn a_task_keeps_the_tokens_it_used_and_an_old_database_gets_the_columns() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE tasks (id INTEGER PRIMARY KEY, request TEXT NOT NULL, status TEXT NOT NULL,
            verdict TEXT NOT NULL DEFAULT '', tool_calls INTEGER NOT NULL DEFAULT 0, started INTEGER NOT NULL, updated INTEGER NOT NULL,
            steps TEXT NOT NULL DEFAULT '[]');
            INSERT INTO tasks (request, status, started, updated) VALUES ('old', 'Completed', 1, 1);").unwrap();
        setup(&c).unwrap();
        let id = begin_on(&c, "new", 2).unwrap();
        usage_on(&c, id, provider::Usage { input: 1200, output: 340 }).unwrap();
        let (i, o): (i64, i64) = c.query_row("SELECT tokens_in, tokens_out FROM tasks WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((i, o), (1200, 340));
        let old: (i64, i64) = c.query_row("SELECT tokens_in, tokens_out FROM tasks WHERE request = 'old'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(old, (0, 0), "an old task has no count");
    }

    #[test]
    fn a_database_from_before_steps_gets_the_column_and_keeps_its_rows() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE tasks (id INTEGER PRIMARY KEY, request TEXT NOT NULL, status TEXT NOT NULL,
            verdict TEXT NOT NULL DEFAULT '', tool_calls INTEGER NOT NULL DEFAULT 0, started INTEGER NOT NULL, updated INTEGER NOT NULL);
            INSERT INTO tasks (request, status, started, updated) VALUES ('old', 'Completed', 1, 1);").unwrap();
        setup(&c).unwrap();
        setup(&c).unwrap();
        let id = begin_on(&c, "new", 2).unwrap();
        steps_on(&c, id, "[]").unwrap();
        assert_eq!(recent_on(&c, 5).unwrap().len(), 2, "the old row is kept");
    }

    #[test]
    fn an_interrupted_task_with_a_plan_resumes_once_on_its_own_row() {
        let c = mem();
        let no_plan = begin_on(&c, "what time is it", 1).unwrap();
        let secret = begin_on(&c, "my bank password is hunter2", 2).unwrap();
        let planned = begin_on(&c, "compare two laptops and then book one", 3).unwrap();
        plan_on(&c, planned, "{\"steps\":[]}").unwrap();
        for id in [no_plan, secret, planned] { set_on(&c, id, TaskStatus::Interrupted, "", 0, 4).unwrap(); }
        let got = resume_candidate_on(&c).unwrap().expect("the planned task is the candidate");
        assert_eq!(got.id, planned, "no plan and a private request are never resumed");
        resume_mark_on(&c, planned, 5).unwrap();
        assert_eq!(recent_on(&c, 9).unwrap().len(), 3, "the resume reuses the row, no duplicate");
        assert_eq!(recent_on(&c, 9).unwrap().iter().find(|r| r.id == planned).unwrap().status, "Running");
        assert!(resume_candidate_on(&c).unwrap().is_none(), "nothing left to resume while it runs");
        set_on(&c, planned, TaskStatus::Interrupted, "", 0, 6).unwrap();
        assert!(resume_candidate_on(&c).unwrap().is_none(), "a second interruption is not resumed again");
    }

    #[test]
    fn a_resume_candidate_carries_its_conversation() {
        let c = mem();
        let remote = begin_in_on(&c, "compare two laptops and then book one", "discord:1:2", 1).unwrap();
        plan_on(&c, remote, "{\"steps\":[]}").unwrap();
        set_on(&c, remote, TaskStatus::Interrupted, "", 0, 2).unwrap();
        let got = resume_candidate_on(&c).unwrap().expect("the remote task is the candidate");
        assert_eq!((got.id, got.conversation.as_str()), (remote, "discord:1:2"));
    }

    #[test]
    fn a_failed_setup_is_tried_again_on_the_next_call() {
        let c = Connection::open_in_memory().unwrap();
        let ready = AtomicBool::new(false);
        let fail = |_: &Connection| -> rusqlite::Result<()> { Err(rusqlite::Error::InvalidQuery) };
        assert!(ensure_setup(&ready, &c, fail).is_err());
        assert!(!ready.load(Ordering::Acquire), "a failed setup leaves the flag unset");
        assert!(ensure_setup(&ready, &c, setup).is_ok());
        assert!(ready.load(Ordering::Acquire));
        assert!(ensure_setup(&ready, &c, |_: &Connection| -> rusqlite::Result<()> { panic!("setup runs once") }).is_ok());
    }

    #[test]
    fn a_request_opens_the_shared_connection_at_most_once() {
        // the counter is process-wide, so compare before and after rather than assume a count
        let _db = memory::SHARED_DB_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let before = memory::opens();
        let id = begin("open Spotify").expect("the task is saved");
        for _ in 0..3 { set(Some(id), TaskStatus::Running, "", 1); }
        for _ in 0..5 { save_steps(Some(id), "[]"); }
        save_usage(Some(id), provider::Usage { input: 10, output: 5 });
        assert_eq!(latest_in(DESKTOP).map(|r| r.id), Some(id));
        assert!(memory::opens() - before <= 1, "one open for the whole sequence");
    }
}
