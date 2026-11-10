//! Rust port of end4_bridge.py's web-facing commands: secrets (DPAPI), translate, booru, wallpaper
//! download, the AI chat/models and the preset gallery. HTTP goes through curl.exe (no TLS stack here).

use super::common::*;
use super::*;
use serde_json::json;
use std::{
    collections::HashSet,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::mpsc,
    time::Duration,
};

/// Streaming commands this module runs.
pub const STREAMS: &[&str] = &["ai-chat"];

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0 Safari/537.36 zor-ui";
const PRESET_REPO: &str = "pctrade/end4-pCpresets";
const RAW: &str = "https://raw.githubusercontent.com/pctrade/end4-pCpresets";

/// {name: has a saved key} for the given providers.
fn key_status(names: &[&str]) -> Value {
    Value::Object(names.iter().map(|k| (k.to_string(), json!(!secret_get(k).is_empty()))).collect())
}

/// One-shot command `name`; None if this module doesn't port it.
pub fn call(cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    let a = |i: usize| args.get(i).copied().unwrap_or("");
    Some(match name {
        "secret-set" => secret_set(a(0), a(1)),
        "secret-status" => key_status(&["gemini", "openai", "anthropic", "mistral", "openrouter", "wallhaven", "elevenlabs", "endpoint"]),
        "translate" => translate(a(0), a(1), &args.iter().skip(2).copied().collect::<Vec<_>>().join(" ")),
        "booru" => booru(a(0), a(1), args.get(2..).unwrap_or(&[])),
        "download-wallpaper" => download_wallpaper(cfg, a(0), args.get(1..).unwrap_or(&[])),
        "ai-models" => {
            let mut keys = key_status(&["gemini", "openai", "anthropic", "mistral", "openrouter"]);
            keys["claude-cli"] = json!(claude_cli_on() && super::claude_cli::find().is_some()); // no key: it signs in through Claude Code
            for m in super::sub_cli::models() { let p = m["provider"].as_str().unwrap_or(""); keys[p] = json!(super::sub_cli::installed(p)); }
            json!({ "models": all_models(), "keys": keys })
        }
        "presets-online" => presets_online(),
        "preset-apply-online" => preset_apply_online(cfg, a(0)),
        "presets-local" => presets_local(),
        "preset-save" => preset_save(a(0)),
        "preset-apply-local" => preset_apply_local(cfg, a(0)),
        "preset-delete" => preset_delete(a(0)),
        _ => return None,
    })
}

/// Runs streaming command `name` (one of STREAMS) until it ends or `out.stopped()`.
pub fn stream(_cfg: &Config, name: &str, _args: &[String], stdin: &str, out: &Sink) {
    if name == "ai-chat" {
        ai_chat(stdin, out);
    }
}

// ------------------------------------------------------------------ small helpers

/// config.json exactly as stored (no defaults merged in), so writing it back adds nothing.
fn raw_config() -> Value { load_json(&pack().join("config.json"), json!({})) }

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `a or b` for JSON values.
fn or(a: &Value, b: &Value) -> Value { if truthy(a) { a.clone() } else { b.clone() } }

/// Percent-encodes like urllib.parse.quote (plus=false) / quote_plus (plus=true).
fn enc(s: &str, plus: bool) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => o.push(b as char),
            b' ' if plus => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

fn urlencode(pairs: &[(&str, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", enc(k, true), enc(v, true))).collect::<Vec<_>>().join("&")
}

/// urllib.parse.unquote.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |i: usize| b.get(i).and_then(|c| (*c as char).to_digit(16));
    let (mut o, mut i) = (Vec::new(), 0);
    while i < b.len() {
        match (b[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => { o.push((h * 16 + l) as u8); i += 3; }
            (c, _, _) => { o.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&o).into_owned()
}

fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' }

/// re.sub(r"[^\w.-]", "_", s), char by char; never all dots, so it can't name a parent folder.
fn safe_id(s: &str) -> String {
    let r: String = s.chars().map(|c| if is_word(c) || c == '.' || c == '-' { c } else { '_' }).collect();
    if !r.is_empty() && r.chars().all(|c| c == '.') { format!("_{r}") } else { r }
}

/// GET through curl.exe with Python's curl_get() error text ("curl: (35) ...").
fn curl_get(url: &str, headers: &[(&str, &str)], timeout_s: u32) -> Result<Vec<u8>, String> {
    let mut c = hidden("curl.exe");
    c.args(["-sSfL", "--max-time", &timeout_s.to_string(), "-H", &format!("User-Agent: {UA}")]);
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept")) {
        c.args(["-H", "Accept: application/json"]);
    }
    for (k, v) in headers {
        c.args(["-H", &format!("{k}: {v}")]);
    }
    let o = c.arg(url).output().map_err(|e| format!("curl: {e}"))?;
    if o.status.success() { Ok(o.stdout) } else { Err(String::from_utf8_lossy(&o.stderr).trim().chars().take(200).collect()) }
}

fn get_json_e(url: &str, headers: &[(&str, &str)], timeout_s: u32) -> Result<Value, String> {
    serde_json::from_slice(&curl_get(url, headers, timeout_s)?).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ secrets (Windows DPAPI)

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(super) fn b64_encode(d: &[u8]) -> String {
    let mut o = String::new();
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() { o.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char) } else { o.push('=') }
        }
    }
    o
}

/// Like Python's b64decode: characters outside the alphabet are skipped.
pub(super) fn b64_decode(s: &str) -> Vec<u8> {
    let (mut o, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for ch in s.bytes() {
        let Some(v) = B64.iter().position(|&x| x == ch) else { continue };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            o.push((acc >> bits & 0xFF) as u8);
        }
    }
    o
}

/// CryptProtectData / CryptUnprotectData for the current Windows account (same blobs as the script).
pub(super) fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the blobs outlive the calls; the output buffer is copied, then freed with LocalFree.
    unsafe {
        if protect {
            CryptProtectData(&input, windows::core::w!("zor-ui"), None, None, None, 0, &mut output).ok()?;
        } else {
            CryptUnprotectData(&input, None, None, None, None, 0, &mut output).ok()?;
        }
        if output.pbData.is_null() { return None; }
        let v = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        Some(v)
    }
}

/// The saved key `name` (DPAPI-protected in secrets.json), else ZORUI_<NAME>_KEY, else "".
pub(crate) fn secret_get(name: &str) -> String {
    let secrets = load_json(&root().join("secrets.json"), json!({}));
    match secrets.get(name) {
        Some(e) if truthy(e) => e.as_str()
            .and_then(|e| dpapi(&b64_decode(e), false))
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default(),
        _ => std::env::var(format!("ZORUI_{}_KEY", name.to_uppercase())).unwrap_or_default(),
    }
}

fn secret_set(name: &str, value: &str) -> Value {
    let path = root().join("secrets.json");
    let mut data = load_json(&path, json!({}));
    if !data.is_object() { data = json!({}); }
    if value.is_empty() {
        if let Some(o) = data.as_object_mut() { o.remove(name); }
    } else {
        let Some(blob) = dpapi(value.as_bytes(), true) else { return err("DPAPI failed") };
        data[name] = json!(b64_encode(&blob));
    }
    save_json(&path, &data);
    json!({ "ok": true, "name": name, "set": !value.is_empty() })
}

// ------------------------------------------------------------------ translate

pub(super) fn translate(target: &str, source: &str, text: &str) -> Value {
    let text = text.trim();
    if text.is_empty() { return json!({ "text": "", "detected": source }); }
    let src = if source.is_empty() { "auto" } else { source };
    let q = urlencode(&[("client", "gtx".into()), ("sl", src.into()), ("tl", target.into()), ("dt", "t".into()), ("q", text.into())]);
    match get_json_e(&format!("https://translate.googleapis.com/translate_a/single?{q}"), &[], 20) {
        Ok(d) => match parse_translation(&d, source) {
            Some((t, det)) => json!({ "text": t, "detected": det }),
            None => json!({ "error": "Translation failed: unexpected response" }),
        },
        Err(e) => json!({ "error": format!("Translation failed: {e}") }),
    }
}

/// The joined translation and the detected language from Google's reply.
fn parse_translation(d: &Value, source: &str) -> Option<(String, Value)> {
    let text: String = d.get(0)?.as_array()?.iter().filter_map(|s| s.get(0)?.as_str()).collect();
    Some((text, d.get(2).cloned().unwrap_or_else(|| json!(source))))
}

// ------------------------------------------------------------------ booru

fn booru_tags(tags: &[&str], nsfw: bool, safe_tag: &str) -> String {
    let t = tags.join(" ");
    let t = t.trim();
    if nsfw { t.to_string() } else { format!("{t} {safe_tag}").trim().to_string() }
}

/// One post list entry; `Null` for fields the site left out.
fn field(it: &Value, k: &str) -> Value { it.get(k).cloned().unwrap_or(Value::Null) }

