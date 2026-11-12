// without the `discord` feature nothing calls the pairing code, so it is unused in that build
#![cfg_attr(not(feature = "discord"), allow(dead_code))]

//! Chat transports that reach the buddy through `agent::ask_remote`. Discord is the first (`discord.rs`, built with the `discord`
//! feature); WhatsApp would sit beside it. A transport only moves text in and out. The buddy's intelligence, tools and approvals
//! stay in `agent`, and a remote chat's approvals are refused there until a remote channel can answer them.

pub mod pairing;
#[cfg(feature = "discord")]
pub mod discord;

use std::collections::{HashSet, VecDeque};

/// The provider name of Discord, as the paired-account file and the conversation ids spell it.
pub const DISCORD: &str = "discord";

/// The most characters one Discord message holds.
pub const DISCORD_LIMIT: usize = 2000;

/// The conversation id of a chat, in the form the plan sets: `discord:<user id>:<channel id>`.
pub fn conversation_id(provider: &str, user: u64, chat: u64) -> String { format!("{provider}:{user}:{chat}") }

/// Cuts `text` into pieces of at most `max` characters, breaking at a line or a space in the second half of a piece when there is
/// one. Always returns at least one piece.
pub fn split(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut rest: Vec<char> = text.chars().collect();
    let mut out = vec![];
    while rest.len() > max {
        let window = &rest[..max];
        let at_break = |c: char| window.iter().rposition(|&x| x == c).filter(|&i| i > max / 2);
        let (end, next) = match at_break('\n').or_else(|| at_break(' ')) {
            Some(i) => (i, i + 1),
            None => (max, max),
        };
        out.push(rest[..end].iter().collect::<String>().trim_end().to_string());
        rest = rest[next..].to_vec();
    }
    out.push(rest.iter().collect());
    out
}

/// The message ids already handled. Discord can deliver a message again after a reconnect, so each id is handled once. Only the
/// newest `cap` ids are kept.
pub struct Seen {
    order: VecDeque<u64>,
    ids: HashSet<u64>,
    cap: usize,
}

impl Seen {
    pub fn new(cap: usize) -> Seen { Seen { order: VecDeque::new(), ids: HashSet::new(), cap: cap.max(1) } }

    /// True the first time `id` is seen, false after.
    pub fn first_time(&mut self, id: u64) -> bool {
        if !self.ids.insert(id) { return false; }
        self.order.push_back(id);
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() { self.ids.remove(&old); }
        }
        true
    }
}

/// Brings the chat transports in line with config.json and the saved secrets: started when enabled, stopped when switched off. Called
/// at app start, on each config reload, and after a token changes. Returns at once; the work runs on a background thread.
pub fn sync() {
    #[cfg(feature = "discord")]
    discord::request_sync();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_answer_is_cut_into_pieces_no_longer_than_the_limit() {
        let text = "word ".repeat(1000);
        let pieces = split(&text, DISCORD_LIMIT);
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(|p| p.chars().count() <= DISCORD_LIMIT));
        assert_eq!(pieces.join(" ").split_whitespace().count(), 1000, "no word is lost");
    }

    #[test]
    fn a_short_answer_is_one_piece_and_breaks_prefer_lines() {
        assert_eq!(split("hi", DISCORD_LIMIT), vec!["hi".to_string()]);
        assert_eq!(split("", DISCORD_LIMIT), vec![String::new()], "an empty answer still sends one piece");
        assert_eq!(split("aaaaaaaaaa\nbbbbbbbbbb", 15), vec!["aaaaaaaaaa".to_string(), "bbbbbbbbbb".to_string()]);
    }

    #[test]
    fn conversation_ids_name_the_provider_the_user_and_the_chat() {
        assert_eq!(conversation_id("discord", 42, 7), "discord:42:7");
    }

    #[test]
    fn a_message_id_is_handled_once_and_the_oldest_are_forgotten() {
        let mut seen = Seen::new(2);
        assert!(seen.first_time(1));
        assert!(!seen.first_time(1), "a replayed message is not handled again");
        assert!(seen.first_time(2));
        assert!(seen.first_time(3));
        assert!(seen.first_time(1), "id 1 fell out of the window, so it counts as new again");
    }
}
