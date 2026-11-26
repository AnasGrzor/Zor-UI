//! Wi-Fi networks in range and paired Bluetooth devices, for the menus on the right sidebar's Wi-Fi and
//! Bluetooth tiles: list them, and connect or disconnect one. Reached through toggles ("-Action wifi-list").

use serde_json::{json, Value};
use windows::{
    core::{HSTRING, PCWSTR},
    Devices::Enumeration::{DeviceInformation, DeviceInformationKind},
    Foundation::{Collections::IIterable, IReference},
    Win32::{
        Devices::DeviceAndDriverInstallation::{CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW, CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS},
        Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE},
        Media::KernelStreaming::{IOCTL_KS_PROPERTY, KSCATEGORY_AUDIO, KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_ONESHOT_DISCONNECT, KSPROPERTY_ONESHOT_RECONNECT, KSPROPERTY_TYPE_GET, KSPROPSETID_BtAudio},
        NetworkManagement::WiFi::*,
        Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
        System::IO::DeviceIoControl,
    },
};

/// Most networks the menu lists (the strongest ones).
const MAX_NETS: usize = 12;

fn log(m: &str) { crate::log::line(&format!("wireless: {m}")); }

// ------------------------------------------------------------------ Wi-Fi

#[derive(Clone, Debug, PartialEq)]
pub struct Net { pub ssid: String, pub profile: String, pub signal: u32, pub secure: bool, pub connected: bool }

/// One row per name (Windows lists a saved network twice, with and without its profile), the connected one
/// first, then by signal. Hidden networks have no name to show and are left out.
fn rank(nets: Vec<Net>) -> Vec<Net> {
    let mut out: Vec<Net> = vec![];
    for n in nets.into_iter().filter(|n| !n.ssid.is_empty()) {
        match out.iter_mut().find(|o| o.ssid == n.ssid) {
            Some(o) => {
                o.connected |= n.connected;
                o.signal = o.signal.max(n.signal);
                if o.profile.is_empty() { o.profile = n.profile; }
            }
            None => out.push(n),
        }
    }
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)));
    out.truncate(MAX_NETS);
    out
}

struct Wlan(HANDLE);
impl Drop for Wlan { fn drop(&mut self) { unsafe { WlanCloseHandle(self.0, None); } } }

/// A WLAN client handle and the GUIDs of its Wi-Fi adapters.
fn wlan() -> Result<(Wlan, Vec<windows::core::GUID>), String> {
    unsafe {
        let (mut ver, mut h) = (0u32, HANDLE::default());
        let e = WlanOpenHandle(2, None, &mut ver, &mut h);
        if e != 0 { return Err(format!("WlanOpenHandle failed ({e})")); }
        let w = Wlan(h);
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let e = WlanEnumInterfaces(h, None, &mut list);
        if e != 0 || list.is_null() { return Err(format!("WlanEnumInterfaces failed ({e})")); }
        let l = &*list;
        let ids = std::slice::from_raw_parts(l.InterfaceInfo.as_ptr(), l.dwNumberOfItems as usize).iter().map(|i| i.InterfaceGuid).collect();
        WlanFreeMemory(list as _);
        Ok((w, ids))
    }
}

/// The networks adapter `g` sees now (Windows' last scan).
fn nets_on(w: &Wlan, g: &windows::core::GUID) -> Vec<Net> {
    let wide = |s: &[u16]| String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]);
    let mut out = vec![];
    unsafe {
        let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
        let e = WlanGetAvailableNetworkList(w.0, g, 0, None, &mut list);
        if e != 0 || list.is_null() { log(&format!("WlanGetAvailableNetworkList failed ({e})")); return out; }
        let l = &*list;
        for n in std::slice::from_raw_parts(l.Network.as_ptr(), l.dwNumberOfItems as usize) {
            let s = &n.dot11Ssid;
            out.push(Net {
                ssid: String::from_utf8_lossy(&s.ucSSID[..(s.uSSIDLength as usize).min(32)]).into_owned(),
                profile: wide(&n.strProfileName), signal: n.wlanSignalQuality, secure: n.bSecurityEnabled.as_bool(),
                connected: n.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
            });
        }
        WlanFreeMemory(list as _);
    }
    out
}

