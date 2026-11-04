//! Now playing: Windows' media sessions (the same source as Zebar's media provider) and the
//! default audio devices' volume. Each runs on its own COM thread and reports through `on`.

use std::{
    collections::HashMap,
    sync::{mpsc, Arc, Mutex, OnceLock},
    time::Duration,
};
use windows::{
    core::implement,
    Foundation::TypedEventHandler,
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    },
    Storage::Streams::DataReader,
    Win32::{
        Media::Audio::{Endpoints::*, *},
        System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED},
    },
};

/// One media session (a player app). `Ctx.media` is the current one, `Ctx.sessions` all of them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    /// The player's AppUserModelId ("Spotify.exe", "chrome.exe"); `bar::pretty_app` makes it readable.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub can_prev: bool,
    pub can_next: bool,
    /// Windows' current session (the one the bar shows).
    pub current: bool,
    /// Track length and position in seconds, and when (unix ms) Windows took that position.
    pub duration: f64,
    pub position: f64,
    pub at: f64,
    /// Thumbnail file bytes; decoded on the UI thread.
    pub art: Option<Arc<Vec<u8>>>,
}

impl Media {
    /// Position now, moving on from Windows' last snapshot while playing.
    pub fn now(&self) -> f64 {
        let p = if self.playing { self.position + (crate::hub::now_ms() - self.at) / 1000.0 } else { self.position };
        if self.duration > 0.0 { p.clamp(0.0, self.duration) } else { p.max(0.0) }
    }
    pub fn progress(&self) -> f32 { if self.duration > 0.0 { (self.now() / self.duration) as f32 } else { 0.0 } }
}

