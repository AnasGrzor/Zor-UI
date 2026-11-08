//! Which app is using the microphone or camera, from Windows' privacy consent store. While an app
//! holds the device its key has `LastUsedTimeStop = 0`; the island shows a dot until that changes.

use windows::{
    core::{w, PCWSTR, PWSTR},
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_QWORD},
    },
};

const STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";
/// 100 ns ticks in a day. A key whose Stop was never written (a crashed app) older than this is ignored.
const DAY: u64 = 864_000_000_000;

/// The app that started most recently, for each device.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Privacy { pub mic: Option<String>, pub cam: Option<String> }

/// True when an app holds the device: it started, and either never stopped or stopped before it started.
/// `now` and the times are FILETIME (100 ns since 1601).
pub fn in_use(start: u64, stop: u64, now: u64) -> bool {
    start > 0 && stop < start && now.saturating_sub(start) < DAY
}

/// A readable name from a consent-store key: "Discord" from a path with `#` for `\`, "slackdesktop"
/// from "com.tinyspeck.slackdesktop_8yrtsj140pw4g".
/// shortcut: a packaged app shows its last id segment, not its display name; upgrade if that reads badly.
pub fn app_name(key: &str) -> String {
    if key.contains('#') {
        let exe = key.rsplit('#').next().unwrap_or(key);
        return if exe.to_ascii_lowercase().ends_with(".exe") { exe[..exe.len() - 4].into() } else { exe.into() };
    }
    let id = key.split('_').next().unwrap_or(key);
    id.rsplit('.').next().unwrap_or(id).into()
}

/// The newest in-use app out of (key name, start, stop).
fn newest(entries: &[(String, u64, u64)], now: u64) -> Option<String> {
    entries.iter().filter(|e| in_use(e.1, e.2, now)).max_by_key(|e| e.1).map(|e| app_name(&e.0))
}

/// What to announce when `new` differs from `old`: (icon, text). The microphone comes first.
pub fn notice(old: &Privacy, new: &Privacy) -> Option<(&'static str, String)> {
    if new.mic.is_some() && new.mic != old.mic { return Some(("mic", format!("{} is using the microphone", new.mic.as_ref()?))); }
    if new.cam.is_some() && new.cam != old.cam { return Some(("videocam", format!("{} is using the camera", new.cam.as_ref()?))); }
    None
}

/// Peak level (0-1) from which the mic counts as picking up a voice, and how long it stays "speaking" after.
const SPEAK_PEAK: f32 = 0.04;
const SPEAK_HOLD_MS: f64 = 300.0;

/// Turns the mic's peak level into "speaking", held for 300 ms so the icon doesn't flicker between words.
#[derive(Default)]
pub struct Hold { loud_at: Option<f64> }
impl Hold {
    pub fn feed(&mut self, peak: f32, now_ms: f64) -> bool {
        if peak >= SPEAK_PEAK { self.loud_at = Some(now_ms); }
        self.loud_at.is_some_and(|t| now_ms - t < SPEAK_HOLD_MS)
    }
}

/// The island's mic icon and its colour: red when muted, green while speaking, else orange.
/// shortcut: the level is the whole mic's, so any sound counts, and an app's own mute button can't be seen.
pub fn mic_look(muted: bool, speaking: bool) -> (&'static str, u32) {
    match (muted, speaking) {
        (true, _) => ("mic_off", 0xFFFF453A),
        (false, true) => ("mic", 0xFF30D158),
        (false, false) => ("mic", 0xFFFF9F0A),
    }
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

/// Names of the subkeys of HKCU\`path`.
fn subkeys(path: &str) -> Vec<String> {
    let mut out = vec![];
    unsafe {
        let mut key = HKEY::default();
        let p = wide(path);
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), 0, KEY_READ, &mut key) != ERROR_SUCCESS { return out; }
        for i in 0.. {
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            if RegEnumKeyExW(key, i, PWSTR(buf.as_mut_ptr()), &mut len, None, PWSTR::null(), None, None) != ERROR_SUCCESS { break; }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        let _ = RegCloseKey(key);
    }
    out
}

fn qword(path: &str, name: PCWSTR) -> u64 {
    let (p, mut v, mut size) = (wide(path), 0u64, 8u32);
    unsafe {
        let r = RegGetValueW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), name, RRF_RT_REG_QWORD, None, Some(&mut v as *mut u64 as *mut _), Some(&mut size));
        if r == ERROR_SUCCESS { v } else { 0 }
    }
}

fn filetime_now() -> u64 {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() / 100) as u64;
    unix + 116_444_736_000_000_000 // 1601 to 1970
}

