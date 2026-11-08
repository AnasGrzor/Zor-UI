//! Talk to the primary buddy: `zorui.exe --buddy listen` (a hotkey) records your voice, the speech
//! recognizer turns it into text with the Parakeet model, and this module does what you asked:
//! search the web, open an app or Steam game, or start a message. Phrases it doesn't know go to the
//! AI model from the sidebar, which can also pick one of those actions. Nothing is ever sent for you:
//! messages open in the app and wait for you to press Enter.

use crate::{conf::Config, proc, screen, util};
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
    Message { to: String, text: String },
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
        if !to.is_empty() && !text.is_empty() { return Intent::Message { to: to.to_string(), text: text.to_string() }; }
    }
    if let Some(a) = rest(&["open up ", "open ", "launch ", "start ", "run ", "play "]) {
        let a = a.strip_prefix("the ").unwrap_or(&a);
        let a = a.strip_prefix("game ").unwrap_or(a);
        let a = a.strip_suffix(" game").or_else(|| a.strip_suffix(" app")).unwrap_or(a);
        return Intent::Open(a.to_string());
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
    let dir = PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join(r"orca\speech-models");
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
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

/// Voice commands are on (Settings, Buddies, Voice commands).
pub fn enabled(cfg: &Config) -> bool { cfg.get("buddies.listen").as_bool().unwrap_or(true) }

/// One action's switch in Settings; on unless turned off.
fn allowed(cfg: &Config, action: &str) -> bool { cfg.get(&format!("buddies.allow.{action}")).as_bool().unwrap_or(true) }

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
fn protocol_handled(scheme: &str) -> bool {
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

/// Does the action and returns what the buddy says about it.
pub fn run(intent: Intent, cfg: &Config) -> String {
    match intent {
        Intent::Search(_) if !allowed(cfg, "search") => "Web search is turned off in Settings.".into(),
        Intent::Open(_) if !allowed(cfg, "open") => "Opening apps is turned off in Settings.".into(),
        Intent::Message { .. } if !allowed(cfg, "message") => "Messages are turned off in Settings.".into(),
        Intent::Search(q) => {
            crate::bridge::sys::shell_open(&format!("https://www.google.com/search?q={}", util::url_encode(&q)));
            format!("Searching for {}.", short(&q, 40))
        }
        Intent::Open(name) => {
            let items = launchables(cfg);
            match best_match(&name, &items) {
                Some((n, target)) => { crate::bridge::sys::shell_open(target); format!("Opening {}.", short(n, 40)) }
                None => format!("I can't find {} on this PC.", short(&name, 40)),
            }
        }
        Intent::Message { to, text } => {
            let all = contacts(cfg);
            let key = to.to_lowercase();
            let found = all.iter().find(|c| c.0.to_lowercase() == key).or_else(|| all.iter().find(|c| c.0.to_lowercase().starts_with(&key)));
            match found {
                None => format!("I don't have a contact called {to}. Add them under Contacts in Settings."),
                Some((name, app, value)) => {
                    // only ask Windows about the WhatsApp app when this contact would use it
                    let (app, fell_back) = effective_app(app, app != "whatsapp" || protocol_handled("whatsapp"));
                    match message_link(&app, value, name, &text) {
                        Ok((link, said)) if app == "whatsappweb" => {
                            // the live logged-in WhatsApp tab if there is one, else a new tab
                            web_reply(name, &crate::webchat::open_whatsapp_web(&link, crate::bridge::sys::shell_open), fell_back, said)
                        }
                        Ok((link, said)) => { crate::bridge::sys::shell_open(&link); said }
                        Err(e) => e,
                    }
                }
            }
        }
        // a question that only sounds like it is about the screen is a normal chat while screen help is off
        Intent::Screen(t) if !screen_on(cfg) && !mentions_screen(&t.to_lowercase()) => run(Intent::Chat(t), cfg),
        Intent::Screen(_) if !screen_on(cfg) => SCREEN_OFF.into(),
        Intent::Screen(_) if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) => AI_OFF.into(),
        Intent::Screen(t) => screen_help(cfg, &t),
        Intent::Drive(_) if !crate::drive::enabled(cfg) => DRIVE_OFF.into(),
        Intent::Drive(_) if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) => AI_OFF.into(),
        Intent::Drive(g) => crate::drive::start(cfg, &g),
        Intent::Chat(_) if !cfg.get("buddies.listenAi").as_bool().unwrap_or(true) => AI_OFF.into(),
        Intent::Chat(t) => ask_ai(cfg, &t),
    }
}

const AI_OFF: &str = "I only know search, open and message. The AI is off in Settings.";
const DRIVE_OFF: &str = "Take-over is off. Turn on \"Let the buddy take over\" in Settings, Buddies.";
const SCREEN_OFF: &str = "Screen help is off. Turn on \"Let the buddy see my screen\" in Settings, Buddies.";

