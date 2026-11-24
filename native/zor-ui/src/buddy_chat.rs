//! The buddy's chat window: every task the buddy did is a chat on the left, newest first, and the open one on the right, with
//! an input bar to continue it by text, a mic to continue it by voice, and Live mode (`live.rs`) to talk back and forth
//! hands-free. A chat is read from the buddy's database (`agent::history`), so a request made by voice or hotkey shows up
//! here too, and reopening a chat days later gives the buddy its whole context back. Messages, the input bar and the model
//! picker are drawn by `chat_ui`, which the sidebar's AI tab shares. Any empty spot drags the window, its edges resize it, and
//! the pin keeps it above other windows.

mod live;

use crate::{
    agent::{self, history},
    app::{Action, Ctx},
    assist::{self, ChatEvent},
    chat_ui::{input, models::ModelMenu, transcript::{Clicked, Message, Role, Tool, Transcript}, Inbox},
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    hub, tokens,
    ui::{self, Edit, IconKind, Scroll, TextBox},
    win::{Input, Phase as Drag, View},
};
use live::{Act, Live, Phase};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// The popup name (popup.rs, app.rs).
pub const NAME: &str = "buddy-chat";

const MIN_SIZE: (f32, f32) = (720.0, 480.0);
const MAX_SIZE: (f32, f32) = (4000.0, 3000.0);
const SIZE_KEY: &str = "buddyChat.size";
/// Settings' Auto mode: consequential steps run without the island's question (agent/policy.rs `auto_skips`).
const AUTO_KEY: &str = "buddies.autoMode";

/// The config a request from the chat runs with: the app's, with Auto mode as the chat's switch shows it, so a request sent
/// right after the switch was flipped doesn't wait for config.json to be read again.
pub fn with_auto(cfg: &crate::conf::Config, auto: bool) -> crate::conf::Config {
    let mut v = cfg.0.clone();
    if !v["buddies"].is_object() { v["buddies"] = serde_json::json!({}); }
    v["buddies"]["autoMode"] = serde_json::Value::Bool(auto);
    crate::conf::Config(v)
}

/// Whether the pin was on, so the window comes back pinned.
const TOP_KEY: &str = "buddyChat.onTop";
/// The model menu's width, and the space between the header's buttons.
const MENU_W: f32 = 320.0;
const HEAD_BTN_GAP: f32 = 8.0;

/// The window's card inside its shadow margin, and its corner radius.
const INSET: f32 = 6.0;
const PANEL_R: f32 = 30.0;
/// How solid the card is at least. The bar's transparency setting would let the windows behind show through the text, which
/// a page of chat can't afford; a little of the glass stays.
const CARD_OPACITY: f32 = 0.94;
/// Space inside the card on every side, and between the conversation list and the chat.
const PAD: f32 = 12.0;
/// Inside the conversation list, on every side.
const RAIL_PAD: f32 = 8.0;
const RAIL_R: f32 = 22.0;
/// The "New chat" button's height.
const NEW_H: f32 = 40.0;
/// One conversation row: its height, the gap between rows, and the space inside it.
pub const ROW_H: f32 = 52.0;
pub const ROW_GAP: f32 = 8.0;
const ROW_PAD: f32 = 12.0;
/// The chat's title row, and the gap under it.
const HEAD_H: f32 = 36.0;
const HEAD_GAP: f32 = 8.0;
/// The one-line note over the input bar ("busy with another request"), and its gap.
const NOTE_H: f32 = 20.0;
const NOTE_GAP: f32 = 6.0;
/// The gap between the messages and the input bar.
const BAR_GAP: f32 = 10.0;
/// The Live orb at rest, how much the voice can grow it, and the End button.
const ORB_R: f32 = 56.0;
const ORB_GROW: f32 = 36.0;
const END_W: f32 = 128.0;
const END_H: f32 = 40.0;
/// The widest the Live captions run, and the gap between two captions.
const CAPTION_W: f32 = 520.0;
const CAPTION_GAP: f32 = 10.0;
/// Where the orb's centre sits, as a share of the Live area's height.
const ORB_AT: f32 = 0.32;

// hit ids (clear of chat_ui::input's and md's ranges)
const BG: u32 = 1;
const NEW: u32 = 2;
const CLOSE: u32 = 3;
const RAIL: u32 = 4;
const VIEW: u32 = 5;
const MIC: u32 = 6;
const LIVE: u32 = 7;
const ORB: u32 = 8;
const END: u32 = 9;
const PIN: u32 = 10;
const AUTO: u32 = 11;
const ROW0: u32 = 100;

/// Set while Live mode runs, so the buddy hotkey taps the orb instead of starting a new request (`live_tap`).
static LIVE_ON: AtomicBool = AtomicBool::new(false);
/// The hotkey was pressed during Live mode; the window taps the orb at its next frame.
static TAPPED: AtomicBool = AtomicBool::new(false);

/// True while Live mode runs: the buddy's bubble voice stays quiet then, since Live speaks the answer itself (buddy.rs).
pub fn live_on() -> bool { LIVE_ON.load(Ordering::Relaxed) }

