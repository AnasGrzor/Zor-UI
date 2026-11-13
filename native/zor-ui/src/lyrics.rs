//! Lyrics from lrclib.net, ported from core.js lyrics() + lib/lyricpick.js. `get` never blocks: it
//! returns Searching, starts a thread, and repaints the UI when the answer is in. Found lyrics are
//! kept in lyrics.json (40 newest tracks) and misses are tried again after 6 h.

use crate::util;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

pub enum Lyrics { Plain(Vec<String>), Synced(Vec<(f64, String)>) }

#[derive(Clone)]
pub enum Lyr { Searching, Missing, Found(Arc<Lyrics>) }

impl Lyr {
    /// A miss on a browser tab (rarely a song) isn't worth a "No lyrics found" note.
    pub fn hidden_for(&self, m: &crate::media::Media) -> bool { matches!(self, Lyr::Missing) && m.browser() }
}

/// What is playing (Media::title / artist / album / duration).
pub struct Track<'a> { pub title: &'a str, pub artist: &'a str, pub album: &'a str, pub dur: f64 }

struct Entry { v: Option<Arc<Lyrics>>, at: f64 }
#[derive(Default)]
struct Store { map: HashMap<String, Entry>, busy: HashSet<String>, failed: HashMap<String, f64>, loaded: bool }
static STORE: Mutex<Option<Store>> = Mutex::new(None);
static FILE_IO: Mutex<()> = Mutex::new(());

fn file() -> std::path::PathBuf { crate::conf::data_dir().join("lyrics.json") }

/// The lyrics for `t`: Searching until the thread started by the first call is done (poll from paint).
pub fn get(t: &Track) -> Lyr {
    if t.title.is_empty() { return Lyr::Missing; }
    let key = format!("{}|{}", t.artist, t.title);
    let now = crate::hub::now_ms();
    // Paint calls this every frame: the disk read happens outside the STORE lock.
    let unloaded = match STORE.lock() { Ok(g) => !g.as_ref().is_some_and(|s| s.loaded), Err(_) => return Lyr::Missing };
    let disk = if unloaded { load() } else { HashMap::new() };
    let Ok(mut g) = STORE.lock() else { return Lyr::Missing };
    let st = g.get_or_insert_with(Store::default);
    if !st.loaded { st.loaded = true; st.map = disk; }
    if let Some(e) = st.map.get(&key) {
        match &e.v {
            Some(l) => return Lyr::Found(l.clone()),
            None if now - e.at < 6.0 * 3600e3 => return Lyr::Missing,
            None => {}
        }
    }
    if st.busy.contains(&key) { return Lyr::Searching; }
    if st.failed.get(&key).is_some_and(|at| now - at < 60e3) { return Lyr::Missing; } // rate-limited / offline: not cached, but not hammered
    st.busy.insert(key.clone());
    let (title, artist, album, dur) = (t.title.to_string(), t.artist.to_string(), t.album.to_string(), t.dur);
    std::thread::spawn(move || {
        let r = fetch(&title, &artist, &album, dur);
        // Encode under STORE, write after it is released; FILE_IO keeps writes in snapshot order.
        let _io = FILE_IO.lock();
        let text = match STORE.lock() {
            Ok(mut g) => g.as_mut().and_then(|st| {
                st.busy.remove(&key);
                match r {
                    Some(found) => {
                        st.map.insert(key, Entry { v: found.map(Arc::new), at: crate::hub::now_ms() });
                        Some(encode(&mut st.map))
                    }
                    None => { st.failed.insert(key, crate::hub::now_ms()); None }
                }
            }),
            Err(_) => None,
        };
        if let Some(text) = text { write(&text); }
        crate::app::post(Box::new(|_| {}));
    });
    Lyr::Searching
}

