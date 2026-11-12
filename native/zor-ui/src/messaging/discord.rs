//! The Discord bot. A DM from a paired account becomes a buddy request, and the answer goes back to that DM. Built with the
//! `discord` feature (`cargo build --features discord`). Servers (guilds) are ignored: only direct messages are read.
//!
//! The bot runs on its own thread with its own current-thread Tokio runtime, so the app's runtime keeps its features. The buddy
//! itself runs on the blocking pool (`spawn_blocking`), so a long request does not stop the gateway's heartbeat. `sync` starts,
//! stops and restarts the bot to match Settings, so a switch takes effect without a restart.

use super::{conversation_id, pairing, split, Seen, DISCORD_LIMIT};
use crate::conf::Config;
use serenity::{
    async_trait,
    builder::{CreateActionRow, CreateButton, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage},
    client::{Client, Context, EventHandler},
    http::Http,
    model::{
        application::{ButtonStyle, Interaction},
        channel::Message,
        gateway::{GatewayIntents, Ready},
        id::ChannelId,
    },
};
use std::{
    collections::{hash_map::RandomState, BTreeMap},
    hash::{BuildHasher, Hasher},
    sync::{mpsc, Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, sync::watch};

pub const PROVIDER: &str = super::DISCORD;

/// The name the bot token is saved under in the DPAPI secret store (`bridge::web::secret_get`). `ZORUI_DISCORD_BOT_KEY` also works.
pub const TOKEN_SECRET: &str = "discord_bot";

/// What the Settings page shows: "off", "connecting", "connected", or why the bot stopped.
pub static STATUS: Mutex<String> = Mutex::new(String::new());

/// What Settings shows: the bot's state, and while no account is paired, where the pairing code is.
pub fn status() -> String {
    let s = STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if s.is_empty() { return "Not started yet".to_string(); }
    if s == "connected" && pairing::paired(PROVIDER).is_empty() {
        return "Connected. No account paired yet: the pairing code is in remote-pairing.txt in the Zor UI data folder".to_string();
    }
    s
}

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

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Component(pressed) = interaction else { return };
        let Some((choice, id)) = pressed.data.custom_id.split_once(':') else { return };
        let outcome = press(&mut WAITING.lock().unwrap_or_else(|e| e.into_inner()), id, pressed.user.id.get(), choice == "yes");
        let response = match outcome {
            Press::Answered(true) => CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new().content("Allowed.").components(vec![])),
            Press::Answered(false) => CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new().content("Not allowed.").components(vec![])),
            Press::NotYours => CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().ephemeral(true).content("Only the account that asked can answer this.")),
            Press::Gone => CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().ephemeral(true).content("This request is no longer waiting for an answer.")),
        };
        if let Err(e) = pressed.create_response(&ctx.http, response).await {
            crate::log::line(&format!("discord: could not answer a button press: {e}"));
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // only direct messages from people: no bots (including this one), no servers
        if msg.author.bot || msg.guild_id.is_some() { return; }
        if !self.seen.lock().unwrap_or_else(|e| e.into_inner()).first_time(msg.id.get()) { return; }
        let user = msg.author.id.get();
        let text = msg.content.trim().to_string();

        if !pairing::paired(PROVIDER).contains(&user) {
            // an unpaired sender can only pair, with the code the PC shows; anything else gets no reply
            let reply = match pairing::redeem(&text, Instant::now()) {
                pairing::Redeem::Paired => {
                    if !pairing::pair(PROVIDER, user) { set_status("could not save the pairing"); }
                    let _ = std::fs::remove_file(code_file());
                    "Paired. Send me a message and I'll answer here."
                }
                pairing::Redeem::Wrong => "That code is wrong.",
                pairing::Redeem::Expired => "That code expired. Ask the PC for a new one.",
                pairing::Redeem::Locked => "Too many wrong codes. Ask the PC for a new one.",
                pairing::Redeem::NoCode => return,
            };
            let _ = msg.channel_id.say(&ctx.http, reply).await;
            return;
        }

        if text.is_empty() { return; }
        // "stop" ends what the buddy is doing for this PC: the request, a browser run, a take-over
        if text.eq_ignore_ascii_case("stop") {
            crate::agent::cancel_running();
            crate::browse::request_stop();
            crate::drive::request_stop();
            let _ = msg.channel_id.say(&ctx.http, "Stopped.").await;
            return;
        }
        // "typing" shows from now until the answer is sent: dropping the guard stops it
        let typing = msg.channel_id.start_typing(&ctx.http);
        let conv = conversation_id(PROVIDER, user, msg.channel_id.get());
        crate::log::line(&format!("discord request: {} characters", text.chars().count()));
        let chat = chat_in(ctx.http.clone(), Handle::current(), msg.channel_id, user);
        let answer = tokio::task::spawn_blocking(move || crate::agent::ask_remote(&Config::load(), &text, &conv, chat)).await;
        let answer = match answer {
            Ok(Ok(a)) => a,
            Ok(Err(e)) => e,
            Err(_) => "Something went wrong on my side.".to_string(),
        };
        drop(typing);
        for piece in split(&answer, DISCORD_LIMIT) {
            if let Err(e) = msg.channel_id.say(&ctx.http, piece).await {
                crate::log::line(&format!("discord: could not send an answer: {e}"));
                break;
            }
        }
    }
}