/// Settings, Buddies, "Let the buddy see my screen". Off unless turned on: the pictures leave this PC.
fn screen_on(cfg: &Config) -> bool { cfg.get("buddies.screen").as_bool().unwrap_or(false) }

/// The model that answers screen questions: the one picked for screen help, else the sidebar's AI model.
pub fn screen_model(cfg: &Config) -> String {
    [cfg.str("buddies.screenModel"), cfg.str("ai.model"), "gemini-2.5-flash"].into_iter().find(|m| !m.is_empty()).unwrap_or_default().to_string()
}

pub fn short(s: &str, n: usize) -> String { match s.char_indices().nth(n) { Some((i, _)) => format!("{}…", &s[..i]), None => s.to_string() } }

/// An AI reply that is a JSON action ({"do":"search","arg":"..."}), maybe inside a code fence.
pub fn ai_action(reply: &str) -> Option<Intent> {
    let (a, b) = (reply.find('{')?, reply.rfind('}')?);
    let v: Value = serde_json::from_str(reply.get(a..=b)?).ok()?;
    let s = |k: &str| v[k].as_str().map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    match v["do"].as_str()? {
        "search" => Some(Intent::Search(s("arg")?)),
        "open" => Some(Intent::Open(s("arg")?)),
        "message" => Some(Intent::Message { to: s("to")?, text: s("text")? }),
        _ => None,
    }
}

const SYSTEM: &str = "You are a small desktop buddy living on the user's taskbar; the user talks to you by voice. \
If they ask you to search the web, open an app or game, or message someone, answer with ONLY one JSON object: \
{\"do\":\"search\",\"arg\":\"query\"} or {\"do\":\"open\",\"arg\":\"app or game name\"} or {\"do\":\"message\",\"to\":\"contact name\",\"text\":\"message\"}. \
Otherwise answer in one or two short, friendly sentences, plain text.";

/// Runs one `ai-chat` conversation (the JSON the bridge command reads) and returns the whole reply. Gives up when
/// nothing arrives for `wait_secs`.
pub fn chat(cfg: &Config, convo: &str, wait_secs: u64) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let (tx2, tx3) = (tx.clone(), tx);
    let stream = proc::stream_stdin(cfg, "ai-chat", &[], convo, move |line| {
        let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if let Some(d) = v["delta"].as_str() { let _ = tx2.send(Ok(d.to_string())); }
        if let Some(e) = v["error"].as_str() { let _ = tx2.send(Err(e.to_string())); }
    }, move || { let _ = tx3.send(Ok(String::new())); drop(tx3); });
    if stream.is_none() { return Err("I can't reach my brain.".into()); }
    let mut reply = String::new();
    // the exit marker is an empty Ok; stop on it, on an error, or when nothing comes for `wait_secs`
    while let Ok(m) = rx.recv_timeout(std::time::Duration::from_secs(wait_secs)) {
        match m {
            Ok(d) if d.is_empty() => break,
            Ok(d) => reply.push_str(&d),
            Err(e) => return Err(e),
        }
    }
    Ok(reply)
}

/// The conversation for a screen question: the question with one picture per monitor, each introduced by its label.
fn screen_convo(model: &str, said: &str, caps: &[screen::Cap]) -> String { convo_with(model, screen::SYSTEM, said, caps) }

/// One question with a labeled picture per monitor, under the given system prompt.
pub fn convo_with(model: &str, system: &str, said: &str, caps: &[screen::Cap]) -> String {
    let images: Vec<Value> = caps.iter().enumerate()
        .map(|(i, c)| json!({ "mime": "image/jpeg", "data": crate::bridge::base64(&c.jpeg), "label": c.label(i, caps.len(), caps.first()) })).collect();
    json!({ "model": model, "messages": [{ "role": "user", "content": said, "images": images }], "system": system, "temperature": 0.2 }).to_string()
}

/// Where the buddy should fly for a point the model gave: screen pixels, or None when the model named a screen that isn't there.
fn point_target(caps: &[screen::Cap], p: &screen::Point) -> Option<(i32, i32)> {
    screen::target(caps, p.screen).map(|c| screen::to_screen(c, p.x, p.y))
}

