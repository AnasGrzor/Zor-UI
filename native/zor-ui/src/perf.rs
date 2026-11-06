//! What each part of the shell costs, for Settings > Usage. Every window reports its paint time and its pixel
//! buffer (win.rs); the process itself and the helper processes it started are read from Windows.

use std::{collections::HashMap, sync::Mutex, time::Instant};
use windows::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
        Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

/// Paint rates are re-measured over windows of this many seconds.
const WINDOW: f64 = 2.0;

struct Acc { paints: u32, ns: u64, since: Instant, cpu: f32, fps: f32 }

static PAINTS: Mutex<Option<HashMap<String, Acc>>> = Mutex::new(None);
/// Window handle -> (part, pixel buffer bytes).
static BUFS: Mutex<Option<HashMap<isize, (String, u64)>>> = Mutex::new(None);

/// Windows of the same kind count as one part ("Zor UI / desk-clock" and "desk-todo" are "desk cards").
pub fn group(title: &str) -> String {
    let t = title.trim_start_matches("Zor UI / ");
    if t.starts_with("desk-") { "desk cards".into() } else { t.to_string() }
}

/// Folds one paint into `a`. The rate rolls over once a window has passed.
fn add(a: &mut Acc, ns: u64, elapsed: f64) {
    a.paints += 1;
    a.ns += ns;
    if elapsed >= WINDOW {
        a.cpu = (a.ns as f64 / 1e9 / elapsed * 100.0) as f32;
        a.fps = (a.paints as f64 / elapsed) as f32;
        (a.paints, a.ns, a.since) = (0, 0, Instant::now());
    }
}

pub fn paint(part: &str, ns: u64) {
    let mut g = PAINTS.lock().unwrap_or_else(|e| e.into_inner());
    let a = g.get_or_insert_with(HashMap::new).entry(part.to_string()).or_insert_with(|| Acc { paints: 0, ns: 0, since: Instant::now(), cpu: 0.0, fps: 0.0 });
    let el = a.since.elapsed().as_secs_f64();
    add(a, ns, el);
}

pub fn buffer(hwnd: isize, part: &str, bytes: u64) {
    BUFS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(hwnd, (part.to_string(), bytes));
}

pub fn forget(hwnd: isize) {
    if let Some(m) = BUFS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() { m.remove(&hwnd); }
}

pub struct Part { pub name: String, pub windows: u32, pub bytes: u64, pub cpu: f32, pub fps: f32 }

/// Every part, busiest first. A part that has not painted for two windows reads as idle.
pub fn parts() -> Vec<Part> {
    let mut by: HashMap<String, Part> = HashMap::new();
    if let Some(m) = PAINTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        for (n, a) in m {
            let p = by.entry(n.clone()).or_insert_with(|| Part { name: n.clone(), windows: 0, bytes: 0, cpu: 0.0, fps: 0.0 });
            if a.since.elapsed().as_secs_f64() < WINDOW * 2.0 { (p.cpu, p.fps) = (a.cpu, a.fps); }
        }
    }
    if let Some(m) = BUFS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        for (n, b) in m.values() {
            let p = by.entry(n.clone()).or_insert_with(|| Part { name: n.clone(), windows: 0, bytes: 0, cpu: 0.0, fps: 0.0 });
            p.windows += 1;
            p.bytes += b;
        }
    }
    let mut v: Vec<Part> = by.into_values().collect();
    v.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.bytes.cmp(&a.bytes)).then(a.name.cmp(&b.name)));
    v
}

/// One process: share of one CPU core, working set and private bytes.
#[derive(Clone, Default)]
pub struct Proc { pub pid: u32, pub name: String, pub cpu: f32, pub ws: u64, pub private: u64 }

struct Sampler { at: Instant, times: HashMap<u32, u64>, me: Proc, kids: Vec<Proc> }

static SAMPLER: Mutex<Option<Sampler>> = Mutex::new(None);

fn ft(t: FILETIME) -> u64 { (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64 }

/// (CPU time in 100 ns, working set, private bytes) of `h`.
fn read(h: HANDLE) -> Option<(u64, u64, u64)> {
    unsafe {
        let (mut c, mut e, mut k, mut u) = Default::default();
        GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).ok()?;
        let mut m = PROCESS_MEMORY_COUNTERS_EX::default();
        m.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(h, &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb).ok()?;
        Some((ft(k) + ft(u), m.WorkingSetSize as u64, m.PrivateUsage as u64))
    }
}

