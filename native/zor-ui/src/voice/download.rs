//! Shared download status survives closing and reopening Settings.
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct Progress {
    pub running: bool,
    pub file: String,
    pub received: u64,
    pub total: u64,
    pub message: String,
}

static PROGRESS: Mutex<Progress> = Mutex::new(Progress {
    running: false, file: String::new(), received: 0, total: 0, message: String::new(),
});

pub fn progress() -> Progress { PROGRESS.lock().unwrap_or_else(|p| p.into_inner()).clone() }
fn update(f: impl FnOnce(&mut Progress)) { f(&mut PROGRESS.lock().unwrap_or_else(|p| p.into_inner())); }

pub fn clear_cache(manifest: &super::manifest::Manifest) {
    let root = crate::conf::data_dir().join("voice-downloads");
    for d in manifest.package.iter().chain(manifest.models.iter().flat_map(|m| m.files.iter().map(|f| &f.download))) {
        let _ = std::fs::remove_file(root.join(format!("{}.part", super::sha256::hex(d.url.as_bytes()))));
    }
}

pub fn begin() {
    update(|p| *p = Progress { running: true, message: "Checking installed voice files".into(), ..Default::default() });
}

pub fn finish(result: &Result<(), String>) {
    update(|p| {
        p.running = false;
        p.message = match result {
            Ok(()) => "Voices installed. JARVIS is ready in the voice library".into(),
            Err(e) => format!("{e}. Select Update voices to retry"),
        };
    });
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.received as f64 / self.total as f64).clamp(0.0, 1.0) as f32 }
    }

    pub fn label(&self) -> String {
        if !self.running || self.file.is_empty() { return self.message.clone(); }
        let mb = |n| n as f64 / 1_048_576.0;
        if self.received >= self.total && self.total > 0 {
            format!("Verifying {} ({:.1} MB)", self.file, mb(self.total))
        } else if self.received == 0 {
            format!("Connecting: {} ({:.1} MB)", self.file, mb(self.total))
        } else {
            format!("{}: {:.1} / {:.1} MB ({:.0}%)", self.file, mb(self.received), mb(self.total), self.fraction() * 100.0)
        }
    }
}

/// Curl writes a resumable partial file; Settings reads its size while it runs.
/// The lifecycle still verifies the exact size and SHA-256 before installation.
pub fn fetch(url: &str) -> Option<Vec<u8>> {
    let manifest = super::manifest::parse(&super::catalog::piper()).ok()?;
    let total = manifest.package.iter().chain(manifest.models.iter().flat_map(|m| m.files.iter().map(|f| &f.download)))
        .find(|d| d.url == url).map_or(0, |d| d.size_bytes);
    let name = url.rsplit('/').next().unwrap_or("voice model");
    let root = crate::conf::data_dir().join("voice-downloads");
    std::fs::create_dir_all(&root).ok()?;
    let path = root.join(format!("{}.part", super::sha256::hex(url.as_bytes())));
    update(|p| { p.file = name.into(); p.total = total; p.received = 0; });
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() as u64 == total {
            let expected = manifest.package.iter().chain(manifest.models.iter().flat_map(|m| m.files.iter().map(|f| &f.download)))
                .find(|d| d.url == url)?;
            if super::sha256::hex(&bytes) == expected.sha256 { return Some(bytes); }
            let _ = std::fs::remove_file(&path);
        }
    }
    let request_url = if url.starts_with("https://huggingface.co/") && !url.contains('?') {
        format!("{url}?download=true")
    } else { url.to_string() };
    let started = Instant::now();
    let mut child = crate::bridge::common::hidden("curl.exe")
        .args(["--fail", "--location", "--silent", "--show-error", "--connect-timeout", "30",
            "--max-time", "7200", "--retry", "2", "--retry-delay", "2", "--continue-at", "-", "--output"])
        .arg(&path).arg(&request_url)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().ok()?;
    loop {
        let received = std::fs::metadata(&path).map_or(0, |m| m.len());
        update(|p| p.received = received);
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    crate::log::line(&format!("voice download failed: {name}, {received}/{total} bytes after {:?}, {status}", started.elapsed()));
                    return None;
                }
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(_) => { let _ = child.kill(); let _ = child.wait(); return None; }
        }
    }
    let bytes = std::fs::read(&path).ok()?;
    // Keep a valid download for a retry if a later file fails.
    let expected = manifest.package.iter().chain(manifest.models.iter().flat_map(|m| m.files.iter().map(|f| &f.download)))
        .find(|d| d.url == url)?;
    if bytes.len() as u64 != total || super::sha256::hex(&bytes) != expected.sha256 {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_distinguishes_connecting_transferring_verifying_and_failure() {
        let mut p = Progress { running: true, file: "jarvis-high.onnx".into(), total: 100, ..Default::default() };
        assert!(p.label().starts_with("Connecting:"));
        p.received = 50;
        assert_eq!(p.fraction(), 0.5);
        assert!(p.label().ends_with("(50%)"));
        p.received = 100;
        assert!(p.label().starts_with("Verifying"));
        p.running = false;
        p.message = "Download failed. Retry".into();
        assert_eq!(p.label(), p.message);
    }
}
