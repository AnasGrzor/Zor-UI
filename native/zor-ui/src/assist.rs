//! Talk to the primary buddy: `zorui.exe --buddy listen` (a hotkey) records your voice, the speech
//! recognizer turns it into text (Windows' own speech recognition, or a Parakeet model if you add one),
//! and this module does what you asked:
//! search the web, open an app or Steam game, play music, or start a message. Phrases it doesn't know go to the
//! AI model from the sidebar, which picks tools and retries when one fails (agent.rs). Nothing is ever sent for you:
//! messages open in the app and wait for you to press Enter.

use crate::{agent::provider, conf::Config, proc, screen, util};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::Ordering, mpsc, Mutex},
};

/// What the buddy was asked to do.
#[derive(Debug, PartialEq)]
pub enum Intent {
    Search(String),
    Open(String),
    /// "play lofi on spotify": `service` is empty when none was named.
    Play { query: String, service: String },
    /// `via`: the app you named ("WhatsApp", "Discord"), or empty.
    Message { to: String, text: String, via: String },
    /// A question about what is on the screen: the AI gets a picture of it.
    Screen(String),
    /// Take over the mouse and keyboard to finish a task (drive.rs). Starts only from these phrases.
    Drive(String),
    /// Anything else: left to the AI.
    Chat(String),
}

/// Lines worker threads want the primary buddy to say: (text, seconds).
static LINES: Mutex<Vec<(String, f32)>> = Mutex::new(Vec::new());
static LISTENER: Mutex<Option<proc::Stream>> = Mutex::new(None);

/// Where the primary buddy should point next: (x, y in screen pixels, label).
static POINT: Mutex<Option<(i32, i32, String)>> = Mutex::new(None);
/// The answer as it is typed out so far (the buddy's bubble shows it; it is not spoken until the final line).
static LIVE: Mutex<Option<String>> = Mutex::new(None);

/// The newest partial answer, once, for the buddy's bubble.
pub fn take_live() -> Option<String> { LIVE.lock().ok().and_then(|mut l| l.take()) }

/// Shows the reply so far: the `say` text of a JSON reply (a tool step shows nothing), or the plain text.
pub fn show_live(acc: &str) {
    let shown = if acc.trim_start().starts_with('{') { crate::agent::partial_say(acc) } else { Some(acc.to_string()) };
    if let Some(s) = shown.filter(|s| !s.trim().is_empty()) { if let Ok(mut l) = LIVE.lock() { *l = Some(s); } }
}

pub fn say_later(s: &str, secs: f32) { if let Ok(mut l) = LINES.lock() { l.push((s.to_string(), secs)); } }
/// Asks the buddy to fly to a spot on the screen (a newer request replaces one he hasn't started).
pub fn point_later(x: i32, y: i32, label: &str) { if let Ok(mut p) = POINT.lock() { *p = Some((x, y, label.to_string())); } }
/// The spot the buddy was asked to point at, once.
pub fn next_point() -> Option<(i32, i32, String)> { POINT.lock().ok().and_then(|mut p| p.take()) }
/// The next line for the buddy's bubble, if any.
pub fn next_line() -> Option<(String, f32)> { LINES.lock().ok().and_then(|mut l| (!l.is_empty()).then(|| l.remove(0))) }

/// How long a failure stays in the bubble (s), and in the island banner (ms): long enough to notice after looking away.
pub const FAIL_SECS: f32 = 30.0;
const FAIL_BANNER_MS: f64 = 60_000.0;

/// A task failed: a long bubble, plus an island banner that stays up longer than the bubble.
pub fn fail_later(s: &str) {
    say_later(s, FAIL_SECS);
    let text = s.to_string();
    crate::app::post(Box::new(move |cx| cx.notif = Some(crate::app::Notif { app: "Zor UI".into(), text, until: crate::hub::now_ms() + FAIL_BANNER_MS, open: String::new() })));
}

/// Seconds a line stays in the bubble: long answers stay longer, between 7 and 30.
pub fn say_secs(s: &str) -> f32 { (s.chars().count() as f32 / 14.0).clamp(7.0, 30.0) }

/// The last answer the buddy gave, for "say that again".
static LAST: Mutex<Option<String>> = Mutex::new(None);
/// Set while a request runs. The cancel flag and the token total are per request, so only one may run at a time.
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Holds the busy flag for one request. Dropping it (the request ends, errors, or panics) frees the buddy for the next.
pub struct Busy;

/// Takes the busy flag, or None while another request holds it.
pub fn try_busy() -> Option<Busy> {
    BUSY.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).ok().map(|_| Busy)
}

/// True while a request runs; the lead buddy shows his working phase then.
pub fn busy() -> bool { BUSY.load(std::sync::atomic::Ordering::Relaxed) }

impl Drop for Busy {
    fn drop(&mut self) { BUSY.store(false, std::sync::atomic::Ordering::SeqCst); }
}
/// How long an answer stays in the island banner (ms), so one you looked away from can still be read.
const ANSWER_BANNER_MS: f64 = 60_000.0;

/// What a click on the buddy's answer banner opens: the chat window (bar.rs).
pub const OPEN_CHAT: &str = "zorui:buddy-chat";

/// Says what a task returned: a success is a short bubble (cut to `MAX_SAY`; the chat has it all) and an island banner whose
/// click opens the chat, a failure stays (see `fail_later`).
fn report(r: Result<String, String>) {
    match r {
        Ok(s) => {
            if let Ok(mut l) = LAST.lock() { *l = Some(s.clone()); }
            let line = short(&s, crate::agent::MAX_SAY);
            say_later(&line, say_secs(&line));
            crate::app::post(Box::new(move |cx| cx.notif = Some(crate::app::Notif { app: "Zor UI".into(), text: line, until: crate::hub::now_ms() + ANSWER_BANNER_MS, open: OPEN_CHAT.into() })));
        }
        Err(e) => fail_later(&e),
    }
}

/// What happened to a request the chat window sent, for that window (`take_events`).
#[derive(Clone, Debug, PartialEq)]
pub enum ChatEvent {
    /// The microphone heard this (empty: nothing was caught, or recording failed).
    Heard(String),
    /// The request `said` ended with this answer or error.
    Answered { said: String, result: Result<String, String> },
}

/// Events of requests sent to a chat (`conv` given), oldest first, as (conversation, event). Capped, so a closed window can't
/// make it grow.
static EVENTS: Mutex<Vec<(String, ChatEvent)>> = Mutex::new(Vec::new());
const EVENTS_MAX: usize = 32;

fn push_event(conv: &str, e: ChatEvent) {
    if let Ok(mut v) = EVENTS.lock() {
        v.push((conv.to_string(), e));
        let over = v.len().saturating_sub(EVENTS_MAX);
        v.drain(..over);
    }
}

/// The request running now as the chat window draws it while it runs: the answer as it is being written, and its tool steps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveChat { pub conversation: String, pub text: String, pub tools: String }

static LIVE_CHAT: Mutex<Option<LiveChat>> = Mutex::new(None);

fn with_live_chat(conv: &str, f: impl FnOnce(&mut LiveChat)) {
    if let Ok(mut l) = LIVE_CHAT.lock() {
        let cur = l.get_or_insert_with(LiveChat::default);
        if cur.conversation != conv { *cur = LiveChat { conversation: conv.to_string(), ..LiveChat::default() }; }
        f(cur);
    }
    crate::chat_ui::wake();
}

/// The model's reply so far (`acc`), for the chat window: the `say` text of a JSON reply (a tool step shows nothing), or the
/// plain text. Each model call starts its text again.
pub fn live_text(conv: &str, acc: &str) {
    let shown = if acc.trim_start().starts_with('{') { crate::agent::partial_say(acc) } else { Some(acc.to_string()) };
    if let Some(s) = shown.filter(|s| !s.trim().is_empty()) { with_live_chat(conv, |l| l.text = s); }
}

/// The request's tool steps so far (`plan::Steps::shown_json`), for the chat window.
pub fn live_tools(conv: &str, tools: &str) { with_live_chat(conv, |l| l.tools = tools.to_string()); }

/// The running request as the chat window draws it, if it is in `conv`.
pub fn live_chat(conv: &str) -> Option<LiveChat> { LIVE_CHAT.lock().ok().and_then(|l| l.clone()).filter(|l| l.conversation == conv) }

/// Every event waiting, oldest first; the chat window keeps those of the chat it shows.
pub fn take_events() -> Vec<(String, ChatEvent)> { EVENTS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default() }

/// Asks to hear the last answer again: "say that again", "repeat that", "what did you say".
pub fn is_replay(said: &str) -> bool {
    let s = said.trim().to_lowercase();
    ["say that again", "say it again", "repeat that", "what did you say"].iter().any(|p| s.contains(p))
}

/// The last answer the buddy gave, or a line saying there is none yet.
fn last_answer() -> String {
    LAST.lock().ok().and_then(|l| l.clone()).unwrap_or_else(|| "I haven't answered anything yet.".into())
}

fn clean(s: &str) -> &str { s.trim().trim_matches(|c: char| ".?!,\"' ".contains(c)) }

/// Splits what you said into an action. Matching is by the first words, so "hey, could you search for
/// cats" and "Search cats." both work.
pub fn parse(said: &str) -> Intent {
    let mut t = clean(said);
    // fillers and "please" at the front
    loop {
        let l = t.to_ascii_lowercase();
        let Some(f) = ["hey", "ok", "okay", "please", "can you", "could you", "would you", "will you"].iter().find(|f| l.strip_prefix(**f).is_some_and(|r| r.is_empty() || r.starts_with([' ', ',']))) else { break };
        t = t[f.len()..].trim_start_matches([' ', ',']);
    }
    // ASCII lowercase keeps every byte offset of `t` (to_lowercase can change length, e.g. "İ"), so the indices below are safe
    let l = t.to_ascii_lowercase();
    let rest = |prefixes: &[&str]| prefixes.iter().find(|p| l.starts_with(**p)).map(|p| clean(&t[p.len()..]).to_string()).filter(|r| !r.is_empty());
    if let Some(q) = rest(&["search for ", "search up ", "search ", "google ", "look up ", "look for "]) { return Intent::Search(q); }
    if let Some(m) = rest(&["send a message to ", "send message to ", "message ", "text ", "whatsapp ", "dm "]) {
        let ml = m.to_ascii_lowercase();
        let mark = [" saying ", " that says ", " that ", " says ", ": "].iter().filter_map(|k| ml.find(k).map(|i| (i, k.len()))).min();
        let (to, text) = match mark {
            Some((i, n)) => (m[..i].trim(), m[i + n..].trim()),
            None => m.split_once(' ').map_or((m.as_str(), ""), |(a, b)| (a, b)),
        };
        // "Sara on WhatsApp": the app you said they are on
        let (to, via) = match to.to_ascii_lowercase().rfind(" on ") {
            Some(i) if !to[..i].trim().is_empty() && !to[i + 4..].trim().is_empty() && to[i + 4..].split_whitespace().count() <= 3 => (to[..i].trim(), to[i + 4..].trim()),
            _ => (to, ""),
        };
        if !to.is_empty() && !text.is_empty() { return Intent::Message { to: to.to_string(), text: text.to_string(), via: via.to_string() }; }
    }
    // "last game" before app_name_of, which would cut " game" off
    if let Some(a) = rest(&["open up ", "open ", "launch ", "start ", "run "]) { return Intent::Open(if crate::lastgame::asks(&a) { "last game".into() } else { app_name_of(&a).to_string() }); }
    if let Some(p) = rest(&["play "]) {
        // "lofi on spotify": the last " on " names the service, unless what follows is too long to be one
        let (query, service) = match p.to_ascii_lowercase().rfind(" on ") {
            Some(i) if !p[..i].trim().is_empty() && !p[i + 4..].trim().is_empty() && p[i + 4..].split_whitespace().count() <= 2 => (p[..i].trim(), p[i + 4..].trim()),
            _ => (p.as_str(), ""),
        };
        return Intent::Play { query: query.strip_prefix("some ").unwrap_or(query).to_string(), service: service.to_string() };
    }
    if let Some(g) = rest(&["take over and ", "take over to ", "take over, ", "take control and ", "take control to ", "do this for me: ", "do this for me, ", "do this for me ", "click "]) {
        // "click" keeps its word: the goal is the whole sentence
        let goal = if l.starts_with("click ") { t.to_string() } else { g };
        return Intent::Drive(goal);
    }
    if mentions_screen(&l) || screen_question(&l) { return Intent::Screen(said.trim().to_string()); }
    Intent::Chat(said.trim().to_string())
}

/// The words that name the screen: it is plainly about what you see.
fn mentions_screen(l: &str) -> bool { ["screen", "this page", "this window"].iter().any(|w| l.contains(w)) }

/// Questions that are about where something is or what this is, which a picture answers. Kept narrow: a general
/// question ("how do I reverse a list") is not sent a screenshot.
fn screen_question(l: &str) -> bool {
    ["where is ", "where's ", "where do i ", "point at ", "point to ", "show me where ", "what am i looking at", "what's this", "what is this", "look at this", "read this", "explain this"]
        .iter().any(|s| l.starts_with(s))
}

/// The entry in `items` ((name, target)) that best matches `query`: exact, then starts with, then contains,
/// then every word present. Shorter names win ties so "Steam" beats "Steam Link".
pub fn best_match<'a>(query: &str, items: &'a [(String, String)]) -> Option<&'a (String, String)> {
    let q = query.to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.is_empty() { return None; }
    items.iter().filter_map(|it| {
        let n = it.0.to_lowercase();
        let rank = if n == q { 0 } else if n.starts_with(&q) { 1 } else if n.contains(&q) { 2 } else if words.iter().all(|w| n.contains(w)) { 3 } else { return None };
        Some(((rank, n.len()), it))
    }).min_by_key(|(k, _)| *k).map(|(_, it)| it)
}

/// Value of `"key"  "value"` on one line of a Valve text file (acf / vdf).
fn vdf_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim().strip_prefix('"')?.strip_prefix(key)?.strip_prefix('"')?;
    rest.trim().strip_prefix('"')?.strip_suffix('"')
}

/// Installed Steam games under one `steamapps` folder: (name, steam://rungameid/ID).
fn steam_games(steamapps: &Path) -> Vec<(String, String)> {
    let Ok(dir) = std::fs::read_dir(steamapps) else { return vec![] };
    dir.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with("appmanifest_")).filter_map(|e| {
        let t = std::fs::read_to_string(e.path()).ok()?;
        let find = |k| t.lines().find_map(|l| vdf_value(l, k)).map(str::to_string);
        Some((find("name")?, format!("steam://rungameid/{}", find("appid")?)))
    }).collect()
}

