//! HTTP for the remote adapters. Uses curl.exe, as the rest of the app does. The request goes to curl as a
//! config file on stdin, so headers such as the API key never appear on its command line.

use super::contract::{Cancel, ProviderError};
use super::process;
use super::profile::new_id;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Requests the voice adapters make. Every answer comes back as a status and body, whatever the status;
/// Err means no answer at all (network down, timeout, cancelled).
pub trait HttpClient: Send + Sync {
    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8], cancel: &Cancel) -> Result<HttpResponse, ProviderError>;
    /// A multipart POST: text fields, then files sent as form parts.
    fn post_form(&self, _url: &str, _headers: &[(String, String)], _fields: &[(String, String)], _files: &[(String, PathBuf)], _cancel: &Cancel) -> Result<HttpResponse, ProviderError> {
        Err(ProviderError::Network("multipart upload is not supported by this client".into()))
    }
    fn get(&self, _url: &str, _headers: &[(String, String)], _cancel: &Cancel) -> Result<HttpResponse, ProviderError> {
        Err(ProviderError::Network("GET is not supported by this client".into()))
    }
}

/// Quotes a value for curl's config syntax: backslash and double quote are escaped.
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// What the request carries besides its headers.
pub enum Body<'a> {
    None,
    /// The file holding the raw request body.
    File(&'a str),
    /// Text fields (sent literally, so a value starting with @ is not read as a file) and files.
    Form { fields: &'a [(String, String)], files: &'a [(String, String)] },
}

/// The curl config that makes one request. Pure, so a test can check the quoting and the parts.
pub fn config(url: &str, method: &str, headers: &[(String, String)], body: Body, out_file: &str) -> String {
    let mut c = format!("url = {}\nrequest = {}\n", quote(url), quote(method));
    for (k, v) in headers { c.push_str(&format!("header = {}\n", quote(&format!("{k}: {v}")))); }
    match body {
        Body::None => {}
        Body::File(path) => c.push_str(&format!("data-binary = {}\n", quote(&format!("@{path}")))),
        Body::Form { fields, files } => {
            for (name, value) in fields { c.push_str(&format!("form-string = {}\n", quote(&format!("{name}={value}")))); }
            for (name, path) in files { c.push_str(&format!("form = {}\n", quote(&format!("{name}=@{path}")))); }
        }
    }
    c.push_str(&format!("output = {}\n", quote(out_file)));
    c.push_str("silent\nshow-error\nmax-time = 120\nwrite-out = \"%{http_code}\"\n");
    c
}

pub struct Curl;

impl Curl {
    /// Runs curl with `cfg` on stdin, waits (cancellable), and reads the status and the output file.
    fn run(cfg: &str, out_path: &PathBuf, cancel: &Cancel) -> Result<HttpResponse, ProviderError> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = Command::new("curl.exe")
            .args(["-K", "-"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn()
            .map_err(|e| ProviderError::Network(format!("could not start curl: {e}")))?;
        let mut stdin = child.stdin.take().ok_or_else(|| ProviderError::Network("no curl stdin".into()))?;
        stdin.write_all(cfg.as_bytes()).map_err(|e| ProviderError::Network(format!("curl refused the request: {e}")))?;
        drop(stdin);
        process::wait_cancellable(&mut child, cancel, "curl").map_err(|e| match e {
            ProviderError::Other(m) => ProviderError::Network(m),
            other => other,
        })?;
        let mut code = String::new();
        if let Some(mut out) = child.stdout.take() { out.read_to_string(&mut code).ok(); }
        let status: u16 = code.trim().parse().map_err(|_| ProviderError::Network("no HTTP status from curl".into()))?;
        let body = std::fs::read(out_path).unwrap_or_default();
        Ok(HttpResponse { status, body })
    }

    /// One request with its own staged files, removed afterwards whatever happened.
    fn with_files<T>(f: impl FnOnce(&PathBuf, &PathBuf) -> Result<T, ProviderError>) -> Result<T, ProviderError> {
        let dir = std::env::temp_dir();
        let id = new_id();
        let body = dir.join(format!("zorui-http-{id}.body"));
        let out = dir.join(format!("zorui-http-{id}.out"));
        let result = f(&body, &out);
        std::fs::remove_file(&body).ok();
        std::fs::remove_file(&out).ok();
        result
    }
}

impl HttpClient for Curl {
    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8], cancel: &Cancel) -> Result<HttpResponse, ProviderError> {
        Curl::with_files(|body_path, out_path| {
            std::fs::write(body_path, body).map_err(|e| ProviderError::Other(format!("could not stage request: {e}")))?;
            let cfg = config(url, "POST", headers, Body::File(&body_path.to_string_lossy()), &out_path.to_string_lossy());
            Curl::run(&cfg, out_path, cancel)
        })
    }

    fn post_form(&self, url: &str, headers: &[(String, String)], fields: &[(String, String)], files: &[(String, PathBuf)], cancel: &Cancel) -> Result<HttpResponse, ProviderError> {
        let files: Vec<(String, String)> = files.iter().map(|(n, p)| (n.clone(), p.to_string_lossy().into_owned())).collect();
        Curl::with_files(|_, out_path| {
            let cfg = config(url, "POST", headers, Body::Form { fields, files: &files }, &out_path.to_string_lossy());
            Curl::run(&cfg, out_path, cancel)
        })
    }

    fn get(&self, url: &str, headers: &[(String, String)], cancel: &Cancel) -> Result<HttpResponse, ProviderError> {
        Curl::with_files(|_, out_path| {
            let cfg = config(url, "GET", headers, Body::None, &out_path.to_string_lossy());
            Curl::run(&cfg, out_path, cancel)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_quotes_and_backslashes() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn a_post_config_carries_headers_body_and_a_status_write_out() {
        let c = config("https://x.test/v1", "POST", &[("Authorization".into(), "Bearer k1".into())], Body::File("C:/t/body"), "C:/t/out");
        assert!(c.contains("url = \"https://x.test/v1\""));
        assert!(c.contains("request = \"POST\""));
        assert!(c.contains("header = \"Authorization: Bearer k1\""));
        assert!(c.contains("data-binary = \"@C:/t/body\""));
        assert!(c.contains("write-out = \"%{http_code}\""));
    }

    #[test]
    fn a_form_config_sends_fields_literally_and_files_by_path() {
        let fields = vec![("name".to_string(), "@not-a-file".to_string())];
        let files = vec![("files".to_string(), "C:/refs/a.wav".to_string())];
        let c = config("https://x.test/add", "POST", &[], Body::Form { fields: &fields, files: &files }, "C:/t/out");
        assert!(c.contains("form-string = \"name=@not-a-file\""), "a field with @ is not read as a file");
        assert!(c.contains("form = \"files=@C:/refs/a.wav\""));
    }

    #[test]
    fn a_get_config_has_no_body() {
        let c = config("https://x.test/voices", "GET", &[], Body::None, "C:/t/out");
        assert!(c.contains("request = \"GET\""));
        assert!(!c.contains("data-binary") && !c.contains("form"));
    }
}
