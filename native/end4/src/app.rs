//! The app: shared data (`Ctx`), every open window, timers, and jobs from background threads.

use crate::{
    bar::{self, Bar, Popup},
    conf::{self, Config, State, Theme, Watched},
    claude_usage, desk, dock, glaze, gojo, gfx::Gfx, gfx::R, hub, media, popup::{self, Pop}, proc, sys, tray, util, viz, weather,
    win::{self, Input, Win, Z},
};
use std::{
    cell::RefCell,
    sync::{atomic::{AtomicIsize, Ordering}, mpsc, Mutex, OnceLock},
};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST},
    System::DataExchange::COPYDATASTRUCT,
    UI::{HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI}, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub struct Notif { pub app: String, pub text: String, pub until: f64 }
pub struct Osd { pub icon: &'static str, pub value: Option<f32>, pub text: String, pub until: f64 }

/// What views can read (and clicks can change).
pub struct Ctx {
    pub cfg: Config,
    pub theme: Theme,
    pub state: State,
    pub weather: Option<weather::Weather>,
    pub claude: Option<claude_usage::Usage>,
    pub host: String,
    pub cpu: f32,
    pub mem: f32,
    pub disk: f32,
    pub wm: glaze::Wm,
    pub media: Option<media::Media>,
    /// Every media session (players), the current one included.
    pub sessions: Vec<media::Media>,
    pub audio: Option<media::Audio>,
    pub net: Option<sys::Net>,
    pub battery: Option<sys::Battery>,
    pub kb: String,
    pub tray: Vec<tray::Icon>,
    pub notif: Option<Notif>,
    pub osd: Option<Osd>,
    /// Bumps whenever any of the above changes; windows repaint on a new version.
    pub ver: u64,
    /// `--all`: run every ported part even if config.json "native" doesn't list it (side-by-side testing).
    pub all: bool,
    /// What views ask the app to do; drained after every input and job.
    pub actions: Vec<Action>,
}

/// A request from a view (or the bar) to the app.
pub enum Action {
    /// Open the popup `name` (a second call closes it). `mon`: a point (physical px) on the monitor to open on,
    /// else GlazeWM's focused monitor. `at`: where the island panel hangs from (physical px).
    Open { name: String, mon: Option<(i32, i32)>, at: Option<(i32, i32)> },
    Close(String),
    /// Pinned popups stay open when focus moves elsewhere.
    TogglePin(String),
    /// A view's size changed outside a paint (rare): repaint and re-place every popup.
    Relayout,
}

impl Action {
    pub fn open(name: &str) -> Action { Action::Open { name: name.into(), mon: None, at: None } }
}

/// Written to the app's message window by `end4.exe --open <name>` (WM_COPYDATA dwData).
pub const COPYDATA_ID: usize = 0x3434_4F50;

impl Ctx {
    pub fn new(all: bool) -> Ctx {
        Ctx {
            cfg: Config::parse(""), theme: Theme::parse(""), state: State::load(conf::data_dir().join("state.json")),
            weather: None, claude: None, host: util::hostname(), cpu: 0.0, mem: 0.0, disk: 0.0,
            wm: Default::default(), media: None, sessions: vec![], audio: None, net: None, battery: sys::battery(), kb: String::new(), tray: vec![],
            notif: None, osd: None, ver: 0, all, actions: vec![],
        }
    }

