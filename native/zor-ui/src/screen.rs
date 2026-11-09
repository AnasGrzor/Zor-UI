//! Screen help, the seeing half: grabs every monitor (the one under the pointer first), shrinks each to
//! 1280 px on its long side as a JPEG for a vision model, and turns the `[POINT:x,y:label:screenN]` tag
//! the model ends its answer with into a real screen position. Only runs when you ask a screen question
//! by voice, and only when Settings, Voice, "Let the buddy see my screen" is on. The buddy does the pointing.
//! Nothing here knows which model or provider answers: the pictures leave as plain chat message images.

use windows::{
    Graphics::Imaging::{BitmapAlphaMode, BitmapEncoder, BitmapPixelFormat, SoftwareBitmap},
    Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
        UI::WindowsAndMessaging::GetCursorPos,
    },
};

/// Longest side of the picture the model gets. Bigger only costs tokens: the model reads it at about this size anyway.
pub const MAX_SIDE: i32 = 1280;

/// One monitor as the model sees it.
pub struct Cap {
    pub jpeg: Vec<u8>,
    /// The monitor in screen pixels (x, y of its top-left corner, and its size).
    pub x: i32, pub y: i32, pub w: i32, pub h: i32,
    /// The picture's size: the coordinates the model answers in.
    pub sw: i32, pub sh: i32,
    pub cursor: bool,
}

impl Cap {
    /// Which way this monitor lies from `other`: "left of", "right of", "above" or "below", by the larger gap between
    /// their centres. Without it the model can't tell which picture "the left monitor" is.
    pub fn side_of(&self, other: &Cap) -> &'static str {
        let (dx, dy) = ((self.x * 2 + self.w) - (other.x * 2 + other.w), (self.y * 2 + self.h) - (other.y * 2 + other.h));
        if dx.abs() >= dy.abs() { if dx < 0 { "left of" } else { "right of" } } else if dy < 0 { "above" } else { "below" }
    }

    /// The text that goes with the picture, so the model knows which screen it is, where it sits compared with the
    /// pointer's screen (`first`, shown as screen 1), and how big the picture is.
    pub fn label(&self, i: usize, n: usize, first: Option<&Cap>) -> String {
        let dims = format!("(image dimensions: {}x{} pixels)", self.sw, self.sh);
        if n == 1 { format!("user's screen {dims}") }
        else if self.cursor || i == 0 { format!("screen {} of {n}, the pointer is on this screen (primary focus) {dims}", i + 1) }
        else {
            let at = first.map(|f| format!(", {} screen 1", self.side_of(f))).unwrap_or_default();
            format!("screen {} of {n}, secondary screen{at} {dims}", i + 1)
        }
    }
}

/// Picture size for a monitor of `w` x `h`: the long side shrinks to `max`, never grows.
pub fn fit(w: i32, h: i32, max: i32) -> (i32, i32) {
    let long = w.max(h);
    if long <= max || w <= 0 || h <= 0 { return (w.max(1), h.max(1)); }
    let k = max as f32 / long as f32;
    (((w as f32 * k).round() as i32).max(1), ((h as f32 * k).round() as i32).max(1))
}

/// The monitors in the order they are shown to the model: the one holding the pointer first, the rest as they were.
pub fn cursor_first(rects: &[(i32, i32, i32, i32)], cur: (i32, i32)) -> Vec<usize> {
    let has = |r: &(i32, i32, i32, i32)| cur.0 >= r.0 && cur.0 < r.0 + r.2 && cur.1 >= r.1 && cur.1 < r.1 + r.3;
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by_key(|&i| !has(&rects[i]));
    order
}

/// Every monitor as (x, y, w, h) in screen pixels.
fn monitors() -> Vec<(i32, i32, i32, i32)> {
    unsafe extern "system" fn each(_: HMONITOR, _: HDC, r: *mut RECT, data: LPARAM) -> BOOL {
        let v = &mut *(data.0 as *mut Vec<(i32, i32, i32, i32)>);
        let r = *r;
        v.push((r.left, r.top, r.right - r.left, r.bottom - r.top));
        BOOL(1)
    }
    let mut v: Vec<(i32, i32, i32, i32)> = vec![];
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut v as *mut _ as isize)); }
    v
}

