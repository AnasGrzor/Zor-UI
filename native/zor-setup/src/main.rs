//! Zor UI Setup: a small wizard window around install.ps1 and uninstall.ps1.
//! Pages: welcome, options (install only), progress, done. The logic lives in plan.rs and job.rs.
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod job;
mod plan;

use job::{Mode, Msg};
use plan::Options;
use std::{cell::RefCell, mem::size_of, os::windows::ffi::OsStrExt, path::Path, sync::mpsc};
use windows::{
    core::{w, PCWSTR, PWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{ApplicationInstallationAndServicing::*, LibraryLoader::GetModuleHandleW},
        UI::{
            Controls::{Dialogs::*, *},
            HiDpi::*,
            Input::KeyboardAndMouse::EnableWindow,
            WindowsAndMessaging::*,
        },
    },
};

const ID_NEXT: isize = 1; // IDOK, so Enter presses it
const ID_CANCEL: isize = 2; // IDCANCEL, so Esc closes
const ID_BROWSE: isize = 1002;
const ACCENT: u32 = 0x00A4_5067; // #6750A4, the default Material You seed (BGR)
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><dependency><dependentAssembly>
<assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
</dependentAssembly></dependency></assembly>"#;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn set_text(h: HWND, s: &str) {
    let _ = SetWindowTextW(h, PCWSTR(wide(s).as_ptr()));
}

unsafe fn text(h: HWND) -> String {
    let mut b = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
    let n = GetWindowTextW(h, &mut b) as usize;
    String::from_utf16_lossy(&b[..n])
}

unsafe fn checked(h: HWND) -> bool {
    SendMessageW(h, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1
}

unsafe fn font(face: PCWSTR, pt: i32, dpi: i32, weight: i32) -> HFONT {
    CreateFontW(-(pt * dpi / 72), 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32, 0, face)
}

/// Windows draws buttons and progress bars in the flat Windows 10 style only when the process asks for comctl32 v6.
/// There is no resource compiler on the GNU toolchain, so the manifest is written to a temp file and activated here.
unsafe fn enable_visual_styles() {
    let path = std::env::temp_dir().join("zorui-setup.manifest");
    if std::fs::write(&path, MANIFEST).is_err() {
        return;
    }
    let src: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let ctx = ACTCTXW { cbSize: size_of::<ACTCTXW>() as u32, lpSource: PCWSTR(src.as_ptr()), ..Default::default() };
    if let Ok(h) = CreateActCtxW(&ctx) {
        let mut cookie = 0usize;
        let _ = ActivateActCtx(h, &mut cookie);
    }
    let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES });
}

struct Ui {
    hwnd: HWND,
    mode: Mode,
    page: usize,
    pages: [Vec<HWND>; 4],
    title: HWND,
    next: HWND,
    cancel: HWND,
    combo: HWND,
    hide: HWND,
    lively: HWND,
    name: HWND,
    wall: HWND,
    step: HWND,
    bar: HWND,
    log: HWND,
    done: HWND,
    dpi: i32,
    opts: Options,
    steps: usize,
    windows_user: String,
    rx: Option<mpsc::Receiver<Msg>>,
}

thread_local! { static UI: RefCell<Option<Ui>> = const { RefCell::new(None) }; }

impl Ui {
    unsafe fn build(hwnd: HWND, mode: Mode, dpi: i32) -> Ui {
        let s = |v: i32| v * dpi / 96;
        let ui_font = font(w!("Segoe UI"), 9, dpi, 400);
        let title_font = font(w!("Segoe UI Semibold"), 20, dpi, 600);
        let mono = font(w!("Consolas"), 9, dpi, 400);
        let windows_user = std::env::var("USERNAME").unwrap_or_default();
        let install = mode == Mode::Install;

        let ctl = |class: PCWSTR, txt: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, f: HFONT, id: isize, x: i32, y: i32, wd: i32, ht: i32| -> HWND {
            let t = wide(txt);
            let h = CreateWindowExW(
                ex,
                class,
                PCWSTR(t.as_ptr()),
                style | WS_CHILD | WS_VISIBLE,
                s(x),
                s(y),
                s(wd),
                s(ht),
                hwnd,
                HMENU(id as *mut core::ffi::c_void),
                None,
                None,
            )
            .unwrap_or_default();
            SendMessageW(h, WM_SETFONT, WPARAM(f.0 as usize), LPARAM(1));
            h
        };
        let none = WINDOW_EX_STYLE(0);
        let label = |txt: &str, x, y, wd, ht| ctl(w!("STATIC"), txt, WINDOW_STYLE(0), none, ui_font, 0, x, y, wd, ht);
        let check = |txt: &str, x, y, wd| ctl(w!("BUTTON"), txt, WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), none, ui_font, 0, x, y, wd, 22);

