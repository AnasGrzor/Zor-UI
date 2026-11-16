//! LocalSend v2 sender. Discovery and streaming uploads run off the UI thread.
//! Protocol: https://github.com/localsend/protocol
use crate::app::{self, Ctx};
use serde_json::{json, Value};
use std::{io::{Read, Write}, net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket}, path::PathBuf,
    sync::{Mutex, OnceLock}, time::{Duration, Instant}};

#[derive(Clone, Debug)]
pub struct Phone { pub alias: String, pub fingerprint: String, pub addr: SocketAddr, pub protocol: String }
#[derive(Clone, Default)]
pub struct Status {
    pub busy: bool,
    pub discovering: bool,
    pub message: String,
    pub phones: Vec<Phone>,
    pending: Vec<PathBuf>,
    pub until: f64,
}
static STATUS: OnceLock<Mutex<Status>> = OnceLock::new();
static DRAG_HOVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn drag_hover() -> bool { DRAG_HOVER.load(std::sync::atomic::Ordering::Relaxed) }
pub fn set_drag_hover(on: bool) {
    if DRAG_HOVER.swap(on, std::sync::atomic::Ordering::Relaxed) != on {
        app::post(Box::new(|_| {}));
    }
}
fn state() -> &'static Mutex<Status> { STATUS.get_or_init(Default::default) }
pub fn status() -> Status { state().lock().unwrap().clone() }
fn message(text: impl Into<String>, busy: bool) {
    let mut s = state().lock().unwrap();
    s.message = text.into(); s.busy = busy; s.until = crate::hub::now_ms() + 8000.0;
    app::post(Box::new(|_| {}));
}
fn info() -> Value {
    // Keep our identity across restarts so the phone can remember Zor UI as a favorite.
    static ID: OnceLock<String> = OnceLock::new();
    let fingerprint = ID.get_or_init(|| {
        let path = crate::conf::data_dir().join("localsend-id");
        if let Ok(id) = std::fs::read_to_string(&path) {
            if id.trim().len() == 36 { return id.trim().to_string(); }
        }
        let guid = unsafe { windows::Win32::System::Com::CoCreateGuid() }
            .unwrap_or_else(|e| { crate::log::line(&format!("LocalSend identity generation failed: {e}")); windows::core::GUID::zeroed() });
        let id = format!("{guid:?}");
        let _ = std::fs::create_dir_all(crate::conf::data_dir());
        let _ = std::fs::write(path, &id);
        id
    });
    json!({"alias":"Zor UI", "version":"2.0", "deviceModel":"Windows", "deviceType":"desktop",
        "fingerprint":fingerprint, "port":53317, "protocol":"http", "download":false})
}
fn phone(v: &Value, source: SocketAddr) -> Option<Phone> {
    if !source.ip().is_ipv4() || source.ip().is_loopback() { crate::log::line(&format!("LocalSend ignored response from non-LAN/IPv4 address {source}")); return None; }
    if !matches!(v["deviceType"].as_str(), Some("mobile" | "desktop" | "web" | "headless" | "server")) {
        crate::log::line(&format!("LocalSend ignored response from {source}: unsupported or missing deviceType {:?}", v["deviceType"])); return None;
    }
    let fingerprint = v["fingerprint"].as_str()?.to_string();
    if fingerprint.is_empty() { return None; }
    let protocol = v["protocol"].as_str().unwrap_or("https");
    if !matches!(protocol, "http" | "https") { return None; }
    let port = v["port"].as_u64().unwrap_or(53317);
    if port == 0 || port > u16::MAX as u64 { return None; }
    let result = Phone { alias: v["alias"].as_str().unwrap_or("Phone").chars().take(60).collect(), fingerprint,
        addr: SocketAddr::new(source.ip(), port as u16), protocol: protocol.into() };
    crate::log::line(&format!("LocalSend discovered {} at {} ({})", result.alias, result.addr, result.protocol));
    Some(result)
}
fn lan_interface_ip(group: Ipv4Addr) -> std::io::Result<Ipv4Addr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    probe.connect((group, 53317))?;
    match probe.local_addr()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Ok(ip),
        _ => Err(std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no LAN interface selected for LocalSend multicast")),
    }
}
fn discover() -> Result<Vec<Phone>, String> {
    use socket2::{Domain, Protocol, Socket, Type};
    crate::log::line("LocalSend discovery started: IPv4 multicast 224.0.0.167:53317");
    let run = || -> std::io::Result<(Vec<Phone>, Vec<String>)> {
        let group = Ipv4Addr::new(224,0,0,167);
        // Pick the route before binding the receiving socket. Binding to 0.0.0.0
        // made Windows send announcements from the wrong adapter on multi-NIC PCs.
        let interface_ip = lan_interface_ip(group)?;
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        socket.set_multicast_if_v4(&interface_ip)?;
        socket.bind(&SocketAddr::new(interface_ip.into(), 53317).into())?;
        let socket: UdpSocket = socket.into();
        let mut issues = Vec::new();
        if let Err(e) = socket.join_multicast_v4(&group, &interface_ip) {
            issues.push(format!("multicast group join failed: {e}"));
        }
        let _ = socket.set_multicast_loop_v4(false);
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        // Receivers reply over HTTP first. A separate ephemeral port avoids colliding
        // with a desktop LocalSend app that is already listening on 53317.
        let local_addr = socket.local_addr()?;
        crate::log::line(&format!("LocalSend discovery socket {local_addr}, selected LAN interface {interface_ip}"));
        let listener = TcpListener::bind((interface_ip, 0))?;
        listener.set_nonblocking(true)?;
        let mut announcement = info(); announcement["announce"] = json!(true);
        announcement["port"] = json!(listener.local_addr()?.port());
        let sent = socket.send_to(announcement.to_string().as_bytes(), (group, 53317));
        match sent { Ok(n) => crate::log::line(&format!("LocalSend multicast announcement sent: {n} bytes from {local_addr}")), Err(e) => issues.push(format!("multicast send failed: {e}")) }
        let start = Instant::now();
        let mut phones: Vec<Phone> = vec![];
        let mut buf = [0u8; 65536];
        let mut responses = 0usize;
        while start.elapsed() < Duration::from_secs(4) {
            while let Ok((mut stream, source)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                let callback_start = Instant::now();
                while request.len() < 65536 && callback_start.elapsed() < Duration::from_millis(500) {
                    let Ok(n) = stream.read(&mut chunk) else { break };
                    if n == 0 { break; }
                    request.extend_from_slice(&chunk[..n]);
                    if let Some(v) = callback_body(&request) {
                        responses += 1;
                        crate::log::line(&format!("LocalSend HTTP registration reply from {source}: {v}"));
                        if let Some(p) = phone(&v, source) {
                            if !phones.iter().any(|x| x.fingerprint == p.fingerprint) { phones.push(p); }
                        }
                        let body = info().to_string();
                        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                        break;
                    }
                }
                if start.elapsed() >= Duration::from_secs(4) { break; }
            }
            match socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    responses += 1;
                    match serde_json::from_slice::<Value>(&buf[..n]) {
                    Ok(v) => {
                    crate::log::line(&format!("LocalSend UDP response from {from}: {v}"));
                    if let Some(p) = phone(&v, from) {
                        if !phones.iter().any(|x| x.fingerprint == p.fingerprint) { phones.push(p); }
                    }
                    }
                    Err(e) => crate::log::line(&format!("LocalSend ignored invalid JSON from {from}: {e}")),
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => { issues.push(format!("multicast receive failed: {e}")); break; }
            }
        }
        phones.sort_by(|a,b| a.alias.cmp(&b.alias));
        crate::log::line(&format!("LocalSend discovery finished: {} device(s), {responses} response(s); {}", phones.len(), if issues.is_empty() { "no socket errors".into() } else { issues.join("; ") }));
        if phones.is_empty() && responses == 0 {
            issues.push("no replies received; check that the phone is open, both devices share the same Wi-Fi, and Windows Firewall allows local UDP/TCP traffic".into());
        }
        Ok((phones, issues))
    };
    match run() {
        Ok((phones, issues)) => {
            if phones.is_empty() {
                // LocalSend v2 legacy discovery scans local IPv4 subnets over HTTP.
                let fallback = legacy_scan();
                let mut phones = phones;
                for p in fallback { if !phones.iter().any(|x| x.fingerprint == p.fingerprint) { phones.push(p); } }
                crate::log::line(&format!("LocalSend legacy scan found {} device(s); discovery notes: {}", phones.len(), issues.join("; ")));
                Ok(phones)
            } else { Ok(phones) }
        }
        Err(e) => { crate::log::line(&format!("LocalSend multicast setup failed: {e}; trying legacy scan"));
            let phones = legacy_scan(); if phones.is_empty() { Err(format!("Phone discovery failed: {e}. See zorui.log for details.")) } else { Ok(phones) }
        }
    }
}