/// The buddy hotkey during Live mode: taps the orb (cuts the voice and listens). False when Live mode is not running, so the
/// hotkey does what it always does.
pub fn live_tap() -> bool {
    if !LIVE_ON.load(Ordering::Relaxed) { return false; }
    TAPPED.store(true, Ordering::Relaxed);
    crate::chat_ui::wake();
    true
}

/// The chip under an answer for how its task ended (`TaskStatus` as saved in the turn), and whether it is a warning. None for
/// a turn without a task, and for an ending that needs no chip.
pub fn status_chip(status: &str) -> Option<(&'static str, bool)> {
    Some(match status {
        "Completed" => ("Done", false),
        "Unverified" => ("Not checked", false),
        "Partial" => ("Partly done", true),
        "Failed" => ("Failed", true),
        "Declined" => ("Declined", false),
        "Cancelled" => ("Stopped", false),
        "Interrupted" => ("Waiting", false),
        _ => return None,
    })
}

/// How long ago unix time `at` was, from `now`, as a row shows it: "now", "5m", "3h", "2d", or the date.
pub fn ago(now: i64, at: i64) -> String {
    let s = (now - at).max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        86_400..=604_799 => format!("{}d", s / 86_400),
        _ => { let (y, m, d) = crate::bridge::football::civil_from_days(at.div_euclid(86_400)); format!("{y:04}-{m:02}-{d:02}") }
    }
}

/// A chat's title in the list: its first words on one line, or "Earlier" for the old shared conversation.
pub fn title(conversation: &str, first: &str) -> String {
    if conversation == agent::DESKTOP { return "Earlier".into(); }
    let one = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() { "Chat".into() } else { assist::short(&one, 60) }
}

/// The tool steps saved with a turn or sent by the running request (`plan::Steps::shown_json`); anything unreadable is left out.
pub fn tools_from_json(text: &str) -> Vec<Tool> {
    let v: Vec<serde_json::Value> = serde_json::from_str(text).unwrap_or_default();
    v.iter().filter_map(|t| Some(Tool {
        icon: t["icon"].as_str().unwrap_or("build").to_string(),
        label: t["label"].as_str()?.to_string(),
        status: t["status"].as_str().unwrap_or("done").to_string(),
    })).collect()
}

/// The messages of a chat as the list draws them: each saved turn, then the request still running (its text and tool steps so
/// far, `live`), then the last error. `open`: the indexes of the replies whose tool steps are unfolded.
pub fn messages(shown: &[history::Shown], pending: Option<&str>, live: Option<&assist::LiveChat>, failed: Option<&(String, String)>, open: &HashSet<usize>) -> Vec<Message> {
    let mut out: Vec<Message> = vec![];
    for t in shown {
        out.push(Message { role: Role::User, text: t.said.clone(), ..Message::default() });
        let chip = status_chip(&t.status);
        let i = out.len();
        out.push(Message { role: Role::Assistant, text: t.answer.clone(), who: "Buddy".into(), chip: chip.map(|c| c.0.to_string()).unwrap_or_default(),
            chip_warn: chip.is_some_and(|c| c.1), tools: tools_from_json(&t.tools), tools_open: open.contains(&i), ..Message::default() });
    }
    if let Some(p) = pending {
        out.push(Message { role: Role::User, text: p.to_string(), ..Message::default() });
        let (text, tools) = live.map(|l| (l.text.clone(), tools_from_json(&l.tools))).unwrap_or_default();
        let i = out.len();
        out.push(Message { role: Role::Assistant, who: "Buddy".into(), pending: true, text, tools, tools_open: open.contains(&i), ..Message::default() });
    }
    if let Some((said, e)) = failed {
        out.push(Message { role: Role::User, text: said.clone(), ..Message::default() });
        out.push(Message { role: Role::Assistant, text: e.clone(), who: "Buddy".into(), error: true, ..Message::default() });
    }
    out
}

pub struct BuddyChat {
    area: (f32, f32),
    chats: Vec<history::ChatRow>,
    /// The chat shown; None is a new chat, which gets its id when the first message is sent.
    conv: Option<String>,
    shown: Vec<history::Shown>,
    /// The `history::VERSION` the list and the chat were read at; None reads them at the next paint.
    read_at: Option<u64>,
    /// The request sent from here that has not answered yet, and the last one that failed (shown, not saved).
    pending: Option<String>,
    failed: Option<(String, String)>,
    input: TextBox,
    tr: Transcript,
    rail: Scroll,
    rail_dims: (f32, f32),
    live: Option<Live>,
    focus: bool,
    /// The replies whose tool steps are unfolded, by their index in the list.
    open: HashSet<usize>,
    /// The model picker, the model chosen (empty: the default), and the `ai-models` answer on its way.
    mm: ModelMenu,
    model: String,
    models_in: Inbox<serde_json::Value>,
    /// The pin: above every other window. `pin_sent` once the saved pin was applied to the new window.
    on_top: bool,
    pin_sent: bool,
    /// Auto mode as the header's switch shows it (saved to `buddies.autoMode`).
    auto: bool,
}