/// Every Steam library's `steamapps` folder, from the main one's libraryfolders.vdf.
fn steam_libraries(main: &Path) -> Vec<PathBuf> {
    let vdf = std::fs::read_to_string(main.join("libraryfolders.vdf")).unwrap_or_default();
    let mut v = vec![main.to_path_buf()];
    v.extend(vdf.lines().filter_map(|l| vdf_value(l, "path")).map(|p| Path::new(&p.replace("\\\\", "\\")).join("steamapps")));
    v.dedup();
    v
}

/// Start Menu shortcuts under `dir`: (name, path of the .lnk).
fn shortcuts(dir: &Path, depth: u32, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() && depth > 0 { shortcuts(&p, depth - 1, out); }
        else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
            if let Some(n) = p.file_stem() { out.push((n.to_string_lossy().into_owned(), p.to_string_lossy().into_owned())); }
        }
    }
}

/// Input devices for the Microphone setting, named the way Windows' waveIn lists them (the recorder matches on a part of the name).
pub fn mics() -> Vec<String> {
    use windows::Win32::Media::Audio::{waveInGetDevCapsW, waveInGetNumDevs, WAVEINCAPSW};
    unsafe {
        (0..waveInGetNumDevs()).filter_map(|i| {
            let mut caps = WAVEINCAPSW::default();
            (waveInGetDevCapsW(i as usize, &mut caps, size_of::<WAVEINCAPSW>() as u32) == 0).then(|| {
                let name = caps.szPname; // a copy: the struct is packed, so a reference into it would be unaligned
                let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                String::from_utf16_lossy(&name[..n])
            })
        }).collect()
    }
}

/// Speech models found on this PC: (folder, name).
pub fn speech_models() -> Vec<(String, String)> {
    let Ok(rd) = std::fs::read_dir(crate::conf::speech_dir()) else { return vec![] };
    rd.filter_map(Result::ok).filter(|e| e.path().join("tokens.txt").is_file())
        .map(|e| (e.path().to_string_lossy().into_owned(), e.file_name().to_string_lossy().into_owned())).collect()
}

/// Steam's own folder, from the registry.
fn steam_dir() -> Option<PathBuf> {
    use windows::{core::w, Win32::{Foundation::ERROR_SUCCESS, System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ}}};
    let mut buf = [0u16; 520];
    let mut bytes = (buf.len() * 2) as u32;
    unsafe {
        let r = RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Valve\\Steam"), w!("SteamPath"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut bytes));
        (r == ERROR_SUCCESS).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..(bytes as usize / 2).saturating_sub(1)])))
    }
}

/// (subkey, default value) for each entry under HKLM's App Paths: "notepad.exe" and the exe it starts.
fn app_path_keys() -> Vec<(String, String)> {
    use windows::{core::{PCWSTR, PWSTR}, Win32::{Foundation::ERROR_SUCCESS, System::Registry::{RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ}}};
    const BASE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";
    let mut out = vec![];
    unsafe {
        let base: Vec<u16> = BASE.encode_utf16().chain([0]).collect();
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(base.as_ptr()), 0, KEY_READ, &mut key) != ERROR_SUCCESS { return out; }
        for i in 0.. {
            let mut name = [0u16; 260];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(key, i, PWSTR(name.as_mut_ptr()), &mut len, None, PWSTR::null(), None, None) != ERROR_SUCCESS { break; }
            let sub = String::from_utf16_lossy(&name[..len as usize]);
            let full: Vec<u16> = format!(r"{BASE}\{sub}").encode_utf16().chain([0]).collect();
            let mut buf = [0u16; 520];
            let mut bytes = (buf.len() * 2) as u32;
            // a null value name reads the key's default value, the exe path
            let r = RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(full.as_ptr()), PCWSTR::null(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut bytes));
            let path = if r == ERROR_SUCCESS { String::from_utf16_lossy(&buf[..(bytes as usize / 2).saturating_sub(1)]) } else { String::new() };
            out.push((sub, path));
        }
        let _ = RegCloseKey(key);
    }
    out
}

/// App Paths entries as launchable items: "notepad.exe" becomes "notepad", lowercased. Entries with no path are skipped.
fn app_path_items(keys: &[(String, String)]) -> Vec<(String, String)> {
    keys.iter().filter(|(_, p)| !p.trim().is_empty()).map(|(k, p)| {
        let k = k.to_lowercase();
        (k.strip_suffix(".exe").unwrap_or(&k).to_string(), p.trim().trim_matches('"').to_string())
    }).collect()
}

/// Everything "open X" can start: Steam games, then Start Menu apps, then App Paths exes (Notepad has no Start Menu shortcut).
fn launchables(_cfg: &Config) -> Vec<(String, String)> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let steam = steam_dir().unwrap_or_else(|| PathBuf::from(env("ProgramFiles(x86)")).join("Steam")).join("steamapps");
    let mut all: Vec<(String, String)> = steam_libraries(&steam).iter().flat_map(|d| steam_games(d)).collect();
    for base in [PathBuf::from(env("ProgramData")), PathBuf::from(env("APPDATA"))] {
        shortcuts(&base.join(r"Microsoft\Windows\Start Menu\Programs"), 3, &mut all);
    }
    all.extend(app_path_items(&app_path_keys()));
    // Notepad has neither an App Paths key nor a Start Menu shortcut on Windows 11; its exe sits in System32.
    let sys32 = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32");
    all.extend(["notepad"].iter().map(|n| (n.to_string(), sys32.join(format!("{n}.exe")))).filter(|(_, p)| p.is_file()).map(|(n, p)| (n, p.to_string_lossy().into_owned())));
    all
}

/// Voice commands are on (Settings, Voice).
pub fn enabled(cfg: &Config) -> bool { cfg.get("buddies.listen").as_bool().unwrap_or(true) }

/// One action's switch in Settings; on unless turned off.
/// Starts the installed app or game that best matches `name` (Start Menu shortcuts, Steam games).
pub fn launch_app(cfg: &Config, name: &str) -> Result<String, String> {
    if crate::lastgame::asks(name) { return crate::lastgame::open(cfg); }
    if let Some(r) = open_installed(cfg, name) { return r; }
    // "discord and valorant": each app opens here, and the ones not installed are said, without a round trip to the AI
    let parts = split_apps(name);
    if parts.len() > 1 {
        let (mut opened, mut missing) = (vec![], vec![]);
        for p in &parts { match open_installed(cfg, p) { Some(Ok(_)) => opened.push(p.as_str()), _ => missing.push(p.as_str()) } }
        return match (opened.is_empty(), missing.is_empty()) {
            (true, _) => Err(format!("I can't find {} on this PC.", short(name, 40))),
            (false, true) => Ok(format!("Opened {}.", opened.join(" and "))),
            (false, false) => Ok(format!("Opened {}. I can't find {} on this PC.", opened.join(" and "), missing.join(" and "))),
        };
    }
    match web_version(name) {
        Some((label, url)) if crate::bridge::sys::shell_open(url) => Ok(format!("{label} isn't installed, so I opened it in your browser.")),
        Some(_) => Err("Windows couldn't open the browser.".into()),
        None => Err(format!("I can't find {} on this PC.", short(name, 40))),
    }
}

/// The app names in "discord and valorant" or "a, b and c", one each.
pub fn split_apps(name: &str) -> Vec<String> {
    name.split(" and ").flat_map(|p| p.split(',')).map(|p| app_name_of(p.trim()).to_string()).filter(|p| !p.is_empty()).collect()
}

/// Opens the installed app or game `name` names: the best match by its start, else the one installed name that holds every word
/// of it. None when there is no such app.
fn open_installed(cfg: &Config, name: &str) -> Option<Result<String, String>> {
    let items = launchables(cfg);
    let (n, target) = match best_match(name, &items) {
        Some((n, t)) => (n.to_string(), t.to_string()),
        None => {
            let names = app_names(cfg, name);
            let [only] = names.as_slice() else { return None };
            items.iter().find(|(n, _)| n == only).map(|(n, t)| (n.clone(), t.clone()))?
        }
    };
    Some(if crate::bridge::sys::shell_open(&target) { Ok(format!("Opening {}.", short(&n, 40))) } else { Err(format!("Windows couldn't open {}.", short(&n, 40))) })
}

/// Apps that have a website, for when the app isn't installed: (name you say, label, address).
const WEB_APPS: [(&str, &str, &str); 9] = [
    ("spotify", "Spotify", "https://open.spotify.com"),
    ("youtube music", "YouTube Music", "https://music.youtube.com"),
    ("youtube", "YouTube", "https://www.youtube.com"),
    ("netflix", "Netflix", "https://www.netflix.com"),
    ("discord", "Discord", "https://discord.com/app"),
    ("whatsapp", "WhatsApp", "https://web.whatsapp.com"),
    ("gmail", "Gmail", "https://mail.google.com"),
    ("twitch", "Twitch", "https://www.twitch.tv"),
    ("telegram", "Telegram", "https://web.telegram.org"),
];

/// The website for an app name ("Spotify", "spotify app"): an exact match only, so "spotify wrapped" is not Spotify.
pub fn web_version(name: &str) -> Option<(&'static str, &'static str)> {
    let n = name.trim().to_lowercase();
    WEB_APPS.iter().find(|(k, ..)| n == *k).map(|(_, label, url)| (*label, *url))
}

/// The app or game name inside "open the game Valorant" / "the Steam app".
pub fn app_name_of(a: &str) -> &str {
    let a = a.strip_prefix("the ").unwrap_or(a);
    let a = a.strip_prefix("game ").unwrap_or(a);
    a.strip_suffix(" game").or_else(|| a.strip_suffix(" app")).unwrap_or(a)
}

/// Starts the installed app or game whose name begins with `name`; None when there is none (a song title must not start a lookalike app).
pub fn launch_named(cfg: &Config, name: &str) -> Option<Result<String, String>> {
    let items = launchables(cfg);
    let (n, target) = best_match(name, &items).filter(|(n, _)| n.to_lowercase().starts_with(&name.trim().to_lowercase()))?;
    Some(if crate::bridge::sys::shell_open(target) { Ok(format!("Opening {}.", short(n, 40))) } else { Err(format!("Windows couldn't open {}.", short(n, 40))) })
}

/// Up to 8 installed apps and games whose names contain every word of `query`.
pub fn app_names(cfg: &Config, query: &str) -> Vec<String> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    if words.is_empty() { return vec![]; }
    let mut v: Vec<String> = launchables(cfg).into_iter().map(|(n, _)| n).filter(|n| { let l = n.to_lowercase(); words.iter().all(|w| l.contains(w)) }).collect();
    v.sort_by_key(|n| n.len());
    v.dedup();
    v.truncate(8);
    v
}

pub(crate) fn allowed(cfg: &Config, action: &str) -> bool { cfg.get(&format!("buddies.allow.{action}")).as_bool().unwrap_or(true) }

/// Contacts from the settings list "ali=whatsapp:+92300..., sam=discord:123": (name, app, number or id).
fn contacts(cfg: &Config) -> Vec<(String, String, String)> {
    cfg.strs("buddies.contacts").iter().filter_map(|e| parse_contact(e)).collect()
}

/// One "name=app:value" entry as (name, app, value).
pub fn parse_contact(entry: &str) -> Option<(String, String, String)> {
    let (name, rest) = entry.split_once('=')?;
    let (app, value) = rest.split_once(':')?;
    Some((name.trim().to_string(), app.trim().to_lowercase(), value.trim().to_string()))
}

/// The "name=app:value" entry for the Contacts list, or None when the name or value is empty or holds a '=' or ','.
pub fn format_contact(name: &str, app: &str, value: &str) -> Option<String> {
    let (name, value) = (name.trim(), value.trim());
    (!name.is_empty() && !value.is_empty() && !name.contains(['=', ',']) && !value.contains(',')).then(|| format!("{name}={}:{value}", app.trim().to_lowercase()))
}

/// Does some program handle links that start with `scheme://`? Asks Windows the way the shell does when it opens a link. When
/// nothing does, opening one only brings up Windows' "get an app to open this link" box.
pub(crate) fn protocol_handled(scheme: &str) -> bool {
    use windows::{core::{w, HSTRING, PWSTR}, Win32::UI::Shell::{AssocQueryStringW, ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE}};
    let (mut buf, mut n) = ([0u16; 520], 520u32);
    unsafe { AssocQueryStringW(ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE, &HSTRING::from(scheme), w!("open"), PWSTR(buf.as_mut_ptr()), &mut n).is_ok() }
}

/// The app a contact is messaged through, and whether that was a fallback. Any spelling of WhatsApp Web is "whatsappweb". Plain
/// "whatsapp" becomes WhatsApp Web when the WhatsApp app isn't installed (`whatsapp_installed` is false), because its link would
/// open nothing.
fn effective_app(app: &str, whatsapp_installed: bool) -> (String, bool) {
    let a = app.trim().to_lowercase();
    match a.as_str() {
        "whatsappweb" | "whatsapp-web" | "whatsapp web" => ("whatsappweb".into(), false),
        "whatsapp" if !whatsapp_installed => ("whatsappweb".into(), true),
        _ => (a, false),
    }
}

/// The contacts list after saving `entry` ("name=app:value"). Adding (`original` is None) replaces a contact with the same name,
/// since the name is what you say. Editing replaces `original` right where it stands, even under a new name, and also drops any
/// other contact that already has the new name. If `original` is no longer in the list (deleted meanwhile) it is an add.
/// Entries that don't parse are left alone.
pub fn with_contact(list: &[String], original: Option<&str>, entry: String) -> Vec<String> {
    let name_of = |e: &str| parse_contact(e).map(|c| c.0.to_lowercase());
    let new_name = name_of(&entry);
    let at = original.and_then(|o| list.iter().position(|e| e == o));
    let (mut out, mut slot) = (Vec::with_capacity(list.len() + 1), None);
    for (i, e) in list.iter().enumerate() {
        if Some(i) == at { slot = Some(out.len()); continue; }
        if new_name.is_some() && name_of(e) == new_name { continue; }
        out.push(e.clone());
    }
    out.insert(slot.unwrap_or(out.len()), entry);
    out
}

