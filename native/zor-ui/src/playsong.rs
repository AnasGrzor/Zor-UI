//! Plays a song for real, in whatever is live, and checks that it started. Routes in order, each only when it can apply:
//! the Spotify app, a Spotify Web tab that is already open (your login is in it), a YouTube tab that is already open, and
//! last a new YouTube tab. A route counts only when a media session starts playing something new; otherwise the next one
//! is tried, and if all fail the answer says what was tried. Windows are found and pressed through UI Automation, and keys go
//! to whatever is in front, so every step checks its result first (see `webchat`).

use crate::{assist, media::{self, Media}, util, webchat};
use windows::{
    core::{Interface, VARIANT},
    Win32::{
        Foundation::{CloseHandle, BOOL, HWND, LPARAM},
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
            Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::{
            Accessibility::*,
            WindowsAndMessaging::{EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible},
        },
    },
};

/// One way to start the song.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Route { SpotifyApp, SpotifyTab, YoutubeTab, YoutubeNew }

/// The routes to try, in order. `spotify_app`: installed or running. Asking for YouTube (or YouTube Music) skips Spotify;
/// naming Spotify, or no service, tries it first.
pub fn routes(service: &str, spotify_app: bool, youtube_tab: bool) -> Vec<Route> {
    let s = service.to_lowercase();
    let spotify = s.trim().is_empty() || s.contains("spotify");
    let mut v = vec![];
    if spotify && spotify_app { v.push(Route::SpotifyApp); }
    // always tried for Spotify: an open Spotify tab is used (your login is in it), otherwise a new one opens
    if spotify { v.push(Route::SpotifyTab); }
    if youtube_tab { v.push(Route::YoutubeTab); }
    v.push(Route::YoutubeNew);
    v
}

/// Runs `run` on each route until one succeeds: (the route, what it said, why the routes before it failed). Err lists what each said.
pub fn first_that_plays(routes: &[Route], mut run: impl FnMut(Route) -> Result<String, String>) -> Result<(Route, String, Vec<String>), String> {
    let mut why = vec![];
    for &r in routes {
        match run(r) { Ok(m) => return Ok((r, m, why)), Err(e) => why.push(format!("{r:?}: {e}")) }
    }
    Err(format!("I couldn't start it. {}", why.join("; ")))
}

/// The first search result's video id on a YouTube results page. Results are the `"videoRenderer":{"videoId":"` entries; a bare
/// `"videoId"` can come from a suggestion or an ad above them. None for a page without results (a consent wall, a changed layout).
pub fn youtube_id(html: &str) -> Option<String> {
    const MARK: &str = "\"videoRenderer\":{\"videoId\":\"";
    let at = html.find(MARK)? + MARK.len();
    let id = html.get(at..at + 11)?;
    id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-').then(|| id.to_string())
}

/// Is this a Spotify Web tab? Its title names Spotify while idle ("Spotify - Web Player", "Search - Spotify") and reads
/// "Song • Artist" while playing, so a tab whose title holds a playing browser session's title counts too.
pub fn is_spotify_tab(title: &str, browser_session_titles: &[String]) -> bool {
    let t = title.to_lowercase();
    t.contains("spotify") || (title.contains(" • ") && browser_session_titles.iter().any(|s| !s.is_empty() && title.contains(s.as_str())))
}

/// The Spotify tab to use (the selected one if several), or None.
pub fn pick_spotify_tab(tabs: &[(String, bool)], session_titles: &[String]) -> Option<usize> {
    let hits: Vec<usize> = (0..tabs.len()).filter(|&i| is_spotify_tab(&tabs[i].0, session_titles)).collect();
    hits.iter().copied().find(|&i| tabs[i].1).or_else(|| hits.first().copied())
}

/// A YouTube tab: the title ends "- YouTube", before any browser suffix such as " - Audio playing".
pub fn is_youtube_tab(title: &str) -> bool { title.contains(" - YouTube") || title.trim() == "YouTube" }

pub fn pick_youtube_tab(tabs: &[(String, bool)]) -> Option<usize> {
    let hits: Vec<usize> = (0..tabs.len()).filter(|&i| is_youtube_tab(&tabs[i].0)).collect();
    hits.iter().copied().find(|&i| tabs[i].1).or_else(|| hits.first().copied())
}

