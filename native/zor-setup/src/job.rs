//! The work behind the wizard: unpack the embedded release zip, run install.ps1 (or uninstall.ps1) with a hidden
//! window, and send its output lines to the window. Runs on its own thread.

use crate::plan::{ps_quote, uninstall_command, Options};
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc::Sender,
    thread,
};

pub enum Msg {
    Line(String),
    Done(Result<(), String>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Install,
    Uninstall,
}

/// dist/zorui-app.zip, copied in by build.rs. Empty when the build had no ZORSETUP_ZIP.
static ZIP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.zip"));
const NO_WINDOW: u32 = 0x0800_0000;
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\ZorUI";

fn hidden(program: &str) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(NO_WINDOW);
    c
}

pub fn spawn(mode: Mode, opts: Options, tx: Sender<Msg>) {
    thread::spawn(move || {
        let r = run(mode, &opts, &tx);
        let _ = tx.send(Msg::Done(r));
    });
}

fn run(mode: Mode, opts: &Options, tx: &Sender<Msg>) -> Result<(), String> {
    if ZIP.is_empty() {
        return Err("this build of the installer has no app inside it".into());
    }
    let work = std::env::temp_dir().join("zorui-setup");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| format!("can't create {}: {e}", work.display()))?;
    let zip = work.join("app.zip");
    fs::write(&zip, ZIP).map_err(|e| format!("can't write {}: {e}", zip.display()))?;
    let app = work.join("app");

    let _ = tx.send(Msg::Line("==> Unpacking files".into()));
    let expand = format!(
        "Expand-Archive -Force -LiteralPath {} -DestinationPath {}",
        ps_quote(&zip.to_string_lossy()),
        ps_quote(&app.to_string_lossy())
    );
    stream(hidden("powershell.exe").args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &expand]), tx)?;

    let script = app.join(if mode == Mode::Install { "install.ps1" } else { "uninstall.ps1" });
    let mut ps = hidden("powershell.exe");
    ps.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&script);
    if mode == Mode::Install {
        ps.args(opts.script_args());
    }
    stream(&mut ps, tx)?;

    if mode == Mode::Install { register() } else { unregister() }
    let _ = fs::remove_dir_all(&work);
    Ok(())
}

/// Runs `c`, sends every output line to the window, and fails with the first error line when it exits non-zero.
fn stream(c: &mut Command, tx: &Sender<Msg>) -> Result<(), String> {
    let mut child = c
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't start PowerShell: {e}"))?;
    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let t2 = tx.clone();
    let errs = thread::spawn(move || pump(err, &t2));
    pump(out, tx);
    let err_text = errs.join().unwrap_or_default();
    let status = child.wait().map_err(|e| format!("PowerShell: {e}"))?;
    if status.success() {
        return Ok(());
    }
    Err(match err_text.lines().find(|l| !l.trim().is_empty()) {
        Some(l) => l.trim().to_string(),
        None => format!("PowerShell exited with code {}", status.code().unwrap_or(-1)),
    })
}

/// Sends each line of `r` to the window and returns them all joined.
fn pump(r: impl Read, tx: &Sender<Msg>) -> String {
    let mut all = String::new();
    for raw in BufReader::new(r).split(b'\n').map_while(Result::ok) {
        let line = String::from_utf8_lossy(&raw).trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        all.push_str(&line);
        all.push('\n');
        let _ = tx.send(Msg::Line(line));
    }
    all
}

/// Where the installed copy of this exe lives, so Apps & Features can run it with --uninstall.
fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("zor-ui-setup")
}

fn reg(args: &[&str]) {
    let _ = hidden("reg.exe").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Copies this exe next to the user data and lists it in Apps & Features. Best effort: the app works without it.
fn register() {
    let Ok(exe) = std::env::current_exe() else { return };
    let dest = data_dir().join("ZorUI-Setup.exe");
    let _ = fs::create_dir_all(data_dir());
    if exe != dest && fs::copy(&exe, &dest).is_err() {
        return;
    }
    let dest = dest.to_string_lossy().into_owned();
    let cmd = uninstall_command(&dest);
    for (name, kind, data) in [
        ("DisplayName", "REG_SZ", "Zor UI"),
        ("DisplayIcon", "REG_SZ", dest.as_str()),
        ("UninstallString", "REG_SZ", cmd.as_str()),
        ("NoModify", "REG_DWORD", "1"),
    ] {
        reg(&["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", data, "/f"]);
    }
}

/// Removes the Apps & Features entry and, when this is the installed copy, deletes it once this process has exited.
fn unregister() {
    reg(&["delete", UNINSTALL_KEY, "/f"]);
    let dir = data_dir();
    if std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p == dir)).unwrap_or(false) {
        let _ = Command::new("cmd.exe")
            .creation_flags(NO_WINDOW)
            .raw_arg(format!("/c ping -n 4 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display()))
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn lines(rx: &std::sync::mpsc::Receiver<Msg>) -> Vec<String> {
        rx.try_iter().filter_map(|m| if let Msg::Line(l) = m { Some(l) } else { None }).collect()
    }

    #[test]
    fn pump_forwards_non_blank_lines() {
        let (tx, rx) = channel();
        let all = pump(&b"==> one\r\n\r\n  two  \nthree"[..], &tx);
        assert_eq!(lines(&rx), ["==> one", "  two", "three"]);
        assert_eq!(all, "==> one\n  two\nthree\n");
    }

    #[test]
    fn stream_reports_the_first_error_line_on_failure() {
        let (tx, rx) = channel();
        let r = stream(Command::new("cmd.exe").args(["/c", "echo hello & echo boom 1>&2 & exit 3"]), &tx);
        assert_eq!(r, Err("boom".to_string()));
        assert!(lines(&rx).contains(&"hello".to_string()));
    }

    #[test]
    fn stream_falls_back_to_the_exit_code() {
        let (tx, _rx) = channel();
        assert_eq!(stream(Command::new("cmd.exe").args(["/c", "exit 7"]), &tx), Err("PowerShell exited with code 7".to_string()));
        assert_eq!(stream(Command::new("cmd.exe").args(["/c", "echo ok"]), &tx), Ok(()));
    }

    #[test]
    fn an_empty_payload_is_reported_not_installed() {
        if ZIP.is_empty() {
            let (tx, _rx) = channel();
            assert!(run(Mode::Install, &Options::default(), &tx).unwrap_err().contains("no app"));
        }
    }
}
