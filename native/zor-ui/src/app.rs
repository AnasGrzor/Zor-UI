//! The app: shared data (`Ctx`), every open window, timers, and jobs from background threads.

use crate::{
    assist,
    bar::{self, Bar, Popup},
    conf::{self, Config, State, Theme, Watched},
    claude_usage, desk, pill, privacy, dock, drive, driveframe, frame, glaze, guides, buddy, gfx::Gfx, gfx::R, hub, media, menu::{self, Menu}, popup::{self, Pop}, proc, sys, tray, util, viz, weather,
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

/// The island's toast banner; `open` is what a click on it launches (see bridge/notify.rs open_target), "" for our own.
pub struct Notif { pub app: String, pub text: String, pub until: f64, pub open: String }

impl Notif {
    /// What a click should launch while the banner is still showing.
    pub fn target(n: &Option<Notif>, now: f64) -> Option<&str> {
        n.as_ref().filter(|n| n.until > now && !n.open.is_empty()).map(|n| n.open.as_str())
    }
}
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
    /// The agents (`zor-ui.agents`), read by the island and bar drawing. Passed in, so tests don't read the real data file.
    pub agents: Vec<serde_json::Value>,
    /// The apps holding the microphone and camera (polled every tick while the island shows it).
    pub privacy: crate::privacy::Privacy,
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
    /// A right-click menu at a point (physical px).
    Menu(Vec<menu::Item>, (i32, i32)),
    CloseMenu,
    /// Runs a command as if `zorui.exe --open` had sent it ("buddy listen").
    Command(String),
}

impl Action {
    pub fn open(name: &str) -> Action { Action::Open { name: name.into(), mon: None, at: None } }
}

/// Written to the app's message window by `zorui.exe --open <name>` (WM_COPYDATA dwData).
pub const COPYDATA_ID: usize = 0x3434_4F50;

impl Ctx {
    pub fn new(all: bool) -> Ctx {
        Ctx {
            cfg: Config::parse(""), theme: Theme::parse(""), state: State::load(conf::data_dir().join("state.json")),
            weather: None, claude: None, host: util::hostname(), cpu: 0.0, mem: 0.0, disk: 0.0,
            wm: Default::default(), media: None, sessions: vec![], audio: None, net: None, battery: sys::battery(), kb: String::new(), tray: vec![],
            notif: None, osd: None, agents: vec![], privacy: Default::default(), ver: 0, all, actions: vec![],
        }
    }

    /// True if this app draws `part` ("desktop", "bar", ...).
    pub fn native(&self, part: &str) -> bool { self.all || self.cfg.bool(&format!("native.{part}")) }
    /// The edge the native bar sits on.
    pub fn bar_edge(&self) -> bar::Edge { if self.native("bar") { bar::Edge::parse(self.cfg.str("bar.position")) } else { bar::Edge::Top } }
    fn island_shown(&self) -> bool { bar::layout(&self.cfg).iter().any(|m| m.0 == "island" && m.1 != "hidden") }
}

const TRAY_FLYOUT: &str = "tray-flyout";

fn contains(r: &RECT, p: POINT) -> bool { p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom }

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
    /// The edge `bars` were built for.
    bars_edge: bar::Edge,
    /// Open sidebars / overview / island panel / settings.
    pops: Vec<Pop>,
    /// The right-click menu (and when it opened).
    menu: Option<(Win, f64)>,
    /// A UTF-16 high surrogate from WM_CHAR waiting for its pair.
    hi: u16,
    /// The focused GlazeWM window id; a change closes unpinned popups (focus moved without a WM_ACTIVATE).
    last_focus: Option<String>,
    /// The bar's hover popup.
    popup: Option<Win>,
    /// Hovered group waiting out the 350 ms delay: (bar index, id, rect).
    pending: Option<(usize, u32, R, String)>,
    shown: Option<(usize, u32)>,
    jobs: mpsc::Receiver<Job>,
    cfg_file: Watched,
    /// Workspaces on the main monitor as last read from config, to rebind GlazeWM when it changes.
    ws_main: Option<u32>,
    theme_file: Watched,
    cpu: util::Cpu,
    netmon: sys::NetMon,
    ticks: u64,
    msg: HWND,
    frame: u32,
    /// timeBeginPeriod(1) is active: without it a 16 ms timer fires every 15.6 or 31 ms.
    hires: bool,
    /// Game mode: the windows that were showing when a full-screen app took over (None = shell is live).
    game: Option<Vec<HWND>>,
    /// Consecutive ticks that disagreed with `game`.
    game_votes: u8,
    started: bool,
    _streams: Vec<proc::Stream>,
}

