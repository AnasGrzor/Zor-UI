//! Zor UI's own icon in the taskbar's notification area (the "hidden icons" overflow), so the running
//! shell can be found, opened and quit from there.

use std::sync::OnceLock;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::HWND,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW},
            WindowsAndMessaging::{CreateIcon, GetSystemMetrics, LoadImageW, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXSMICON, SM_CYSMICON, WM_APP},
        },
    },
};

/// Mouse messages for the icon arrive on the app's message window as this (the low word of lparam is the mouse message).
pub const CALLBACK: u32 = WM_APP + 2;
const ID: u32 = 1;
const SIZE: usize = 32;

/// A 32 px BGRA picture: a purple disc with a white dot, drawn pixel by pixel (the exe has no icon resource).
fn pixels() -> Vec<u8> {
    let mut v = vec![0u8; SIZE * SIZE * 4];
    let c = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let (rgb, a): ([u8; 3], f32) = if d <= 5.5 { ([255, 255, 255], 1.0) } else { ([0xA4, 0x50, 0x67], (c + 0.5 - d).clamp(0.0, 1.0)) };
            let i = (y * SIZE + x) * 4;
            // premultiplied BGRA
            v[i..i + 4].copy_from_slice(&[(rgb[0] as f32 * a) as u8, (rgb[1] as f32 * a) as u8, (rgb[2] as f32 * a) as u8, (a * 255.0) as u8]);
        }
    }
    v
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut d = NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: ID, ..Default::default() };
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    d.uCallbackMessage = CALLBACK;
    for (i, c) in "Zor UI".encode_utf16().enumerate() { d.szTip[i] = c; }
    d
}

/// The icon's handle (stored as isize: a raw pointer is not Sync). Made once and kept for the process:
/// `add` runs every 30 s, and each new handle would leak.
static ICON: OnceLock<isize> = OnceLock::new();

/// The exe's own icon (resource 1, see build.rs); the drawn one only if it has none.
fn icon() -> Option<HICON> {
    if let Some(&h) = ICON.get() { return Some(HICON(h as _)); }
    let and = vec![0u8; SIZE * SIZE / 8];
    let made = unsafe {
        GetModuleHandleW(None).ok().and_then(|m| LoadImageW(m, PCWSTR(1 as *const u16), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_DEFAULTCOLOR).ok())
            .map(|h| HICON(h.0))
            .or_else(|| CreateIcon(None, SIZE as i32, SIZE as i32, 1, 32, and.as_ptr(), pixels().as_ptr()).ok())?
    };
    Some(HICON(*ICON.get_or_init(|| made.0 as isize) as _))
}

/// Adds the icon. Harmless to call again: Windows refuses a duplicate, and it brings the icon back after Explorer restarts.
pub fn add(hwnd: HWND) {
    let Some(icon) = icon() else { return };
    let mut d = data(hwnd);
    d.hIcon = icon;
    unsafe { let _ = Shell_NotifyIconW(NIM_ADD, &d); }
}

pub fn remove(hwnd: HWND) {
    unsafe { let _ = Shell_NotifyIconW(NIM_DELETE, &data(hwnd)); }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_made_once() {
        let (a, b) = (super::icon().map(|h| h.0), super::icon().map(|h| h.0));
        assert!(a.is_some());
        assert_eq!(a, b);
    }

    #[test]
    fn picture_is_round_with_a_white_dot() {
        let p = super::pixels();
        assert_eq!(p.len(), 32 * 32 * 4);
        assert_eq!(p[3], 0, "corner is transparent");
        let mid = (16 * 32 + 16) * 4;
        assert_eq!(&p[mid..mid + 4], &[255, 255, 255, 255]);
    }
}
