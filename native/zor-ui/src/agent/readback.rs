//! Read-back for the two actions nothing reports on (M1): a volume change is read from the output device, and an app launch is
//! checked against the running processes. The Windows calls are passed in as functions, so the rules are tested with fakes.
//! The texts here say "not confirmed" when the read-back could not confirm; `verifier::check` turns that into `Check::NotConfirmed`.

use std::time::Duration;

/// How far a read level may be from the level asked for and still count (points out of 100).
pub const LEVEL_TOLERANCE: f32 = 2.0;
/// The change lands a moment after the request (the audio thread applies it), so the level is read again for about 1 s.
const VOLUME_RETRIES: u32 = 4;
/// Time between looks at the processes or the output device.
const POLL: Duration = Duration::from_millis(250);
/// How long an app launch is waited for before it is not confirmed.
pub const APP_WAIT: Duration = Duration::from_secs(5);

/// What the output should be after a volume tool ran.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Want { Level(f32), Mute(bool) }

/// Whether the level and mute an output reads back are what was asked for.
fn reads_as(want: Want, (level, muted): (f32, bool)) -> bool {
    match want { Want::Level(v) => (level - v).abs() <= LEVEL_TOLERANCE, Want::Mute(m) => muted == m }
}

/// The reply for a volume change, from what the output reads back: Ok when the read matches what was asked (within `LEVEL_TOLERANCE`),
/// Err with the actual level when it still differs after the retries, and "not confirmed" when no read API answers.
/// `read` gives (level, muted) or None; `sleep` waits between reads.
pub fn volume_reply(want: Want, read: &dyn Fn() -> Option<(f32, bool)>, sleep: &dyn Fn(Duration)) -> Result<String, String> {
    // shortcut: with no read API nothing is confirmed, so the verdict is Unverified; there is no second reading to compare
    const NO_READ: &str = "Volume not confirmed: I can't read the output level on this PC.";
    let Some(mut now) = read() else { return Ok(NO_READ.into()) };
    let mut retries = 0;
    loop {
        if reads_as(want, now) {
            return Ok(match want { Want::Level(v) => format!("Volume {}%.", v.round()), Want::Mute(true) => "Muted.".into(), Want::Mute(false) => "Unmuted.".into() });
        }
        if retries == VOLUME_RETRIES { break; }
        retries += 1;
        sleep(POLL);
        match read() { Some(n) => now = n, None => return Ok(NO_READ.into()) }
    }
    let (level, _) = now;
    Err(match want {
        Want::Level(v) => format!("Volume is {}%, not {}%.", level.round(), v.round()),
        Want::Mute(true) => "The output is still not muted.".into(),
        Want::Mute(false) => "The output is still muted.".into(),
    })
}

/// A volume step: the level read before the step, plus `delta`, is what the output should read after it. With no level read before
/// (no read API), the step is not confirmed.
pub fn step_reply(delta: i32, before: Option<f32>, read: &dyn Fn() -> Option<(f32, bool)>, sleep: &dyn Fn(Duration)) -> Result<String, String> {
    match before {
        Some(b) => volume_reply(Want::Level((b.round() + delta as f32).clamp(0.0, 100.0)), read, sleep),
        None => Ok("Volume not confirmed: I can't read the output level on this PC.".into()),
    }
}

/// Whether a process image name (`Notepad.exe`, `VALORANT-Win64-Shipping.exe`) is the app `app` names: the same letters and digits,
/// or the name followed by a dash or underscore (a game's launcher image). shortcut: a short name can still match an app whose image
/// name only starts with it; upgrade to comparing the shortcut's own target file if that shows up as a false Verified.
pub fn image_matches(app: &str, image: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let stem = image.to_lowercase();
    let stem = stem.strip_suffix(".exe").unwrap_or(&stem).to_string();
    let a = norm(app);
    if a.is_empty() { return false; }
    if norm(&stem) == a { return true; }
    stem.strip_prefix(&a).is_some_and(|rest| matches!(rest.chars().next(), Some('-' | '_')))
}

/// Waits up to `APP_WAIT` for each app in `names` to show as running and returns the ones that never did. `running` lists the process
/// image names now; `sleep` waits 250 ms between looks; `stop` ends the wait early (a cancelled request).
pub fn wait_for_apps(names: &[String], running: &dyn Fn() -> Vec<String>, sleep: &dyn Fn(Duration), stop: &dyn Fn() -> bool) -> Vec<String> {
    let mut missing: Vec<String> = names.to_vec();
    // the first look is at once; each later look is after one POLL, so the wait is APP_WAIT at most
    for tick in 0..=(APP_WAIT.as_millis() / POLL.as_millis()) {
        if tick > 0 { sleep(POLL); }
        let images = running();
        missing.retain(|n| !images.iter().any(|i| image_matches(n, i)));
        if missing.is_empty() || stop() { break; }
    }
    missing
}