/// A button name that plays a result: "Play Interstellar by Hans Zimmer" on a row, or a plain "Play" on the top result's card. The
/// player bar has a plain "Play" too; `pick_play` tells it apart by where it is. "Play on a device" and Connect entries are not it.
pub fn is_result_play(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    (n == "play" || n.starts_with("play ")) && !n.starts_with("play on ") && !n.contains("connect")
}

/// The button to press out of (top, left, name) rows: the top-most, then left-most result button above `bar_top` (the player bar
/// along the bottom of the window is skipped). None without one.
pub fn pick_play(buttons: &[(i32, i32, String)], bar_top: i32) -> Option<usize> {
    (0..buttons.len()).filter(|&i| is_result_play(&buttons[i].2) && buttons[i].0 < bar_top).min_by_key(|&i| (buttons[i].0, buttons[i].1))
}

/// A session now playing something it wasn't playing before the attempt: a new session, a new song, or one that just resumed.
pub fn started(before: &[Media], after: &[Media]) -> Option<Media> {
    after.iter().find(|m| m.playing && !before.iter().any(|b| b.playing && b.id == m.id && b.title == m.title && b.artist == m.artist)).cloned()
}

fn now_playing(m: &Media) -> String { if m.artist.is_empty() { m.title.clone() } else { format!("{} by {}", m.title, m.artist) } }

/// Waits up to `secs` for a session that started playing (see `started`).
fn verify(before: &[Media], secs: u64) -> Option<Media> {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if let Some(m) = started(before, &media::sessions()) { return Some(m); }
        if std::time::Instant::now() >= end { return None; }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

fn uia() -> Option<IUIAutomation> {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() }
}

/// The result "Play ..." buttons in `window`, as (top, left, name, element).
fn play_buttons(uia: &IUIAutomation, window: HWND) -> Vec<(i32, i32, String, IUIAutomationElement)> {
    let mut out = vec![];
    let _ = (|| -> windows::core::Result<()> {
        unsafe {
            let root = uia.ElementFromHandle(window)?;
            let cond = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ButtonControlTypeId.0))?;
            let all = root.FindAll(TreeScope_Descendants, &cond)?;
            for i in 0..all.Length()? {
                let e = all.GetElement(i)?;
                let name = e.CurrentName()?.to_string();
                if !is_result_play(&name) || e.CurrentIsOffscreen().is_ok_and(|b| b.as_bool()) { continue; }
                let r = e.CurrentBoundingRectangle()?;
                out.push((r.top, r.left, name, e));
            }
        }
        Ok(())
    })();
    out
}