/// Posts from a yande.re-style site (konachan tries .com then .net when NSFW is allowed).
fn booru_moebooru(source: &str, nsfw: bool, page: i64, tags: &[&str], items: &mut Vec<Value>) -> Result<(), String> {
    let bases: &[&str] = match (source, nsfw) {
        ("yandere", _) => &["https://yande.re"],
        (_, true) => &["https://konachan.com", "https://konachan.net"],
        _ => &["https://konachan.net"],
    };
    let q = format!("/post.json?{}", urlencode(&[("tags", booru_tags(tags, nsfw, "rating:safe")), ("limit", "30".into()), ("page", page.to_string())]));
    let mut found = None;
    for (i, base) in bases.iter().enumerate() {
        match get_json_e(&format!("{base}{q}"), &[], 20) {
            Ok(p) => { found = Some((*base, p)); break; }
            Err(e) if i + 1 == bases.len() => return Err(e),
            Err(_) => {} // konachan.com is blocked on some networks; .net is the same site
        }
    }
    let (base, posts) = found.ok_or("no response")?;
    for it in posts.as_array().ok_or("unexpected response")? {
        if !truthy(&field(it, "file_url")) { continue; }
        items.push(json!({ "id": field(it, "id"), "preview": field(it, "preview_url"), "sample": or(&field(it, "sample_url"), &field(it, "file_url")),
            "file": field(it, "file_url"), "w": field(it, "width"), "h": field(it, "height"), "tags": it.get("tags").cloned().unwrap_or(json!("")),
            "nsfw": it["rating"].as_str() != Some("s"), "page": format!("{base}/post/show/{}", field(it, "id")),
            "source": or(&field(it, "source"), &json!("")) }));
    }
    Ok(())
}

fn booru_danbooru(nsfw: bool, page: i64, tags: &[&str], items: &mut Vec<Value>) -> Result<(), String> {
    let n = tags.len().min(if nsfw { 2 } else { 1 });
    let url = format!("https://danbooru.donmai.us/posts.json?{}", urlencode(&[
        ("tags", booru_tags(&tags[..n], nsfw, "rating:general")), ("limit", "30".into()), ("page", page.to_string())]));
    for it in get_json_e(&url, &[], 20)?.as_array().ok_or("unexpected response")? {
        if !truthy(&field(it, "file_url")) || !truthy(&field(it, "preview_file_url")) { continue; }
        items.push(json!({ "id": field(it, "id"), "preview": field(it, "preview_file_url"), "sample": or(&field(it, "large_file_url"), &field(it, "file_url")),
            "file": field(it, "file_url"), "w": field(it, "image_width"), "h": field(it, "image_height"), "tags": it.get("tag_string").cloned().unwrap_or(json!("")),
            "nsfw": !matches!(it["rating"].as_str(), Some("g" | "s")), "page": format!("https://danbooru.donmai.us/posts/{}", field(it, "id")),
            "source": or(&field(it, "source"), &json!("")) }));
    }
    Ok(())
}

fn booru_wallhaven(nsfw: bool, page: i64, tags: &[&str], items: &mut Vec<Value>) -> Result<(), String> {
    let key = secret_get("wallhaven");
    let mut params = vec![("q", tags.join(" ")), ("page", page.to_string()),
        ("purity", if nsfw && !key.is_empty() { "110" } else { "100" }.into()), ("categories", "111".into()),
        ("sorting", if tags.is_empty() { "toplist" } else { "relevance" }.into()), ("atleast", "1920x1080".into())];
    if !key.is_empty() { params.push(("apikey", key)); }
    let data = get_json_e(&format!("https://wallhaven.cc/api/v1/search?{}", urlencode(&params)), &[], 20)?;
    for it in data["data"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        items.push(json!({ "id": field(it, "id"), "preview": it["thumbs"]["small"], "sample": it["thumbs"]["large"], "file": field(it, "path"),
            "w": field(it, "dimension_x"), "h": field(it, "dimension_y"), "tags": it.get("category").cloned().unwrap_or(json!("")),
            "nsfw": it["purity"].as_str() != Some("sfw"), "page": field(it, "url"), "source": or(&field(it, "source"), &json!("")) }));
    }
    Ok(())
}

fn booru(source: &str, page: &str, tags: &[&str]) -> Value {
    let nsfw = truthy(&raw_config()["booru"]["allowNsfw"]);
    let page = page.trim().parse::<i64>().unwrap_or(1).max(1);
    let mut items = Vec::new();
    let res = match source {
        "yandere" | "konachan" => booru_moebooru(source, nsfw, page, tags, &mut items),
        "danbooru" => booru_danbooru(nsfw, page, tags, &mut items),
        "wallhaven" => booru_wallhaven(nsfw, page, tags, &mut items),
        // api.waifu.im no longer exists and waifu.im now serves an unrelated spam page (Nov 2026).
        "waifuim" => return json!({ "error": "waifu.im has shut down. Pick another source." }),
        _ => return json!({ "error": format!("unknown source {source}") }),
    };
    if let Err(e) = res {
        if ["curl: (35)", "Connection was reset", "ConnectionReset", "WinError 10054"].iter().any(|p| e.contains(p)) {
            return json!({ "error": format!("{source}: the connection was reset. Your network or ISP is probably blocking this site; it should work over a VPN.") });
        }
        return json!({ "error": format!("{source}: {e}") });
    }
    if !nsfw { items.retain(|i| i["nsfw"] != json!(true)); }
    json!({ "source": source, "page": page, "items": items })
}

// ------------------------------------------------------------------ wallpaper download

fn wallpapers_dir() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Pictures").join("Wallpapers")
}

/// re.sub(r"[^\w.\-]+", "_", s): each run of other characters becomes one underscore.
fn collapse_underscores(s: &str) -> String {
    let mut o = String::new();
    let mut in_run = false;
    for c in s.chars() {
        if is_word(c) || c == '.' || c == '-' { o.push(c); in_run = false; } else if !in_run { o.push('_'); in_run = true; }
    }
    o
}

/// The file name a wallpaper URL is saved under (last path segment, made filename-safe, at most 120 chars).
fn download_name(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |x| x.1);
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    let path = path.split(['?', '#']).next().unwrap_or("");
    let name = collapse_underscores(&unquote(path.rsplit('/').next().unwrap_or("")));
    let skip = name.chars().count().saturating_sub(120);
    let name: String = name.chars().skip(skip).collect();
    if name.is_empty() { "wallpaper.jpg".into() } else { name }
}

/// Downloads `url` into `dest_dir` (skips a file that is already there). Returns the file path.
fn download(url: &str, dest_dir: &Path) -> Result<PathBuf, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) { return Err("only http(s) links".into()); }
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join(download_name(url));
    if !dest.exists() {
        let host = url.split_once("://").map_or("", |x| x.1).split(['/', '?', '#']).next().unwrap_or("");
        let part = PathBuf::from(format!("{}.part", dest.display()));
        let o = hidden("curl.exe")
            .args(["-sSfL", "--max-time", "120", "-H", &format!("User-Agent: {UA}"), "-H", "Accept: */*", "-H", &format!("Referer: https://{host}/"), "-o"])
            .arg(&part).arg(url).output().map_err(|e| format!("curl: {e}"))?;
        if !o.status.success() {
            let _ = std::fs::remove_file(&part);
            return Err(String::from_utf8_lossy(&o.stderr).trim().chars().take(200).collect());
        }
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    }
    Ok(dest)
}

/// switchwall.ps1 through the shared dispatcher (the Rust port when there is one).
fn run_switchwall(cfg: &Config, args: &[&str]) -> Value {
    match crate::proc::run(cfg, "switchwall", args) { Value::Null => err("switchwall gave no result"), v => v }
}

fn download_wallpaper(cfg: &Config, url: &str, flags: &[&str]) -> Value {
    let path = match download(url, &wallpapers_dir()) { Ok(p) => p, Err(e) => return err(e) };
    let p = path.display().to_string();
    let mut res = json!({ "ok": true, "path": p });
    if flags.contains(&"--apply") { res["theme"] = run_switchwall(cfg, &["-Image", &p]); }
    res
}

// ------------------------------------------------------------------ AI models and chat

fn default_models() -> Vec<Value> {
    [("gemini-2.5-flash", "Gemini 2.5 Flash", "gemini"), ("gemini-3-flash-preview", "Gemini 3 Flash", "gemini"),
     ("mistral-medium-latest", "Mistral Medium", "mistral"), ("claude-sonnet-5-5", "Claude Sonnet", "anthropic"),
     ("claude-haiku-5-5", "Claude Haiku", "anthropic"), ("gpt-4o-mini", "GPT-4o mini", "openai"),
     ("openrouter/auto", "OpenRouter Auto", "openrouter")]
        .iter().map(|(id, name, provider)| json!({ "id": id, "name": name, "provider": provider })).collect()
}

fn openai_like(provider: &str) -> &'static str {
    match provider {
        "mistral" => "https://api.mistral.ai/v1/chat/completions",
        "openrouter" => "https://openrouter.ai/api/v1/chat/completions",
        "ollama" => "http://localhost:11434/v1/chat/completions",
        _ => "https://api.openai.com/v1/chat/completions",
    }
}

fn ollama_models() -> Vec<Value> {
    let Ok(tags) = get_json_e("http://localhost:11434/api/tags", &[], 2) else { return vec![] };
    tags["models"].as_array().map(|a| a.iter().filter_map(|m| {
        let n = m["name"].as_str()?;
        Some(json!({ "id": n, "name": format!("Ollama: {n}"), "provider": "ollama" }))
    }).collect()).unwrap_or_default()
}

/// Settings, Services, "Use my Claude subscription" (on unless turned off).
fn claude_cli_on() -> bool { raw_config()["ai"]["claudeCli"].as_bool().unwrap_or(true) }

fn all_models() -> Vec<Value> {
    let mut v = if claude_cli_on() && super::claude_cli::find().is_some() { super::claude_cli::models() } else { vec![] };
    v.extend(super::sub_cli::models());
    v.extend(default_models());
    if let Some(c) = raw_config()["ai"]["customModels"].as_array() {
        v.extend(c.iter().filter(|m| m.is_object() && truthy(&m["id"])).cloned());
    }
    v.extend(ollama_models());
    v
}

