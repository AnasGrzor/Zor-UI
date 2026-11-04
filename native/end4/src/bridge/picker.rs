//! Rust port of end4_bridge.py's color-pick: a frozen, topmost copy of every monitor with a magnifier.
//! A click copies the pixel's hex colour, Esc or right-click cancels, and it gives up after 60 s.

use super::*;
use serde_json::json;
use std::cell::RefCell;
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

const N: i32 = 11; // pixels shown in the magnifier, each drawn K px wide
const K: i32 = 12;
const SIDE: i32 = N * K;
const LABEL_H: i32 = 30;
const TIMER_ID: usize = 1;

struct State { dc: HDC, w: i32, h: i32, mouse: (i32, i32), hex: Option<String> }
thread_local! { static ST: RefCell<Option<State>> = const { RefCell::new(None) }; }

pub fn call(_cfg: &Config, name: &str, _args: &[&str]) -> Option<Value> {
    (name == "color-pick").then(|| {
        // own thread: the overlay pumps messages and must not stall whoever called us
        std::thread::spawn(color_pick).join().unwrap_or_else(|_| json!({ "error": "color-pick crashed" }))
    })
}

fn hex_at(dc: HDC, w: i32, h: i32, x: i32, y: i32) -> String {
    let c = unsafe { GetPixel(dc, x.clamp(0, w - 1), y.clamp(0, h - 1)) }.0;
    format!("#{:02x}{:02x}{:02x}", c & 0xff, (c >> 8) & 0xff, (c >> 16) & 0xff)
}

/// Top-left of the magnifier for a pointer at x, y: right and below it, flipped at the screen edges.
fn magnifier_origin(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    let ox = if x + 24 + SIDE < w { x + 24 } else { x - 24 - SIDE };
    let oy = if y + 24 + SIDE + LABEL_H < h { y + 24 } else { y - 24 - SIDE - LABEL_H };
    (ox, oy)
}

fn magnifier_rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    let (ox, oy) = magnifier_origin(x, y, w, h);
    RECT { left: ox - 2, top: oy - 2, right: ox + SIDE + 2, bottom: oy + SIDE + LABEL_H + 2 }
}