enum Msg { Changed, Control(Option<String>, &'static str), Seek(String, f64) }
static MEDIA: OnceLock<Mutex<mpsc::Sender<Msg>>> = OnceLock::new();

fn send(m: Msg) { if let Some(Ok(t)) = MEDIA.get().map(|m| m.lock()) { let _ = t.send(m); } }

/// "togglePlayPause" | "play" | "pause" | "next" | "previous" on the current session, like Zebar's media provider methods.
pub fn control(what: &'static str) { send(Msg::Control(None, what)); }
/// The same on the session `id` (`Media::id`).
pub fn control_of(id: &str, what: &'static str) { send(Msg::Control(Some(id.into()), what)); }
/// Jumps session `id` to `secs`.
pub fn seek(id: &str, secs: f64) { send(Msg::Seek(id.into(), secs)); }

/// DateTime (100 ns since 1601) to unix ms.
fn unix_ms(t: i64) -> f64 { (t - 116_444_736_000_000_000) as f64 / 10_000.0 }

type ArtCache = HashMap<String, (String, Option<Arc<Vec<u8>>>)>;

fn read(s: &Session, arts: &mut ArtCache, current: bool) -> Option<Media> {
    let id = s.SourceAppUserModelId().ok()?.to_string();
    let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = props.Title().ok()?.to_string();
    if title.is_empty() { return None; }
    let artist = props.Artist().map(|a| a.to_string()).unwrap_or_default();
    let key = format!("{title}|{artist}");
    let e = arts.entry(id.clone()).or_default();
    if e.0 != key {
        e.0 = key;
        e.1 = (|| {
            let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.get().ok()?;
            let size = stream.Size().ok()? as u32;
            let r = DataReader::CreateDataReader(&stream).ok()?;
            r.LoadAsync(size).ok()?.get().ok()?;
            let mut buf = vec![0u8; size as usize];
            r.ReadBytes(&mut buf).ok()?;
            Some(Arc::new(buf))
        })();
    }
    let info = s.GetPlaybackInfo().ok()?;
    let playing = info.PlaybackStatus().ok()? == Status::Playing;
    let ctl = info.Controls().ok();
    let tl = s.GetTimelineProperties().ok()?;
    let secs = |t: windows::Foundation::TimeSpan| t.Duration as f64 / 1e7;
    Some(Media {
        id, title, artist, playing, current,
        album: props.AlbumTitle().map(|a| a.to_string()).unwrap_or_default(),
        can_prev: ctl.as_ref().and_then(|c| c.IsPreviousEnabled().ok()).unwrap_or(true),
        can_next: ctl.as_ref().and_then(|c| c.IsNextEnabled().ok()).unwrap_or(true),
        duration: (secs(tl.EndTime().ok()?) - secs(tl.StartTime().ok()?)).max(0.0),
        position: secs(tl.Position().ok()?),
        at: unix_ms(tl.LastUpdatedTime().ok()?.UniversalTime),
        art: e.1.clone(),
    })
}

/// An event handler that wakes the media thread.
fn ping<A, B>(tx: &mpsc::Sender<Msg>) -> impl Fn(&Option<A>, &Option<B>) -> windows::core::Result<()> + Send + 'static {
    let tx = tx.clone();
    move |_, _| { let _ = tx.send(Msg::Changed); Ok(()) }
}

fn by_id<'a>(all: &'a [Session], id: &str) -> Option<&'a Session> {
    all.iter().find(|s| s.SourceAppUserModelId().is_ok_and(|x| x.to_string() == id))
}

/// `on` gets (the current session, every session) after each change.
pub fn start_media(on: impl Fn(Option<Media>, Vec<Media>) + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    let _ = MEDIA.set(Mutex::new(tx.clone()));
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(mgr) = Manager::RequestAsync().and_then(|a| a.get()) else { return };
        let _ = mgr.CurrentSessionChanged(&TypedEventHandler::new(ping(&tx)));
        let _ = mgr.SessionsChanged(&TypedEventHandler::new(ping(&tx)));
        let (mut known, mut arts, mut last) = (Vec::<Session>::new(), ArtCache::new(), None::<(Option<Media>, Vec<Media>)>);
        loop {
            let cur = mgr.GetCurrentSession().ok();
            let all: Vec<Session> = mgr.GetSessions().map(|l| l.into_iter().collect()).unwrap_or_default();
            for s in &all {
                if !known.contains(s) {
                    let _ = s.MediaPropertiesChanged(&TypedEventHandler::new(ping(&tx)));
                    let _ = s.PlaybackInfoChanged(&TypedEventHandler::new(ping(&tx)));
                    let _ = s.TimelinePropertiesChanged(&TypedEventHandler::new(ping(&tx)));
                    known.push(s.clone());
                }
            }
            known.retain(|k| all.contains(k));
            let list: Vec<Media> = all.iter().filter_map(|s| read(s, &mut arts, cur.as_ref() == Some(s))).collect();
            arts.retain(|id, _| list.iter().any(|m| &m.id == id));
            let now = (list.iter().find(|m| m.current).cloned(), list);
            if last.as_ref() != Some(&now) { on(now.0.clone(), now.1.clone()); last = Some(now); }
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Msg::Control(id, c)) => {
                    if let Some(s) = id.and_then(|id| by_id(&all, &id)).or(cur.as_ref()) {
                        let _ = match c { "next" => s.TrySkipNextAsync(), "previous" => s.TrySkipPreviousAsync(), "play" => s.TryPlayAsync(), "pause" => s.TryPauseAsync(), _ => s.TryTogglePlayPauseAsync() }.and_then(|a| a.get());
                    }
                }
                Ok(Msg::Seek(id, secs)) => {
                    if let Some(s) = by_id(&all, &id) { let _ = s.TryChangePlaybackPositionAsync((secs * 1e7) as i64).and_then(|a| a.get()); }
                }
                Ok(Msg::Changed) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return,
            }
            // coalesce bursts of events, but keep a queued control / seek
            while let Ok(m) = rx.try_recv() { if !matches!(m, Msg::Changed) { let _ = tx.send(m); break; } }
        }
    });
}

// ------------------------------------------------------------------ volume

/// Volumes are 0-100. `mic_*` is None without a capture device.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Audio { pub volume: f32, pub muted: bool, pub mic_muted: Option<bool>, pub mic_volume: Option<f32> }

enum AMsg { Changed, Step(i32), Volume(f32), Mute(bool), MicMute(bool), MicVolume(f32) }
static AUDIO: OnceLock<Mutex<mpsc::Sender<AMsg>>> = OnceLock::new();