impl BuddyChat {
    /// `area`: the size the popup placement gives (popup::area); a size the user dragged to wins.
    pub fn new(area: (f32, f32)) -> BuddyChat {
        let area = hub::get(SIZE_KEY).and_then(|s| parse_size(&s)).unwrap_or(area);
        let mut input = TextBox::new(true);
        input.enter_submits = true;
        BuddyChat { area, chats: vec![], conv: None, shown: vec![], read_at: None, pending: None, failed: None, input, tr: Transcript::new(),
            rail: Scroll::default(), rail_dims: (0.0, 0.0), live: None, focus: true, open: HashSet::new(),
            mm: ModelMenu::default(), model: crate::conf::Config::load().str("ai.model").to_string(), models_in: load_models(),
            on_top: hub::get(TOP_KEY).as_deref() == Some("1"), pin_sent: false, auto: crate::conf::Config::load().bool(AUTO_KEY) }
    }

    /// The model the buddy uses, as the chip names it.
    fn model_name(&self) -> String { self.mm.info(if self.model.is_empty() { DEFAULT_MODEL } else { &self.model }).name }

    /// Picks the buddy's model: the one the sidebar's AI tab uses too (`ai.model`), saved through the bridge.
    fn pick_model(&mut self, id: &str, cx: &Ctx) {
        self.model = id.to_string();
        let (cfg, v) = (crate::conf::Config(cx.cfg.0.clone()), serde_json::Value::from(id).to_string());
        std::thread::spawn(move || { crate::proc::bridge(&cfg, "config-set", &["ai.model", &v]); });
    }

    /// Reads the list and the open chat again when a turn was saved since. The first read opens the newest chat.
    fn refresh(&mut self) {
        let v = history::VERSION.load(Ordering::Relaxed);
        if self.read_at == Some(v) { return; }
        let first = self.read_at.is_none();
        self.read_at = Some(v);
        self.chats = history::chats(200);
        if first && self.conv.is_none() { self.conv = self.chats.first().map(|c| c.conversation.clone()); }
        self.shown = self.conv.as_deref().map(history::turns_of).unwrap_or_default();
        // the request sent from here is saved now: it shows as a turn, not as pending
        if self.pending.as_deref().is_some_and(|p| self.shown.last().is_some_and(|t| t.said == p)) { self.pending = None; }
    }

    fn open_chat(&mut self, conv: Option<String>) {
        if self.live.is_some() { self.end_live(); }
        self.conv = conv;
        self.pending = None;
        self.failed = None;
        self.read_at = Some(u64::MAX); // forces the next refresh
        self.open.clear();
        self.tr.reset();
    }

    /// The chat's id, made now for a new chat.
    fn conv_id(&mut self) -> String {
        if self.conv.is_none() { self.conv = Some(agent::new_conversation()); }
        self.conv.clone().unwrap_or_default()
    }

    fn send(&mut self, cx: &Ctx) {
        if self.pending.is_some() { agent::cancel_running(); return; } // the stop button
        let text = self.input.text.trim().to_string();
        if text.is_empty() { return; }
        if assist::busy() { return; } // the note over the bar says why
        self.input.set("");
        let conv = self.conv_id();
        self.failed = None;
        self.pending = Some(text.clone());
        self.tr.stick = true;
        assist::ask_in(&with_auto(&cx.cfg, self.auto), &text, &conv);
    }

    /// Picks up what happened to this chat's requests and runs Live mode's next step.
    fn drain(&mut self, cx: &Ctx) {
        let now = Instant::now();
        let conv = self.conv.clone().unwrap_or_default();
        let mut acts = vec![];
        for (c, e) in assist::take_events() {
            if c != conv { continue; }
            match e {
                ChatEvent::Heard(t) => {
                    if !t.is_empty() && self.live.is_none() { self.pending = Some(t.clone()); self.tr.stick = true; }
                    if let Some(l) = &mut self.live {
                        if !t.is_empty() && !live::is_end(&t) { self.pending = Some(t.clone()); self.tr.stick = true; }
                        acts.extend(l.on_heard(&t, now));
                    }
                }
                ChatEvent::Answered { said, result } => {
                    self.pending = None;
                    self.failed = result.as_ref().err().map(|e| (said.clone(), e.clone()));
                    self.read_at = Some(u64::MAX); // read the saved turn
                    if let Some(l) = &mut self.live { acts.extend(l.on_answer(&result, now)); }
                }
            }
        }
        if TAPPED.swap(false, Ordering::Relaxed) { if let Some(l) = &mut self.live { acts.extend(l.tap(now)); } }
        // the answer as it streams: Live speaks each finished sentence
        if let (Some(l), Some(lc)) = (&mut self.live, assist::live_chat(&conv)) { acts.extend(l.on_text(&lc.text, now)); }
        for v in self.models_in.take() { self.mm.set(&v); }
        if let Some(l) = &mut self.live { acts.extend(l.tick(crate::voice::idle(), now)); }
        for a in acts { self.act(a, cx); }
    }