/// Answers a question about the screen: looks at every monitor, asks the model, and sends the buddy to the spot it named.
fn screen_help(cfg: &Config, said: &str) -> String {
    let model = screen_model(cfg);
    // checked first, so nothing is captured or sent for a model that can't use it
    if !crate::bridge::model_can_see(&model) { return "This model can't see images. Pick another one under Settings, Buddies.".into(); }
    let caps = match screen::capture_all() { Ok(c) => c, Err(e) => return short(&e, 110) };
    let reply = match chat(cfg, &screen_convo(&model, said, &caps), 60) { Ok(r) => r, Err(e) => return short(&e, 110) };
    if reply.trim().is_empty() { return "I got no answer from the AI.".into(); }
    let (spoken, point) = screen::parse_point(&reply);
    let mut said = if spoken.is_empty() { "Here.".to_string() } else { short(&spoken, 160) }; // the bubble is small
    if let Some(p) = point {
        match point_target(&caps, &p) {
            Some(_) if !crate::buddy::on() => said.push_str(" (Turn on a buddy to see where.)"),
            Some((x, y)) => point_later(x, y, &p.label),
            None => {}
        }
    }
    said
}

/// Sends `said` to the sidebar's AI model and acts on or returns its answer.
fn ask_ai(cfg: &Config, said: &str) -> String {
    let model = match cfg.str("ai.model") { "" => "gemini-2.5-flash", m => m };
    let convo = json!({ "model": model, "messages": [{ "role": "user", "content": said }], "system": SYSTEM, "temperature": 0.4 }).to_string();
    let reply = match chat(cfg, &convo, 30) { Ok(r) => r, Err(e) => return short(&e, 110) };
    match ai_action(&reply) {
        Some(i) => run(i, cfg),
        None if reply.trim().is_empty() => "I got no answer from the AI.".into(),
        None => short(reply.trim(), 160),
    }
}

/// Runs `said` on a worker thread and shows the result in the primary buddy's bubble.
pub fn ask(cfg: &Config, said: &str) {
    let (cfg, said) = (Config(cfg.0.clone()), said.to_string());
    std::thread::spawn(move || { say_later("On it…", 3.0); let r = run(parse(&said), &cfg); say_later(&r, 7.0); });
}

/// Bridge arguments for the speech model folder and microphone name; empty ones are left out.
fn listen_args(model: &str, mic: &str) -> Vec<String> {
    [("--model", model), ("--mic", mic)].iter().filter(|(_, v)| !v.trim().is_empty()).flat_map(|(k, v)| [k.to_string(), v.trim().to_string()]).collect()
}

/// Starts recording: the bridge prints states and then the text, which goes to `ask`.
pub fn listen(cfg: &Config) {
    crate::drive::request_stop(); // speaking to the buddy hands a take-over's mouse back
    let cfg2 = Config(cfg.0.clone());
    let model = cfg.str("buddies.model").to_string();
    let mic = cfg.str("buddies.mic").to_string();
    let args = listen_args(&model, &mic);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let s = proc::stream(cfg, "buddy-listen", &args, move |line| {
        let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if let Some(l) = v["level"].as_f64() { crate::pill::level(l as f32); }
        match v["state"].as_str() {
            Some("listening") => crate::pill::set(crate::pill::LISTENING),
            Some("thinking") => crate::pill::set(crate::pill::THINKING),
            _ => {}
        }
        if let Some(e) = v["error"].as_str() { crate::pill::set(0); say_later(&short(e, 110), 8.0); }
        if let Some(t) = v["text"].as_str() {
            crate::pill::set(0);
            if t.trim().is_empty() { say_later("I didn't catch that.", 3.0); } else { ask(&cfg2, t); }
        }
    });
    if s.is_none() { crate::pill::set(0); say_later("I can't start listening.", 6.0); }
    if let Ok(mut l) = LISTENER.lock() { *l = s; } // replacing it stops a recording already running
}