pub fn wifi_list() -> Value {
    let (w, ifs) = match wlan() { Ok(x) => x, Err(e) => { log(&format!("wifi-list: {e}")); return json!({ "error": e }) } };
    let mut all = vec![];
    for g in &ifs {
        all.extend(nets_on(&w, g));
        // freshens the list for the next open; this one shows what Windows already has, so the menu doesn't wait
        unsafe { WlanScan(w.0, g, None, None, None); }
    }
    let nets = rank(all);
    log(&format!("wifi-list: {} adapter(s), {} network(s), connected to {}", ifs.len(), nets.len(),
        nets.iter().find(|n| n.connected).map_or("none".into(), |n| format!("\"{}\"", n.ssid))));
    json!({ "networks": nets.iter().map(|n| json!({ "ssid": n.ssid, "signal": n.signal, "secure": n.secure, "saved": !n.profile.is_empty(), "connected": n.connected })).collect::<Vec<_>>() })
}

/// Connects to `ssid` (its saved profile), or disconnects when it is the connected one. A network with no saved
/// profile needs a password Windows asks for: answers `{"flyout": true}` and the caller opens Windows' list.
pub fn wifi_connect(ssid: &str) -> Value {
    let (w, ifs) = match wlan() { Ok(x) => x, Err(e) => { log(&format!("wifi-connect: {e}")); return json!({ "error": e }) } };
    let Some((g, n)) = ifs.iter().find_map(|g| rank(nets_on(&w, g)).into_iter().find(|n| n.ssid == ssid).map(|n| (*g, n))) else {
        log(&format!("wifi-connect \"{ssid}\": not in range any more"));
        return json!({ "error": format!("{ssid} is not in range") });
    };
    if n.connected {
        let e = unsafe { WlanDisconnect(w.0, &g, None) };
        log(&format!("wifi-connect \"{ssid}\": was connected, disconnect -> {}", if e == 0 { "ok".into() } else { format!("failed ({e})") }));
        return json!({ "ok": e == 0, "connected": false });
    }
    if n.profile.is_empty() {
        log(&format!("wifi-connect \"{ssid}\": no saved profile, opening Windows' network list for the password"));
        return json!({ "ok": false, "flyout": true });
    }
    let profile = HSTRING::from(n.profile.as_str());
    let p = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile, strProfile: PCWSTR(profile.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(), pDesiredBssidList: std::ptr::null_mut(), dot11BssType: dot11_BSS_type_infrastructure, dwFlags: 0,
    };
    let e = unsafe { WlanConnect(w.0, &g, &p, None) };
    if e != 0 { log(&format!("wifi-connect \"{ssid}\": WlanConnect failed ({e})")); return json!({ "error": format!("WlanConnect failed ({e})") }); }
    // WlanConnect only starts it: wait until the adapter reports the network connected
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if nets_on(&w, &g).iter().any(|x| x.ssid == ssid && x.connected) { log(&format!("wifi-connect \"{ssid}\": connected")); return json!({ "ok": true, "connected": true }); }
    }
    log(&format!("wifi-connect \"{ssid}\": asked, not connected after 10 s"));
    json!({ "ok": false, "error": format!("{ssid} did not connect") })
}

// ------------------------------------------------------------------ Bluetooth

#[derive(Clone, Debug, PartialEq)]
pub struct Bt { pub name: String, pub addr: String, pub connected: bool }

/// Paired classic and Bluetooth LE devices (Windows' association endpoints).
const BT_AQS: &str = "(System.Devices.Aep.ProtocolId:=\"{e0cbf06c-cd8b-4647-bb8a-263b43f0f974}\" OR System.Devices.Aep.ProtocolId:=\"{bb7bb05e-5972-42b5-94fc-76eaa7084d49}\") AND System.Devices.Aep.IsPaired:=System.StructuredQueryType.Boolean#True";
const CONNECTED: &str = "System.Devices.Aep.IsConnected";

