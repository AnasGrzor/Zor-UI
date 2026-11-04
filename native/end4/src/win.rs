//! One layered window per widget. A `View` paints into it; the window turns pixels into
//! UpdateLayeredWindow calls and mouse messages into hover / click / wheel / drag.

use crate::{app::Ctx, gfx::{Canvas, Gfx, R}};
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
    /// Repaint interval while something moves (16 for the island spring, 100 for pulsing dots), 0 when still.
    fn frame_ms(&self) -> u32 { 0 }
    /// The hovered spot that has a popup, and its rect (bar groups).
    fn hover_popup(&self) -> Option<(u32, R)> { None }
    /// Popups: height (CSS px) of a header strip that drags the window (settings). 0: not movable.
    fn movable(&self) -> f32 { 0.0 }
    /// Popups: true if Esc is the view's (clear a search box); it then closes itself with Action::Close.
    fn captures_esc(&self) -> bool { false }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Z { Normal = 0, Bottom = 1, Top = 2 }

pub struct Win {
    pub hwnd: HWND,
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
    hover: u32,
    press: u32,
    drag: Option<Drag>,
    /// The left button is down on a spot (Input::Drag is flowing).
    held: bool,
    last: (u64, String, u32, (i32, i32)),
    /// When this window last painted; the frame timer uses it to give each view its own rate.
    painted: std::time::Instant,
}

struct Drag { start: POINT, from: POINT, moved: bool }

pub const CLASS: windows::core::PCWSTR = w!("end4-widget");
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
            Ok(Win { hwnd, view, drag_key: None, appbar: false, dc, dib: HBITMAP::default(), old: HGDIOBJ::default(), bits: std::ptr::null_mut(), px: (0, 0),
                hits: vec![], hover: 0, press: 0, drag: None, held: false, last: (u64::MAX, String::new(), 0, (0, 0)), painted: std::time::Instant::now() })
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

    /// Reserves `h` physical px at the top of the monitor for this window (Zebar's dockToEdge), so
    /// maximised windows and GlazeWM's tiling stay below it.
    pub fn dock_top(&mut self, mon: RECT, h: i32) {
        unsafe {
            let mut d = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, uCallbackMessage: WM_APPBAR, uEdge: ABE_TOP,
                rc: RECT { left: mon.left, top: mon.top, right: mon.right, bottom: mon.top + h }, ..Default::default() };
            if !self.appbar {
                // Explorer leaves tool windows out when it lays out app bars; WS_EX_NOACTIVATE alone
                // keeps the bar off the taskbar and out of Alt+Tab.
                let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) & !(WS_EX_TOOLWINDOW.0 as isize);
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex);
                SHAppBarMessage(ABM_NEW, &mut d);
                self.appbar = true;
            }
            SHAppBarMessage(ABM_QUERYPOS, &mut d);
            d.rc.bottom = d.rc.top + h;
            SHAppBarMessage(ABM_SETPOS, &mut d);
        }
    }

    /// True when the view has been due for a frame (`View::frame_ms`) since it last painted. One
    /// animation mustn't drag every other window down to its rate.
    pub fn due(&self, now: std::time::Instant, ms: u32) -> bool {
        now.duration_since(self.painted) >= std::time::Duration::from_millis(ms as u64)
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
            }
            let (pw, ph) = (px.0.max(1) as u32, px.1.max(1) as u32);
            let pixels = std::slice::from_raw_parts_mut(self.bits, (pw * ph * 4) as usize);
            let Some(mut c) = Canvas::new(g, pixels, pw, ph, s, self.hover) else { return };
            self.view.paint(&mut c, cx);
            self.hits = std::mem::take(&mut c.hits);
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
                        let _ = SetWindowPos(self.hwnd, None, d.from.x + dx, d.from.y + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                        return false;
                    }
                    if self.held {
                        let (x, y) = self.xy(lp);
                        self.view.input(Input::Drag(self.press, x, y, Phase::Move), cx);
                    }
                    let id = self.hit_lp(lp);
                    if id != self.hover { self.hover = id; self.view.input(Input::Hover(id), cx); }
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut t);
                }
                WM_MOUSELEAVE => if self.hover != 0 { self.hover = 0; self.view.input(Input::Hover(0), cx); },
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
                    self.press = self.hit_lp(lp);
                    let (x, y) = self.xy(lp);
                    if msg == WM_LBUTTONDOWN && self.press != 0 {
                        self.held = true;
                        self.view.input(Input::Drag(self.press, x, y, Phase::Start), cx);
                    }
                    if msg == WM_LBUTTONDOWN && self.press == 0 && (self.drag_key.is_some() || y < self.view.movable()) {
                        let mut p = POINT::default();
                        let _ = GetCursorPos(&mut p);
                        let (x, y) = self.pos();
                        self.drag = Some(Drag { start: p, from: POINT { x, y }, moved: false });
                    }
                    SetCapture(self.hwnd);
                }
                WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                    let _ = ReleaseCapture();
                    if self.held && msg == WM_LBUTTONUP {
                        self.held = false;
                        let (x, y) = self.xy(lp);
                        self.view.input(Input::Drag(self.press, x, y, Phase::End), cx);
                    }
                    if let Some(d) = self.drag.take() { if d.moved { return true; } }
                    let id = self.hit_lp(lp);
                    if id != 0 && id == self.press {
                        let b = match msg { WM_RBUTTONUP => Btn::Right, WM_MBUTTONUP => Btn::Middle, _ => Btn::Left };
                        self.view.input(Input::Click(id, b), cx);
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