/// Waits up to `secs` for a result "Play" button in `window` (search results load after the page does) and presses the first one.
/// Returns its name.
fn press_first_play(window: HWND, secs: u64) -> Result<String, String> {
    let uia = uia().ok_or("I can't use UI Automation")?;
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let found = play_buttons(&uia, window);
        // the player bar is the bottom 15% of the window
        let bar_top = unsafe { uia.ElementFromHandle(window).and_then(|w| w.CurrentBoundingRectangle()) }.map_or(i32::MAX, |r| r.top + (r.bottom - r.top) * 85 / 100);
        if let Some(i) = pick_play(&found.iter().map(|b| (b.0, b.1, b.2.clone())).collect::<Vec<_>>(), bar_top) {
            let (_, _, name, el) = &found[i];
            let pressed = unsafe { el.GetCurrentPattern(UIA_InvokePatternId) }.ok().and_then(|p| p.cast::<IUIAutomationInvokePattern>().ok()).is_some_and(|p| unsafe { p.Invoke() }.is_ok());
            crate::log::line(&format!("play: pressed {name:?} (invoked: {pressed})"));
            return if pressed { Ok(name.clone()) } else { Err(format!("I found \"{name}\" but couldn't press it")) };
        }
        if std::time::Instant::now() >= end { return Err("I couldn't find a play button on the results".into()); }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// The track named by Spotify's player bar ("Now playing: Cornfield Chase by Hans Zimmer" -> "Cornfield Chase by Hans Zimmer").
pub fn track_from(names: &[String]) -> Option<String> {
    names.iter().find_map(|n| n.strip_prefix("Now playing: ")).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Did the press work? `pre` is the bar before it ((playing, track), None when there was no bar yet), `now` after. It did when the bar
/// shows playing and either nothing was playing before or the track changed. (A browser has one media session for all its tabs and
/// reports the wrong song, so Spotify is checked on its own page.)
pub fn took(pre: &Option<(bool, String)>, now: &(bool, String)) -> bool {
    now.0 && pre.as_ref().is_none_or(|p| !p.0 || p.1 != now.1)
}

/// (is it playing, the track) from the Spotify page or app in `window`; None when there is no player bar.
fn spotify_state(uia: &IUIAutomation, window: HWND) -> Option<(bool, String)> {
    let by_type = |t: UIA_CONTROLTYPE_ID| -> Vec<IUIAutomationElement> {
        let mut v = vec![];
        let _ = (|| -> windows::core::Result<()> {
            unsafe {
                let cond = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(t.0))?;
                let all = uia.ElementFromHandle(window)?.FindAll(TreeScope_Descendants, &cond)?;
                for i in 0..all.Length()? { v.push(all.GetElement(i)?); }
            }
            Ok(())
        })();
        v
    };
    let name = |e: &IUIAutomationElement| unsafe { e.CurrentName() }.map(|n| n.to_string()).unwrap_or_default();
    let track = track_from(&by_type(UIA_GroupControlTypeId).iter().map(name).collect::<Vec<_>>())?;
    let r = unsafe { uia.ElementFromHandle(window).and_then(|w| w.CurrentBoundingRectangle()) }.ok()?;
    let bar_top = r.top + (r.bottom - r.top) * 85 / 100;
    let playing = by_type(UIA_ButtonControlTypeId).iter().any(|b| name(b) == "Pause" && unsafe { b.CurrentBoundingRectangle() }.is_ok_and(|r| r.top >= bar_top));
    Some((playing, track))
}

/// Presses the first result's Play on the Spotify page or app in `window` and waits for the player bar to show it playing, and
/// returns the track. Spotify's player ignores a press until the page has loaded, so a press that starts nothing is repeated, up to
/// three times; a repeat finds no plain "Play" once the first took, so it can't pause what it started.
fn press_until_playing(window: HWND) -> Result<String, String> {
    let uia = uia().ok_or("I can't use UI Automation")?;
    let pre = spotify_state(&uia, window);
    for attempt in 0..3 {
        if let Err(e) = press_first_play(window, if attempt == 0 { 10 } else { 4 }) {
            // no Play to press because the top result is already the thing playing
            return match &pre { Some((true, t)) => Ok(t.clone()), _ => Err(e) };
        }
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < end {
            if let Some(now) = spotify_state(&uia, window).filter(|n| took(&pre, n)) {
                // the bar shows Pause a moment before it names the new track: read the name again once it settles
                std::thread::sleep(std::time::Duration::from_millis(1800));
                return Ok(spotify_state(&uia, window).filter(|n| n.0).map_or(now.1, |n| n.1));
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
    Err("I pressed Play but the player didn't start".into())
}

pub(crate) fn exe_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..n as usize]).rsplit('\\').next().unwrap_or("").to_lowercase())
    }
}

/// The Spotify app's main window: visible, titled, owned by spotify.exe.
fn spotify_window() -> Option<HWND> {
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let out = &mut *(l.0 as *mut Option<HWND>);
        let (mut pid, mut class, mut title) = (0u32, [0u16; 64], [0u16; 8]);
        GetWindowThreadProcessId(h, Some(&mut pid));
        let (n, t) = (GetClassNameW(h, &mut class).max(0) as usize, GetWindowTextW(h, &mut title));
        if out.is_none() && t > 0 && IsWindowVisible(h).as_bool() && String::from_utf16_lossy(&class[..n]) == "Chrome_WidgetWin_1" && exe_name(pid).as_deref() == Some("spotify.exe") { *out = Some(h); }
        BOOL(1)
    }
    let mut found: Option<HWND> = None;
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut _ as isize)); }
    found
}

/// Spotify is running: it has a media session (its id names it), or a window.
fn spotify_running() -> bool { media::sessions().iter().any(|m| m.id.to_lowercase().contains("spotify")) || spotify_window().is_some() }