/// The device address from an endpoint id ("Bluetooth#Bluetooth<local>-<device>"), as 12 lowercase hex digits.
fn addr_of(id: &str) -> String { id.rsplit('-').next().unwrap_or("").chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_lowercase() }

/// One row per address (a dual-mode device pairs as classic and LE), connected ones first; nameless ones are left out.
fn merge(devs: Vec<Bt>) -> Vec<Bt> {
    let mut out: Vec<Bt> = vec![];
    for d in devs.into_iter().filter(|d| !d.name.trim().is_empty() && d.addr.len() == 12) {
        match out.iter_mut().find(|o| o.addr == d.addr) { Some(o) => o.connected |= d.connected, None => out.push(d) }
    }
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

fn bt_devices() -> windows::core::Result<Vec<Bt>> {
    let props: IIterable<HSTRING> = vec![HSTRING::from(CONNECTED)].try_into()?;
    let all = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&HSTRING::from(BT_AQS), &props, DeviceInformationKind::AssociationEndpoint)?.get()?;
    let mut out = vec![];
    for d in all {
        let connected = d.Properties().and_then(|p| p.Lookup(&HSTRING::from(CONNECTED))).and_then(|v| windows::core::Interface::cast::<IReference<bool>>(&v)).and_then(|r| r.Value()).unwrap_or(false);
        out.push(Bt { name: d.Name()?.to_string(), addr: addr_of(&d.Id()?.to_string()), connected });
    }
    Ok(merge(out))
}

