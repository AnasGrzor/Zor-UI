//! zorui-svc: a Windows service that starts zorui and GlazeWM in the logged-on user's session at boot,
//! the way Wallpaper Engine's service starts its app.
//!
//! Why a service: glazewm.exe is a `uiAccess` exe. Windows refuses to `CreateProcess` it from a
//! Task Scheduler task (error 740), and `CreateProcessAsUser` fails the same way even with the user's
//! full admin token. `ShellExecute` can start it, so GlazeWM is started with `cmd /c start` under the
//! user's normal token; the service then watches for the process to appear. zorui starts directly.
//! The service waits until the user's Explorer has been up for a while, because glazewm panics
//! (`Result::unwrap` on E_FAIL) when started before the shell exists, and retries either program with a
//! growing delay if it fails or exits. Once both have been up for CONFIRMED_MS the service exits cleanly
//! (it stays set to start at boot), so closing a program afterwards does not start it again.
//!
//! Config: %ProgramData%\zor-ui\svc.json, written by install.ps1 (admin-only writable):
//!   {"glazewm": "C:\\Program Files\\glzr.io\\GlazeWM\\glazewm.exe", "zorui": "C:\\...\\zorui.exe", "settle_ms": 8000}
//!
//! `zorui-svc check` prints what the service would see (config, session, Explorer) without needing SYSTEM.

use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::{
    DuplicateTokenEx, GetTokenInformation, SecurityImpersonation, TokenElevation, TokenElevationType, TokenPrimary,
    TOKEN_ALL_ACCESS, TOKEN_INFORMATION_CLASS,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW, SERVICE_ACCEPT_STOP,
    SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE,
    SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetTickCount64};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetExitCodeProcess, Sleep, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
    PROCESS_INFORMATION, STARTUPINFOW,
};

const NAME: &str = "ZorShell";
/// A program that ran this long counts as healthy: its failure count resets.
const STABLE_MS: u64 = 15_000;
/// GlazeWM waits this long after zorui is up, so zorui is running when GlazeWM's own `startup_commands`
/// try to start it (zorui is single instance and the extra copy exits).
const GLAZE_AFTER_ZORUI_MS: u64 = 3_000;
/// After this many failures zorui no longer holds GlazeWM back.
const ZORUI_GIVE_UP: u32 = 3;
/// `cmd /c start` returns almost at once; this is how long the service waits for it.
const SHELL_WAIT_MS: u32 = 5_000;
/// A program started through `cmd /c start` has to show up in the session within this long, or the start failed.
const APPEAR_MS: u64 = 15_000;
/// Every managed program that has been up this long counts as started, and the service exits. glazewm's
/// startup crash comes within about 3 s, so this is well past it.
const CONFIRMED_MS: u64 = 10_000;
static STOP: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------- pure logic (unit tested)

#[derive(Debug, PartialEq)]
struct Cfg {
    glazewm: String,
    zorui: String,
    settle_ms: u64,
}

fn parse_cfg(text: &str) -> Result<Cfg, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("svc.json is not valid JSON: {e}"))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let cfg = Cfg { glazewm: s("glazewm"), zorui: s("zorui"), settle_ms: v.get("settle_ms").and_then(|x| x.as_u64()).unwrap_or(8_000) };
    if cfg.glazewm.is_empty() && cfg.zorui.is_empty() {
        return Err("svc.json names neither glazewm nor zorui".into());
    }
    Ok(cfg)
}

/// Normalises a Windows path for comparison: lower case, backslashes, no trailing slash.
fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn exe_name(path: &str) -> String {
    norm(path).rsplit('\\').next().unwrap_or("").to_string()
}

/// The command line that starts `exe` through ShellExecute (`start`), which can launch uiAccess exes.
fn shell_cmdline(exe: &str) -> String {
    format!("cmd.exe /c start \"\" \"{exe}\"")
}

/// Delay before the next start after `fails` consecutive failures: 2, 4, 8, 16, then 30 seconds.
fn backoff_ms(fails: u32) -> u64 {
    (1_000u64 << fails.min(5)).min(30_000)
}

#[derive(Default, Debug)]
struct Backoff {
    fails: u32,
    next_at: u64,
}

impl Backoff {
    fn due(&self, now: u64) -> bool {
        now >= self.next_at
    }
    fn failed(&mut self, now: u64) {
        self.fails = self.fails.saturating_add(1);
        self.next_at = now + backoff_ms(self.fails);
    }
    /// The program exited after running `ran_ms`: a long run was healthy, a short one is a failure.
    fn exited(&mut self, now: u64, ran_ms: u64) {
        if ran_ms >= STABLE_MS {
            self.fails = 0;
            self.next_at = now;
        } else {
            self.failed(now);
        }
    }
}