/// How long an approval waits for a button press before it counts as Deny.
const APPROVAL_SECS: u64 = 120;

/// An approval waiting for a press: the account that may press, and where its answer goes. Its id is in the buttons' custom ids.
struct Waiting {
    user: u64,
    answer: mpsc::Sender<bool>,
}

static WAITING: Mutex<BTreeMap<String, Waiting>> = Mutex::new(BTreeMap::new());

/// What a press on an approval's button does.
#[derive(Debug, PartialEq, Eq)]
enum Press {
    /// The request is answered now, with this answer, and is gone.
    Answered(bool),
    /// Someone else pressed it. The request still waits.
    NotYours,
    /// Nothing waits under that id: it was answered, or it ran out of time.
    Gone,
}

/// Applies a press. Only the account that was asked may answer, and only once: an answer removes its request.
fn press(waiting: &mut BTreeMap<String, Waiting>, id: &str, by: u64, yes: bool) -> Press {
    match waiting.get(id).map(|w| w.user) {
        None => Press::Gone,
        Some(asked) if asked != by => Press::NotYours,
        Some(_) => {
            if let Some(w) = waiting.remove(id) { let _ = w.answer.send(yes); }
            Press::Answered(yes)
        }
    }
}

/// Asks the account in `channel` whether the buddy may do `question`, with an Allow and a Deny button. Runs on the request's
/// worker thread and waits for the press up to `APPROVAL_SECS`; no press, or a failed send, counts as Deny.
fn approve_in_chat(handle: &Handle, http: &Arc<Http>, channel: ChannelId, user: u64, question: &str) -> bool {
    let id = format!("{:016x}", RandomState::new().build_hasher().finish());
    let (answer, heard) = mpsc::channel();
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), Waiting { user, answer });
    let buttons = CreateActionRow::Buttons(vec![
        CreateButton::new(format!("yes:{id}")).label("Allow").style(ButtonStyle::Success),
        CreateButton::new(format!("no:{id}")).label("Deny").style(ButtonStyle::Danger),
    ]);
    let message = CreateMessage::new().content(question).components(vec![buttons]);
    let sent = handle.block_on(channel.send_message(http, message));
    let yes = sent.is_ok() && heard.recv_timeout(Duration::from_secs(APPROVAL_SECS)).unwrap_or(false);
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    crate::log::line(&format!("discord approval: {}", if yes { "allowed" } else { "not allowed" }));
    yes
}

