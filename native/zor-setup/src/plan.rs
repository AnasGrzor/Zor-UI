//! Pure logic of the installer: what install.ps1 gets as arguments, how its output maps to progress, and the
//! text of the last page. No Win32 here, so the unit tests cover all of it.

/// What the user picked on the options page.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Options {
    pub win_key: bool,
    pub hide_taskbar: bool,
    pub lively: bool,
    pub user: String,
    pub wallpaper: String,
}

impl Options {
    /// The parameters of install.ps1. Empty fields are left out so the script keeps its own defaults.
    pub fn script_args(&self) -> Vec<String> {
        let mut a = vec![];
        if self.win_key {
            a.extend(["-Mod".into(), "win".into()]);
        }
        if self.hide_taskbar {
            a.push("-HideTaskbar".into());
        }
        if self.lively {
            a.push("-WithLively".into());
        }
        if !self.wallpaper.is_empty() {
            a.extend(["-Wallpaper".into(), self.wallpaper.clone()]);
        }
        if !self.user.is_empty() {
            a.extend(["-UserName".into(), self.user.clone()]);
        }
        a
    }
}

/// The name to pass as -UserName: none when the field still holds the Windows user name, because install.ps1 then keeps
/// the name a returning user already set in Settings.
pub fn user_arg(typed: &str, windows_user: &str) -> String {
    let t = typed.trim();
    if t.eq_ignore_ascii_case(windows_user) { String::new() } else { t.to_string() }
}

/// install.ps1 prints each phase as "==> title". Returns the title.
pub fn step_title(line: &str) -> Option<&str> {
    line.trim().strip_prefix("==> ").map(str::trim).filter(|t| !t.is_empty())
}

/// How many "==>" phases a typical install prints (the optional ones are the old-name move, the Zebar removal and the taskbar).
const TYPICAL_STEPS: usize = 8;

/// Progress bar position after `steps` phases started. Stops at 95 so the bar never shows done before the script exits.
pub fn percent(steps: usize) -> u32 {
    ((steps * 100 / TYPICAL_STEPS) as u32).min(95)
}

/// Quotes `s` as a PowerShell single-quoted string.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The shortcut list on the last page, for the modifier the user picked.
pub fn shortcuts(win_key: bool) -> String {
    let (m, overview) = if win_key { ("Win", "Win+Tab") } else { ("Alt", "Alt+`") };
    format!(
        "{m}+A   left sidebar (media, AI chat, translator)\r\n{m}+N   right sidebar (toggles, notifications, calendar)\r\n\
         {m}+I   settings (colors, wallpapers, API keys)\r\n{overview}   overview (search and move windows)\r\n\
         Alt+Space   launcher\r\n{m}+Ctrl+T   change wallpaper and colors\r\n{m}+1..0   workspaces"
    )
}

/// The command Apps & Features runs to uninstall.
pub fn uninstall_command(exe: &str) -> String {
    format!("\"{exe}\" --uninstall")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_pass_no_arguments() {
        assert!(Options::default().script_args().is_empty());
    }

    #[test]
    fn every_option_becomes_its_parameter() {
        let o = Options { win_key: true, hide_taskbar: true, lively: true, user: "Ana".into(), wallpaper: r"C:\p\w.jpg".into() };
        assert_eq!(
            o.script_args(),
            ["-Mod", "win", "-HideTaskbar", "-WithLively", "-Wallpaper", r"C:\p\w.jpg", "-UserName", "Ana"]
        );
    }

    #[test]
    fn user_name_equal_to_windows_user_is_not_passed() {
        assert_eq!(user_arg(" Anas ", "anas"), "");
        assert_eq!(user_arg("Ana", "anas"), "Ana");
        assert_eq!(user_arg("   ", "anas"), "");
    }

    #[test]
    fn step_lines_are_recognised() {
        assert_eq!(step_title("==> Copying files to C:\\x"), Some("Copying files to C:\\x"));
        assert_eq!(step_title("  ==> Starting GlazeWM  "), Some("Starting GlazeWM"));
        assert_eq!(step_title("    already installed"), None);
        assert_eq!(step_title("==> "), None);
        assert_eq!(step_title("WARNING: ==> x"), None);
    }

    #[test]
    fn progress_grows_and_never_reaches_100() {
        assert_eq!(percent(0), 0);
        assert!(percent(3) > percent(2));
        assert_eq!(percent(8), 95);
        assert_eq!(percent(40), 95);
    }

    #[test]
    fn quoting_doubles_single_quotes() {
        assert_eq!(ps_quote(r"C:\Users\O'Neil\a.zip"), r"'C:\Users\O''Neil\a.zip'");
    }

    #[test]
    fn shortcuts_follow_the_modifier() {
        assert!(shortcuts(false).starts_with("Alt+A"));
        assert!(shortcuts(true).contains("Win+Tab"));
        assert!(!shortcuts(true).contains("Alt+`"));
    }

    #[test]
    fn uninstall_command_quotes_the_path() {
        assert_eq!(uninstall_command(r"C:\a b\s.exe"), r#""C:\a b\s.exe" --uninstall"#);
    }
}
