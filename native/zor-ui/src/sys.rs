//! What Zebar's network, battery, keyboard, cpu, memory and date providers gave the bar.

use crate::util::date_fmt;
use std::time::Instant;
use windows::Win32::{
    Foundation::*,
    Globalization::LCIDToLocaleName,
    NetworkManagement::{IpHelper::*, WiFi::*},
    Networking::WinSock::AF_INET,
    System::{Power::*, SystemInformation::*},
    UI::{Input::KeyboardAndMouse::GetKeyboardLayout, WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId}},
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Net {
    /// "ethernet" | "wifi" | "proprietary_virtual" | "other" (Zebar's interface types).
    pub kind: &'static str,
    pub name: String,
    pub ssid: String,
    pub signal: u32,
    pub ip: String,
    /// Bytes per second since the previous sample, and totals since the interface came up.
    pub rx: f64,
    pub tx: f64,
    pub total_rx: u64,
    pub total_tx: u64,
}

#[derive(Default)]
pub struct NetMon { last: Option<(u32, u64, u64, Instant)>, wlan: Option<HANDLE> }

impl NetMon {
    /// The interface Windows routes the internet through; None when offline.
    pub fn sample(&mut self) -> Option<Net> {
        unsafe {
            let mut idx = 0u32;
            if GetBestInterface(0x0808_0808, &mut idx) != 0 { self.last = None; return None; } // route to 8.8.8.8
            let mut row = MIB_IF_ROW2 { InterfaceIndex: idx, ..Default::default() };
            if GetIfEntry2(&mut row).is_err() { return None; }
            let now = Instant::now();
            let (rx, tx) = match self.last {
                Some((i, inb, outb, t)) if i == idx => {
                    let dt = now.duration_since(t).as_secs_f64().max(0.001);
                    (row.InOctets.saturating_sub(inb) as f64 / dt, row.OutOctets.saturating_sub(outb) as f64 / dt)
                }
                _ => (0.0, 0.0),
            };
            self.last = Some((idx, row.InOctets, row.OutOctets, now));
            let kind = match row.Type { 6 => "ethernet", 71 => "wifi", 53 => "proprietary_virtual", _ => "other" };
            let wide = |w: &[u16]| String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())]);
            let (ssid, signal) = if kind == "wifi" { self.wifi().unwrap_or_default() } else { Default::default() };
            Some(Net { kind, name: wide(&row.Alias), ssid, signal, ip: ipv4(idx).unwrap_or_default(), rx, tx, total_rx: row.InOctets, total_tx: row.OutOctets })
        }
    }

    /// SSID and signal quality (0-100) of the connected Wi-Fi network.
    fn wifi(&mut self) -> Option<(String, u32)> {
        unsafe {
            if self.wlan.is_none() {
                let (mut ver, mut h) = (0u32, HANDLE::default());
                if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 { return None; }
                self.wlan = Some(h);
            }
            let h = self.wlan?;
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            if WlanEnumInterfaces(h, None, &mut list) != 0 || list.is_null() { return None; }
            let l = &*list;
            let infos = std::slice::from_raw_parts(l.InterfaceInfo.as_ptr(), l.dwNumberOfItems as usize);
            let mut found = None;
            for i in infos {
                let (mut size, mut data) = (0u32, std::ptr::null_mut());
                if WlanQueryInterface(h, &i.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None) != 0 || data.is_null() { continue; }
                let c = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                if c.isState == wlan_interface_state_connected {
                    let s = &c.wlanAssociationAttributes.dot11Ssid;
                    found = Some((String::from_utf8_lossy(&s.ucSSID[..(s.uSSIDLength as usize).min(32)]).into_owned(), c.wlanAssociationAttributes.wlanSignalQuality));
                }
                WlanFreeMemory(data);
                if found.is_some() { break; }
            }
            WlanFreeMemory(list as _);
            found
        }
    }
}

fn ipv4(idx: u32) -> Option<String> {
    unsafe {
        let mut t: *mut MIB_UNICASTIPADDRESS_TABLE = std::ptr::null_mut();
        if GetUnicastIpAddressTable(AF_INET, &mut t).is_err() || t.is_null() { return None; }
        let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), (*t).NumEntries as usize);
        let ip = rows.iter().find(|r| r.InterfaceIndex == idx).map(|r| {
            let b = r.Address.Ipv4.sin_addr.S_un.S_un_b;
            format!("{}.{}.{}.{}", b.s_b1, b.s_b2, b.s_b3, b.s_b4)
        });
        FreeMibTable(t as _);
        ip
    }
}

/// rate() in bar.js.
pub fn rate(b: f64) -> String { if b >= 1e6 { format!("{:.1} MB/s", b / 1e6) } else { format!("{} KB/s", (b / 1e3).round()) } }
pub fn gb(b: f64) -> String { format!("{:.1} GB", b / 1_073_741_824.0) }

/// networkIcon() in core.js.
pub fn net_icon(n: Option<&Net>) -> &'static str {
    match n.map(|n| n.kind) {
        Some("ethernet") => "lan",
        Some("wifi") => match n.map_or(0, |n| n.signal) { s if s >= 80 => "signal_wifi_4_bar", s if s >= 60 => "network_wifi_3_bar", s if s >= 40 => "network_wifi_2_bar", s if s >= 20 => "network_wifi_1_bar", _ => "signal_wifi_0_bar" },
        Some("proprietary_virtual") => "vpn_lock",
        Some(_) => "lan",
        None => "signal_wifi_off",
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Battery { pub percent: f32, pub charging: bool, pub full: bool, pub secs_left: Option<u32> }

pub fn battery() -> Option<Battery> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()?; }
    if s.BatteryFlag & 128 != 0 || s.BatteryFlag == 255 || s.BatteryLifePercent > 100 { return None; } // no battery / unknown
    let ac = s.ACLineStatus == 1;
    Some(Battery {
        percent: s.BatteryLifePercent as f32,
        charging: s.BatteryFlag & 8 != 0 || (ac && s.BatteryLifePercent < 100),
        full: ac && s.BatteryLifePercent >= 100,
        secs_left: (s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime),
    })
}