/// The shell counts as ready once Explorer has been seen for `settle_ms`.
fn shell_ready(first_seen: Option<u64>, now: u64, settle_ms: u64) -> bool {
    first_seen.is_some_and(|t| now.saturating_sub(t) >= settle_ms)
}

/// GlazeWM starts once zorui has been up for GLAZE_AFTER_ZORUI_MS, or at once when zorui isn't managed or keeps failing.
fn glaze_gate(zorui_managed: bool, zorui_up_since: Option<u64>, zorui_fails: u32, now: u64) -> bool {
    !zorui_managed || zorui_fails >= ZORUI_GIVE_UP || zorui_up_since.is_some_and(|t| now.saturating_sub(t) >= GLAZE_AFTER_ZORUI_MS)
}

/// The job is done when something is managed and every managed program (`up_since` of each) has been up for CONFIRMED_MS.
fn all_confirmed(up_since: &[Option<u64>], now: u64) -> bool {
    !up_since.is_empty() && up_since.iter().all(|up| up.is_some_and(|t| now.saturating_sub(t) >= CONFIRMED_MS))
}

/// A `cmd /c start` launch made at `since` has failed once the program is still missing APPEAR_MS later.
fn appear_timeout(since: u64, now: u64) -> bool {
    now.saturating_sub(since) >= APPEAR_MS
}

/// "elevated, Full" / "not elevated, Limited" from TokenIsElevated and TokenElevationType (1 Default, 2 Full, 3 Limited).
fn elev_label(elevated: bool, ty: u32) -> String {
    let ty = match ty {
        1 => "Default",
        2 => "Full",
        3 => "Limited",
        _ => "unknown type",
    };
    format!("{}, {ty}", if elevated { "elevated" } else { "not elevated" })
}

/// "0x800702e4" for an error, to put next to its message in the log.
fn hr(e: &windows::core::Error) -> String {
    format!("{e} ({:#010x})", e.code().0 as u32)
}

// ---------------------------------------------------------------- windows plumbing

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct H(HANDLE);
impl Drop for H {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}

fn data_dir() -> std::path::PathBuf {
    Path::new(&std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into())).join("zor-ui")
}

fn log(msg: &str) {
    use std::io::Write;
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("svc.log");
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("svc.log.old"));
    }
    let t = unsafe { GetLocalTime() };
    let line = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {msg}\n", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

/// startup.json, `{"run_on_startup": false}`: the Settings switch "Start Zor UI and GlazeWM at login". Missing or unreadable means on.
fn startup_enabled(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| v.get("run_on_startup").and_then(|x| x.as_bool())).unwrap_or(true)
}

fn load_cfg() -> Result<Cfg, String> {
    let p = data_dir().join("svc.json");
    parse_cfg(&std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?)
}

/// Process ids named `exe` (lower case) that run in session `sid`.
fn pids_in_session(exe: &str, sid: u32) -> Vec<u32> {
    let mut out = vec![];
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let snap = H(snap);
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap.0, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase() == exe {
                let mut s = 0u32;
                if ProcessIdToSessionId(e.th32ProcessID, &mut s).is_ok() && s == sid {
                    out.push(e.th32ProcessID);
                }
            }
            ok = Process32NextW(snap.0, &mut e).is_ok();
        }
    }
    out
}

/// The active console session, if a user is logged on to it (it has a token to query).
fn active_session() -> Option<(u32, H)> {
    unsafe {
        let sid = WTSGetActiveConsoleSessionId();
        if sid == u32::MAX {
            return None;
        }
        let mut t = HANDLE::default();
        WTSQueryUserToken(sid, &mut t).ok()?;
        Some((sid, H(t)))
    }
}