/// What the buddy says after sending a WhatsApp Web link: where the chat opened, or why it didn't. `said` is the plain
/// new-tab line, `fell_back` whether this contact was plain WhatsApp with no WhatsApp app installed.
fn web_reply(name: &str, opened: &crate::webchat::Opened, fell_back: bool, said: String) -> String {
    use crate::webchat::Opened;
    match opened {
        Opened::LiveTab => format!("Opened the chat with {name} in your WhatsApp tab. Press Enter to send."),
        Opened::NewTab if fell_back => format!("WhatsApp isn't installed here, so I opened WhatsApp Web on {name}. Press Enter to send."),
        Opened::NewTab => said,
        Opened::Failed(why) => format!("Your WhatsApp tab is open but I couldn't load the chat: {}. I opened nothing else.", why.trim_end_matches('.')),
    }
}

/// What an app name you said means: any spelling of WhatsApp Web, WhatsApp, or the lowercase name.
fn app_name(s: &str) -> String {
    let l = s.trim().to_lowercase();
    if l.contains("whatsapp") { (if l.contains("web") { "whatsappweb" } else { "whatsapp" }).into() } else { l }
}

/// How a saved contact is reached when you also named an app.
#[derive(Debug, PartialEq)]
enum Route {
    /// Open a chat link through this app with the contact's number or id.
    Link(String),
    /// The saved number or id is no use in that app: the take-over finds them there by name.
    Find,
}

/// The app you named overrides the saved one. WhatsApp and WhatsApp Web share a phone number, so those switch with a link; any
/// other app needs an id the contact doesn't have, so the take-over looks them up by name.
fn route(saved_app: &str, via: &str) -> Route {
    let (saved, said) = (app_name(saved_app), app_name(via));
    let whatsapp = |a: &str| a.starts_with("whatsapp");
    if said.is_empty() || said == saved { Route::Link(saved) }
    else if whatsapp(&saved) && whatsapp(&said) { Route::Link(said) }
    else { Route::Find }
}

/// The task the take-over gets for someone who is not in Contacts: find them in the app, check it is them, type the message,
/// and send it only after the island asks you and you say yes.
pub fn find_goal(to: &str, text: &str, via: &str) -> String {
    let on = if via.is_empty() { String::new() } else { format!(" on {via}") };
    format!("Find {to}{on} and message them. Use the app's search to find the chat with {to}, open it, and check the chat's title is {to}. \
If no chat matches, or several people match, use an ask action to tell the user and then say done. Type exactly this message in the message box: \"{text}\". \
Do not send it yet. Use an ask action with the question \"Send to {to}: {text}?\" and press Enter to send only if the user said yes. Then say done.")
}

/// Opens the app a message goes through before the take-over looks for the person. WhatsApp without the app installed is WhatsApp Web.
fn open_for_find(cfg: &Config, via: &str) {
    let v = via.to_lowercase();
    let (app, _) = effective_app(&v, v != "whatsapp" || protocol_handled("whatsapp"));
    if app == "whatsappweb" { let _ = crate::webchat::open_whatsapp_web("https://web.whatsapp.com/", |u| { crate::bridge::sys::shell_open(u); }); }
    else if !v.is_empty() { if let Some((_, target)) = best_match(&v, &launchables(cfg)) { crate::bridge::sys::shell_open(target); } }
}

/// `to` is not in Contacts: the take-over finds them where you said (or on the screen) and messages them.
fn find_and_message(cfg: &Config, to: &str, text: &str, via: &str) -> Result<String, String> {
    if !crate::drive::enabled(cfg) {
        return Err(format!("I don't have a contact called {to}. Add them under Contacts, or turn on \"Let the buddy take over\" in Settings, Voice, and I'll find them myself."));
    }
    if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) { return Err(AI_OFF.into()); }
    let on = if via.is_empty() { String::new() } else { format!(" on {via}") };
    let shown = format!("find {to}{on} and message them: \"{text}\"");
    let (cfg2, via2) = (Config(cfg.0.clone()), via.to_string());
    // a take-over that still asks in the island: saved as a task, ended as Declined on a no or as its end state when it finishes
    let task = crate::agent::tasks::begin(&shown);
    let done: crate::drive::OnDone = Box::new(move |status| crate::agent::tasks::set(task, status, &format!("{status:?}"), 0));
    // the take-over is a Consequential call, so it passes the gate like any other (the island's question is the same words as before)
    let call = crate::agent::Call::TakeOver(shown.clone());
    let permit = match crate::agent::policy::gate(&call, cfg, &mut crate::agent::ask_island_yes) {
        Ok(p) => p,
        Err(e) if e == crate::agent::DECLINED => { crate::agent::tasks::set(task, crate::agent::state::TaskStatus::Declined, "Declined", 0); return Ok("Okay, I won't.".into()); }
        Err(e) => { crate::agent::tasks::set(task, crate::agent::state::TaskStatus::Failed, "Failed", 0); return Err(e); }
    };
    let r = crate::drive::start_with(&permit, cfg, &find_goal(to, text, via), Some(shown), Some(Box::new(move || open_for_find(&cfg2, &via2))), true, Some(done));
    if r.is_err() { crate::agent::tasks::set(task, crate::agent::state::TaskStatus::Failed, "Failed", 0); }
    r
}

/// The link that opens a chat with `text` ready, and what to tell you to do next.
fn message_link(app: &str, value: &str, to: &str, text: &str) -> Result<(String, String), String> {
    match app {
        "whatsapp" => {
            let phone: String = value.chars().filter(char::is_ascii_digit).collect();
            if phone.is_empty() { return Err(format!("{to} has no phone number in buddies.contacts.")); }
            Ok((format!("whatsapp://send?phone={phone}&text={}", util::url_encode(text)), format!("WhatsApp is open on {to}. Press Enter to send.")))
        }
        "whatsappweb" => {
            let phone: String = value.chars().filter(char::is_ascii_digit).collect();
            if phone.is_empty() { return Err(format!("{to} has no phone number in buddies.contacts.")); }
            Ok((format!("https://web.whatsapp.com/send?phone={phone}&text={}", util::url_encode(text)), format!("WhatsApp Web is open on {to}. Press Enter to send.")))
        }
        "discord" => {
            let id: String = value.chars().filter(char::is_ascii_digit).collect();
            if id.is_empty() { return Err(format!("{to} has no Discord user id in buddies.contacts.")); }
            util::clipboard_set(text);
            Ok((format!("discord://-/users/{id}"), format!("Discord is open on {to}. Paste with Ctrl+V and press Enter.")))
        }
        other => Err(format!("I can't message through {other} yet.")),
    }
}

/// The agent call that an acting intent is the same as, so a fixed phrase passes the same gate as the AI's tool call. None for
/// the intents that change nothing (`Screen` answers a question, `Chat` goes to the AI, whose tool calls are gated one by one) and
/// for `Drive`, which `agent::take_over_with` gates itself.
pub fn call_of(intent: &Intent) -> Option<crate::agent::Call> {
    use crate::agent::Call;
    match intent {
        Intent::Search(q) => Some(Call::Search(q.clone())),
        Intent::Open(n) => Some(Call::Open(n.clone())),
        Intent::Play { query, service } => Some(Call::Play { query: query.clone(), service: service.clone() }),
        Intent::Message { to, text, via } => Some(Call::Message { to: to.clone(), text: text.clone(), via: via.clone() }),
        Intent::Screen(_) | Intent::Drive(_) | Intent::Chat(_) => None,
    }
}

/// Does the action and returns what the buddy says about it: `Ok` when it worked, `Err` when it didn't. An acting phrase goes
/// through `policy::gate` first (`confirm` answers its question), then `run_cleared`.
pub fn run(intent: Intent, cfg: &Config) -> Result<String, String> { run_with(intent, cfg, &mut crate::agent::ask_island_yes) }

/// `run` with the answer to the island's question passed in, so a test can say no.
pub fn run_with(intent: Intent, cfg: &Config, confirm: &mut dyn FnMut(&str) -> bool) -> Result<String, String> {
    if let Intent::Drive(g) = &intent { return crate::agent::take_over_with(cfg, g, confirm); }
    if let Some(call) = call_of(&intent) {
        let permit = crate::agent::policy::gate(&call, cfg, confirm)?;
        return run_cleared(&permit, intent, cfg);
    }
    // Screen and Chat change nothing; a take-over or tool call that the AI makes from a chat is gated in `agent::run_exec`
    match intent {
        Intent::Screen(t) if !screen_on(cfg) && !mentions_screen(&t.to_lowercase()) => run_with(Intent::Chat(t), cfg, confirm),
        Intent::Screen(_) if !screen_on(cfg) => Err(SCREEN_OFF.into()),
        Intent::Screen(_) if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) => Err(AI_OFF.into()),
        Intent::Screen(t) => screen_help(cfg, &t),
        Intent::Chat(_) if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) => Err(AI_OFF.into()),
        Intent::Chat(t) => crate::agent::ask(cfg, &t),
        other => unreachable!("{other:?} is handled above"),
    }
}

/// Does an acting intent. It takes the `Cleared` permit of `policy::gate`, so it can't be reached without the gate.
pub fn run_cleared(_permit: &crate::agent::policy::Cleared, intent: Intent, cfg: &Config) -> Result<String, String> {
    match intent {
        Intent::Search(_) if !allowed(cfg, "search") => Err("Web search is turned off in Settings.".into()),
        Intent::Open(_) | Intent::Play { .. } if !allowed(cfg, "open") => Err("Opening apps is turned off in Settings.".into()),
        Intent::Message { .. } if !allowed(cfg, "message") => Err("Messages are turned off in Settings.".into()),
        Intent::Search(q) => {
            if !crate::bridge::sys::shell_open(&format!("https://www.google.com/search?q={}", util::url_encode(&q))) { return Err("Windows couldn't open the browser.".into()); }
            Ok(format!("Searching for {}.", short(&q, 40)))
        }
        Intent::Open(name) => launch_app(cfg, &name),
        Intent::Play { query, service } => crate::agent::play(cfg, &query, &service),
        Intent::Message { to, text, via } => {
            let all = contacts(cfg);
            let key = to.to_lowercase();
            let found = all.iter().find(|c| c.0.to_lowercase() == key).or_else(|| all.iter().find(|c| c.0.to_lowercase().starts_with(&key)));
            match found {
                None => find_and_message(cfg, &to, &text, &via),
                Some((name, app, value)) => {
                    let app = match route(app, &via) { Route::Link(a) => a, Route::Find => return find_and_message(cfg, name, &text, &via) };
                    // only ask Windows about the WhatsApp app when this contact would use it
                    let (app, fell_back) = effective_app(&app, app != "whatsapp" || protocol_handled("whatsapp"));
                    match message_link(&app, value, name, &text) {
                        Ok((link, said)) if app == "whatsappweb" => {
                            // the live logged-in WhatsApp tab if there is one, else a new tab
                            let opened = crate::webchat::open_whatsapp_web(&link, |u| { crate::bridge::sys::shell_open(u); });
                            let r = web_reply(name, &opened, fell_back, said);
                            if matches!(opened, crate::webchat::Opened::Failed(_)) { Err(r) } else { Ok(r) }
                        }
                        Ok((link, said)) if crate::bridge::sys::shell_open(&link) => Ok(said),
                        Ok(_) => Err(format!("Windows couldn't open {app} for {name}.")),
                        Err(e) => Err(e),
                    }
                }
            }
        }
        // the other intents never reach here: `run_with` handles them
        other => Err(format!("{other:?} is not an action.")),
    }
}

const AI_OFF: &str = "I only know search, open, play and message. The AI is off in Settings.";
const DRIVE_OFF: &str = "Take-over is off. Turn on \"Let the buddy take over\" in Settings, Voice.";

/// The Settings switches a take-over needs, checked before anyone is asked: take-over is on and the AI is on.
pub fn drive_switches(cfg: &Config) -> Result<(), String> {
    if !crate::drive::enabled(cfg) { return Err(DRIVE_OFF.into()); }
    if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) { return Err(AI_OFF.into()); }
    Ok(())
}
const SCREEN_OFF: &str = "Screen help is off. Turn on \"Let the buddy see my screen\" in Settings, Voice.";

/// Settings, Voice, "Let the buddy see my screen". Off unless turned on: the pictures leave this PC.
fn screen_on(cfg: &Config) -> bool { cfg.get("buddies.screen").as_bool().unwrap_or(false) }

/// The model that answers screen questions: the first of the one picked for screen help, the sidebar's AI model and a default
/// that can be sent pictures (`bridge::model_can_see`). When none can, the first one is kept, and the chat says so.
pub fn screen_model(cfg: &Config) -> String {
    let candidates: Vec<String> = [cfg.str("buddies.screenModel"), cfg.str("ai.model"), "gemini-2.5-flash"].into_iter()
        .filter(|m| !m.is_empty()).map(crate::bridge::current_model_id).collect();
    let pick = candidates.iter().find(|m| crate::bridge::model_can_see(m)).or(candidates.first()).cloned().unwrap_or_default();
    crate::log::line(&format!("buddy route: {pick} ({})", crate::agent::routing::Reason::Vision.word()));
    pick
}

/// A screen question from the agent. Nothing is captured while screen help or the AI is switched off.
pub fn see_screen(cfg: &Config, said: &str) -> Result<String, String> {
    if !screen_on(cfg) { return Err(SCREEN_OFF.into()); }
    if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) { return Err(AI_OFF.into()); }
    screen_help(cfg, said)
}

pub fn short(s: &str, n: usize) -> String { match s.char_indices().nth(n) { Some((i, _)) => format!("{}…", &s[..i]), None => s.to_string() } }

/// Runs one `ai-chat` conversation (the JSON the bridge command reads) and returns the whole reply. Gives up when
/// nothing arrives for `wait_secs`.
pub fn chat(cfg: &Config, convo: &str, wait_secs: u64) -> Result<String, String> {
    stream_turn(cfg, convo, wait_secs).map(|t| t.text)
}

/// One model reply that may ask for tools: its text, the tool calls it made, and calls whose input could not be read.
/// A call is `{"id", "name", "input"}`; a bad one is `{"id", "name", "message"}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    pub text: String,
    pub calls: Vec<Value>,
    pub bad: Vec<Value>,
    /// The tokens the provider reported for this reply, when it reported any (`provider::usage_event`).
    pub usage: Option<provider::Usage>,
}