    fn act(&mut self, a: Act, cx: &Ctx) {
        match a {
            Act::Listen => self.listen(cx),
            Act::Interrupt => { crate::voice::stop(); self.listen(cx); }
            // the bubble's voice stays quiet in Live mode (buddy.rs), so this is the one voice saying it.
            // Each sentence queues after the last; a plain speak would cut the one still playing.
            Act::Speak(s) => { let _ = cx; crate::voice::speak_after(0, &s); }
            Act::End => self.end_live(),
        }
    }

    fn listen(&mut self, cx: &Ctx) {
        // Live mode: load the voice while the user talks, so the answer's first line is not held up by it
        if self.live.is_some() { crate::voice::warm(0); }
        let conv = self.conv_id();
        assist::listen_into(&with_auto(&cx.cfg, self.auto), false, Some(conv));
    }

    fn start_live(&mut self, cx: &Ctx) {
        if self.live.is_some() || assist::busy() { return; }
        let (l, first) = Live::start();
        self.live = Some(l);
        LIVE_ON.store(true, Ordering::Relaxed);
        self.act(first, cx);
    }

    fn end_live(&mut self) {
        if self.live.take().is_none() { return; }
        LIVE_ON.store(false, Ordering::Relaxed);
        assist::cancel(); // a recording still running stops
        crate::voice::stop();
    }