/// (key name, start, stop) of every app that ever asked for `device` ("microphone" or "webcam").
fn entries(device: &str) -> Vec<(String, u64, u64)> {
    let base = format!(r"{STORE}\{device}");
    let read = |dir: &str, name: &str| {
        let p = format!(r"{dir}\{name}");
        (name.to_string(), qword(&p, w!("LastUsedTimeStart")), qword(&p, w!("LastUsedTimeStop")))
    };
    let mut out: Vec<_> = subkeys(&base).iter().filter(|n| *n != "NonPackaged").map(|n| read(&base, n)).collect();
    let np = format!(r"{base}\NonPackaged");
    out.extend(subkeys(&np).iter().map(|n| read(&np, n)));
    out
}

/// Reads the store: about 100 registry calls, cheap enough for once a second.
pub fn read() -> Privacy {
    let now = filetime_now();
    Privacy { mic: newest(&entries("microphone"), now), cam: newest(&entries("webcam"), now) }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: u64 = 134_385_390_602_421_285; // from the live check on 2026-11-07

    #[test]
    fn open_while_stop_is_zero_or_older() {
        assert!(in_use(NOW - 10_000_000, 0, NOW)); // Discord joined 1 s ago, never stopped
        assert!(!in_use(NOW - 20_000_000, NOW - 10_000_000, NOW)); // stopped after it started
        assert!(!in_use(0, 0, NOW)); // never used
        assert!(in_use(NOW - 10_000_000, NOW - 20_000_000, NOW)); // restarted after its last stop
    }

    #[test]
    fn a_key_stuck_open_for_a_day_is_ignored() {
        assert!(in_use(NOW - DAY + 1, 0, NOW));
        assert!(!in_use(NOW - DAY, 0, NOW));
    }

    #[test]
    fn names() {
        assert_eq!(app_name(r"C:#Users#Anas#AppData#Local#Discord#app-1.0.9267#Discord.exe"), "Discord");
        assert_eq!(app_name("C:#Program Files#Brave#brave.EXE"), "brave");
        assert_eq!(app_name("com.tinyspeck.slackdesktop_8yrtsj140pw4g"), "slackdesktop");
        assert_eq!(app_name("Claude_pzs8sxrjxfjjc"), "Claude");
        assert_eq!(app_name("MSTeams_8wekyb3d8bbwe"), "MSTeams");
    }

    #[test]
    fn the_newest_user_wins() {
        let e = |n: &str, a, b| (n.to_string(), a, b);
        let list = [e("Old_x", NOW - 90_000_000, 0), e("C:#a#Discord.exe", NOW - 10_000_000, 0), e("Idle_x", NOW - 5_000_000, NOW - 1_000_000)];
        assert_eq!(newest(&list, NOW).as_deref(), Some("Discord"));
        assert_eq!(newest(&[], NOW), None);
    }

    #[test]
    fn announces_only_a_new_user() {
        let none = Privacy::default();
        let mic = Privacy { mic: Some("Discord".into()), cam: None };
        assert_eq!(notice(&none, &mic), Some(("mic", "Discord is using the microphone".into())));
        assert_eq!(notice(&mic, &mic), None);
        assert_eq!(notice(&mic, &none), None); // stopping is silent: the dot just goes
        let both = Privacy { mic: Some("Discord".into()), cam: Some("Zoom".into()) };
        assert_eq!(notice(&mic, &both), Some(("videocam", "Zoom is using the camera".into())));
    }

    #[test]
    fn speaking_holds_for_300_ms() {
        let mut h = Hold::default();
        assert!(!h.feed(0.0, 0.0)); // silence before any voice
        assert!(h.feed(0.2, 1000.0));
        assert!(h.feed(0.0, 1299.0)); // a pause between words
        assert!(!h.feed(0.0, 1300.0));
        assert!(!h.feed(0.039, 1400.0)); // background hiss stays under the line
        assert!(h.feed(0.04, 1500.0));
    }

    #[test]
    fn mic_icon_and_colour() {
        assert_eq!(mic_look(false, false), ("mic", 0xFFFF9F0A));
        assert_eq!(mic_look(false, true), ("mic", 0xFF30D158));
        assert_eq!(mic_look(true, false), ("mic_off", 0xFFFF453A));
        assert_eq!(mic_look(true, true), ("mic_off", 0xFFFF453A)); // muted wins, whatever the meter says
    }

    #[test]
    fn filetime_is_after_the_live_sample() {
        assert!(filetime_now() > NOW);
    }
}
