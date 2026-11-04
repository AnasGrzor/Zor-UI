//! Rust port of end4_bridge.py's screen-translate: drag a box over text on any monitor, Windows OCR
//! (Windows.Media.Ocr, the languages installed in Settings) reads it, and the translation is drawn over
//! it and copied. Click, Esc or right-click closes; it closes itself after 2 minutes.

use super::picker::Shot;
use super::*;
use serde_json::json;
use std::{cell::RefCell, sync::Mutex};
use windows::{
    core::{w, PCWSTR},
    Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::OcrEngine,
    Storage::Streams::DataWriter,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{Com::{CoInitializeEx, COINIT_MULTITHREADED}, LibraryLoader::GetModuleHandleW},
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

const TIMER_ID: usize = 1;
const HINT: &str = "Drag over the text to translate  (Esc to cancel)";

pub fn call(cfg: &Config, name: &str, _args: &[&str]) -> Option<Value> {
    (name == "screen-translate").then(|| {
        let target = Some(cfg.str("translator.target")).filter(|t| !t.is_empty()).unwrap_or("en").to_string();
        // own thread: the overlay pumps messages and must not stall whoever called us
        std::thread::spawn(move || run(&target)).join().unwrap_or_else(|_| json!({ "error": "screen-translate crashed" }))
    })
}

struct Line { text: String, x: f32, y: f32, w: f32, h: f32 }

/// One translated line (or the whole block) drawn over the screenshot, in screen pixels.
struct Item { x: i32, y: i32, w: i32, h: i32, text: String, size: i32, wrap: bool }

enum Done { Status(String), Result { items: Vec<Item>, text: String } }

#[derive(PartialEq)]
enum Stage { Select, Busy, Done }

struct State {
    shot: Shot, back_dc: HDC, back_bmp: HBITMAP, back_old: HGDIOBJ,
    start: Option<(i32, i32)>, cur: (i32, i32),
    stage: Stage, status: String, items: Vec<Item>, text: String, target: String,
}
thread_local! { static ST: RefCell<Option<State>> = const { RefCell::new(None) }; }
static INBOX: Mutex<Option<Done>> = Mutex::new(None);

fn run(target: &str) -> Value {
    unsafe {
        let Some(shot) = Shot::capture() else { return json!({ "error": "screen capture failed" }) };
        let hi = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        let class = w!("end4-screen-translate");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: hi, lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);
        let screen = GetDC(None);
        let back_dc = CreateCompatibleDC(screen);
        let back_bmp = CreateCompatibleBitmap(screen, shot.w, shot.h);
        ReleaseDC(None, screen);
        let back_old = SelectObject(back_dc, back_bmp);
        let (x, y, ww, hh) = (shot.x, shot.y, shot.w, shot.h);
        ST.with(|s| *s.borrow_mut() = Some(State { shot, back_dc, back_bmp, back_old, start: None, cur: (0, 0),
            stage: Stage::Select, status: HINT.into(), items: vec![], text: String::new(), target: target.into() }));
        if let Ok(hwnd) = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class, PCWSTR::null(), WS_POPUP | WS_VISIBLE, x, y, ww, hh, None, None, hi, None) {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(hwnd);
            SetTimer(hwnd, TIMER_ID, 120_000, None); // never leave a frozen full-screen overlay behind
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
        }
        let st = ST.with(|s| s.borrow_mut().take());
        let _ = UnregisterClassW(class, hi);
        let text = st.map(|s| { SelectObject(s.back_dc, s.back_old); let _ = DeleteObject(s.back_bmp); let _ = DeleteDC(s.back_dc); s.text }).unwrap_or_default();
        if text.is_empty() { return json!({ "cancelled": true }); }
        crate::util::clipboard_set(&text);
        json!({ "text": text })
    }
}

fn xy(lp: LPARAM) -> (i32, i32) { ((lp.0 & 0xffff) as i16 as i32, ((lp.0 >> 16) & 0xffff) as i16 as i32) }