/// Switching needs two ticks in a row (about 2 s) so a flicker of the foreground window doesn't hide the shell.
fn game_vote(suspended: bool, want: bool, votes: u8) -> (bool, u8) {
    if suspended == want { return (false, 0); }
    if votes + 1 >= 2 { (true, 0) } else { (false, votes + 1) }
}

thread_local!(pub static APP: RefCell<Option<App>> = const { RefCell::new(None) });

const TICK: usize = 1;
const FRAME: usize = 2;
const HOVER: usize = 3;
/// While the tray flyout is open: watches for a click outside it, the only thing that closes it.
const OUTSIDE: usize = 4;
/// Hides the tray library's stand-in `Shell_TrayWnd` if something shows it (tray.rs).
const GUARD: usize = 5;

impl App {
    pub fn start(all: bool) -> windows::core::Result<()> {
        let pack = conf::pack_dir();
        let g = Gfx::new(&pack.join(r"assets\fonts"), &conf::data_dir())?;
        let (tx, jobs) = mpsc::channel();
        let _ = JOBS.set(Mutex::new(tx));
        win::register(Some(wndproc));
        let msg = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), win::CLASS, windows::core::w!("zor-ui"), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, None, None)?
        };
        MSG_HWND.store(msg.0 as isize, Ordering::Relaxed);
        let mut app = App {
            g,
            cx: Ctx { agents: hub::get_json("zor-ui.agents").as_array().cloned().unwrap_or_default(), ..Ctx::new(all) },
            wins: vec![], bars: vec![], bars_edge: bar::Edge::Top, pops: vec![], menu: None, hi: 0, last_focus: None, popup: None, pending: None, shown: None, jobs,
            cfg_file: Watched::new(pack.join("config.json")),
            ws_main: None,
            theme_file: Watched::new(pack.join("theme.json")),
            cpu: util::Cpu::new(), netmon: sys::NetMon::default(), ticks: 0, msg, frame: 0, hires: false, game: None, game_votes: 0, started: false, _streams: vec![],
        };
        app.reload();
        desk::sync(&mut app);
        buddy::sync(&mut app);
        // an interrupted task with a plan resumes from its next step, or waits for the user (agent::resume_on_start)
        crate::agent::resume_on_start();
        // the chat bots (Discord, when built with the feature and enabled in Settings), on their own threads
        crate::messaging::sync();
        dock::sync(&mut app);
        frame::sync(&mut app);
        guides::sync(&mut app);
        viz::sync(&mut app);
        app.sync_bars();
        APP.with_borrow_mut(|a| *a = Some(app));
        unsafe { SetTimer(msg, TICK, 1000, None); SetTimer(msg, GUARD, 300, None); }
        crate::appicon::add(msg);

        std::thread::spawn(|| loop {
            // Weather every 30 min; retry a minute after a failed fetch.
            match weather::fetch() {
                Some(w) => { post(Box::new(move |cx| cx.weather = Some(w))); std::thread::sleep(std::time::Duration::from_secs(1800)); }
                None => std::thread::sleep(std::time::Duration::from_secs(60)),
            }
        });
        // Claude plan usage for Gojo, the bar chip and auto-switch, every 3 min; a failed fetch keeps the last value.
        std::thread::spawn(|| loop {
            let cfg = conf::Config::load();
            if buddy::on() || crate::bridge::claude_acct::wanted(&cfg) {
                if let Some(u) = claude_usage::fetch() { post(Box::new(move |cx| cx.claude = Some(u))); }
                crate::bridge::claude_acct::auto_tick(&cfg);
            }
            std::thread::sleep(std::time::Duration::from_secs(180));
        });
        Ok(())
    }

    /// The bar's data sources, started the first time a native bar shows.
    fn start_bar_sources(&mut self) {
        if self.started { return; }
        self.started = true;
        glaze::start(|wm| post(Box::new(move |cx| cx.wm = wm)));
        crate::rescue::watch();
        media::start_media(|m, all| post(Box::new(move |cx| { cx.media = m; cx.sessions = all; })));
        media::start_audio(|a| post(Box::new(move |cx| osd(cx, a))));
        tray::start(|icons| post(Box::new(move |cx| cx.tray = icons)));
        let cfg = &self.cx.cfg;
        // Each of these is a bridge stream.
        if self.cx.island_shown() {
            self._streams.extend(proc::stream(cfg, "agents-watch", &[], |line| {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
                if let Some(a) = v.get("agents") {
                    hub::set("zor-ui.agents", Some(a.to_string()));
                    let list = a.as_array().cloned().unwrap_or_default();
                    post(Box::new(move |cx| cx.agents = list));
                }
            }));
            self._streams.extend(proc::stream(cfg, "notifications-watch", &[], |line| {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
                let n = &v["new"];
                if n.is_null() { return; }
                hub::set("zor-ui.notifLast", Some(n.to_string())); // the full notification: a banner click opens it in the island
                let s = |k: &str| n[k].as_str().unwrap_or("").to_string();
                let (app, text, open) = (s("app"), if s("title").is_empty() { s("body") } else { s("title") }, s("open"));
                post(Box::new(move |cx| {
                    if cx.cfg.get("island.notifications").as_bool() == Some(false) { return; }
                    let secs = cx.cfg.get("island.notifySeconds").as_f64().unwrap_or(5.0);
                    cx.notif = Some(Notif { app, text, until: hub::now_ms() + secs * 1000.0, open });
                }));
            }));
        }
        if cfg.get("wallpaperEngine.follow").as_bool() != Some(false) {
            self._streams.extend(proc::stream(cfg, "we-sync", &[], |_| {})); // re-themes when Wallpaper Engine's wallpaper changes
        }
    }

    /// One bar per monitor while config "native" has "bar".
    fn sync_bars(&mut self) { self.place_bars(true) }

    /// `restart_ok`: a bar that moved may restart the app (config changes). A display change rebuilds in place.
    fn place_bars(&mut self, restart_ok: bool) {
        let want = self.cx.native("bar");
        if !want { self.bars.clear(); self.popup = None; return; }
        let (mons, _) = util::monitors();
        // The tray's stand-in Shell_TrayWnd swallows app bar messages once it runs, so a bar built
        // now (moved edge, new monitor) could not reserve its strip: start over in a fresh process.
        let edge = self.cx.bar_edge();
        let spot = |m: RECT| { let r = bar::strip(m, edge, (edge.thick() * dpi_scale(m)).round() as i32); (r.left, r.top) };
        let have: Vec<(i32, i32)> = self.bars.iter().map(|b| b.pos()).collect();
        let want: Vec<(i32, i32)> = mons.iter().map(|m| spot(*m)).collect();
        // every config.json change comes here (a hairstyle, a slider): only a bar that must move restarts the app
        if bars_current(&have, &want, self.bars_edge == edge) { return; }
        // Only bars that existed and moved need a new process. With none at all, building them again is enough;
        // restarting here looped forever (have [] vs want two bars, every pass).
        if restart_ok && self.started && !have.is_empty() { crate::log::line(&format!("bars moved: have {have:?}, want {want:?}, edge {edge:?} was {:?}", self.bars_edge)); return restart(); }
        self.bars_edge = edge;
        self.bars.clear();
        for (m, (x, y)) in mons.into_iter().zip(want) {
            let mut w = match Win::new(Box::new(Bar::new((m.left, m.top, m.right - m.left, m.bottom - m.top), edge, (x, y))), "Zor UI / bar", x, y, Z::Normal, false) {
                Ok(w) => w,
                Err(e) => { crate::log::line(&format!("bar window failed at {x},{y}: {e}")); continue; }
            };
            w.update(&self.g, &self.cx, true);
            w.move_to(x, y);
            w.show();
            let h = (edge.thick() * w.scale()).round() as i32;
            w.dock(m, edge, h); // after show: the shell ignores app bars that aren't visible yet
            w.move_to(x, y); // in case the shell moved it while docking
            w.pin();
            self.bars.push(w);
        }
        // After docking: the tray's stand-in Shell_TrayWnd (systray-util) catches SHAppBarMessage,
        // and Explorer ignores the app bar messages it forwards.
        self.start_bar_sources();
    }

    /// Re-reads config.json / theme.json if they changed. True if anything did.
    fn reload(&mut self) -> bool {
        let mut changed = false;
        if let Some(t) = self.cfg_file.changed() {
            // A broken config.json keeps the last good settings: parse would give defaults, and the default
            // workspacesMain would rebind GlazeWM and reset keybinds.
            if Config::is_broken(&t) {
                crate::log::line("config.json isn't valid JSON; keeping the last good settings");
            } else {
                self.cx.cfg = Config::parse(&t);
                // a switch in Settings turns a chat bot on or off here, without a restart
                crate::messaging::sync();
                self.g.set_font(self.cx.cfg.str("font.family"), self.cx.cfg.get("font.size").as_f64().unwrap_or(1.0) as f32,
                    self.cx.cfg.get("font.weight").as_i64().unwrap_or(0) as i32);
                let m = self.cx.cfg.get("workspacesMain").as_u64().unwrap_or(5) as u32;
                if self.ws_main.is_some_and(|p| p != m) && self.cx.cfg.get("workspacesRebind").as_bool().unwrap_or(true) { glaze::apply_split(m); }
                self.ws_main = Some(m);
                if !cfg!(test) { crate::keymap::apply(&self.cx.cfg); }
                changed = true;
            }
        }
        if let Some(t) = self.theme_file.changed() {
            let mut theme = Theme::parse(&t);
            let mut animate = BOOL(0);
            let system_motion = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some((&mut animate as *mut BOOL).cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_ok() } && animate.as_bool();
            theme.transition_from(&self.cx.theme, self.cx.ver > 0 && system_motion && !self.cx.cfg.bool("wallpaperTheme.reducedMotion"));
            self.cx.theme = theme; changed = true;
        }
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
                // a resizable window is placed by the user's drag, not re-centred
                if p.win.view.limits().is_some() { continue; }
                let (x, y) = popup::spot(&p.name, p.mon, p.win.scale(), px, p.at, self.cx.bar_edge());
                p.win.move_to(x, y);
            }
        }
    }

    /// Keeps a fast timer running while any view animates.
    fn frames(&mut self) {
        if self.game.is_some() { return; }
        let ms = self.bars.iter().chain(self.wins.iter()).chain(self.pops.iter().map(|p| &p.win)).chain(self.menu.iter().map(|m| &m.0)).map(|w| if w.gliding { 16 } else { w.view.frame_ms() }).chain(self.cx.theme.animating().then_some(16)).filter(|&m| m > 0).min().unwrap_or(0);
        let glide = self.bars.iter().chain(self.wins.iter()).chain(self.pops.iter().map(|p| &p.win)).chain(self.menu.iter().map(|m| &m.0)).any(|w| w.gliding);
        if glide != self.hires {
            unsafe { if glide { windows::Win32::Media::timeBeginPeriod(1); } else { windows::Win32::Media::timeEndPeriod(1); } }
            self.hires = glide;
        }
        if ms != self.frame {
            unsafe { if ms > 0 { SetTimer(self.msg, FRAME, ms, None); } else { let _ = KillTimer(self.msg, FRAME); } }
            self.frame = ms;
        }
    }

    /// Reads who holds the mic and camera; a new app also gets a few seconds in the island.
    fn privacy_check(&mut self) {
        let cx = &mut self.cx;
        let on = cx.island_shown() && cx.cfg.get("island.privacy").as_bool() != Some(false);
        let new = if on { privacy::read() } else { Default::default() };
        if new == cx.privacy { return; }
        if let Some((icon, text)) = privacy::notice(&cx.privacy, &new) {
            cx.osd = Some(Osd { icon, value: None, text, until: hub::now_ms() + 4000.0 });
        }
        media::MIC_WATCH.store(new.mic.is_some(), std::sync::atomic::Ordering::Relaxed);
        cx.privacy = new;
        cx.ver += 1;
    }

    /// Hides the shell while a game (any full-screen app) is in front, and restores it after.
    fn game_check(&mut self) {
        // With a second monitor only the bar on the covered one hides (bars_for_fullscreen); the other keeps working.
        let want = self.cx.cfg.bool("autoGameMode") && buddy::fullscreen_monitor().is_some() && util::monitors().0.len() < 2;
        let (flip, votes) = game_vote(self.game.is_some(), want, self.game_votes);
        self.game_votes = votes;
        if !flip { return; }
        if want { self.suspend() } else { self.resume() }
    }

    fn suspend(&mut self) {
        let names: Vec<String> = self.pops.iter().map(|p| p.name.clone()).collect();
        for n in names { self.close(&n); }
        (self.menu, self.popup, self.pending, self.shown) = (None, None, None, None);
        let hid = self.bars.iter().chain(self.wins.iter()).filter(|w| w.visible()).map(|w| { w.hide(); w.hwnd }).collect();
        self.game = Some(hid);
        unsafe { let _ = KillTimer(self.msg, FRAME); }
        self.frame = 0;
    }

    fn resume(&mut self) {
        let hid = self.game.take().unwrap_or_default();
        for w in self.bars.iter().chain(self.wins.iter()).filter(|w| hid.contains(&w.hwnd)) { w.show(); }
        self.cx.ver += 1;
        self.frames();
    }

    /// Setting "bar.hideFullscreen": a bar hides while an app covers its monitor and comes back after.
    fn bars_for_fullscreen(&mut self) {
        let fs = if self.cx.cfg.bool("bar.hideFullscreen") || self.cx.cfg.bool("autoGameMode") { buddy::fullscreen_monitor() } else { None };
        for b in &self.bars {
            let hide = fs.is_some_and(|m| on_monitor(m, b.rect()));
            if hide == b.visible() { if hide { b.hide() } else { b.show() } }
        }
    }

    fn tick(&mut self) {
        self.game_check();
        if self.game.is_some() { return; }
        self.bars_for_fullscreen();
        self.ticks += 1;
        let n = self.ticks;
        if n % 60 == 0 {
            crate::glass::trim_idle(std::time::Duration::from_secs(30));
            // hand freed pages back to Windows, so Task Manager shows what is really in use
            unsafe { let _ = windows::Win32::System::Threading::SetProcessWorkingSetSize(windows::Win32::System::Threading::GetCurrentProcess(), usize::MAX, usize::MAX); }
        }
        if n % 30 == 0 { crate::appicon::add(self.msg); } // Explorer forgets it when it restarts
        // hook up coding agents found on the PC: at start (after 2 s), then every 5 minutes for ones installed later
        if (n == 2 || n % 300 == 0) && self.cx.cfg.get("autoTrackAgents").as_bool() != Some(false) {
            let custom = self.cx.cfg.get("customAgents").clone();
            std::thread::spawn(move || {
                let added = crate::bridge::agent_hook::auto_install(&custom);
                if added.is_empty() { return; }
                let text = format!("Now tracking {}", added.join(", "));
                post(Box::new(move |cx| cx.notif = Some(Notif { app: "Zor UI".into(), text, until: hub::now_ms() + 6000.0, open: String::new() })));
            });
        }
        if n % 2 == 0 && self.reload() {
            desk::sync(self);
            buddy::sync(self);
            pill::sync(self);
            dock::sync(self);
            frame::sync(self);
            guides::sync(self);
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
            self.privacy_check();
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

    /// Window `i` of the bars, then the other windows (the dock), then the open popups: `popup_check`'s index.
    fn hover_win(&self, i: usize) -> Option<&Win> {
        let j = i.checked_sub(self.bars.len());
        self.bars.get(i).or_else(|| self.wins.get(j?)).or_else(|| self.pops.get(j?.checked_sub(self.wins.len())?).map(|p| &p.win))
    }

    /// Closes the tray flyout on a mouse press anywhere but on it. Focus loss can't do this: the app's
    /// menu or window an icon opens takes focus too.
    fn outside_check(&mut self) {
        let Some(p) = self.pops.iter().find(|p| p.name == TRAY_FLYOUT) else { unsafe { let _ = KillTimer(self.msg, OUTSIDE); } return };
        let down = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|k| unsafe { GetAsyncKeyState(k.0 as i32) } as u16 & 0x8000 != 0);
        let mut pt = POINT::default();
        unsafe { let _ = GetCursorPos(&mut pt); }
        if down && hub::now_ms() - p.armed_at > 300.0 && !contains(&p.win.rect(), pt) { self.close(TRAY_FLYOUT); }
    }

    fn popup_check(&mut self, bi: usize) {
        let hp = self.hover_win(bi).and_then(|b| b.view.hover_popup());
        match hp {
            Some((id, r, tip)) => {
                if self.shown == Some((bi, id)) || self.pending.as_ref().map(|p| (p.0, p.1)) == Some((bi, id)) { return; }
                self.hide_popup();
                self.pending = Some((bi, id, r, tip));
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
        let Some((bi, id, r, tip)) = self.pending.take() else { return };
        let Some(bar) = self.hover_win(bi) else { return };
        if bar.view.hover_popup().map(|p| p.0) != Some(id) { return; }
        let s = bar.scale();
        let origin = bar.pos();
        let bar_size = bar.size_px();
        let edge = bar.view.popup_edge().unwrap_or(self.cx.bar_edge());
        if self.popup.is_none() { self.popup = Win::new(Box::new(Popup { id, tip: tip.clone() }), "Zor UI / bar-popup", -32000, -32000, Z::Top, true).ok(); }
        let Some(p) = self.popup.as_mut() else { return };
        p.view = Box::new(Popup { id, tip });
        p.update(&self.g, &self.cx, true);
        let (x, y) = bar::popup_spot(edge, origin, bar_size, r, s, p.size_px());
        p.move_to(x, y);
        p.show();
        p.raise(); // a focused popup (the tray flyout) is topmost too and would hide it
        self.shown = Some((bi, id));
    }

    // ---- popups

    /// Opens the popup `name` natively open again closes it.
    pub fn open(&mut self, name: &str, mon: Option<(i32, i32)>, at: Option<(i32, i32)>) {
        // "keybinds" is the Settings window on its Keybinds page
        let is_settings = name == "settings" || name == "keybinds";
        if self.pops.iter().any(|p| p.name == name) {
            // a desk card asking Settings for a section while it's open: go there instead of closing it
            if !(is_settings && hub::get(crate::settings::GOTO).is_some()) { self.close(name); }
            return;
        }
        // the click or key that just closed it (focus loss) must not reopen it
        let closed: f64 = hub::get(&format!("zor-ui.closedAt.{name}")).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        if hub::now_ms() - closed < 600.0 { return; }
        self.start_bar_sources();
        let (mons, _) = util::monitors();
        let pt = mon.or_else(|| self.cx.wm.monitors.iter().find(|m| m.focus).map(|m| (m.x, m.y))).unwrap_or((0, 0));
        let Some(rect) = mons.iter().copied().find(|m| pt.0 >= m.left && pt.0 < m.right && pt.1 >= m.top && pt.1 < m.bottom).or(mons.first().copied()) else { return };
        let s = dpi_scale(rect);
        let area = popup::area(name, (rect.right - rect.left) as f32 / s, (rect.bottom - rect.top) as f32 / s, self.cx.bar_edge());
        let Some(view) = popup::make(name, area) else { return };
        let Ok(mut w) = Win::new(view, &format!("Zor UI / {name}"), rect.left, rect.top, if is_settings { Z::Normal } else { Z::Top }, false) else { return };
        w.set_focusable(true);
        if name == "island-panel" { w.grow = Some((std::time::Instant::now(), self.cx.bar_edge() == bar::Edge::Bottom)); }
        w.update(&self.g, &self.cx, true);
        let px = w.size_px();
        let (x, y) = popup::spot(name, rect, s, px, at, self.cx.bar_edge());
        w.move_to(x, y);
        w.show();
        w.focus();
        w.view.input(Input::Focus(true), &mut self.cx);
        w.update(&self.g, &self.cx, true);
        self.pops.push(Pop { name: name.into(), win: w, mon: rect, at, armed_at: hub::now_ms(), pinned: is_settings || name == TRAY_FLYOUT, px });
        if name == TRAY_FLYOUT { unsafe { SetTimer(self.msg, OUTSIDE, 40, None); } }
        self.frames();
    }

    pub fn close(&mut self, name: &str) {
        if let Some(i) = self.pops.iter().position(|p| p.name == name) {
            self.pops.remove(i);
            self.hide_popup(); // its tooltip goes with it
            crate::gfx::drop_backdrops();
            hub::set(&format!("zor-ui.closedAt.{name}"), Some(hub::now_ms().to_string()));
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
                    Action::Menu(items, at) => self.show_menu(items, at),
                    Action::CloseMenu => { self.menu = None; self.frames(); }
                    Action::Command(t) => self.command(&t),
                }
            }
            self.cx.ver += 1;
            self.paint_pops(true);
        }
    }

    fn show_menu(&mut self, items: Vec<menu::Item>, (px, py): (i32, i32)) {
        let (mons, _) = util::monitors();
        let Some(m) = mons.iter().copied().find(|m| px >= m.left && px < m.right && py >= m.top && py < m.bottom).or(mons.first().copied()) else { return };
        let Ok(mut w) = Win::new(Box::new(Menu::new(items)), "Zor UI / menu", px, py, Z::Top, false) else { return };
        w.update(&self.g, &self.cx, true);
        // the 12 px shadow margin sits around the card, so the card's corner lands on the pointer
        let (cw, ch) = w.size_px();
        let pad = (12.0 * w.scale()) as i32;
        let (x, y) = ((px - pad).min(m.right - cw).max(m.left), (py - pad).min(m.bottom - ch).max(m.top));
        w.move_to(x, y);
        w.show();
        self.menu = Some((w, hub::now_ms()));
        self.frames();
    }

    /// The menu never has focus, so a click anywhere outside it, or Esc, is polled for here.
    fn menu_check(&mut self) {
        let Some((w, t)) = &self.menu else { return };
        // bit 0 is "pressed since the last call": a click shorter than the 16 ms poll still counts
        let down = |k: VIRTUAL_KEY| unsafe { GetAsyncKeyState(k.0 as i32) as u16 & 0x8001 != 0 };
        let mut p = POINT::default();
        unsafe { let _ = GetCursorPos(&mut p); }
        let r = w.rect();
        let outside = !(p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom);
        let (esc, click) = (down(VK_ESCAPE), down(VK_LBUTTON) | down(VK_RBUTTON) | down(VK_MBUTTON)); // read every poll to reset bit 0
        if esc || (hub::now_ms() - t > 200.0 && outside && click) {
            self.menu = None;
            self.frames();
        }
    }

    /// A message from `zorui.exe --open <name>`.
    pub fn command(&mut self, text: &str) {
        if text == "buddy listen" || text.starts_with("buddy ask ") {
            if !assist::enabled(&self.cx.cfg) { assist::say_later("Voice commands are off. Turn them on in Settings, Voice.", 5.0); return; }
            if text == "buddy listen" && pill::active() { if !assist::press_again(&self.cx.cfg) { assist::cancel(); } return; } // the hotkey toggles
            match text.strip_prefix("buddy ask ") { Some(said) => assist::ask(&self.cx.cfg, said), None => { pill::open(self); assist::listen(&self.cx.cfg) } }
            return;
        }
        if text == "restart" { glaze::command("wm-reload-config"); restart(); return; }
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
            _ => if !self.keys(None, Some(pi), msg, wp) {
                self.pops[pi].win.mouse(msg, wp, lp, &mut self.cx);
                // popups with tooltips (the tray flyout) ask for them like the bar does
                if !matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN) { self.popup_check(self.bars.len() + self.wins.len() + pi); }
            },
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
                (WM_TIMER, GUARD) => if let Some(front) = tray::guard_standin() { crate::log::line(&format!("tray stand-in window was shown; hid it (window in front: {front:?})")); },
                (WM_TIMER, FRAME) => {
                    if self.cx.theme.advance() { self.cx.ver += 1; }
                    // each view paints at its own rate: a tile that wants 33 ms frames must not drag the
                    // bar, the desk cards and the island down to 30 fps with it
                    let now = std::time::Instant::now();
                    for w in self.bars.iter_mut().chain(self.wins.iter_mut()).chain(self.pops.iter_mut().map(|p| &mut p.win)) {
                        let ms = if w.gliding { 16 } else { w.view.frame_ms() };
                        if ms > 0 && w.due(now, ms) { w.update(&self.g, &self.cx, true); }
                        else { w.update(&self.g, &self.cx, false); }
                    }
                    self.menu_check();
                    self.frames();
                }
                (WM_TIMER, HOVER) => self.show_popup(),
                (WM_TIMER, OUTSIDE) => self.outside_check(),
                (crate::appicon::CALLBACK, _) => match lp.0 as u32 & 0xFFFF {
                    WM_LBUTTONUP => self.open("settings", None, None),
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        let mut p = POINT::default();
                        unsafe { let _ = GetCursorPos(&mut p); }
                        let hwnd = self.msg;
                        self.show_menu(vec![
                            menu::Item::new("settings", "Settings", |cx| cx.actions.push(Action::open("settings"))),
                            menu::Item::new("visibility", "Recover hidden windows", |_| { std::thread::spawn(|| { crate::log::line(&format!("rescue: uncloaked {} windows", crate::rescue::uncloak_all())); }); }),
                            menu::Item::new("restart_alt", "Restart", |_| restart()),
                            menu::Item::new("close", "Quit", move |_| { crate::appicon::remove(hwnd); crate::voice::shutdown(); std::process::exit(0); }),
                        ], (p.x, p.y));
                    }
                    _ => {}
                },
                (WM_APP, _) => {
                    while let Ok(job) = self.jobs.try_recv() { job(&mut self.cx); }
                    driveframe::sync(self); // the border while the buddy drives
                    self.cx.ver += 1;
                    // GlazeWM moved focus to another window: popups opened by a shortcut may never get a WM_ACTIVATE to lose
                    let focus = self.cx.wm.windows.iter().find(|w| w.focus).map(|w| w.id.clone());
                    if focus != self.last_focus {
                        self.last_focus = focus;
                        self.menu = None;
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
            crate::log::line(&format!("display changed: {} bars", self.bars.len()));
            // Bars stay up unless their monitor spots moved: a rebuild is what flickered on each fullscreen switch.
            self.pops.clear();
            self.place_bars(false);
            desk::sync(self);
            return;
        }
        if let Some((w, _)) = self.menu.as_mut().filter(|m| m.0.hwnd == hwnd) {
            w.mouse(msg, wp, lp, &mut self.cx);
            w.update(&self.g, &self.cx, false);
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
                let click = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN);
                if click { self.hide_popup(); }
                if self.wins[i].mouse(msg, wp, lp, &mut self.cx) {
                    if let Some(k) = self.wins[i].drag_key.clone() { let p = self.wins[i].pos(); desk::save_pos(&mut self.cx, &k, p); }
                }
                if !click { self.popup_check(self.bars.len() + i); }
                if self.wins[i].drag_key.is_some() { guides::refresh(self); }
                self.wins[i].update(&self.g, &self.cx, false);
                self.frames(); // a wheel may have started a scroll glide
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
/// PowerShell command that starts `exe` again after the old process has gone.
fn restart_script(exe: &str) -> String { format!("Start-Sleep 1; Start-Process -FilePath '{}' -WindowStyle Hidden", exe.replace('\'', "''")) }

/// True when the bars on screen (`have`: their positions) are the ones wanted (`want`) on the same edge.
fn bars_current(have: &[(i32, i32)], want: &[(i32, i32)], same_edge: bool) -> bool {
    same_edge && have.len() == want.len() && want.iter().all(|w| have.contains(w))
}

/// Starts a new zorui and exits this one (once, however many messages ask).
#[track_caller]
fn restart() {
    use std::{os::windows::process::CommandExt, sync::atomic::AtomicBool};
    static GOING: AtomicBool = AtomicBool::new(false);
    if GOING.swap(true, Ordering::SeqCst) { return; }
    crate::log::line(&format!("restarting, requested at {}", std::panic::Location::caller()));
    let Ok(exe) = std::env::current_exe() else { GOING.store(false, Ordering::SeqCst); return };
    let spawned = std::process::Command::new("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &restart_script(&exe.to_string_lossy())])
        .creation_flags(0x0800_0000).spawn();
    if spawned.is_ok() {
        crate::appicon::remove(HWND(MSG_HWND.load(Ordering::Relaxed) as *mut _));
        crate::voice::shutdown();
        // not process::exit: its cleanup drops the windows, whose handlers panic on the already destroyed thread-locals (a crash report each restart)
        unsafe { let _ = windows::Win32::System::Threading::TerminateProcess(windows::Win32::System::Threading::GetCurrentProcess(), 0); }
        std::process::exit(0);
    }
    GOING.store(false, Ordering::SeqCst);
}

/// True if the middle of `w` is on monitor `m`.
fn on_monitor(m: RECT, w: RECT) -> bool {
    let (x, y) = ((w.left + w.right) / 2, (w.top + w.bottom) / 2);
    x >= m.left && x < m.right && y >= m.top && y < m.bottom
}

pub fn dpi_scale(r: RECT) -> f32 {
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
    let ours = |m: u32| MOUSE.contains(&m) || matches!(m, WM_TIMER | WM_APP | win::WM_APPBAR | crate::appicon::CALLBACK | WM_KEYDOWN | WM_CHAR);
    if ours(msg) || matches!(msg, WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SYSKEYDOWN | WM_ACTIVATE) {
        // try_borrow: a call made while handling a message (SetWindowPos, DestroyWindow) can re-enter here
        APP.with(|a| if let Ok(mut a) = a.try_borrow_mut() { if let Some(app) = a.as_mut() { app.on(hwnd, msg, wp, lp); } });
        if ours(msg) { return LRESULT(0); }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

#[cfg(test)]
mod tests {
    #[test]
    fn click_inside_or_outside_a_window() {
        use windows::Win32::Foundation::{POINT, RECT};
        let r = RECT { left: -100, top: 10, right: 50, bottom: 60 };
        let at = |x, y| super::contains(&r, POINT { x, y });
        assert!(at(-100, 10) && at(49, 59)); // edges inside
        assert!(!at(50, 30) && !at(0, 60) && !at(-101, 30) && !at(0, 9)); // right and bottom edges are outside
    }

    #[test]
    fn banner_click_target() {
        let n = |until: f64, open: &str| Some(super::Notif { app: "a".into(), text: "t".into(), until, open: open.into() });
        assert_eq!(super::Notif::target(&n(5000.0, "ms-phone:x"), 1000.0), Some("ms-phone:x"));
        assert_eq!(super::Notif::target(&n(500.0, "ms-phone:x"), 1000.0), None); // expired
        assert_eq!(super::Notif::target(&n(5000.0, ""), 1000.0), None); // our own banner
        assert_eq!(super::Notif::target(&None, 1000.0), None);
    }

    #[test]
    fn game_mode_needs_two_agreeing_ticks() {
        assert_eq!(super::game_vote(false, true, 0), (false, 1));
        assert_eq!(super::game_vote(false, true, 1), (true, 0));
        assert_eq!(super::game_vote(false, true, 1).0 && super::game_vote(false, false, 1) == (false, 0), true, "a flicker resets the count");
        assert_eq!(super::game_vote(true, false, 1), (true, 0));
        assert_eq!(super::game_vote(true, true, 1), (false, 0));
    }

    #[test]
    fn a_bar_belongs_to_the_monitor_it_sits_on() {
        let r = |l, t, rt, b| windows::Win32::Foundation::RECT { left: l, top: t, right: rt, bottom: b };
        let (left, right) = (r(-1920, 0, 0, 1080), r(0, 0, 1080, 1920));
        assert!(super::on_monitor(left, r(-1920, 1024, 0, 1080)));
        assert!(!super::on_monitor(right, r(-1920, 1024, 0, 1080)));
        assert!(super::on_monitor(right, r(1012, 0, 1080, 1920)));
    }

    #[test]
    fn unchanged_bars_do_not_restart_the_app() {
        let at = [(0, 0), (1920, 0)];
        assert!(super::bars_current(&at, &[(1920, 0), (0, 0)], true), "same bars, any order");
        assert!(!super::bars_current(&at, &at, false), "a moved edge needs new bars");
        assert!(!super::bars_current(&at, &[(0, 0)], true), "a monitor left");
        assert!(!super::bars_current(&[(0, 0)], &at, true), "a monitor arrived");
        assert!(!super::bars_current(&at, &[(0, 0), (3840, 0)], true), "a monitor moved");
    }

    #[test]
    fn restart_command_quotes_the_path() {
        assert_eq!(super::restart_script(r"C:\Users\O'Neil\zorui.exe"), r"Start-Sleep 1; Start-Process -FilePath 'C:\Users\O''Neil\zorui.exe' -WindowStyle Hidden");
    }
}
