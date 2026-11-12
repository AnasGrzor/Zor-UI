//! Installing and removing providers and their models. Nothing is downloaded without an approval that names
//! the item and its total size. Every file is fetched and checked against its exact size and SHA-256 before
//! anything is written, so a model is either fully installed or not at all. What is installed is recorded in
//! a small JSON file.

use super::manifest::{Download, Manifest, ModelSpec, FILE};
use super::sha256;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub enum InstallError {
    /// No approval names this item, or its size differs from the manifest's total.
    NotApproved(String),
    Download(String),
    /// Wrong size or hash: nothing from this item is installed.
    Corrupt(String),
    Io(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::NotApproved(w) => write!(f, "{w} was not approved for download"),
            InstallError::Download(m) => write!(f, "download failed: {m}"),
            InstallError::Corrupt(m) => write!(f, "download rejected: {m}"),
            InstallError::Io(m) => write!(f, "could not write files: {m}"),
        }
    }
}

/// The user said yes to downloading `what` (a provider id or a model id) of `size_bytes` in total.
/// Built from the size the prompt showed, so the approval cannot outlive a manifest change.
#[derive(Clone, Debug, PartialEq)]
pub struct Approval {
    pub what: String,
    pub size_bytes: u64,
}

impl Approval {
    fn find<'a>(approvals: &'a [Approval], what: &str, size: u64) -> Result<&'a Approval, InstallError> {
        approvals.iter().find(|a| a.what == what && a.size_bytes == size)
            .ok_or_else(|| InstallError::NotApproved(what.into()))
    }
}

/// Fetches `d` and checks its exact size and SHA-256. Nothing is written here.
fn fetch_checked(d: &Download, fetch: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Vec<u8>, InstallError> {
    let bytes = fetch(&d.url).ok_or_else(|| InstallError::Download(d.url.clone()))?;
    if bytes.len() as u64 != d.size_bytes {
        return Err(InstallError::Corrupt(format!("{} bytes, expected {}", bytes.len(), d.size_bytes)));
    }
    let hash = sha256::hex(&bytes);
    if hash != d.sha256 { return Err(InstallError::Corrupt(format!("sha256 {hash} does not match"))); }
    Ok(bytes)
}

/// Writes `bytes` to `path` through a `.part` file, so a crash never leaves a half file under the real name.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), InstallError> {
    let part = path.with_extension("part");
    std::fs::write(&part, bytes).map_err(|e| InstallError::Io(e.to_string()))?;
    std::fs::rename(&part, path).map_err(|e| InstallError::Io(e.to_string()))
}

