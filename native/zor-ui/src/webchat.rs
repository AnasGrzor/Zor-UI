//! Opens a WhatsApp Web chat link the way a person would: in the logged-in WhatsApp tab that is already open. WhatsApp Web
//! allows one live session, so a link opened in a new tab only shows "WhatsApp is open in another window" and drops the number
//! and text. Only when there is no WhatsApp tab does the link open in a new tab.
//!
//! The tab is found and driven through Windows UI Automation, in Edge, Chrome or any other Chromium browser window. Every step
//! checks its result before the next one, because the keys go to whatever window is in front: if a check fails nothing more is
//! typed, and the result says what went wrong. Nothing here presses Send: the chat opens with the text ready.

use windows::{
    core::{Interface, BSTR, VARIANT},
    Win32::{
        Foundation::{BOOL, HWND, LPARAM},
        System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
        UI::{
            Accessibility::*,
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::{EnumWindows, GetClassNameW, GetForegroundWindow, IsWindowVisible, SetForegroundWindow},
        },
    },
};

/// What happened to the link.
#[derive(Debug, PartialEq)]
pub enum Opened {
    /// Loaded in the WhatsApp tab that was already open.
    LiveTab,
    /// There was no WhatsApp tab, so it opened in a new one.
    NewTab,
    /// There is a WhatsApp tab but it could not be driven; nothing was typed after the failing step and no new tab was opened.
    Failed(String),
}

/// Is this browser tab title the WhatsApp Web tab? Titles read "WhatsApp" or "(81) WhatsApp", and the browser may add
/// " - Memory usage - 614 MB", " - Audio playing" and the like after a spaced hyphen. "WhatsApp Web" and any other page that
/// merely mentions WhatsApp are not it.
pub fn is_whatsapp_tab(title: &str) -> bool {
    let chars: Vec<char> = title.chars().collect();
    // the browser's suffix starts at the first hyphen with a space on each side (spaces may be non-breaking)
    let cut = (1..chars.len().saturating_sub(1)).find(|&i| chars[i] == '-' && chars[i - 1].is_whitespace() && chars[i + 1].is_whitespace()).unwrap_or(chars.len());
    let name: String = chars[..cut].iter().collect();
    name.trim_matches(|c: char| !c.is_alphabetic()) == "WhatsApp"
}

/// The WhatsApp tab to use out of the tabs' titles (with whether each is the selected one): the selected one if several match,
/// else the first. None when there is none.
pub fn pick_tab(tabs: &[(String, bool)]) -> Option<usize> {
    let hits: Vec<usize> = (0..tabs.len()).filter(|&i| is_whatsapp_tab(&tabs[i].0)).collect();
    hits.iter().copied().find(|&i| tabs[i].1).or_else(|| hits.first().copied())
}

/// Is `value` an address bar showing a page of this host? Used to tell the address bar apart from a text box in the page after
/// Ctrl+L, whatever language the browser is in.
pub fn shows_host(value: &str, host: &str) -> bool {
    let v = value.trim().to_lowercase();
    let v = v.strip_prefix("https://").or_else(|| v.strip_prefix("http://")).unwrap_or(&v);
    v == host || v.starts_with(&format!("{host}/")) || v.starts_with(&format!("{host}?"))
}

fn key(vk: VIRTUAL_KEY, down: bool) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP }, ..Default::default() } } }
}

fn send(keys: &[INPUT]) -> bool { unsafe { SendInput(keys, size_of::<INPUT>() as i32) as usize == keys.len() } }

fn pause(ms: u64) { std::thread::sleep(std::time::Duration::from_millis(ms)); }