/// Puts the streamed events of one reply together: text deltas join in order, tool calls and their failures are kept, and the
/// provider's running token counts are taken from the last event that gave each one.
pub fn fold_turn(events: &[Value]) -> Turn {
    let mut t = Turn::default();
    for e in events {
        if let Some(d) = e["delta"].as_str() { t.text.push_str(d); }
        if e["tool_use"].is_object() { t.calls.push(e["tool_use"].clone()); }
        if e["tool_error"].is_object() { t.bad.push(e["tool_error"].clone()); }
        if e["usage"].is_object() {
            let u = t.usage.get_or_insert_with(provider::Usage::default);
            if let Some(i) = e["usage"]["input"].as_u64() { u.input = i; }
            if let Some(o) = e["usage"]["output"].as_u64() { u.output = o; }
        }
    }
    t
}

/// Runs one `ai-chat` conversation and returns its reply with any tool calls. A stream error fails the whole reply.
/// Gives up when nothing arrives for `wait_secs`.
pub fn stream_turn(cfg: &Config, convo: &str, wait_secs: u64) -> Result<Turn, String> { stream_turn_with(cfg, convo, wait_secs, |_| {}) }

/// `stream_turn`, and `live` gets the text so far after each delta (the chat window streams it, `live_text`).
pub fn stream_turn_with(cfg: &Config, convo: &str, wait_secs: u64, live: impl FnMut(&str)) -> Result<Turn, String> {
    let turn = stream_turn_live(cfg, convo, wait_secs, live)?;
    if let Some(u) = turn.usage {
        crate::log::line(&format!("buddy usage: {} in, {} out", u.input, u.output));
        provider::add(u);
    }
    Ok(turn)
}

/// `stream_turn` without the token count: the caller adds `Turn::usage` itself (`agent::delegate` does).
pub fn stream_turn_raw(cfg: &Config, convo: &str, wait_secs: u64) -> Result<Turn, String> {
    stream_turn_live(cfg, convo, wait_secs, |_| {})
}

/// `stream_turn_raw`, and `live` gets the text so far after each delta.
pub fn stream_turn_live(cfg: &Config, convo: &str, wait_secs: u64, live: impl FnMut(&str)) -> Result<Turn, String> {
    // each model call's time to first text and total time, and whether it failed, go to the routing figures (provider.rs)
    let model = serde_json::from_str::<Value>(convo).ok().and_then(|v| v["model"].as_str().map(String::from));
    let started = std::time::Instant::now();
    let first = std::cell::Cell::new(None::<u64>);
    let mut live = live;
    let r = stream_turn_timed(cfg, convo, wait_secs, |t| { if first.get().is_none() { first.set(Some(started.elapsed().as_millis() as u64)); } live(t) });
    if let Some(m) = model {
        // the user stopping a call is not the model's failure
        let stopped = matches!(&r, Err(e) if e == crate::browse::STOPPED || e == crate::agent::CANCELLED);
        if !stopped { provider::note_call(&m, r.is_ok(), first.get(), started.elapsed().as_millis() as u64); }
    }
    r
}

fn stream_turn_timed(cfg: &Config, convo: &str, wait_secs: u64, live: impl FnMut(&str)) -> Result<Turn, String> {
    let (tx, rx) = mpsc::channel::<Option<Value>>();
    let (tx2, tx3) = (tx.clone(), tx);
    let stream = proc::stream_stdin(cfg, "ai-chat", &[], convo, move |line| {
        let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if v.is_object() { let _ = tx2.send(Some(v)); }
    }, move || { let _ = tx3.send(None); });
    if stream.is_none() { return Err("I can't reach my brain.".into()); }
    Ok(fold_turn(&collect_events(&rx, std::time::Duration::from_secs(wait_secs), live)?))
}

/// Reads stream events until the end (a None, or the sender dropped). A stream that goes quiet for `wait` is an error, not a
/// short reply, and so is an error event. Split out so the timeout can be tested with a plain channel.
fn collect_events(rx: &mpsc::Receiver<Option<Value>>, wait: std::time::Duration, mut live: impl FnMut(&str)) -> Result<Vec<Value>, String> {
    let mut events = vec![];
    let mut acc = String::new();
    loop {
        match rx.recv_timeout(wait) {
            Ok(Some(v)) => {
                if let Some(e) = v["error"].as_str() { return Err(e.to_string()); }
                if let Some(d) = v["delta"].as_str() { acc.push_str(d); live(&acc); }
                events.push(v);
            }
            Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(events),
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(format!("The AI went quiet for {} seconds, so I stopped.", wait.as_secs())),
        }
    }
}

/// The conversation for a screen question: the recent screen questions and answers as text (`recent_looks`), then the
/// question with one picture per monitor, each introduced by its label.
fn screen_convo(cfg: &Config, model: &str, history: &[(String, String)], said: &str, caps: &[screen::Cap]) -> String {
    convo_at(model, screen::SYSTEM, history, said, caps, crate::agent::temperature(cfg))
}

/// One question with a labeled picture per monitor, under the given system prompt. Take-over uses this at 0.2, so its clicks stay precise.
pub fn convo_with(model: &str, system: &str, said: &str, caps: &[screen::Cap]) -> String { convo_at(model, system, &[], said, caps, 0.2) }

fn convo_at(model: &str, system: &str, history: &[(String, String)], said: &str, caps: &[screen::Cap], temp: f64) -> String {
    let images: Vec<Value> = caps.iter().enumerate()
        .map(|(i, c)| json!({ "mime": "image/jpeg", "data": crate::bridge::base64(&c.jpeg), "label": c.label(i, caps.len(), caps.first()) })).collect();
    let mut messages: Vec<Value> = history.iter().flat_map(|(q, a)| [json!({ "role": "user", "content": q }), json!({ "role": "assistant", "content": a })]).collect();
    messages.push(json!({ "role": "user", "content": said, "images": images }));
    json!({ "model": model, "messages": messages, "system": system, "temperature": temp }).to_string()
}

/// Screen questions asked within `FOLLOW_S` of the one before go out with the last `FOLLOW_MAX` questions and answers, so
/// "and then?" or "the other one" make sense. Text only: the old pictures and control numbers would mislead next to new ones.
const FOLLOW_S: u64 = 120;
const FOLLOW_MAX: usize = 3;
/// Recent screen questions: when, what you asked, what the buddy answered (with what it pointed at).
static LOOKS: Mutex<Vec<(std::time::Instant, String, String)>> = Mutex::new(Vec::new());

/// Drops what is too old or too many: a gap of more than `FOLLOW_S` since the newest ends the conversation.
fn keep_recent(v: &mut Vec<(std::time::Instant, String, String)>, now: std::time::Instant) {
    if v.last().is_some_and(|l| now.duration_since(l.0).as_secs() > FOLLOW_S) { v.clear(); }
    let extra = v.len().saturating_sub(FOLLOW_MAX);
    v.drain(..extra);
}

/// The screen questions and answers a new screen question follows on from.
fn recent_looks() -> Vec<(String, String)> {
    let Ok(mut v) = LOOKS.lock() else { return vec![] };
    keep_recent(&mut v, std::time::Instant::now());
    v.iter().map(|(_, q, a)| (q.clone(), a.clone())).collect()
}

fn remember_look(asked: &str, answer: &str) {
    if let Ok(mut v) = LOOKS.lock() {
        v.push((std::time::Instant::now(), asked.to_string(), answer.to_string()));
        keep_recent(&mut v, std::time::Instant::now());
    }
}

/// Where the buddy should fly for a point the model gave: screen pixels, or None when the model named a screen that isn't there.
fn point_target(caps: &[screen::Cap], p: &screen::Point) -> Option<(i32, i32)> {
    screen::target(caps, p.screen).map(|c| screen::to_screen(c, p.x, p.y))
}

/// The front window's controls for a screen question, numbered as `[POINT:#n]` takes them, at their centres in picture
/// pixels (and the screen, when there are several). Controls on no captured monitor are left out of the text but keep
/// their number, so `n` always names the same control. Empty when there are none.
pub fn screen_elements(caps: &[screen::Cap], els: &[crate::drive::El]) -> String {
    let lines: Vec<String> = els.iter().enumerate().filter_map(|(i, e)| {
        let (c, x, y) = screen::to_picture(caps, e.centre().0, e.centre().1)?;
        let on = if caps.len() > 1 { format!(" on screen{}", c + 1) } else { String::new() };
        Some(format!("[{}] {} \"{}\" at {x},{y}{on}", i + 1, e.role, short(e.name.trim(), 40)))
    }).collect();
    if lines.is_empty() { String::new() } else { format!("\n\nButtons, fields and links in the window in front:\n{}", lines.join("\n")) }
}

/// Where to point and what to outline for the model's point: a listed control's centre and box, or the picture pixel on its
/// monitor with no box. None for a control number that isn't in the list or a screen that isn't there.
fn spot(caps: &[screen::Cap], els: &[crate::drive::El], p: &screen::Point) -> Option<((i32, i32), Option<[i32; 4]>)> {
    match p.el {
        Some(n) => els.get(n - 1).map(|e| (e.centre(), Some([e.rect.0, e.rect.1, e.rect.2, e.rect.3]))),
        None => point_target(caps, p).map(|at| (at, None)),
    }
}

/// Answers a question about the screen: looks at every monitor and the controls of the window in front, asks the model,
/// outlines the control it names and sends the buddy there.
fn screen_help(cfg: &Config, said: &str) -> Result<String, String> {
    let model = screen_model(cfg);
    // checked first, so nothing is captured or sent for a model that can't use it
    if !crate::bridge::model_can_see(&model) { return Err("This model can't see images. Pick another one under Settings, Voice.".into()); }
    let caps = screen::capture_all().map_err(|e| short(&e, 110))?;
    let t = std::time::Instant::now();
    let els = crate::drive::tidy_elements(crate::drive::walk_front_window());
    crate::log::line(&format!("screen help: {} controls in {} ms", els.len(), t.elapsed().as_millis()));
    let asked = format!("{said}{}", screen_elements(&caps, &els));
    let reply = chat(cfg, &screen_convo(cfg, &model, &recent_looks(), &asked, &caps), 60).map_err(|e| short(&e, 110))?;
    if reply.trim().is_empty() { return Err("I got no answer from the AI.".into()); }
    let (spoken, point) = screen::parse_point(&reply);
    let (spoken, more, done) = screen::cut_marks(&spoken);
    let pointed = point.as_ref().filter(|p| !p.label.is_empty()).map(|p| format!(" (I pointed at: {})", p.label)).unwrap_or_default();
    remember_look(said, &format!("{spoken}{pointed}"));
    let step = walk_step(STEPPING.swap(false, Ordering::SeqCst), said);
    let mut said = if spoken.is_empty() { "Here.".to_string() } else { short(&spoken, 160) }; // the bubble is small
    if let Some(p) = &point {
        if let Some(((x, y), rect)) = spot(&caps, &els, p) {
            // a listed control has its own box; for a pixel, UI Automation is asked what is there
            let walk = keep_walking(more, done, GOAL.lock().is_ok_and(|g| screen::how_to(&g)), step);
            let hold = if walk { WALK_WAIT_S as f32 } else { crate::highlight::HOLD_S }; // a step stays up until it is clicked
            let boxed = match rect { Some(r) => Some(r).filter(|r| crate::highlight::show_fit(*r, (x, y), hold)), None => crate::highlight::snap(x, y).inspect(|r| crate::highlight::show_for(*r, hold)) };
            if crate::buddy::on() { point_later(x, y, &p.label); }
            else if boxed.is_none() { said.push_str(" (Turn on a buddy to see where.)"); }
            // one step of several: the next is asked for once you click what it points at
            if walk { watch_step(cfg, boxed.unwrap_or([x - WALK_NEAR, y - WALK_NEAR, x + WALK_NEAR, y + WALK_NEAR])); }
        }
    }
    Ok(said)
}

/// A walkthrough: screen help gives one step at a time (`[MORE]`), and a click on what it pointed at asks for the next.
/// Stops after `WALK_MAX` steps, when nothing is clicked for `WALK_WAIT_S`, or when another request or a cancel comes.
const WALK_MAX: u32 = 8;
const WALK_WAIT_S: u64 = 60;
/// How near (screen px) to a spot with no box a click must land.
const WALK_NEAR: i32 = 40;
/// Bumped by every request and cancel: a step watcher whose number is old stops.
static WALK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// The step of the walkthrough running, and the request it is for.
static STEP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static GOAL: Mutex<String> = Mutex::new(String::new());
/// Set by the step watcher just before it asks, so screen help counts the question as the next step.
static STEPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The step number of this screen question: the next one when the step watcher asked it, else 1 with `said` as the goal.
fn walk_step(stepping: bool, said: &str) -> u32 {
    if !stepping { if let Ok(mut g) = GOAL.lock() { *g = said.trim().to_string(); } }
    let n = if stepping { STEP.load(Ordering::SeqCst) + 1 } else { 1 };
    STEP.store(n, Ordering::SeqCst);
    n
}

/// Whether to wait for a click on this step and then ask for the next: the model said more steps follow, or the question
/// asks how to do something and the model didn't say this step is the last. A pointless extra look costs one AI call;
/// a walkthrough that stops early because the model forgot `[MORE]` leaves you stuck.
fn keep_walking(more: bool, done: bool, how_to: bool, step: u32) -> bool { step < WALK_MAX && !done && (more || how_to) }

/// What the step watcher says once you clicked: the goal goes with it, since older steps drop out of the follow-ups.
pub fn next_step_question(goal: &str) -> String { format!("I did that. What is the next step to: {goal}") }

/// Whether the button went down this poll (it was up the last) at a point inside `area` (left, top, right, bottom).
pub fn clicked_in(was: bool, down: bool, at: (i32, i32), area: [i32; 4]) -> bool {
    down && !was && (area[0]..=area[2]).contains(&at.0) && (area[1]..=area[3]).contains(&at.1)
}