fn legacy_scan() -> Vec<Phone> {
    crate::log::line("LocalSend legacy discovery scan started");
    let selected_ip = match lan_interface_ip(Ipv4Addr::new(224,0,0,167)) {
        Ok(ip) => ip,
        Err(e) => { crate::log::line(&format!("LocalSend fallback could not select LAN interface: {e}")); return Vec::new(); }
    };
    // Query the interface table so VPN, Ethernet and Wi-Fi subnets can all be scanned.
    let mut size = 16 * 1024u32;
    let mut bytes = vec![0u8; size as usize];
    let get_adapters = |bytes: &mut [u8], size: &mut u32| unsafe { windows::Win32::NetworkManagement::IpHelper::GetAdaptersAddresses(
        windows::Win32::Networking::WinSock::AF_INET.0 as u32,
        windows::Win32::NetworkManagement::IpHelper::GET_ADAPTERS_ADDRESSES_FLAGS(0), None,
        Some(bytes.as_mut_ptr() as *mut _), size) };
    let result = get_adapters(&mut bytes, &mut size);
    if result != 0 {
        bytes.resize(size as usize, 0);
        let retry = get_adapters(&mut bytes, &mut size);
        if retry != 0 { crate::log::line(&format!("LocalSend adapter enumeration failed: Windows error {retry}")); }
    }
    let mut prefixes: Vec<([u8;3],u8)> = Vec::new();
    let mut adapter = bytes.as_ptr() as *const windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH;
    unsafe {
        while !adapter.is_null() {
            let a = &*adapter;
            let mut unicast = a.FirstUnicastAddress;
            while !unicast.is_null() {
                let u = &*unicast;
                if !u.Address.lpSockaddr.is_null() {
                    let sa = &*(u.Address.lpSockaddr as *const windows::Win32::Networking::WinSock::SOCKADDR_IN);
                    let ip = sa.sin_addr.S_un.S_un_b.s_b1;
                    let octets = [ip,sa.sin_addr.S_un.S_un_b.s_b2,sa.sin_addr.S_un.S_un_b.s_b3,sa.sin_addr.S_un.S_un_b.s_b4];
                    let mask = u.OnLinkPrefixLength.min(30);
                    if octets[0] != 0 && octets[0] != 127 && octets[0] < 224 && mask >= 16 {
                        let net = u32::from_be_bytes(octets) & (u32::MAX << (32-mask));
                        let prefix = net.to_be_bytes();
                        let p = ([prefix[0],prefix[1],prefix[2]], mask);
                        if !prefixes.contains(&p) { prefixes.push(p); }
                    }
                }
                unicast = u.Next;
            }
            adapter = a.Next;
        }
    }
    let selected = u32::from_be_bytes(selected_ip.octets());
    prefixes.retain(|(prefix, mask)| {
        // Probe only the physical LAN that Windows selected for multicast. This
        // avoids spending a minute walking unrelated Hyper-V/VPN subnets.
        let mask_bits = u32::MAX << (32 - *mask);
        let network = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], 0]);
        selected & mask_bits == network & mask_bits
    });
    crate::log::line(&format!("LocalSend scan interfaces: {}", prefixes.iter().map(|(p,m)| format!("{}.{}.{}.{}/{}",p[0],p[1],p[2],0,m)).collect::<Vec<_>>().join(", ")));
    let payload = info().to_string(); let mut phones: Vec<Phone> = Vec::new();
    for (prefix,mask) in prefixes {
        let count = 1u32 << (32-mask);
        for host in 1..count.saturating_sub(1).min(1024) {
            let addr = (u32::from_be_bytes([prefix[0],prefix[1],prefix[2],0]) | host).to_be_bytes();
            let ip = Ipv4Addr::from(addr);
            let Ok(mut stream) = std::net::TcpStream::connect_timeout(&SocketAddr::new(ip.into(),53317), Duration::from_millis(35)) else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
            let request = format!("POST /api/localsend/v2/register HTTP/1.1\r\nHost: {ip}:53317\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len());
            if stream.write_all(request.as_bytes()).is_err() { continue; }
            let mut response = String::new(); let _ = stream.read_to_string(&mut response);
            if let Some((_,body)) = response.split_once("\r\n\r\n") {
                if let Ok(v) = serde_json::from_str::<Value>(body) {
                    crate::log::line(&format!("LocalSend legacy response from {ip}: {v}"));
                    if let Some(p) = phone(&v, SocketAddr::new(ip.into(),53317)) {
                        if !phones.iter().any(|x| x.fingerprint == p.fingerprint) { phones.push(p); }
                    }
                }
            }
        }
    }
    crate::log::line(&format!("LocalSend legacy discovery finished: {} device(s)", phones.len()));
    phones
}

