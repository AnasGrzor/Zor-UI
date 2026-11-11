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
    sync::{mpsc, Mutex},
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
/// How long an answer stays in the island banner (ms), so one you looked away from can still be read.
const ANSWER_BANNER_MS: f64 = 60_000.0;

/// Says what a task returned: a success is a short bubble and an island banner, a failure stays (see `fail_later`).
fn report(r: Result<String, String>) {
    match r {
        Ok(s) => {
            if let Ok(mut l) = LAST.lock() { *l = Some(s.clone()); }
            say_later(&s, say_secs(&s));
            let text = s.clone();
            crate::app::post(Box::new(move |cx| cx.notif = Some(crate::app::Notif { app: "Zor UI".into(), text, until: crate::hub::now_ms() + ANSWER_BANNER_MS, open: String::new() })));
        }
        Err(e) => fail_later(&e),
    }
}

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
        let l = t.to_lowercase();
        let Some(f) = ["hey", "ok", "okay", "please", "can you", "could you", "would you", "will you"].iter().find(|f| l.strip_prefix(**f).is_some_and(|r| r.is_empty() || r.starts_with([' ', ',']))) else { break };
        t = t[f.len()..].trim_start_matches([' ', ',']);
    }
    let l = t.to_lowercase();
    let rest = |prefixes: &[&str]| prefixes.iter().find(|p| l.starts_with(**p)).map(|p| clean(&t[p.len()..]).to_string()).filter(|r| !r.is_empty());
    if let Some(q) = rest(&["search for ", "search up ", "search ", "google ", "look up ", "look for "]) { return Intent::Search(q); }
    if let Some(m) = rest(&["send a message to ", "send message to ", "message ", "text ", "whatsapp ", "dm "]) {
        let ml = m.to_lowercase();
        let mark = [" saying ", " that says ", " that ", " says ", ": "].iter().filter_map(|k| ml.find(k).map(|i| (i, k.len()))).min();
        let (to, text) = match mark {
            Some((i, n)) => (m[..i].trim(), m[i + n..].trim()),
            None => m.split_once(' ').map_or((m.as_str(), ""), |(a, b)| (a, b)),
        };
        // "Sara on WhatsApp": the app you said they are on
        let (to, via) = match to.to_lowercase().rfind(" on ") {
            Some(i) if !to[..i].trim().is_empty() && !to[i + 4..].trim().is_empty() && to[i + 4..].split_whitespace().count() <= 3 => (to[..i].trim(), to[i + 4..].trim()),
            _ => (to, ""),
        };
        if !to.is_empty() && !text.is_empty() { return Intent::Message { to: to.to_string(), text: text.to_string(), via: via.to_string() }; }
    }
    if let Some(a) = rest(&["open up ", "open ", "launch ", "start ", "run "]) { return Intent::Open(app_name_of(&a).to_string()); }
    if let Some(p) = rest(&["play "]) {
        // "lofi on spotify": the last " on " names the service, unless what follows is too long to be one
        let (query, service) = match p.to_lowercase().rfind(" on ") {
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
    Some(rest.trim().strip_prefix('"')?.strip_suffix('"')?)
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

/// Everything "open X" can start: Steam games, then Start Menu apps.
fn launchables(_cfg: &Config) -> Vec<(String, String)> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let steam = steam_dir().unwrap_or_else(|| PathBuf::from(env("ProgramFiles(x86)")).join("Steam")).join("steamapps");
    let mut all: Vec<(String, String)> = steam_libraries(&steam).iter().flat_map(|d| steam_games(d)).collect();
    for base in [PathBuf::from(env("ProgramData")), PathBuf::from(env("APPDATA"))] {
        shortcuts(&base.join(r"Microsoft\Windows\Start Menu\Programs"), 3, &mut all);
    }
    all
}

/// Voice commands are on (Settings, Voice).
pub fn enabled(cfg: &Config) -> bool { cfg.get("buddies.listen").as_bool().unwrap_or(true) }

/// One action's switch in Settings; on unless turned off.
/// Starts the installed app or game that best matches `name` (Start Menu shortcuts, Steam games).
pub fn launch_app(cfg: &Config, name: &str) -> Result<String, String> {
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
    let r = crate::drive::start_with(cfg, &find_goal(to, text, via), Some(shown), Some(Box::new(move || open_for_find(&cfg2, &via2))), false, Some(done));
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

/// Does the action and returns what the buddy says about it: `Ok` when it worked, `Err` when it didn't.
pub fn run(intent: Intent, cfg: &Config) -> Result<String, String> {
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
        // a question that only sounds like it is about the screen is a normal chat while screen help is off
        Intent::Screen(t) if !screen_on(cfg) && !mentions_screen(&t.to_lowercase()) => run(Intent::Chat(t), cfg),
        Intent::Screen(_) if !screen_on(cfg) => Err(SCREEN_OFF.into()),
        Intent::Screen(_) if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) => Err(AI_OFF.into()),
        Intent::Screen(t) => screen_help(cfg, &t),
        Intent::Drive(g) => crate::agent::take_over(cfg, &g),
        Intent::Chat(_) if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) => Err(AI_OFF.into()),
        Intent::Chat(t) => crate::agent::ask(cfg, &t),
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
        .filter(|m| !m.is_empty()).map(|m| crate::bridge::current_model_id(m)).collect();
    candidates.iter().find(|m| crate::bridge::model_can_see(m)).or(candidates.first()).cloned().unwrap_or_default()
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
pub fn stream_turn(cfg: &Config, convo: &str, wait_secs: u64) -> Result<Turn, String> {
    let (tx, rx) = mpsc::channel::<Option<Value>>();
    let (tx2, tx3) = (tx.clone(), tx);
    let stream = proc::stream_stdin(cfg, "ai-chat", &[], convo, move |line| {
        let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if v.is_object() { let _ = tx2.send(Some(v)); }
    }, move || { let _ = tx3.send(None); });
    if stream.is_none() { return Err("I can't reach my brain.".into()); }
    let mut events = vec![];
    // None is the end of the stream; stop on it, on an error, or when nothing comes for `wait_secs`
    while let Ok(Some(v)) = rx.recv_timeout(std::time::Duration::from_secs(wait_secs)) {
        if let Some(e) = v["error"].as_str() { return Err(e.to_string()); }
        events.push(v);
    }
    let turn = fold_turn(&events);
    if let Some(u) = turn.usage {
        crate::log::line(&format!("buddy usage: {} in, {} out", u.input, u.output));
        provider::add(u);
    }
    Ok(turn)
}

/// The conversation for a screen question: the question with one picture per monitor, each introduced by its label.
fn screen_convo(cfg: &Config, model: &str, said: &str, caps: &[screen::Cap]) -> String { convo_at(model, screen::SYSTEM, said, caps, crate::agent::temperature(cfg)) }

/// One question with a labeled picture per monitor, under the given system prompt. Take-over uses this at 0.2, so its clicks stay precise.
pub fn convo_with(model: &str, system: &str, said: &str, caps: &[screen::Cap]) -> String { convo_at(model, system, said, caps, 0.2) }

fn convo_at(model: &str, system: &str, said: &str, caps: &[screen::Cap], temp: f64) -> String {
    let images: Vec<Value> = caps.iter().enumerate()
        .map(|(i, c)| json!({ "mime": "image/jpeg", "data": crate::bridge::base64(&c.jpeg), "label": c.label(i, caps.len(), caps.first()) })).collect();
    json!({ "model": model, "messages": [{ "role": "user", "content": said, "images": images }], "system": system, "temperature": temp }).to_string()
}

/// Where the buddy should fly for a point the model gave: screen pixels, or None when the model named a screen that isn't there.
fn point_target(caps: &[screen::Cap], p: &screen::Point) -> Option<(i32, i32)> {
    screen::target(caps, p.screen).map(|c| screen::to_screen(c, p.x, p.y))
}

/// Answers a question about the screen: looks at every monitor, asks the model, and sends the buddy to the spot it named.
fn screen_help(cfg: &Config, said: &str) -> Result<String, String> {
    let model = screen_model(cfg);
    // checked first, so nothing is captured or sent for a model that can't use it
    if !crate::bridge::model_can_see(&model) { return Err("This model can't see images. Pick another one under Settings, Voice.".into()); }
    let caps = screen::capture_all().map_err(|e| short(&e, 110))?;
    let reply = chat(cfg, &screen_convo(cfg, &model, said, &caps), 60).map_err(|e| short(&e, 110))?;
    if reply.trim().is_empty() { return Err("I got no answer from the AI.".into()); }
    let (spoken, point) = screen::parse_point(&reply);
    let mut said = if spoken.is_empty() { "Here.".to_string() } else { short(&spoken, 160) }; // the bubble is small
    if let Some(p) = point {
        match point_target(&caps, &p) {
            Some(_) if !crate::buddy::on() => said.push_str(" (Turn on a buddy to see where.)"),
            Some((x, y)) => point_later(x, y, &p.label),
            None => {}
        }
    }
    Ok(said)
}

/// Runs `said` on a worker thread and shows the result in the primary buddy's bubble.
pub fn ask(cfg: &Config, said: &str) {
    let (cfg, said) = (Config(cfg.0.clone()), said.to_string());
    std::thread::spawn(move || {
        if is_replay(&said) { let a = last_answer(); say_later(&a, say_secs(&a)); return; }
        say_later("On it…", 3.0);
        // Intent is not Clone and run consumes it, so the (pure, cheap) parse runs twice
        let ai_on = cfg.get("buddies.listenAi").as_bool().unwrap_or(true);
        let r = match run(parse(&said), &cfg) {
            Err(e) if falls_back(&parse(&said), &e, ai_on) => crate::agent::ask_with_note(&cfg, said.trim(), &fallback_note(&e)),
            r => r,
        };
        report(r);
    });
}

/// True when a fixed command failed and the AI should get the sentence instead. Not when the AI is off, not for a
/// feature the user turned off in Settings, and not for Message, Screen, Drive or Chat (those asked or went to the AI already).
pub fn falls_back(intent: &Intent, err: &str, ai_on: bool) -> bool {
    ai_on && matches!(intent, Intent::Open(_) | Intent::Play { .. } | Intent::Search(_)) && !err.contains("turned off")
}

/// The note the AI gets with the request after a fixed command failed: why the fixed command could not do it. It goes to the AI
/// only; the request itself is saved as the user said it.
pub fn fallback_note(err: &str) -> String {
    format!("(I first tried it as a fixed command and it failed: {err} Find another way.)")
}

/// Bridge arguments for the speech model folder and microphone name; empty ones are left out.
fn listen_args(model: &str, mic: &str, mode: &str) -> Vec<String> {
    let mode = if mode == "auto" { "" } else { mode }; // Auto is what the recorder does without the flag
    [("--model", model), ("--mic", mic), ("--mode", mode)].iter().filter(|(_, v)| !v.trim().is_empty()).flat_map(|(k, v)| [k.to_string(), v.trim().to_string()]).collect()
}

/// Starts recording: the bridge prints states and then the text, which goes to `ask`.
pub fn listen(cfg: &Config) {
    // a question from the take-over is waiting: your words answer it. Otherwise speaking to the buddy hands a take-over's mouse back.
    let answering = crate::drive::waiting();
    if !answering { crate::drive::request_stop(); crate::browse::request_stop(); }
    let cfg2 = Config(cfg.0.clone());
    let model = cfg.str("buddies.model").to_string();
    let mic = cfg.str("buddies.mic").to_string();
    crate::pill::set_mode(cfg.str("buddies.listenMode"));
    let args = listen_args(&model, &mic, cfg.str("buddies.listenMode"));
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
        if let Some(e) = v["error"].as_str() { crate::pill::set(0); fail_later(&short(e, 110)); }
        if let Some(t) = v["text"].as_str() {
            crate::pill::set(0);
            if t.trim().is_empty() { say_later("I didn't catch that.", 3.0); }
            else if answering && crate::drive::reply_spoken(t) {} // the take-over heard you
            else { ask(&cfg2, t); }
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

/// Stops a recording in progress and hides the pill (the hotkey pressed a second time).
pub fn cancel() {
    crate::pill::set(0);
    if let Ok(mut l) = LISTENER.lock() { *l = None; } // dropping the stream kills the recorder
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parses_what_you_say() {
        assert_eq!(parse("Search for the best ramen near me."), Intent::Search("the best ramen near me".into()));
        assert_eq!(parse("hey, could you search cats?"), Intent::Search("cats".into()));
        assert_eq!(parse("Google rust lifetimes"), Intent::Search("rust lifetimes".into()));
        assert_eq!(parse("Open the game Valorant."), Intent::Open("Valorant".into()));
        assert_eq!(parse("open Spotify"), Intent::Open("Spotify".into()));
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
        for i in [&open, &play, &search] { assert!(falls_back(i, err, true), "{i:?}"); }
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
        let v: Value = serde_json::from_str(&screen_convo(&cfg(json!({ "ai": { "temperature": 0.8 } })), "m", "where is save?", &[cap(0, true), cap(1920, false)])).unwrap();
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
    fn a_point_maps_to_its_own_monitor_or_nowhere() {
        let cap = |x, cursor| screen::Cap { jpeg: vec![], x, y: 0, w: 1920, h: 1080, sw: 1280, sh: 720, cursor };
        let caps = [cap(0, true), cap(1920, false)];
        let p = |screen| screen::Point { x: 640.0, y: 360.0, label: String::new(), screen };
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