fn load() -> HashMap<String, Entry> {
    let v: Value = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    v.as_object().into_iter().flatten().map(|(k, e)| (k.clone(), Entry { v: from_json(&e["v"]).map(Arc::new), at: e["at"].as_f64().unwrap_or(0.0) })).collect()
}

fn from_json(v: &Value) -> Option<Lyrics> {
    if let Some(l) = v["synced"].as_array() { return Some(Lyrics::Synced(l.iter().map(|x| (x["t"].as_f64().unwrap_or(0.0), x["text"].as_str().unwrap_or("").into())).collect())); }
    v["plain"].as_array().map(|l| Lyrics::Plain(l.iter().map(|x| x.as_str().unwrap_or("").into()).collect()))
}

/// Keeps the 40 newest entries and returns the JSON text to store.
fn encode(map: &mut HashMap<String, Entry>) -> String {
    let mut keys: Vec<(&String, f64)> = map.iter().map(|(k, e)| (k, e.at)).collect();
    keys.sort_by(|a, b| b.1.total_cmp(&a.1));
    let drop: Vec<String> = keys.iter().skip(40).map(|k| k.0.clone()).collect();
    for k in drop { map.remove(&k); }
    let out: serde_json::Map<String, Value> = map.iter().map(|(k, e)| (k.clone(), json!({ "at": e.at, "v": match e.v.as_deref() {
        Some(Lyrics::Synced(l)) => json!({ "synced": l.iter().map(|(t, s)| json!({ "t": t, "text": s })).collect::<Vec<_>>() }),
        Some(Lyrics::Plain(l)) => json!({ "plain": l }),
        None => Value::Null,
    } }))).collect();
    Value::Object(out).to_string()
}

fn write(text: &str) {
    let tmp = file().with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() { let _ = std::fs::rename(&tmp, file()); }
}

/// lrclib sits behind Cloudflare, which answers 503 whenever it's busy; the same request usually works a second later.
fn lrclib(url: &str) -> (u16, Value) {
    for attempt in 0..4 {
        let (code, body) = util::http_status(url);
        if (code == 429 || code >= 500 || code == 0) && attempt < 3 { std::thread::sleep(Duration::from_secs(attempt + 1)); continue; }
        return (code, serde_json::from_str(&body).unwrap_or(Value::Null));
    }
    (0, Value::Null)
}

/// Some(result) is final (None inside: no lyrics exist); None: try again later (rate-limited, offline).
fn fetch(title: &str, artist: &str, album: &str, dur: f64) -> Option<Option<Lyrics>> {
    let dur = dur.round();
    let mut url = format!("https://lrclib.net/api/get?track_name={}&artist_name={}", util::url_encode(title), util::url_encode(artist));
    if !album.is_empty() { url += &format!("&album_name={}", util::url_encode(album)); }
    if dur > 0.0 { url += &format!("&duration={dur}"); }
    let (code, v) = lrclib(&url);
    if code == 0 || code == 429 || code >= 500 { return None; }
    let mut found = if code < 300 { parse_item(&v) } else { None };
    if found.is_none() {
        for q in queries(title, artist) {
            let (code, list) = lrclib(&format!("https://lrclib.net/api/search?q={}", util::url_encode(&q)));
            if code == 0 || code == 429 || code >= 500 { return None; }
            found = pick(if code < 300 { &list } else { &Value::Null }, dur, title, artist)
                .and_then(|(item, synced)| if synced { parse_item(&item) } else { Some(Lyrics::Plain(plain_text(&item))) });
            if found.is_some() { break; }
        }
    }
    Some(found)
}

/// parse() in lyrics(): synced if it has timings, else plain.
fn parse_item(d: &Value) -> Option<Lyrics> {
    parse_lrc(d["syncedLyrics"].as_str().unwrap_or("")).map(Lyrics::Synced)
        .or_else(|| d["plainLyrics"].as_str().filter(|p| !p.is_empty()).map(|p| Lyrics::Plain(p.split('\n').map(String::from).collect())))
}