pub fn bt_list() -> Value {
    match bt_devices() {
        Ok(d) => {
            log(&format!("bt-list: {} paired, connected: {}", d.len(), d.iter().filter(|d| d.connected).map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")));
            json!({ "devices": d.iter().map(|d| json!({ "name": d.name, "addr": d.addr, "connected": d.connected })).collect::<Vec<_>>() })
        }
        Err(e) => { log(&format!("bt-list failed: {e}")); json!({ "error": e.message().to_string() }) }
    }
}

/// Every audio kernel-streaming interface Windows has now (device interface paths).
fn ks_audio_paths() -> Vec<String> {
    unsafe {
        let mut len = 0u32;
        if CM_Get_Device_Interface_List_SizeW(&mut len, &KSCATEGORY_AUDIO, PCWSTR::null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT) != CR_SUCCESS || len == 0 { return vec![]; }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_Interface_ListW(&KSCATEGORY_AUDIO, PCWSTR::null(), &mut buf, CM_GET_DEVICE_INTERFACE_LIST_PRESENT) != CR_SUCCESS { return vec![]; }
        buf.split(|&c| c == 0).filter(|p| !p.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// The A2DP driver interface of the device at `addr` (12 lowercase hex digits): "\\?\BTHENUM#{0000110b-...}_VID...&0&1A1C4D9606CC_C00000000#...".
/// It stays present while the device is disconnected, unlike the device's audio endpoints.
fn bt_filter<'a>(paths: &'a [String], addr: &str) -> Option<&'a str> {
    paths.iter().map(String::as_str).find(|p| { let l = p.to_lowercase(); l.contains("bthenum#{0000110b") && l.contains(addr) })
}

/// Sends the Bluetooth audio driver at `path` its one-shot reconnect or disconnect.
fn ks_oneshot(path: &str, connect: bool) -> Result<(), String> {
    let id = if connect { KSPROPERTY_ONESHOT_RECONNECT } else { KSPROPERTY_ONESHOT_DISCONNECT };
    let prop = KSIDENTIFIER { Anonymous: KSIDENTIFIER_0 { Anonymous: KSIDENTIFIER_0_0 { Set: KSPROPSETID_BtAudio, Id: id.0 as u32, Flags: KSPROPERTY_TYPE_GET } } };
    unsafe {
        let h = CreateFileW(&HSTRING::from(path), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None)
            .map_err(|e| format!("open driver: {}", e.message()))?;
        let mut ret = 0u32;
        let r = DeviceIoControl(h, IOCTL_KS_PROPERTY, Some(&prop as *const _ as _), size_of::<KSIDENTIFIER>() as u32, None, 0, Some(&mut ret), None);
        let _ = CloseHandle(h);
        r.map_err(|e| e.message().to_string())
    }
}

/// Connects the paired device at `addr`, or disconnects it when it is connected. Windows has no public call to
/// connect a Bluetooth device in general; audio devices take the one-shot reconnect their driver offers (what the
/// Settings app's Connect does). Anything else answers `{"settings": true}` and the caller opens Bluetooth settings.
pub fn bt_connect(addr: &str) -> Value {
    let addr = addr.to_lowercase();
    let dev = bt_devices().ok().and_then(|d| d.into_iter().find(|d| d.addr == addr));
    let Some(dev) = dev else { log(&format!("bt-connect {addr}: not paired")); return json!({ "error": "That device is not paired" }) };
    let paths = ks_audio_paths();
    let Some(path) = bt_filter(&paths, &addr) else {
        log(&format!("bt-connect \"{}\": no A2DP audio driver among {} audio interfaces (not a headset or speaker), opening Bluetooth settings", dev.name, paths.len()));
        return json!({ "ok": false, "settings": true });
    };
    let want = !dev.connected;
    log(&format!("bt-connect \"{}\": asking its audio driver to {}", dev.name, if want { "reconnect" } else { "disconnect" }));
    if let Err(e) = ks_oneshot(path, want) { log(&format!("bt-connect \"{}\": {} failed: {e}", dev.name, if want { "reconnect" } else { "disconnect" })); return json!({ "error": e }); }
    // the property only starts it: wait for Windows to report the new state
    for _ in 0..16 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if bt_devices().ok().and_then(|d| d.into_iter().find(|d| d.addr == addr)).is_some_and(|d| d.connected == want) {
            log(&format!("bt-connect \"{}\": {}", dev.name, if want { "connected" } else { "disconnected" }));
            return json!({ "ok": true, "connected": want });
        }
    }
    log(&format!("bt-connect \"{}\": asked to {}, state unchanged after 8 s (is it on and in range?)", dev.name, if want { "connect" } else { "disconnect" }));
    json!({ "ok": false, "error": format!("{} did not {}", dev.name, if want { "connect" } else { "disconnect" }) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(ssid: &str, profile: &str, signal: u32, connected: bool) -> Net { Net { ssid: ssid.into(), profile: profile.into(), signal, secure: true, connected } }

    #[test]
    fn networks_merge_by_name_connected_first() {
        let r = rank(vec![n("Cafe", "", 90, false), n("Home", "", 40, false), n("Home", "Home", 40, true), n("", "", 99, false), n("Cafe", "", 95, false)]);
        assert_eq!(r, vec![n("Home", "Home", 40, true), n("Cafe", "", 95, false)]);
        assert_eq!(rank((0..20).map(|i| n(&i.to_string(), "", i, false)).collect()).len(), MAX_NETS);
    }

    #[test]
    fn bluetooth_address_and_merge() {
        assert_eq!(addr_of("Bluetooth#Bluetooth00:1a:7d:da:71:13-9C:19:C2:3A:8B:1E"), "9c19c23a8b1e");
        let b = |name: &str, addr: &str, connected| Bt { name: name.into(), addr: addr.into(), connected };
        let m = merge(vec![b("Mouse", "aaaaaaaaaaaa", false), b("Buds", "bbbbbbbbbbbb", false), b("Buds", "bbbbbbbbbbbb", true), b("", "cccccccccccc", true)]);
        assert_eq!(m, vec![b("Buds", "bbbbbbbbbbbb", true), b("Mouse", "aaaaaaaaaaaa", false)]);
        let paths = [r"\\?\HDAUDIO#FUNC_01#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\eLineOutTopo", r"\\?\BTHHFENUM#BthHFPAudio#9&2a362ad3&7&97#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\X",
            r"\\?\BTHENUM#{0000110b-0000-1000-8000-00805f9b34fb}_VID&000105d6_PID&000a#8&1eba52ed&0&1A1C4D9606CC_C00000000#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\SRC"].map(String::from);
        assert_eq!(bt_filter(&paths, "1a1c4d9606cc"), Some(paths[2].as_str()));
        assert_eq!(bt_filter(&paths, "41420e203a9e"), None);
    }
}