/// The pictures attached to a chat message as (mime type, base64 data, text that introduces it). A message is
/// {"role", "content", "images": [{"mime", "data", "label"}]}; every provider's adapter reads them from here, so
/// the callers never need to know which model answers.
pub(super) fn images_of(m: &Value) -> Vec<(String, String, String)> {
    m["images"].as_array().map(|a| a.iter().filter_map(|i| {
        let data = i["data"].as_str().filter(|d| !d.is_empty())?;
        let mime = i["mime"].as_str().filter(|x| !x.is_empty()).unwrap_or("image/jpeg");
        Some((mime.to_string(), data.to_string(), i["label"].as_str().unwrap_or("").to_string()))
    }).collect()).unwrap_or_default()
}

/// Can this provider's models be sent pictures? The Gemini and Codex command line tools only take text.
pub(super) fn can_see(provider: &str) -> bool { provider == "claude-cli" || !provider.ends_with("-cli") }

/// The provider that serves the model with this id ("anthropic", "gemini", ...), or None when the model isn't in the list.
pub(crate) fn provider_of(id: &str) -> Option<String> {
    all_models().into_iter().find(|m| m["id"].as_str() == Some(id)).and_then(|m| m["provider"].as_str().map(str::to_string))
}

/// Can the model with this id be sent pictures? A model that isn't in the list is taken as OpenAI-style, as `ai_chat` does.
pub(super) fn can_see_model(id: &str) -> bool {
    let provider = all_models().into_iter().find(|m| m["id"].as_str() == Some(id)).and_then(|m| m["provider"].as_str().map(str::to_string));
    can_see(provider.as_deref().unwrap_or("openai"))
}

/// Can the model with this id be sent pictures, judging by the id alone? No model list is needed, so Settings can ask every
/// frame. Subscription command line models are named "<provider>-cli:<name>".
pub(super) fn id_can_see(id: &str) -> bool { can_see(id.split(':').next().unwrap_or("")) }

/// The message text, or None when it is empty (the providers refuse empty text blocks).
fn text_of(m: &Value) -> Option<&str> { m["content"].as_str().filter(|t| !t.trim().is_empty()) }

/// Gemini parts of a message: each picture (after its label), then the text.
fn gemini_parts(m: &Value) -> Vec<Value> {
    let imgs = images_of(m);
    if imgs.is_empty() { return vec![json!({ "text": m["content"] })]; }
    let mut p = vec![];
    for (mime, data, label) in imgs {
        if !label.is_empty() { p.push(json!({ "text": label })); }
        p.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }
    p.extend(text_of(m).map(|t| json!({ "text": t })));
    p
}

/// Content of a message in the Anthropic and OpenAI shapes (`openai`: image_url data URLs instead of base64 sources).
fn blocks_of(m: &Value, openai: bool) -> Value {
    let imgs = images_of(m);
    if imgs.is_empty() { return m["content"].clone(); }
    let mut b = vec![];
    for (mime, data, label) in imgs {
        if !label.is_empty() { b.push(json!({ "type": "text", "text": label })); }
        b.push(if openai { json!({ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{data}") } }) }
               else { json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } }) });
    }
    b.extend(text_of(m).map(|t| json!({ "type": "text", "text": t })));
    Value::Array(b)
}

/// What to send for a chat turn: URL, headers and JSON body for `provider`.
fn chat_request(provider: &str, model: &Value, key: &str, msgs: &[Value], system: &str, temp: f64) -> (String, Vec<(String, String)>, Value) {
    let id = model["id"].as_str().unwrap_or("");
    let role = |m: &Value| m["role"].as_str().unwrap_or("user").to_string();
    let h = |k: &str, v: &str| (k.to_string(), v.to_string());
    let mut headers = vec![h("User-Agent", UA), h("Content-Type", "application/json"), h("Accept", "text/event-stream")];
    match provider {
        "gemini" => {
            headers.push(h("x-goog-api-key", key));
            let contents: Vec<Value> = msgs.iter().map(|m| json!({ "role": if role(m) == "assistant" { "model" } else { "user" }, "parts": gemini_parts(m) })).collect();
            (format!("https://generativelanguage.googleapis.com/v1beta/models/{id}:streamGenerateContent?alt=sse"), headers,
             json!({ "contents": contents, "systemInstruction": { "parts": [{ "text": system }] }, "generationConfig": { "temperature": temp } }))
        }
        "anthropic" => {
            headers.push(h("x-api-key", key));
            headers.push(h("anthropic-version", "2023-06-01"));
            let messages: Vec<Value> = msgs.iter().map(|m| json!({ "role": role(m), "content": blocks_of(m, false) })).collect();
            ("https://api.anthropic.com/v1/messages".into(), headers,
             json!({ "model": id, "max_tokens": 4096, "system": system, "temperature": temp, "stream": true, "messages": messages }))
        }
        _ => {
            if !key.is_empty() { headers.push(h("Authorization", &format!("Bearer {key}"))); }
            if provider == "openrouter" {
                headers.push(h("HTTP-Referer", "https://github.com/pctrade/end4-pC"));
                headers.push(h("X-Title", "Zor UI"));
            }
            let url = model["endpoint"].as_str().filter(|e| !e.is_empty()).unwrap_or(openai_like(provider)).to_string();
            let mut messages = vec![json!({ "role": "system", "content": system })];
            messages.extend(msgs.iter().map(|m| json!({ "role": role(m), "content": blocks_of(m, true) })));
            (url, headers, json!({ "model": id, "stream": true, "temperature": temp, "messages": messages }))
        }
    }
}

/// The lines to emit for one parsed SSE event of `provider`.
fn sse_events(provider: &str, ev: &Value) -> Vec<Value> {
    let delta = |t: &Value| t.as_str().filter(|t| !t.is_empty()).map(|t| json!({ "delta": t }));
    match provider {
        "gemini" => ev["candidates"][0]["content"]["parts"].as_array().map(|p| p.iter().filter_map(|x| delta(&x["text"])).collect()).unwrap_or_default(),
        "anthropic" => match ev["type"].as_str() {
            Some("content_block_delta") => delta(&ev["delta"]["text"]).into_iter().collect(),
            Some("error") => vec![json!({ "error": ev["error"]["message"].as_str().unwrap_or("error") })],
            _ => vec![],
        },
        _ => delta(&ev["choices"][0]["delta"]["content"]).into_iter().collect(),
    }
}

/// A value as a curl --config quoted string.
fn cfg_quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c { '\\' => o.push_str("\\\\"), '"' => o.push_str("\\\""), '\n' => o.push_str("\\n"), '\r' => o.push_str("\\r"), '\t' => o.push_str("\\t"), c => o.push(c) }
    }
    o.push('"');
    o
}

/// stdin: {"model", "messages": [{"role", "content"}], "system", "temperature"}; streams {"delta"} lines, then {"done"}.
fn ai_chat(stdin: &str, out: &Sink) {
    let Some(req) = serde_json::from_str::<Value>(stdin.lines().next().unwrap_or("")).ok().filter(Value::is_object) else {
        out.emit(&json!({ "error": "bad request" }));
        return;
    };
    let model = all_models().into_iter().find(|m| m["id"] == req["model"]).unwrap_or_else(|| json!({ "id": req["model"], "provider": "openai" }));
    let provider = model["provider"].as_str().unwrap_or("openai").to_string();
    let msgs: Vec<Value> = req["messages"].as_array().map(|a| a.iter().filter(|m| truthy(&m["content"]) || !images_of(m).is_empty()).cloned().collect()).unwrap_or_default();
    if !can_see(&provider) && msgs.iter().any(|m| !images_of(m).is_empty()) {
        out.emit(&json!({ "error": "This model can't see images. Pick another one for screen help in Settings, Voice." }));
        return;
    }
    let system = req["system"].as_str().filter(|s| !s.is_empty()).unwrap_or("You are a helpful assistant inside a Windows desktop sidebar. Use Markdown. Be concise.");
    let temp = req["temperature"].as_f64().or_else(|| req["temperature"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0.5);
    if provider == "claude-cli" {
        if !claude_cli_on() { out.emit(&json!({ "error": "Claude subscription is turned off in Settings, Services." })); return; }
        super::claude_cli::chat(model["id"].as_str().unwrap_or(""), &msgs, system, out);
        return;
    }
    if provider.ends_with("-cli") {
        super::sub_cli::chat(&provider, &msgs, system, out);
        return;
    }
    let key = if provider == "ollama" { String::new() } else { secret_get(&provider) };
    if provider != "ollama" && provider != "custom" && key.is_empty() {
        out.emit(&json!({ "error": format!("No API key for {provider}. Type /key {provider} YOUR_KEY") }));
        return;
    }
    let (url, headers, mut body) = chat_request(&provider, &model, &key, &msgs, system, temp);
    if provider == "anthropic" { super::native_tools::add_to_body(&mut body, &req["tools"]); }
    // The key and body go through curl's config on stdin, so they never show up in the process list.
    let mut conf = format!("url = {}\n", cfg_quote(&url));
    for (k, v) in &headers { conf.push_str(&format!("header = {}\n", cfg_quote(&format!("{k}: {v}")))); }
    conf.push_str(&format!("data-binary = {}\n", cfg_quote(&body.to_string())));
    let spawned = hidden("curl.exe")
        .args(["-sS", "-N", "--speed-limit", "1", "--speed-time", "120", "-w", "\nZORUI_STATUS:%{http_code}\n", "-K", "-"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => { out.emit(&json!({ "error": format!("{provider}: {e}") })); return; }
    };
    if let Some(mut si) = child.stdin.take() { let _ = si.write_all(conf.as_bytes()); }
    let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else { let _ = child.kill(); return };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(stdout);
        let mut buf = Vec::new();
        while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
            if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() { break; }
            buf.clear();
        }
    });
    let (mut status, mut other, mut finished) = (0u32, String::new(), false);
    let mut tools = super::native_tools::ToolParser::default();
    loop {
        if out.stopped() { let _ = child.kill(); let _ = child.wait(); return; }
        let line = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(l) => l,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let t = line.trim();
        if let Some(code) = t.strip_prefix("ZORUI_STATUS:") {
            status = code.parse().unwrap_or(0);
        } else if let Some(data) = t.strip_prefix("data:") {
            let data = data.trim();
            if data == "[DONE]" { finished = true; break; }
            if let Ok(ev) = serde_json::from_str::<Value>(data) {
                for e in sse_events(&provider, &ev) { out.emit(&e); }
                if provider == "anthropic" {
                    match tools.feed(&ev) {
                        Some(Ok(t)) => { out.emit(&json!({ "tool_use": { "id": t.id, "name": t.name, "input": t.input } })); }
                        Some(Err(f)) => { out.emit(&json!({ "tool_error": { "id": f.id, "name": f.name, "message": f.message } })); }
                        None => {}
                    }
                }
            }
        } else if other.len() < 2000 {
            other.push_str(&line);
        }
    }
    if finished { let _ = child.kill(); }
    let _ = child.wait();
    if !finished && status >= 400 {
        let detail: String = other.chars().take(400).collect();
        out.emit(&json!({ "error": format!("{provider} returned {status}: {detail}") }));
    } else if !finished && status == 0 {
        let mut e = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut e);
        out.emit(&json!({ "error": format!("{provider}: {}", e.trim()) }));
    } else {
        out.emit(&json!({ "done": true }));
    }
}

