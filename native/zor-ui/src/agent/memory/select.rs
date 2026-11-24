//! Which stored facts a request gets. Only the relevant ones, inside a fixed budget, so a long memory never floods the context:
//! - the core: facts marked `always` (the user's name, how they like answers) and preferences about no project or subject, ranked
//!   by how often they were sent, at most `CORE_CHARS`;
//! - the named: facts whose subject or alias the request names ("Zor UI", "LinkedIn", "my sister"), and facts of the project in
//!   front;
//! - the matched: facts that share a meaningful word with the request. A fact that shares nothing is never sent, so a quiet
//!   request ("open spotify") gets the core only.
//!
//! The project and preference notes of `memory::for_context` (kind- and project-aware, capped on their own) come first.
//! All of it together is at most `FACT_CHARS`. Anything left out can still be found: the model has `search_memory` for that.
//! shortcut: matching is by words and names, so a request that never names its subject ("help me write a post about my job")
//! misses facts filed under "linkedin" unless they share a word; upgrade to embeddings if the log shows many `search_memory`
//! calls after a request.

use super::{is_sensitive, words, STOP};
use rusqlite::{params, Connection};

/// The most characters the core facts take (about 400 tokens).
pub const CORE_CHARS: usize = 1600;
/// The most characters all the facts of one request take (about 700 tokens).
pub const FACT_CHARS: usize = 2800;

/// Words too common in requests to tie one to a fact ("open", "show", "help"). The memory's `STOP` words are left out too.
const WEAK: &[&str] = &["open", "play", "show", "help", "make", "want", "need", "get", "use", "put", "set", "start", "stop", "close",
    "turn", "give", "find", "look", "see", "left", "right", "new", "old", "one", "all", "any", "some", "more", "much", "very",
    "really", "thing", "things", "stuff", "also", "just", "still", "now", "then", "here", "when", "how", "why", "who", "where",
    "which", "will", "its", "our", "out", "off", "yes", "not", "but", "like", "it's", "i'm", "let", "lets", "let's", "today", "time"];

/// One stored fact with what selection reads.
#[derive(Clone, Debug)]
struct Row { id: i64, text: String, kind: Option<String>, project: Option<String>, subject: Option<String>, aliases: String, always: bool, uses: i64, updated: i64 }

fn rows(conn: &Connection) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare("SELECT id, text, kind, project, subject, aliases, always, uses, updated FROM memories")?;
    let it = stmt.query_map([], |r| Ok(Row {
        id: r.get(0)?, text: r.get(1)?, kind: r.get(2)?, project: r.get(3)?, subject: r.get(4)?, aliases: r.get(5)?,
        always: r.get::<_, i64>(6)? != 0, uses: r.get(7)?, updated: r.get(8)?,
    }))?;
    it.collect()
}

/// True when `said` names `name`: "zor ui" and "zor-ui" both name zor-ui (`project::says_name`).
fn names(said: &str, name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && crate::agent::project::says_name(said, &name.to_lowercase())
}

/// A core fact: marked `always`, or a preference tied to no project and no subject.
fn is_core(r: &Row) -> bool {
    r.always || (r.kind.as_deref() == Some(super::KIND_PREFERENCE) && r.project.is_none() && r.subject.is_none())
}

/// The facts for a request, core first, then named, then matched, each group best first, within the budgets. `project` is the
/// project in front (its folder name), if any. Returns (id, text).
fn pick_rows(rows: &[Row], said: &str, project: Option<&str>, budget: usize) -> Vec<(i64, String)> {
    let terms: std::collections::BTreeSet<String> = words(said).into_iter().filter(|w| !STOP.contains(&w.as_str()) && !WEAK.contains(&w.as_str())).collect();
    let here = project.map(str::to_lowercase).filter(|p| !p.is_empty());
    let safe: Vec<&Row> = rows.iter().filter(|r| !is_sensitive(&r.text)).collect();
    let named = |r: &Row| {
        r.subject.as_deref().is_some_and(|s| names(said, s) || here.as_deref() == Some(&s.to_lowercase()))
            || r.aliases.split(',').any(|a| names(said, a))
            || r.project.as_deref().is_some_and(|p| names(said, p) || here.as_deref() == Some(&p.to_lowercase()))
    };
    let shared = |r: &Row| words(&r.text).intersection(&terms).count();
    let mut core: Vec<&Row> = safe.iter().copied().filter(|r| is_core(r)).collect();
    core.sort_by_key(|r| std::cmp::Reverse((r.uses, r.updated)));
    let mut named_rows: Vec<&Row> = safe.iter().copied().filter(|r| !is_core(r) && named(r)).collect();
    named_rows.sort_by_key(|r| std::cmp::Reverse((shared(r), r.uses, r.updated)));
    let mut matched: Vec<&Row> = safe.iter().copied().filter(|r| !is_core(r) && !named(r) && shared(r) > 0).collect();
    matched.sort_by_key(|r| std::cmp::Reverse((shared(r), r.uses, r.updated)));
    let mut out = vec![];
    let mut used = 0;
    for (group, cap) in [(core, CORE_CHARS), (named_rows, budget), (matched, budget)] {
        let mut in_group = 0;
        for r in group {
            let n = r.text.chars().count() + 3;
            // a long fact that does not fit is skipped, and a shorter one after it may still go in
            if in_group + n > cap || used + n > budget { continue; }
            in_group += n;
            used += n;
            out.push((r.id, r.text.clone()));
        }
    }
    out
}

