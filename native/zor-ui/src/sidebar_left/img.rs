//! Web pictures for the anime grid and football logos: curl.exe downloads on threads (four at a time),
//! WIC decodes on the UI thread (a few per paint), cached per url and size.

use super::{sync, wake};
use crate::gfx::Gfx;
use std::{
    collections::{HashMap, HashSet},
    os::windows::process::CommandExt,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tiny_skia::Pixmap;

type Bytes = Arc<Mutex<HashMap<String, Option<Arc<Vec<u8>>>>>>;

#[derive(Default)]
pub struct Imgs {
    bytes: Bytes,
    queued: Vec<String>,
    asked: HashSet<String>,
    px: HashMap<(String, u32, u32), Option<Pixmap>>,
    inflight: Arc<AtomicUsize>,
    decoded: usize,
    deferred: bool,
}

/// GET a picture's bytes through curl.exe (same reason as util::http_get: no TLS stack here).
fn fetch(url: &str) -> Option<Vec<u8>> {
    if !crate::util::web_url(url) { return None; }
    let out = std::process::Command::new("curl.exe")
        .args(["-s", "-f", "-L", "--max-time", "25", "-A", "Mozilla/5.0", "--", url])
        .creation_flags(0x0800_0000)
        .output().ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
}

impl Imgs {
    /// Call at the start of every paint.
    pub fn begin(&mut self) {
        self.decoded = 0;
        self.deferred = false;
        while self.inflight.load(Ordering::Relaxed) < 4 && !self.queued.is_empty() {
            let url = self.queued.remove(0);
            self.inflight.fetch_add(1, Ordering::Relaxed);
            let (bytes, n) = (self.bytes.clone(), self.inflight.clone());
            super::bg(move || {
                let got = fetch(&url).map(Arc::new);
                if let Ok(mut m) = bytes.lock() { m.insert(url, got); }
                n.fetch_sub(1, Ordering::Relaxed);
                wake();
            });
        }
    }

    /// Something is still coming in: the tab should keep repainting.
    pub fn busy(&self) -> bool { self.deferred || self.inflight.load(Ordering::Relaxed) > 0 || !self.queued.is_empty() }

    /// The picture at `url`, decoded cover-cropped to `w` x `h` physical px; None until it has arrived (or if it failed).
    pub fn get(&mut self, g: &Gfx, url: &str, w: u32, h: u32) -> Option<&Pixmap> {
        let key = (url.to_string(), w, h);
        if !self.px.contains_key(&key) {
            let got = self.bytes.lock().ok().and_then(|m| m.get(url).cloned());
            match got {
                Some(Some(b)) if self.decoded < 4 || sync() => { self.decoded += 1; self.px.insert(key.clone(), g.load_image_bytes(&b, w, h)); }
                Some(Some(_)) => self.deferred = true,
                Some(None) => { self.px.insert(key.clone(), None); }
                None => {
                    if self.asked.insert(url.to_string()) { self.queued.push(url.to_string()); self.begin_queue(); }
                    return None;
                }
            }
        }
        self.px.get(&key)?.as_ref()
    }

    fn begin_queue(&mut self) { let (d, df) = (self.decoded, self.deferred); self.begin(); (self.decoded, self.deferred) = (d, df); }

    /// Forget everything (a new search).
    pub fn clear(&mut self) {
        self.queued.clear();
        self.asked.clear();
        self.px.clear();
        if let Ok(mut m) = self.bytes.lock() { m.clear(); }
    }
}