/// One 32-bit field of a token (TokenElevation, TokenElevationType).
fn token_u32(t: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Option<u32> {
    let (mut v, mut got) = (0u32, 0u32);
    unsafe { GetTokenInformation(t, class, Some(&mut v as *mut _ as *mut c_void), 4, &mut got) }.ok()?;
    Some(v)
}

fn token_label(t: HANDLE) -> String {
    match (token_u32(t, TokenElevation), token_u32(t, TokenElevationType)) {
        (Some(e), Some(ty)) => elev_label(e != 0, ty),
        _ => "elevation unreadable".into(),
    }
}

/// Starts `exe` in the user's session with the user's normal token. With `via_shell` it runs
/// `cmd /c start "" exe` (ShellExecute) and waits for cmd to finish; the program itself is not a child
/// the service can wait on, so the caller watches for it to appear. Every step is logged.
fn launch(user: &H, exe: &str, via_shell: bool) -> Result<H, String> {
    let n = exe_name(exe);
    log(&format!("{n}: launching {exe} (file exists: {}, through cmd start: {via_shell})", Path::new(exe).exists()));
    log(&format!("{n}: session token is {}", token_label(user.0)));
    unsafe {
        let mut prim = HANDLE::default();
        DuplicateTokenEx(user.0, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut prim)
            .map_err(|e| format!("DuplicateTokenEx: {}", hr(&e)))?;
        let prim = H(prim);
        let mut env: *mut c_void = std::ptr::null_mut();
        CreateEnvironmentBlock(&mut env, prim.0, false).map_err(|e| format!("CreateEnvironmentBlock: {}", hr(&e)))?;
        let mut desktop = w("winsta0\\default");
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let (app, cmdline, flags) = if via_shell {
            let sys = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
            (format!("{sys}\\System32\\cmd.exe"), shell_cmdline(exe), CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW)
        } else {
            (exe.to_string(), format!("\"{exe}\""), CREATE_UNICODE_ENVIRONMENT)
        };
        log(&format!("{n}: CreateProcessAsUser {app} with command line {cmdline}"));
        let (wapp, mut wcmd) = (w(&app), w(&cmdline));
        let cwd = w(&Path::new(exe).parent().map(|p| p.display().to_string()).unwrap_or_default());
        let r = CreateProcessAsUserW(
            prim.0,
            PCWSTR(wapp.as_ptr()),
            PWSTR(wcmd.as_mut_ptr()),
            None,
            None,
            false,
            flags,
            Some(env as *const c_void),
            PCWSTR(cwd.as_ptr()),
            &si,
            &mut pi,
        );
        let _ = DestroyEnvironmentBlock(env);
        r.map_err(|e| format!("CreateProcessAsUser: {}", hr(&e)))?;
        log(&format!("{n}: CreateProcessAsUser ok, pid {} (desktop winsta0\\default, cwd {})", pi.dwProcessId, Path::new(exe).parent().map(|p| p.display().to_string()).unwrap_or_default()));
        let _ = CloseHandle(pi.hThread);
        let proc = H(pi.hProcess);
        if via_shell {
            let waited = WaitForSingleObject(proc.0, SHELL_WAIT_MS);
            let mut code = 0u32;
            let _ = GetExitCodeProcess(proc.0, &mut code);
            log(&format!("{n}: cmd start finished: waited ok {}, exit code {code}", waited == WAIT_OBJECT_0));
            if waited != WAIT_OBJECT_0 {
                return Err(format!("cmd start did not finish within {SHELL_WAIT_MS} ms"));
            }
            if code != 0 {
                return Err(format!("cmd start exited with code {code}"));
            }
        }
        Ok(proc)
    }
}

/// One program the service keeps running.
struct Prog {
    exe: String,
    /// Started through `cmd /c start` (uiAccess exes) instead of directly.
    shell: bool,
    child: Option<(H, u64)>, // direct launches: process, start time
    /// Shell launches: when `cmd start` succeeded; the program has APPEAR_MS to show up.
    pending: Option<u64>,
    /// When it was first seen up (started by us or already running); the GlazeWM gate reads it.
    up_since: Option<u64>,
    bo: Backoff,
    adopted_logged: bool,
}

impl Prog {
    fn new(exe: &str, shell: bool) -> Self {
        Prog { exe: exe.to_string(), shell, child: None, pending: None, up_since: None, bo: Backoff::default(), adopted_logged: false }
    }

    fn tick(&mut self, user: &H, sid: u32, now: u64) {
        let n = exe_name(&self.exe);
        if let Some((h, started)) = &self.child {
            if unsafe { WaitForSingleObject(h.0, 0) } != WAIT_OBJECT_0 {
                return;
            }
            let mut code = 0u32;
            unsafe { let _ = GetExitCodeProcess(h.0, &mut code); }
            log(&format!("{n} exited with {code:#x} after {} ms", now - started));
            self.bo.exited(now, now - started);
            self.child = None;
            self.up_since = None;
        }
        let running = pids_in_session(&n, sid);
        if !running.is_empty() {
            if let Some(t) = self.pending.take() {
                log(&format!("{n} appeared (pid {running:?}) {} ms after the launch", now - t));
                self.adopted_logged = true;
            } else if !self.adopted_logged {
                // started by hand or by GlazeWM: leave it alone, both programs are single instance
                log(&format!("{n} is already running in session {sid} (pid {running:?}): leaving it alone"));
                self.adopted_logged = true;
            }
            self.up_since.get_or_insert(now);
            return;
        }
        if self.adopted_logged {
            log(&format!("{n} is no longer running"));
            self.adopted_logged = false;
        }
        self.up_since = None;
        if let Some(t) = self.pending {
            if appear_timeout(t, now) {
                self.pending = None;
                self.bo.failed(now);
                log(&format!("{n} did not appear within {APPEAR_MS} ms of the launch; failure {} in a row, next try in {} ms", self.bo.fails, backoff_ms(self.bo.fails)));
            }
            return;
        }
        if !self.bo.due(now) {
            return;
        }
        match launch(user, &self.exe, self.shell) {
            Ok(h) => {
                log(&format!("started {}{}", self.exe, if self.shell { " through cmd start: waiting for it to appear" } else { "" }));
                if self.shell {
                    self.pending = Some(now);
                } else {
                    self.child = Some((h, now));
                    self.up_since = Some(now);
                }
            }
            Err(e) => {
                self.bo.failed(now);
                log(&format!("could not start {}: {e}; failure {} in a row, next try in {} ms", self.exe, self.bo.fails, backoff_ms(self.bo.fails)));
            }
        }
    }
}

/// What the log has already said, so a state that lasts is logged once, not every second.
#[derive(Default)]
struct Notes {
    no_user: bool,
    explorer: bool,
    ready: bool,
    gate: bool,
    off: bool,
    cfg_err: Option<String>,
}

#[derive(Default)]
struct Run {
    session: Option<u32>,
    explorer_first_seen: Option<u64>,
    zor: Option<Prog>,
    glaze: Option<Prog>,
    notes: Notes,
}

fn run_loop() {
    let mut st = Run::default();
    log(&format!("service running (pid {}, zorui-svc {})", std::process::id(), env!("CARGO_PKG_VERSION")));
    while !STOP.load(Ordering::Relaxed) {
        step(&mut st);
        // both are up: leave them alone from here on, so closing one does not start it again
        let ups: Vec<Option<u64>> = [&st.zor, &st.glaze].into_iter().flatten().map(|p| p.up_since).collect();
        if all_confirmed(&ups, now_ms()) {
            log(&format!("everything has been running for {CONFIRMED_MS} ms: the service exits and starts again at the next boot"));
            return;
        }
        unsafe { Sleep(1000) };
    }
    log("service stopped (programs left running)");
}

fn step(st: &mut Run) {
    let now = now_ms();
    let Some((sid, user)) = active_session() else {
        // nobody logged on: forget the old session so the next logon starts fresh
        if !st.notes.no_user {
            log("no user is logged on to the console: waiting");
            *st = Run::default();
            st.notes.no_user = true;
        }
        return;
    };
    st.notes.no_user = false;
    if !startup_enabled(&std::fs::read_to_string(data_dir().join("startup.json")).unwrap_or_default()) {
        if !st.notes.off {
            st.notes.off = true;
            log("startup.json turns run on startup off: starting nothing");
        }
        return;
    }
    st.notes.off = false;
    let cfg = match load_cfg() {
        Ok(c) => {
            st.notes.cfg_err = None;
            c
        }
        Err(e) => {
            if st.notes.cfg_err.as_deref() != Some(e.as_str()) {
                log(&format!("config: {e}"));
                st.notes.cfg_err = Some(e);
            }
            return;
        }
    };
    if st.session != Some(sid) {
        log(&format!("console session {sid} has a user"));
        log(&format!(
            "config: glazewm={} (exists: {}) zorui={} (exists: {}) settle_ms={}",
            cfg.glazewm,
            Path::new(&cfg.glazewm).exists(),
            cfg.zorui,
            Path::new(&cfg.zorui).exists(),
            cfg.settle_ms
        ));
        *st = Run {
            session: Some(sid),
            zor: (!cfg.zorui.is_empty()).then(|| Prog::new(&cfg.zorui, false)),
            glaze: (!cfg.glazewm.is_empty()).then(|| Prog::new(&cfg.glazewm, true)),
            ..Default::default()
        };
    }
    let explorer = pids_in_session("explorer.exe", sid);
    if explorer.is_empty() {
        if st.notes.explorer {
            log("explorer.exe is gone from the session: waiting for it");
        }
        *st = Run { session: st.session, zor: st.zor.take(), glaze: st.glaze.take(), ..Default::default() };
        return;
    }
    if st.explorer_first_seen.is_none() {
        st.explorer_first_seen = Some(now);
        st.notes.explorer = true;
        log(&format!("explorer.exe is running (pid {explorer:?}): waiting {} ms for the shell to settle", cfg.settle_ms));
    }
    if !shell_ready(st.explorer_first_seen, now, cfg.settle_ms) {
        return;
    }
    if !st.notes.ready {
        st.notes.ready = true;
        log("shell is ready: starting programs");
    }
    if let Some(z) = st.zor.as_mut() {
        z.tick(&user, sid, now);
    }
    let (managed, up, fails) = st.zor.as_ref().map_or((false, None, 0), |z| (true, z.up_since, z.bo.fails));
    if glaze_gate(managed, up, fails, now) {
        if !st.notes.gate {
            st.notes.gate = true;
            log(&match (managed, fails >= ZORUI_GIVE_UP) {
                (false, _) => "zorui is not managed: GlazeWM may start".to_string(),
                (true, true) => format!("zorui failed {fails} times: not holding GlazeWM back"),
                _ => format!("zorui has been up for {} ms: GlazeWM may start", GLAZE_AFTER_ZORUI_MS),
            });
        }
        if let Some(g) = st.glaze.as_mut() {
            g.tick(&user, sid, now);
        }
    }
}

fn report(h: windows::Win32::System::Services::SERVICE_STATUS_HANDLE, state: SERVICE_STATUS_CURRENT_STATE) {
    let s = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP } else { 0 },
        ..Default::default()
    };
    unsafe { let _ = SetServiceStatus(h, &s); }
}