/// The screen area (x, y, w, h) as a `sw` x `sh` top-down BGRA picture.
fn grab(x: i32, y: i32, w: i32, h: i32, sw: i32, sh: i32) -> Option<Vec<u8>> {
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: sw, biHeight: -sh, biPlanes: 1, biBitCount: 32, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else { ReleaseDC(None, screen); let _ = DeleteDC(dc); return None };
        let old = SelectObject(dc, bmp);
        SetStretchBltMode(dc, HALFTONE);
        let _ = SetBrushOrgEx(dc, 0, 0, None);
        let ok = StretchBlt(dc, 0, 0, sw, sh, screen, x, y, w, h, SRCCOPY).as_bool();
        let px = ok.then(|| {
            let mut v = std::slice::from_raw_parts(bits as *const u8, (sw * sh * 4) as usize).to_vec();
            v.chunks_exact_mut(4).for_each(|p| p[3] = 255);
            v
        });
        SelectObject(dc, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        px
    }
}

/// Windows' own JPEG encoder (WIC through WinRT), so no image crate is needed.
fn jpeg(px: &[u8], w: i32, h: i32) -> Result<Vec<u8>, String> {
    let e = |what: &'static str| move |er: windows::core::Error| format!("screenshot ({what}): {}", er.message());
    let writer = DataWriter::new().map_err(e("buffer"))?;
    writer.WriteBytes(px).map_err(e("buffer"))?;
    let buf = writer.DetachBuffer().map_err(e("buffer"))?;
    let bmp = SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Bgra8, w, h).map_err(e("bitmap"))?;
    // JPEG has no alpha: the encoder refuses a bitmap that has some
    let bmp = SoftwareBitmap::ConvertWithAlpha(&bmp, BitmapPixelFormat::Bgra8, BitmapAlphaMode::Ignore).map_err(e("convert"))?;
    let stream = InMemoryRandomAccessStream::new().map_err(e("stream"))?;
    let enc = BitmapEncoder::CreateAsync(BitmapEncoder::JpegEncoderId().map_err(e("encoder"))?, &stream).and_then(|o| o.get()).map_err(e("encoder"))?;
    enc.SetSoftwareBitmap(&bmp).map_err(e("encode"))?;
    enc.FlushAsync().and_then(|o| o.get()).map_err(e("encode"))?;
    let size = stream.Size().map_err(e("read"))? as u32;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(e("read"))?).map_err(e("read"))?;
    reader.LoadAsync(size).and_then(|o| o.get()).map_err(e("read"))?;
    let mut out = vec![0u8; size as usize];
    reader.ReadBytes(&mut out).map_err(e("read"))?;
    Ok(out)
}

/// Every monitor, the one under the pointer first. Call from a worker thread.
pub fn capture_all() -> Result<Vec<Cap>, String> {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let rects = monitors();
    if rects.is_empty() { return Err("No screen found.".into()); }
    let mut cur = POINT::default();
    unsafe { let _ = GetCursorPos(&mut cur); }
    let mut out = vec![];
    for (n, i) in cursor_first(&rects, (cur.x, cur.y)).into_iter().enumerate() {
        let (x, y, w, h) = rects[i];
        let (sw, sh) = fit(w, h, MAX_SIDE);
        let px = grab(x, y, w, h, sw, sh).ok_or("Couldn't copy the screen.")?;
        out.push(Cap { jpeg: jpeg(&px, sw, sh)?, x, y, w, h, sw, sh, cursor: n == 0 });
    }
    Ok(out)
}

/// Where the model said to point, in the coordinates of the picture it was shown.
#[derive(Debug, PartialEq)]
pub struct Point { pub x: f32, pub y: f32, pub label: String, pub screen: Option<usize> }

/// Splits an answer into the words to show and the `[POINT:x,y:label:screenN]` tag at its end. `[POINT:none]`,
/// no tag, or a tag that doesn't parse gives no point; a tag that is there is always cut from the words.
pub fn parse_point(reply: &str) -> (String, Option<Point>) {
    let t = reply.trim_end();
    let Some(start) = t.rfind("[POINT:").filter(|_| t.ends_with(']')) else { return (reply.trim().to_string(), None) };
    let spoken = t[..start].trim().to_string();
    let parts: Vec<&str> = t[start + 7..t.len() - 1].split(':').map(str::trim).collect();
    let screen_of = |s: &str| s.to_lowercase().strip_prefix("screen").and_then(|n| n.parse::<usize>().ok()).filter(|n| *n > 0);
    let Some((x, y)) = parts[0].split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))) else { return (spoken, None) };
    let screen = parts.iter().skip(1).rev().find_map(|p| screen_of(p));
    let label = parts.get(1).filter(|p| screen_of(p).is_none()).map(|p| p.to_string()).unwrap_or_default();
    (spoken, Some(Point { x, y, label, screen }))
}

/// The picture a point belongs to: screen N (counted as in the labels, 1 first) or the pointer's screen.
pub fn target(caps: &[Cap], screen: Option<usize>) -> Option<&Cap> {
    match screen { Some(n) => caps.get(n.checked_sub(1)?), None => caps.first() }
}