/// parseLrc(): "[mm:ss.xx] text" lines, in file order.
pub fn parse_lrc(text: &str) -> Option<Vec<(f64, String)>> {
    let lines: Vec<(f64, String)> = text.split('\n').filter_map(|raw| {
        let raw = raw.trim_end_matches('\r').strip_prefix('[')?;
        let (tag, rest) = raw.split_once(']')?;
        let (m, s) = tag.split_once(':')?;
        if m.is_empty() || !m.bytes().all(|b| b.is_ascii_digit()) || s.is_empty() || s.starts_with('.') || s.ends_with('.') || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') || s.matches('.').count() > 1 { return None; }
        Some((m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?, rest.trim_start().to_string()))
    }).collect();
    (!lines.is_empty()).then_some(lines)
}

/// currentLyricIndex(): the last line that has started (a quarter second early), None before the first.
pub fn current_index(synced: &[(f64, String)], pos: f64) -> Option<usize> {
    synced.iter().take_while(|l| l.0 <= pos + 0.25).count().checked_sub(1)
}

/// plainText(): the lyrics without timings.
pub fn plain_text(item: &Value) -> Vec<String> {
    match item["plainLyrics"].as_str().filter(|p| !p.is_empty()) {
        Some(p) => p.split('\n').map(String::from).collect(),
        None => item["syncedLyrics"].as_str().unwrap_or("").split('\n').map(|l| {
            // strip "[mm:ss.xx]" and one following space
            match l.strip_prefix('[').and_then(|r| r.split_once(']')) {
                Some((tag, rest)) if !tag.is_empty() && tag.bytes().all(|b| b.is_ascii_digit() || b == b':' || b == b'.') => rest.strip_prefix(|c: char| c.is_whitespace()).unwrap_or(rest).to_string(),
                _ => l.to_string(),
            }
        }).collect(),
    }
}

// ------------------------------------------------------------------ query building and picking

/// Words of a title piece, lowercase, split on anything that isn't a letter or digit.
fn tokens(s: &str) -> Vec<String> { s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect() }

/// LYRIC_NOISE in core.js: "official", "lyrics", "full video", "season 2", "remastered"...
fn is_noise(part: &str) -> bool {
    let t = tokens(part);
    const ONE: [&str; 15] = ["official", "lyric", "lyrics", "lyrical", "video", "audio", "hd", "4k", "hq", "visualizer", "visualiser", "ost", "remaster", "remastered", "live"];
    t.iter().enumerate().any(|(i, w)| ONE.contains(&w.as_str())
        || (w == "full" && matches!(t.get(i + 1).map(String::as_str), Some("song" | "video")))
        || (w == "coke" && t.get(i + 1).map(String::as_str) == Some("studio"))
        || (w == "season" && t.get(i + 1).is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))))
}

/// LABELS in core.js: channels that upload for labels, so their name says nothing about the artist.
fn is_label(who: &str) -> bool {
    let w = who.to_lowercase();
    ["t-series", "zee music company", "tips official", "yrf", "speed records", "eros now music"].contains(&w.as_str())
        || w.starts_with("sony music") || w.starts_with("saregama") || w.ends_with("vevo")
}

/// "(Official Video)", "[LYRICS]", "【...】" removed.
fn strip_brackets(s: &str) -> String {
    let (mut out, mut close) = (String::new(), None::<&[char]>);
    for c in s.chars() {
        match close {
            Some(cl) => if cl.contains(&c) { close = None; out.push(' '); },
            None => match c { '[' | '(' | '【' => close = Some(&[']', ')', '】']), _ => out.push(c) },
        }
    }
    if close.is_some() { /* unclosed bracket: JS keeps the text */ return s.to_string(); }
    out
}