    fn paint_rail(&mut self, c: &mut Canvas, cx: &Ctx, rail: R) {
        let t = &cx.theme;
        c.rect(rail, RAIL_R, t.c("surfaceContainerLow"));
        let nb = R::new(rail.x + RAIL_PAD, rail.y + RAIL_PAD, rail.w - 2.0 * RAIL_PAD, NEW_H);
        let new_sel = self.conv.is_none();
        c.rect(nb, NEW_H / 2.0, if new_sel { t.c("secondaryContainer") } else { t.c("surfaceContainerHigh") });
        if c.hit(nb, NEW) { c.rect(nb, NEW_H / 2.0, alpha(t.c("onSurface"), tokens::HOVER_A)); }
        c.icon("edit_square", R::new(nb.x + ROW_PAD, nb.y, 20.0, NEW_H), ico(20.0, false), t.c("onSecondaryContainer"));
        c.text("New chat", R::new(nb.x + ROW_PAD + 28.0, nb.y, nb.w - ROW_PAD - 28.0, NEW_H), txt(13.5, 500), t.c("onSecondaryContainer"), Align::Left);
        let list = R::new(rail.x + RAIL_PAD, nb.y + NEW_H + RAIL_PAD, rail.w - 2.0 * RAIL_PAD, rail.y + rail.h - RAIL_PAD - (nb.y + NEW_H + RAIL_PAD));
        c.hit(list, RAIL);
        let n = self.chats.len();
        let content = if n == 0 { ROW_H } else { n as f32 * (ROW_H + ROW_GAP) - ROW_GAP };
        self.rail_dims = (content, list.h);
        let now = history::now();
        let (chats, conv) = (&self.chats, self.conv.as_deref());
        let room = ui::bar_room(content, list.h);
        self.rail.region(c, cx, list, content, |c, top| {
            if n == 0 { c.text("No chats yet", R::new(list.x + ROW_PAD, top, list.w - 2.0 * ROW_PAD, ROW_H), txt(12.5, 400), t.c("onSurfaceVariant"), Align::Left); }
            for (i, ch) in chats.iter().enumerate() {
                let y = top + i as f32 * (ROW_H + ROW_GAP);
                if y + ROW_H < list.y || y > list.y + list.h { continue; }
                let row = R::new(list.x, y, list.w - room, ROW_H);
                let sel = conv == Some(ch.conversation.as_str());
                if sel { c.rect(row, tokens::NAV_R, t.c("secondaryContainer")); }
                if c.hit(row, ROW0 + i as u32) && !sel { c.rect(row, tokens::NAV_R, alpha(t.c("onSurface"), tokens::HOVER_A)); }
                let fg = if sel { t.c("onSecondaryContainer") } else { t.c("onSurface") };
                c.text(&title(&ch.conversation, &ch.title), R::new(row.x + ROW_PAD, row.y + 8.0, row.w - 2.0 * ROW_PAD, 18.0), txt(13.0, 500), fg, Align::Left);
                c.text(&ago(now, ch.at), R::new(row.x + ROW_PAD, row.y + 28.0, row.w - 2.0 * ROW_PAD, 16.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Left);
            }
        });
    }

    fn paint_empty(&self, c: &mut Canvas, cx: &Ctx, view: R) {
        let t = &cx.theme;
        let w = (view.w - 2.0 * PAD).min(CAPTION_W);
        let x = view.x + (view.w - w) / 2.0;
        let hint = "Type or tap the mic to talk. Live talks back and forth hands-free. Every task the buddy does is kept here as its own chat, so you can come back and ask about it.";
        let hh = c.g.measure_wrapped(hint, txt(12.5, 400), w);
        let total = 48.0 + 12.0 + 24.0 + 8.0 + hh;
        let mut y = view.y + ((view.h - total) / 2.0).max(0.0);
        c.icon("smart_toy", R::new(x, y, w, 48.0), ico(48.0, false), t.c("primary"));
        y += 48.0 + 12.0;
        c.text("Ask the buddy anything", R::new(x, y, w, 24.0), txt(17.0, 500), t.c("onSurface"), Align::Center);
        y += 24.0 + 8.0;
        c.para(hint, R::new(x, y, w, hh), txt(12.5, 400), t.c("onSurfaceVariant"), Align::Center);
    }

    fn paint_live(&self, c: &mut Canvas, cx: &Ctx, area: R, l: &Live) {
        let t = &cx.theme;
        let secs = hub::now_ms() / 1000.0;
        let (col, grow) = match l.phase {
            Phase::Listening => (t.c("primary"), (crate::pill::loudness() * 6.0).clamp(0.0, 1.0)),
            Phase::Thinking => (t.c("tertiary"), (((secs * 2.0).sin() * 0.5 + 0.5) * 0.3) as f32),
            Phase::Speaking => (t.c("secondary"), (((secs * 3.0).sin() * 0.5 + 0.5) * 0.5) as f32),
        };
        let r = ORB_R + ORB_GROW * grow;
        // the orb sits high enough to leave the captions their room on the shortest window, and never under the title row
        let (ox, oy) = (area.x + area.w / 2.0, area.y + (area.h * ORB_AT).max(ORB_R + ORB_GROW + 18.0));
        c.circle(ox, oy, r + 18.0, alpha(col, 0.12));
        c.circle(ox, oy, r, col);
        c.hit(R::new(ox - r, oy - r, 2.0 * r, 2.0 * r), ORB);
        c.icon(match l.phase { Phase::Listening => "mic", Phase::Thinking => "more_horiz", Phase::Speaking => "graphic_eq" }, R::new(ox - 20.0, oy - 20.0, 40.0, 40.0), ico(40.0, false), t.c("surface"));
        let w = (area.w - 2.0 * PAD).min(CAPTION_W);
        let x = area.x + (area.w - w) / 2.0;
        let mut y = oy + ORB_R + ORB_GROW + 24.0;
        c.text(l.label(), R::new(x, y, w, 20.0), txt(13.0, 500), t.c("onSurfaceVariant"), Align::Center);
        y += 20.0 + 12.0;
        let heard = if l.phase == Phase::Listening { crate::pill::heard() } else { l.heard.clone() };
        let end = R::new(area.x + (area.w - END_W) / 2.0, area.y + area.h - END_H - PAD, END_W, END_H);
        // a caption is drawn only when it fits whole above the End button: shortened first, left out if it still doesn't
        let caption = |c: &mut Canvas, y: &mut f32, text: &str, size: f32, col: u32| {
            for s in [assist::short(text, 280), assist::short(text, 100), assist::short(text, 50)] {
                let h = c.g.measure_wrapped(&s, txt(size, 400), w);
                if *y + h <= end.y - PAD { c.para(&s, R::new(x, *y, w, h), txt(size, 400), col, Align::Center); *y += h + CAPTION_GAP; return; }
            }
        };
        // while the buddy speaks, the line being heard is the caption, like subtitles (the chat keeps the whole answer);
        // before that, what was heard
        let saying = if l.phase == Phase::Speaking { crate::voice::saying() } else { String::new() };
        if !saying.is_empty() { caption(c, &mut y, &saying, 14.5, t.c("onSurface")); }
        else if !heard.is_empty() { caption(c, &mut y, &crate::pill::tail(&heard, 160), 13.0, t.c("onSurfaceVariant")); }
        c.rect(end, END_H / 2.0, t.c("error"));
        if c.hit(end, END) { c.rect(end, END_H / 2.0, alpha(t.c("onError"), 0.1)); }
        c.icon("call_end", R::new(end.x + 18.0, end.y, 22.0, END_H), ico(22.0, false), t.c("onError"));
        c.text("End", R::new(end.x + 46.0, end.y, end.w - 46.0 - 18.0, END_H), txt(13.5, 500), t.c("onError"), Align::Left);
    }
}

/// The model the buddy uses when none is set (agent.rs `run_task`).
const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// Asks the bridge for the models on a thread; the answer waits in the inbox for the next paint.
fn load_models() -> Inbox<serde_json::Value> {
    let ib = Inbox::new();
    let out = ib.clone();
    std::thread::spawn(move || {
        let v = crate::proc::bridge(&crate::conf::Config::load(), "ai-models", &[]);
        out.push(v);
        crate::chat_ui::wake();
    });
    ib
}

fn clamp_size((w, h): (f32, f32)) -> (f32, f32) { (w.clamp(MIN_SIZE.0, MAX_SIZE.0), h.clamp(MIN_SIZE.1, MAX_SIZE.1)) }

/// The saved "w,h" clamped to the limits, if it parses.
fn parse_size(s: &str) -> Option<(f32, f32)> {
    let (w, h) = s.split_once(',')?;
    Some(clamp_size((w.trim().parse().ok()?, h.trim().parse().ok()?)))
}

impl Drop for BuddyChat {
    // closing the window ends Live mode: nothing keeps listening or talking behind it
    fn drop(&mut self) { self.end_live(); }
}

impl View for BuddyChat {
    fn size(&self, _: &Gfx, _: &Ctx, _: f32) -> (f32, f32) { self.area }
    fn hand(&self, id: u32) -> bool { !matches!(id, 0 | BG | RAIL | VIEW | input::TEXT) && !ModelMenu::is_background(id) }
    fn key(&self, _: &Ctx) -> String {
        match self.frame_ms() { 0 => String::new(), ms => ((hub::now_ms() / ms as f64) as u64).to_string() }
    }
    fn frame_ms(&self) -> u32 { if self.live.is_some() { 33 } else if self.pending.is_some() || crate::pill::state() != 0 { 100 } else { 500 } }
    fn movable(&self) -> f32 { self.area.1 }
    fn captures_esc(&self) -> bool { true }
    fn limits(&self) -> Option<((f32, f32), (f32, f32))> { Some((MIN_SIZE, MAX_SIZE)) }
    fn resize(&mut self, w: f32, h: f32) { self.area = clamp_size((w, h)); }
    fn resize_end(&mut self) { hub::set(SIZE_KEY, Some(format!("{},{}", self.area.0.round(), self.area.1.round()))); }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        self.drain(cx);
        self.refresh();
        let t = &cx.theme;
        let panel = R::new(INSET, INSET, c.w - 2.0 * INSET, c.h - 2.0 * INSET);
        ui::panel(c, cx, panel, PANEL_R);
        c.rect(R::new(panel.x + 1.0, panel.y + 1.0, panel.w - 2.0, panel.h - 2.0), PANEL_R - 1.0, alpha(t.c("background"), CARD_OPACITY));
        // no spot over the card itself: an empty spot is where a press drags the window (win.rs)
        let rail = R::new(panel.x + PAD, panel.y + PAD, tokens::NAV_W, panel.h - 2.0 * PAD);
        self.paint_rail(c, cx, rail);
        let main = R::new(rail.x + rail.w + PAD, panel.y + PAD, panel.x + panel.w - PAD - (rail.x + rail.w + PAD), panel.h - 2.0 * PAD);