/// The reply for an app launch. It is Ok with the launch's own reply when every app the reply says opened shows as running within the
/// wait; otherwise Ok with "not confirmed" and the reason. A launch that failed is returned as it is.
pub fn open_reply(launched: Result<String, String>, asked: &str, running: &dyn Fn() -> Vec<String>, sleep: &dyn Fn(Duration), stop: &dyn Fn() -> bool) -> Result<String, String> {
    let reply = launched?;
    // a web page opened in the browser is not an app on this PC, so no process can confirm it
    if reply.contains("in your browser") { return Ok(format!("{reply} Not confirmed: that opened a web page, not an app on this PC.")); }
    // the apps the reply did not give up on ("I can't find X on this PC" names the ones that were not opened)
    let names: Vec<String> = crate::assist::split_apps(asked).into_iter().filter(|p| !reply.contains(&format!("find {p} "))).collect();
    if names.is_empty() { return Ok(format!("{reply} Not confirmed: no app name to look for.")); }
    let missing = wait_for_apps(&names, running, sleep, stop);
    if missing.is_empty() { Ok(reply) } else { Ok(format!("{reply} Not confirmed: {} did not show up as running within {} seconds.", missing.join(" and "), APP_WAIT.as_secs())) }
}

/// The image names (`Notepad.exe`) of the processes running now. Empty when the snapshot cannot be taken.
pub fn running_images() -> Vec<String> {
    use windows::Win32::{Foundation::CloseHandle, System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS}};
    let mut out = vec![];
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snap, &mut e).is_ok();
        while more {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(String::from_utf16_lossy(&e.szExeFile[..len]));
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{outcome, state::TaskStatus, verifier::{self, Check, Kind, Verdict}, with_verdict_note};
    use std::cell::{Cell, RefCell};

    /// A sleep that only counts, so no test waits for real.
    fn counting(n: &Cell<u32>) -> impl Fn(Duration) + '_ { move |_| n.set(n.get() + 1) }

    #[test]
    fn a_volume_read_within_two_points_is_verified() {
        let n = Cell::new(0);
        let read = || Some((29.0, false));
        assert_eq!(volume_reply(Want::Level(30.0), &read, &counting(&n)), Ok("Volume 30%.".into()));
        assert_eq!(n.get(), 0, "the first read already matches: no wait");
        assert_eq!(verifier::check(Kind::Volume, &volume_reply(Want::Level(30.0), &read, &|_| {})), Check::Verified);
        assert_eq!(volume_reply(Want::Level(30.0), &|| Some((32.0, false)), &|_| {}), Ok("Volume 30%.".into()), "2 points is still within");
    }

    #[test]
    fn a_read_ten_points_away_is_failed_with_the_actual_level_in_the_record() {
        let r = volume_reply(Want::Level(30.0), &|| Some((40.0, false)), &|_| {});
        assert_eq!(r, Err("Volume is 40%, not 30%.".into()));
        assert_eq!(verifier::check(Kind::Volume, &r), Check::Failed);
    }

    #[test]
    fn a_change_that_lands_late_is_found_by_reading_again() {
        let reads = RefCell::new(vec![20.0_f32, 30.0].into_iter());
        let read = || reads.borrow_mut().next().map(|l| (l, false));
        let n = Cell::new(0);
        assert_eq!(volume_reply(Want::Level(30.0), &read, &counting(&n)), Ok("Volume 30%.".into()));
        assert_eq!(n.get(), 1, "one wait of 250 ms before the level came back");
    }

    #[test]
    fn no_read_api_is_unverified_never_verified() {
        let r = volume_reply(Want::Level(30.0), &|| None, &|_| {});
        assert!(r.as_ref().is_ok_and(|s| s.contains("not confirmed")), "{r:?}");
        assert_eq!(verifier::check(Kind::Volume, &r), Check::NotConfirmed);
        assert_eq!(verifier::judge(&[Check::NotConfirmed]), Verdict::Unverified);
        let r = step_reply(-10, None, &|| Some((40.0, false)), &|_| {});
        assert_eq!(verifier::check(Kind::Volume, &r), Check::NotConfirmed, "a step with no level read before is not confirmed");
    }

    #[test]
    fn mute_is_read_back_too() {
        assert_eq!(volume_reply(Want::Mute(true), &|| Some((30.0, true)), &|_| {}), Ok("Muted.".into()));
        assert_eq!(volume_reply(Want::Mute(false), &|| Some((30.0, false)), &|_| {}), Ok("Unmuted.".into()));
        assert_eq!(volume_reply(Want::Mute(true), &|| Some((30.0, false)), &|_| {}), Err("The output is still not muted.".into()));
    }

    #[test]
    fn a_step_is_checked_against_the_level_before_it() {
        let reads = RefCell::new(vec![40.0_f32, 30.0].into_iter());
        let read = || reads.borrow_mut().next().map(|l| (l, false));
        assert_eq!(step_reply(-10, Some(40.0), &read, &|_| {}), Ok("Volume 30%.".into()));
        assert_eq!(step_reply(10, Some(100.0), &|| Some((100.0, false)), &|_| {}), Ok("Volume 100%.".into()), "clamped at 100");
    }

    #[test]
    fn a_volume_read_that_is_verified_completes_the_task_with_no_other_step() {
        let r = volume_reply(Want::Level(30.0), &|| Some((30.0, false)), &|_| {});
        let checks = [verifier::check(Kind::Volume, &r)];
        assert_eq!(outcome(&r, &checks), (Verdict::Completed, TaskStatus::Completed));
    }

    #[test]
    fn an_unverified_volume_read_ends_unverified_with_the_sentence_in_the_answer() {
        let r = volume_reply(Want::Level(30.0), &|| None, &|_| {});
        let checks = [verifier::check(Kind::Volume, &r)];
        let (verdict, end) = outcome(&r, &checks);
        assert_eq!((verdict, end), (Verdict::Unverified, TaskStatus::Unverified));
        let answer = with_verdict_note(r, verdict, &checks).unwrap();
        assert!(answer.ends_with("I couldn't confirm that on this PC."), "{answer}");
    }

    #[test]
    fn image_names_match_the_app_asked_for() {
        assert!(image_matches("notepad", "Notepad.exe"));
        assert!(image_matches("Discord", "Discord.exe"));
        assert!(image_matches("valorant", "VALORANT-Win64-Shipping.exe"), "a game's launcher image: name, then a dash");
        assert!(image_matches("counter strike", "counterstrike.exe"), "spaces are not in the image name");
        assert!(!image_matches("notepad", "Notepad2.exe"), "a longer name is another app");
        assert!(!image_matches("notepad", "explorer.exe"));
        assert!(!image_matches("   ", "explorer.exe"));
    }

    #[test]
    fn an_app_that_appears_within_the_wait_is_verified() {
        let looks = Cell::new(0);
        let running = || { looks.set(looks.get() + 1); if looks.get() >= 4 { vec!["Notepad.exe".to_string()] } else { vec![] } };
        let n = Cell::new(0);
        let missing = wait_for_apps(&["notepad".into()], &running, &counting(&n), &|| false);
        assert!(missing.is_empty());
        assert_eq!(n.get(), 3, "found on the fourth look, after three waits of 250 ms");
    }

    #[test]
    fn an_app_that_never_appears_is_unverified_and_the_wait_stops_within_its_bound() {
        let n = Cell::new(0);
        let missing = wait_for_apps(&["notepad".into()], &|| vec![], &counting(&n), &|| false);
        assert_eq!(missing, vec!["notepad".to_string()]);
        assert_eq!(n.get(), 20, "5 s in 250 ms steps, and no more");
        let r = open_reply(Ok("Opening Notepad.".into()), "notepad", &|| vec![], &|_| {}, &|| false);
        let r = r.unwrap();
        assert!(r.contains("Not confirmed: notepad did not show up as running within 5 seconds."), "{r}");
        assert_eq!(verifier::check(Kind::AppOpen, &Ok(r)), Check::NotConfirmed);
    }

    #[test]
    fn a_cancel_ends_the_app_wait_at_once() {
        let n = Cell::new(0);
        let missing = wait_for_apps(&["notepad".into()], &|| vec![], &counting(&n), &|| true);
        assert_eq!(missing.len(), 1);
        assert_eq!(n.get(), 0, "stopped at the first look");
    }

    #[test]
    fn an_app_that_is_opened_and_found_is_verified_and_a_failed_launch_stays_failed() {
        let r = open_reply(Ok("Opening Notepad.".into()), "notepad", &|| vec!["Notepad.exe".into()], &|_| {}, &|| false);
        assert_eq!(r, Ok("Opening Notepad.".into()));
        assert_eq!(verifier::check(Kind::AppOpen, &r), Check::Verified);
        let failed = open_reply(Err("I can't find Nope on this PC.".into()), "nope", &|| vec![], &|_| {}, &|| false);
        assert_eq!(verifier::check(Kind::AppOpen, &failed), Check::Failed);
    }

    #[test]
    fn a_web_page_is_not_an_app_and_is_not_confirmed_without_a_wait() {
        let n = Cell::new(0);
        let r = open_reply(Ok("Spotify isn't installed, so I opened it in your browser.".into()), "spotify", &|| vec![], &counting(&n), &|| false);
        assert_eq!(n.get(), 0);
        assert_eq!(verifier::check(Kind::AppOpen, &r), Check::NotConfirmed);
    }

    #[test]
    fn a_mixed_launch_waits_only_for_the_apps_it_opened() {
        let r = open_reply(Ok("Opened discord. I can't find valorant on this PC.".into()), "discord and valorant", &|| vec!["Discord.exe".into()], &|_| {}, &|| false);
        assert_eq!(verifier::check(Kind::AppOpen, &r), Check::Verified, "valorant was never opened, so it is not waited for");
    }
}