/// Chromium browser windows: top-level, visible, class Chrome_WidgetWin_1 (Edge, Chrome, Brave and the rest).
fn browser_windows() -> Vec<HWND> {
    unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
        let v = &mut *(data.0 as *mut Vec<HWND>);
        let mut class = [0u16; 64];
        let n = GetClassNameW(h, &mut class) as usize;
        if IsWindowVisible(h).as_bool() && String::from_utf16_lossy(&class[..n]) == "Chrome_WidgetWin_1" { v.push(h); }
        BOOL(1)
    }
    let mut v: Vec<HWND> = vec![];
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut v as *mut _ as isize)); }
    v
}

struct Tab { window: HWND, element: IUIAutomationElement, title: String, selected: bool }

fn tab_items(uia: &IUIAutomation, window: HWND) -> Vec<Tab> {
    let mut out = vec![];
    let mut find = || -> windows::core::Result<()> {
        let root = unsafe { uia.ElementFromHandle(window)? };
        let cond = unsafe { uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_TabItemControlTypeId.0))? };
        let all = unsafe { root.FindAll(TreeScope_Descendants, &cond)? };
        for i in 0..unsafe { all.Length()? } {
            let element = unsafe { all.GetElement(i)? };
            let title = unsafe { element.CurrentName() }.map(|b| b.to_string()).unwrap_or_default();
            let selected = unsafe { element.GetCurrentPattern(UIA_SelectionItemPatternId) }.ok()
                .and_then(|p| p.cast::<IUIAutomationSelectionItemPattern>().ok()).and_then(|p| unsafe { p.CurrentIsSelected() }.ok()).is_some_and(|b| b.as_bool());
            out.push(Tab { window, element, title, selected });
        }
        Ok(())
    };
    let _ = find();
    out
}

/// The focused element if it is an address bar showing a page of `host`.
fn focused_address(uia: &IUIAutomation, host: &str) -> Option<IUIAutomationValuePattern> {
    let el = unsafe { uia.GetFocusedElement() }.ok()?;
    let pat = unsafe { el.GetCurrentPattern(UIA_ValuePatternId) }.ok()?.cast::<IUIAutomationValuePattern>().ok()?;
    let v = unsafe { pat.CurrentValue() }.ok()?.to_string();
    shows_host(&v, host).then_some(pat)
}

fn in_front(window: HWND) -> bool { unsafe { GetForegroundWindow() == window } }

/// Brings `window` to the front (a tap on Alt first, which Windows needs before it lets a background program do this).
fn bring_to_front(window: HWND) -> bool {
    send(&[key(VK_MENU, true), key(VK_MENU, false)]);
    unsafe { let _ = SetForegroundWindow(window); }
    pause(500);
    in_front(window)
}