fn callback_body(request: &[u8]) -> Option<Value> {
    let boundary = request.windows(4).position(|w| w == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&request[..boundary]).ok()?;
    if !headers.starts_with("POST /api/localsend/v2/register HTTP/1.") { return None; }
    let length: usize = headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.eq_ignore_ascii_case("content-length") { value.trim().parse().ok() } else { None }
    })?;
    if length > 60000 || request.len() < boundary + 4 + length { return None; }
    serde_json::from_slice(&request[boundary+4..boundary+4+length]).ok()
}

/// Never switch to a different phone when the remembered one is unavailable.
fn target(phones: &[Phone], remembered: &str) -> Option<Phone> {
    if !remembered.is_empty() { phones.iter().find(|p| p.fingerprint == remembered).cloned() }
    else if phones.len() == 1 { phones.first().cloned() } else { None }
}
pub fn refresh() {
    { let mut s = state().lock().unwrap(); if s.busy { return; } s.busy = true; s.discovering = true; }
    message("Looking for phones...", true);
    std::thread::spawn(|| {
        match discover() {
            Ok(phones) => {
                let empty = phones.is_empty(); state().lock().unwrap().phones = phones;
                message(if empty { "No phone found. Open LocalSend on the same Wi-Fi and try again." }
                    else { "Choose your phone below. Drop files here to send." }, false);
            }
            Err(e) => message(e, false),
        }
    });
}
pub fn choose(index: usize, cx: &mut Ctx) {
    let s = status(); if s.busy { return; }
    let Some(p) = s.phones.get(index).cloned() else { crate::log::line(&format!("LocalSend phone selection ignored: index {index}, devices {}", s.phones.len())); return };
    crate::log::line(&format!("LocalSend phone selected: {} at {} ({})", p.alias, p.addr, p.protocol));
    let fingerprint = p.fingerprint.clone(); let alias = p.alias.clone();
    cx.state.update(|v| { v["localsendPhone"] = json!({"fingerprint":fingerprint,"alias":alias,"address":p.addr.to_string(),"protocol":p.protocol}); });
    state().lock().unwrap().pending.clear();
    if s.pending.is_empty() { message(format!("Ready to send to {}", p.alias), false); }
    else { start_upload(s.pending, p); }
}
pub fn drop_files(paths: Vec<PathBuf>, cx: &mut Ctx) {
    { let mut s = state().lock().unwrap(); if s.busy { return; } s.busy = true; s.pending.clear(); }
    if paths.is_empty() || paths.iter().any(|p| !p.is_file()) {
        message("Drop files only. Folders are not supported.", false); return;
    }
    let remembered = cx.state.v["localsendPhone"]["fingerprint"].as_str().unwrap_or("").to_string();
    // Discovery replies can be throttled by the receiver. A selected endpoint
    // remains usable even when a second multicast announcement gets no reply.
    if let Some(selected) = target(&status().phones, &remembered) {
        crate::log::line(&format!("LocalSend using selected endpoint directly: {} at {}", selected.alias, selected.addr));
        start_upload(paths, selected);
        return;
    }
    let saved = &cx.state.v["localsendPhone"];
    if !remembered.is_empty() {
        if let (Some(address), Some(protocol)) = (saved["address"].as_str(), saved["protocol"].as_str()) {
            if let Ok(addr) = address.parse::<SocketAddr>() {
                if addr.ip().is_ipv4() && !addr.ip().is_loopback() && matches!(protocol, "http" | "https") {
                    let selected = Phone { alias: saved["alias"].as_str().unwrap_or("Phone").into(),
                        fingerprint: remembered.clone(), addr, protocol: protocol.into() };
                    crate::log::line(&format!("LocalSend using saved phone endpoint: {}", selected.addr));
                    start_upload(paths, selected);
                    return;
                }
            }
        }
    }
    state().lock().unwrap().discovering = true;
    message("Finding your phone...", true);
    std::thread::spawn(move || {
        match discover() {
            Ok(phones) => {
                let selected = target(&phones, &remembered);
                state().lock().unwrap().phones = phones;
                if let Some(p) = selected {
                    app::post(Box::new(move |cx| {
                        cx.state.update(|v| v["localsendPhone"] = json!({"fingerprint":p.fingerprint,"alias":p.alias,"address":p.addr.to_string(),"protocol":p.protocol}));
                        start_upload(paths, p);
                    }));
                } else {
                    let mut s = state().lock().unwrap(); s.pending = paths; s.busy = false;
                    s.message = if s.phones.is_empty() { "Phone unavailable. Open LocalSend on the same Wi-Fi, then drop again." }
                        else { "Select your phone below to send the dropped files." }.into();
                    s.until = crate::hub::now_ms() + 8000.0;
                    drop(s);
                    app::post(Box::new(|cx| cx.actions.push(app::Action::open("island-panel"))));
                }
            }
            Err(e) => message(e, false),
        }
    });
}
fn start_upload(paths: Vec<PathBuf>, p: Phone) {
    state().lock().unwrap().discovering = false;
    crate::log::line(&format!("LocalSend send started: {} file(s) to {} at {} ({})", paths.len(), p.alias, p.addr, p.protocol));
    message(format!("Waiting for {} to accept...", p.alias), true);
    std::thread::spawn(move || {
        match send(&paths, &p, |text| message(text, true)) {
            Ok(count) => message(format!("Sent {count} file(s) to {}", p.alias), false),
            Err(e) => { crate::log::line(&format!("LocalSend send failed for {}: {e}", p.alias)); message(e, false) },
        }
    });
}
fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() }
        else { format!("%{b:02X}") }).collect()
}
fn metadata(paths: &[PathBuf]) -> Result<Value, String> {
    let mut files = serde_json::Map::new();
    for (i,p) in paths.iter().enumerate() {
        let m = p.metadata().map_err(|e| format!("Cannot read {}: {e}", p.display()))?;
        if !m.is_file() { return Err("Drop files only. Folders are not supported.".into()); }
        let id = i.to_string();
        files.insert(id.clone(), json!({"id":id,"fileName":p.file_name().unwrap_or_default().to_string_lossy(),
            "size":m.len(),"fileType":"application/octet-stream"}));
    }
    Ok(json!({"info":info(),"files":files}))
}
// LocalSend uses self-signed certificates. Curl streams files without loading them into RAM.
fn request(url: &str, body: Option<&Value>, file: Option<&PathBuf>, seconds: &str) -> Result<(u16, String), String> {
    let endpoint = url.split('?').next().unwrap_or(url);
    crate::log::line(&format!("LocalSend HTTP request: {}{}", if file.is_some() { "upload " } else if body.is_some() { "prepare " } else { "cancel " }, endpoint));
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let identity_path = crate::conf::data_dir().join("localsend-client.pem");
        let pem = match std::fs::read(&identity_path) {
            Ok(pem) => pem,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cert = rcgen::generate_simple_self_signed(vec!["zor-ui.local".into()]).map_err(|e| e.to_string())?;
                let pem = format!("{}{}", cert.cert.pem(), cert.key_pair.serialize_pem()).into_bytes();
                std::fs::write(&identity_path, &pem).map_err(|e| e.to_string())?;
                pem
            }
            Err(e) => return Err(e.to_string()),
        };
        let identity = reqwest::Identity::from_pem(&pem).map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder().use_rustls_tls().identity(identity).danger_accept_invalid_certs(true)
            .no_proxy().connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(seconds.parse().unwrap_or(120)))
            .build().map_err(|e| e.to_string())?;
        let mut req = client.post(url);
        if let Some(body) = body {
            req = req.header("Content-Type", "application/json").body(body.to_string());
        }
        if let Some(path) = file {
            let source = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let length = source.metadata().map_err(|e| e.to_string())?.len();
            let stream = tokio_util::io::ReaderStream::new(tokio::fs::File::from_std(source));
            req = req.header("Content-Type", "application/octet-stream")
                .header("Content-Length", length).body(reqwest::Body::wrap_stream(stream));
        }
        let response = req.send().await.map_err(|e| {
            crate::log::line(&format!("LocalSend HTTPS request failed: {e:?}"));
            format!("Phone transfer failed: {e}")
        })?;
        let code = response.status().as_u16();
        crate::log::line(&format!("LocalSend HTTP response: {endpoint} returned {code}"));
        let text = response.text().await.map_err(|e| e.to_string())?;
        Ok((code, text))
    })
}
fn check(code: u16) -> Result<(), String> {
    if !(200..=299).contains(&code) { crate::log::line(&format!("LocalSend receiver returned HTTP {code}")); }
    match code { 200..=299 => Ok(()),
        401 => Err("Your phone requires a PIN. Turn off its LocalSend PIN to use island sending.".into()),
        403 => Err("Your phone declined the transfer.".into()),
        409 => Err("Your phone is busy with another transfer. Try again when it finishes.".into()),
        _ => Err(format!("LocalSend transfer failed (HTTP {code}). Try again.")) }
}
fn send(paths: &[PathBuf], p: &Phone, progress: impl Fn(String)) -> Result<usize, String> {
    let base = format!("{}://{}/api/localsend/v2", p.protocol, p.addr);
    let meta = metadata(paths)?;
    crate::log::line(&format!("LocalSend prepare metadata: {} file(s), {} bytes", paths.len(), paths.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum::<u64>()));
    let (code, body) = request(&format!("{base}/prepare-upload"), Some(&meta), None, "120")?;
    check(code)?;
    if code == 204 { return Err("Phone reports no files needed. No files were uploaded.".into()); }
    let reply: Value = serde_json::from_str(&body).map_err(|_| "Invalid LocalSend preparation response")?;
    crate::log::line(&format!("LocalSend prepare accepted: session {}, {} file token(s)", reply["sessionId"].as_str().unwrap_or("missing"), reply["files"].as_object().map_or(0, |f| f.len())));
    let session = reply["sessionId"].as_str().ok_or("Missing LocalSend session")?;
    let upload = || -> Result<usize, String> {
        let mut count = 0;
        for (i,path) in paths.iter().enumerate() {
            let id = i.to_string();
            let Some(token) = reply["files"][&id].as_str() else { continue };
            progress(format!("Sending file {} of {} to {}...", i+1, paths.len(), p.alias));
            let url = format!("{base}/upload?sessionId={}&fileId={id}&token={}", encode(session), encode(token));
            let (code, _) = request(&url, None, Some(path), "3600")?;
            check(code)?; count += 1;
        }
        if count != paths.len() { return Err(format!("Sent {count} of {} files. Your phone did not accept the remaining files.", paths.len())); }
        Ok(count)
    };
    let result = upload();
    if result.is_err() { let _ = request(&format!("{base}/cancel?sessionId={}", encode(session)), None, None, "5"); }
    match result {
        Ok(count) => { crate::log::line(&format!("LocalSend send completed: {count} file(s)")); Ok(count) }
        Err(e) => { crate::log::line(&format!("LocalSend upload failed: {e}")); Err(e) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(fp: &str) -> Phone { Phone { alias:fp.into(), fingerprint:fp.into(), addr:"192.168.1.2:53317".parse().unwrap(), protocol:"https".into() } }
    #[test] fn never_send_to_another_phone_when_saved_phone_is_offline() {
        assert!(target(&[p("other")], "mine").is_none());
        assert_eq!(target(&[p("mine"),p("other")], "mine").unwrap().fingerprint, "mine");
        assert!(target(&[p("a"),p("b")], "").is_none());
        assert_eq!(target(&[p("a")], "").unwrap().fingerprint, "a");
    }
    #[test] fn discovery_validates_endpoint() {
        let source = "192.168.1.2:53317".parse().unwrap();
        let mut v = json!({"deviceType":"mobile","fingerprint":"phone","protocol":"https"});
        assert_eq!(phone(&v, source).unwrap().addr.port(), 53317);
        v["port"] = json!(65536); assert!(phone(&v, source).is_none());
        v["port"] = json!(53317); v["protocol"] = json!("file"); assert!(phone(&v, source).is_none());
    }
    #[test] fn tokens_cannot_change_query_parameters() {
        assert_eq!(encode("a&x=1 /?"), "a%26x%3D1%20%2F%3F");
        assert!(check(403).unwrap_err().contains("declined"));
    }
    #[test] fn http_discovery_waits_for_complete_body() {
        let body = json!({"deviceType":"mobile","fingerprint":"phone"}).to_string();
        let headers = format!("POST /api/localsend/v2/register HTTP/1.1\r\nContent-Length: {}\r\n\r\n", body.len());
        assert!(callback_body(headers.as_bytes()).is_none());
        let request = format!("{headers}{body}");
        assert_eq!(callback_body(request.as_bytes()).unwrap()["fingerprint"], "phone");
        assert!(callback_body(request.replace("/register", "/other").as_bytes()).is_none());
    }
    #[test] fn uploads_file_bytes_with_the_receivers_session_and_token() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut receiver = p("mock"); receiver.addr = listener.local_addr().unwrap(); receiver.protocol = "http".into();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let expected = std::fs::read(&path).unwrap();
        let server = std::thread::spawn(move || {
            for upload in [false, true] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut data = Vec::new();
                let (header, body) = loop {
                    let mut buf = [0u8;4096]; let n = stream.read(&mut buf).unwrap();
                    assert_ne!(n, 0); data.extend_from_slice(&buf[..n]);
                    if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&data[..at]).into_owned();
                        let length: usize = header.lines().find_map(|l| {
                            let (k,v) = l.split_once(':')?;
                            if k.eq_ignore_ascii_case("content-length") { v.trim().parse().ok() } else { None }
                        }).unwrap();
                        if data.len() >= at + 4 + length { break (header, data[at+4..at+4+length].to_vec()); }
                    }
                };
                let response = if upload {
                    assert!(header.starts_with("POST /api/localsend/v2/upload?sessionId=s%26x&fileId=0&token=t%2Fq HTTP/"));
                    assert_eq!(body, expected);
                    String::new()
                } else {
                    assert!(header.starts_with("POST /api/localsend/v2/prepare-upload HTTP/"));
                    let meta: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(meta["files"]["0"]["size"], expected.len());
                    json!({"sessionId":"s&x","files":{"0":"t/q"}}).to_string()
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
            }
        });
        assert_eq!(send(&[path], &receiver, |_| {}).unwrap(), 1);
        server.join().unwrap();
    }
    #[test] fn metadata_preserves_names_and_sizes_and_rejects_directories() {
        let paths = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")];
        let v = metadata(&paths).unwrap();
        assert_eq!(v["files"]["0"]["fileName"], "Cargo.toml");
        assert!(v["files"]["0"]["size"].as_u64().unwrap() > 0);
        assert!(metadata(&[PathBuf::from(env!("CARGO_MANIFEST_DIR"))]).is_err());
    }
}