        let title = ctl(w!("STATIC"), "Zor UI", WINDOW_STYLE(0), none, title_font, 0, 28, 18, 484, 36);
        ctl(w!("STATIC"), "", WINDOW_STYLE(0x10) /* SS_ETCHEDHORZ */, none, ui_font, 0, 0, 338, 540, 2);
        let next = ctl(w!("BUTTON"), "Next", WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32), none, ui_font, ID_NEXT, 300, 354, 110, 30);
        let cancel = ctl(w!("BUTTON"), "Cancel", WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32), none, ui_font, ID_CANCEL, 422, 354, 110, 30);

        // page 0: welcome
        let welcome = label(
            if install {
                "This sets up Zor UI on your PC:\r\n\r\n  \u{2022}  GlazeWM (tiling windows) and Flow Launcher, installed with winget when missing\r\n  \u{2022}  The Zor UI shell: bar, Dynamic Island, sidebars and desktop cards\r\n  \u{2022}  Material You colors taken from your wallpaper\r\n\r\nYour current GlazeWM and Flow Launcher settings are backed up first."
            } else {
                "This removes Zor UI and puts back your previous GlazeWM and Flow Launcher settings.\r\n\r\nGlazeWM and Flow Launcher stay installed. Remove them with winget if you want."
            },
            28, 76, 484, 240,
        );
        // page 1: options
        let l1 = label("Modifier key for the shortcuts", 28, 76, 300, 20);
        let combo = ctl(w!("COMBOBOX"), "", WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32), none, ui_font, 0, 28, 98, 240, 200);
        for item in ["Alt (never clashes with Windows)", "Win (closest to the original Super key)"] {
            SendMessageW(combo, CB_ADDSTRING, WPARAM(0), LPARAM(wide(item).as_ptr() as isize));
        }
        SendMessageW(combo, CB_SETCURSEL, WPARAM(0), LPARAM(0));
        let l2 = label("Your name (shown in the greeting)", 28, 134, 400, 20);
        let name = ctl(w!("EDIT"), &windows_user, WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), WS_EX_CLIENTEDGE, ui_font, 0, 28, 156, 240, 24);
        let l3 = label("Wallpaper to take colors from (empty: your current one)", 28, 192, 484, 20);
        let wall = ctl(w!("EDIT"), "", WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), WS_EX_CLIENTEDGE, ui_font, 0, 28, 214, 370, 24);
        let browse = ctl(w!("BUTTON"), "Browse...", WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32), none, ui_font, ID_BROWSE, 408, 213, 104, 26);
        let hide = check("Hide the Windows taskbar (auto-hide, only the Zor UI bar stays)", 28, 258, 484);
        let lively = check("Also install Lively Wallpaper (video wallpapers)", 28, 284, 484);
        // page 2: progress
        let step = label("Starting...", 28, 76, 484, 22);
        let bar = ctl(PROGRESS_CLASSW, "", WINDOW_STYLE(if install { PBS_SMOOTH } else { PBS_MARQUEE }), none, ui_font, 0, 28, 104, 484, 18);
        let log = ctl(w!("EDIT"), "", WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32), WS_EX_CLIENTEDGE, mono, 0, 28, 134, 484, 196);
        // page 3: done
        let done = label("", 28, 76, 484, 250);

        Ui {
            hwnd,
            mode,
            page: 0,
            pages: [vec![welcome], vec![l1, combo, l2, name, l3, wall, browse, hide, lively], vec![step, bar, log], vec![done]],
            title,
            next,
            cancel,
            combo,
            hide,
            lively,
            name,
            wall,
            step,
            bar,
            log,
            done,
            dpi,
            opts: Options::default(),
            steps: 0,
            windows_user,
            rx: None,
        }
    }

    unsafe fn show_page(&mut self, n: usize) {
        for (i, p) in self.pages.iter().enumerate() {
            for &h in p {
                let _ = ShowWindow(h, if i == n { SW_SHOW } else { SW_HIDE });
            }
        }
        self.page = n;
        let label = match (n, self.mode) {
            (0, Mode::Uninstall) => "Uninstall",
            (1, _) => "Install",
            (3, _) => "Finish",
            _ => "Next",
        };
        set_text(self.next, label);
        let _ = EnableWindow(self.next, n != 2);
        let _ = EnableWindow(self.cancel, n != 2);
        let _ = ShowWindow(self.cancel, if n == 3 { SW_HIDE } else { SW_SHOW });
    }

    /// The Next button. Returns a message to show when the options are not valid.
    unsafe fn next(&mut self) -> Option<String> {
        match (self.page, self.mode) {
            (0, Mode::Install) => self.show_page(1),
            (0, Mode::Uninstall) => self.start(),
            (1, _) => {
                let wall = text(self.wall).trim().to_string();
                if !wall.is_empty() && !Path::new(&wall).is_file() {
                    return Some("That wallpaper file does not exist.".into());
                }
                self.opts = Options {
                    win_key: SendMessageW(self.combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 == 1,
                    hide_taskbar: checked(self.hide),
                    lively: checked(self.lively),
                    user: plan::user_arg(&text(self.name), &self.windows_user),
                    wallpaper: wall,
                };
                self.start();
            }
            (3, _) => {
                let _ = DestroyWindow(self.hwnd);
            }
            _ => {}
        }
        None
    }

    unsafe fn start(&mut self) {
        self.show_page(2);
        if self.mode == Mode::Uninstall {
            SendMessageW(self.bar, PBM_SETMARQUEE, WPARAM(1), LPARAM(30));
        }
        let (tx, rx) = mpsc::channel();
        job::spawn(self.mode, self.opts.clone(), tx);
        self.rx = Some(rx);
        SetTimer(self.hwnd, 1, 100, None);
    }

    unsafe fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let msgs: Vec<Msg> = rx.try_iter().collect();
        for m in msgs {
            match m {
                Msg::Line(l) => self.line(&l),
                Msg::Done(r) => self.finish(r),
            }
        }
    }

    unsafe fn line(&mut self, l: &str) {
        let len = GetWindowTextLengthW(self.log) as usize;
        SendMessageW(self.log, EM_SETSEL, WPARAM(len), LPARAM(len as isize));
        SendMessageW(self.log, EM_REPLACESEL, WPARAM(0), LPARAM(wide(&format!("{l}\r\n")).as_ptr() as isize));
        if let Some(t) = plan::step_title(l) {
            self.steps += 1;
            set_text(self.step, t);
            SendMessageW(self.bar, PBM_SETPOS, WPARAM(plan::percent(self.steps) as usize), LPARAM(0));
        } else if self.mode == Mode::Uninstall {
            set_text(self.step, l.trim());
        }
    }

    unsafe fn finish(&mut self, r: Result<(), String>) {
        let _ = KillTimer(self.hwnd, 1);
        self.rx = None;
        let (verb, past) = if self.mode == Mode::Install { ("Setup", "installed") } else { ("Uninstall", "removed") };
        self.show_page(3);
        let s = |v: i32| v * self.dpi / 96;
        match r {
            Ok(()) if self.mode == Mode::Install => {
                set_text(self.done, &format!("Zor UI is {past} and running.\r\n\r\n{}", plan::shortcuts(self.opts.win_key)))
            }
            Ok(()) => set_text(
                self.done,
                "Zor UI is removed.\r\n\r\nTaskbar auto-hide, if you turned it on, is in Settings > Personalization > Taskbar.",
            ),
            Err(e) => {
                set_text(self.done, &format!("{verb} did not finish.\r\n\r\n{e}\r\n\r\nThe log below shows where it stopped."));
                let _ = MoveWindow(self.done, s(28), s(76), s(484), s(90), true);
                let _ = MoveWindow(self.log, s(28), s(176), s(484), s(154), true);
                let _ = ShowWindow(self.log, SW_SHOW);
            }
        }
    }

    unsafe fn browse(&self) {
        let mut buf = [0u16; 520];
        let filter: Vec<u16> = "Images\0*.jpg;*.jpeg;*.png;*.bmp;*.webp\0All files\0*.*\0\0".encode_utf16().collect();
        let mut ofn = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: self.hwnd,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buf.as_mut_ptr()),
            nMaxFile: buf.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
            ..Default::default()
        };
        if GetOpenFileNameW(&mut ofn).as_bool() {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            set_text(self.wall, &String::from_utf16_lossy(&buf[..end]));
        }
    }

    /// White background for labels and check boxes, accent color for the title.
    unsafe fn color(&self, hdc: HDC, ctl: HWND) -> LRESULT {
        SetBkColor(hdc, COLORREF(0xFF_FFFF));
        SetTextColor(hdc, COLORREF(if ctl == self.title { ACCENT } else { 0x20_2020 }));
        LRESULT(GetStockObject(WHITE_BRUSH).0 as isize)
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if m == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    // try_borrow_mut: a message sent while a handler runs (a modal dialog, DestroyWindow) falls through to the default
    let mut message: Option<String> = None;
    let handled = UI.with(|c| {
        let mut g = c.try_borrow_mut().ok()?;
        let ui = g.as_mut()?;
        match m {
            WM_COMMAND => {
                match (wp.0 & 0xFFFF) as isize {
                    ID_NEXT => message = ui.next(),
                    ID_CANCEL if ui.page != 2 => {
                        let _ = DestroyWindow(h);
                    }
                    ID_BROWSE => ui.browse(),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_TIMER => {
                ui.poll();
                Some(LRESULT(0))
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => Some(ui.color(HDC(wp.0 as *mut core::ffi::c_void), HWND(lp.0 as *mut core::ffi::c_void))),
            WM_CLOSE if ui.page == 2 => Some(LRESULT(0)), // no closing while the script runs
            _ => None,
        }
    });
    if let Some(msg) = message {
        MessageBoxW(h, PCWSTR(wide(&msg).as_ptr()), w!("Zor UI Setup"), MB_OK | MB_ICONINFORMATION);
    }
    handled.unwrap_or_else(|| DefWindowProcW(h, m, wp, lp))
}

fn main() {
    let mode = if std::env::args().any(|a| a == "--uninstall") { Mode::Uninstall } else { Mode::Install };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
        enable_visual_styles();
        let dpi = GetDpiForSystem() as i32;
        let hi = HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hi,
            hIcon: LoadIconW(hi, PCWSTR(1 as *const u16)).unwrap_or_default(), // build.rs embeds the icon as resource 1
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            lpszClassName: w!("ZorSetup"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut rc = RECT { left: 0, top: 0, right: 540 * dpi / 96, bottom: 400 * dpi / 96 };
        let _ = AdjustWindowRect(&mut rc, style, false);
        let (ww, wh) = (rc.right - rc.left, rc.bottom - rc.top);
        let title = if mode == Mode::Install { w!("Zor UI Setup") } else { w!("Zor UI Uninstall") };
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ZorSetup"),
            title,
            style,
            (GetSystemMetrics(SM_CXSCREEN) - ww) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - wh) / 2,
            ww,
            wh,
            None,
            None,
            hi,
            None,
        ) else {
            return;
        };
        let mut ui = Ui::build(hwnd, mode, dpi);
        ui.show_page(0);
        UI.with(|c| *c.borrow_mut() = Some(ui));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