/// Opens `url` (a web.whatsapp.com/send link) in the live WhatsApp tab, or with `new_tab` if there is none. Call from a worker thread.
pub fn open_whatsapp_web(url: &str, new_tab: impl FnOnce(&str)) -> Opened {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let Ok(uia) = (unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }) else {
        new_tab(url);
        return Opened::NewTab;
    };
    let tabs: Vec<Tab> = browser_windows().into_iter().flat_map(|w| tab_items(&uia, w)).collect();
    let Some(i) = pick_tab(&tabs.iter().map(|t| (t.title.clone(), t.selected)).collect::<Vec<_>>()) else {
        new_tab(url);
        return Opened::NewTab;
    };
    let tab = &tabs[i];
    let fail = |why: &str| Opened::Failed(why.to_string());
    // 1. switch to the tab and bring its window forward
    let Some(sel) = unsafe { tab.element.GetCurrentPattern(UIA_SelectionItemPatternId) }.ok().and_then(|p| p.cast::<IUIAutomationSelectionItemPattern>().ok()) else { return fail("I can't switch to it") };
    if unsafe { sel.Select() }.is_err() { return fail("I can't switch to it"); }
    pause(600);
    if !bring_to_front(tab.window) { return fail("I can't bring the browser to the front"); }
    // 2. Ctrl+L, then make sure the focus really is the address bar before typing into it
    if !send(&[key(VK_CONTROL, true), key(VK_L, true), key(VK_L, false), key(VK_CONTROL, false)]) { return fail("I can't press keys"); }
    pause(500);
    let Some(address) = focused_address(&uia, "web.whatsapp.com") else { return fail("the address bar didn't take focus") };
    // 3. put the link in and load it, if the browser is still in front and the focus hasn't moved
    if unsafe { address.SetValue(&BSTR::from(url)) }.is_err() { return fail("I can't type the link"); }
    pause(250);
    let typed = unsafe { address.CurrentValue() }.map(|v| v.to_string()).ok().as_deref() == Some(url);
    if !in_front(tab.window) || !typed { return fail("the browser lost focus before I loaded the link"); }
    if !send(&[key(VK_RETURN, true), key(VK_RETURN, false)]) { return fail("I can't press Enter"); }
    Opened::LiveTab
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whatsapp_tab_is_recognised_by_its_title() {
        for t in ["WhatsApp", "(81) WhatsApp", "(1) WhatsApp - Memory usage - 614 MB", "WhatsApp - Audio playing - Memory usage - 12 MB",
                  "(8)\u{a0}WhatsApp\u{a0}-\u{a0}Memory usage\u{a0}-\u{a0}614 MB", "  WhatsApp  ", "(1,204) WhatsApp"] {
            assert!(is_whatsapp_tab(t), "{t:?}");
        }
        for t in ["WhatsApp Web", "AnasGrzor/whatsapp-bot: A bot", "How to use WhatsApp - YouTube", "WhatsApp Business", "Page not found - WhatsApp", "", " - ", "Hans Zimmer - Interstellar - YouTube"] {
            assert!(!is_whatsapp_tab(t), "{t:?}");
        }
    }

    #[test]
    fn the_live_tab_is_the_selected_whatsapp_tab_else_the_first() {
        let t = |s: &str, sel: bool| (s.to_string(), sel);
        assert_eq!(pick_tab(&[]), None);
        assert_eq!(pick_tab(&[t("YouTube", true), t("GitHub", false)]), None, "no WhatsApp tab: open a new one");
        assert_eq!(pick_tab(&[t("YouTube", true), t("(81) WhatsApp - Memory usage - 600 MB", false)]), Some(1));
        assert_eq!(pick_tab(&[t("WhatsApp", false), t("(3) WhatsApp", true)]), Some(1), "of two, the one in front");
        assert_eq!(pick_tab(&[t("WhatsApp", false), t("(3) WhatsApp", false)]), Some(0), "else the first");
        assert_eq!(pick_tab(&[t("WhatsApp Web", true), t("Docs", false)]), None, "the 'Use here' page is not the live session");
    }

    #[test]
    fn the_address_bar_is_told_from_a_page_text_box_by_what_it_shows() {
        assert!(shows_host("https://web.whatsapp.com", "web.whatsapp.com"));
        assert!(shows_host("https://web.whatsapp.com/", "web.whatsapp.com"));
        assert!(shows_host("  HTTPS://WEB.WHATSAPP.COM/send?phone=1&text=hi ", "web.whatsapp.com"));
        assert!(shows_host("web.whatsapp.com", "web.whatsapp.com"), "browsers hide the scheme");
        assert!(!shows_host("https://web.whatsapp.com.evil.example/", "web.whatsapp.com"), "a look-alike host is not it");
        assert!(!shows_host("Hi, this message is from Gojo.", "web.whatsapp.com"), "a message box is not the address bar");
        assert!(!shows_host("", "web.whatsapp.com"));
        assert!(!shows_host("https://www.google.com/search?q=web.whatsapp.com", "web.whatsapp.com"));
    }

    #[test]
    fn the_outcomes_are_distinct() {
        assert_ne!(Opened::LiveTab, Opened::NewTab);
        assert_ne!(Opened::NewTab, Opened::Failed("x".into()));
        assert_eq!(Opened::Failed("x".into()), Opened::Failed("x".into()));
    }
}