/// The DM as a `Chat`: its questions are Allow and Deny buttons that `user` presses, and what it is told goes to the same channel.
/// Both can be used from the request's thread or a take-over's thread, which is outside the runtime.
fn chat_in(http: Arc<Http>, handle: Handle, channel: ChannelId, user: u64) -> crate::agent::Chat {
    let (ask_handle, ask_http) = (handle.clone(), http.clone());
    crate::agent::Chat {
        ask: Arc::new(move |question: &str| approve_in_chat(&ask_handle, &ask_http, channel, user, question)),
        tell: Arc::new(move |text: &str| {
            for piece in split(text, DISCORD_LIMIT) {
                if let Err(e) = handle.block_on(channel.say(&http, piece)) {
                    crate::log::line(&format!("discord: could not send a result: {e}"));
                    break;
                }
            }
        }),
    }
}

/// The file the pairing code is written to, for the PC owner to read.
fn code_file() -> std::path::PathBuf { crate::conf::data_dir().join("remote-pairing.txt") }

/// The running bot: the sender that stops it, and the token it runs with. A different token restarts it.
struct Running {
    stop: watch::Sender<bool>,
    token: String,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

static SYNC: OnceLock<mpsc::Sender<()>> = OnceLock::new();

/// Asks the sync worker to look again at the switch and the token. Returns at once: nothing here reads a file or a secret, so the
/// UI thread never waits on the disk.
pub fn request_sync() {
    let tx = SYNC.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        if std::thread::Builder::new().name("discord-sync".into()).spawn(move || sync_worker(rx)).is_err() {
            set_status("could not start the sync thread");
        }
        tx
    });
    let _ = tx.send(());
}

/// Runs one pass per request. Requests that pile up while a pass runs become one more pass, which reads the newest state anyway.
fn sync_worker(rx: mpsc::Receiver<()>) {
    while rx.recv().is_ok() {
        while rx.try_recv().is_ok() {}
        if let Some(enabled) = switch_on_disk() { sync_now(enabled); }
    }
}

/// The switch as config.json says it now. None when the file can't be read or is broken: the bot then stays as it is.
fn switch_on_disk() -> Option<bool> {
    let text = std::fs::read_to_string(crate::conf::pack_dir().join("config.json")).ok()?;
    if Config::is_broken(&text) {
        crate::log::line("discord: config.json is broken, keeping the bot as it is");
        return None;
    }
    Some(Config::parse(&text).bool("remote.discord.enabled"))
}

/// Brings the bot in line with the switch and the saved token: runs it when the switch is on and a token is saved, stops it when
/// the switch is off or the token is cleared, and restarts it when the token changed. Runs on the sync worker only.
fn sync_now(enabled: bool) {
    let token = if enabled { crate::bridge::web::secret_get(TOKEN_SECRET) } else { String::new() };
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    let action = step(enabled, &token, running.as_ref().map(|r| r.token.as_str()));
    if action == Step::Keep { return; }
    if let Some(old) = running.take() { let _ = old.stop.send(true); }
    match action {
        Step::Keep | Step::Stop => {}
        Step::Off => set_status("off"),
        Step::NoToken => set_status("no bot token saved"),
        Step::Start => start(&token, &mut *running),
    }
}

/// What `sync` does: the switch, the saved token, and the token of the bot that runs (None when none runs).
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// The bot already runs with this token.
    Keep,
    /// The switch is off or the token was cleared while the bot ran: it stops.
    Stop,
    /// Off, and nothing runs.
    Off,
    /// On, but there is no token to connect with.
    NoToken,
    /// On with a token that no bot runs with yet (a changed token stops the old bot first).
    Start,
}

fn step(enabled: bool, token: &str, running: Option<&str>) -> Step {
    match (enabled, token.is_empty(), running) {
        (false, _, Some(_)) => Step::Stop,
        (false, _, None) => Step::Off,
        (true, true, Some(_)) => Step::Stop,
        (true, true, None) => Step::NoToken,
        (true, false, Some(r)) if r == token => Step::Keep,
        (true, false, _) => Step::Start,
    }
}

