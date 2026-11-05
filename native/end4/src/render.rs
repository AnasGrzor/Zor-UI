//! `zorui.exe --render <part> <out.png> [w h scale]`: paints one view offscreen with the real config and
//! theme and sample media, and writes a PNG, so drawing code can be checked without windows.
//! Parts: a desk card id ("clock", "weather"...), "bar", "dock", "visualizer", "gojo", or a popup name.
//! w, h are CSS px (default: the view's own size, or the popup preset on a 1920x1080 screen); scale defaults to 1.

use crate::{
    app::Ctx,
    bar::Bar,
    conf::{self, Config, Theme},
    desk::Card,
    dock::Dock,
    gfx::{Canvas, Gfx},
    gojo,
    media::{Audio, Media},
    popup,
    viz::Viz,
    win::View,
};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::GENERIC_WRITE,
        Graphics::Imaging::*,
    },
};

const FLAT: [u8; 3] = [0x3C, 0x38, 0x44]; // transparent pixels show as this, in the PNG

pub fn run(args: &[String]) {
    let (Some(part), Some(out)) = (args.first(), args.get(1)) else { return eprintln_ui("usage: zorui.exe --render <part> <out.png> [w h scale]") };
    let num = |i: usize| args.get(i).and_then(|a| a.parse::<f32>().ok());
    let pack = conf::pack_dir();
    let Ok(g) = Gfx::new(&pack.join(r"assets\fonts"), &conf::data_dir()) else { return };
    let mut cx = Ctx::new(true);
    cx.cfg = Config::parse(&std::fs::read_to_string(pack.join("config.json")).unwrap_or_default());
    cx.theme = Theme::parse(&std::fs::read_to_string(pack.join("theme.json")).unwrap_or_default());
    g.set_font(cx.cfg.str("font.family"), cx.cfg.get("font.size").as_f64().unwrap_or(1.0) as f32, cx.cfg.get("font.weight").as_i64().unwrap_or(0) as i32);
    sample(&mut cx);
    let scale = num(4).unwrap_or(1.0);
    let area = match (num(2), num(3)) { (Some(w), Some(h)) => (w, h), _ => popup::area(part, 1920.0, 1080.0, crate::bar::Edge::Top) };
    let mon = (0, 0, (1920.0 * scale) as i32);
    let mut view: Box<dyn View> = match part.as_str() {
        // END4_BAR_EDGE=left|right|bottom previews a bar on another edge (a 1080 px tall monitor)
        "bar" => Box::new(Bar::new((0, 0, mon.2, (1080.0 * scale) as i32), crate::bar::Edge::parse(&std::env::var("END4_BAR_EDGE").unwrap_or_default()), (0, 0))),
        "dock" => Box::new(Dock::new([0, 0, mon.2, (1080.0 * scale) as i32], cx.cfg.str("dock.side") == "right")),
        "visualizer" => Box::new(Viz::new(mon)),
        "gojo" => Box::new(gojo::Gojo::new()),
        "pill" => Box::new(crate::pill::Pill::demo()),
        p => popup::make(p, area).unwrap_or_else(|| Box::new(Card::new(p))),
    };
    let (w, h) = if num(2).is_some() && num(3).is_some() { area } else { view.size(&g, &cx, scale) };
    let (pw, ph) = ((w * scale).ceil() as u32, (h * scale).ceil() as u32);
    let mut px = vec![0u8; (pw * ph * 4) as usize];
    {
        let Some(mut c) = Canvas::new(&g, &mut px, pw, ph, scale, 0) else { return };
        view.paint(&mut c, &cx);
        if std::env::var("END4_BENCH").is_ok() { // TEMP
            let t = std::time::Instant::now();
            for _ in 0..20 { view.paint(&mut c, &cx); }
            eprintln!("{part} paint avg {:?}", t.elapsed() / 20);
        }
    }
    // premultiplied BGRA over a flat backdrop -> opaque BGRA
    for p in px.chunks_exact_mut(4) {
        let inv = 255 - p[3] as u32;
        for k in 0..3 { p[k] = (p[k] as u32 + FLAT[2 - k] as u32 * inv / 255).min(255) as u8; }
        p[3] = 255;
    }
    if write_png(&g, out, &px, pw, ph).is_none() { eprintln_ui("could not write the PNG"); }
}

/// What a typical session looks like, so media views have something to draw.
fn sample(cx: &mut Ctx) {
    let m = Media { id: "Spotify.exe".into(), title: "Sample Song".into(), artist: "Sample Artist".into(), album: "Sample Album".into(), playing: true,
        can_prev: true, can_next: true, current: true, duration: 214.0, position: 73.0, at: crate::hub::now_ms(), snap: (73.0, 0.0), art: None };
    cx.media = Some(m.clone());
    cx.sessions = vec![m];
    cx.audio = Some(Audio { volume: 62.0, muted: false, mic_muted: Some(false), mic_volume: Some(80.0) });
    (cx.cpu, cx.mem, cx.disk) = (0.34, 0.58, 0.71);
}

fn eprintln_ui(msg: &str) { eprintln!("{msg}"); }

fn write_png(g: &Gfx, path: &str, bgra: &[u8], w: u32, h: u32) -> Option<()> {
    unsafe {
        let enc = g.wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).ok()?;
        let stream = g.wic.CreateStream().ok()?;
        stream.InitializeFromFilename(&HSTRING::from(path), GENERIC_WRITE.0).ok()?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
        let (mut frame, mut props) = (None, None);
        enc.CreateNewFrame(&mut frame, &mut props).ok()?;
        let frame = frame?;
        frame.Initialize(props.as_ref()).ok()?;
        frame.SetSize(w, h).ok()?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt).ok()?;
        frame.WritePixels(h, w * 4, bgra).ok()?;
        frame.Commit().ok()?;
        enc.Commit().ok()
    }
}