// ------------------------------------------------------------------ presets

/// The preset names in the repo tree (presets/<n>/<n>.json) sorted case-insensitively, and all file paths.
fn preset_names(tree: &Value) -> (Vec<String>, HashSet<String>) {
    let paths: Vec<&str> = tree["tree"].as_array().map(|a| a.iter().filter_map(|t| t["path"].as_str()).collect()).unwrap_or_default();
    let mut names: Vec<String> = paths.iter().filter_map(|p| {
        let (dir, file) = p.strip_prefix("presets/")?.split_once('/')?;
        (!dir.is_empty() && file.strip_suffix(".json") == Some(dir)).then(|| dir.to_string())
    }).collect::<HashSet<_>>().into_iter().collect();
    names.sort_by_key(|n| (n.to_lowercase(), n.clone()));
    (names, paths.iter().map(|p| p.to_string()).collect())
}

/// Gallery entries for `names` (sorted case-insensitively) with author/updated from the index. `has_preview` says
/// whether a name has a preview.png; the index alone can't tell, so index-only entries assume yes (a 404 is skipped).
fn preset_entries(names: &[String], index: &Value, has_preview: impl Fn(&str) -> bool) -> Vec<Value> {
    let mut names = names.to_vec();
    names.sort_by_key(|n| (n.to_lowercase(), n.clone()));
    names.iter().map(|n| json!({
        "name": n, "author": or(&index[n]["author"], &json!("")), "updated": or(&index[n]["updated"], &json!("")),
        "preview": if has_preview(n) { format!("{RAW}/main/presets/{}/preview.png", enc(n, false)) } else { String::new() },
    })).collect()
}

/// The repo tree call is a big recursive listing that GitHub sometimes answers with a 504, so the static
/// gallery-data/index.json (raw host) is the primary list and the tree is only the fallback.
fn presets_online() -> Value {
    let index = get_json_e(&format!("{RAW}/gallery-data/index.json"), &[], 20).ok().and_then(|v| v.get("presets").cloned()).unwrap_or(json!({}));
    if let Some(m) = index.as_object().filter(|m| !m.is_empty()) {
        let names: Vec<String> = m.keys().cloned().collect();
        return json!({ "presets": preset_entries(&names, &index, |_| true) });
    }
    let url = format!("https://api.github.com/repos/{PRESET_REPO}/git/trees/main?recursive=1");
    let hdr = [("Accept", "application/vnd.github+json")];
    let tree = match get_json_e(&url, &hdr, 20).or_else(|_| get_json_e(&url, &hdr, 20)) {
        Ok(t) => t,
        Err(e) => return json!({ "error": format!("GitHub: {e}"), "presets": [] }),
    };
    let (names, files) = preset_names(&tree);
    json!({ "presets": preset_entries(&names, &index, |n| files.contains(&format!("presets/{n}/preview.png"))) })
}

/// Python int(x or default) for a JSON number or numeric string.
fn int_or(v: &Value, default: i64) -> i64 {
    let n = v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(default);
    if n == 0 { default } else { n }
}

/// Preset widget key -> native desktop card id.
const CARD_IDS: [(&str, &str); 15] = [("clock", "clock"), ("weather", "weather"), ("calendar", "calendar"), ("userCard", "greet"), ("media", "media"), ("resources", "resources"),
    ("worldClock", "worldclock"), ("todo", "todo"), ("notes", "notes"), ("timers", "timer"), ("customText", "text"), ("imageCard", "image1"), ("customImage", "image2"),
    ("images", "image3"), ("sticker", "sticker")];

/// Enabled desktop widgets in a preset that this app has no card for (`visualizer` is handled separately).
fn unsupported_from_end4(preset: &Value) -> Vec<String> {
    let Some(w) = preset["background"]["widgets"].as_object() else { return vec![] };
    let mut out: Vec<String> = w.iter().filter(|(k, v)| truthy(&v["enable"]) && !matches!(k.as_str(), "visualizer" | "glass") && !CARD_IDS.iter().any(|(c, _)| c == k)).map(|(k, _)| k.clone()).collect();
    out.sort();
    out
}

/// The pictures a preset's image cards use: (card id, file in the preset's folder, shape, size px).
fn preset_images(pw: &Value) -> Vec<(&'static str, String, String, Option<i64>)> {
    [("imageCard", "image1"), ("customImage", "image2"), ("sticker", "sticker")].iter().filter_map(|(k, id)| {
        let w = &pw[*k];
        let file = w["path"].as_str().filter(|f| truthy(&w["enable"]) && !f.is_empty() && !f.contains(['/', '\\']))?;
        let size = w["size"].as_f64().map(|s| s.round().clamp(120.0, 800.0) as i64);
        Some((*id, file.to_string(), w["shape"].as_str().unwrap_or("cookie").to_lowercase(), size))
    }).collect()
}

/// Where the preset puts each enabled card: id -> { x, y } (state.json "deskPos").
fn positions_from_end4(pw: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for (k, id) in CARD_IDS {
        let w = &pw[k];
        if let (true, Some(x), Some(y)) = (truthy(&w["enable"]), w["x"].as_f64(), w["y"].as_f64()) { out.insert(id.into(), json!({ "x": x.round() as i64, "y": y.round() as i64 })); }
    }
    Value::Object(out)
}

/// The native bar style for a preset's `bar`: `borderless` picks it (pills, separated and segmented groups are
/// islands, "transparent" draws nothing behind the contents); otherwise corner style 1 floats and 0/2 sit flat.
fn bar_style_from_end4(b: &Value) -> Option<&'static str> {
    match b["borderless"].as_str() {
        Some("pills" | "separated" | "segmented") => return Some("islands"),
        Some("transparent") => return Some("clear"),
        _ => {}
    }
    match b["cornerStyle"].as_i64()? { 1 => Some("float"), 0 | 2 => Some("flat"), _ => None }
}

/// A preset's bar module name -> this app's module id (modules with no native twin are left out).
const BAR_MODULE_IDS: [(&str, &str); 12] = [("activeWindow", "window"), ("leftSidebarButton", "launcher"), ("launcherButton", "launcher"), ("clockWidget", "clock"),
    ("resources", "resources"), ("media", "media"), ("dynamicIsland", "island"), ("workspaces", "workspaces"), ("utilButtons", "utils"), ("weatherBar", "weather"),
    ("networkSpeed", "netspeed"), ("sysTray", "tray")];

/// `bar.modules` from a preset's left / middle / right layouts: the listed modules in their order on their side. A
/// module the preset doesn't list keeps the side it has in `current` (so a preset can't hide the island); one that
/// `current` doesn't place either is hidden. None when the preset lists no layout.
fn modules_from_end4(layouts: &Value, current: &Value) -> Option<Value> {
    let mut out: Vec<(String, &str)> = vec![];
    for (key, side) in [("leftLayout", "left"), ("middleLayout", "center"), ("rightLayout", "right")] {
        for name in layouts[key].as_array().into_iter().flatten().filter_map(Value::as_str) {
            // a preset module with no native twin (battery, bluetooth, divider...) is skipped
            let Some(id) = (if name == "systemIcons" { Some("system") } else { BAR_MODULE_IDS.iter().find(|(k, _)| *k == name).map(|m| m.1) }) else { continue };
            if !out.iter().any(|(i, _)| i == id) { out.push((id.to_string(), side)); }
        }
    }
    if out.is_empty() { return None; }
    let all = ["launcher", "window", "resources", "media", "island", "workspaces", "clock", "utils", "weather", "claude", "wm", "netspeed", "tray", "system"]; // settings::layout::BAR_MODULES
    let side_now = |id: &str| current.as_array().into_iter().flatten().find(|m| m[0] == id).and_then(|m| m[1].as_str().map(str::to_string));
    let kept: Vec<Value> = all.into_iter().filter(|id| !out.iter().any(|(i, _)| i == id)).map(|id| json!([id, side_now(id).unwrap_or_else(|| "hidden".into())])).collect();
    Some(Value::Array(out.iter().map(|(i, s)| json!([i, s])).chain(kept).collect()))
}

