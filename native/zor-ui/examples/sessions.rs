//! Diagnostic: prints the media sessions Windows exposes, and which one it calls current.

use windows::{Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager, Win32::System::Com::*};

fn main() -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mgr = Manager::RequestAsync()?.get()?;
        let cur = mgr.GetCurrentSession().ok();
        let cur_id = cur.as_ref().and_then(|s| s.SourceAppUserModelId().ok()).map(|h| h.to_string());
        for s in mgr.GetSessions()? {
            let id = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
            let props = s.TryGetMediaPropertiesAsync()?.get()?;
            let title = props.Title().map(|t| t.to_string()).unwrap_or_default();
            let artist = props.Artist().map(|a| a.to_string()).unwrap_or_default();
            let art = props.Thumbnail().ok().and_then(|r| r.OpenReadAsync().ok()).and_then(|a| a.get().ok()).and_then(|st| st.Size().ok());
            let status = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).map(|st| format!("{st:?}")).unwrap_or_default();
            println!("{id:<28} current={:<5} {status:<9} art={art:?} {title} - {artist}", cur_id.as_deref() == Some(id.as_str()));
        }
        println!("current: {cur_id:?}");
    }
    Ok(())
}