        // the title row: the chat's first words, and close
        let head = R::new(main.x, main.y, main.w, HEAD_H);
        let name = match (&self.conv, self.chats.iter().find(|ch| Some(&ch.conversation) == self.conv.as_ref())) {
            (Some(c0), Some(ch)) => title(c0, &ch.title),
            _ => "New chat".into(),
        };
        let name = if self.live.is_some() { format!("Live · {name}") } else { name };
        // right to left: close, pin, then the model chip; the title takes what is left
        let close = R::new(head.x + head.w - 36.0, head.y, 36.0, 36.0);
        ui::icon_btn(c, cx, close, CLOSE, "close", IconKind::Plain, 22.0, false);
        let pin = R::new(close.x - HEAD_BTN_GAP - 36.0, head.y, 36.0, 36.0);
        ui::icon_btn(c, cx, pin, PIN, "keep", if self.on_top { IconKind::Tonal } else { IconKind::Plain }, 22.0, self.on_top);
        let model = self.model_name();
        let chip_max = (head.w * 0.4).min(260.0);
        let cw = c.g.measure(&model, txt(13.0, 400)).0.ceil() + 2.0 * 14.0 + 48.0;
        let chip = ModelMenu::chip(c, cx, pin.x - HEAD_BTN_GAP - cw.min(chip_max), head.y + (HEAD_H - crate::chat_ui::models::CHIP_H) / 2.0, chip_max, &model);
        // Auto mode: on, the buddy's steps run without asking (power and a site's send or buy still ask)
        let aw = 2.0 * tokens::CHIP_PAD + tokens::CHIP_ICON + tokens::CHIP_ICON_GAP + c.g.measure("Auto", txt(13.0, 400)).0.ceil();
        let auto = ui::chip(c, cx, chip.x - HEAD_BTN_GAP - aw, head.y + (HEAD_H - tokens::CHIP_H) / 2.0, AUTO, Some(if self.auto { "bolt" } else { "back_hand" }), "Auto", self.auto);
        c.text(&name, R::new(head.x + 4.0, head.y, auto.x - PAD - head.x - 4.0, HEAD_H), txt(16.0, 500), t.c("onSurface"), Align::Left);
        let menu_at = R::new((chip.x + chip.w - MENU_W).max(main.x), chip.y + chip.h + 6.0, MENU_W.min(main.w), 0.0);
        let body = R::new(main.x, head.y + HEAD_H + HEAD_GAP, main.w, main.y + main.h - (head.y + HEAD_H + HEAD_GAP));