/// `bar.utils` from the preset's `utilButtons` switches; None when it has none of them.
fn utils_from_end4(u: &Value) -> Option<Value> {
    let map = [("showScreenSnip", "snip"), ("showColorPicker", "pick"), ("showWallpaperToggle", "wall"), ("showDarkModeToggle", "mode")];
    if !map.iter().any(|(k, _)| u.get(*k).is_some()) { return None; }
    Some(json!(map.iter().filter(|(k, _)| truthy(&u[*k])).map(|(_, id)| *id).collect::<Vec<_>>()))
}

/// Dock, notification/OSD timeouts and night light from a preset, merged over the current `cfg` sections.
fn extras_from_end4(preset: &Value, cfg: &Value) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    let secs = |ms: &Value| ms.as_f64().filter(|m| *m > 0.0).map(|m| (m / 1000.0 * 10.0).round() / 10.0);
    let mut island = cfg["island"].clone();
    if island.is_object() {
        if let Some(n) = secs(&preset["notifications"]["timeout"]) { island["notifySeconds"] = json!(n); }
        if let Some(o) = secs(&preset["osd"]["timeout"]) { island["osdSeconds"] = json!(o); }
        out.insert("island".into(), island);
    }
    let mut dock = cfg["dock"].clone();
    if dock.is_object() && preset["dock"].get("enable").is_some() {
        dock["enabled"] = json!(truthy(&preset["dock"]["enable"]));
        dock["side"] = json!(if preset["dock"]["position"] == "right" { "right" } else { "bottom" });
        if let Some(st) = preset["dock"]["style"].as_str().filter(|s| ["hug", "float", "plain"].contains(s)) { dock["style"] = json!(st); }
        if let Some(i) = preset["dock"]["iconSize"].as_f64().filter(|i| (20.0..=56.0).contains(i)) { dock["iconSize"] = json!(i.round() as i64); }
        if let Some(r) = preset["dock"]["radius"].as_f64().filter(|r| (0.0..=40.0).contains(r)) { dock["radius"] = json!(r.round() as i64); }
        out.insert("dock".into(), dock);
    }
    let mut bar = cfg["bar"].clone();
    if bar.is_object() {
        let mut changed = false;
        if let Some(style) = bar_style_from_end4(&preset["bar"]) { bar["style"] = json!(style); changed = true; }
        // the bar's edge is the user's choice: a preset never moves it
        if let Some(m) = modules_from_end4(&preset["bar"]["layouts"], &cfg["bar"]["modules"]) { bar["modules"] = m; changed = true; }
        if let Some(u) = utils_from_end4(&preset["bar"]["utilButtons"]) { bar["utils"] = u; changed = true; }
        if changed { out.insert("bar".into(), bar); }
    }
    let mut frame = cfg["frame"].clone();
    if frame.is_object() && preset["bar"].get("showFrame").is_some() {
        frame["enabled"] = json!(truthy(&preset["bar"]["showFrame"]) && int_or(&preset["appearance"]["fakeScreenRounding"], 0) != 0);
        if let Some(t) = preset["bar"]["frameThickness"].as_u64().filter(|t| (1..=40).contains(t)) { frame["thickness"] = json!(t); }
        out.insert("frame".into(), frame);
    }
    let mut night = cfg["nightLight"].clone();
    if let (true, Some(k)) = (night.is_object(), preset["light"]["night"]["colorTemperature"].as_f64().filter(|k| (1000.0..=10000.0).contains(k))) {
        night["kelvin"] = json!(k as i64);
        out.insert("nightLight".into(), night);
    }
    out
}