/// `pick` on the stored facts. Each fact sent counts one more use, which ranks it higher next time.
pub fn select_in(conn: &Connection, said: &str, project: Option<&str>) -> rusqlite::Result<Vec<String>> {
    let mut out = super::for_context(conn, said, project)?;
    let used: usize = out.iter().map(|t| t.chars().count() + 3).sum();
    let picked = pick_rows(&rows(conn)?, said, project, FACT_CHARS.saturating_sub(used));
    for (id, text) in picked {
        conn.execute("UPDATE memories SET uses = uses + 1 WHERE id = ?1", params![id])?;
        if !out.contains(&text) { out.push(text); }
    }
    Ok(out)
}

/// (id, text, subject) of every fact, the newest first, for the learning call (`memory::known_lines`).
pub fn subjects(conn: &Connection) -> rusqlite::Result<Vec<(i64, String, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT id, text, subject FROM memories ORDER BY updated DESC")?;
    let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    it.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, text: &str, kind: Option<&str>, subject: Option<&str>, aliases: &str, always: bool) -> Row {
        Row { id, text: text.into(), kind: kind.map(Into::into), project: None, subject: subject.map(Into::into), aliases: aliases.into(), always, uses: 0, updated: id }
    }

    /// The sample memory the selection is checked against: a name, a style preference, project, career and family facts.
    fn sample() -> Vec<Row> {
        vec![
            row(1, "My name is Sam", None, Some("user"), "", true),
            row(2, "I prefer short answers", Some("preference"), None, "", false),
            row(3, "Zor UI is written in Rust with tiny-skia", Some("project"), Some("zor-ui"), "", false),
            row(4, "Zor UI releases are built with cargo build --release", Some("project"), Some("zor-ui"), "", false),
            row(5, "My LinkedIn headline is Rust developer", None, Some("linkedin"), "", false),
            row(6, "My sister is called Lina", None, Some("lina"), "sister", false),
            row(7, "I'm allergic to peanuts", None, Some("health"), "allergy", false),
        ]
    }

    fn texts(said: &str, project: Option<&str>) -> Vec<String> { pick_rows(&sample(), said, project, FACT_CHARS).into_iter().map(|p| p.1).collect() }

    #[test]
    fn a_question_about_zor_ui_gets_its_facts_and_not_linkedin() {
        let got = texts("what's left on Zor UI", None);
        assert!(got.iter().any(|t| t.starts_with("Zor UI is written")) && got.iter().any(|t| t.starts_with("Zor UI releases")), "{got:?}");
        assert!(!got.iter().any(|t| t.contains("LinkedIn")), "{got:?}");
        assert!(!got.iter().any(|t| t.contains("Lina") || t.contains("peanuts")), "{got:?}");
    }

    #[test]
    fn a_quiet_request_gets_the_core_only() {
        let mut got = texts("open spotify", None);
        got.sort();
        assert_eq!(got, ["I prefer short answers", "My name is Sam"]);
    }

    #[test]
    fn an_alias_finds_its_fact() {
        let got = texts("what's my sister's name", None);
        assert!(got.contains(&"My sister is called Lina".to_string()), "{got:?}");
        assert!(!got.iter().any(|t| t.contains("Zor UI") || t.contains("LinkedIn")), "{got:?}");
    }

    #[test]
    fn a_request_naming_two_subjects_gets_both() {
        let got = texts("post about Zor UI on LinkedIn", None);
        assert!(got.iter().any(|t| t.contains("LinkedIn")) && got.iter().any(|t| t.starts_with("Zor UI")), "{got:?}");
    }

    #[test]
    fn the_project_in_front_brings_its_facts_without_being_named() {
        let got = texts("why does the build fail", Some("zor-ui"));
        assert!(got.iter().any(|t| t.starts_with("Zor UI releases")), "{got:?}");
        assert!(!got.iter().any(|t| t.contains("LinkedIn")), "{got:?}");
    }

    #[test]
    fn many_facts_stay_inside_the_budget_and_the_most_used_core_goes_first() {
        let mut rows: Vec<Row> = (0..300).map(|i| row(i, &format!("I prefer option number {i} for rust builds and tests"), Some("preference"), None, "", false)).collect();
        rows[250].uses = 99;
        let got = pick_rows(&rows, "rust builds", None, FACT_CHARS);
        let chars: usize = got.iter().map(|g| g.1.chars().count() + 3).sum();
        assert!(chars <= FACT_CHARS, "{chars}");
        assert!(chars <= CORE_CHARS, "all of these are core facts");
        assert_eq!(got[0].0, 250, "the most used comes first");
    }

    #[test]
    fn a_secret_is_never_sent() {
        let rows = vec![row(1, "my password is hunter2", None, Some("user"), "", true)];
        assert!(pick_rows(&rows, "what is my password", None, FACT_CHARS).is_empty());
    }

    #[test]
    fn project_notes_come_first_and_everything_stays_inside_the_budget() {
        let c = Connection::open_in_memory().unwrap();
        super::super::setup(&c).unwrap();
        super::super::add_as(&c, "zor-ui uses tiny-skia for drawing", Some(super::super::KIND_PROJECT), Some("zor-ui"), 1).unwrap();
        for i in 0..200 { super::super::add_as(&c, &format!("I prefer reply style number {i} with extra words to fill"), Some(super::super::KIND_PREFERENCE), None, 2 + i).unwrap(); }
        let got = select_in(&c, "the build", Some("zor-ui")).unwrap();
        assert_eq!(got[0], "zor-ui uses tiny-skia for drawing");
        let chars: usize = got.iter().map(|t| t.chars().count() + 3).sum();
        assert!(chars <= FACT_CHARS + 3, "{chars}");
    }

    #[test]
    fn sending_a_fact_counts_a_use() {
        let c = Connection::open_in_memory().unwrap();
        super::super::setup(&c).unwrap();
        super::super::add(&c, "My sister is called Lina", 1).unwrap();
        assert_eq!(select_in(&c, "tell me about my sister", None).unwrap(), ["My sister is called Lina"]);
        let uses: i64 = c.query_row("SELECT uses FROM memories", [], |r| r.get(0)).unwrap();
        assert_eq!(uses, 1);
    }

    #[test]
    fn a_learned_fact_replaces_the_one_it_names_and_keeps_its_subject() {
        let c = Connection::open_in_memory().unwrap();
        super::super::setup(&c).unwrap();
        let old = match super::super::add(&c, "I live in Oslo", 1).unwrap() { super::super::Outcome::Added(id) => id, o => panic!("{o:?}") };
        let new = super::super::Learned { text: "I moved to Lisbon".into(), subject: "Home".into(), aliases: vec!["city".into()], always: false, replaces: Some(old) };
        assert_eq!(super::super::keep_learned_in(&c, &new, 2), Ok(super::super::Outcome::Updated(old)));
        let rows = rows(&c).unwrap();
        assert_eq!(rows.len(), 1, "replaced, not added");
        assert_eq!((rows[0].text.as_str(), rows[0].subject.as_deref(), rows[0].aliases.as_str()), ("I moved to Lisbon", Some("home"), "city"));
        assert_eq!(select_in(&c, "which city am I in", None).unwrap(), ["I moved to Lisbon"], "found by its alias");
        let gone = super::super::Learned { text: "My dog is Rex".into(), replaces: Some(999), ..Default::default() };
        assert!(matches!(super::super::keep_learned_in(&c, &gone, 3), Ok(super::super::Outcome::Added(_))), "a missing id adds the fact");
        let secret = super::super::Learned { text: "my password is hunter2".into(), ..Default::default() };
        assert!(super::super::keep_learned_in(&c, &secret, 4).is_err());
    }
}