/// Normalised (left, top, right, bottom) of the dragged box.
fn rect_of(a: (i32, i32), b: (i32, i32)) -> (i32, i32, i32, i32) { (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)) }

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => {
            let close = ST.with(|s| s.borrow_mut().as_mut().map(|s| match s.stage {
                Stage::Done => true,
                Stage::Select => { s.start = Some(xy(lp)); s.cur = xy(lp); false }
                Stage::Busy => false,
            }));
            if close == Some(true) { let _ = DestroyWindow(hwnd); } else { SetCapture(hwnd); }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let drawing = ST.with(|s| s.borrow_mut().as_mut().is_some_and(|s| { s.cur = xy(lp); s.start.is_some() && s.stage == Stage::Select }));
            if drawing { let _ = InvalidateRect(hwnd, None, false); }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            let job = ST.with(|s| s.borrow_mut().as_mut().and_then(|s| {
                let start = s.start.take().filter(|_| s.stage == Stage::Select)?;
                let (x0, y0, x1, y1) = rect_of(start, xy(lp));
                if x1 - x0 < 8 || y1 - y0 < 8 { return None; }
                s.stage = Stage::Busy;
                s.status = "Reading text…".into();
                let scale = if x1 - x0 < 400 { 2 } else { 1 }; // Windows OCR reads small text better enlarged
                Some((grab_bgra(&s.shot, x0, y0, x1 - x0, y1 - y0, scale), (x0, y0, x1, y1), scale, s.target.clone()))
            }));
            if let Some((pixels, area, scale, target)) = job {
                let h = hwnd.0 as isize;
                std::thread::spawn(move || {
                    let done = translate_area(pixels, area, scale, &target);
                    if let Ok(mut i) = INBOX.lock() { *i = Some(done); }
                    let _ = PostMessageW(HWND(h as *mut _), WM_APP, WPARAM(0), LPARAM(0));
                });
            }
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_APP => {
            let done = INBOX.lock().ok().and_then(|mut i| i.take());
            ST.with(|s| if let (Some(s), Some(d)) = (s.borrow_mut().as_mut(), done) {
                s.stage = Stage::Done;
                match d {
                    Done::Status(t) => s.status = t,
                    Done::Result { items, text } => { s.status = format!("Translated to {} and copied.  Click anywhere to close.", s.target); s.items = items; s.text = text; }
                }
            });
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_TIMER => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
            ST.with(|s| if let Some(s) = s.borrow_mut().as_mut() { s.text.clear(); });
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => { paint(hwnd); LRESULT(0) }
        WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn text_out(dc: HDC, x: i32, y: i32, size: i32, s: &str) {
    let font = CreateFontW(-size, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"));
    let old = SelectObject(dc, font);
    let wide: Vec<u16> = s.encode_utf16().collect();
    let _ = TextOutW(dc, x, y, &wide);
    SelectObject(dc, old);
    let _ = DeleteObject(font);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    ST.with(|s| {
        let s = s.borrow();
        let Some(s) = s.as_ref() else { return };
        let d = s.back_dc;
        let _ = BitBlt(d, 0, 0, s.shot.w, s.shot.h, s.shot.dc, 0, 0, SRCCOPY);
        SetBkMode(d, TRANSPARENT);
        if let (Some(a), Stage::Select) = (s.start, &s.stage) {
            let (l, t, r, b) = rect_of(a, s.cur);
            let rc = RECT { left: l, top: t, right: r, bottom: b };
            let (black, white) = (CreateSolidBrush(COLORREF(0)), CreateSolidBrush(COLORREF(0x00ff_ffff)));
            FrameRect(d, &rc, black);
            FrameRect(d, &RECT { left: l - 1, top: t - 1, right: r + 1, bottom: b + 1 }, white);
            let _ = (DeleteObject(black), DeleteObject(white));
        }
        let dark = CreateSolidBrush(COLORREF(0x001e_1e1e));
        SetTextColor(d, COLORREF(0x00ff_ffff));
        for it in &s.items {
            FillRect(d, &RECT { left: it.x - 3, top: it.y - 2, right: it.x + it.w + 3, bottom: it.y + it.h + 2 }, dark);
            if it.wrap {
                let font = CreateFontW(-it.size, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"));
                let old = SelectObject(d, font);
                let mut wide: Vec<u16> = it.text.encode_utf16().collect();
                let mut rc = RECT { left: it.x, top: it.y, right: it.x + it.w.max(120), bottom: it.y + it.h };
                DrawTextW(d, &mut wide, &mut rc, DT_WORDBREAK | DT_NOPREFIX);
                SelectObject(d, old);
                let _ = DeleteObject(font);
            } else {
                text_out(d, it.x, it.y, it.size, &it.text);
            }
        }
        let _ = DeleteObject(dark);
        // status line, on a dark strip so it reads on any screenshot
        let bar = CreateSolidBrush(COLORREF(0x001e_1e1e));
        let width = (s.status.chars().count() as i32 * 11 + 24).min(s.shot.w - 20);
        FillRect(d, &RECT { left: 14, top: 14, right: 14 + width, bottom: 50 }, bar);
        let _ = DeleteObject(bar);
        let font = CreateFontW(-20, 0, 0, 0, 700, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"));
        let old = SelectObject(d, font);
        let wide: Vec<u16> = s.status.encode_utf16().collect();
        let _ = TextOutW(d, 20, 18, &wide);
        SelectObject(d, old);
        let _ = DeleteObject(font);
        let _ = BitBlt(hdc, 0, 0, s.shot.w, s.shot.h, d, 0, 0, SRCCOPY);
    });
    let _ = EndPaint(hwnd, &ps);
}

/// The box (x, y, w, h in the shot) as top-down BGRA bytes, scaled up `scale` times, alpha 255.
fn grab_bgra(shot: &Shot, x: i32, y: i32, w: i32, h: i32, scale: i32) -> (Vec<u8>, i32, i32) {
    let (pw, ph) = (w * scale, h * scale);
    unsafe {
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: pw, biHeight: -ph, biPlanes: 1, biBitCount: 32, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        let dc = CreateCompatibleDC(shot.dc);
        let Ok(bmp) = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else { let _ = DeleteDC(dc); return (vec![], 0, 0) };
        let old = SelectObject(dc, bmp);
        SetStretchBltMode(dc, HALFTONE);
        let _ = SetBrushOrgEx(dc, 0, 0, None);
        let _ = StretchBlt(dc, 0, 0, pw, ph, shot.dc, x, y, w, h, SRCCOPY);
        let mut px = std::slice::from_raw_parts(bits as *const u8, (pw * ph * 4) as usize).to_vec();
        px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
        SelectObject(dc, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(dc);
        (px, pw, ph)
    }
}

/// Lines of text in a BGRA image: {text, x, y, w, h} in its pixels.
fn ocr(px: &[u8], w: i32, h: i32) -> Result<Vec<Line>, String> {
    let e = |what: &'static str| move |er: windows::core::Error| format!("OCR failed ({what}): {}", er.message());
    let writer = DataWriter::new().map_err(e("buffer"))?;
    writer.WriteBytes(px).map_err(e("buffer"))?;
    let buf = writer.DetachBuffer().map_err(e("buffer"))?;
    let bmp = SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Bgra8, w, h).map_err(e("bitmap"))?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| "No OCR language installed. Add one in Windows Settings > Language.".to_string())?;
    let res = engine.RecognizeAsync(&bmp).and_then(|o| o.get()).map_err(e("recognize"))?;
    let mut out = vec![];
    for l in res.Lines().map_err(e("lines"))? {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for word in l.Words().map_err(e("words"))? {
            let r = word.BoundingRect().map_err(e("words"))?;
            (x0, y0, x1, y1) = (x0.min(r.X), y0.min(r.Y), x1.max(r.X + r.Width), y1.max(r.Y + r.Height));
        }
        if x1 >= x0 { out.push(Line { text: l.Text().map_err(e("text"))?.to_string(), x: x0, y: y0, w: x1 - x0, h: y1 - y0 }); }
    }
    Ok(out)
}

/// Worker thread: OCR the grabbed box, translate it, lay the result out over the box.
fn translate_area((px, pw, ph): (Vec<u8>, i32, i32), (x0, y0, x1, y1): (i32, i32, i32, i32), scale: i32, target: &str) -> Done {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    if px.is_empty() { return Done::Status("Failed: couldn't copy the screen  (Click to close)".into()); }
    let lines = match ocr(&px, pw, ph) { Ok(l) => l, Err(m) => return Done::Status(format!("{m}  (Click to close)")) };
    if lines.is_empty() {
        return Done::Status("No text found. Other scripts need their language added in Windows Settings > Language.  (Click to close)".into());
    }
    let joined = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    let r = super::web::translate(target, "", &joined);
    if let Some(err) = r["error"].as_str() { return Done::Status(format!("Failed: {err}  (Click to close)").chars().take(200).collect()); }
    let text = r["text"].as_str().unwrap_or("").to_string();
    Done::Result { items: layout(&lines, &text, (x0, y0, x1, y1), scale as f32), text }
}

/// Puts each translated line over the line it came from. If the translation has a different number of
/// lines (merged or split), shows it as one block over the box.
fn layout(lines: &[Line], text: &str, (x0, y0, x1, y1): (i32, i32, i32, i32), scale: f32) -> Vec<Item> {
    let parts: Vec<&str> = text.split('\n').collect();
    let one = parts.len() != lines.len();
    let boxed = [Line { text: String::new(), x: 0.0, y: 0.0, w: (x1 - x0) as f32 * scale, h: (y1 - y0) as f32 * scale }];
    let (lines, parts) = if one { (&boxed[..], vec![text]) } else { (lines, parts) };
    let many = lines.len() > 1;
    lines.iter().zip(parts).map(|(l, t)| {
        let lh = l.h / scale;
        Item { x: x0 + (l.x / scale) as i32, y: y0 + (l.y / scale) as i32, w: (l.w / scale) as i32, h: lh as i32, text: t.to_string(),
            size: if many { ((lh * 0.8) as i32).max(9) } else { 14 }, wrap: !many }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line(t: &str, y: f32) -> Line { Line { text: t.into(), x: 10.0, y, w: 100.0, h: 20.0 } }

    #[test]
    fn matching_line_counts_stay_per_line() {
        let items = layout(&[line("a", 0.0), line("b", 30.0)], "x\ny", (100, 200, 400, 400), 1.0);
        assert_eq!((items.len(), items[1].y, items[1].size, items[1].wrap), (2, 230, 16, false));
    }

    #[test]
    fn mismatched_line_counts_become_one_block_over_the_box() {
        let items = layout(&[line("a", 0.0), line("b", 30.0)], "only one", (100, 200, 400, 400), 2.0);
        assert_eq!((items.len(), items[0].x, items[0].w, items[0].size, items[0].wrap), (1, 100, 300, 14, true));
    }
}