/// Splits on | ｜ • and on a dash with spaces around it.
fn split_title(s: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let cs: Vec<char> = s.chars().collect();
    for (i, &c) in cs.iter().enumerate() {
        let dash = matches!(c, '-' | '–' | '—') && i > 0 && cs[i - 1].is_whitespace() && cs.get(i + 1).is_some_and(|n| n.is_whitespace());
        if matches!(c, '|' | '｜' | '•') || dash { parts.push(String::new()); } else { parts.last_mut().map(|p| p.push(c)); }
    }
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty() && !is_noise(p)).collect()
}

/// The text after a "Video:"-style label, matched case-insensitively on the original string.
/// Lowercasing can change the char count ("İ" becomes two chars), so no count is taken from a lowercase copy.
fn after_label<'a>(s: &'a str, labels: &[&str]) -> Option<&'a str> {
    let s = s.trim_start();
    labels.iter().find_map(|p| {
        let n = p.chars().count();
        if !s.chars().take(n).flat_map(char::to_lowercase).eq(p.chars()) { return None; }
        let rest = &s[s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)..];
        rest.trim_start().strip_prefix(':')
    })
}

/// lyricQueries(): free-text searches to try when the exact lookup finds nothing.
pub fn queries(title: &str, artist: &str) -> Vec<String> {
    let mut t = strip_brackets(title);
    if let Some(rest) = after_label(&t, &["lyrical", "full video", "full song", "video", "audio"]).map(str::to_string) { t = rest; }
    let parts = split_title(&t);
    let who = {
        let a = artist.trim();
        let l = a.to_lowercase();
        if l.ends_with("topic") && a[..a.len() - 5].trim_end().ends_with('-') { a[..a.len() - 5].trim_end().trim_end_matches('-').trim_end().to_string() } else { a.to_string() }
    };
    let with_artist = |q: String| if !who.is_empty() && !is_label(&who) && !q.to_lowercase().contains(&who.to_lowercase()) { format!("{q} {who}") } else { q };
    let mut qs: Vec<String> = vec![];
    if parts.len() > 1 { qs.push(with_artist(parts[..2].join(" "))); }
    if let Some(p) = parts.first() { qs.push(with_artist(p.clone())); }
    if let Some(p) = parts.get(1) { qs.push(p.clone()); }
    let mut seen = HashSet::new();
    qs.retain(|q| !q.is_empty() && seen.insert(q.clone()));
    qs
}