/// A frozen copy of the whole virtual screen (every monitor) in a memory DC.
pub(super) struct Shot { pub dc: HDC, bmp: HBITMAP, old: HGDIOBJ, pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

impl Shot {
    pub fn capture() -> Option<Shot> {
        unsafe {
            let (x, y, w, h) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(dc, bmp);
            let copied = BitBlt(dc, 0, 0, w, h, screen, x, y, SRCCOPY).is_ok();
            ReleaseDC(None, screen);
            let shot = Shot { dc, bmp, old, x, y, w, h };
            copied.then_some(shot) // a failed capture drops here and frees the DC
        }
    }
}

impl Drop for Shot {
    fn drop(&mut self) { unsafe { SelectObject(self.dc, self.old); let _ = DeleteObject(self.bmp); let _ = DeleteDC(self.dc); } }
}

fn color_pick() -> Value {
    unsafe {
        let Some(shot) = Shot::capture() else { return json!({ "error": "screen capture failed" }) };
        let hi = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        let class = w!("end4-color-pick");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: hi, lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);
        ST.with(|s| *s.borrow_mut() = Some(State { dc: shot.dc, w: shot.w, h: shot.h, mouse: (-1000, -1000), hex: None }));
        let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class, PCWSTR::null(), WS_POPUP | WS_VISIBLE,
            shot.x, shot.y, shot.w, shot.h, None, None, hi, None);
        if let Ok(hwnd) = hwnd {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(hwnd);
            SetTimer(hwnd, TIMER_ID, 60_000, None); // never leave a frozen full-screen overlay behind
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
        }
        let hex = ST.with(|s| s.borrow_mut().take()).and_then(|s| s.hex);
        let _ = UnregisterClassW(class, hi);
        match hex {
            Some(h) => { crate::util::clipboard_set(&h); json!({ "hex": h }) }
            None => json!({ "cancelled": true }),
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = ((lp.0 & 0xffff) as i16 as i32, ((lp.0 >> 16) & 0xffff) as i16 as i32);
            let rects = ST.with(|s| s.borrow_mut().as_mut().map(|s| {
                let old = magnifier_rect(s.mouse.0, s.mouse.1, s.w, s.h);
                s.mouse = (x, y);
                (old, magnifier_rect(x, y, s.w, s.h))
            }));
            if let Some((a, b)) = rects { let _ = InvalidateRect(hwnd, Some(&a), false); let _ = InvalidateRect(hwnd, Some(&b), false); }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp.0 & 0xffff) as i16 as i32, ((lp.0 >> 16) & 0xffff) as i16 as i32);
            let picked = ST.with(|s| s.borrow_mut().as_mut().filter(|s| s.hex.is_none()).map(|s| {
                s.mouse = (x, y);
                s.hex = Some(hex_at(s.dc, s.w, s.h, x, y));
            }));
            if picked.is_some() { let _ = InvalidateRect(hwnd, None, false); SetTimer(hwnd, TIMER_ID, 500, None); } // show "Copied" briefly
            LRESULT(0)
        }
        WM_RBUTTONDOWN => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
            ST.with(|s| if let Some(s) = s.borrow_mut().as_mut() { s.hex = None });
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_TIMER => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => { paint(hwnd); LRESULT(0) }
        WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    ST.with(|s| {
        let s = s.borrow();
        let Some(s) = s.as_ref() else { return };
        let _ = BitBlt(hdc, 0, 0, s.w, s.h, s.dc, 0, 0, SRCCOPY); // clipped to the invalid region
        let (x, y) = s.mouse;
        if x < 0 { return; }
        let (ox, oy) = magnifier_origin(x, y, s.w, s.h);
        // the pixels around the pointer, black where they fall off the screen
        let tmp = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, N, N);
        let old = SelectObject(tmp, bmp);
        let _ = PatBlt(tmp, 0, 0, N, N, BLACKNESS);
        let _ = BitBlt(tmp, 0, 0, N, N, s.dc, x - N / 2, y - N / 2, SRCCOPY);
        SetStretchBltMode(hdc, COLORONCOLOR);
        let _ = StretchBlt(hdc, ox, oy, SIDE, SIDE, tmp, 0, 0, N, N, SRCCOPY);
        SelectObject(tmp, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(tmp);
        let white = CreateSolidBrush(COLORREF(0x00ff_ffff));
        let cell = RECT { left: ox + N / 2 * K, top: oy + N / 2 * K, right: ox + (N / 2 + 1) * K, bottom: oy + (N / 2 + 1) * K };
        FrameRect(hdc, &cell, white);
        FrameRect(hdc, &RECT { left: cell.left - 1, top: cell.top - 1, right: cell.right + 1, bottom: cell.bottom + 1 }, white);
        let _ = DeleteObject(white);
        let dark = CreateSolidBrush(COLORREF(0x001e_1e1e));
        FillRect(hdc, &RECT { left: ox, top: oy + SIDE, right: ox + SIDE, bottom: oy + SIDE + LABEL_H }, dark);
        let _ = DeleteObject(dark);
        let text = match &s.hex { Some(h) => format!("Copied {h}"), None => hex_at(s.dc, s.w, s.h, x, y) };
        let font = CreateFontW(-16, 0, 0, 0, 700, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, w!("Segoe UI"));
        let old_font = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(0x00ff_ffff));
        let wide: Vec<u16> = text.encode_utf16().collect();
        let _ = TextOutW(hdc, ox + 6, oy + SIDE + 5, &wide);
        SelectObject(hdc, old_font);
        let _ = DeleteObject(font);
    });
    let _ = EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn magnifier_flips_at_edges() {
        assert_eq!(magnifier_origin(100, 100, 1920, 1080), (124, 124));
        assert_eq!(magnifier_origin(1900, 1070, 1920, 1080), (1900 - 24 - SIDE, 1070 - 24 - SIDE - LABEL_H));
    }
}