unsafe extern "system" fn handler(control: u32, _ty: u32, _data: *mut c_void, _ctx: *mut c_void) -> u32 {
    if control == SERVICE_CONTROL_STOP {
        log("stop requested by the service manager");
        STOP.store(true, Ordering::Relaxed);
    } else {
        log(&format!("service control {control} ignored"));
    }
    0
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = w(NAME);
    let h = match RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(handler), None) {
        Ok(h) => h,
        Err(e) => return log(&format!("RegisterServiceCtrlHandlerExW failed: {}", hr(&e))),
    };
    report(h, SERVICE_START_PENDING);
    report(h, SERVICE_RUNNING);
    run_loop();
    report(h, SERVICE_STOPPED);
    log("reported STOPPED to the service manager");
}

/// `zorui-svc check`: what the service would see right now.
fn check() {
    match load_cfg() {
        Ok(c) => println!("config ok: glazewm={} zorui={} settle_ms={}", c.glazewm, c.zorui, c.settle_ms),
        Err(e) => println!("config: {e}"),
    }
    let sid = unsafe { WTSGetActiveConsoleSessionId() };
    println!("active console session: {sid}");
    for exe in ["explorer.exe", "glazewm.exe", "zorui.exe"] {
        println!("{exe} in that session: {:?}", pids_in_session(exe, sid));
    }
}

