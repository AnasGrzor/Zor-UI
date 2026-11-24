//! The game played last, so "open last game" can start it again. Once a second, while a window is full screen, the app calls
//! `note`: a full-screen window whose program sits in a games folder is saved to %LOCALAPPDATA%\zor-ui\last_game.json as its
//! title and exe path.

use serde_json::json;
use std::sync::Mutex;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};

/// The exe saved last, so the file is written only when the game changes.
static SEEN: Mutex<String> = Mutex::new(String::new());

fn file() -> std::path::PathBuf { crate::bridge::common::root().join("last_game.json") }

/// Is the program at `exe` a game? Only programs in a games folder count, so a browser or video player shown full screen never does.
/// shortcut: a game installed outside these folders is never noted; add its folder here when one is missed.
pub fn is_game(exe: &str) -> bool {
    let p = exe.to_lowercase().replace('/', "\\");
    ["\\steamapps\\common\\", "\\epic games\\", "\\riot games\\", "\\xboxgames\\", "\\ea games\\", "\\games\\"].iter().any(|f| p.contains(f))
}

/// The name to look the game up by: its window title without trademark signs ("ELDEN RING™" -> "ELDEN RING") and without the
/// zero-width characters Call of Duty puts between every letter of its title.
pub fn clean_title(t: &str) -> String {
    t.chars().filter(|c| !matches!(c, '™' | '®' | '©' | '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}')).collect::<String>().trim().to_string()
}

/// Notes the window in front when it is a game. Call only while a window is full screen.
pub fn note() {
    let (title, exe) = unsafe {
        let fg = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(fg, &mut buf).max(0) as usize;
        (clean_title(&String::from_utf16_lossy(&buf[..n])), crate::bridge::sys::exe_of(pid).unwrap_or_default())
    };
    if title.is_empty() || !is_game(&exe) { return; }
    let Ok(mut seen) = SEEN.lock() else { return };
    if *seen == exe { return; }
    crate::log::line(&format!("last game: {title}"));
    if crate::bridge::common::save_json(&file(), &json!({ "title": title, "exe": exe })) { *seen = exe; }
}

/// Does an app name mean the last game ("last game", "my last game", "the game I played last")?
pub fn asks(name: &str) -> bool {
    let a = name.to_lowercase();
    ["last game", "game i played last", "last played game"].iter().any(|p| a.contains(p))
}

/// Starts the game noted last: by its installed name first (a launcher shortcut starts it the right way), else its exe.
pub fn open(cfg: &crate::conf::Config) -> Result<String, String> {
    let v = crate::bridge::common::load_json(&file(), json!({}));
    // cleaned again: a title saved before clean_title dropped zero-width characters still holds them
    let (title, exe) = (clean_title(v["title"].as_str().unwrap_or("")), v["exe"].as_str().unwrap_or(""));
    if exe.is_empty() { return Err("I haven't seen you play a game yet. Play one full screen once and I'll remember it.".into()); }
    if let Some(r) = crate::assist::launch_named(cfg, &title) { return r; }
    if crate::bridge::sys::shell_open(exe) { Ok(format!("Opening {title}.")) } else { Err(format!("Windows couldn't open {title}.")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_programs_in_a_games_folder_are_games() {
        assert!(is_game(r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe"));
        assert!(is_game(r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe"));
        assert!(is_game(r"D:\Games\Hades\Hades.exe") && is_game(r"C:\XboxGames\Forza\Content\forza.exe"));
        assert!(!is_game(r"C:\Program Files\Google\Chrome\Application\chrome.exe"), "a full-screen video is not a game");
        assert!(!is_game(r"C:\Program Files\VideoLAN\VLC\vlc.exe") && !is_game(""));
    }

    #[test]
    fn the_title_loses_trademark_signs_and_the_request_is_recognised() {
        assert_eq!(clean_title(" ELDEN RING™ "), "ELDEN RING");
        assert_eq!(clean_title("\u{200b}C\u{200b}a\u{200b}l\u{200b}l\u{200b} \u{200b}o\u{200b}f\u{200b} \u{200b}D\u{200b}u\u{200b}t\u{200b}y\u{200b}®\u{200b}\u{200b}"), "Call of Duty", "Call of Duty's real title");
        assert!(asks("last game") && asks("my last game") && asks("the game I played last"));
        assert!(!asks("Valorant") && !asks("Discord"));
    }
}