fn spotify_app_route(query: &str) -> Result<String, String> {
    if !crate::bridge::sys::shell_open(&format!("spotify:search:{}", util::url_encode(query))) { return Err("Windows couldn't open Spotify".into()); }
    let end = std::time::Instant::now() + std::time::Duration::from_secs(12);
    let window = loop {
        if let Some(w) = spotify_window() { break w; }
        if std::time::Instant::now() >= end { return Err("Spotify didn't open a window".into()); }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    if !webchat::bring_to_front(window) { return Err("I can't bring Spotify to the front".into()); }
    press_until_playing(window).map(|t| format!("Playing {t} on Spotify."))
}

fn window_title(h: HWND) -> String {
    let mut b = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut b) }.max(0) as usize;
    String::from_utf16_lossy(&b[..n])
}

fn browser_route(url: &str, host: &str, pick: impl Fn(&[(String, bool)]) -> Option<usize>, press: bool, before: &[Media]) -> Result<String, String> {
    // no open tab for the site: the link opens in a new one (the browser's own tab), and the page is then checked
    match webchat::open_in_tab(url, host, pick, |u| { crate::bridge::sys::shell_open(u); }) {
        webchat::Opened::LiveTab | webchat::Opened::NewTab => {}
        webchat::Opened::Failed(why) => return Err(why),
    }
    if press {
        webchat::pause(2500); // the page has to load before its player takes a press
        let window = unsafe { GetForegroundWindow() };
        crate::log::line(&format!("spotify window title: {:?}", window_title(window)));
        press_until_playing(window)?;
        // success is a new track in the media session, not the button's label (the label can be stale)
        return verify(before, 10).map(|m| format!("Playing {} on Spotify.", now_playing(&m)))
            .ok_or_else(|| "Spotify didn't start a new track (not signed in, or nothing to play)".into());
    }
    verify(before, 10).map(|m| format!("Playing {}.", now_playing(&m))).ok_or_else(|| "nothing started playing".into())
}