    /// True if this app, not Zebar, draws `part` ("desktop", "bar", ...).
    pub fn native(&self, part: &str) -> bool { self.all || self.cfg.bool(&format!("native.{part}")) }
    fn island_shown(&self) -> bool { bar::layout(&self.cfg).iter().any(|m| m.0 == "island" && m.1 != "hidden") }
}

pub type Job = Box<dyn FnOnce(&mut Ctx) + Send>;
static JOBS: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
static MSG_HWND: AtomicIsize = AtomicIsize::new(0);

/// Runs `job` on the UI thread (from any thread), then repaints what changed.
pub fn post(job: Job) {
    if let Some(tx) = JOBS.get() { let _ = tx.lock().map(|t| t.send(job)); }
    unsafe { let _ = PostMessageW(HWND(MSG_HWND.load(Ordering::Relaxed) as *mut _), WM_APP, WPARAM(0), LPARAM(0)); }
}

pub struct App {
    pub g: Gfx,
    pub cx: Ctx,
    pub wins: Vec<Win>,
    bars: Vec<Win>,
    /// Open sidebars / overview / island panel / settings.
    pops: Vec<Pop>,
    /// A UTF-16 high surrogate from WM_CHAR waiting for its pair.
    hi: u16,
    /// The focused GlazeWM window id; a change closes unpinned popups (focus moved without a WM_ACTIVATE).
    last_focus: Option<String>,
    /// The bar's hover popup.
    popup: Option<Win>,
    /// Hovered group waiting out the 350 ms delay: (bar index, id, rect).
    pending: Option<(usize, u32, R)>,
    shown: Option<(usize, u32)>,
    jobs: mpsc::Receiver<Job>,
    cfg_file: Watched,
    theme_file: Watched,
    cpu: util::Cpu,
    netmon: sys::NetMon,
    ticks: u64,
    msg: HWND,
    frame: u32,
    started: bool,
    _streams: Vec<proc::Stream>,
}

thread_local!(pub static APP: RefCell<Option<App>> = const { RefCell::new(None) });

const TICK: usize = 1;
const FRAME: usize = 2;
const HOVER: usize = 3;

impl App {
    pub fn start(all: bool) -> windows::core::Result<()> {
        let pack = conf::pack_dir();
        let g = Gfx::new(&pack.join(r"assets\fonts"), &conf::data_dir())?;
        let (tx, jobs) = mpsc::channel();
        let _ = JOBS.set(Mutex::new(tx));
        win::register(Some(wndproc));
        let msg = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), win::CLASS, windows::core::w!("end4"), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, None, None)?
        };
        MSG_HWND.store(msg.0 as isize, Ordering::Relaxed);
        let mut app = App {
            g,
            cx: Ctx::new(all),
            wins: vec![], bars: vec![], pops: vec![], hi: 0, last_focus: None, popup: None, pending: None, shown: None, jobs,
            cfg_file: Watched::new(pack.join("config.json")),
            theme_file: Watched::new(pack.join("theme.json")),
            cpu: util::Cpu::new(), netmon: sys::NetMon::default(), ticks: 0, msg, frame: 0, started: false, _streams: vec![],
        };
        app.reload();
        desk::sync(&mut app);
        gojo::sync(&mut app);
        dock::sync(&mut app);
        viz::sync(&mut app);
        app.sync_bars();
        APP.with_borrow_mut(|a| *a = Some(app));
        unsafe { SetTimer(msg, TICK, 1000, None); }

        std::thread::spawn(|| loop {
            // Weather every 30 min like the Zebar provider; retry a minute after a failed fetch.
            match weather::fetch() {
                Some(w) => { post(Box::new(move |cx| cx.weather = Some(w))); std::thread::sleep(std::time::Duration::from_secs(1800)); }
                None => std::thread::sleep(std::time::Duration::from_secs(60)),
            }
        });
        // Claude plan usage for Gojo, every 3 min; a failed fetch keeps the last value.
        std::thread::spawn(|| loop {
            if gojo::on() {
                if let Some(u) = claude_usage::fetch() { post(Box::new(move |cx| cx.claude = Some(u))); }
            }
            std::thread::sleep(std::time::Duration::from_secs(180));
        });
        Ok(())
    }

    /// The bar's data sources, started the first time a native bar shows.
    fn start_bar_sources(&mut self) {
        if self.started { return; }
        self.started = true;
        hub::serve(|| post(Box::new(|_| {})));
        glaze::start(|wm| post(Box::new(move |cx| cx.wm = wm)));
        media::start_media(|m, all| post(Box::new(move |cx| { cx.media = m; cx.sessions = all; })));
        media::start_audio(|a| post(Box::new(move |cx| osd(cx, a))));
        tray::start(|icons| post(Box::new(move |cx| cx.tray = icons)));
        let cfg = &self.cx.cfg;
        // These ran in the Zebar bar (bar.js); each is a bridge (Python) stream.
        if self.cx.island_shown() {
            self._streams.extend(proc::stream(cfg, "agents-watch", &[], |line| {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
                if let Some(a) = v.get("agents") { hub::set("end4.agents", Some(a.to_string())); post(Box::new(|_| {})); }
            }));
            self._streams.extend(proc::stream(cfg, "notifications-watch", &[], |line| {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
                let n = &v["new"];
                if n.is_null() { return; }
                let s = |k: &str| n[k].as_str().unwrap_or("").to_string();
                let (app, text) = (s("app"), if s("title").is_empty() { s("body") } else { s("title") });
                post(Box::new(move |cx| {
                    if cx.cfg.get("island.notifications").as_bool() == Some(false) { return; }
                    let secs = cx.cfg.get("island.notifySeconds").as_f64().unwrap_or(5.0);
                    cx.notif = Some(Notif { app, text, until: hub::now_ms() + secs * 1000.0 });
                }));
            }));
        }
        if cfg.get("wallpaperEngine.follow").as_bool() != Some(false) {
            self._streams.extend(proc::stream(cfg, "we-sync", &[], |_| {})); // re-themes when Wallpaper Engine's wallpaper changes
        }
    }

    /// One bar per monitor while config "native" has "bar".
    fn sync_bars(&mut self) {
        let want = self.cx.native("bar");
        if !want { self.bars.clear(); self.popup = None; return; }
        let (mons, _) = util::monitors();
        let have: Vec<(i32, i32, i32)> = self.bars.iter().map(|b| b.pos()).map(|(x, y)| (x, y, 0)).collect();
        if have.len() == mons.len() && mons.iter().all(|m| have.iter().any(|h| h.0 == m.left && h.1 == m.top)) { return; }
        self.bars.clear();
        for m in mons {
            let Ok(mut w) = Win::new(Box::new(Bar::new((m.left, m.top, m.right - m.left))), "Zor UI / bar", m.left, m.top, Z::Normal, false) else { continue };
            w.update(&self.g, &self.cx, true);
            w.move_to(m.left, m.top);
            w.show();
            let h = (56.0 * w.scale()).round() as i32;
            w.dock_top(m, h); // after show: the shell ignores app bars that aren't visible yet
            w.move_to(m.left, m.top); // in case the shell moved it while docking
            w.pin();
            self.bars.push(w);
        }
        // After docking: the tray's stand-in Shell_TrayWnd (systray-util) catches SHAppBarMessage,
        // and Explorer ignores the app bar messages it forwards. Zebar docks first for the same reason.
        // shortcut: bars rebuilt later (monitor change) can't reserve their strip until a restart
        self.start_bar_sources();
    }

    /// Re-reads config.json / theme.json if they changed. True if anything did.
    fn reload(&mut self) -> bool {
        let mut changed = false;
        if let Some(t) = self.cfg_file.changed() {
            self.cx.cfg = Config::parse(&t);
            self.g.set_font(self.cx.cfg.str("font.family"), self.cx.cfg.get("font.size").as_f64().unwrap_or(1.0) as f32,
                self.cx.cfg.get("font.weight").as_i64().unwrap_or(0) as i32);
            changed = true;
        }
        if let Some(t) = self.theme_file.changed() { self.cx.theme = Theme::parse(&t); changed = true; }
        if changed { self.cx.ver += 1; }
        changed
    }

    fn repaint(&mut self) {
        for w in self.wins.iter_mut().chain(self.bars.iter_mut()) { w.update(&self.g, &self.cx, false); }
        if let Some(p) = self.popup.as_mut().filter(|p| p.visible()) { p.update(&self.g, &self.cx, false); }
        self.paint_pops(false);
        self.frames();
    }

    /// Repaints the open popups and re-places any whose size changed (the island panel stays centred under the island).
    fn paint_pops(&mut self, force: bool) {
        for p in self.pops.iter_mut() {
            p.win.update(&self.g, &self.cx, force);
            let px = p.win.size_px();
            if px != p.px {
                p.px = px;
                let (x, y) = popup::spot(&p.name, p.mon, p.win.scale(), px, p.at);
                p.win.move_to(x, y);
            }
        }
    }

    /// Keeps a fast timer running while any view animates.
    fn frames(&mut self) {
        let ms = self.bars.iter().chain(self.wins.iter()).chain(self.pops.iter().map(|p| &p.win)).map(|w| w.view.frame_ms()).filter(|&m| m > 0).min().unwrap_or(0);
        if ms != self.frame {
            unsafe { if ms > 0 { SetTimer(self.msg, FRAME, ms, None); } else { let _ = KillTimer(self.msg, FRAME); } }
            self.frame = ms;
        }
    }

    fn tick(&mut self) {
        self.ticks += 1;
        let n = self.ticks;
        if n % 2 == 0 && self.reload() {
            desk::sync(self);
            gojo::sync(self);
            dock::sync(self);
            viz::sync(self);
            self.sync_bars();
            let gone: Vec<String> = self.pops.iter().filter(|p| !self.cx.native(&p.name)).map(|p| p.name.clone()).collect();
            for g in gone { self.close(&g); }
        }
        let popups = !self.pops.is_empty();
        let bars = !self.bars.is_empty() || popups;
        let shows = |id: &str| self.wins.iter().any(|w| w.drag_key.as_deref() == Some(id));
        if n % 3 == 0 && (bars || shows("resources")) {
            self.cx.cpu = self.cpu.usage();
            self.cx.mem = util::memory_usage();
            if shows("resources") || popups { self.cx.disk = util::disk_usage(); }
            self.cx.ver += 1;
        }
        if bars {
            let net_fast = bar::layout(&self.cx.cfg).iter().any(|m| m.0 == "netspeed" && m.1 != "hidden");
            if n % if net_fast { 2 } else { 5 } == 0 { self.cx.net = self.netmon.sample(); self.cx.ver += 1; }
            if n % 15 == 0 { self.cx.battery = sys::battery(); }
            if n % 2 == 0 && self.cx.cfg.bool("showKeyboardLayout") { self.cx.kb = sys::keyboard(); }
            // pomodoro phases end here (bar.js renderTimer): whichever bar sees it first announces it
            if let Some(done) = hub::tick(&self.cx.cfg) {
                hub::set("zor-ui.timerDone", Some(done.clone()));
                proc::run_bg(&self.cx.cfg, "toggles", &["-Action", "timer-done", "-Value", &done]);
            }
            // a recording that died mustn't leave its chip behind
            if n % 15 == 0 && hub::get("zor-ui.rec").is_some() {
                let cfg = Config(self.cx.cfg.0.clone());
                std::thread::spawn(move || {
                    if proc::bridge(&cfg, "record", &[])["recording"].as_bool() == Some(false) { hub::set("zor-ui.rec", None); post(Box::new(|_| {})); }
                });
            }
        }
        self.repaint();
    }

    fn popup_check(&mut self, bi: usize) {
        let hp = self.bars.get(bi).and_then(|b| b.view.hover_popup());
        match hp {
            Some((id, r)) => {
                if self.shown == Some((bi, id)) || self.pending.map(|p| (p.0, p.1)) == Some((bi, id)) { return; }
                self.hide_popup();
                self.pending = Some((bi, id, r));
                unsafe { SetTimer(self.msg, HOVER, 350, None); }
            }
            None => self.hide_popup(),
        }
    }

    fn hide_popup(&mut self) {
        self.pending = None;
        self.shown = None;
        if let Some(p) = &self.popup { p.hide(); }
    }

    fn show_popup(&mut self) {
        unsafe { let _ = KillTimer(self.msg, HOVER); }
        let Some((bi, id, r)) = self.pending.take() else { return };
        let Some(bar) = self.bars.get(bi) else { return };
        if bar.view.hover_popup().map(|p| p.0) != Some(id) { return; }
        let (bx, by) = bar.pos();
        let s = bar.scale();
        if self.popup.is_none() { self.popup = Win::new(Box::new(Popup { id }), "Zor UI / bar-popup", -32000, -32000, Z::Top, true).ok(); }
        let Some(p) = self.popup.as_mut() else { return };
        p.view = Box::new(Popup { id });
        p.update(&self.g, &self.cx, true);
        let (w, _) = p.view.size(&self.g, &self.cx, s);
        let x = (bx as f32 + (r.x + r.w / 2.0) * s - w * s / 2.0).max(bx as f32);
        p.move_to(x.round() as i32, (by as f32 + (r.y + r.h + 4.0) * s).round() as i32);
        p.show();
        self.shown = Some((bi, id));
    }

    // ---- popups

    /// Opens the popup `name` natively when config "native" has it (else Zebar's copy); open again closes it.
    pub fn open(&mut self, name: &str, mon: Option<(i32, i32)>, at: Option<(i32, i32)>) {
        // "keybinds" is the Settings window on its Keybinds page
        let is_settings = name == "settings" || name == "keybinds";
        if !self.cx.native(if is_settings { "settings" } else { name }) { proc::zebar_widget(name); return; }
        if self.pops.iter().any(|p| p.name == name) { self.close(name); return; }
        // the click or key that just closed it (focus loss) must not reopen it
        let closed: f64 = hub::get(&format!("end4.closedAt.{name}")).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        if hub::now_ms() - closed < 600.0 { return; }
        self.start_bar_sources();
        let (mons, _) = util::monitors();
        let pt = mon.or_else(|| self.cx.wm.monitors.iter().find(|m| m.focus).map(|m| (m.x, m.y))).unwrap_or((0, 0));
        let Some(rect) = mons.iter().copied().find(|m| pt.0 >= m.left && pt.0 < m.right && pt.1 >= m.top && pt.1 < m.bottom).or(mons.first().copied()) else { return };
        let s = dpi_scale(rect);
        let area = popup::area(name, (rect.right - rect.left) as f32 / s, (rect.bottom - rect.top) as f32 / s);
        let Some(view) = popup::make(name, area) else { return };
        let Ok(mut w) = Win::new(view, &format!("Zor UI / {name}"), rect.left, rect.top, if is_settings { Z::Normal } else { Z::Top }, false) else { return };
        w.set_focusable(true);
        w.update(&self.g, &self.cx, true);
        let px = w.size_px();
        let (x, y) = popup::spot(name, rect, s, px, at);
        w.move_to(x, y);
        w.show();
        w.focus();
        w.view.input(Input::Focus(true), &mut self.cx);
        w.update(&self.g, &self.cx, true);
        self.pops.push(Pop { name: name.into(), win: w, mon: rect, at, armed_at: hub::now_ms(), pinned: is_settings, px });
        self.frames();
    }

    pub fn close(&mut self, name: &str) {
        if let Some(i) = self.pops.iter().position(|p| p.name == name) {
            self.pops.remove(i);
            hub::set(&format!("end4.closedAt.{name}"), Some(hub::now_ms().to_string()));
            self.frames();
        }
    }

    fn run_actions(&mut self) {
        while !self.cx.actions.is_empty() {
            for a in std::mem::take(&mut self.cx.actions) {
                match a {
                    Action::Open { name, mon, at } => self.open(&name, mon, at),
                    Action::Close(n) => self.close(&n),
                    Action::TogglePin(n) => if let Some(p) = self.pops.iter_mut().find(|p| p.name == n) { p.pinned = !p.pinned; },
                    Action::Relayout => {}
                }
            }
            self.cx.ver += 1;
            self.paint_pops(true);
        }
    }

    /// A message from `end4.exe --open <name>`.
    pub fn command(&mut self, text: &str) {
        if let Some(n) = text.strip_prefix("open ").filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')) {
            self.open(n, None, None);
            self.run_actions();
        }
    }

    /// Keys, text and focus for a window with keyboard focus; true if `msg` was one of those.
    fn keys(&mut self, i: Option<usize>, pop: Option<usize>, msg: u32, wp: WPARAM) -> bool {
        let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) < 0 };
        let ev = match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN => Input::Key { vk: wp.0 as u32, ctrl: down(VK_CONTROL), shift: down(VK_SHIFT), alt: down(VK_MENU) },
            WM_CHAR => {
                let u = wp.0 as u16;
                let ch = match u {
                    0xD800..=0xDBFF => { self.hi = u; return true; }
                    0xDC00..=0xDFFF => char::decode_utf16([self.hi, u]).next().and_then(Result::ok),
                    _ => char::from_u32(u as u32),
                };
                match ch { Some(c) if !c.is_control() => Input::Char(c), _ => return true }
            }
            WM_ACTIVATE => Input::Focus(wp.0 & 0xFFFF != 0),
            _ => return false,
        };
        let w = match (i, pop) { (Some(i), _) => &mut self.wins[i], (_, Some(p)) => &mut self.pops[p].win, _ => return true };
        w.view.input(ev, &mut self.cx);
        w.update(&self.g, &self.cx, true);
        true
    }

    fn pop_msg(&mut self, pi: usize, msg: u32, wp: WPARAM, lp: LPARAM) {
        let name = self.pops[pi].name.clone();
        match msg {
            WM_KEYDOWN if wp.0 == 0x1B && !self.pops[pi].win.view.captures_esc() => { self.close(&name); return; }
            WM_ACTIVATE if wp.0 & 0xFFFF == 0 => {
                let p = &self.pops[pi];
                if !p.pinned && hub::now_ms() - p.armed_at > 500.0 { self.close(&name); return; }
                self.keys(None, Some(pi), msg, wp);
            }
            WM_DPICHANGED => {}
            _ => if !self.keys(None, Some(pi), msg, wp) { self.pops[pi].win.mouse(msg, wp, lp, &mut self.cx); },
        }
        self.cx.ver += 1;
        self.paint_pops(true);
        self.frames();
    }

    fn on(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) {
        self.dispatch(hwnd, msg, wp, lp);
        self.run_actions();
    }

    fn dispatch(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) {
        if hwnd == self.msg {
            match (msg, wp.0) {
                (WM_TIMER, TICK) => self.tick(),
                (WM_TIMER, FRAME) => {
                    // each view paints at its own rate: a tile that wants 33 ms frames must not drag the
                    // bar, the desk cards and the island down to 30 fps with it
                    let now = std::time::Instant::now();
                    for w in self.bars.iter_mut().chain(self.wins.iter_mut()).chain(self.pops.iter_mut().map(|p| &mut p.win)) {
                        let ms = w.view.frame_ms();
                        if ms > 0 && w.due(now, ms) { w.update(&self.g, &self.cx, true); }
                        else { w.update(&self.g, &self.cx, false); }
                    }
                    self.frames();
                }
                (WM_TIMER, HOVER) => self.show_popup(),
                (WM_APP, _) => {
                    while let Ok(job) = self.jobs.try_recv() { job(&mut self.cx); }
                    self.cx.ver += 1;
                    // GlazeWM moved focus to another window: popups opened by a shortcut may never get a WM_ACTIVATE to lose
                    let focus = self.cx.wm.windows.iter().find(|w| w.focus).map(|w| w.id.clone());
                    if focus != self.last_focus {
                        self.last_focus = focus;
                        let stale: Vec<String> = self.pops.iter().filter(|p| !p.pinned && hub::now_ms() - p.armed_at > 500.0).map(|p| p.name.clone()).collect();
                        for n in stale { self.close(&n); }
                    }
                    self.repaint();
                }
                _ => {}
            }
            return;
        }
        if msg == WM_DISPLAYCHANGE {
            self.bars.clear();
            self.pops.clear();
            self.sync_bars();
            desk::sync(self);
            return;
        }
        if let Some(pi) = self.pops.iter().position(|p| p.win.hwnd == hwnd) { self.pop_msg(pi, msg, wp, lp); return; }
        if let Some(i) = self.wins.iter().position(|w| w.hwnd == hwnd) { if self.keys(Some(i), None, msg, wp) { return; } }
        if let Some(bi) = self.bars.iter().position(|w| w.hwnd == hwnd) {
            match msg {
                WM_DPICHANGED => self.bars[bi].update(&self.g, &self.cx, true),
                // shortcut: ABN_POSCHANGED / ABN_FULLSCREENAPP are ignored; re-docking on them can loop through the shell
                win::WM_APPBAR => {}
                _ => {
                    if matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN) { self.hide_popup(); } // a click opens a sidebar instead
                    let before = self.cx.ver;
                    self.bars[bi].mouse(msg, wp, lp, &mut self.cx);
                    if !matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN) { self.popup_check(bi); }
                    if self.cx.ver != before || matches!(msg, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_LBUTTONDBLCLK | WM_MOUSEWHEEL) { self.cx.ver += 1; }
                    // only this bar changed: repainting every window on each pointer move is what made
                    // the whole desktop feel sluggish
                    self.bars[bi].update(&self.g, &self.cx, false);
                    self.paint_pops(false);
                    self.frames();
                }
            }
            return;
        }
        let Some(i) = self.wins.iter().position(|w| w.hwnd == hwnd) else { return };
        match msg {
            WM_DPICHANGED => self.wins[i].update(&self.g, &self.cx, true),
            _ => {
                if self.wins[i].mouse(msg, wp, lp, &mut self.cx) {
                    if let Some(k) = self.wins[i].drag_key.clone() { let p = self.wins[i].pos(); desk::save_pos(&mut self.cx, &k, p); }
                }
                self.wins[i].update(&self.g, &self.cx, false);
            }
        }
    }
}

