//! Plays normalized PCM through Windows' sound player, so providers that return samples are audible.
//! The samples go to a temporary WAV that is deleted afterwards, whatever happens.

use super::contract::{Cancel, ProviderError};
use super::{process, profile::new_id, wav};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

const SCRIPT: &str = "(New-Object System.Media.SoundPlayer $env:ZOR_WAV).PlaySync()";

pub fn play(samples: &[f32], rate: u32, cancel: &Cancel) -> Result<(), ProviderError> {
    if cancel.is_cancelled() { return Err(ProviderError::Cancelled); }
    let path = std::env::temp_dir().join(format!("zorui-voice-{}.wav", new_id()));
    std::fs::write(&path, wav::encode(rate, 1, &wav::to_i16(samples)))
        .map_err(|e| ProviderError::Other(format!("could not write audio: {e}")))?;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let result = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("ZOR_WAV", &path)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn()
        .map_err(|e| ProviderError::Other(format!("could not start the sound player: {e}")))
        .and_then(|mut child| process::wait_cancellable(&mut child, cancel, "the sound player"));
    std::fs::remove_file(&path).ok();
    result
}
