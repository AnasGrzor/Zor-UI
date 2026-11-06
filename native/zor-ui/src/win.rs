//! One layered window per widget. A `View` paints into it; the window turns pixels into
//! UpdateLayeredWindow calls and mouse messages into hover / click / wheel / drag.

use crate::{app::{Action, Ctx}, gfx::{Canvas, Gfx, R}, menu::Item};
use windows::{
    core::w,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Btn { Left, Right, Middle, Double }

pub enum Input {
    /// The pointer moved onto spot `id` (0: nothing).
    Hover(u32),
    Click(u32, Btn),
    /// Wheel over spot `id`: >0 is away from the user (up).
    Wheel(u32, i32),
    /// A key went down in a focused window (popups). `vk` is a virtual-key code (0x1B Esc, 0x0D Enter...).
    Key { vk: u32, ctrl: bool, shift: bool, alt: bool },
    /// Typed text (already layout- and shift-aware); control characters never arrive.
    Char(char),
    /// The left button is held on spot `id`; x, y in CSS px of the window. Start on press, End on release
    /// (then the Click follows if it ended on the same spot). For sliders, seek bars, caret placement.
    Drag(u32, f32, f32, Phase),
    /// The window gained or lost keyboard focus.
    Focus(bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase { Start, Move, End }

pub trait View {
    /// Size in CSS px.
    fn size(&self, g: &Gfx, cx: &Ctx, scale: f32) -> (f32, f32);
    /// Anything time-based the picture depends on; the window repaints when it changes.
    fn key(&self, _cx: &Ctx) -> String { String::new() }
    fn paint(&mut self, c: &mut Canvas, cx: &Ctx);
    fn input(&mut self, _ev: Input, _cx: &mut Ctx) {}
    /// Right-click menu for spot `id` (0: the window's background). Empty: no menu.
    fn context(&mut self, _id: u32, _cx: &Ctx) -> Vec<Item> { vec![] }
    /// Repaint interval while something moves (16 for the island spring, 100 for pulsing dots), 0 when still.
    fn frame_ms(&self) -> u32 { 0 }
    /// The hovered spot that has a popup, and its rect (bar groups).
    fn hover_popup(&self) -> Option<(u32, R, String)> { None }
    /// The screen edge the window sits on, when it is not the bar's (the dock). The popup opens away from it.
    fn popup_edge(&self) -> Option<crate::bar::Edge> { None }
    /// Popups: height (CSS px) of a header strip that drags the window (settings). 0: not movable.
    fn movable(&self) -> f32 { 0.0 }
    /// Popups: true if Esc is the view's (clear a search box); it then closes itself with Action::Close.
    fn captures_esc(&self) -> bool { false }
    /// Popups: (min, max) size in CSS px if dragging any edge or corner resizes the window (settings).
    fn limits(&self) -> Option<((f32, f32), (f32, f32))> { None }
    /// Sets the size in CSS px; only called when `limits` is Some.
    fn resize(&mut self, _w: f32, _h: f32) {}
    /// A resize drag ended (settings saves the size).
    fn resize_end(&mut self) {}
}

/// Width of the resize band along each edge, CSS px. Settings' panel starts 6 px in and the shadow
/// outside it is see-through (the mouse passes through), so the band reaches 6 px into the panel.
pub const EDGE: f32 = 12.0;

/// Which edges the point (x, y) touches on a (w, h) window: (-1 left, 1 right, 0 neither; -1 top, 1 bottom, 0 neither).
pub fn edge_at(x: f32, y: f32, w: f32, h: f32) -> (i32, i32) {
    let f = |v: f32, n: f32| if v < EDGE { -1 } else if v >= n - EDGE { 1 } else { 0 };
    (f(x, w), f(y, h))
}

/// Window rect (left, top, w, h) after dragging `edge` by `d`, with each side held to [min, max].
/// Dragging a left or top edge moves the window so the opposite edge stays put.
pub fn drag_rect(r: (i32, i32, i32, i32), edge: (i32, i32), d: (i32, i32), min: (i32, i32), max: (i32, i32)) -> (i32, i32, i32, i32) {
    let side = |p: i32, len: i32, e: i32, d: i32, lo: i32, hi: i32| match e {
        1 => (p, (len + d).clamp(lo, hi)),
        -1 => { let n = (len - d).clamp(lo, hi); (p + len - n, n) }
        _ => (p, len),
    };
    let (x, w) = side(r.0, r.2, edge.0, d.0, min.0, max.0);
    let (y, h) = side(r.1, r.3, edge.1, d.1, min.1, max.1);
    (x, y, w, h)
}

thread_local! {
    /// The pointer is over a resizable window's edge (see `edge_at`): WM_SETCURSOR shows the matching arrow.
    static EDGE_HOT: std::cell::Cell<(i32, i32)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_and_corners() {
        assert_eq!(edge_at(100.0, 100.0, 900.0, 700.0), (0, 0));
        assert_eq!(edge_at(2.0, 100.0, 900.0, 700.0), (-1, 0));
        assert_eq!(edge_at(890.0, 100.0, 900.0, 700.0), (1, 0));
        assert_eq!(edge_at(100.0, 3.0, 900.0, 700.0), (0, -1));
        assert_eq!(edge_at(100.0, 695.0, 900.0, 700.0), (0, 1));
        assert_eq!(edge_at(1.0, 1.0, 900.0, 700.0), (-1, -1));
        assert_eq!(edge_at(899.0, 699.0, 900.0, 700.0), (1, 1));
    }

    #[test]
    fn right_and_bottom_grow_in_place() {
        let r = (100, 50, 800, 600);
        assert_eq!(drag_rect(r, (1, 1), (40, -30), (300, 300), (2000, 2000)), (100, 50, 840, 570));
        assert_eq!(drag_rect(r, (0, 1), (99, 10), (300, 300), (2000, 2000)), (100, 50, 800, 610));
    }

    #[test]
    fn left_and_top_move_the_window() {
        let r = (100, 50, 800, 600);
        // dragging the left edge 40 px left grows the window and keeps the right edge at 900
        assert_eq!(drag_rect(r, (-1, 0), (-40, 0), (300, 300), (2000, 2000)), (60, 50, 840, 600));
        assert_eq!(drag_rect(r, (0, -1), (0, 20), (300, 300), (2000, 2000)), (100, 70, 800, 580));
    }

    #[test]
    fn clamps_stop_the_window_moving() {
        let r = (100, 50, 800, 600);
        // shrinking past the minimum pins the width, and the left edge stops at 900 - 300
        assert_eq!(drag_rect(r, (-1, 0), (700, 0), (300, 300), (2000, 2000)), (600, 50, 300, 600));
        assert_eq!(drag_rect(r, (1, 0), (5000, 0), (300, 300), (2000, 2000)), (100, 50, 2000, 600));
        assert_eq!(drag_rect(r, (-1, 0), (-5000, 0), (300, 300), (2000, 2000)), (-1100, 50, 2000, 600));
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Z { Normal = 0, Bottom = 1, Top = 2 }

pub struct Win {
    pub hwnd: HWND,
    /// What Settings > Usage calls this window (see perf::group).
    part: String,
    pub view: Box<dyn View>,
    /// Desk cards: drag anywhere that isn't a control, and remember the spot under this name.
    pub drag_key: Option<String>,
    appbar: bool,
    dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    px: (i32, i32),
    hits: Vec<(R, u32)>,
    bars: Vec<crate::ui::Bar>,
    /// Scrollbar being dragged: (bar id, pointer offset inside the thumb).
    sdrag: Option<(u32, f32)>,
    hover: u32,
    press: u32,
    drag: Option<Drag>,
    /// Edge being dragged: which edge, pointer position and window rect (physical px) at the press.
    rsz: Option<((i32, i32), POINT, (i32, i32, i32, i32))>,
    /// The left button is down on a spot (Input::Drag is flowing).
    held: bool,
    last: (u64, String, u32, (i32, i32)),
    /// When this window last painted; the frame timer uses it to give each view its own rate.
    painted: std::time::Instant,
    /// The last paint left a scroll glide running: needs 16 ms frames until it settles.
    pub gliding: bool,
}

struct Drag { start: POINT, from: POINT, moved: bool, snap: Vec<(i32, i32, i32, i32)> }

pub const CLASS: windows::core::PCWSTR = w!("zor-ui-widget");
/// Not in the WindowsAndMessaging module of the windows crate.
pub const WM_MOUSELEAVE: u32 = 0x02A3;
/// Shell -> app bar notifications (ABN_POSCHANGED...).
pub const WM_APPBAR: u32 = WM_APP + 1;
/// GWLP_USERDATA marker for pinned windows (bars); the Z values use 0-2.
const PINNED: isize = 3;

pub fn register(proc_: WNDPROC) {
    unsafe {
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: proc_,
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

impl Win {
    /// `click_through`: mouse goes to whatever is underneath (hover popups).
    pub fn new(view: Box<dyn View>, title: &str, x: i32, y: i32, z: Z, click_through: bool) -> windows::core::Result<Win> {
        unsafe {
            let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            if z == Z::Top { ex |= WS_EX_TOPMOST; }
            if click_through { ex |= WS_EX_TRANSPARENT; }
            let hwnd = CreateWindowExW(ex, CLASS, &windows::core::HSTRING::from(title), WS_POPUP, x, y, 1, 1, None, None, GetModuleHandleW(None)?, None)?;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, z as isize); // read by wndproc without borrowing the app
            let dc = CreateCompatibleDC(None);
            Ok(Win { hwnd, part: crate::perf::group(title), view, drag_key: None, appbar: false, dc, dib: HBITMAP::default(), old: HGDIOBJ::default(), bits: std::ptr::null_mut(), px: (0, 0),
                hits: vec![], bars: vec![], sdrag: None, hover: 0, press: 0, drag: None, rsz: None, held: false, last: (u64::MAX, String::new(), 0, (0, 0)), painted: std::time::Instant::now(), gliding: false })
        }
    }

    pub fn scale(&self) -> f32 { unsafe { GetDpiForWindow(self.hwnd) as f32 / 96.0 } }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            if GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) == Z::Bottom as isize {
                let _ = SetWindowPos(self.hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    /// Keeps the window where it is: the shell moves windows out of a new app bar strip,
    /// including the app bar itself.
    pub fn pin(&self) { unsafe { SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, PINNED); } }

    /// Lets the window take keyboard focus (popups, a notes card being typed in); off again for click-through widgets.
    pub fn set_focusable(&self, on: bool) {
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let flag = WS_EX_NOACTIVATE.0 as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, if on { ex & !flag } else { ex | flag });
        }
    }

    /// Takes the keyboard focus. Windows only lets the process that got the last input do that, and a
    /// shortcut's key went to GlazeWM, so a no-op key (F24) first makes this process eligible
    /// (cmd_focus_widget in end4_bridge.py does the same).
    pub fn focus(&self) {
        unsafe {
            keybd_event(0x87, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(0x87, 0, KEYEVENTF_KEYUP, 0);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    /// Size of the last paint in physical px.
    pub fn size_px(&self) -> (i32, i32) { self.px }

    /// Window rect in physical px.
    pub fn rect(&self) -> RECT {
        let mut r = RECT::default();
        unsafe { let _ = GetWindowRect(self.hwnd, &mut r); }
        r
    }

    pub fn hide(&self) { unsafe { let _ = ShowWindow(self.hwnd, SW_HIDE); } }
    pub fn visible(&self) -> bool { unsafe { IsWindowVisible(self.hwnd).as_bool() } }

    pub fn pos(&self) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe { let _ = GetWindowRect(self.hwnd, &mut r); }
        (r.left, r.top)
    }

    pub fn move_to(&self, x: i32, y: i32) {
        unsafe { let _ = SetWindowPos(self.hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE); }
    }

    /// Reserves `h` physical px along `edge` of the monitor for this window (Zebar's dockToEdge), so
    /// maximised windows and GlazeWM's tiling stay clear of it.
    pub fn dock(&mut self, mon: RECT, edge: crate::bar::Edge, h: i32) {
        unsafe {
            let mut d = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, uCallbackMessage: WM_APPBAR, uEdge: edge.abe(),
                rc: crate::bar::strip(mon, edge, h), ..Default::default() };
            if !self.appbar {
                // Explorer leaves tool windows out when it lays out app bars; WS_EX_NOACTIVATE alone
                // keeps the bar off the taskbar and out of Alt+Tab.
                let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) & !(WS_EX_TOOLWINDOW.0 as isize);
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex);
                SHAppBarMessage(ABM_NEW, &mut d);
                self.appbar = true;
            }
            SHAppBarMessage(ABM_QUERYPOS, &mut d);
            d.rc = crate::bar::strip(d.rc, edge, h); // the shell may have moved the edge in, past other app bars
            SHAppBarMessage(ABM_SETPOS, &mut d);
        }
    }

    /// True when the view has been due for a frame (`View::frame_ms`) since it last painted. One
    /// animation mustn't drag every other window down to its rate.
    pub fn due(&self, now: std::time::Instant, ms: u32) -> bool {
        // Windows timers tick every ~15.6 ms, so a 33 ms timer fires at ~31 ms: without slack every
        // other tick is skipped and a 30 fps animation (Gojo's walk) runs at 15 fps.
        now.duration_since(self.painted) >= std::time::Duration::from_millis((ms - ms / 4) as u64)
    }

    /// Repaints if the view's picture could have changed (data version, time key, hover, DPI).
    pub fn update(&mut self, g: &Gfx, cx: &Ctx, force: bool) {
        let key = self.view.key(cx);
        let s = self.scale();
        let (w, h) = self.view.size(g, cx, s);
        let px = ((w * s).ceil() as i32, (h * s).ceil() as i32);
        let now = (cx.ver, key, self.hover, px);
        if !force && now == self.last { return; }
        self.last = now;
        unsafe {
            if px != self.px {
                if !self.dib.is_invalid() { SelectObject(self.dc, self.old); let _ = DeleteObject(HGDIOBJ::from(self.dib)); }
                let bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: px.0.max(1), biHeight: -px.1.max(1), biPlanes: 1, biBitCount: 32, ..Default::default() }, ..Default::default() };
                let mut bits = std::ptr::null_mut();
                let Ok(dib) = CreateDIBSection(self.dc, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else { return };
                self.dib = dib;
                self.old = SelectObject(self.dc, HGDIOBJ::from(dib));
                self.bits = bits as *mut u8;
                self.px = px;
                crate::perf::buffer(self.hwnd.0 as isize, &self.part, (px.0.max(1) as u64) * (px.1.max(1) as u64) * 4);
            }
            let (pw, ph) = (px.0.max(1) as u32, px.1.max(1) as u32);
            let pixels = std::slice::from_raw_parts_mut(self.bits, (pw * ph * 4) as usize);
            let Some(mut c) = Canvas::new(g, pixels, pw, ph, s, self.hover) else { return };
            crate::ui::MOVING.with(|m| m.set(false));
            crate::ui::BARS.with(|b| b.borrow_mut().clear());
            let t0 = std::time::Instant::now();
            self.view.paint(&mut c, cx);
            crate::perf::paint(&self.part, t0.elapsed().as_nanos() as u64);
            self.gliding = crate::ui::MOVING.with(|m| m.get());
            self.hits = std::mem::take(&mut c.hits);
            self.bars = crate::ui::BARS.with(|b| std::mem::take(&mut *b.borrow_mut()));
            drop(c);
            let size = SIZE { cx: pw as i32, cy: ph as i32 };
            let src = POINT::default();
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8, ..Default::default() };
            let _ = UpdateLayeredWindow(self.hwnd, None, None, Some(&size), self.dc, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        }
        self.painted = std::time::Instant::now();
    }

    fn hit_at(&self, x: f32, y: f32) -> u32 {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map_or(0, |&(_, id)| id)
    }
    /// Moves the dragged scrollbar's thumb to pointer y (CSS px).
    fn sbar_to(&self, y: f32) {
        let Some((id, off)) = self.sdrag else { return };
        if let Some(b) = self.bars.iter().find(|b| b.id == id) {
            b.pend.set(Some(b.target(y, off)));
        }
    }
    fn xy(&self, lp: LPARAM) -> (f32, f32) {
        let s = self.scale();
        ((lp.0 & 0xFFFF) as i16 as f32 / s, ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s)
    }
    fn hit_lp(&self, lp: LPARAM) -> u32 { let (x, y) = self.xy(lp); self.hit_at(x, y) }

    /// Mouse input -> view events. Returns true if a card drag just ended.
    pub fn mouse(&mut self, msg: u32, wp: WPARAM, lp: LPARAM, cx: &mut Ctx) -> bool {
        unsafe {
            match msg {
                WM_MOUSEMOVE => {
                    if let Some(d) = &mut self.drag {
                        let mut p = POINT::default();
                        let _ = GetCursorPos(&mut p);
                        let (dx, dy) = (p.x - d.start.x, p.y - d.start.y);
                        if !d.moved && dx * dx + dy * dy < 16 { return false; } // a move under 4px is still a click
                        d.moved = true;
                        let (mut nx, mut ny) = (d.from.x + dx, d.from.y + dy);
                        // Figma-style guides: cards line up with each other unless Alt is held or Settings turns it off
                        if !d.snap.is_empty() && GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000 == 0 {
                            let k = GetDpiForWindow(self.hwnd) as f32 / 96.0;
                            let mut r = RECT::default();
                            let _ = GetWindowRect(self.hwnd, &mut r);
                            let m = (4.0 * k).round() as i32;
                            (nx, ny) = crate::desk::snap_card((nx, ny), (r.right - r.left, r.bottom - r.top), m, &d.snap, (12.0 * k).round() as i32, (8.0 * k).round() as i32);
                            if crate::guides::wanted(cx) { crate::guides::set(crate::guides::align_lines((nx + m, ny + m, nx + r.right - r.left - m, ny + r.bottom - r.top - m), &d.snap)); }
                        } else { crate::guides::clear(); }
                        let _ = SetWindowPos(self.hwnd, None, nx, ny, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                        return false;
                    }
                    if let (Some((edge, p0, r0)), Some((min, max))) = (self.rsz, self.view.limits()) {
                        let mut p = POINT::default();
                        let _ = GetCursorPos(&mut p);
                        let s = self.scale();
                        let px = |(w, h): (f32, f32)| ((w * s).round() as i32, (h * s).round() as i32);
                        let (x, y, w, h) = drag_rect(r0, edge, (p.x - p0.x, p.y - p0.y), px(min), px(max));
                        self.view.resize(w as f32 / s, h as f32 / s);
                        self.move_to(x, y);
                        cx.ver += 1;
                        return false;
                    }
                    if self.view.limits().is_some() {
                        let (x, y) = self.xy(lp);
                        let s = self.scale();
                        EDGE_HOT.with(|e| e.set(edge_at(x, y, self.px.0 as f32 / s, self.px.1 as f32 / s)));
                    }
                    if self.sdrag.is_some() { let (_, y) = self.xy(lp); self.sbar_to(y); cx.ver += 1; return false; }
                    if self.held {
                        let (x, y) = self.xy(lp);
                        self.view.input(Input::Drag(self.press, x, y, Phase::Move), cx);
                    }
                    let id = self.hit_lp(lp);
                    if id != self.hover { self.hover = id; self.view.input(Input::Hover(id), cx); }
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut t);
                }
                WM_MOUSELEAVE => if EDGE_HOT.with(|e| e.replace((0, 0))) != (0, 0) || self.hover != 0 { self.hover = 0; self.view.input(Input::Hover(0), cx); },
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
                    let (x, y) = self.xy(lp);
                    if msg == WM_LBUTTONDOWN && self.view.limits().is_some() {
                        let s = self.scale();
                        let edge = edge_at(x, y, self.px.0 as f32 / s, self.px.1 as f32 / s);
                        if edge != (0, 0) {
                            let mut p = POINT::default();
                            let _ = GetCursorPos(&mut p);
                            let r = self.rect();
                            self.rsz = Some((edge, p, (r.left, r.top, r.right - r.left, r.bottom - r.top)));
                            SetCapture(self.hwnd);
                            return false;
                        }
                    }
                    self.press = self.hit_lp(lp);
                    if msg == WM_LBUTTONDOWN && self.press >= crate::ui::SBAR {
                        if let Some(b) = self.bars.iter().find(|b| b.id == self.press) {
                            self.sdrag = Some((b.id, b.grab(y)));
                            self.sbar_to(y); cx.ver += 1;
                        }
                        self.press = 0;
                        SetCapture(self.hwnd);
                        return false;
                    }
                    if msg == WM_LBUTTONDOWN && self.press != 0 {
                        self.held = true;
                        self.view.input(Input::Drag(self.press, x, y, Phase::Start), cx);
                    }
                    if msg == WM_LBUTTONDOWN && self.press == 0 && (self.drag_key.is_some() || y < self.view.movable()) {
                        let mut p = POINT::default();
                        let _ = GetCursorPos(&mut p);
                        let (x, y) = self.pos();
                        let k = GetDpiForWindow(self.hwnd) as f32 / 96.0;
                        let snap = if self.drag_key.is_some() && cx.cfg.get("desktop.snap").as_bool() != Some(false) { crate::desk::snap_targets(self.hwnd, (4.0 * k).round() as i32) } else { vec![] };
                        self.drag = Some(Drag { start: p, from: POINT { x, y }, moved: false, snap });
                    }
                    SetCapture(self.hwnd);
                }
                WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                    let _ = ReleaseCapture();
                    if self.sdrag.take().is_some() { return false; }
                    if self.rsz.take().is_some() { self.view.resize_end(); return false; }
                    if self.held && msg == WM_LBUTTONUP {
                        self.held = false;
                        let (x, y) = self.xy(lp);
                        self.view.input(Input::Drag(self.press, x, y, Phase::End), cx);
                    }
                    if let Some(d) = self.drag.take() { crate::guides::clear(); if d.moved { return true; } }
                    let id = self.hit_lp(lp);
                    if id != 0 && id == self.press {
                        let b = match msg { WM_RBUTTONUP => Btn::Right, WM_MBUTTONUP => Btn::Middle, _ => Btn::Left };
                        self.view.input(Input::Click(id, b), cx);
                    }
                    if msg == WM_RBUTTONUP {
                        let items = self.view.context(id, cx);
                        if !items.is_empty() {
                            let m = GetMessagePos(); // where the click was, not where the pointer is by the time we get here
                            cx.actions.push(Action::Menu(items, (m as i16 as i32, (m >> 16) as i16 as i32)));
                        }
                    }
                    self.press = 0;
                }
                WM_LBUTTONDBLCLK => { let id = self.hit_lp(lp); if id != 0 { self.view.input(Input::Click(id, Btn::Double), cx); } }
                WM_MOUSEWHEEL => {
                    // wheel coordinates are screen px
                    let (mut r, s) = (RECT::default(), self.scale());
                    let _ = GetWindowRect(self.hwnd, &mut r);
                    let (x, y) = (((lp.0 & 0xFFFF) as i16 as i32 - r.left) as f32 / s, (((lp.0 >> 16) & 0xFFFF) as i16 as i32 - r.top) as f32 / s);
                    let d = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                    let id = self.hit_at(x, y);
                    let id = self.hits.iter().rev().find(|(r, i)| *i < crate::ui::SBAR && r.contains(x, y)).map_or(0, |&(_, i)| i);
                    if id != 0 { self.view.input(Input::Wheel(id, d), cx); }
                }
                _ => {}
            }
        }
        false
    }
}