/// watchAudio() in bar.js: a volume or mic change shows its level for a moment.
fn osd(cx: &mut Ctx, a: media::Audio) {
    if let Some(prev) = cx.audio {
        let mic = prev.mic_muted != a.mic_muted;
        let island = cx.island_shown();
        let on = |k: &str| cx.cfg.get(&format!("island.{k}")).as_bool() != Some(false);
        let changed = mic || prev.muted != a.muted || prev.volume.round() != a.volume.round();
        if changed && (!island || on(if mic { "mic" } else { "volume" })) {
            let secs = if island { cx.cfg.get("island.osdSeconds").as_f64().unwrap_or(2.0) } else { 1.8 };
            let until = hub::now_ms() + secs * 1000.0;
            let m = a.mic_muted == Some(true);
            cx.osd = Some(if mic {
                Osd { icon: if m { "mic_off" } else { "mic" }, value: None, text: if m { "Microphone muted" } else { "Microphone on" }.into(), until }
            } else {
                Osd { icon: bar::volume_icon(Some(&a)), value: Some(if a.muted { 0.0 } else { a.volume }), text: if a.muted { "Muted".into() } else { format!("{}%", a.volume.round()) }, until }
            });
        }
    }
    cx.audio = Some(a);
}

/// DPI scale of the monitor `r` is (GetDpiForMonitor, so it's right before any window exists there).
fn dpi_scale(r: RECT) -> f32 {
    let (mut x, mut y) = (96u32, 96u32);
    unsafe {
        let m = MonitorFromPoint(POINT { x: r.left + 1, y: r.top + 1 }, MONITOR_DEFAULTTONEAREST);
        let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y);
    }
    x as f32 / 96.0
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(r) = win::early(hwnd, msg, wp, lp) { return r; }
    if msg == WM_COPYDATA {
        let cds = &*(lp.0 as *const COPYDATASTRUCT);
        if cds.dwData == COPYDATA_ID && !cds.lpData.is_null() {
            let text = String::from_utf8_lossy(std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize)).into_owned();
            APP.with(|a| if let Ok(mut a) = a.try_borrow_mut() { if let Some(app) = a.as_mut() { app.command(&text); } });
            return LRESULT(1);
        }
    }
    const MOUSE: [u32; 10] = [WM_MOUSEMOVE, win::WM_MOUSELEAVE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_LBUTTONDBLCLK, WM_MOUSEWHEEL];
    let ours = |m: u32| MOUSE.contains(&m) || matches!(m, WM_TIMER | WM_APP | win::WM_APPBAR | WM_KEYDOWN | WM_CHAR);
    if ours(msg) || matches!(msg, WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SYSKEYDOWN | WM_ACTIVATE) {
        // try_borrow: a call made while handling a message (SetWindowPos, DestroyWindow) can re-enter here
        APP.with(|a| if let Ok(mut a) = a.try_borrow_mut() { if let Some(app) = a.as_mut() { app.on(hwnd, msg, wp, lp); } });
        if ours(msg) { return LRESULT(0); }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