/// Waits on a worker thread for a click inside `area`, then asks screen help for the next step.
fn watch_step(cfg: &Config, area: [i32; 4]) {
    use windows::Win32::UI::{Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON}, WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_SWAPBUTTON}};
    let id = WALK.load(Ordering::SeqCst);
    let cfg = Config(cfg.0.clone());
    std::thread::spawn(move || {
        // the key state is of the physical button: with buttons swapped the primary click is the right one
        let vk = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 { VK_RBUTTON } else { VK_LBUTTON };
        let t0 = std::time::Instant::now();
        let mut was = true; // a button already down when the watch starts is not the click
        // shortcut: polls every 20 ms, so a click shorter than that is missed; a mouse hook would catch every one
        loop {
            if WALK.load(Ordering::SeqCst) != id || t0.elapsed().as_secs() >= WALK_WAIT_S { return; }
            let down = unsafe { GetAsyncKeyState(vk.0 as i32) } < 0;
            let mut at = windows::Win32::Foundation::POINT::default();
            let _ = unsafe { GetCursorPos(&mut at) };
            if clicked_in(was, down, (at.x, at.y), area) { break; }
            was = down;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        crate::highlight::hide();
        std::thread::sleep(std::time::Duration::from_millis(900)); // the app opens the menu or dialog the click asked for
        if WALK.load(Ordering::SeqCst) != id || busy() { return; }
        let goal = GOAL.lock().map(|g| g.clone()).unwrap_or_default();
        crate::log::line(&format!("walkthrough: step {} clicked", STEP.load(Ordering::SeqCst)));
        STEPPING.store(true, Ordering::SeqCst);
        ask_as(&cfg, &next_step_question(&goal), true, None);
    });
}

/// Runs `said` on a worker thread and shows the result in the primary buddy's bubble. A request that comes while another
/// runs is refused at once, so two never share the cancel flag and the token total.
pub fn ask(cfg: &Config, said: &str) { ask_as(cfg, said, false, None) }

/// `ask` in the chat `conv` (the chat window): the turn is saved there and its outcome comes back as a `ChatEvent`.
pub fn ask_in(cfg: &Config, said: &str, conv: &str) { ask_as(cfg, said, false, Some(conv.to_string())) }

/// `ask`, or with `look` the words go to screen help whatever they say (the look hotkey). `conv`: the chat it is said in; None is
/// a request from outside a chat, which gets its own (`agent::desktop_conversation`).
fn ask_as(cfg: &Config, said: &str, look: bool, conv: Option<String>) {
    let Some(busy) = try_busy() else {
        let busy_line = "I'm still working on the last request.";
        say_later(busy_line, 3.0);
        if let Some(c) = &conv { push_event(c, ChatEvent::Answered { said: said.to_string(), result: Err(busy_line.into()) }); }
        return;
    };
    WALK.fetch_add(1, Ordering::SeqCst); // a new request ends a walkthrough's wait for a click, and its box
    crate::highlight::hide();
    let (cfg, said) = (Config(cfg.0.clone()), said.to_string());
    std::thread::spawn(move || {
        let _busy = busy; // held until this request ends, also when it panics
        if is_replay(&said) {
            let a = last_answer();
            say_later(&a, say_secs(&a));
            if let Some(c) = &conv { push_event(c, ChatEvent::Answered { said: said.clone(), result: Ok(a) }); }
            return;
        }
        say_later("On it…", 3.0);
        // one task, one conversation: the chat it was said in, else a new one
        let chat = conv.clone().unwrap_or_else(crate::agent::desktop_conversation);
        // Fixed commands bypass the agent's history writer. Keep their recognized
        // words and result too, before Answered clears the chat's pending line.
        let history_before = crate::agent::history::VERSION.load(Ordering::Relaxed);
        crate::agent::set_pc_conversation(Some(chat.clone()));
        let r = if look { see_screen(&cfg, said.trim()) }
            // a custom command from Settings: each of its steps runs as if it had been said
            else if let Some(steps) = crate::commands::matching(&cfg.strs("buddies.commands"), &said) { crate::commands::run_steps(&steps, |s| do_said(&cfg, s)) }
            else { do_said(&cfg, &said) };
        crate::agent::set_pc_conversation(None);
        if let Ok(answer) = &r {
            if crate::agent::history::VERSION.load(Ordering::Relaxed) == history_before {
                crate::agent::history::save(&chat, &said, answer);
            }
        }
        if let Ok(mut l) = LIVE_CHAT.lock() { *l = None; } // the request is over: its saved turn shows from now on
        if let Some(c) = &conv { push_event(c, ChatEvent::Answered { said: said.clone(), result: r.clone() }); }
        report(r);
    });
}

/// Does what `said` asks: a fixed command when it parses as one, and the AI when it doesn't or the fixed command failed.
fn do_said(cfg: &Config, said: &str) -> Result<String, String> {
    // Intent is not Clone and run consumes it, so the (pure, cheap) parse runs twice
    let ai_on = cfg.get("buddies.listenAi").as_bool().unwrap_or(true);
    match run(parse(said), cfg) {
        Err(e) if falls_back(&parse(said), &e, ai_on) => crate::agent::ask_with_note(cfg, said.trim(), &fallback_note(&e)),
        r => r,
    }
}

/// True when a fixed command failed and the AI should get the sentence instead. Not when the AI is off, not for a
/// feature the user turned off in Settings, and not for Message, Screen, Drive or Chat (those asked or went to the AI already).
pub fn falls_back(intent: &Intent, err: &str, ai_on: bool) -> bool {
    // Playback already exhausts its routes. Handing it to the model starts them again.
    ai_on && matches!(intent, Intent::Open(_) | Intent::Search(_)) && !err.contains("turned off")
}

/// The note the AI gets with the request after a fixed command failed: why the fixed command could not do it. It goes to the AI
/// only; the request itself is saved as the user said it.
pub fn fallback_note(err: &str) -> String {
    format!("(I first tried it as a fixed command and it failed: {err} Find another way.)")
}

/// Bridge flags that turn off echo cancellation or noise suppression, when Settings has them off.
fn clean_args(cfg: &Config) -> Vec<String> {
    let off = |k: &str| cfg.get(k).as_bool() == Some(false);
    [("buddies.echoCancel", "--no-echo"), ("buddies.denoise", "--no-denoise")].iter().filter(|(k, _)| off(k)).map(|(_, f)| f.to_string()).collect()
}

/// Bridge arguments for the speech model folder and microphone name; empty ones are left out.
fn listen_args(model: &str, mic: &str, mode: &str) -> Vec<String> {
    let mode = if mode == "auto" { "" } else { mode }; // Auto is what the recorder does without the flag
    [("--model", model), ("--mic", mic), ("--mode", mode)].iter().filter(|(_, v)| !v.trim().is_empty()).flat_map(|(k, v)| [k.to_string(), v.trim().to_string()]).collect()
}

/// Starts recording: the bridge prints states and then the text, which goes to `ask`. With `look` what you say is a
/// question about the screen (the look hotkey).
pub fn listen_as(cfg: &Config, look: bool) { listen_into(cfg, look, None) }

/// `listen_as` for the chat `conv` (its mic button and Live mode): the words go to that chat, and what was heard comes back
/// as a `ChatEvent`. With a chat it always stops on silence (Auto), whatever the dictation setting says, so Live mode can
/// listen hands-free.
pub fn listen_into(cfg: &Config, look: bool, conv: Option<String>) {
    // a question from the take-over is waiting: your words answer it. Otherwise speaking to the buddy hands a take-over's mouse back.
    let answering = crate::drive::waiting() || crate::bridge::permit::waiting();
    if !answering { crate::drive::request_stop(); crate::browse::request_stop(); }
    let cfg2 = Config(cfg.0.clone());
    let model = cfg.str("buddies.model").to_string();
    let mic = cfg.str("buddies.mic").to_string();
    let mode = if conv.is_some() { "auto" } else { cfg.str("buddies.listenMode") };
    crate::pill::set_mode(mode);
    crate::pill::set_look(look);
    let mut args = listen_args(&model, &mic, mode);
    args.extend(clean_args(cfg));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let s = proc::stream(cfg, "buddy-listen", &args, move |line| {
        let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if let Some(l) = v["level"].as_f64() { crate::pill::meter(l as f32, v["peak"].as_f64().unwrap_or(0.0) as f32, v["floor"].as_f64().unwrap_or(0.0) as f32); }
        if let Some(t) = v["partial"].as_str() { crate::pill::partial(t); }
        match v["state"].as_str() {
            Some("listening") => crate::pill::set(crate::pill::LISTENING),
            Some("thinking") => crate::pill::set(crate::pill::THINKING),
            _ => {}
        }
        if let Some(e) = v["error"].as_str() {
            crate::pill::set(0);
            fail_later(&short(e, 110));
            if let Some(c) = &conv { push_event(c, ChatEvent::Heard(String::new())); }
        }
        if let Some(t) = v["text"].as_str() {
            crate::pill::partial(t.trim());
            crate::pill::set(0);
            if let Some(c) = &conv { push_event(c, ChatEvent::Heard(t.trim().to_string())); }
            if t.trim().is_empty() { say_later("I didn't catch that.", 3.0); }
            else if answering && (crate::drive::reply_spoken(t) || crate::bridge::permit::answer_with(t)) {} // the take-over or a Claude Code permission heard you
            else { ask_as(&cfg2, t, look, conv.clone()); }
        }
    });
    if s.is_none() { crate::pill::set(0); fail_later("I can't start listening."); }
    if let Ok(mut l) = LISTENER.lock() { *l = s; } // replacing it stops a recording already running
}

/// The hotkey pressed again while recording in Toggle mode: stop and send what was said. False in any other mode.
pub fn press_again(cfg: &Config) -> bool {
    let toggle = cfg.str("buddies.listenMode") == "toggle" && crate::pill::listening();
    if toggle { crate::bridge::listen::request_finish(); }
    toggle
}

/// The always-on listener for "hey <name>" (buddies.wake), with the bridge arguments it was started with.
static WAKE: Mutex<Option<(proc::Stream, Vec<String>)>> = Mutex::new(None);
/// The latest settings, for the listener's thread: names and switches changed in Settings apply without restarting it.
static WAKE_CFG: Mutex<Option<Config>> = Mutex::new(None);

/// Starts or stops the "hey <name>" listener to match Settings, and restarts it when the microphone or speech model changes.
pub fn wake_sync(cfg: &Config) {
    let on = enabled(cfg) && cfg.get("buddies.wake").as_bool().unwrap_or(false);
    if let Ok(mut c) = WAKE_CFG.lock() { *c = Some(Config(cfg.0.clone())); }
    let Ok(mut w) = WAKE.lock() else { return };
    let mut args = listen_args(cfg.str("buddies.model"), cfg.str("buddies.mic"), "auto");
    args.extend(clean_args(cfg));
    args.push("--wake".into());
    if !on { *w = None; return; }
    if w.as_ref().is_some_and(|(_, a)| *a == args) { return; }
    *w = None; // the old one lets go of the model first
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let s = proc::stream(cfg, "buddy-listen", &a, |line| {
        let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if let Some(e) = v["error"].as_str() { crate::log::line(&format!("wake listener: {e}")); }
        let Some(heard) = v["heard"].as_str() else { return };
        // the hotkey's recording or a running request already has these words
        if crate::pill::active() || busy() { crate::log::line(&format!("wake: ignored {heard:?}, already listening or busy")); return; }
        let Some(cfg) = WAKE_CFG.lock().ok().and_then(|c| c.as_ref().map(|c| Config(c.0.clone()))) else { return };
        let words = wake_words(&cfg);
        match after_wake(heard, &words) {
            None => crate::log::line(&format!("wake: heard {heard:?}, no wake word {words:?} in it")),
            // just the name: listen for the request, as the hotkey does
            Some(rest) if rest.is_empty() => {
                crate::log::line(&format!("wake: heard {heard:?}, only the wake word: listening for the request"));
                crate::app::post(Box::new(|cx| cx.actions.push(crate::app::Action::Command("buddy listen".into()))));
            }
            Some(rest) => { crate::log::line(&format!("wake: heard {heard:?}, request {rest:?}")); ask(&cfg, &rest); }
        }
    });
    *w = s.map(|s| (s, args));
}

/// What wakes the buddy: the wake word set in Settings (buddies.wakeWord), else the buddies' names.
fn wake_words(cfg: &Config) -> Vec<String> {
    let own = cfg.str("buddies.wakeWord").trim();
    if own.is_empty() { wake_names(cfg) } else { vec![own.to_string()] }
}

/// The names the buddies answer to when no wake word is set: each one's own name, else its character's. The second buddy's
/// only while it is on.
pub fn wake_names(cfg: &Config) -> Vec<String> {
    ["a", "b"].iter().filter(|k| **k == "a" || cfg.bool("buddies.b.on")).map(|k| {
        let own = cfg.str(&format!("buddies.{k}.name")).trim().to_string();
        if !own.is_empty() { return own; }
        crate::cast::pick_with(crate::cast::custom_on(cfg), cfg.get("buddies.custom"), cfg.str(&format!("buddies.{k}.character"))).0.label.to_string()
    }).collect()
}

/// Words said before a name to call the buddy. "a" and "hay" are how recognizers often write "hey".
const WAKE_GREETINGS: [&str; 8] = ["hey", "hi", "hello", "ok", "okay", "yo", "a", "hay"];

/// The request after the wake word in `heard` ("Hey Gojo, play woh" gives "play woh", "Gojo" gives ""), or None when there is none.
/// The name counts at the start, or anywhere right after a greeting. Recognizers split names and get a letter wrong, so
/// "Go Jo" and "Gojoe" count too.
pub fn after_wake(heard: &str, names: &[String]) -> Option<String> {
    let bare = |w: &str| w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let words: Vec<&str> = heard.split_whitespace().collect();
    let greeting = |i: usize| WAKE_GREETINGS.contains(&bare(words[i]).as_str());
    let starts = std::iter::once(usize::from(!words.is_empty() && greeting(0))).chain((0..words.len()).filter(|&i| greeting(i)).map(|i| i + 1));
    for at in starts {
        for name in names.iter().map(|n| bare(n)).filter(|n| !n.is_empty()) {
            for n in 1..=3 {
                let Some(said) = words.get(at..at + n) else { break };
                if close(&said.iter().map(|w| bare(w)).collect::<String>(), &name) { return Some(words[at + n..].join(" ")); }
            }
        }
    }
    None
}

/// Same word, or one letter off for names of four letters or more.
fn close(said: &str, name: &str) -> bool {
    if said == name { return true; }
    let (a, b): (Vec<char>, Vec<char>) = (said.chars().collect(), name.chars().collect());
    if b.len() < 4 || a.len().abs_diff(b.len()) > 1 { return false; }
    // edit distance, one row at a time
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = std::mem::replace(&mut row[0], i + 1);
        for (j, cb) in b.iter().enumerate() {
            let next = (prev + usize::from(ca != cb)).min(row[j] + 1).min(row[j + 1] + 1);
            prev = std::mem::replace(&mut row[j + 1], next);
        }
    }
    row[b.len()] <= 1
}