/// Downloads and verifies every file of `model`, then writes them all into `models_root/<provider>/<model>/`.
pub fn install_model(
    store: &mut Store,
    models_root: &Path,
    provider: &str,
    model: &ModelSpec,
    approval: &Approval,
    fetch: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<PathBuf, InstallError> {
    Approval::find(std::slice::from_ref(approval), &model.id, model.size_bytes())?;
    let fetched: Vec<(String, Vec<u8>)> = model.files.iter()
        .map(|f| fetch_checked(&f.download, fetch).map(|b| (f.file.clone(), b)))
        .collect::<Result<_, _>>()?;
    let dir = models_root.join(provider).join(&model.id);
    std::fs::create_dir_all(&dir).map_err(|e| InstallError::Io(e.to_string()))?;
    for (file, bytes) in &fetched { write_atomic(&dir.join(file), bytes)?; }
    store.models.retain(|m| !(m.provider == provider && m.id == model.id));
    store.models.push(InstalledModel { provider: provider.into(), id: model.id.clone(), dir: dir.clone(), sha256: model.files[0].download.sha256.clone() });
    Ok(dir)
}

/// Installs a provider: its package (unpacked by `extract`) and its manifest, into `providers_root/<id>`.
/// A provider with no package is just its manifest.
pub fn install_provider(
    store: &mut Store,
    providers_root: &Path,
    m: &Manifest,
    raw: &Value,
    approval: &Approval,
    fetch: &dyn Fn(&str) -> Option<Vec<u8>>,
    extract: &dyn Fn(&Path, &Path) -> Result<(), String>,
) -> Result<PathBuf, InstallError> {
    let dir = providers_root.join(&m.id);
    let bytes = match &m.package {
        Some(pkg) => {
            Approval::find(std::slice::from_ref(approval), &m.id, pkg.size_bytes)?;
            Some(fetch_checked(pkg, fetch)?)
        }
        None => None,
    };
    std::fs::create_dir_all(&dir).map_err(|e| InstallError::Io(e.to_string()))?;
    if let Some(bytes) = bytes {
        let zip = dir.join("package.zip");
        write_atomic(&zip, &bytes)?;
        let unpacked = extract(&zip, &dir).map_err(InstallError::Io);
        std::fs::remove_file(&zip).ok();
        unpacked?;
    }
    std::fs::write(dir.join(FILE), raw.to_string()).map_err(|e| InstallError::Io(e.to_string()))?;
    store.providers.retain(|p| p.id != m.id);
    store.providers.push(InstalledProvider { id: m.id.clone(), version: m.version.clone(), dir: dir.clone() });
    Ok(dir)
}

/// Installs a whole manifest: the package and every model. Each item needs its own approval, and a missing
/// one stops the install before anything is downloaded.
#[allow(clippy::too_many_arguments)] // the args mirror the callers; a signature refactor is out of scope
pub fn install_manifest(
    store: &mut Store,
    providers_root: &Path,
    models_root: &Path,
    m: &Manifest,
    raw: &Value,
    approvals: &[Approval],
    fetch: &dyn Fn(&str) -> Option<Vec<u8>>,
    extract: &dyn Fn(&Path, &Path) -> Result<(), String>,
) -> Result<(), InstallError> {
    if let Some(pkg) = &m.package { Approval::find(approvals, &m.id, pkg.size_bytes)?; }
    for model in &m.models { Approval::find(approvals, &model.id, model.size_bytes())?; }
    let package_approval = approvals.iter().find(|a| a.what == m.id).cloned().unwrap_or(Approval { what: m.id.clone(), size_bytes: 0 });
    install_provider(store, providers_root, m, raw, &package_approval, fetch, extract)?;
    for model in &m.models {
        let a = approvals.iter().find(|a| a.what == model.id).cloned().ok_or_else(|| InstallError::NotApproved(model.id.clone()))?;
        install_model(store, models_root, &m.id, model, &a, fetch)?;
    }
    Ok(())
}

/// Removes a provider's folder and its record. Its models stay unless `with_models` is set, so a reinstall
/// does not download them again.
pub fn remove_provider(store: &mut Store, providers_root: &Path, models_root: &Path, id: &str, with_models: bool) -> Result<(), InstallError> {
    let dir = providers_root.join(id);
    if dir.exists() { std::fs::remove_dir_all(&dir).map_err(|e| InstallError::Io(e.to_string()))?; }
    store.providers.retain(|p| p.id != id);
    if with_models {
        let dir = models_root.join(id);
        if dir.exists() { std::fs::remove_dir_all(&dir).map_err(|e| InstallError::Io(e.to_string()))?; }
        store.models.retain(|m| m.provider != id);
    }
    Ok(())
}

/// Unpacks a zip with the tar.exe that ships with Windows 10 and later.
pub fn extract_with_tar(zip: &Path, dest: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("tar.exe")
        .arg("-xf").arg(zip).arg("-C").arg(dest)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run tar.exe: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstalledProvider { pub id: String, pub version: String, pub dir: PathBuf }

#[derive(Clone, Debug, PartialEq)]
pub struct InstalledModel { pub provider: String, pub id: String, pub dir: PathBuf, pub sha256: String }

/// What is installed, read from and written to `voice-install.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Store {
    pub providers: Vec<InstalledProvider>,
    pub models: Vec<InstalledModel>,
}

impl Store {
    pub fn to_json(&self) -> Value {
        json!({
            "schema": 1,
            "providers": self.providers.iter().map(|p| json!({ "id": p.id, "version": p.version, "dir": p.dir.to_string_lossy() })).collect::<Vec<_>>(),
            "models": self.models.iter().map(|m| json!({ "provider": m.provider, "id": m.id, "dir": m.dir.to_string_lossy(), "sha256": m.sha256 })).collect::<Vec<_>>(),
        })
    }

    /// Damaged entries are dropped; a damaged file reads as empty rather than blocking startup.
    pub fn from_json(v: &Value) -> Store {
        let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).map(String::from);
        Store {
            providers: v.get("providers").and_then(Value::as_array).into_iter().flatten().filter_map(|p| Some(InstalledProvider {
                id: s(p, "id")?, version: s(p, "version").unwrap_or_default(), dir: PathBuf::from(s(p, "dir")?),
            })).collect(),
            models: v.get("models").and_then(Value::as_array).into_iter().flatten().filter_map(|m| Some(InstalledModel {
                provider: s(m, "provider")?, id: s(m, "id")?, dir: PathBuf::from(s(m, "dir")?), sha256: s(m, "sha256")?,
            })).collect(),
        }
    }

    pub fn load(path: &Path) -> Store {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|v| Store::from_json(&v)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), InstallError> {
        write_atomic(path, self.to_json().to_string().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{self, Download, ModelFile};
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zorui-life-{name}-{}", super::super::profile::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A model with one file per payload, each with its real hash and size, so verification can pass.
    fn model(files: &[(&str, &[u8])]) -> ModelSpec {
        ModelSpec {
            id: "voice-a".into(),
            name: "Voice A".into(),
            files: files.iter().map(|(name, payload)| ModelFile {
                file: name.to_string(),
                download: Download { url: format!("https://example.com/{name}"), sha256: sha256::hex(payload), size_bytes: payload.len() as u64 },
            }).collect(),
        }
    }

    /// Serves the payload registered for each URL; anything else fails the fetch.
    fn serve(table: Vec<(String, Vec<u8>)>) -> impl Fn(&str) -> Option<Vec<u8>> {
        move |url: &str| table.iter().find(|(u, _)| u == url).map(|(_, b)| b.clone())
    }

    fn table(m: &ModelSpec, files: &[(&str, &[u8])]) -> Vec<(String, Vec<u8>)> {
        let _ = m;
        files.iter().map(|(n, b)| (format!("https://example.com/{n}"), b.to_vec())).collect()
    }

    #[test]
    fn a_model_installs_only_with_the_approval_for_its_total_size() {
        let root = temp("approve");
        let files: [(&str, &[u8]); 2] = [("a.onnx", b"model"), ("a.onnx.json", b"{}")];
        let m = model(&files);
        let mut store = Store::default();
        let wrong = Approval { what: "other".into(), size_bytes: m.size_bytes() };
        assert!(matches!(install_model(&mut store, &root, "p", &m, &wrong, &serve(table(&m, &files))), Err(InstallError::NotApproved(_))));
        let stale = Approval { what: "voice-a".into(), size_bytes: m.files[0].download.size_bytes };
        assert!(matches!(install_model(&mut store, &root, "p", &m, &stale, &serve(table(&m, &files))), Err(InstallError::NotApproved(_))), "the approval shows the total, not one file");
        assert!(store.models.is_empty() && !root.join("p").exists(), "nothing written without approval");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_verified_model_is_written_as_all_its_files_and_recorded() {
        let root = temp("ok");
        let files: [(&str, &[u8]); 2] = [("a.onnx", b"model"), ("a.onnx.json", b"{}")];
        let m = model(&files);
        let mut store = Store::default();
        let ok = Approval { what: "voice-a".into(), size_bytes: m.size_bytes() };
        let dir = install_model(&mut store, &root, "p", &m, &ok, &serve(table(&m, &files))).unwrap();
        assert_eq!(std::fs::read(dir.join("a.onnx")).unwrap(), b"model");
        assert_eq!(std::fs::read(dir.join("a.onnx.json")).unwrap(), b"{}");
        assert_eq!(store.models.len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn one_bad_file_means_no_file_of_the_model_is_installed() {
        let root = temp("partial");
        let good: [(&str, &[u8]); 2] = [("a.onnx", b"model"), ("a.onnx.json", b"{}")];
        let m = model(&good);
        let mut store = Store::default();
        let ok = Approval { what: "voice-a".into(), size_bytes: m.size_bytes() };
        // the second file arrives tampered (same size, other bytes)
        let served = vec![
            ("https://example.com/a.onnx".to_string(), b"model".to_vec()),
            ("https://example.com/a.onnx.json".to_string(), b"XX".to_vec()),
        ];
        let r = install_model(&mut store, &root, "p", &m, &ok, &serve(served));
        assert!(matches!(r, Err(InstallError::Corrupt(_))));
        assert!(!root.join("p").join("voice-a").join("a.onnx").exists(), "the valid file is not left behind");
        assert!(store.models.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_failed_network_fetch_is_reported_not_panicked() {
        let root = temp("net");
        let files: [(&str, &[u8]); 1] = [("a.onnx", b"x")];
        let m = model(&files);
        let ok = Approval { what: "voice-a".into(), size_bytes: m.size_bytes() };
        let mut store = Store::default();
        let r = install_model(&mut store, &root, "p", &m, &ok, &|_: &str| None);
        assert!(matches!(r, Err(InstallError::Download(_))));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A manifest with a package and one two-file model, built from real payloads.
    fn package_manifest(zip: &[u8], files: &[(&str, &[u8])]) -> (Manifest, Value) {
        let mut raw = manifest::sample();
        raw["package"] = json!({ "url": "https://example.com/pkg.zip", "sha256": sha256::hex(zip), "size_bytes": zip.len() });
        raw["models"] = json!([{ "id": "voice-a", "name": "Voice A", "files": files.iter().map(|(n, b)| json!({
            "file": n, "url": format!("https://example.com/{n}"), "sha256": sha256::hex(b), "size_bytes": b.len()
        })).collect::<Vec<_>>() }]);
        (manifest::parse(&raw).unwrap(), raw)
    }

    #[test]
    fn a_whole_manifest_installs_with_per_item_approvals_and_removes_cleanly() {
        let root = temp("prov");
        let providers = root.join("providers");
        let models = root.join("models");
        let zip: &[u8] = b"zip bytes";
        let files: [(&str, &[u8]); 2] = [("a.onnx", b"model"), ("a.onnx.json", b"{}")];
        let (m, raw) = package_manifest(zip, &files);
        let approvals = vec![
            Approval { what: m.id.clone(), size_bytes: zip.len() as u64 },
            Approval { what: "voice-a".into(), size_bytes: 5 + 2 },
        ];
        let mut served = table(&m.models[0], &files);
        served.push(("https://example.com/pkg.zip".into(), zip.to_vec()));
        let unpack = |_: &Path, dest: &Path| -> Result<(), String> {
            std::fs::create_dir_all(dest.join("bin")).map_err(|e| e.to_string())?;
            std::fs::write(dest.join("bin").join("tts.exe"), b"exe").map_err(|e| e.to_string())
        };
        let mut store = Store::default();
        install_manifest(&mut store, &providers, &models, &m, &raw, &approvals, &serve(served.clone()), &unpack).unwrap();
        let dir = providers.join(&m.id);
        assert!(dir.join("bin").join("tts.exe").is_file() && dir.join(FILE).is_file());
        assert!(!dir.join("package.zip").exists(), "the package is removed after unpacking");
        assert_eq!(store.providers[0].version, "1.2.0");
        assert_eq!(store.models.len(), 1);

        remove_provider(&mut store, &providers, &models, &m.id, false).unwrap();
        assert!(!dir.exists() && store.providers.is_empty());
        assert_eq!(store.models.len(), 1, "models survive a plain removal");
        remove_provider(&mut store, &providers, &models, &m.id, true).unwrap();
        assert!(store.models.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_manifest_with_a_missing_approval_downloads_nothing() {
        let root = temp("noapprove");
        let zip: &[u8] = b"zip bytes";
        let files: [(&str, &[u8]); 1] = [("a.onnx", b"model")];
        let (m, raw) = package_manifest(zip, &files);
        // approval for the package only; the model has none
        let approvals = vec![Approval { what: m.id.clone(), size_bytes: zip.len() as u64 }];
        let fetched = std::cell::Cell::new(0);
        let fetch = |_: &str| { fetched.set(fetched.get() + 1); Some(zip.to_vec()) };
        let mut store = Store::default();
        let r = install_manifest(&mut store, &root.join("p"), &root.join("m"), &m, &raw, &approvals, &fetch, &|_, _| Ok(()));
        assert_eq!(r, Err(InstallError::NotApproved("voice-a".into())));
        assert_eq!(fetched.get(), 0, "nothing downloaded before every approval is present");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_store_round_trips_and_damaged_entries_are_dropped() {
        let s = Store {
            providers: vec![InstalledProvider { id: "a".into(), version: "1".into(), dir: PathBuf::from("C:/x/a") }],
            models: vec![InstalledModel { provider: "a".into(), id: "m".into(), dir: PathBuf::from("C:/x/m"), sha256: "00".into() }],
        };
        assert_eq!(Store::from_json(&s.to_json()), s);
        let damaged = json!({ "providers": [{ "version": "1" }, { "id": "ok", "dir": "C:/ok" }], "models": "nope" });
        let read = Store::from_json(&damaged);
        assert_eq!(read.providers.len(), 1);
        assert!(read.models.is_empty());
    }
}