        if let Some(l) = self.live.take() {
            self.paint_live(c, cx, body, &l);
            self.live = Some(l);
            let cur = if self.model.is_empty() { DEFAULT_MODEL.to_string() } else { self.model.clone() };
            self.mm.paint(c, cx, menu_at, body.h, &cur);
            return;
        }
        let pending = self.pending.is_some();
        let mic_on = crate::pill::state() == crate::pill::LISTENING;
        let extras = [input::Extra { id: MIC, icon: "mic", on: mic_on }, input::Extra { id: LIVE, icon: "graphic_eq", on: false }];
        let bar = input::paint(c, cx, body, &mut self.input, "Ask the buddy\u{2026}", self.focus, pending, &extras);
        let mut bottom = bar.y - BAR_GAP;
        if assist::busy() && !pending {
            let note = R::new(body.x + 4.0, bar.y - NOTE_GAP - NOTE_H, body.w - 8.0, NOTE_H);
            c.text("The buddy is working on another request. Send when it's done.", note, txt(12.0, 400), t.c("onSurfaceVariant"), Align::Left);
            bottom = note.y - NOTE_GAP;
        }
        let view = R::new(body.x, body.y, body.w, (bottom - body.y).max(40.0));
        c.hit(view, VIEW);
        let live = self.conv.as_deref().and_then(assist::live_chat);
        let msgs = messages(&self.shown, self.pending.as_deref(), live.as_ref(), self.failed.as_ref(), &self.open);
        if msgs.is_empty() { self.paint_empty(c, cx, view); } else { self.tr.paint(c, cx, view, &msgs, "smart_toy"); }
        // the model menu, on top of everything
        let cur = if self.model.is_empty() { DEFAULT_MODEL.to_string() } else { self.model.clone() };
        self.mm.paint(c, cx, menu_at, body.h, &cur);
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        if let Input::Click(id, _) = &ev {
            let (took, pick) = self.mm.click(*id);
            if let Some(m) = pick { self.pick_model(&m, cx); }
            if took { return; }
        }
        match &ev {
            Input::Focus(on) => {
                self.focus = *on;
                // a pinned window comes back pinned: the first focus is right after it opens
                if *on && !self.pin_sent { self.pin_sent = true; if self.on_top { cx.actions.push(Action::OnTop(NAME.into(), true)); } }
            }
            Input::Click(AUTO, _) => {
                self.auto = !self.auto;
                let (cfg, v) = (crate::conf::Config(cx.cfg.0.clone()), self.auto.to_string());
                std::thread::spawn(move || { crate::proc::bridge(&cfg, "config-set", &[AUTO_KEY, &v]); });
            }
            Input::Click(PIN, _) => {
                self.on_top = !self.on_top;
                hub::set(TOP_KEY, Some(if self.on_top { "1".into() } else { "0".into() }));
                cx.actions.push(Action::OnTop(NAME.into(), self.on_top));
            }
            Input::Click(CLOSE, _) => cx.actions.push(Action::Close(NAME.into())),
            Input::Click(NEW, _) => { self.open_chat(None); self.input.set(""); }
            Input::Click(id, _) if (ROW0..ROW0 + 10_000).contains(id) => {
                let conv = self.chats.get((*id - ROW0) as usize).map(|c| c.conversation.clone());
                if conv.is_some() && conv != self.conv { self.open_chat(conv); }
            }
            Input::Click(input::SEND, _) => self.send(cx),
            Input::Click(MIC, _) => {
                let conv = self.conv_id();
                cx.actions.push(Action::Command(format!("buddy listen-in {conv}")));
            }
            Input::Click(LIVE, _) => self.start_live(cx),
            Input::Click(ORB, _) => { if let Some(a) = self.live.as_mut().and_then(|l| l.tap(Instant::now())) { self.act(a, cx); } }
            Input::Click(END, _) => self.end_live(),
            Input::Click(id, _) => match self.tr.click(*id) {
                // the bridge's `open` checks the link and opens it, off the UI thread (as the AI tab does)
                Some(Clicked::Link(u)) => {
                    let cfg = crate::conf::Config(cx.cfg.0.clone());
                    std::thread::spawn(move || { crate::proc::bridge(&cfg, "open", &[&u]); });
                }
                Some(Clicked::Tools(i)) => { if !self.open.remove(&i) { self.open.insert(i); } }
                _ => {}
            },
            Input::Key { vk: 0x1B, .. } => if self.mm.open { self.mm.open = false } else if self.live.is_some() { self.end_live() } else { cx.actions.push(Action::Close(NAME.into())) },
            Input::Drag(input::TEXT, x, y, Drag::Start) => self.input.press(*x, *y),
            Input::Key { vk, ctrl, shift, alt } if self.live.is_none() => if self.input.key(*vk, *ctrl, *shift, *alt) == Edit::Submit { self.send(cx); },
            Input::Char(ch) if self.live.is_none() => { self.input.ch(*ch); }
            Input::Wheel(_, d) if self.mm.open => self.mm.wheel(*d),
            Input::Wheel(RAIL, d) => self.rail.wheel(*d, self.rail_dims.0, self.rail_dims.1),
            Input::Wheel(_, d) => self.tr.wheel(*d),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(said: &str, answer: &str, status: &str) -> history::Shown { history::Shown { said: said.into(), answer: answer.into(), at: 0, status: status.into(), tools: String::new() } }

    #[test]
    fn a_task_status_becomes_a_chip_and_a_plain_turn_gets_none() {
        assert_eq!(status_chip("Completed"), Some(("Done", false)));
        assert_eq!(status_chip("Failed"), Some(("Failed", true)));
        assert_eq!(status_chip("Partial"), Some(("Partly done", true)));
        assert_eq!(status_chip(""), None);
        assert_eq!(status_chip("Running"), None);
    }

    #[test]
    fn rows_say_how_long_ago() {
        assert_eq!(ago(1_000, 1_000), "now");
        assert_eq!(ago(1_000, 2_000), "now", "a clock that went back is now");
        assert_eq!(ago(10_000, 10_000 - 300), "5m");
        assert_eq!(ago(100_000, 100_000 - 3 * 3600), "3h");
        assert_eq!(ago(1_000_000, 1_000_000 - 2 * 86_400), "2d");
        assert_eq!(ago(1_800_000_000, 0), "1970-01-01");
    }

    #[test]
    fn titles_are_one_line_and_the_old_conversation_is_earlier() {
        assert_eq!(title("pc:1", "open\n  steam please"), "open steam please");
        assert_eq!(title(agent::DESKTOP, "anything"), "Earlier");
        assert_eq!(title("pc:1", "   "), "Chat");
        assert_eq!(title("pc:1", &"a".repeat(80)).chars().count(), 61);
    }

    #[test]
    fn a_chat_shows_its_turns_then_the_running_request_then_the_error() {
        let shown = vec![turn("open steam", "Opened Steam.", "Completed"), turn("hi", "Hello!", "")];
        let msgs = messages(&shown, Some("now close it"), None, Some(&("play lofi".to_string(), "No player.".to_string())), &HashSet::new());
        let roles: Vec<Role> = msgs.iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant, Role::User, Role::Assistant, Role::User, Role::Assistant]);
        assert_eq!(msgs[1].chip, "Done");
        assert_eq!(msgs[3].chip, "", "a turn without a task has no chip");
        assert!(msgs[5].pending && msgs[5].text.is_empty());
        assert!(msgs[7].error);
        assert!(messages(&[], None, None, None, &HashSet::new()).is_empty());
    }