impl Drop for Win {
    fn drop(&mut self) {
        crate::perf::forget(self.hwnd.0 as isize);
        unsafe {
            if self.appbar {
                let mut d = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, ..Default::default() };
                SHAppBarMessage(ABM_REMOVE, &mut d);
            }            if !self.dib.is_invalid() { SelectObject(self.dc, self.old); let _ = DeleteObject(HGDIOBJ::from(self.dib)); }
            let _ = DeleteDC(self.dc);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Window messages that need no app state (so they work while the app is busy painting).
pub unsafe fn early(hwnd: HWND, msg: u32, _wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT && EDGE_HOT.with(|e| e.get()) != (0, 0) => {
            let (ex, ey) = EDGE_HOT.with(|e| e.get());
            let c = match (ex, ey) { (0, _) => IDC_SIZENS, (_, 0) => IDC_SIZEWE, _ if ex == ey => IDC_SIZENWSE, _ => IDC_SIZENESW };
            SetCursor(LoadCursorW(None, c).unwrap_or_default());
            Some(LRESULT(1))
        }
        WM_MOUSEACTIVATE => {
            let noact = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_NOACTIVATE.0 as isize != 0;
            Some(LRESULT(if noact { MA_NOACTIVATE } else { MA_ACTIVATE } as isize))
        }
        WM_WINDOWPOSCHANGING if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == Z::Bottom as isize => {
            // zOrder "bottom_most": stay under every other window
            let wp = &mut *(lp.0 as *mut WINDOWPOS);
            wp.hwndInsertAfter = HWND_BOTTOM;
            wp.flags &= !SWP_NOZORDER;
            Some(LRESULT(0))
        }
        WM_WINDOWPOSCHANGING if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == PINNED => {
            (*(lp.0 as *mut WINDOWPOS)).flags |= SWP_NOMOVE;
            Some(LRESULT(0))
        }
        _ => None,
    }
}
