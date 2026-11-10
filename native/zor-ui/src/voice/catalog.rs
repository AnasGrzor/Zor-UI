//! The providers the app knows how to install. Each entry is a manifest. Sizes and hashes were taken from the
//! files themselves (Piper release 2023.11.14-2, and the voice's published etag), so a download that differs
//! is rejected before it is installed.

use serde_json::{json, Value};

const PIPER_ZIP_URL: &str = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip";
const PIPER_ZIP_SHA: &str = "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea";
const PIPER_ZIP_SIZE: u64 = 22_477_236;
const VOICE_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium";

/// The Piper manifest: the Windows build and the Lessac (US English, medium) voice.
pub fn piper() -> Value {
    json!({
        "id": "piper",
        "name": "Piper (local voices)",
        "version": "2023.11.14-2",
        "adapter": "piper",
        "command": "piper/piper.exe",
        "capabilities": { "speed": true },
        "package": { "url": PIPER_ZIP_URL, "sha256": PIPER_ZIP_SHA, "size_bytes": PIPER_ZIP_SIZE },
        "models": [{
            "id": "en_US-lessac-medium",
            "name": "Lessac, US English (medium)",
            "files": [
                { "file": "en_US-lessac-medium.onnx",
                  "url": format!("{VOICE_BASE}/en_US-lessac-medium.onnx"),
                  "sha256": "5efe09e69902187827af646e1a6e9d269dee769f9877d17b16b1b46eeaaf019f",
                  "size_bytes": 63_201_294 },
                { "file": "en_US-lessac-medium.onnx.json",
                  "url": format!("{VOICE_BASE}/en_US-lessac-medium.onnx.json"),
                  "sha256": "efe19c417bed055f2d69908248c6ba650fa135bc868b0e6abb3da181dab690a0",
                  "size_bytes": 4_885 }
            ]
        }]
    })
}

/// Every provider the app can install, by id.
pub fn entries() -> Vec<Value> { vec![piper()] }

pub fn find(id: &str) -> Option<Value> { entries().into_iter().find(|v| v["id"] == id) }

#[cfg(test)]
mod tests {
    use super::super::manifest::{self, Adapter};
    use super::*;

    #[test]
    fn the_piper_entry_parses_and_is_the_piper_adapter() {
        let m = manifest::parse(&piper()).unwrap();
        assert_eq!(m.adapter, Adapter::PiperCli);
        assert_eq!(m.command, "piper/piper.exe");
        assert_eq!(m.models[0].id, "en_US-lessac-medium");
        assert_eq!(m.models[0].files.len(), 2, "the .onnx and its config");
    }

    #[test]
    fn the_sizes_are_the_measured_ones() {
        // measured on the downloaded files; a change here means the release or voice changed
        let m = manifest::parse(&piper()).unwrap();
        assert_eq!(m.package.as_ref().unwrap().size_bytes, 22_477_236);
        assert_eq!(m.models[0].size_bytes(), 63_201_294 + 4_885);
    }

    #[test]
    fn find_knows_piper_and_nothing_else() {
        assert!(find("piper").is_some());
        assert!(find("nope").is_none());
    }
}