/// Stops a recording in progress and hides the pill (the hotkey pressed a second time).
pub fn cancel() {
    crate::pill::set(0);
    WALK.fetch_add(1, Ordering::SeqCst);
    crate::highlight::hide();
    if let Ok(mut l) = LISTENER.lock() { *l = None; } // dropping the stream kills the recorder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wake_word_starts_the_request_and_forgives_how_names_are_heard() {
        let names = vec!["Gojo".to_string(), "Mary Jane".to_string()];
        let w = |s: &str| after_wake(s, &names);
        assert_eq!(w("Hey Gojo, play woh.").as_deref(), Some("play woh."));
        assert_eq!(w("Hey, go to.").as_deref(), Some(""), "how Parakeet heard the user's own \"hey Gojo\"");
        assert_eq!(w("gojo play woh").as_deref(), Some("play woh"));
        assert_eq!(w("A go jo open discord").as_deref(), Some("open discord"), "split name, misheard hey");
        assert_eq!(w("hey gojoe pause").as_deref(), Some("pause"), "one letter off");
        assert_eq!(w("so um hey Gojo play woh").as_deref(), Some("play woh"), "after a greeting mid-sentence");
        assert_eq!(w("hey mary jane what time is it").as_deref(), Some("what time is it"));
        assert_eq!(w("Hey Gojo").as_deref(), Some(""), "just the name: listen next");
        assert_eq!(w("I told gojo to play woh"), None, "the name mid-sentence without a greeting");
        assert_eq!(w("hey there play woh"), None);
        assert_eq!(w(""), None);
        assert_eq!(after_wake("hey yuj play", &["Yuji".to_string()]).as_deref(), Some("play"));
        assert_eq!(after_wake("hey ri play", &["Rin".to_string()]), None, "short names must match exactly");
    }

    #[test]
    fn the_buddies_answer_to_their_own_name_else_their_characters() {
        let c = cfg(serde_json::json!({ "buddies": { "a": { "character": "gojo", "name": "" }, "b": { "on": false, "character": "geto", "name": "Sugu" } } }));
        assert_eq!(wake_names(&c), ["Gojo"]);
        let c = cfg(serde_json::json!({ "buddies": { "a": { "character": "gojo", "name": " Satoru " }, "b": { "on": true, "character": "geto", "name": "" } } }));
        assert_eq!(wake_names(&c), ["Satoru", "Geto"]);
        assert_eq!(wake_words(&c), ["Satoru", "Geto"], "no wake word set: the names wake them");
        let c = cfg(serde_json::json!({ "buddies": { "wakeWord": " Jarvis ", "a": { "character": "gojo", "name": "Satoru" } } }));
        assert_eq!(wake_words(&c), ["Jarvis"], "a wake word set takes the place of the names");
    }

    #[test]
    fn a_second_request_is_refused_while_one_runs_and_the_flag_frees_on_drop() {
        let first = try_busy().expect("the first request gets the flag");
        assert!(try_busy().is_none(), "a request during the first is refused");
        drop(first);
        assert!(try_busy().is_some(), "the flag is free once the first ends");
    }

    #[test]
    fn a_turkish_capital_i_does_not_break_the_intent_offsets() {
        match parse("message İrem saying çok güzel") {
            Intent::Message { to, text, .. } => assert_eq!((to.as_str(), text.as_str()), ("İrem", "çok güzel")),
            other => panic!("not a message: {other:?}"),
        }
        match parse("play İstanbul on spotify") {
            Intent::Play { query, service } => assert_eq!((query.as_str(), service.as_str()), ("İstanbul", "spotify")),
            other => panic!("not a play: {other:?}"),
        }
    }

    #[test]
    fn a_stream_that_goes_quiet_is_an_error_and_a_finished_one_is_not() {
        use std::time::Duration;
        let (tx, rx) = mpsc::channel::<Option<Value>>();
        tx.send(Some(json!({ "text": "half" }))).unwrap();
        assert!(collect_events(&rx, Duration::from_millis(20), |_| {}).is_err(), "no end marker before the wait: a timeout, not a short reply");
        let (tx, rx) = mpsc::channel::<Option<Value>>();
        tx.send(Some(json!({ "text": "whole" }))).unwrap();
        tx.send(None).unwrap();
        assert_eq!(collect_events(&rx, Duration::from_millis(20), |_| {}).unwrap().len(), 1, "the end marker ends it normally");
        let (tx, rx) = mpsc::channel::<Option<Value>>();
        drop(tx);
        assert_eq!(collect_events(&rx, Duration::from_millis(20), |_| {}).unwrap().len(), 0, "a dropped sender is a normal end");
    }

    #[test]
    fn repeat_requests_are_recognised_and_return_the_last_answer() {
        assert!(is_replay("Say that again, please"));
        assert!(is_replay("repeat that"));
        assert!(!is_replay("open steam"));
        *LAST.lock().unwrap() = Some("The top story is X.".into());
        assert_eq!(last_answer(), "The top story is X.");
        *LAST.lock().unwrap() = None;
        assert_eq!(last_answer(), "I haven't answered anything yet.");
    }

    fn items(v: &[(&str, &str)]) -> Vec<(String, String)> { v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() }

    #[test]
    fn app_paths_keys_become_names_and_skip_empty_paths() {
        let keys = items(&[("notepad.exe", r"C:\Windows\system32\notepad.exe"), ("Calc.EXE", r#""C:\Windows\system32\calc.exe""#), ("empty.exe", ""), ("blank.exe", "  ")]);
        assert_eq!(app_path_items(&keys), items(&[("notepad", r"C:\Windows\system32\notepad.exe"), ("calc", r"C:\Windows\system32\calc.exe")]));
    }

    #[test]
    fn notepad_matches_an_app_paths_entry() {
        let all = app_path_items(&items(&[("notepad.exe", r"C:\Windows\system32\notepad.exe"), ("wordpad.exe", r"C:\x\wordpad.exe")]));
        assert_eq!(best_match("notepad", &all).map(|(n, t)| (n.as_str(), t.as_str())), Some(("notepad", r"C:\Windows\system32\notepad.exe")));
        assert_eq!(best_match("note", &all).map(|(n, _)| n.as_str()), Some("notepad"), "a prefix still finds it");
    }

    #[test]
    fn parses_what_you_say() {
        assert_eq!(parse("Search for the best ramen near me."), Intent::Search("the best ramen near me".into()));
        assert_eq!(parse("hey, could you search cats?"), Intent::Search("cats".into()));
        assert_eq!(parse("Google rust lifetimes"), Intent::Search("rust lifetimes".into()));
        assert_eq!(parse("Open the game Valorant."), Intent::Open("Valorant".into()));
        assert_eq!(parse("open Spotify"), Intent::Open("Spotify".into()));
        assert_eq!(parse("open my last game"), Intent::Open("last game".into()), "not cut down to \"my last\"");
        assert_eq!(parse("open the Spotify app"), Intent::Open("Spotify".into()), "the web lookup gets the bare name");
        assert_eq!(web_version("Spotify"), Some(("Spotify", "https://open.spotify.com")));
        assert_eq!(web_version(" YouTube Music "), Some(("YouTube Music", "https://music.youtube.com")));
        assert_eq!(web_version("spotify wrapped"), None);
        assert_eq!(web_version("notepad"), None);
        for (k, ..) in WEB_APPS { assert!(web_version(k).is_some(), "{k}"); }
        let play = |q: &str, s: &str| Intent::Play { query: q.into(), service: s.into() };
        assert_eq!(parse("play lofi on spotify"), play("lofi", "spotify"));
        assert_eq!(parse("Play some Arctic Monkeys."), play("Arctic Monkeys", ""));
        assert_eq!(parse("play the weeknd on youtube music"), play("the weeknd", "youtube music"));
        assert_eq!(parse("play a song that is on the radio today"), play("a song that is on the radio today", ""), "a long tail is part of the title");
        assert_eq!(parse("launch the Counter-Strike game"), Intent::Open("Counter-Strike".into()));
        assert_eq!(parse("message Ali saying I'll be late"), Intent::Message { to: "Ali".into(), text: "I'll be late".into(), via: String::new() });
        assert_eq!(parse("Send a message to Sam that dinner is ready."), Intent::Message { to: "Sam".into(), text: "dinner is ready".into(), via: String::new() });
        assert_eq!(parse("text mum are you home"), Intent::Message { to: "mum".into(), text: "are you home".into(), via: String::new() });
        assert_eq!(parse("message Ali"), Intent::Chat("message Ali".into()), "no text: left to the AI");
        assert_eq!(parse("how's the weather?"), Intent::Chat("how's the weather?".into()));
        assert_eq!(parse("search"), Intent::Chat("search".into()));
        assert_eq!(parse("okay okay please open notepad"), Intent::Open("notepad".into()));
        assert_eq!(parse("tell me a joke"), Intent::Chat("tell me a joke".into()));
    }

    #[test]
    fn a_list_of_apps_is_split_into_one_name_each() {
        assert_eq!(split_apps("discord and valorant"), ["discord", "valorant"]);
        assert_eq!(split_apps("the chrome, spotify and steam app"), ["chrome", "spotify", "steam"]);
        assert_eq!(split_apps("notepad"), ["notepad"], "one name stays one name");
    }

    #[test]
    fn a_message_can_name_the_app_the_person_is_on() {
        let m = |to: &str, text: &str, via: &str| Intent::Message { to: to.into(), text: text.into(), via: via.into() };
        assert_eq!(parse("message Sara on WhatsApp saying I'm late"), m("Sara", "I'm late", "WhatsApp"));
        assert_eq!(parse("text Sara Khan on Discord that dinner is ready"), m("Sara Khan", "dinner is ready", "Discord"));
        assert_eq!(parse("message Sara on WhatsApp Web: hi"), m("Sara", "hi", "WhatsApp Web"));
        assert_eq!(parse("message Sara saying meet me on Friday"), m("Sara", "meet me on Friday", ""), "an 'on' inside the message is not an app");
        assert_eq!(parse("message on discord saying hi"), m("on discord", "hi", ""), "no name, no app split");
    }

    #[test]
    fn someone_not_in_contacts_is_found_by_the_take_over_and_sent_only_after_a_yes() {
        let g = find_goal("Sara", "I'm late", "WhatsApp");
        assert!(g.starts_with("Find Sara on WhatsApp and message them."), "{g}");
        assert!(g.contains("check the chat's title is Sara") && g.contains("\"I'm late\"") && g.contains("Send to Sara: I'm late?"), "{g}");
        assert!(g.contains("Do not send it yet") && g.contains("only if the user said yes"), "sending waits for the island's yes");
        assert!(find_goal("Sara", "hi", "").starts_with("Find Sara and message them."), "no app named: the screen shows where");
        // with the take-over switch off the buddy says how to turn it on, and nothing is opened
        let off = cfg(json!({ "buddies": { "contacts": [] } }));
        let said = run(Intent::Message { to: "Sara".into(), text: "hi".into(), via: "WhatsApp".into() }, &off).unwrap_err();
        assert!(said.contains("don't have a contact called Sara") && said.contains("take over"), "{said}");
    }

    #[test]
    fn the_app_you_name_overrides_the_saved_one() {
        let link = |a: &str| Route::Link(a.to_string());
        assert_eq!(route("whatsapp", ""), link("whatsapp"), "no app named: the saved one");
        assert_eq!(route("Discord", " "), link("discord"));
        assert_eq!(route("whatsapp", "WhatsApp"), link("whatsapp"));
        assert_eq!(route("whatsapp", "WhatsApp Web"), link("whatsappweb"), "same phone number, so a link works");
        assert_eq!(route("whatsappweb", "whatsapp"), link("whatsapp"));
        assert_eq!(route("whatsapp", "web whatsapp"), link("whatsappweb"));
        assert_eq!(route("whatsapp", "Discord"), Route::Find, "a phone number is not a Discord id");
        assert_eq!(route("discord", "WhatsApp"), Route::Find, "a Discord id is not a phone number");
        assert_eq!(route("discord", "Telegram"), Route::Find);
    }

    #[test]
    fn picks_the_closest_app() {
        let all = items(&[("Steam", "a"), ("Steam Link", "b"), ("Valorant", "c"), ("Visual Studio Code", "d"), ("Google Chrome", "e")]);
        assert_eq!(best_match("steam", &all).unwrap().1, "a");
        assert_eq!(best_match("VALO", &all).unwrap().1, "c");
        assert_eq!(best_match("studio code", &all).unwrap().1, "d");
        assert_eq!(best_match("chrome google", &all).unwrap().1, "e", "all words, any order");
        assert_eq!(best_match("minecraft", &all), None);
        assert_eq!(best_match("  ", &all), None);
    }

    #[test]
    fn finds_steam_games_and_start_menu_apps() {
        let root = std::env::temp_dir().join(format!("zor-ui-assist-test-{}", std::process::id()));
        let apps = root.join("lib2").join("steamapps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        std::fs::write(apps.join("appmanifest_730.acf"), "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"name\"\t\t\"Counter-Strike 2\"\n}\n").unwrap();
        std::fs::write(root.join("steamapps").join("libraryfolders.vdf"),
            format!("\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n", root.join("lib2").to_string_lossy().replace('\\', "\\\\"))).unwrap();
        let libs = steam_libraries(&root.join("steamapps"));
        assert_eq!(libs.len(), 2);
        let games: Vec<_> = libs.iter().flat_map(|d| steam_games(d)).collect();
        assert_eq!(games, items(&[("Counter-Strike 2", "steam://rungameid/730")]));

        let menu = root.join("Programs").join("Dev");
        std::fs::create_dir_all(&menu).unwrap();
        std::fs::write(menu.join("Notepad++.lnk"), "").unwrap();
        std::fs::write(menu.join("readme.txt"), "").unwrap();
        let mut found = vec![];
        shortcuts(&root.join("Programs"), 3, &mut found);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "Notepad++");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn builds_message_links_without_sending() {
        let (link, said) = message_link("whatsapp", "+92 300 1234567", "Ali", "see you & soon").unwrap();
        assert_eq!(link, "whatsapp://send?phone=923001234567&text=see%20you%20%26%20soon");
        assert!(said.contains("Press Enter"), "{said}");
        assert!(message_link("whatsapp", "", "Ali", "hi").is_err());
        assert!(message_link("discord", "abc", "Sam", "hi").is_err(), "no id");
        assert!(message_link("signal", "1", "Sam", "hi").unwrap_err().contains("signal"));
    }

    #[test]
    fn whatsapp_web_links_open_in_the_browser_with_the_text_ready() {
        let (link, said) = message_link("whatsappweb", "+92 300 1234567", "Ali", "see you & soon").unwrap();
        assert_eq!(link, "https://web.whatsapp.com/send?phone=923001234567&text=see%20you%20%26%20soon");
        assert!(said.contains("WhatsApp Web") && said.contains("Press Enter"), "{said}");
        assert!(message_link("whatsappweb", "", "Ali", "hi").unwrap_err().contains("no phone number"));
        assert!(message_link("whatsappweb", "abc", "Ali", "hi").is_err(), "no digits, no link");
        assert!(!message_link("whatsappweb", "1", "Ali", "hi").unwrap().0.starts_with("whatsapp://"), "never the app link");
    }

    #[test]
    fn plain_whatsapp_falls_back_to_the_web_only_when_the_app_is_missing() {
        assert_eq!(effective_app("whatsapp", false), ("whatsappweb".to_string(), true), "no app: the web, and say so");
        assert_eq!(effective_app("whatsapp", true), ("whatsapp".to_string(), false), "the app is there: use it");
        for spelling in ["whatsappweb", "WhatsAppWeb", "whatsapp-web", " whatsapp web "] {
            assert_eq!(effective_app(spelling, true), ("whatsappweb".to_string(), false), "{spelling}: asked for the web, so not a fallback");
            assert_eq!(effective_app(spelling, false), ("whatsappweb".to_string(), false), "{spelling}");
        }
        assert_eq!(effective_app("discord", false), ("discord".to_string(), false), "other apps are left alone");
        assert_eq!(effective_app("Signal", false), ("signal".to_string(), false));
    }

    fn l(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn adding_a_contact_replaces_the_same_name_and_goes_last() {
        assert_eq!(with_contact(&l(&["Ali=whatsapp:1"]), None, "Sam=discord:2".into()), l(&["Ali=whatsapp:1", "Sam=discord:2"]));
        assert_eq!(with_contact(&l(&["Ali=whatsapp:1", "Sam=discord:2"]), None, "ali=discord:9".into()), l(&["Sam=discord:2", "ali=discord:9"]), "names match without case");
        assert_eq!(with_contact(&[], None, "Ali=whatsapp:1".into()), l(&["Ali=whatsapp:1"]));
    }

    #[test]
    fn editing_a_contact_changes_it_where_it_stands() {
        let all = l(&["A=whatsapp:1", "B=discord:2", "C=whatsapp:3"]);
        assert_eq!(with_contact(&all, Some("B=discord:2"), "B=whatsappweb:2".into()), l(&["A=whatsapp:1", "B=whatsappweb:2", "C=whatsapp:3"]), "a new app keeps the place");
        assert_eq!(with_contact(&all, Some("B=discord:2"), "B=discord:22".into()), l(&["A=whatsapp:1", "B=discord:22", "C=whatsapp:3"]), "a new number keeps the place");
        assert_eq!(with_contact(&all, Some("B=discord:2"), "Bee=discord:2".into()), l(&["A=whatsapp:1", "Bee=discord:2", "C=whatsapp:3"]), "a new name keeps the place, and the old name is gone");
        assert_eq!(with_contact(&all, Some("B=discord:2"), "B=discord:2".into()), all, "saving without a change changes nothing");
        assert_eq!(with_contact(&all, Some("A=whatsapp:1"), "a=whatsapp:1".into()), l(&["a=whatsapp:1", "B=discord:2", "C=whatsapp:3"]), "only the case of the name changed");
    }

    #[test]
    fn renaming_onto_another_contact_replaces_that_one() {
        let all = l(&["A=whatsapp:1", "B=discord:2", "C=whatsapp:3"]);
        // C becomes "a": the old A goes, C's entry stays in C's place
        assert_eq!(with_contact(&all, Some("C=whatsapp:3"), "a=whatsapp:9".into()), l(&["B=discord:2", "a=whatsapp:9"]));
        // A becomes "c": the old C goes, A's entry stays in A's place
        assert_eq!(with_contact(&all, Some("A=whatsapp:1"), "c=whatsapp:1".into()), l(&["c=whatsapp:1", "B=discord:2"]));
    }

    #[test]
    fn editing_something_that_was_deleted_meanwhile_just_adds() {
        let all = l(&["A=whatsapp:1", "B=discord:2"]);
        assert_eq!(with_contact(&all, Some("Z=discord:5"), "Z=discord:6".into()), l(&["A=whatsapp:1", "B=discord:2", "Z=discord:6"]));
        assert_eq!(with_contact(&all, Some("Z=discord:5"), "a=whatsapp:7".into()), l(&["B=discord:2", "a=whatsapp:7"]), "and it still replaces the same name");
    }

    #[test]
    fn entries_that_dont_parse_survive_every_save() {
        let all = l(&["broken", "A=whatsapp:1", "also broken"]);
        assert_eq!(with_contact(&all, None, "B=discord:2".into()), l(&["broken", "A=whatsapp:1", "also broken", "B=discord:2"]));
        assert_eq!(with_contact(&all, Some("A=whatsapp:1"), "A=discord:3".into()), l(&["broken", "A=discord:3", "also broken"]));
    }

    #[test]
    fn the_reply_says_where_the_chat_opened_or_why_it_did_not() {
        use crate::webchat::Opened;
        let plain = "WhatsApp Web is open on Ali. Press Enter to send.".to_string();
        assert_eq!(web_reply("Ali", &Opened::LiveTab, false, plain.clone()), "Opened the chat with Ali in your WhatsApp tab. Press Enter to send.");
        assert_eq!(web_reply("Ali", &Opened::LiveTab, true, plain.clone()), web_reply("Ali", &Opened::LiveTab, false, plain.clone()), "the fallback note doesn't matter once the live tab is used");
        assert_eq!(web_reply("Ali", &Opened::NewTab, false, plain.clone()), plain);
        assert!(web_reply("Ali", &Opened::NewTab, true, plain.clone()).starts_with("WhatsApp isn't installed here"));
        let failed = web_reply("Ali", &Opened::Failed("the address bar didn't take focus".into()), false, plain.clone());
        assert!(failed.contains("couldn't load the chat: the address bar didn't take focus.") && failed.contains("opened nothing else"), "{failed}");
        assert!(!web_reply("Ali", &Opened::Failed("x.".into()), false, plain).contains(".."), "no doubled full stop");
    }

    #[test]
    fn a_scheme_nobody_handles_is_reported_as_unhandled() {
        assert!(!protocol_handled("zorui-no-such-scheme-7"), "Windows has no association for it");
        assert!(!protocol_handled(""), "an empty scheme is never handled");
    }

    fn cfg(j: serde_json::Value) -> Config { Config::parse(&j.to_string()) }

    #[test]
    fn reads_contacts_from_the_settings_list() {
        let c = cfg(json!({ "buddies": { "contacts": ["Ali = whatsapp:+92 300", "sam=Discord:123", "broken", "x=noapp"] } }));
        assert_eq!(contacts(&c), vec![("Ali".to_string(), "whatsapp".to_string(), "+92 300".to_string()), ("sam".into(), "discord".into(), "123".into())]);
        assert!(contacts(&cfg(json!({}))).is_empty());
    }

    #[test]
    fn contact_entries_round_trip_and_reject_bad_input() {
        let e = format_contact(" Ali ", "WhatsApp", " +92 300 ").unwrap();
        assert_eq!(e, "Ali=whatsapp:+92 300");
        assert_eq!(parse_contact(&e), Some(("Ali".into(), "whatsapp".into(), "+92 300".into())));
        assert!(format_contact("", "discord", "1").is_none());
        assert!(format_contact("a", "discord", " ").is_none());
        assert!(format_contact("a=b", "discord", "1").is_none());
        assert!(format_contact("a,b", "discord", "1").is_none());
        assert!(format_contact("a", "discord", "1,2").is_none());
    }

    #[test]
    fn a_failed_fixed_command_falls_back_to_the_ai() {
        let (open, play, search) = (Intent::Open("a timer".into()), Intent::Play { query: "x".into(), service: String::new() }, Intent::Search("x".into()));
        let err = "No app called a timer.";
        for i in [&open, &search] { assert!(falls_back(i, err, true), "{i:?}"); }
        assert!(!falls_back(&play, err, true), "playback must not restart through the AI");
        for i in [&open, &play, &search] { assert!(!falls_back(i, err, false), "{i:?}: the AI is off"); }
        assert!(!falls_back(&open, "Opening apps is turned off in Settings.", true), "a feature turned off stays off");
        assert!(!falls_back(&Intent::Message { to: "a".into(), text: "b".into(), via: String::new() }, err, true));
        assert!(!falls_back(&Intent::Chat("hi".into()), err, true));
        assert!(!falls_back(&Intent::Drive("click save".into()), err, true));
        assert!(!falls_back(&Intent::Screen("what's on my screen".into()), err, true));
        let note = fallback_note(err);
        assert!(note.starts_with("(I first tried") && note.contains(err), "{note}");
        assert_eq!(parse("start a timer"), Intent::Open("a timer".into()), "the misfire the fallback catches");
    }

    #[test]
    fn a_fixed_phrase_whose_switch_is_off_is_refused_before_any_question() {
        let never = &mut |_: &str| -> bool { panic!("a refused phrase asks nothing") };
        let off = |k: &str| cfg(json!({ "buddies": { "allow": { k: false }, "listenAi": false } }));
        assert!(run_with(Intent::Open("x".into()), &off("open"), never).unwrap_err().contains("turned off"));
        assert!(run_with(Intent::Play { query: "x".into(), service: String::new() }, &off("open"), never).unwrap_err().contains("turned off"));
        assert!(run_with(Intent::Message { to: "a".into(), text: "b".into(), via: String::new() }, &off("message"), never).unwrap_err().contains("turned off"));
        assert_eq!(run_with(Intent::Drive("click save".into()), &cfg(json!({})), never), Err(DRIVE_OFF.into()), "take-over is off by default");
    }

    #[test]
    fn a_fixed_phrase_take_over_does_not_start_on_a_no_and_every_acting_phrase_has_a_call() {
        use crate::agent::{policy, DECLINED};
        // Drive is the one fixed phrase that is not Reversible: the call it stands for is Consequential and asks
        let take = crate::agent::Call::TakeOver("click save".into());
        assert_eq!(policy::risk_of(&take), policy::Risk::Consequential);
        let mut asked = 0;
        assert_eq!(policy::approve(&take, &cfg(json!({})), &mut |_| { asked += 1; false }).unwrap_err(), DECLINED);
        assert_eq!(asked, 1);
        // every other acting phrase maps to a call that the gate clears without a question, and Screen and Chat map to none
        for i in [Intent::Search("x".into()), Intent::Open("x".into()), Intent::Play { query: "x".into(), service: String::new() }, Intent::Message { to: "a".into(), text: "b".into(), via: String::new() }] {
            let c = call_of(&i).expect("an acting phrase has a call");
            assert!(policy::gate(&c, &cfg(json!({})), &mut |_| panic!("{c:?} asks nothing")).is_ok(), "{c:?}");
        }
        assert!(call_of(&Intent::Screen("x".into())).is_none() && call_of(&Intent::Chat("x".into())).is_none() && call_of(&Intent::Drive("x".into())).is_none());
    }

    #[test]
    fn every_switch_turns_its_part_off() {
        let off = |k: &str| cfg(json!({ "buddies": { "allow": { k: false }, "listenAi": false } }));
        assert!(run(Intent::Search("x".into()), &off("search")).unwrap_err().contains("turned off"));
        assert!(run(Intent::Open("x".into()), &off("open")).unwrap_err().contains("turned off"));
        assert!(run(Intent::Message { to: "a".into(), text: "b".into(), via: String::new() }, &off("message")).unwrap_err().contains("turned off"));
        assert!(run(Intent::Chat("hi".into()), &off("none")).unwrap_err().contains("AI is off"), "no bridge call when the AI switch is off");
        assert!(!enabled(&cfg(json!({ "buddies": { "listen": false } }))));
        assert!(enabled(&cfg(json!({}))), "on unless turned off");
    }

    #[test]
    fn screen_questions_are_told_apart_from_chat() {
        for s in ["What's on my screen?", "where is the save button", "Where's the volume mixer?", "hey, where do I click to share",
                  "point at the search bar", "show me where settings is", "what is this", "explain this error", "look at my screen",
                  "what am I looking at", "read this page to me", "is anything wrong on screen"] {
            assert_eq!(parse(s), Intent::Screen(s.into()), "{s}");
        }
        for s in ["how's the weather?", "tell me a joke", "how do I reverse a list in python", "where was Einstein born",
                  "what is the capital of France", "what's the time"] {
            assert_eq!(parse(s), Intent::Chat(s.into()), "{s}: a general question is not sent a screenshot");
        }
        assert_eq!(parse("open the screen saver settings"), Intent::Open("screen saver settings".into()), "the other actions come first");
        assert_eq!(parse("search for screen protectors"), Intent::Search("screen protectors".into()));
    }

    #[test]
    fn a_take_over_starts_only_from_its_own_phrases() {
        assert_eq!(parse("take over and rename this file"), Intent::Drive("rename this file".into()));
        assert_eq!(parse("Hey, could you take control and fill in the form."), Intent::Drive("fill in the form".into()));
        assert_eq!(parse("do this for me: close all tabs"), Intent::Drive("close all tabs".into()));
        assert_eq!(parse("click the Save button"), Intent::Drive("click the Save button".into()));
        for s in ["take over the world", "how do I click a link", "what does click mean", "take over", "click", "where is the save button"] {
            assert!(!matches!(parse(s), Intent::Drive(_)), "{s}");
        }
    }

    #[test]
    fn a_take_over_is_off_until_turned_on() {
        let off = cfg(json!({ "buddies": { "listenAi": true } }));
        assert_eq!(run(Intent::Drive("click save".into()), &off), Err(DRIVE_OFF.into()), "nothing is captured or moved while the switch is off");
        let no_ai = cfg(json!({ "buddies": { "drive": true, "listenAi": false } }));
        assert_eq!(run(Intent::Drive("click save".into()), &no_ai), Err(AI_OFF.into()));
    }

    #[test]
    fn screen_help_is_off_until_turned_on() {
        assert!(!screen_on(&cfg(json!({}))));
        assert!(screen_on(&cfg(json!({ "buddies": { "screen": true } }))));
        let off = cfg(json!({ "buddies": { "listenAi": false } }));
        assert_eq!(run(Intent::Screen("what's on my screen".into()), &off), Err(SCREEN_OFF.into()), "asking about the screen says how to turn it on");
        assert_eq!(run(Intent::Screen("where is save".into()), &off), Err(AI_OFF.into()), "a question that only sounds like it falls back to plain chat");
        let on = cfg(json!({ "buddies": { "screen": true, "listenAi": false } }));
        assert_eq!(run(Intent::Screen("what's on my screen".into()), &on), Err(AI_OFF.into()), "no screenshot is taken when the AI is off");
    }

    #[test]
    fn screen_help_uses_the_chosen_model_and_names_no_provider() {
        assert_eq!(screen_model(&cfg(json!({}))), "gemini-2.5-flash");
        assert_eq!(screen_model(&cfg(json!({ "ai": { "model": "m1" } }))), "m1");
        assert_eq!(screen_model(&cfg(json!({ "ai": { "model": "m1" }, "buddies": { "screenModel": "m2" } }))), "m2", "the screen model wins");
        assert_eq!(screen_model(&cfg(json!({ "buddies": { "screenModel": "" }, "ai": { "model": "m1" } }))), "m1", "empty follows the AI tab");
    }

    #[test]
    fn a_reply_keeps_its_text_its_tool_calls_and_its_bad_calls() {
        let events = vec![
            json!({ "delta": "Let me " }),
            json!({ "tool_use": { "id": "toolu_1", "name": "open", "input": { "arg": "Spotify" } } }),
            json!({ "delta": "check." }),
            json!({ "tool_error": { "id": "toolu_2", "name": "play", "message": "tool input is not JSON" } }),
            json!({ "done": true }),
        ];
        let t = fold_turn(&events);
        assert_eq!(t.text, "Let me check.");
        assert_eq!(t.calls, vec![json!({ "id": "toolu_1", "name": "open", "input": { "arg": "Spotify" } })]);
        assert_eq!(t.bad, vec![json!({ "id": "toolu_2", "name": "play", "message": "tool input is not JSON" })]);
        assert_eq!(fold_turn(&[]), Turn::default());
    }

    #[test]
    fn screen_questions_use_a_model_that_can_see_pictures() {
        // a picked model that can't see (Antigravity, a subscription command line) is passed over for one that can
        let cfg = Config::parse(r#"{"buddies":{"screenModel":"antigravity-cli:default"},"ai":{"model":"codex-cli:default"}}"#);
        assert_eq!(screen_model(&cfg), "gemini-2.5-flash");
        // a saved id from before the rename is mapped first, so it is passed over the same way
        let old = Config::parse(r#"{"buddies":{"screenModel":"gemini-cli:default"},"ai":{"model":"claude-haiku-5-5"}}"#);
        assert_eq!(screen_model(&old), "claude-haiku-5-5", "the mapped id can't see, so the next one is used");
        // the picked model is used when it can see
        let picked = Config::parse(r#"{"buddies":{"screenModel":"gpt-4o-mini"},"ai":{"model":"claude-haiku-5-5"}}"#);
        assert_eq!(screen_model(&picked), "gpt-4o-mini");
    }

    #[test]
    fn the_last_token_counts_win_and_a_reply_without_them_has_none() {
        let events = vec![
            json!({ "usage": { "input": 12, "output": null } }),
            json!({ "delta": "hi" }),
            json!({ "usage": { "input": null, "output": 40 } }),
        ];
        let t = fold_turn(&events);
        assert_eq!(t.usage, Some(crate::agent::provider::Usage { input: 12, output: 40 }));
        assert_eq!(fold_turn(&[json!({ "delta": "hi" })]).usage, None, "the provider said nothing");
    }

    #[test]
    fn the_question_goes_out_with_one_labeled_picture_per_monitor() {
        let cap = |x, cursor| screen::Cap { jpeg: vec![0xFF, 0xD8, 0xFF], x, y: 0, w: 1920, h: 1080, sw: 1280, sh: 720, cursor };
        let v: Value = serde_json::from_str(&screen_convo(&cfg(json!({ "ai": { "temperature": 0.8 } })), "m", &[], "where is save?", &[cap(0, true), cap(1920, false)])).unwrap();
        assert_eq!(v["model"], "m");
        assert_eq!(v["system"], screen::SYSTEM);
        assert_eq!(v["temperature"], 0.8, "a screen question uses the Settings temperature");
        let take_over: Value = serde_json::from_str(&convo_with("m", "sys", "click save", &[cap(0, true)])).unwrap();
        assert_eq!(take_over["temperature"], 0.2, "take-over keeps its low temperature");
        let imgs = v["messages"][0]["images"].as_array().unwrap();
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0]["data"], "/9j/", "base64 of the JPEG bytes");
        assert!(imgs[0]["label"].as_str().unwrap().contains("screen 1 of 2") && imgs[1]["label"].as_str().unwrap().contains("secondary"));
        assert!(imgs[1]["label"].as_str().unwrap().contains("right of screen 1"), "the model is told where the second monitor is");
        assert_eq!(v["messages"][0]["content"], "where is save?");
    }

    #[test]
    fn a_follow_up_screen_question_carries_the_last_answers_as_text() {
        let cap = screen::Cap { jpeg: vec![0xFF, 0xD8, 0xFF], x: 0, y: 0, w: 1920, h: 1080, sw: 1280, sh: 720, cursor: true };
        let history = [("where is export?".to_string(), "Under File. (I pointed at: File)".to_string())];
        let v: Value = serde_json::from_str(&screen_convo(&cfg(json!({})), "m", &history, "and then?", &[cap])).unwrap();
        let m = v["messages"].as_array().unwrap();
        assert_eq!((m.len(), &m[0]["role"], &m[1]["role"], &m[2]["role"]), (3, &json!("user"), &json!("assistant"), &json!("user")));
        assert_eq!(m[1]["content"], "Under File. (I pointed at: File)");
        assert!(m[0]["images"].is_null() && m[2]["images"].as_array().is_some_and(|a| a.len() == 1), "only the new question has a picture");
    }

    #[test]
    fn a_walkthrough_counts_its_steps_and_keeps_the_goal() {
        assert_eq!(walk_step(false, " export this as pdf "), 1);
        assert_eq!(walk_step(true, &next_step_question("x")), 2, "the watcher's question is the next step");
        assert_eq!(walk_step(true, "anything"), 3);
        assert_eq!(*GOAL.lock().unwrap(), "export this as pdf", "the goal is the question that started it");
        assert_eq!(next_step_question("export this as pdf"), "I did that. What is the next step to: export this as pdf");
        assert_eq!(walk_step(false, "where is undo?"), 1, "a question of your own starts over");
    }

    #[test]
    fn a_how_to_question_walks_on_without_more_until_the_model_says_done() {
        assert!(keep_walking(true, false, false, 1), "the model said more steps follow");
        assert!(keep_walking(false, false, true, 1), "a how-to question goes on though [MORE] was left out");
        assert!(!keep_walking(false, true, true, 1), "[DONE] ends it");
        assert!(!keep_walking(false, false, false, 1), "a plain question gets one answer");
        assert!(!keep_walking(true, false, true, WALK_MAX), "never past the step cap");
    }

    #[test]
    fn a_step_is_done_by_a_fresh_click_inside_its_box() {
        let area = [100, 100, 200, 140];
        assert!(clicked_in(false, true, (150, 120), area));
        assert!(!clicked_in(true, true, (150, 120), area), "held down since before: not a new click");
        assert!(!clicked_in(false, false, (150, 120), area));
        assert!(!clicked_in(false, true, (250, 120), area), "a click elsewhere");
    }

    #[test]
    fn follow_ups_keep_the_last_few_and_end_after_a_long_gap() {
        let t0 = std::time::Instant::now();
        let at = |s: u64| t0 + std::time::Duration::from_secs(s);
        let mut v: Vec<_> = (0..5).map(|i| (at(i * 10), format!("q{i}"), "a".to_string())).collect();
        keep_recent(&mut v, at(50));
        assert_eq!(v.iter().map(|l| l.1.as_str()).collect::<Vec<_>>(), ["q2", "q3", "q4"]);
        keep_recent(&mut v, at(40 + FOLLOW_S));
        assert_eq!(v.len(), 3, "within the gap of the newest");
        keep_recent(&mut v, at(41 + FOLLOW_S));
        assert!(v.is_empty(), "a long pause starts a new conversation");
    }

    #[test]
    fn screen_help_lists_controls_in_picture_pixels_and_points_at_them_by_number() {
        let cap = |x, cursor| screen::Cap { jpeg: vec![], x, y: 0, w: 1920, h: 1080, sw: 1280, sh: 720, cursor };
        let el = |name: &str, rect| crate::drive::El { name: name.into(), role: "button", rect };
        let els = [el("Save", (90, 90, 210, 150)), el("Lost", (-500, 0, -400, 40)), el("Send", (1920 + 300, 600, 1920 + 420, 660))];
        let one = screen_elements(&[cap(0, true)], &els);
        assert!(one.contains("[1] button \"Save\" at 100,80") && !one.contains("screen"), "{one}");
        assert!(!one.contains("Lost") && !one.contains("Send"), "off every captured screen: left out");
        let two = screen_elements(&[cap(0, true), cap(1920, false)], &els);
        assert!(two.contains("[3] button \"Send\" at 240,420 on screen2"), "numbers stay put when one is left out: {two}");
        assert_eq!(screen_elements(&[cap(0, true)], &[]), "");
        let caps = [cap(0, true)];
        let p = |el| screen::Point { x: 640.0, y: 360.0, label: String::new(), screen: None, el };
        assert_eq!(spot(&caps, &els, &p(Some(1))), Some(((150, 120), Some([90, 90, 210, 150]))));
        assert_eq!(spot(&caps, &els, &p(Some(9))), None, "a number that isn't listed points nowhere");
        assert_eq!(spot(&caps, &els, &p(None)), Some(((960, 540), None)));
    }

    #[test]
    fn a_point_maps_to_its_own_monitor_or_nowhere() {
        let cap = |x, cursor| screen::Cap { jpeg: vec![], x, y: 0, w: 1920, h: 1080, sw: 1280, sh: 720, cursor };
        let caps = [cap(0, true), cap(1920, false)];
        let p = |screen| screen::Point { x: 640.0, y: 360.0, label: String::new(), screen, el: None };
        assert_eq!(point_target(&caps, &p(None)), Some((960, 540)));
        assert_eq!(point_target(&caps, &p(Some(2))), Some((1920 + 960, 540)));
        assert_eq!(point_target(&caps, &p(Some(3))), None, "a screen the model made up points nowhere");
    }

    #[test]
    fn the_buddy_is_sent_to_one_spot_at_a_time() {
        let _ = next_point();
        assert_eq!(next_point(), None);
        point_later(10, 20, "a");
        point_later(30, 40, "b");
        assert_eq!(next_point(), Some((30, 40, "b".into())), "a newer request replaces one he hasn't started");
        assert_eq!(next_point(), None, "and each is taken once");
    }

    #[test]
    fn listen_arguments_skip_empty_settings() {
        assert!(listen_args("", " ", "").is_empty());
        assert!(listen_args("", "", "auto").is_empty());
        assert_eq!(listen_args("D:/m", "MR720", ""), ["--model", "D:/m", "--mic", "MR720"]);
        assert_eq!(listen_args("", "MR720", ""), ["--mic", "MR720"]);
        assert_eq!(listen_args("", "", "hold"), ["--mode", "hold"]);
        assert!(clean_args(&cfg(serde_json::json!({}))).is_empty(), "both on by default");
        assert_eq!(clean_args(&cfg(serde_json::json!({ "buddies": { "echoCancel": false, "denoise": false } }))), ["--no-echo", "--no-denoise"]);
    }

    #[test]
    fn device_and_model_lists_work_on_any_pc() {
        // whatever hardware this is: no panic, no empty names, no duplicates of the same folder
        assert!(mics().iter().all(|m| !m.is_empty()));
        let models = speech_models();
        let mut dirs: Vec<_> = models.iter().map(|m| &m.0).collect();
        dirs.dedup();
        assert_eq!(dirs.len(), models.len());
    }

    #[test]
    fn lines_queue_in_order() {
        while next_line().is_some() {}
        say_later("a", 1.0);
        say_later("b", 2.0);
        assert_eq!(next_line(), Some(("a".into(), 1.0)));
        assert_eq!(next_line(), Some(("b".into(), 2.0)));
        assert_eq!(next_line(), None);
        // a failure stays in the bubble longer than a success (one test: the queue is shared)
        report(Ok("Opening Notepad.".into()));
        report(Err("I can't find Zork on this PC.".into()));
        assert_eq!(next_line(), Some(("Opening Notepad.".into(), 7.0)));
        assert_eq!(next_line(), Some(("I can't find Zork on this PC.".into(), FAIL_SECS)));
        assert!(FAIL_SECS > 7.0);
    }

    #[test]
    fn a_long_answer_stays_longer() {
        assert_eq!(say_secs("Done."), 7.0);
        assert_eq!(say_secs(&"a".repeat(280)), 20.0);
        assert_eq!(say_secs(&"a".repeat(2000)), 30.0);
    }

    #[test]
    fn an_app_that_is_not_installed_is_an_error() {
        let c = cfg(json!({}));
        let r = run(Intent::Open("zzqx no such app 91".into()), &c);
        assert!(r.is_err_and(|e| e.contains("can't find")));
    }

    /// `cargo test real_machine -- --ignored --nocapture`: what "open X" finds on this PC.
    #[test]
    #[ignore]
    fn real_machine() {
        let cfg = Config::parse("{}");
        let all = launchables(&cfg);
        println!("{} launchables, {} steam", all.len(), all.iter().filter(|a| a.1.starts_with("steam:")).count());
        for q in ["steam", "chrome", "discord", "whatsapp", "notepad", "spotify", "valorant", "counter strike"] {
            println!("{q:>16} -> {:?}", best_match(q, &all));
        }
    }
}
