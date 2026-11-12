//! Pairing: a chat account is allowed once it sends the one-time code the PC shows. A code is six digits, expires after
//! `CODE_SECS`, and stops working after `MAX_TRIES` wrong tries. Paired accounts are kept per provider in `remote-chats.json`
//! under the data folder, as ids in text form.

use crate::bridge::common::{load_json, save_json};
use serde_json::{json, Value};
use std::{
    collections::hash_map::RandomState,
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

/// Adds `id` to the paired ids of `provider` in the file at `path`. Returns whether the file was saved.
pub fn pair_in(path: &Path, provider: &str, id: u64) -> bool {
    let mut ids = paired_in(path, provider);
    if ids.contains(&id) { return true; }
    ids.push(id);
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
    fn paired_ids_are_kept_per_provider_and_not_repeated() {
        let dir = std::env::temp_dir().join(format!("zorui-pairing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("remote-chats.json");
        assert!(pair_in(&path, "discord", 42));
        assert!(pair_in(&path, "discord", 42));
        assert!(pair_in(&path, "discord", 7));
        assert_eq!(paired_in(&path, "discord"), vec![42, 7]);
        assert!(paired_in(&path, "whatsapp").is_empty(), "another provider has no pairs");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