/// batteryIcon() in core.js.
pub fn battery_icon(b: &Battery) -> String {
    if b.charging { return "battery_charging_full".into(); }
    if b.percent > 95.0 { "battery_full".into() } else { format!("battery_{}_bar", ((b.percent / 16.6).round() as i32).clamp(0, 6)) }
}

/// Keyboard layout of the focused window ("en-US").
pub fn keyboard() -> String {
    unsafe {
        let tid = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let lang = (GetKeyboardLayout(tid).0 as usize & 0xFFFF) as u32;
        let mut buf = [0u16; 85];
        let n = LCIDToLocaleName(lang, Some(&mut buf), 0);
        String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
    }
}

/// CPU clock (MHz) and (physical, logical) core counts, for the resources popup.
pub fn cpu_info() -> (u32, u32, u32) {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Ppi { number: u32, max_mhz: u32, current_mhz: u32, limit: u32, max_idle: u32, idle: u32 }
    unsafe {
        let mut si = SYSTEM_INFO::default();
        GetSystemInfo(&mut si);
        let n = si.dwNumberOfProcessors.max(1);
        let mut info = vec![Ppi::default(); n as usize];
        let _ = CallNtPowerInformation(ProcessorInformation, None, 0, Some(info.as_mut_ptr() as _), (n as usize * size_of::<Ppi>()) as u32);
        let mut len = 0u32;
        let _ = GetLogicalProcessorInformation(None, &mut len);
        let mut v = vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); len as usize / size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>() + 1];
        let cores = if GetLogicalProcessorInformation(Some(v.as_mut_ptr()), &mut len).is_ok() {
            v.iter().take(len as usize / size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>()).filter(|i| i.Relationship == RelationProcessorCore).count() as u32
        } else { n };
        (info[0].current_mhz, cores, n)
    }
}

/// (used, total) RAM and page file, in bytes.
pub fn memory() -> ((f64, f64), (f64, f64)) {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { let _ = GlobalMemoryStatusEx(&mut m); }
    let ram = ((m.ullTotalPhys - m.ullAvailPhys) as f64, m.ullTotalPhys as f64);
    let swap_total = m.ullTotalPageFile.saturating_sub(m.ullTotalPhys) as f64;
    let commit = m.ullTotalPageFile.saturating_sub(m.ullAvailPageFile) as f64;
    (ram, ((commit - ram.0).clamp(0.0, swap_total), swap_total))
}

/// Luxon's toFormat() tokens, which Zebar's date provider used for config clockFormat.
pub fn luxon(fmt: &str, st: &SYSTEMTIME) -> String {
    let chars: Vec<char> = fmt.chars().collect();
    let (mut out, mut i) = (String::new(), 0);
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            let end = chars[i + 1..].iter().position(|&x| x == '\'').map_or(chars.len(), |p| i + 1 + p);
            if end == i + 1 { out.push('\''); } else { out.extend(&chars[i + 1..end]); }
            i = end + 1;
            continue;
        }
        if !c.is_ascii_alphabetic() { out.push(c); i += 1; continue; }
        let n = chars[i..].iter().take_while(|&&x| x == c).count();
        let h12 = if st.wHour % 12 == 0 { 12 } else { st.wHour % 12 };
        out += &match (c, n) {
            ('E' | 'c', 3) => date_fmt(st, "ddd"),
            ('E' | 'c', _) if n >= 4 => date_fmt(st, "dddd"),
            ('d', 1) => st.wDay.to_string(), ('d', _) => format!("{:02}", st.wDay),
            ('M' | 'L', 1) => st.wMonth.to_string(), ('M' | 'L', 2) => format!("{:02}", st.wMonth),
            ('M' | 'L', 3) => date_fmt(st, "MMM"), ('M' | 'L', _) => date_fmt(st, "MMMM"),
            ('y', 2) => format!("{:02}", st.wYear % 100), ('y', _) => st.wYear.to_string(),
            ('H', 1) => st.wHour.to_string(), ('H', _) => format!("{:02}", st.wHour),
            ('h', 1) => h12.to_string(), ('h', _) => format!("{h12:02}"),
            ('m', 1) => st.wMinute.to_string(), ('m', _) => format!("{:02}", st.wMinute),
            ('s', 1) => st.wSecond.to_string(), ('s', _) => format!("{:02}", st.wSecond),
            ('a', _) => (if st.wHour < 12 { "AM" } else { "PM" }).to_string(),
            _ => chars[i..i + n].iter().collect(),
        };
        i += n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn luxon_tokens() {
        let st = SYSTEMTIME { wYear: 2026, wMonth: 11, wDay: 4, wDayOfWeek: 3, wHour: 14, wMinute: 5, ..Default::default() };
        assert_eq!(luxon("dd/MM '•' HH:mm", &st), "04/11 • 14:05");
        assert_eq!(luxon("h:mm a yy", &st), "2:05 PM 26");
        assert_eq!(rate(1_500_000.0), "1.5 MB/s");
        assert_eq!(battery_icon(&Battery { percent: 50.0, ..Default::default() }), "battery_3_bar");
    }
}