fn asend(m: AMsg) { if let Some(Ok(t)) = AUDIO.get().map(|m| m.lock()) { let _ = t.send(m); } }

/// Volume up or down by `delta` percent (the bar's scroll wheel).
pub fn step_volume(delta: i32) { asend(AMsg::Step(delta)); }
/// Absolute output volume, 0-100.
pub fn set_volume(pct: f32) { asend(AMsg::Volume(pct)); }
pub fn set_mute(on: bool) { asend(AMsg::Mute(on)); }
pub fn set_mic_mute(on: bool) { asend(AMsg::MicMute(on)); }
pub fn set_mic_volume(pct: f32) { asend(AMsg::MicVolume(pct)); }

/// Device id (GetId allocates it; freed here, it runs every 3 s).
unsafe fn dev_id(d: &IMMDevice) -> Option<String> {
    let p = d.GetId().ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s
}

#[implement(IAudioEndpointVolumeCallback)]
struct Notify(mpsc::Sender<AMsg>);
impl IAudioEndpointVolumeCallback_Impl for Notify_Impl {
    fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> { let _ = self.0.send(AMsg::Changed); Ok(()) }
}

pub fn start_audio(on: impl Fn(Audio) + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    let _ = AUDIO.set(Mutex::new(tx.clone()));
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };
        let cb: IAudioEndpointVolumeCallback = Notify(tx).into();
        loop {
            let ep = |flow| -> Option<(String, IAudioEndpointVolume)> {
                let d = en.GetDefaultAudioEndpoint(flow, eConsole).ok()?;
                let id = dev_id(&d)?;
                let v: IAudioEndpointVolume = d.Activate(CLSCTX_ALL, None).ok()?;
                let _ = v.RegisterControlChangeNotify(&cb);
                Some((id, v))
            };
            let (out, mic) = (ep(eRender), ep(eCapture));
            let mut last = None;
            loop {
                let level = |d: &Option<(String, IAudioEndpointVolume)>| d.as_ref().and_then(|(_, v)| v.GetMasterVolumeLevelScalar().ok());
                let a = Audio {
                    volume: level(&out).unwrap_or(0.0) * 100.0,
                    muted: out.as_ref().and_then(|(_, v)| v.GetMute().ok()).is_some_and(|m| m.as_bool()),
                    mic_muted: mic.as_ref().and_then(|(_, v)| v.GetMute().ok()).map(|m| m.as_bool()),
                    mic_volume: level(&mic).map(|v| v * 100.0),
                };
                if Some(a) != last { on(a); last = Some(a); }
                let set = |d: &Option<(String, IAudioEndpointVolume)>, pct: f32| if let Some((_, v)) = d { let _ = v.SetMasterVolumeLevelScalar(pct.clamp(0.0, 100.0) / 100.0, std::ptr::null()); };
                let mute = |d: &Option<(String, IAudioEndpointVolume)>, on: bool| if let Some((_, v)) = d { let _ = v.SetMute(on, std::ptr::null()); };
                match rx.recv_timeout(Duration::from_secs(3)) {
                    Ok(AMsg::Step(d)) => set(&out, a.volume.round() + d as f32),
                    Ok(AMsg::Volume(p)) => set(&out, p),
                    Ok(AMsg::MicVolume(p)) => set(&mic, p),
                    Ok(AMsg::Mute(m)) => mute(&out, m),
                    Ok(AMsg::MicMute(m)) => mute(&mic, m),
                    Ok(AMsg::Changed) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // the default device changed (headphones plugged in): start over with the new one
                        let id = |flow| en.GetDefaultAudioEndpoint(flow, eConsole).ok().and_then(|d| dev_id(&d));
                        if id(eRender) != out.as_ref().map(|o| o.0.clone()) || id(eCapture) != mic.as_ref().map(|o| o.0.clone()) { break; }
                    }
                    Err(_) => return,
                }
            }
            for (_, v) in out.iter().chain(mic.iter()) { let _ = v.UnregisterControlChangeNotify(&cb); }
        }
    });
}