/// Makes the pairing code if no account is paired, and starts the bot thread with `token`.
fn start(token: &str, running: &mut Option<Running>) {
    if pairing::paired(PROVIDER).is_empty() {
        let code = pairing::new_code(Instant::now());
        let note = format!("Pairing code for Discord: {code}\r\nSend it to the bot in a direct message within {} minutes.\r\n", pairing::CODE_SECS / 60);
        let _ = std::fs::write(code_file(), note);
    }
    let (stop, stop_rx) = watch::channel(false);
    let worker_token = token.to_string();
    match std::thread::Builder::new().name("discord".into()).spawn(move || run(worker_token, stop_rx)) {
        Ok(_) => *running = Some(Running { stop, token: token.to_string() }),
        Err(_) => set_status("could not start the thread"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting_for(user: u64) -> (BTreeMap<String, Waiting>, mpsc::Receiver<bool>) {
        let (answer, heard) = mpsc::channel();
        let mut map = BTreeMap::new();
        map.insert("a1".to_string(), Waiting { user, answer });
        (map, heard)
    }

    #[test]
    fn only_the_asked_account_answers_an_approval_and_only_once() {
        let (mut map, heard) = waiting_for(42);
        assert_eq!(press(&mut map, "a1", 7, true), Press::NotYours, "someone else's press changes nothing");
        assert!(map.contains_key("a1"), "the request still waits");
        assert_eq!(press(&mut map, "a1", 42, true), Press::Answered(true));
        assert_eq!(heard.recv().ok(), Some(true), "the answer reaches the request");
        assert_eq!(press(&mut map, "a1", 42, true), Press::Gone, "a second press finds nothing");
        assert_eq!(press(&mut map, "unknown", 42, true), Press::Gone);
    }

    #[test]
    fn a_deny_press_answers_no() {
        let (mut map, heard) = waiting_for(42);
        assert_eq!(press(&mut map, "a1", 42, false), Press::Answered(false));
        assert_eq!(heard.recv().ok(), Some(false));
    }

    #[test]
    fn the_switch_and_the_token_decide_what_runs() {
        assert_eq!(step(false, "", None), Step::Off);
        assert_eq!(step(false, "t", None), Step::Off, "a saved token does not start a switched-off bot");
        assert_eq!(step(false, "t", Some("t")), Step::Stop, "switching off stops it");
        assert_eq!(step(true, "", None), Step::NoToken);
        assert_eq!(step(true, "", Some("t")), Step::Stop, "clearing the token stops it");
        assert_eq!(step(true, "t", None), Step::Start);
        assert_eq!(step(true, "t", Some("t")), Step::Keep, "a reload with nothing changed does not restart it");
        assert_eq!(step(true, "new", Some("old")), Step::Start, "a new token starts the bot again with it");
    }
}

fn run(token: String, mut stop: watch::Receiver<bool>) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => { set_status(&format!("could not start the runtime: {e}")); return; }
    };
    rt.block_on(async move {
        // a dropped connection is made again after a wait that doubles up to five minutes
        let mut wait = 5u64;
        loop {
            set_status("connecting");
            let intents = GatewayIntents::DIRECT_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
            let handler = Handler { seen: Mutex::new(Seen::new(512)) };
            match Client::builder(&token, intents).event_handler(handler).await {
                Ok(mut client) => {
                    let shards = client.shard_manager.clone();
                    tokio::select! {
                        r = client.start() => { if let Err(e) = r { set_status(&format!("stopped: {e}")); } }
                        _ = stop.changed() => { shards.shutdown_all().await; set_status("off"); return; }
                    }
                }
                Err(e) => set_status(&format!("could not connect: {e}")),
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                _ = stop.changed() => { set_status("off"); return; }
            }
            wait = (wait * 2).min(300);
        }
    });
}
