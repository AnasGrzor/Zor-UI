//! The Discord bot. A DM from a paired account becomes a buddy request, and the answer goes back to that DM. Built with the
//! `discord` feature (`cargo build --features discord`). Servers (guilds) are ignored: only direct messages are read.
//!
//! The bot runs on its own thread with its own current-thread Tokio runtime, so the app's runtime keeps its features. The buddy
//! itself runs on the blocking pool (`spawn_blocking`), so a long request does not stop the gateway's heartbeat.

use super::{conversation_id, pairing, split, Seen, DISCORD_LIMIT};
use crate::conf::Config;
use serenity::{
    async_trait,
    client::{Client, Context, EventHandler},
    model::{channel::Message, gateway::{GatewayIntents, Ready}},
};
use std::{sync::Mutex, time::{Duration, Instant}};

pub const PROVIDER: &str = "discord";

/// The name the bot token is saved under in the DPAPI secret store (`bridge::web::secret_get`). `ZORUI_DISCORD_BOT_KEY` also works.
pub const TOKEN_SECRET: &str = "discord_bot";

/// What the Settings page shows: "off", "connecting", "connected", or why the bot stopped.
pub static STATUS: Mutex<String> = Mutex::new(String::new());

fn set_status(s: &str) {
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = s.to_string();
    crate::log::line(&format!("discord: {s}"));
}

struct Handler {
    seen: Mutex<Seen>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, _ready: Ready) {
        set_status("connected");
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // only direct messages from people: no bots (including this one), no servers
        if msg.author.bot || msg.guild_id.is_some() { return; }
        if !self.seen.lock().unwrap_or_else(|e| e.into_inner()).first_time(msg.id.get()) { return; }
        let user = msg.author.id.get();
        let text = msg.content.trim().to_string();

        if !pairing::paired_in(&pairing::store_path(), PROVIDER).contains(&user) {
            // an unpaired sender can only pair, with the code the PC shows; anything else gets no reply
            let reply = match pairing::redeem(&text, Instant::now()) {
                pairing::Redeem::Paired => {
                    if !pairing::pair_in(&pairing::store_path(), PROVIDER, user) { set_status("could not save the pairing"); }
                    let _ = std::fs::remove_file(code_file());
                    "Paired. Send me a message and I'll answer here."
                }
                pairing::Redeem::Wrong => "That code is wrong.",
                pairing::Redeem::Expired => "That code expired. Restart Zor UI for a new one.",
                pairing::Redeem::Locked => "Too many wrong codes. Restart Zor UI for a new one.",
                pairing::Redeem::NoCode => return,
            };
            let _ = msg.channel_id.say(&ctx.http, reply).await;
            return;
        }

        if text.is_empty() { return; }
        let conv = conversation_id(PROVIDER, user, msg.channel_id.get());
        crate::log::line(&format!("discord request: {} characters", text.chars().count()));
        let answer = tokio::task::spawn_blocking(move || crate::agent::ask_remote(&Config::load(), &text, &conv)).await;
        let answer = match answer {
            Ok(Ok(a)) => a,
            Ok(Err(e)) => e,
            Err(_) => "Something went wrong on my side.".to_string(),
        };
        for piece in split(&answer, DISCORD_LIMIT) {
            if let Err(e) = msg.channel_id.say(&ctx.http, piece).await {
                crate::log::line(&format!("discord: could not send an answer: {e}"));
                break;
            }
        }
    }
}

/// The file the pairing code is written to, for the PC owner to read.
fn code_file() -> std::path::PathBuf { crate::conf::data_dir().join("remote-pairing.txt") }

/// Starts the bot when Settings enables it and a token is saved. Returns at once; the bot runs on its own thread.
pub fn start() {
    let cfg = Config::load();
    if !cfg.bool("remote.discord.enabled") { set_status("off"); return; }
    let token = crate::bridge::web::secret_get(TOKEN_SECRET);
    if token.is_empty() { set_status("no bot token saved"); return; }
    if pairing::paired_in(&pairing::store_path(), PROVIDER).is_empty() {
        let code = pairing::new_code(Instant::now());
        let note = format!("Pairing code for Discord: {code}\r\nSend it to the bot in a direct message within {} minutes.\r\n", pairing::CODE_SECS / 60);
        let _ = std::fs::write(code_file(), note);
    }
    let spawned = std::thread::Builder::new().name("discord".into()).spawn(move || run(token));
    if spawned.is_err() { set_status("could not start the thread"); }
}

fn run(token: String) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => { set_status(&format!("could not start the runtime: {e}")); return; }
    };
    rt.block_on(async move {
        // a stopped gateway is connected again, after a wait that doubles up to five minutes
        let mut wait = 5u64;
        loop {
            set_status("connecting");
            let intents = GatewayIntents::DIRECT_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
            let handler = Handler { seen: Mutex::new(Seen::new(512)) };
            match Client::builder(&token, intents).event_handler(handler).await {
                Ok(mut client) => {
                    if let Err(e) = client.start().await { set_status(&format!("stopped: {e}")); }
                }
                Err(e) => set_status(&format!("could not connect: {e}")),
            }
            tokio::time::sleep(Duration::from_secs(wait)).await;
            wait = (wait * 2).min(300);
        }
    });
}