/// A point in the picture as a position on the real screen (pixels). Points outside the picture are held to its edge.
pub fn to_screen(cap: &Cap, x: f32, y: f32) -> (i32, i32) {
    let (x, y) = (x.clamp(0.0, cap.sw as f32), y.clamp(0.0, cap.sh as f32));
    (cap.x + (x * cap.w as f32 / cap.sw as f32).round() as i32, cap.y + (y * cap.h as f32 / cap.sh as f32).round() as i32)
}

/// What the model is told when it is shown the screen.
pub const SYSTEM: &str = "You are a small desktop buddy that can see the user's screen. The user talks to you by voice and your \
answer is shown in a small speech bubble, so write one or two short, plain, friendly sentences (under 150 characters): no lists, \
no markdown, no emoji. If their question is \
about what is on the screen, mention the specific thing you see. If it is not about the screen, just answer it. \
When a button, menu, field or area on the screen would help them, point at it by ending your answer with a tag \
[POINT:x,y:label] where x,y are pixel coordinates in the picture (origin top-left, x to the right, y down; each picture \
is labeled with its size) and label is 1 to 3 words naming it. When you are shown more than one screen, always end the tag \
with the number of the screen the spot is on, like [POINT:x,y:label:screen1], using the numbers in the picture labels (they \
say where each screen sits, so \"the left monitor\" is the one marked left of screen 1). If pointing would not help, end with [POINT:none].";

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(x: i32, y: i32, w: i32, h: i32, sw: i32, sh: i32, cursor: bool) -> Cap { Cap { jpeg: vec![], x, y, w, h, sw, sh, cursor } }

    #[test]
    fn pictures_shrink_to_the_long_side_and_never_grow() {
        assert_eq!(fit(2560, 1440, 1280), (1280, 720));
        assert_eq!(fit(1080, 1920, 1280), (720, 1280));
        assert_eq!(fit(1280, 800, 1280), (1280, 800));
        assert_eq!(fit(800, 600, 1280), (800, 600), "a small screen is not enlarged");
        assert_eq!(fit(3440, 1440, 1280), (1280, 536));
        assert_eq!(fit(0, 0, 1280), (1, 1), "a broken monitor size never gives a zero-size bitmap");
    }

    #[test]
    fn the_pointer_screen_comes_first() {
        let m = [(0, 0, 1920, 1080), (1920, 0, 1920, 1080), (-1280, 0, 1280, 720)];
        assert_eq!(cursor_first(&m, (100, 100)), [0, 1, 2]);
        assert_eq!(cursor_first(&m, (2000, 10)), [1, 0, 2]);
        assert_eq!(cursor_first(&m, (-5, 10)), [2, 0, 1], "a monitor left of the primary has negative coordinates");
        assert_eq!(cursor_first(&m, (9999, 9999)), [0, 1, 2], "off every monitor keeps the order");
    }

    #[test]
    fn labels_say_which_screen_and_how_big() {
        let c = cap(0, 0, 1920, 1080, 1280, 720, true);
        assert_eq!(c.label(0, 1, Some(&c)), "user's screen (image dimensions: 1280x720 pixels)");
        assert!(c.label(0, 2, Some(&c)).contains("screen 1 of 2") && c.label(0, 2, Some(&c)).contains("primary focus") && c.label(0, 2, Some(&c)).contains("1280x720"));
        let s = cap(1920, 0, 1920, 1080, 1280, 720, false);
        assert!(s.label(1, 2, Some(&c)).contains("screen 2 of 2") && s.label(1, 2, Some(&c)).contains("secondary"));
        assert!(!c.label(0, 2, Some(&c)).contains("left of") && !c.label(0, 2, Some(&c)).contains("right of"), "the pointer's screen is the reference, so it has no position");
    }

    #[test]
    fn a_secondary_screen_says_where_it_sits_from_the_pointers_screen() {
        let main = cap(0, 0, 1920, 1080, 1280, 720, true);
        let left = cap(-1920, 0, 1920, 1080, 1280, 720, false);
        let right = cap(1920, 0, 1920, 1080, 1280, 720, false);
        let above = cap(0, -1080, 1920, 1080, 1280, 720, false);
        let below = cap(0, 1080, 1920, 1080, 1280, 720, false);
        assert_eq!(left.label(1, 2, Some(&main)), "screen 2 of 2, secondary screen, left of screen 1 (image dimensions: 1280x720 pixels)");
        assert!(right.label(1, 2, Some(&main)).contains(", right of screen 1 "));
        assert!(above.label(1, 2, Some(&main)).contains(", above screen 1 "));
        assert!(below.label(1, 2, Some(&main)).contains(", below screen 1 "));
        // three screens: each secondary one is described from screen 1, not from each other
        let l = left.label(1, 3, Some(&main));
        let r = right.label(2, 3, Some(&main));
        assert!(l.contains("screen 2 of 3") && l.contains("left of screen 1") && r.contains("screen 3 of 3") && r.contains("right of screen 1"));
        assert!(!left.label(1, 2, None).contains("of screen 1"), "no reference screen, no position");
    }

    #[test]
    fn position_follows_the_larger_gap_between_centres() {
        let main = cap(0, 0, 1920, 1080, 1280, 720, true);
        // a smaller monitor up and to the left, further across than up
        assert_eq!(cap(-1280, -300, 1280, 720, 1280, 720, false).side_of(&main), "left of");
        // the same size monitor, further up than across
        assert_eq!(cap(-400, -1080, 1920, 1080, 1280, 720, false).side_of(&main), "above");
        // a laptop screen under a big monitor, offset to the right
        assert_eq!(cap(300, 1080, 1366, 768, 1280, 720, false).side_of(&main), "below");
        assert_eq!(main.side_of(&main), "right of", "a tie is never a panic");
    }

    #[test]
    fn the_tag_is_cut_from_the_words() {
        let (said, p) = parse_point("It's up in the toolbar. [POINT:1100,42:color inspector]");
        assert_eq!(said, "It's up in the toolbar.");
        assert_eq!(p, Some(Point { x: 1100.0, y: 42.0, label: "color inspector".into(), screen: None }));
        let (said, p) = parse_point("It's on your other monitor. [POINT:400,300:terminal:screen2]\n");
        assert_eq!(said, "It's on your other monitor.");
        assert_eq!(p, Some(Point { x: 400.0, y: 300.0, label: "terminal".into(), screen: Some(2) }));
        assert_eq!(parse_point("Hypertext markup language. [POINT:none]"), ("Hypertext markup language.".into(), None));
        assert_eq!(parse_point("No tag at all."), ("No tag at all.".into(), None));
    }

    #[test]
    fn odd_tags_never_break_the_answer() {
        assert_eq!(parse_point("Here. [POINT:10, 20]").1, Some(Point { x: 10.0, y: 20.0, label: String::new(), screen: None }));
        assert_eq!(parse_point("Here. [POINT:10.5,20.5:save button:Screen3]").1.map(|p| (p.x, p.screen)), Some((10.5, Some(3))));
        assert_eq!(parse_point("Here. [POINT:10,20:screen2]").1, Some(Point { x: 10.0, y: 20.0, label: String::new(), screen: Some(2) }));
        assert_eq!(parse_point("Here. [POINT:left,top:x]"), ("Here.".into(), None), "a tag that doesn't parse is cut, with no point");
        assert_eq!(parse_point("[POINT:5,5:a]").0, "");
        assert_eq!(parse_point("Mid [POINT:1,2:a] text").1, None, "a tag that is not at the end is just words");
        assert_eq!(parse_point("Here. [POINT:1,2:a:screen0]").1.map(|p| p.screen), Some(None), "screen 0 does not exist");
    }

    #[test]
    fn points_land_on_the_right_monitor_pixel() {
        let a = cap(0, 0, 2560, 1440, 1280, 720, true);
        assert_eq!(to_screen(&a, 640.0, 360.0), (1280, 720));
        assert_eq!(to_screen(&a, 0.0, 0.0), (0, 0));
        let b = cap(2560, -200, 1920, 1080, 1280, 720, false);
        assert_eq!(to_screen(&b, 640.0, 360.0), (2560 + 960, -200 + 540), "a second monitor adds its own origin");
        let l = cap(-1280, 0, 1280, 720, 1280, 720, false);
        assert_eq!(to_screen(&l, 100.0, 50.0), (-1180, 50), "a monitor to the left stays on negative x");
        assert_eq!(to_screen(&a, 5000.0, -30.0), (2560, 0), "points outside the picture are held to its edge");
    }

    #[test]
    fn a_screen_number_picks_that_picture() {
        let caps = vec![cap(0, 0, 10, 10, 10, 10, true), cap(10, 0, 10, 10, 10, 10, false)];
        assert_eq!(target(&caps, None).map(|c| c.x), Some(0));
        assert_eq!(target(&caps, Some(2)).map(|c| c.x), Some(10));
        assert!(target(&caps, Some(3)).is_none(), "a screen that doesn't exist is not guessed");
        assert!(target(&caps, Some(0)).is_none(), "screen 0 never wraps around");
    }
}