    #[test]
    fn the_running_request_streams_its_text_and_steps_and_saved_steps_fold() {
        let mut done = turn("find my notes", "Found 2 files.", "Completed");
        done.tools = r#"[{"icon":"manage_search","label":"Looked for the file “notes”","status":"done"}]"#.into();
        let live = assist::LiveChat { conversation: "pc:1".into(), text: "Opening".into(), tools: r#"[{"icon":"open_in_new","label":"Opened Steam","status":"running"}]"#.into() };
        let msgs = messages(&[done.clone()], Some("open steam"), Some(&live), None, &HashSet::new());
        assert_eq!(msgs[1].tools.len(), 1);
        assert!(!msgs[1].tools_open, "a finished reply's steps start folded");
        assert_eq!((msgs[3].text.as_str(), msgs[3].tools[0].status.as_str()), ("Opening", "running"), "the running reply shows what it has so far");
        let open: HashSet<usize> = [1].into();
        assert!(messages(&[done], None, None, None, &open)[1].tools_open);
    }

    #[test]
    fn the_chat_switch_sets_auto_mode_for_its_requests() {
        let cfg = crate::conf::Config(serde_json::json!({ "buddies": { "autoMode": false, "listen": true } }));
        assert!(with_auto(&cfg, true).bool("buddies.autoMode"));
        assert!(with_auto(&cfg, true).bool("buddies.listen"), "the rest of the config is kept");
        assert!(!with_auto(&cfg, false).bool("buddies.autoMode"));
        assert!(with_auto(&crate::conf::Config(serde_json::json!({})), true).bool("buddies.autoMode"), "no buddies section yet");
    }

    #[test]
    fn tool_json_that_cannot_be_read_shows_nothing() {
        assert!(tools_from_json("").is_empty());
        assert!(tools_from_json("not json").is_empty());
        assert_eq!(tools_from_json(r#"[{"icon":"x"},{"label":"Read a"}]"#), vec![Tool { icon: "build".into(), label: "Read a".into(), status: "done".into() }]);
    }

    #[test]
    fn a_row_has_room_for_its_two_lines() {
        // title line at 8 (18 tall), time line at 28 (16 tall): both inside the row with room under them
        assert!(8.0 + 18.0 <= 28.0 && 28.0 + 16.0 + 4.0 <= ROW_H);
        assert!(ROW_GAP >= 8.0);
    }
}
