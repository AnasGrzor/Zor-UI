//! How a tool step reads in the chat window: an icon and a short line in plain words ("Searched the web for lofi beats"), the
//! way Claude shows its tool calls. The step's own record (`plan::Entry`) keeps the exact call for the model.

use super::Call;
use crate::assist::short;

/// The longest argument a label quotes, in characters.
const ARG: usize = 60;

/// (Material icon, label) for one tool call.
pub fn tool_label(c: &Call) -> (&'static str, String) {
    let q = |s: &str| format!("\u{201c}{}\u{201d}", short(s.trim(), ARG));
    let file = |p: &str| short(std::path::Path::new(p.trim()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.trim().to_string()).as_str(), ARG);
    match c {
        Call::Search(s) | Call::WebSearch(s) => ("search", format!("Searched the web for {}", q(s))),
        Call::ReadUrl(u) | Call::Url(u) => ("public", format!("{} {}", if matches!(c, Call::Url(_)) { "Opened" } else { "Read" }, short(u.trim(), ARG))),
        Call::Open(a) => ("open_in_new", format!("Opened {}", short(a.trim(), ARG))),
        Call::FindApp(a) => ("apps", format!("Looked for the app {}", q(a))),
        Call::Play { query, service } => ("music_note", if service.trim().is_empty() { format!("Played {}", q(query)) } else { format!("Played {} on {}", q(query), service.trim()) }),
        Call::Media(a) => ("play_pause", format!("Media: {}", a.trim())),
        Call::Message { to, via, .. } => ("chat", if via.trim().is_empty() { format!("Wrote a message to {}", to.trim()) } else { format!("Wrote a message to {} on {}", to.trim(), via.trim()) }),
        Call::Volume(v) => ("volume_up", format!("Set the volume: {}", v.trim())),
        Call::Brightness(v) => ("brightness_6", format!("Set the brightness: {}", v.trim())),
        Call::Radio { which, state } => ("wifi", format!("Turned {} {}", which.trim(), state.trim())),
        Call::Power(a) => ("power_settings_new", format!("Power: {}", a.trim())),
        Call::Clip(_) => ("content_paste", "Changed the clipboard".into()),
        Call::FindFile(f) => ("manage_search", format!("Looked for the file {}", q(f))),
        Call::OpenFile(f) => ("description", format!("Opened {}", file(f))),
        Call::SeeScreen(_) => ("visibility", "Looked at the screen".into()),
        Call::TakeOver(t) => ("mouse", format!("Took over the mouse and keyboard: {}", short(t.trim(), ARG))),
        Call::Wallpaper(_) => ("wallpaper", "Changed the wallpaper".into()),
        Call::Remember(f) => ("bookmark_add", format!("Remembered {}", q(f))),
        Call::Forget(f) => ("bookmark_remove", format!("Forgot {}", q(f))),
        Call::SessionSearch(s) => ("history", format!("Searched past chats for {}", q(s))),
        Call::SearchMemory(s) => ("psychology", format!("Searched memory for {}", q(s))),
        Call::Browser { tool, .. } => ("travel_explore", format!("Browser: {}", tool.replace('_', " "))),
        Call::ListTree(d) => ("folder_open", if d.trim().is_empty() { "Listed the project's files".into() } else { format!("Listed {}", short(d.trim(), ARG)) }),
        Call::ReadFile(f) => ("description", format!("Read {}", file(f))),
        Call::SearchCode(s) => ("code", format!("Searched the code for {}", q(s))),
        Call::GitStatus => ("commit", "Checked git status".into()),
        Call::Metrics(m) => ("monitoring", if m.trim().is_empty() { "Read the PC's load".into() } else { format!("Read the PC's {}", m.trim()) }),
        Call::RunCheck(n) => ("fact_check", format!("Ran {}", n.trim())),
        Call::StartAgent { agent, task, .. } => ("smart_toy", format!("Started {} on {}", if agent.trim().is_empty() { "a coding agent" } else { agent.trim() }, q(task))),
        Call::RunCommand(cmd) => ("terminal", format!("Ran {}", q(cmd))),
        Call::WriteFile { path, .. } => ("edit_document", format!("Wrote {}", file(path))),
        Call::DeleteFile(p) => ("delete", format!("Deleted {}", file(p))),
        Call::MoveFile { from, to } => ("drive_file_move", format!("Moved {} to {}", file(from), short(to.trim(), ARG))),
        Call::EditFile { path, .. } => ("edit", format!("Edited {}", file(path))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_reads_as_plain_words_with_its_argument() {
        assert_eq!(tool_label(&Call::WebSearch("lofi beats".into())), ("search", "Searched the web for \u{201c}lofi beats\u{201d}".into()));
        assert_eq!(tool_label(&Call::ReadFile("C:\\proj\\src\\main.rs".into())).1, "Read main.rs");
        assert_eq!(tool_label(&Call::Open("Steam".into())).1, "Opened Steam");
        assert_eq!(tool_label(&Call::GitStatus).1, "Checked git status");
        let long = tool_label(&Call::RunCommand("x".repeat(200))).1;
        assert!(long.chars().count() < 80, "{long}");
    }
}