/// Stops a recording in progress and hides the pill (the hotkey pressed a second time).
pub fn cancel() {
    crate::pill::set(0);
    if let Ok(mut l) = LISTENER.lock() { *l = None; } // dropping the stream kills the recorder
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(v: &[(&str, &str)]) -> Vec<(String, String)> { v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() }

    #[test]
    fn parses_what_you_say() {
        assert_eq!(parse("Search for the best ramen near me."), Intent::Search("the best ramen near me".into()));
        assert_eq!(parse("hey, could you search cats?"), Intent::Search("cats".into()));
        assert_eq!(parse("Google rust lifetimes"), Intent::Search("rust lifetimes".into()));
        assert_eq!(parse("Open the game Valorant."), Intent::Open("Valorant".into()));
        assert_eq!(parse("open Spotify"), Intent::Open("Spotify".into()));
        assert_eq!(parse("launch the Counter-Strike game"), Intent::Open("Counter-Strike".into()));
        assert_eq!(parse("message Ali saying I'll be late"), Intent::Message { to: "Ali".into(), text: "I'll be late".into() });
        assert_eq!(parse("Send a message to Sam that dinner is ready."), Intent::Message { to: "Sam".into(), text: "dinner is ready".into() });
        assert_eq!(parse("text mum are you home"), Intent::Message { to: "mum".into(), text: "are you home".into() });
        assert_eq!(parse("message Ali"), Intent::Chat("message Ali".into()), "no text: left to the AI");
        assert_eq!(parse("how's the weather?"), Intent::Chat("how's the weather?".into()));
        assert_eq!(parse("search"), Intent::Chat("search".into()));
        assert_eq!(parse("okay okay please open notepad"), Intent::Open("notepad".into()));
        assert_eq!(parse("tell me a joke"), Intent::Chat("tell me a joke".into()));
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
    fn every_switch_turns_its_part_off() {
        let off = |k: &str| cfg(json!({ "buddies": { "allow": { k: false }, "listenAi": false } }));
        assert!(run(Intent::Search("x".into()), &off("search")).contains("turned off"));
        assert!(run(Intent::Open("x".into()), &off("open")).contains("turned off"));
        assert!(run(Intent::Message { to: "a".into(), text: "b".into() }, &off("message")).contains("turned off"));
        assert!(run(Intent::Chat("hi".into()), &off("none")).contains("AI is off"), "no bridge call when the AI switch is off");
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
        assert_eq!(run(Intent::Drive("click save".into()), &off), DRIVE_OFF, "nothing is captured or moved while the switch is off");
        let no_ai = cfg(json!({ "buddies": { "drive": true, "listenAi": false } }));
        assert_eq!(run(Intent::Drive("click save".into()), &no_ai), AI_OFF);
    }

    #[test]
    fn screen_help_is_off_until_turned_on() {
        assert!(!screen_on(&cfg(json!({}))));
        assert!(screen_on(&cfg(json!({ "buddies": { "screen": true } }))));
        let off = cfg(json!({ "buddies": { "listenAi": false } }));
        assert_eq!(run(Intent::Screen("what's on my screen".into()), &off), SCREEN_OFF, "asking about the screen says how to turn it on");
        assert_eq!(run(Intent::Screen("where is save".into()), &off), AI_OFF, "a question that only sounds like it falls back to plain chat");
        let on = cfg(json!({ "buddies": { "screen": true, "listenAi": false } }));
        assert_eq!(run(Intent::Screen("what's on my screen".into()), &on), AI_OFF, "no screenshot is taken when the AI is off");
    }

    #[test]
    fn screen_help_uses_the_chosen_model_and_names_no_provider() {
        assert_eq!(screen_model(&cfg(json!({}))), "gemini-2.5-flash");
        assert_eq!(screen_model(&cfg(json!({ "ai": { "model": "m1" } }))), "m1");
        assert_eq!(screen_model(&cfg(json!({ "ai": { "model": "m1" }, "buddies": { "screenModel": "m2" } }))), "m2", "the screen model wins");
        assert_eq!(screen_model(&cfg(json!({ "buddies": { "screenModel": "" }, "ai": { "model": "m1" } }))), "m1", "empty follows the AI tab");
    }

    #[test]
    fn the_question_goes_out_with_one_labeled_picture_per_monitor() {
        let cap = |x, cursor| screen::Cap { jpeg: vec![0xFF, 0xD8, 0xFF], x, y: 0, w: 1920, h: 1080, sw: 1280, sh: 720, cursor };
        let v: Value = serde_json::from_str(&screen_convo("m", "where is save?", &[cap(0, true), cap(1920, false)])).unwrap();
        assert_eq!(v["model"], "m");
        assert_eq!(v["system"], screen::SYSTEM);
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
    fn reads_ai_actions() {
        assert_eq!(ai_action("{\"do\":\"search\",\"arg\":\"cats\"}"), Some(Intent::Search("cats".into())));
        assert_eq!(ai_action("```json\n{\"do\":\"open\",\"arg\":\"Steam\"}\n```"), Some(Intent::Open("Steam".into())));
        assert_eq!(ai_action("{\"do\":\"message\",\"to\":\"Ali\",\"text\":\"hi\"}"), Some(Intent::Message { to: "Ali".into(), text: "hi".into() }));
        assert_eq!(ai_action("{\"do\":\"format c:\",\"arg\":\"x\"}"), None, "only the three known actions");
        assert_eq!(ai_action("{\"do\":\"search\",\"arg\":\"\"}"), None);
        assert_eq!(ai_action("It's sunny, enjoy it!"), None);
    }

    #[test]
    fn listen_arguments_skip_empty_settings() {
        assert!(listen_args("", " ").is_empty());
        assert_eq!(listen_args("D:/m", "MR720"), ["--model", "D:/m", "--mic", "MR720"]);
        assert_eq!(listen_args("", "MR720"), ["--mic", "MR720"]);
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