/// (pid, parent, exe name) of every process.
fn all() -> Vec<(u32, u32, String)> {
    let mut v = vec![];
    unsafe {
        let Ok(s) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return v };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(s, &mut e).is_ok();
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            v.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
            ok = Process32NextW(s, &mut e).is_ok();
        }
        let _ = CloseHandle(s);
    }
    v
}

/// The pids below `root` in the process tree.
pub fn descendants(list: &[(u32, u32, String)], root: u32) -> Vec<u32> {
    let mut out: Vec<u32> = vec![];
    loop {
        let n = out.len();
        for (pid, parent, _) in list {
            if (*parent == root || out.contains(parent)) && *pid != root && !out.contains(pid) { out.push(*pid); }
        }
        if out.len() == n { return out; }
    }
}

/// This process and its helpers (CPU over the time since the last call; cached for a second).
pub fn processes() -> (Proc, Vec<Proc>) {
    let mut g = SAMPLER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = g.as_ref().filter(|s| s.at.elapsed().as_secs_f64() < 1.0) { return (s.me.clone(), s.kids.clone()); }
    let (now, prev) = (Instant::now(), g.take());
    let dt = prev.as_ref().map_or(0.0, |s| s.at.elapsed().as_secs_f64());
    let old = prev.map(|s| s.times).unwrap_or_default();
    let mut times = HashMap::new();
    let mut sample = |pid: u32, name: String, h: HANDLE| -> Proc {
        let (t, ws, private) = read(h).unwrap_or_default();
        times.insert(pid, t);
        let cpu = match old.get(&pid) { Some(&o) if dt > 0.0 => (t.saturating_sub(o) as f64 / 1e7 / dt * 100.0) as f32, _ => 0.0 };
        Proc { pid, name, cpu, ws, private }
    };
    let me_pid = unsafe { GetCurrentProcessId() };
    let me = sample(me_pid, "zorui.exe".into(), unsafe { GetCurrentProcess() });
    let list = all();
    let mut kids = vec![];
    for pid in descendants(&list, me_pid) {
        let Ok(h) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { continue };
        let name = list.iter().find(|p| p.0 == pid).map(|p| p.2.clone()).unwrap_or_default();
        kids.push(sample(pid, name, h));
        unsafe { let _ = CloseHandle(h); }
    }
    kids.sort_by(|a, b| b.ws.cmp(&a.ws));
    *g = Some(Sampler { at: now, times, me: me.clone(), kids: kids.clone() });
    (me, kids)
}

/// "12.3 MB" / "840 KB".
pub fn mb(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1 << 20) as f64) } else { format!("{} KB", bytes >> 10) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desk_cards_are_one_part() {
        assert_eq!(group("Zor UI / desk-clock"), "desk cards");
        assert_eq!(group("Zor UI / desk-todo"), "desk cards");
        assert_eq!(group("Zor UI / bar"), "bar");
    }

    #[test]
    fn rate_rolls_after_a_window() {
        let mut a = Acc { paints: 0, ns: 0, since: Instant::now(), cpu: 0.0, fps: 0.0 };
        add(&mut a, 100_000_000, 0.5);
        assert_eq!((a.cpu, a.paints), (0.0, 1));
        add(&mut a, 100_000_000, 2.0);
        assert_eq!(a.paints, 0);
        assert!((a.cpu - 10.0).abs() < 0.01, "{}", a.cpu); // 0.2 s of paint in 2 s
        assert!((a.fps - 1.0).abs() < 0.01);
    }

    #[test]
    fn finds_children_and_grandchildren() {
        let l = vec![(1, 0, "a".to_string()), (2, 1, "b".into()), (3, 2, "c".into()), (4, 9, "d".into())];
        assert_eq!(descendants(&l, 1), vec![2, 3]);
        assert!(descendants(&l, 4).is_empty());
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(mb(512 * 1024), "512 KB");
        assert_eq!(mb(3 << 20), "3.0 MB");
    }

    #[test]
    fn this_process_is_measured() {
        let (me, _) = processes();
        assert!(me.ws > 0 && me.private > 0);
    }
}