fn main() {
    // release builds abort on panic: write the reason to the log first
    std::panic::set_hook(Box::new(|info| log(&format!("PANIC: {info}"))));
    if std::env::args().nth(1).as_deref() == Some("check") {
        return check();
    }
    log(&format!("process started: {}", std::env::args().collect::<Vec<_>>().join(" ")));
    let mut name = w(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR(name.as_mut_ptr()), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR::null(), lpServiceProc: None },
    ];
    if let Err(e) = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } {
        log(&format!("StartServiceCtrlDispatcher failed: {}", hr(&e)));
        eprintln!("zorui-svc is a Windows service: install it with install.ps1 (as admin), or run `zorui-svc check`.");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg_reads_paths_and_defaults_settle() {
        let c = parse_cfg(r#"{"glazewm":" C:\\a\\glazewm.exe ","zorui":"C:\\b\\zorui.exe"}"#).unwrap();
        assert_eq!(c, Cfg { glazewm: "C:\\a\\glazewm.exe".into(), zorui: "C:\\b\\zorui.exe".into(), settle_ms: 8_000 });
        assert_eq!(parse_cfg(r#"{"zorui":"z.exe","settle_ms":500}"#).unwrap().settle_ms, 500);
    }

    #[test]
    fn cfg_rejects_junk_and_empty() {
        assert!(parse_cfg("not json").is_err());
        assert!(parse_cfg("{}").is_err());
        assert!(parse_cfg(r#"{"glazewm":"  ","zorui":""}"#).is_err());
    }

    #[test]
    fn exe_name_is_the_lower_case_file_name() {
        assert_eq!(exe_name("C:\\Program Files\\glzr.io\\GlazeWM\\GlazeWM.exe"), "glazewm.exe");
        assert_eq!(exe_name("zorui.exe"), "zorui.exe");
    }

    #[test]
    fn startup_flag_defaults_to_on() {
        assert!(startup_enabled(""));
        assert!(startup_enabled("not json"));
        assert!(startup_enabled("{}"));
        assert!(startup_enabled(r#"{"run_on_startup": true}"#));
        assert!(!startup_enabled(r#"{"run_on_startup": false}"#));
    }

    #[test]
    fn shell_start_quotes_the_path_and_the_empty_title() {
        assert_eq!(
            shell_cmdline("C:\\Program Files\\glzr.io\\GlazeWM\\glazewm.exe"),
            "cmd.exe /c start \"\" \"C:\\Program Files\\glzr.io\\GlazeWM\\glazewm.exe\""
        );
    }

    #[test]
    fn a_shell_launch_fails_only_after_the_appear_window() {
        assert!(!appear_timeout(1_000, 1_000 + APPEAR_MS - 1));
        assert!(appear_timeout(1_000, 1_000 + APPEAR_MS));
        assert!(!appear_timeout(5_000, 1_000)); // clock never runs backwards, but don't underflow
    }

    #[test]
    fn exits_only_when_every_managed_program_is_confirmed_up() {
        let t = CONFIRMED_MS;
        assert!(!all_confirmed(&[], 99_999)); // nothing managed: never done
        assert!(!all_confirmed(&[Some(0), None], t)); // the second program is not up
        assert!(!all_confirmed(&[Some(0), Some(500)], t)); // the second one is up for less than CONFIRMED_MS
        assert!(all_confirmed(&[Some(0), Some(500)], t + 500));
        assert!(all_confirmed(&[Some(100)], 100 + t)); // a single managed program counts
        assert!(!all_confirmed(&[Some(100)], 100 + t - 1));
    }

    #[test]
    fn backoff_grows_then_caps() {
        let v: Vec<u64> = (1..=7).map(backoff_ms).collect();
        assert_eq!(v, [2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000]);
    }

    #[test]
    fn short_runs_fail_and_long_runs_reset() {
        let mut b = Backoff::default();
        assert!(b.due(0));
        b.exited(1_000, 3_000); // crashed after 3 s
        b.exited(4_000, 3_000);
        assert_eq!(b.fails, 2);
        assert!(!b.due(4_000 + 3_999));
        assert!(b.due(4_000 + 4_000));
        b.exited(50_000, STABLE_MS); // ran long enough: healthy
        assert_eq!(b.fails, 0);
        assert!(b.due(50_000));
    }

    #[test]
    fn shell_ready_waits_for_the_settle_time() {
        assert!(!shell_ready(None, 99_999, 8_000));
        assert!(!shell_ready(Some(1_000), 8_999, 8_000));
        assert!(shell_ready(Some(1_000), 9_000, 8_000));
    }

    #[test]
    fn elevation_labels_read_naturally() {
        assert_eq!(elev_label(true, 2), "elevated, Full");
        assert_eq!(elev_label(false, 3), "not elevated, Limited");
        assert_eq!(elev_label(false, 1), "not elevated, Default");
        assert_eq!(elev_label(false, 9), "not elevated, unknown type");
    }

    #[test]
    fn errors_show_their_hex_code() {
        let e = windows::core::Error::from(windows::core::HRESULT(0x800702E4u32 as i32));
        assert!(hr(&e).ends_with("(0x800702e4)"), "{}", hr(&e));
    }

    #[test]
    fn glazewm_waits_for_zorui_unless_it_is_unmanaged_or_failing() {
        assert!(glaze_gate(false, None, 0, 0)); // zorui not configured
        assert!(!glaze_gate(true, None, 0, 100)); // zorui not up yet
        assert!(!glaze_gate(true, Some(100), 0, 100 + GLAZE_AFTER_ZORUI_MS - 1));
        assert!(glaze_gate(true, Some(100), 0, 100 + GLAZE_AFTER_ZORUI_MS));
        assert!(glaze_gate(true, None, ZORUI_GIVE_UP, 0)); // zorui keeps failing: don't hold GlazeWM back
    }
}
