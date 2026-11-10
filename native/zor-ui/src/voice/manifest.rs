//! Provider manifests: what a provider is, how to start it, what it can do, and where its files come from.
//! Every field is checked here, so nothing is downloaded or run from a manifest that did not pass.

use super::contract::{ProviderCapabilities, VoiceCapabilities};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A file to fetch: where from, the SHA-256 it must have, and its exact size.
#[derive(Clone, Debug, PartialEq)]
pub struct Download {
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// One file of a model, placed at `<model folder>/<file>`.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelFile {
    pub file: String,
    pub download: Download,
}

/// A model is one or more files (Piper needs the .onnx and its .json). Its size is the total.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelSpec {
    pub id: String,
    pub name: String,
    pub files: Vec<ModelFile>,
}

impl ModelSpec {
    /// Total bytes of every file: the figure the approval prompt shows.
    pub fn size_bytes(&self) -> u64 { self.files.iter().map(|f| f.download.size_bytes).sum() }
}

/// How the app talks to the provider's executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    /// Newline-delimited JSON requests (src/voice/sidecar.rs).
    JsonSidecar,
    /// The Piper command line: text in on stdin, a WAV file out (src/voice/piper.rs).
    PiperCli,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Executable, relative to the provider's install folder.
    pub command: String,
    pub args: Vec<String>,
    pub adapter: Adapter,
    pub capabilities: ProviderCapabilities,
    /// The runtime package (a zip, unpacked into the install folder). None for a provider that ships in-tree.
    pub package: Option<Download>,
    pub models: Vec<ModelSpec>,
}

/// File name a provider's manifest is saved under in its install folder.
pub const FILE: &str = "provider.json";

pub fn parse(v: &Value) -> Result<Manifest, String> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(String::from).ok_or(format!("manifest needs a string '{k}'"));
    let id = text("id")?;
    if !valid_id(&id) { return Err(format!("provider id '{id}' must be 1-40 lowercase letters, digits or '-'")); }
    let command = text("command")?;
    if !safe_relative(&command) { return Err(format!("command '{command}' must be a path inside the provider folder")); }
    let args = v.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
    let adapter = match v.get("adapter").and_then(Value::as_str).unwrap_or("json") {
        "json" => Adapter::JsonSidecar,
        "piper" => Adapter::PiperCli,
        other => return Err(format!("unknown adapter '{other}'")),
    };
    let caps = v.get("capabilities").unwrap_or(&Value::Null);
    let flag = |k: &str| caps.get(k).and_then(Value::as_bool).unwrap_or(false);
    let capabilities = ProviderCapabilities {
        cloning: flag("cloning"),
        design: flag("design"),
        streaming: flag("streaming"),
        voice: VoiceCapabilities { speed: flag("speed"), pitch: flag("pitch"), emotion: flag("emotion") },
    };
    let package = v.get("package").filter(|p| !p.is_null()).map(download).transpose()?;
    let mut models = Vec::new();
    for m in v.get("models").and_then(Value::as_array).into_iter().flatten() {
        let mid = m.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if !valid_model_id(&mid) { return Err(format!("model id '{mid}' must be 1-80 letters, digits, '-' or '_'")); }
        let mut files = Vec::new();
        for f in m.get("files").and_then(Value::as_array).into_iter().flatten() {
            let file = f.get("file").and_then(Value::as_str).unwrap_or("").to_string();
            if !safe_file(&file) { return Err(format!("model '{mid}' file '{file}' must be a plain file name")); }
            files.push(ModelFile { file, download: download(f)? });
        }
        if files.is_empty() { return Err(format!("model '{mid}' has no files")); }
        models.push(ModelSpec {
            name: m.get("name").and_then(Value::as_str).unwrap_or(&mid).to_string(),
            id: mid,
            files,
        });
    }
    Ok(Manifest {
        id,
        name: text("name")?,
        version: v.get("version").and_then(Value::as_str).unwrap_or("0").to_string(),
        command,
        args,
        adapter,
        capabilities,
        package,
        models,
    })
}

fn download(v: &Value) -> Result<Download, String> {
    let url = v.get("url").and_then(Value::as_str).unwrap_or("");
    if !url.starts_with("https://") { return Err("downloads must use https://".into()); }
    let sha256 = v.get("sha256").and_then(Value::as_str).unwrap_or("").to_string();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err("sha256 must be 64 lowercase hex characters".into());
    }
    let size_bytes = v.get("size_bytes").and_then(Value::as_u64).filter(|&s| s > 0).ok_or("size_bytes must be a positive number")?;
    Ok(Download { url: url.into(), sha256, size_bytes })
}

