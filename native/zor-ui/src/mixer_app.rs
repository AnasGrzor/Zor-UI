//! The playing app's volume in Windows' mixer. The desk's now-playing card and the island both use it: one poll of
//! the mixer (every 4 s while a user lives), and one background thread that sends the changes, one bridge call per
//! pause in a drag.

use crate::{
    app::{self, Ctx},
    conf::Config,
    proc,
};
use std::sync::{mpsc, Arc, Mutex};

/// (app name, volume 0-100, muted) per app in Windows' mixer.
pub type AppVols = Vec<(String, f32, bool)>;

pub enum Cmd { Volume(String, f32), Mute(String, bool) }

pub fn parse_mixer(v: &serde_json::Value) -> AppVols {
    v["apps"].as_array().map(|a| a.iter().filter_map(|x| Some((x["app"].as_str()?.to_string(), x["volume"].as_f64()? as f32, x["muted"].as_bool().unwrap_or(false)))).collect()).unwrap_or_default()
}

/// Collapses each run of Volume commands for one app to the last, so a drag makes one bridge call.
pub fn coalesce(cmds: Vec<Cmd>) -> Vec<Cmd> {
    let mut out: Vec<Cmd> = vec![];
    for c in cmds {
        if let (Some(Cmd::Volume(a, _)), Cmd::Volume(b, _)) = (out.last(), &c) { if a == b { out.pop(); } }
        out.push(c);
    }
    out
}

/// sessionApp() in desktop.js: "Spotify.exe" / "pkg!App" -> "spotify" / "app".
pub fn session_app(id: &str) -> String {
    let n = id.rsplit(['\\', '!']).next().unwrap_or("");
    n.strip_suffix(".exe").or_else(|| n.strip_suffix(".EXE")).unwrap_or(n).to_lowercase()
}

/// The mixer list as last polled, and the thread that changes it.
#[derive(Default)]
pub struct AppMix {
    apps: Arc<Mutex<AppVols>>,
    polling: bool,
    tx: Option<mpsc::Sender<Cmd>>,
}

impl AppMix {
    /// Starts the 4 s poll on the first call; later calls do nothing.
    pub fn poll(&mut self, cx: &Ctx) {
        if self.polling { return; }
        self.polling = true;
        let (cfg, apps) = (Config(cx.cfg.0.clone()), Arc::downgrade(&self.apps));
        std::thread::spawn(move || loop {
            let Some(a) = apps.upgrade() else { return };
            let list = parse_mixer(&proc::bridge(&cfg, "mixer", &[]));
            let changed = a.lock().is_ok_and(|mut g| if *g != list { *g = list; true } else { false });
            drop(a);
            if changed { app::post(Box::new(|_| {})); }
            std::thread::sleep(std::time::Duration::from_secs(4));
        });
    }

    /// The media's app in the mixer: (name, volume 0-100, muted). None when it isn't playing audio.
    pub fn app(&self, cx: &Ctx) -> Option<(String, f32, bool)> {
        let want = cx.media.as_ref().filter(|m| !m.title.is_empty()).map(|m| session_app(&m.id)).filter(|w| !w.is_empty())?;
        self.apps.lock().ok()?.iter().find(|a| a.0.to_lowercase() == want).cloned()
    }

    /// The level the media's controls show: (volume 0-100, muted, is the app's own). Falls back to the master volume.
    pub fn level(&self, cx: &Ctx) -> Option<(f32, bool, bool)> {
        if let Some((_, v, m)) = self.app(cx) { return Some((v, m, true)); }
        cx.audio.map(|a| (a.volume, a.muted, false))
    }

    /// Sets the cached volume and/or mute of `app` at once, so the slider doesn't wait for the next poll.
    pub fn cache(&mut self, app: &str, volume: Option<f32>, muted: Option<bool>) {
        if let Ok(mut l) = self.apps.lock() {
            if let Some(a) = l.iter_mut().find(|a| a.0 == app) {
                if let Some(v) = volume { a.1 = v; }
                if let Some(m) = muted { a.2 = m; }
            }
        }
    }

    /// Whether `app` is muted in the last poll (or the cache).
    pub fn muted(&self, app: &str) -> bool {
        self.apps.lock().ok().and_then(|l| l.iter().find(|a| a.0 == app).map(|a| a.2)).unwrap_or(false)
    }

    /// Sends a change to the mixer, off the UI thread.
    pub fn send(&mut self, cx: &Ctx, c: Cmd) {
        let tx = self.tx.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<Cmd>();
            let cfg = Config(cx.cfg.0.clone());
            std::thread::spawn(move || {
                while let Ok(first) = rx.recv() {
                    std::thread::sleep(std::time::Duration::from_millis(150)); // one bridge call per pause in a drag
                    let mut all = vec![first];
                    while let Ok(n) = rx.try_recv() { all.push(n); }
                    for cmd in coalesce(all) {
                        match cmd {
                            Cmd::Volume(a, v) => proc::bridge(&cfg, "mixer", &["volume", &a, &v.to_string()]),
                            Cmd::Mute(a, m) => proc::bridge(&cfg, "mixer", &["mute", &a, if m { "1" } else { "0" }]),
                        };
                    }
                }
            });
            tx
        });
        let _ = tx.send(c);
    }

    /// Sets the media's volume (0-100): the app's own when it's in the mixer, else the master. Unmutes the app first if needed.
    pub fn set_volume(&mut self, cx: &Ctx, v: f32) {
        match self.app(cx) {
            None => {
                crate::media::set_volume(v);
                if cx.audio.is_some_and(|a| a.muted) { crate::media::set_mute(false); }
            }
            Some((app, _, _)) => {
                let unmute = self.muted(&app);
                self.cache(&app, Some(v), Some(false));
                if unmute { self.send(cx, Cmd::Mute(app.clone(), false)); }
                self.send(cx, Cmd::Volume(app, v));
            }
        }
    }
}