/// The first YouTube result's id, fetched with curl (a short wait; None when the page can't be read or has no results).
fn youtube_lookup(query: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let url = format!("https://www.youtube.com/results?search_query={}", util::url_encode(query));
    let out = std::process::Command::new("curl.exe")
        .args(["-s", "-f", "-L", "--max-time", "10", "-A", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120 Safari/537.36", "-H", "Accept-Language: en-US", "--", &url])
        .creation_flags(0x0800_0000).output().ok()?;
    youtube_id(&String::from_utf8_lossy(&out.stdout))
}

/// Plays `query` in whatever is live. Call from a worker thread. `service`: "spotify", "youtube", "youtube music", or empty.
pub fn play_song(query: &str, service: &str) -> Result<String, String> {
    let before = media::sessions();
    let tabs = webchat::tab_titles();
    let session_titles: Vec<String> = before.iter().filter(|m| m.browser()).map(|m| m.title.clone()).collect();
    let spotify_app = assist::protocol_handled("spotify") || spotify_running();
    let youtube_tab = pick_youtube_tab(&tabs).is_some();
    let plan = routes(service, spotify_app, youtube_tab);
    let mut watch: Option<Option<String>> = None; // looked up at most once
    let mut watch_url = |q: &str| -> String {
        let id = watch.get_or_insert_with(|| youtube_lookup(q)).clone();
        match id { Some(id) => format!("https://www.youtube.com/watch?v={id}"), None => format!("https://www.youtube.com/results?search_query={}", util::url_encode(q)) }
    };
    let (route, said, why) = first_that_plays(&plan, |r| match r {
        Route::SpotifyApp => spotify_app_route(query),
        Route::SpotifyTab => browser_route(&format!("https://open.spotify.com/search/{}", util::url_encode(query)), "open.spotify.com", |t| pick_spotify_tab(t, &session_titles), true, &before),
        Route::YoutubeTab => browser_route(&watch_url(query), "www.youtube.com", pick_youtube_tab, false, &before),
        Route::YoutubeNew => {
            let url = watch_url(query);
            if !crate::bridge::sys::shell_open(&url) { return Err("Windows couldn't open the browser".into()); }
            Ok(match verify(&before, 10) {
                Some(m) => format!("Playing {} on YouTube.", now_playing(&m)),
                None => format!("Opened YouTube for {}. Press play if it doesn't start.", assist::short(query, 40)),
            })
        }
    })?;
    crate::log::line(&format!("play routes tried first: {why:?}"));
    // named Spotify but it ended on YouTube: say why instead of guessing
    let asked_spotify = service.to_lowercase().contains("spotify");
    Ok(if asked_spotify && matches!(route, Route::YoutubeTab | Route::YoutubeNew) {
        let reason = if why.is_empty() { "Spotify isn't open here".to_string() } else { why.join("; ") };
        format!("I couldn't play it on Spotify ({reason}). {said}")
    } else { said })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, title: &str, artist: &str, playing: bool) -> Media { Media { id: id.into(), title: title.into(), artist: artist.into(), playing, ..Default::default() } }
    fn tabs(v: &[(&str, bool)]) -> Vec<(String, bool)> { v.iter().map(|(t, s)| (t.to_string(), *s)).collect() }

    #[test]
    fn routes_follow_what_is_live() {
        use Route::*;
        assert_eq!(routes("", true, true), [SpotifyApp, SpotifyTab, YoutubeTab, YoutubeNew], "the app first, then the web");
        assert_eq!(routes("Spotify", false, false), [SpotifyTab, YoutubeNew], "nothing open: Spotify still opens its page before YouTube");
        assert_eq!(routes("youtube", true, true), [YoutubeTab, YoutubeNew], "asking for YouTube skips Spotify");
        assert_eq!(routes("YouTube Music", true, false), [YoutubeNew]);
        assert_eq!(routes("", false, true), [SpotifyTab, YoutubeTab, YoutubeNew], "no service tries Spotify first");
    }

    #[test]
    fn a_failed_route_hands_over_to_the_next_and_the_end_lists_why() {
        use Route::*;
        let mut tried = vec![];
        let r = first_that_plays(&[SpotifyApp, SpotifyTab, YoutubeNew], |r| { tried.push(r); if r == SpotifyTab { Ok("Playing x.".into()) } else { Err("no".into()) } });
        assert_eq!(r, Ok((SpotifyTab, "Playing x.".into(), vec!["SpotifyApp: no".to_string()])), "and why the first one failed");
        assert_eq!(tried, [SpotifyApp, SpotifyTab], "nothing runs after a success");
        let all = first_that_plays(&[SpotifyApp, YoutubeNew], |r| Err(format!("{r:?} broke"))).unwrap_err();
        assert!(all.contains("SpotifyApp: SpotifyApp broke") && all.contains("YoutubeNew: YoutubeNew broke"), "{all}");
    }

    #[test]
    fn the_first_video_id_is_read_from_the_results_page() {
        assert_eq!(youtube_id(r#"{"videoId":"adAdAdAdAd1"}..."videoRenderer":{"videoId":"rFZHOHl-L8A","thumbnail":{}"#), Some("rFZHOHl-L8A".into()), "a bare id above the results is skipped");
        assert_eq!(youtube_id(r#""videoRenderer":{"videoId":"a_b-c1D2e3F"}"videoRenderer":{"videoId":"zzzzzzzzzzz"}"#), Some("a_b-c1D2e3F".into()), "the first result");
        assert_eq!(youtube_id(r#"{"videoId":"rFZHOHl-L8A"}"#), None, "no result entry: the search page is opened instead");
        assert_eq!(youtube_id("<html>consent.youtube.com</html>"), None);
        assert_eq!(youtube_id(r#""videoRenderer":{"videoId":"short""#), None, "too short");
        assert_eq!(youtube_id(r#""videoRenderer":{"videoId":"ab cd&e=fgh""#), None, "an id has no spaces or symbols");
        assert_eq!(youtube_id(""), None);
    }

    #[test]
    fn tabs_are_picked_by_title() {
        let none: Vec<String> = vec![];
        assert!(is_spotify_tab("Spotify – Web Player: Music for everyone", &none) && is_spotify_tab("Search - Spotify", &none));
        assert!(!is_spotify_tab("Hans Zimmer - Interstellar - YouTube", &none) && !is_spotify_tab("Gmail", &none));
        let playing = vec!["Starboy".to_string()];
        assert!(is_spotify_tab("Starboy • The Weeknd", &playing), "a playing Spotify tab shows the song");
        assert!(!is_spotify_tab("Starboy • The Weeknd", &none), "without a session of that song it is not known");
        assert!(!is_spotify_tab("Other • Someone", &playing));
        assert_eq!(pick_spotify_tab(&tabs(&[("Gmail", true), ("Search - Spotify", false), ("Spotify - Web Player", true)]), &none), Some(2), "the selected one");
        assert_eq!(pick_spotify_tab(&tabs(&[("Gmail", true)]), &none), None);
        assert!(is_youtube_tab("Lofi - YouTube") && is_youtube_tab("Lofi - YouTube - Audio playing - Memory usage - 80 MB") && is_youtube_tab("YouTube"));
        assert!(!is_youtube_tab("Gmail") && !is_youtube_tab("Spotify - Web Player"));
        assert_eq!(pick_youtube_tab(&tabs(&[("Gmail", true), ("Lofi - YouTube", false)])), Some(1));
    }

    #[test]
    fn only_a_result_play_button_is_pressed() {
        for n in ["Play Interstellar", "Play Starboy by The Weeknd", "play x", "Play", " play "] { assert!(is_result_play(n), "{n:?}"); }
        for n in ["Pause", "Play on Spotify Connect", "Play on a device", "Playlist", "Playing", "Next", "Open Miniplayer", ""] { assert!(!is_result_play(n), "{n:?}"); }
        let b = |t: i32, l: i32, n: &str| (t, l, n.to_string());
        // what Spotify Web shows for a search: the top result's card has a plain "Play", rows are "Play <title>", the bar has a plain "Play"
        let page = [b(1002, 1040, "Play"), b(338, 381, "Play Interstellar Main Theme - Piano Version"), b(238, 1457, "Play"), b(246, 1377, "More options for X")];
        assert_eq!(pick_play(&page, 920), Some(2), "the top result's own Play, not a lower row and never the bar's");
        assert_eq!(pick_play(&page, i32::MAX), Some(2), "even without a bar position the top-most wins");
        assert_eq!(pick_play(&[b(1002, 1040, "Play")], 920), None, "only the bar's Play: nothing to press");
        assert_eq!(pick_play(&[b(400, 300, "Play Song B"), b(400, 100, "Play Song A")], 920), Some(1), "same height: the left-most");
        assert_eq!(pick_play(&[b(1, 1, "Pause")], 920), None);
        assert_eq!(pick_play(&[], 920), None);
    }

    #[test]
    fn spotify_is_checked_on_its_own_player_bar() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(track_from(&names(&["Now playing bar", "Now playing: Cornfield Chase by Hans Zimmer", "Now playing view"])), Some("Cornfield Chase by Hans Zimmer".into()));
        assert_eq!(track_from(&names(&["Now playing bar", "Now playing view"])), None, "no track named");
        assert_eq!(track_from(&names(&["Now playing: "])), None);
        let (idle, playing) = (Some((false, "Old by A".to_string())), Some((true, "Old by A".to_string())));
        let now = |p: bool, t: &str| (p, t.to_string());
        assert!(took(&idle, &now(true, "Old by A")), "it was paused and now plays");
        assert!(took(&playing, &now(true, "New by B")), "a new track while another played");
        assert!(!took(&playing, &now(true, "Old by A")), "still the same track: the press did nothing");
        assert!(!took(&idle, &now(false, "New by B")), "a track that is loaded but paused");
        assert!(took(&None, &now(true, "X by Y")), "no bar before");
    }

    #[test]
    fn a_start_is_a_new_song_or_a_new_player() {
        let before = [m("Spotify.exe", "Old", "A", true), m("chrome.exe", "Lofi", "", false)];
        assert_eq!(started(&before, &before), None, "nothing changed");
        assert_eq!(started(&before, &[m("Spotify.exe", "New", "B", true)]).map(|x| x.title), Some("New".into()), "a new song");
        assert_eq!(started(&before, &[m("Spotify.exe", "Old", "A", true), m("msedge.exe", "Vid", "", true)]).map(|x| x.id), Some("msedge.exe".into()), "a new player");
        assert_eq!(started(&before, &[m("chrome.exe", "Lofi", "", true)]).map(|x| x.title), Some("Lofi".into()), "a paused one resumed");
        assert_eq!(started(&before, &[m("Spotify.exe", "New", "B", false)]), None, "a paused session is not playing");
        assert_eq!(now_playing(&m("x", "Song", "Me", true)), "Song by Me");
        assert_eq!(now_playing(&m("x", "Song", "", true)), "Song");
    }
}
