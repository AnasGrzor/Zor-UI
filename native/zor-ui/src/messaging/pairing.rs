//! Pairing: a chat account is allowed once it sends the one-time code the PC shows. A code is six digits, expires after
//! `CODE_SECS`, and stops working after `MAX_TRIES` wrong tries. Paired accounts are kept per provider in `remote-chats.json`
//! under the data folder, as ids in text form.

use crate::bridge::common::{load_json, save_json};
use serde_json::{json, Value};
use std::{
    collections::{hash_map::RandomState, BTreeMap},
    hash::{BuildHasher, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

pub const CODE_SECS: u64 = 300;
pub const MAX_TRIES: u32 = 5;

struct Pending {
    code: String,
    made: Instant,
    tries: u32,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

#[derive(Debug, PartialEq, Eq)]
pub enum Redeem {
    /// The code was right. The sender is paired, and the code is used up.
    Paired,
    /// A six-digit try that is not the code. One of the tries is used.
    Wrong,
    /// The code ran out of time. It is cleared.
    Expired,
    /// No tries are left. Only a new code can pair now.
    Locked,
    /// No code is pending, or the message is not a code try. Nothing happens.
    NoCode,
}

/// A new six-digit code, replacing any pending one. The digits come from `RandomState`, which the OS seeds.
pub fn new_code(now: Instant) -> String {
    let code = format!("{:06}", RandomState::new().build_hasher().finish() % 1_000_000);
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { code: code.clone(), made: now, tries: 0 });
    code
}

/// Checks one chat message against the pending code.
pub fn redeem(text: &str, now: Instant) -> Redeem {
    redeem_in(&mut PENDING.lock().unwrap_or_else(|e| e.into_inner()), text, now)
}

/// The pending code and the seconds it has left, when one is still usable (not expired, not locked).
pub fn active_code(now: Instant) -> Option<(String, u64)> {
    active_in(&PENDING.lock().unwrap_or_else(|e| e.into_inner()), now)
}

fn active_in(pending: &Option<Pending>, now: Instant) -> Option<(String, u64)> {
    let p = pending.as_ref()?;
    let age = now.saturating_duration_since(p.made).as_secs();
    (age <= CODE_SECS && p.tries < MAX_TRIES).then(|| (p.code.clone(), CODE_SECS - age))
}

fn looks_like_code(text: &str) -> bool { text.len() == 6 && text.bytes().all(|b| b.is_ascii_digit()) }

fn redeem_in(pending: &mut Option<Pending>, text: &str, now: Instant) -> Redeem {
    let text = text.trim();
    let Some(p) = pending.as_mut() else { return Redeem::NoCode };
    if !looks_like_code(text) { return Redeem::NoCode; }
    if now.duration_since(p.made) > Duration::from_secs(CODE_SECS) { *pending = None; return Redeem::Expired; }
    if p.tries >= MAX_TRIES { return Redeem::Locked; }
    if text == p.code { *pending = None; return Redeem::Paired; }
    p.tries += 1;
    Redeem::Wrong
}

/// The file that keeps the paired ids.
pub fn store_path() -> PathBuf { crate::conf::data_dir().join("remote-chats.json") }

/// The paired ids of `provider` in the file at `path`.
pub fn paired_in(path: &Path, provider: &str) -> Vec<u64> {
    load_json(path, json!({}))
        .get(provider)
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(|v| v.as_str()?.parse().ok()).collect())
        .unwrap_or_default()
}

/// The paired ids per provider, as last read from or written to the file. Drawing the Settings page and checking each message
/// use this, so the file is read once. Only `pair` and `unpair` write the file, and they update this too.
static PAIRED: Mutex<BTreeMap<String, Vec<u64>>> = Mutex::new(BTreeMap::new());

/// The paired ids of `provider`. Read from the file the first time, then from the cache.
pub fn paired(provider: &str) -> Vec<u64> {
    cached(&mut PAIRED.lock().unwrap_or_else(|e| e.into_inner()), &store_path(), provider)
}

/// Pairs `id` with `provider`. Returns whether the file was saved; the cache changes only then.
pub fn pair(provider: &str, id: u64) -> bool {
    pair_cached(&mut PAIRED.lock().unwrap_or_else(|e| e.into_inner()), &store_path(), provider, id)
}

/// Removes `id` from `provider`. Its messages are refused from the next one on. Returns whether the file was saved.
pub fn unpair(provider: &str, id: u64) -> bool {
    unpair_cached(&mut PAIRED.lock().unwrap_or_else(|e| e.into_inner()), &store_path(), provider, id)
}

fn cached(map: &mut BTreeMap<String, Vec<u64>>, path: &Path, provider: &str) -> Vec<u64> {
    map.entry(provider.to_string()).or_insert_with(|| paired_in(path, provider)).clone()
}

fn pair_cached(map: &mut BTreeMap<String, Vec<u64>>, path: &Path, provider: &str, id: u64) -> bool {
    let mut ids = cached(map, path, provider);
    if ids.contains(&id) { return true; }
    ids.push(id);
    let saved = save_ids(path, provider, &ids);
    if saved { map.insert(provider.to_string(), ids); }
    saved
}

fn unpair_cached(map: &mut BTreeMap<String, Vec<u64>>, path: &Path, provider: &str, id: u64) -> bool {
    let ids: Vec<u64> = cached(map, path, provider).into_iter().filter(|&i| i != id).collect();
    let saved = save_ids(path, provider, &ids);
    if saved { map.insert(provider.to_string(), ids); }
    saved
}

fn save_ids(path: &Path, provider: &str, ids: &[u64]) -> bool {
    let mut data = load_json(path, json!({}));
    if !data.is_object() { data = json!({}); }
    data[provider] = json!(ids.iter().map(|i| i.to_string()).collect::<Vec<_>>());
    save_json(path, &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(code: &str, made: Instant) -> Option<Pending> { Some(Pending { code: code.into(), made, tries: 0 }) }

    #[test]
    fn the_right_code_pairs_once_and_then_is_used_up() {
        let now = Instant::now();
        let mut p = pending("123456", now);
        assert_eq!(redeem_in(&mut p, "123456", now), Redeem::Paired);
        assert_eq!(redeem_in(&mut p, "123456", now), Redeem::NoCode, "a used code does nothing");
    }

    #[test]
    fn ordinary_chat_is_not_a_try_and_five_wrong_codes_lock_the_code() {
        let now = Instant::now();
        let mut p = pending("123456", now);
        assert_eq!(redeem_in(&mut p, "what time is it", now), Redeem::NoCode);
        assert_eq!(redeem_in(&mut p, "12345", now), Redeem::NoCode, "five digits is not a try");
        for _ in 0..MAX_TRIES { assert_eq!(redeem_in(&mut p, "000000", now), Redeem::Wrong); }
        assert_eq!(redeem_in(&mut p, "123456", now), Redeem::Locked, "even the right code is refused once locked");
    }

    #[test]
    fn a_code_expires_after_its_time() {
        let made = Instant::now();
        let mut p = pending("123456", made);
        let late = made + Duration::from_secs(CODE_SECS + 1);
        assert_eq!(redeem_in(&mut p, "123456", late), Redeem::Expired);
        assert_eq!(redeem_in(&mut p, "123456", late), Redeem::NoCode, "an expired code is cleared");
    }

    #[test]
    fn a_new_code_is_six_digits() {
        let code = new_code(Instant::now());
        assert!(looks_like_code(&code), "{code}");
    }

    #[test]
    fn the_shown_code_counts_down_and_disappears_when_it_expires_or_locks() {
        let made = Instant::now();
        let mut p = pending("123456", made);
        assert_eq!(active_in(&p, made + Duration::from_secs(60)), Some(("123456".to_string(), CODE_SECS - 60)));
        assert_eq!(active_in(&p, made + Duration::from_secs(CODE_SECS + 1)), None, "an expired code is not shown");
        for _ in 0..MAX_TRIES { redeem_in(&mut p, "000000", made); }
        assert_eq!(active_in(&p, made), None, "a locked code is not shown");
        assert_eq!(active_in(&None, made), None);
    }

    #[test]
    fn an_unpaired_account_is_refused_again_and_the_others_stay() {
        let dir = std::env::temp_dir().join(format!("zorui-unpair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("remote-chats.json");
        let mut map = BTreeMap::new();
        pair_cached(&mut map, &path, "discord", 42);
        pair_cached(&mut map, &path, "discord", 7);
        assert!(unpair_cached(&mut map, &path, "discord", 42));
        assert_eq!(cached(&mut map, &path, "discord"), vec![7], "only the revoked account goes");
        assert_eq!(paired_in(&path, "discord"), vec![7], "the file agrees with the cache");
        assert!(unpair_cached(&mut map, &path, "discord", 99), "revoking an unknown id changes nothing and still saves");
        assert_eq!(cached(&mut map, &path, "discord"), vec![7]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paired_ids_are_kept_per_provider_and_not_repeated() {
        let dir = std::env::temp_dir().join(format!("zorui-pairing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("remote-chats.json");
        let mut map = BTreeMap::new();
        assert!(pair_cached(&mut map, &path, "discord", 42));
        assert!(pair_cached(&mut map, &path, "discord", 42));
        assert!(pair_cached(&mut map, &path, "discord", 7));
        assert_eq!(paired_in(&path, "discord"), vec![42, 7]);
        assert!(cached(&mut map, &path, "whatsapp").is_empty(), "another provider has no pairs");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