/// Preset widget keys -> desktop card ids.
/// `desktop` is config.json's current desktop section; anything but an object is returned untouched.
fn desktop_from_end4(pw: &Value, mut desktop: Value) -> Value {
    if pw.is_object() && desktop.is_object() {
        desktop["widgets"] = json!(CARD_IDS.iter().filter(|(k, _)| truthy(&pw[*k]["enable"])).map(|(_, id)| *id).collect::<Vec<_>>());
        if let Some(st) = pw["clock"]["style"].as_str().filter(|s| ["cookie", "digital", "pixel"].contains(s)) { desktop["clock"] = json!(st); }
        if let Some(t) = pw["customText"]["content"].as_str() { desktop["text"] = json!(t); }
        if let Some(v) = pw["clock"]["digital"]["vertical"].as_bool() { desktop["clockVertical"] = json!(v); }
        // "<rows>x<cols>" cell sizes of the cards that have a big layout
        // resources has no size in a preset: it is always a row of three tiles
        let sizes: serde_json::Map<String, Value> = CARD_IDS.iter().filter(|(k, _)| truthy(&pw[*k]["enable"]))
            .filter_map(|(k, id)| Some((id.to_string(), if *k == "resources" { "1x3".into() } else { pw[*k]["sizeMode"].as_str().filter(|m| m.len() == 3 && m.contains('x'))?.into() }))).collect();
        desktop["sizes"] = Value::Object(sizes);
        // world clock cities
        let zones: Vec<&str> = pw["worldClock"]["timezones"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if !zones.is_empty() && truthy(&pw["worldClock"]["enable"]) {
            let n = pw["worldClock"]["clockCount"].as_u64().filter(|n| *n > 0).unwrap_or(zones.len() as u64) as usize;
            desktop["worldClocks"] = json!(zones.iter().take(n).collect::<Vec<_>>());
        }
        let g = &pw["glass"];
        desktop["glass"] = if truthy(&g["enable"]) {
            let mut m = serde_json::Map::new();
            for k in ["haze", "pull", "rimWidth", "sheen", "split", "tintStrength", "vividness"] { if let Some(n) = g[k].as_f64() { m.insert(k.into(), json!(n)); } }
            if let Some(t) = g["tint"].as_str() { m.insert("tint".into(), json!(t)); }
            m.insert("enable".into(), json!(true));
            Value::Object(m)
        } else { json!({ "enable": false }) };
    }
    desktop
}

/// Maps the parts of an zor-ui preset that mean something on Windows. `viz` is config.json's visualizer.
fn windows_settings_from_end4(preset: &Value, viz: &Value) -> Value {
    let a = &preset["appearance"];
    let palette = a["palette"]["type"].as_str().filter(|s| !s.is_empty()).unwrap_or("auto");
    let scheme = if palette.starts_with("scheme-") { palette.replace("scheme-", "") } else { "auto".into() };
    let known = ["content", "expressive", "fidelity", "fruit-salad", "monochrome", "neutral", "rainbow", "tonal-spot", "vibrant"];
    let ws = &preset["bar"]["workspaces"];
    let tr = &a["transparency"];
    let transparency = if truthy(&tr["enable"]) {
        let f = tr["backgroundTransparency"].as_f64().or_else(|| tr["backgroundTransparency"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0);
        json!((f * 1000.0).round() / 1000.0)
    } else { json!(0) };
    let pv = &preset["background"]["widgets"]["visualizer"];
    let mut visualizer = if viz.is_object() { viz.clone() } else { json!({}) };
    if pv.get("enable").is_some() { visualizer["desktop"] = json!(truthy(&pv["enable"])); }
    let desktop = desktop_from_end4(&preset["background"]["widgets"], crate::conf::Config::load().get("desktop").clone());
    let mut config = json!({
            "workspaces": int_or(&ws["shown"], 10),
            "workspaceLabels": or(&ws["numberMap"], &json!([])),
            "alwaysShowNumbers": truthy(&ws["alwaysShowNumbers"]),
            "transparency": transparency,
            "visualizer": visualizer,
    });
    if desktop.is_object() { config["desktop"] = desktop; }
    for (k, v) in extras_from_end4(preset, &crate::conf::Config::load().0) { config[k] = v; }
    json!({
        "scheme": if known.contains(&scheme.as_str()) { scheme.as_str() } else { "auto" },
        "config": config,
        "positions": positions_from_end4(&preset["background"]["widgets"]),
        "wallpaper": preset["background"]["wallpaperPath"].as_str().unwrap_or(""),
    })
}

/// Shallow-merges `extra` into config.json, saves it and returns the result.
fn merge_config(extra: &Value) -> Value {
    let mut cfg = raw_config();
    if let (Some(c), Some(e)) = (cfg.as_object_mut(), extra.as_object()) {
        for (k, v) in e { c.insert(k.clone(), v.clone()); }
    }
    save_json(&pack().join("config.json"), &cfg);
    cfg
}

/// Merges card positions into state.json; the desktop reads them the next time it starts.
fn save_positions(pos: &Value) {
    let Some(map) = pos.as_object().filter(|m| !m.is_empty()) else { return };
    let path = root().join("state.json");
    let mut st = load_json(&path, json!({}));
    if !st["deskPos"].is_object() { st["deskPos"] = json!({}); }
    for (id, p) in map { st["deskPos"][id] = p.clone(); }
    save_json(&path, &st);
}

/// Preset card positions are in a 1920 px wide screen's coordinates; this puts them on the primary monitor's
/// work area (`left`, `top`, `width` physical px).
fn place_positions(pos: &Value, left: i32, top: i32, width: i32) -> Value {
    let k = width as f64 / 1920.0;
    let Some(m) = pos.as_object() else { return json!({}) };
    Value::Object(m.iter().filter_map(|(id, p)| Some((id.clone(), json!({ "x": left + (p["x"].as_f64()? * k).round() as i64 as i32, "y": top + (p["y"].as_f64()? * k).round() as i64 as i32 })))).collect())
}

fn apply_settings(cfg: &Config, mapped: &Value, wallpaper: Option<&str>, mode: Option<&str>) -> Value {
    // the preset's coordinates start at the monitor's corner, not under the bar
    let (mons, work) = crate::util::monitors();
    let mon = mons.iter().find(|m| m.left <= work.left && m.top <= work.top && m.right >= work.right && m.bottom >= work.bottom).copied().unwrap_or(work);
    save_positions(&place_positions(&mapped["positions"], mon.left, mon.top, mon.right - mon.left));
    merge_config(&mapped["config"]);
    crate::log::line(&format!("preset: config merged, switchwall {}", wallpaper.unwrap_or("(no wallpaper)")));
    let mut args: Vec<&str> = Vec::new();
    if let Some(w) = wallpaper { args.extend(["-Image", w]); }
    args.extend(["-Scheme", mapped["scheme"].as_str().unwrap_or("auto")]);
    if let Some(m @ ("dark" | "light")) = mode { args.extend(["-Mode", m]); }
    run_switchwall(cfg, &args)
}

fn preset_apply_online(cfg: &Config, name: &str) -> Value {
    crate::log::line(&format!("preset {name}: apply online"));
    if name.is_empty() || name.contains(['/', '\\']) { return err("bad name"); }
    let q = enc(name, false);
    let res: Result<Value, String> = (|| {
        let preset = get_json_e(&format!("{RAW}/main/presets/{q}/{q}.json"), &[], 20)?;
        crate::log::line(&format!("preset {name}: json fetched"));
        let mapped = windows_settings_from_end4(&preset, &raw_config()["visualizer"]);
        let mut wall = None;
        if let Some(w) = mapped["wallpaper"].as_str().filter(|w| !w.is_empty()) {
            let file = w.rsplit(['/', '\\']).next().unwrap_or(w);
            wall = Some(download(&format!("{RAW}/main/presets/{q}/{}", enc(file, false)), &wallpapers_dir().join("end4-presets").join(safe_id(name)))?);
        }
        let mut mapped = mapped;
        // the preset's own pictures for its image cards; one that fails to download leaves the card as it was
        for (id, file, shape, size) in preset_images(&preset["background"]["widgets"]) {
            let dir = pack().join("images").join(safe_id(name));
            let Ok(path) = download(&format!("{RAW}/main/presets/{q}/{}", enc(&file, false)), &dir) else { continue };
            let Some(stored) = path.file_name().map(|f| format!("{}/{}", safe_id(name), f.to_string_lossy())) else { continue };
            let mut img = json!({ "file": stored, "shape": shape });
            if let Some(s) = size { img["size"] = json!(s); }
            if mapped["config"]["desktop"]["images"].is_object() || mapped["config"]["desktop"].is_object() {
                mapped["config"]["desktop"]["images"][id] = img;
            }
        }
        let wall_s = wall.map(|p| p.display().to_string());
        crate::log::line(&format!("preset {name}: wallpaper {}", wall_s.as_deref().unwrap_or("none")));
        let mut saved = mapped.clone();
        saved["name"] = json!(name);
        saved["from"] = json!("online");
        saved["wallpaper"] = json!(wall_s.clone().unwrap_or_default());
        save_json(&root().join("presets").join(safe_id(name)).join("preset.json"), &saved);
        Ok(json!({ "ok": true, "name": name, "unsupported": unsupported_from_end4(&preset), "theme": apply_settings(cfg, &mapped, wall_s.as_deref(), None) }))
    })();
    if let Err(e) = &res { crate::log::line(&format!("preset {name}: failed: {e}")); }
    res.unwrap_or_else(err)
}

fn presets_local() -> Value {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root().join("presets")).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    dirs.sort_by_key(|d| d.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let items: Vec<Value> = dirs.iter().filter_map(|d| {
        let p = load_json(&d.join("preset.json"), Value::Null);
        if !truthy(&p) { return None; }
        let id = d.file_name()?.to_string_lossy().into_owned();
        Some(json!({ "id": id, "name": p.get("name").cloned().unwrap_or(json!(id)), "scheme": p["scheme"], "mode": p["mode"],
            "wallpaper": p.get("wallpaper").cloned().unwrap_or(json!("")), "from": p.get("from").cloned().unwrap_or(json!("local")) }))
    }).collect();
    json!({ "presets": items })
}

fn preset_save(name: &str) -> Value {
    let state = load_json(&root().join("state.json"), json!({}));
    let cfg = raw_config();
    let pid: String = safe_id(name).chars().take(60).collect();
    let pid = if pid.is_empty() { "preset".to_string() } else { pid };
    let keep: serde_json::Map<String, Value> = ["workspaces", "workspaceLabels", "alwaysShowNumbers", "transparency", "visualizer", "clockFormat", "tempUnit", "desktopWidgets", "desktop"]
        .iter().filter_map(|k| cfg.get(*k).map(|v| (k.to_string(), v.clone()))).collect();
    save_json(&root().join("presets").join(&pid).join("preset.json"), &json!({
        "name": name, "from": "local", "scheme": state.get("scheme").cloned().unwrap_or(json!("auto")),
        "mode": state.get("mode").cloned().unwrap_or(json!("dark")), "wallpaper": or(&state["image"], &json!("")),
        "color": state["color"], "config": keep }));
    json!({ "ok": true, "id": pid })
}

fn preset_apply_local(cfg: &Config, pid: &str) -> Value {
    crate::log::line(&format!("preset {pid}: apply local"));
    let p = load_json(&root().join("presets").join(safe_id(pid)).join("preset.json"), Value::Null);
    if !truthy(&p) { return err("preset not found"); }
    let wall = p["wallpaper"].as_str().filter(|w| !w.is_empty() && Path::new(w).exists());
    let scheme = p.get("scheme").and_then(Value::as_str).unwrap_or("auto");
    let mapped = json!({ "scheme": scheme, "config": p.get("config").cloned().unwrap_or(json!({})) });
    if let (None, Some(color)) = (wall, p["color"].as_str().filter(|c| !c.is_empty())) {
        merge_config(&mapped["config"]);
        let mode = p.get("mode").and_then(Value::as_str).unwrap_or("dark");
        return json!({ "ok": true, "theme": run_switchwall(cfg, &["-Color", color, "-Scheme", scheme, "-Mode", mode]) });
    }
    json!({ "ok": true, "theme": apply_settings(cfg, &mapped, wall, p["mode"].as_str()) })
}

fn preset_delete(pid: &str) -> Value {
    let d = root().join("presets").join(safe_id(pid));
    if d.exists() { let _ = std::fs::remove_dir_all(&d); }
    json!({ "ok": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_and_known() {
        assert_eq!(b64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(b64_decode("aGVsbG8="), b"hello");
        for n in 0..8u8 { let d: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect(); assert_eq!(b64_decode(&b64_encode(&d)), d); }
    }

    #[test]
    fn dpapi_roundtrip() {
        let blob = dpapi(b"sk-secret", true).expect("protect");
        assert_ne!(blob, b"sk-secret");
        assert_eq!(dpapi(&blob, false).as_deref(), Some(&b"sk-secret"[..]));
        assert!(dpapi(b"not a blob", false).is_none());
    }

    #[test]
    fn url_helpers() {
        assert_eq!(enc("a b/c~é", true), "a+b%2Fc~%C3%A9");
        assert_eq!(enc("a b", false), "a%20b");
        assert_eq!(unquote("a%20b%2"), "a b%2");
        assert_eq!(urlencode(&[("q", "x y".into()), ("n", "1".into())]), "q=x+y&n=1");
    }

    #[test]
    fn names() {
        assert_eq!(download_name("https://h.com/a/b%20c d.jpg?x=1"), "b_c_d.jpg");
        assert_eq!(download_name("https://h.com/"), "wallpaper.jpg");
        assert_eq!(safe_id("My preset!"), "My_preset_");
        assert_eq!(safe_id(".."), "_..");
    }

    #[test]
    fn translation_parse() {
        let d = json!([[["Hola ", "Hello ", null], ["mundo", "world"], null], null, "en"]);
        assert_eq!(parse_translation(&d, "auto"), Some(("Hola mundo".into(), json!("en"))));
        assert!(parse_translation(&json!({}), "auto").is_none());
    }

    #[test]
    fn booru_tag_string() {
        assert_eq!(booru_tags(&["a", "b"], false, "rating:safe"), "a b rating:safe");
        assert_eq!(booru_tags(&[], false, "rating:safe"), "rating:safe");
        assert_eq!(booru_tags(&["a"], true, "rating:safe"), "a");
    }

    #[test]
    fn sse_per_provider() {
        let g = json!({ "candidates": [{ "content": { "parts": [{ "text": "hi" }, { "text": "" }] } }] });
        assert_eq!(sse_events("gemini", &g), vec![json!({ "delta": "hi" })]);
        let a = json!({ "type": "content_block_delta", "delta": { "text": "yo" } });
        assert_eq!(sse_events("anthropic", &a), vec![json!({ "delta": "yo" })]);
        assert_eq!(sse_events("anthropic", &json!({ "type": "error", "error": { "message": "boom" } })), vec![json!({ "error": "boom" })]);
        assert_eq!(sse_events("openai", &json!({ "choices": [{ "delta": { "content": "x" } }] })), vec![json!({ "delta": "x" })]);
        assert!(sse_events("openai", &json!({ "choices": [] })).is_empty());
    }

    #[test]
    fn chat_requests() {
        let msgs = vec![json!({ "role": "assistant", "content": "a" })];
        let (url, h, body) = chat_request("gemini", &json!({ "id": "m" }), "K", &msgs, "sys", 0.5);
        assert!(url.contains("models/m:streamGenerateContent") && h.iter().any(|(k, v)| k == "x-goog-api-key" && v == "K"));
        assert_eq!(body["contents"][0]["role"], "model");
        let (url, _, body) = chat_request("ollama", &json!({ "id": "m" }), "", &msgs, "sys", 0.5);
        assert!(url.starts_with("http://localhost:11434") && body["messages"][0]["role"] == "system");
        let (url, _, _) = chat_request("custom", &json!({ "id": "m", "endpoint": "http://x/y" }), "", &msgs, "s", 0.5);
        assert_eq!(url, "http://x/y");
    }

    fn shot() -> Vec<Value> {
        vec![json!({ "role": "user", "content": "where is save?", "images": [{ "mime": "image/jpeg", "data": "QUJD", "label": "screen 1" }] })]
    }

    #[test]
    fn pictures_reach_every_provider_in_its_own_shape() {
        let (_, _, g) = chat_request("gemini", &json!({ "id": "m" }), "K", &shot(), "s", 0.5);
        assert_eq!(g["contents"][0]["parts"], json!([{ "text": "screen 1" }, { "inline_data": { "mime_type": "image/jpeg", "data": "QUJD" } }, { "text": "where is save?" }]));
        let (_, _, a) = chat_request("anthropic", &json!({ "id": "m" }), "K", &shot(), "s", 0.5);
        assert_eq!(a["messages"][0]["content"], json!([{ "type": "text", "text": "screen 1" }, { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "QUJD" } }, { "type": "text", "text": "where is save?" }]));
        for p in ["openai", "openrouter", "mistral", "ollama"] {
            let (_, _, o) = chat_request(p, &json!({ "id": "m" }), "K", &shot(), "s", 0.5);
            assert_eq!(o["messages"][1]["content"][1], json!({ "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,QUJD" } }), "{p}");
        }
    }

    #[test]
    fn text_only_messages_are_sent_exactly_as_before() {
        let msgs = vec![json!({ "role": "user", "content": "hi" })];
        let (_, _, a) = chat_request("anthropic", &json!({ "id": "m" }), "K", &msgs, "s", 0.5);
        assert_eq!(a["messages"][0]["content"], "hi");
        let (_, _, o) = chat_request("openai", &json!({ "id": "m" }), "K", &msgs, "s", 0.5);
        assert_eq!(o["messages"][1]["content"], "hi");
        let (_, _, g) = chat_request("gemini", &json!({ "id": "m" }), "K", &msgs, "s", 0.5);
        assert_eq!(g["contents"][0]["parts"], json!([{ "text": "hi" }]));
    }

    #[test]
    fn a_picture_without_words_sends_no_empty_text_block() {
        let msgs = vec![json!({ "role": "user", "content": "", "images": [{ "data": "QUJD" }] })];
        let (_, _, a) = chat_request("anthropic", &json!({ "id": "m" }), "K", &msgs, "s", 0.5);
        assert_eq!(a["messages"][0]["content"], json!([{ "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "QUJD" } }]), "mime defaults to jpeg");
        let (_, _, g) = chat_request("gemini", &json!({ "id": "m" }), "K", &msgs, "s", 0.5);
        assert_eq!(g["contents"][0]["parts"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn images_without_data_are_ignored() {
        assert!(images_of(&json!({ "images": [{ "mime": "image/png", "data": "" }, { "mime": "image/png" }] })).is_empty());
        assert!(images_of(&json!({ "content": "x" })).is_empty());
        assert_eq!(images_of(&json!({ "images": [{ "mime": "image/png", "data": "QQ==", "label": "l" }] })), [("image/png".to_string(), "QQ==".to_string(), "l".to_string())]);
    }

    #[test]
    fn a_model_id_tells_whether_it_takes_pictures() {
        for id in ["claude-cli:haiku", "claude-cli:sonnet", "gemini-2.5-flash", "gpt-4o-mini", "openrouter/auto", "claude-sonnet-5-5", "llava:13b", ""] {
            assert!(id_can_see(id), "{id}");
        }
        for id in ["gemini-cli:default", "codex-cli:default"] { assert!(!id_can_see(id), "{id}"); }
        // the same answer as asking by provider, for every model the app lists
        for m in default_models().into_iter().chain(super::sub_cli::models()).chain(super::claude_cli::models()) {
            assert_eq!(id_can_see(m["id"].as_str().unwrap()), can_see(m["provider"].as_str().unwrap()), "{}", m["id"]);
        }
    }

    #[test]
    fn only_some_providers_take_pictures() {
        for p in ["gemini", "anthropic", "openai", "openrouter", "mistral", "ollama", "custom", "claude-cli"] { assert!(can_see(p), "{p}"); }
        for p in ["gemini-cli", "codex-cli"] { assert!(!can_see(p), "{p}"); }
    }

    #[test]
    fn curl_config_quote() {
        assert_eq!(cfg_quote("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }

    #[test]
    fn preset_tree() {
        let t = json!({ "tree": [{ "path": "presets/Zed/Zed.json" }, { "path": "presets/abc/abc.json" }, { "path": "presets/abc/preview.png" }, { "path": "presets/x/y.json" }] });
        let (names, files) = preset_names(&t);
        assert_eq!(names, ["abc", "Zed"]);
        assert!(files.contains("presets/abc/preview.png"));
    }

    #[test]
    fn preset_positions_land_on_the_primary_monitor() {
        let p = json!({ "calendar": { "x": 228, "y": 324 }, "bad": { "x": "no" } });
        assert_eq!(place_positions(&p, 0, 0, 1920)["calendar"], json!({ "x": 228, "y": 324 }));
        assert_eq!(place_positions(&p, 10, 20, 3840)["calendar"], json!({ "x": 466, "y": 668 }));
        assert!(place_positions(&p, 0, 0, 1920).get("bad").is_none());
    }

    #[test]
    fn preset_entries_from_index() {
        let index = json!({ "Zed": { "author": "z" }, "abc": { "author": "a", "updated": "d" } });
        let names: Vec<String> = index.as_object().unwrap().keys().cloned().collect();
        let e = preset_entries(&names, &index, |_| true);
        assert_eq!(e[0]["name"], "abc");
        assert_eq!(e[0]["author"], "a");
        assert_eq!(e[1]["updated"], "");
        assert_eq!(e[1]["preview"], format!("{RAW}/main/presets/Zed/preview.png"));
        assert_eq!(preset_entries(&names, &index, |_| false)[0]["preview"], "");
    }

    #[test]
    fn end4_mapping() {
        let p = json!({ "appearance": { "palette": { "type": "scheme-tonal-spot" }, "transparency": { "enable": true, "backgroundTransparency": 0.12345 } },
            "bar": { "workspaces": { "shown": 7, "numberMap": ["a"], "alwaysShowNumbers": true } },
            "background": { "wallpaperPath": "C:/x/y.png", "widgets": { "visualizer": { "enable": false } } } });
        let m = windows_settings_from_end4(&p, &json!({ "bars": 64, "desktop": true }));
        assert_eq!(m["scheme"], "tonal-spot");
        assert_eq!(m["config"]["workspaces"], 7);
        assert_eq!(m["config"]["transparency"], 0.123);
        assert_eq!(m["config"]["visualizer"], json!({ "bars": 64, "desktop": false }));
        assert_eq!(m["wallpaper"], "C:/x/y.png");
        let d = windows_settings_from_end4(&json!({}), &Value::Null);
        assert_eq!((d["scheme"].clone(), d["config"]["workspaces"].clone(), d["config"]["transparency"].clone()), (json!("auto"), json!(10), json!(0)));
    }

    #[test]
    fn unsupported_widgets_are_reported() {
        let p = json!({ "background": { "widgets": { "clock": { "enable": true }, "glass": { "enable": true }, "visualizer": { "enable": true },
            "mystery": { "enable": true }, "todo": { "enable": false }, "weather": { "enable": false } } } });
        assert_eq!(unsupported_from_end4(&p), ["mystery"]);
        assert!(unsupported_from_end4(&json!({})).is_empty());
    }

    /// Network: every preset in the community repo maps without panicking, and the widgets we drop are only the known ones.
    /// `cargo test -- --ignored every_community_preset` to run it.
    #[test]
    #[ignore]
    fn every_community_preset_maps() {
        let index = get_json_e(&format!("{RAW}/gallery-data/index.json"), &[], 30).expect("index");
        let mut dropped = std::collections::BTreeMap::<String, usize>::new();
        for name in index["presets"].as_object().expect("presets").keys() {
            let q = enc(name, false);
            let Ok(p) = get_json_e(&format!("{RAW}/main/presets/{q}/{q}.json"), &[], 30) else { continue };
            let m = windows_settings_from_end4(&p, &json!({}));
            assert!(m["config"].is_object(), "{name}");
            for k in unsupported_from_end4(&p) { *dropped.entry(k).or_default() += 1; }
        }
        println!("widgets with no native card: {dropped:?}");
        assert!(dropped.is_empty(), "{dropped:?}");
    }

    #[test]
    fn preset_pictures_and_clocks() {
        let pw = json!({ "imageCard": { "enable": true, "path": "a b.jpg" }, "customImage": { "enable": true, "path": "x.jpg", "shape": "Flower", "size": 180.7 },
            "sticker": { "enable": false, "path": "s.png" }, "images": { "enable": true, "path": "../evil.png" },
            "worldClock": { "enable": true, "clockCount": 2, "timezones": ["Asia/Tokyo", "Asia/Seoul", "Asia/Manila"] } });
        let p = preset_images(&pw);
        assert_eq!(p.iter().map(|x| x.0).collect::<Vec<_>>(), ["image1", "image2"]);
        assert_eq!((p[1].2.as_str(), p[1].3), ("flower", Some(181)));
        assert_eq!(p[0].3, None);
        assert!(preset_images(&json!({ "imageCard": { "enable": true, "path": "a/b.jpg" } })).is_empty(), "no path tricks");
        assert_eq!(desktop_from_end4(&pw, json!({}))["worldClocks"], json!(["Asia/Tokyo", "Asia/Seoul"]));
    }

    #[test]
    fn preset_glass_is_kept() {
        let pw = json!({ "glass": { "enable": true, "haze": 6, "pull": 80, "tint": "black", "tintStrength": 0.35, "junk": 1 } });
        let d = desktop_from_end4(&pw, json!({}));
        assert_eq!(d["glass"], json!({ "enable": true, "haze": 6.0, "pull": 80.0, "tint": "black", "tintStrength": 0.35 }));
        assert_eq!(desktop_from_end4(&json!({ "glass": { "enable": false, "haze": 6 } }), json!({}))["glass"], json!({ "enable": false }));
    }

    #[test]
    fn preset_card_sizes_are_kept() {
        let pw = json!({ "calendar": { "enable": true, "sizeMode": "2x3" }, "weather": { "enable": true, "sizeMode": "huge" }, "media": { "enable": false, "sizeMode": "1x2" } });
        let d = desktop_from_end4(&pw, json!({}));
        assert_eq!(d["sizes"], json!({ "calendar": "2x3" }));
        assert_eq!(desktop_from_end4(&json!({ "resources": { "enable": true } }), json!({}))["sizes"], json!({ "resources": "1x3" }));
    }

    #[test]
    fn preset_widgets_become_desktop_cards() {
        let pw = json!({ "clock": { "enable": true, "style": "pixel" }, "weather": { "enable": false }, "sticker": { "enable": true },
            "userCard": { "enable": true }, "imageCard": { "enable": true }, "customImage": { "enable": true }, "images": { "enable": false }, "customText": { "enable": true, "content": "hi" }, "glass": { "enable": true } });
        let d = desktop_from_end4(&pw, json!({ "widgets": ["weather"], "worldClocks": ["Asia/Tokyo"], "clock": "cookie", "text": "" }));
        assert_eq!(d["widgets"], json!(["clock", "greet", "text", "image1", "image2", "sticker"]));
        assert_eq!((d["clock"].clone(), d["text"].clone(), d["worldClocks"].clone()), (json!("pixel"), json!("hi"), json!(["Asia/Tokyo"])));
        let d = desktop_from_end4(&json!({ "clock": { "enable": true, "style": "bogus" } }), json!({ "clock": "cookie" }));
        assert_eq!((d["clock"].clone(), d["widgets"].clone()), (json!("cookie"), json!(["clock"])));
        let d = desktop_from_end4(&json!({ "clock": { "enable": true, "style": "digital", "digital": { "vertical": true } } }), json!({}));
        assert_eq!(d["clockVertical"], json!(true));
        assert_eq!(desktop_from_end4(&Value::Null, json!({ "widgets": ["clock"] })), json!({ "widgets": ["clock"] }));
        assert_eq!(desktop_from_end4(&pw, Value::Null), Value::Null);
    }

    #[test]
    fn angel_style_preset_applies_on_a_config_without_desktop() {
        // config.json written before the user touched Desktop settings has no "desktop" key; the defaults supply it.
        let base = crate::conf::Config::parse("{}").get("desktop").clone();
        let d = desktop_from_end4(&json!({ "clock": { "enable": true, "style": "pixel" }, "weather": { "enable": false } }), base);
        assert_eq!((d["widgets"].clone(), d["clock"].clone()), (json!(["clock"]), json!("pixel")));
    }

    #[test]
    fn preset_positions_and_extras() {
        let pw = json!({ "clock": { "enable": true, "x": 60.4, "y": 84 }, "weather": { "enable": false, "x": 1, "y": 2 }, "sticker": { "enable": true }, "userCard": { "enable": true, "x": 24, "y": 108 } });
        assert_eq!(positions_from_end4(&pw), json!({ "clock": { "x": 60, "y": 84 }, "greet": { "x": 24, "y": 108 } }));
        let cfg = json!({ "island": { "notifySeconds": 5, "osdSeconds": 2, "keep": 1 }, "dock": { "enabled": false, "pinned": [1] }, "nightLight": { "kelvin": 4500 } });
        let p = json!({ "notifications": { "timeout": 7000 }, "osd": { "timeout": 1000 }, "dock": { "enable": true, "position": "right", "style": "float", "iconSize": 33, "radius": 23 }, "light": { "night": { "colorTemperature": 5000 } } });
        let e = extras_from_end4(&p, &cfg);
        assert_eq!(e["island"], json!({ "notifySeconds": 7.0, "osdSeconds": 1.0, "keep": 1 }));
        assert_eq!(e["dock"], json!({ "enabled": true, "side": "right", "style": "float", "iconSize": 33, "radius": 23, "pinned": [1] }));
        assert_eq!(e["nightLight"], json!({ "kelvin": 5000 }));
        assert_eq!((bar_style_from_end4(&json!({ "borderless": "segmented", "cornerStyle": 1 })), bar_style_from_end4(&json!({ "cornerStyle": 1 })),
            bar_style_from_end4(&json!({ "cornerStyle": 2 })), bar_style_from_end4(&json!({ "cornerStyle": 9 })), bar_style_from_end4(&json!({}))),
            (Some("islands"), Some("float"), Some("flat"), None, None));
        assert_eq!((bar_style_from_end4(&json!({ "borderless": "pills", "cornerStyle": 0 })), bar_style_from_end4(&json!({ "borderless": "transparent", "cornerStyle": 3 })),
            bar_style_from_end4(&json!({ "borderless": "separated" }))), (Some("islands"), Some("clear"), Some("islands")));
        let b = extras_from_end4(&json!({ "bar": { "cornerStyle": 2 } }), &json!({ "bar": { "style": "float", "groups": "filled" } }));
        assert_eq!(b["bar"], json!({ "style": "flat", "groups": "filled" }));
        let l = json!({ "leftLayout": ["activeWindow"], "middleLayout": ["workspaces", "media", "systemIcons", "mystery"], "rightLayout": [] });
        let m = modules_from_end4(&l, &json!([])).unwrap();
        assert_eq!((m[0].clone(), m[1].clone(), m[3].clone()), (json!(["window", "left"]), json!(["workspaces", "center"]), json!(["system", "center"]))); // "mystery" is skipped
        let l = json!({ "leftLayout": [], "middleLayout": ["workspaces", "media", "weatherBar", "clockWidget", "utilButtons", "systemIcons", "networkSpeed"], "rightLayout": [] });
        let m = modules_from_end4(&l, &json!([])).unwrap();
        assert_eq!(m[0], json!(["workspaces", "center"]));
        assert_eq!(m[5], json!(["system", "center"]));
        assert!(m.as_array().unwrap().contains(&json!(["launcher", "hidden"])));
        assert_eq!(m.as_array().unwrap().len(), 14);
        assert_eq!(modules_from_end4(&json!({}), &json!([])), None);
        // a module the preset leaves out keeps its place: the island stays shown
        let m = modules_from_end4(&l, &json!([["island", "center"], ["launcher", "hidden"]])).unwrap();
        assert!(m.as_array().unwrap().contains(&json!(["island", "center"])) && m.as_array().unwrap().contains(&json!(["launcher", "hidden"])));
        assert_eq!(m.as_array().unwrap().len(), 14);
        // a preset never moves the bar
        let b = extras_from_end4(&json!({ "bar": { "bottom": true, "vertical": true, "cornerStyle": 2 } }), &json!({ "bar": { "position": "top", "style": "float" } }));
        assert_eq!(b["bar"]["position"], "top");
        assert_eq!(utils_from_end4(&json!({ "showScreenSnip": false, "showColorPicker": true, "showWallpaperToggle": true })), Some(json!(["pick", "wall"])));
        assert_eq!(utils_from_end4(&json!({})), None);
        let cfg2 = json!({ "frame": { "enabled": false, "thickness": 5, "radius": 20 } });
        let f = extras_from_end4(&json!({ "bar": { "showFrame": true, "frameThickness": 8 }, "appearance": { "fakeScreenRounding": 1 } }), &cfg2);
        assert_eq!(f["frame"], json!({ "enabled": true, "thickness": 8, "radius": 20 }));
        let f = extras_from_end4(&json!({ "bar": { "showFrame": true }, "appearance": { "fakeScreenRounding": 0 } }), &cfg2);
        assert_eq!(f["frame"]["enabled"], json!(false));
        assert!(extras_from_end4(&json!({}), &cfg).get("dock").is_none() && extras_from_end4(&json!({ "osd": { "timeout": 0 } }), &cfg)["island"] == cfg["island"]);
    }
}