fn norm(s: &str) -> String { s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ") }
fn words(s: &str) -> Vec<String> { norm(s).split(' ').filter(|w| w.chars().count() > 2).map(String::from).collect() }

/// pickLyrics(): the lrclib /api/search result that is this track. (item, has timings).
/// Same length (within 5 s) first; else a title that appears in the playing title with a shared
/// artist word, as plain lyrics only (uploads run longer than the song, so timings wouldn't line up).
pub fn pick(list: &Value, dur: f64, title: &str, artist: &str) -> Option<(Value, bool)> {
    let items: Vec<&Value> = list.as_array().into_iter().flatten().filter(|x| x["syncedLyrics"].as_str().is_some_and(|s| !s.is_empty()) || x["plainLyrics"].as_str().is_some_and(|s| !s.is_empty())).collect();
    let has = |x: &Value, k: &str| x[k].as_str().is_some_and(|s| !s.is_empty());
    let near: Vec<&&Value> = items.iter().filter(|x| !(dur > 0.0) || (x["duration"].as_f64().unwrap_or(f64::INFINITY) - dur).abs() <= 5.0).collect();
    if let Some(b) = near.iter().find(|x| has(x, "syncedLyrics")).or_else(|| near.iter().find(|x| has(x, "plainLyrics"))) {
        return Some(((***b).clone(), has(b, "syncedLyrics")));
    }
    let t = norm(title);
    let who = words(&format!("{artist} {title}"));
    items.iter().find(|x| {
        let xt = norm(x["trackName"].as_str().unwrap_or(""));
        !xt.is_empty() && (xt == t || format!(" {t} ").contains(&format!(" {xt} "))) && words(x["artistName"].as_str().unwrap_or("")).iter().any(|w| who.contains(w))
    }).map(|x| ((**x).clone(), false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(name: &str, artist: &str, dur: f64) -> Value {
        json!({ "trackName": name, "artistName": artist, "duration": dur, "plainLyrics": "line one\nline two", "syncedLyrics": "[00:01.00] line one\n[00:03.50] line two" })
    }

    #[test]
    fn picks_like_lyricpick_test() {
        let r = pick(&json!([song("Tum Hi Ho", "Arijit Singh", 261.0)]), 262.0, "Tum Hi Ho", "Arijit Singh").unwrap();
        assert!(r.1);
        // a YouTube video 40 s longer than the song: same song, plain lyrics only
        let (yt_t, yt_a) = ("Teriyaan - Asim Azhar & Aima Baig (Official Video)", "Asim Azhar");
        let list = json!([song("Teriyaan Yaadan", "Sharry Nexus", 220.0), song("Teriyaan", "Asim Azhar, Aima Baig", 261.0)]);
        let r = pick(&list, 301.0, yt_t, yt_a).unwrap();
        assert_eq!(r.0["artistName"], "Asim Azhar, Aima Baig");
        assert!(!r.1);
        // the artist only in the title (a label channel uploaded it) still counts
        assert!(pick(&json!([song("Teriyaan", "Asim Azhar", 261.0)]), 301.0, yt_t, "T-Series").is_some());
        // a different song with a similar name and another artist is rejected
        assert!(pick(&json!([song("Teriyaan Yaadan", "Sharry Nexus", 220.0)]), 301.0, yt_t, yt_a).is_none());
        // a title word alone ("Love" inside "Lovely") isn't a match
        assert!(pick(&json!([song("Love", "Billie Eilish", 100.0)]), 200.0, "Lovely", "Billie Eilish").is_none());
        assert_eq!(plain_text(&json!({ "syncedLyrics": "[00:01.00] line one\n[00:03.50] line two" })), ["line one", "line two"]);
    }

    #[test]
    fn builds_queries() {
        assert_eq!(queries("Teriyaan - Asim Azhar & Aima Baig (Official Video)", "T-Series"), ["Teriyaan Asim Azhar & Aima Baig", "Teriyaan", "Asim Azhar & Aima Baig"]);
        assert_eq!(queries("Lyrical: Tum Hi Ho | Aashiqui 2 | Arijit Singh", "Arijit Singh - Topic"), ["Tum Hi Ho Aashiqui 2 Arijit Singh", "Tum Hi Ho Arijit Singh", "Aashiqui 2"]);
        assert_eq!(queries("Song Name", ""), ["Song Name"]);
        // "İ" lowercases to two chars; the label must still be cut without underflow
        assert_eq!(queries("Video: İİİİİİİ - Song", ""), ["İİİİİİİ Song", "İİİİİİİ", "Song"]);
        assert_eq!(queries("Video: Song", ""), ["Song"]);
        assert!(is_noise("Full Video") && is_noise("Season 2") && !is_noise("Liver") && !is_noise("Alive"));
    }

    #[test]
    fn parses_and_indexes() {
        let l = parse_lrc("[00:01.50] hello\n[01:02.00]\r\nnot a line\n[bad]x\n[00:05] bye").unwrap();
        assert_eq!(l, [(1.5, "hello".to_string()), (62.0, "".to_string()), (5.0, "bye".to_string())]);
        assert!(parse_lrc("nothing").is_none());
        let s = [(1.0, "a".to_string()), (3.0, "b".to_string())];
        assert_eq!(current_index(&s, 0.5), None);
        assert_eq!(current_index(&s, 0.8), Some(0)); // a quarter second early
        assert_eq!(current_index(&s, 99.0), Some(1));
    }
}