pub fn valid_id(s: &str) -> bool {
    (1..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Model ids name a folder, and Piper's own voice ids use capitals and underscores (en_US-lessac-medium).
pub fn valid_model_id(s: &str) -> bool {
    (1..=80).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn safe_file(s: &str) -> bool {
    !s.is_empty() && !s.contains(['/', '\\', ':']) && s != "." && s != ".."
}

fn safe_relative(s: &str) -> bool {
    !s.is_empty() && !s.starts_with(['/', '\\']) && !s.contains(':') && !s.split(['/', '\\']).any(|p| p == "..")
}

/// Manifests found in `root/<folder>/provider.json`, each either parsed or rejected with the reason.
pub fn discover(root: &Path) -> Vec<(PathBuf, Result<Manifest, String>)> {
    let Ok(rd) = std::fs::read_dir(root) else { return vec![] };
    let mut dirs: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.into_iter().filter_map(|dir| {
        let file = dir.join(FILE);
        if !file.is_file() { return None; }
        let parsed = std::fs::read_to_string(&file).map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
            .and_then(|v| parse(&v));
        Some((dir, parsed))
    }).collect()
}

#[cfg(test)]
pub fn sample() -> Value {
    serde_json::json!({
        "id": "kitten-local", "name": "Local sidecar", "version": "1.2.0",
        "command": "bin/tts.exe", "args": ["--stdio"],
        "capabilities": { "speed": true, "pitch": false },
        "package": { "url": "https://example.com/pkg.zip", "sha256": "a".repeat(64), "size_bytes": 1000 },
        "models": [{ "id": "small", "name": "Small", "files": [
            { "file": "small.onnx", "url": "https://example.com/small.onnx", "sha256": "b".repeat(64), "size_bytes": 5000 },
            { "file": "small.onnx.json", "url": "https://example.com/small.json", "sha256": "c".repeat(64), "size_bytes": 500 }
        ]}]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_good_manifest_parses_with_its_downloads_and_capabilities() {
        let m = parse(&sample()).unwrap();
        assert_eq!((m.id.as_str(), m.version.as_str()), ("kitten-local", "1.2.0"));
        assert!(m.capabilities.voice.speed && !m.capabilities.voice.pitch && !m.capabilities.cloning);
        assert_eq!(m.adapter, Adapter::JsonSidecar, "json is the default adapter");
        assert_eq!(m.models[0].files.len(), 2, "a model keeps all its files");
        assert_eq!(m.models[0].size_bytes(), 5500, "the approval shows the total");
        assert!(m.package.is_some());
    }

    #[test]
    fn the_piper_adapter_is_chosen_by_name_and_unknown_ones_are_refused() {
        let mut v = sample();
        v["adapter"] = "piper".into();
        assert_eq!(parse(&v).unwrap().adapter, Adapter::PiperCli);
        v["adapter"] = "exe-of-doom".into();
        assert!(parse(&v).is_err());
    }

    #[test]
    fn bad_ids_commands_and_files_are_refused() {
        let mut v = sample();
        v["id"] = "../evil".into();
        assert!(parse(&v).is_err());
        let mut v = sample();
        v["command"] = "..\\..\\evil.exe".into();
        assert!(parse(&v).is_err(), "no escaping the install folder");
        let mut v = sample();
        v["command"] = "C:\\Windows\\cmd.exe".into();
        assert!(parse(&v).is_err(), "no absolute paths");
        let mut v = sample();
        v["models"][0]["files"][0]["file"] = "../x".into();
        assert!(parse(&v).is_err());
        let mut v = sample();
        v["models"][0]["files"] = serde_json::json!([]);
        assert!(parse(&v).is_err(), "a model needs at least one file");
    }

    #[test]
    fn downloads_must_be_https_with_a_real_hash_and_size() {
        let mut v = sample();
        v["models"][0]["files"][0]["url"] = "http://example.com/m.onnx".into();
        assert!(parse(&v).is_err(), "no plain http");
        let mut v = sample();
        v["models"][0]["files"][0]["sha256"] = "ABC".into();
        assert!(parse(&v).is_err());
        let mut v = sample();
        v["models"][0]["files"][0]["size_bytes"] = 0.into();
        assert!(parse(&v).is_err(), "size must be known before download is approved");
    }

    #[test]
    fn discover_reads_each_folder_and_reports_bad_manifests() {
        let root = std::env::temp_dir().join(format!("zorui-manifest-test-{}", super::super::profile::new_id()));
        std::fs::create_dir_all(root.join("good")).unwrap();
        std::fs::create_dir_all(root.join("bad")).unwrap();
        std::fs::write(root.join("good").join(FILE), sample().to_string()).unwrap();
        std::fs::write(root.join("bad").join(FILE), "{ not json").unwrap();
        let found = discover(&root);
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(found.len(), 2);
        assert!(found[0].1.is_err(), "bad folder reported");
        assert_eq!(found[1].1.as_ref().unwrap().id, "kitten-local");
    }
}
