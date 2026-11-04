//! AI tab (tab-ai.js): streaming chat with Gemini, Claude, OpenAI, Mistral, OpenRouter or local Ollama
//! models through the bridge's `ai-chat`, a model picker grouped by provider, the `/key /model /prompt
//! /temp /clear` commands, and a history that survives a restart (data_dir/ai.json).
//! API keys stay in the bridge (DPAPI-encrypted); the webview never saw them either.

use super::{call, config_set, dots, md, sync, wake, Inbox, Tab};
use crate::{
    app::Ctx,
    conf::Config,
    gfx::{alpha, ico, txt, Align, Canvas, Font, R},
    hub,
    proc::{self, Stream},
    settings::store::open_url,
    ui::{self, Edit, IconKind, Scroll, TextBox},
    util,
    win::{Input, Phase},
};
use serde_json::{json, Value};
use std::collections::HashMap;

const MODEL: u32 = 501;
const CLEAR: u32 = 502;
const SEND: u32 = 503;
const INPUT: u32 = 504;
const MSGS: u32 = 505;
const MENU: u32 = 506;
const ITEM0: u32 = 520;
const GAP: f32 = 10.0;
const PROVIDERS: [&str; 5] = ["gemini", "anthropic", "openai", "mistral", "openrouter"];
const HELP: [(&str, &str); 5] = [
    ("/key <provider> <key>", "Save an API key (gemini, anthropic, openai, mistral, openrouter). Without a key it removes it."),
    ("/model <name>", "Switch model (or click the model chip)."),
    ("/prompt <text>", "Set the system prompt. \"/prompt\" alone resets it."),
    ("/temp <0-2>", "Set the temperature."),
    ("/clear", "Clear the conversation."),
];

#[derive(Clone, Copy, PartialEq)]
enum Role { User, Assistant, Note }

impl Role {
    fn key(self) -> &'static str { match self { Role::User => "user", Role::Assistant => "assistant", Role::Note => "note" } }
    fn wire(self) -> &'static str { if self == Role::User { "user" } else { "assistant" } }
}

struct Msg { role: Role, content: String, model: String, error: bool }

struct Model { id: String, name: String, provider: String }

/// What the bridge's threads hand back to the tab.
enum Ev {
    Models(Value),
    Delta(u32, Value),
    End(u32),
    Key(String, bool, Value),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind { Note, User, Bot }

/// One laid-out message: where it sits in the list, how wide it is, and its Markdown (assistant only).
struct Line { kind: Kind, y: f32, w: f32, h: f32, laid: Option<md::Laid> }

pub struct Ai {
    models: Vec<Model>,
    keys: HashMap<String, bool>,
    model: String,
    temp: f64,
    system: String,
    history: Vec<Msg>,
    input: TextBox,
    busy: bool,
    stream: Option<Stream>,
    /// Bumped for every send and every stop: answers from an older stream are ignored.
    gen: u32,
    inbox: Inbox<Ev>,
    menu: bool,
    /// The model id of each row drawn in the open menu ("" for a provider header).
    menu_rows: Vec<String>,
    menu_scroll: Scroll,
    menu_dims: (f32, f32),
    scroll: Scroll,
    dims: (f32, f32),
    sink: md::Sink,
    copied: Option<u32>,
    copied_at: f64,
    /// Follow the newest message (cleared when you scroll up).
    stick: bool,
    ready: bool,
}

/// `s` shortened with an ellipsis until it measures no wider than `w`.
fn fit(c: &Canvas, s: &str, f: Font, w: f32) -> String {
    if c.g.measure(s, f).0 <= w { return s.to_string(); }
    let mut out = String::new();
    for ch in s.chars() {
        out.push(ch);
        if c.g.measure(&format!("{out}\u{2026}"), f).0 > w { out.pop(); break; }
    }
    format!("{out}\u{2026}")
}

impl Ai {
    pub fn new() -> Ai {
        Ai {
            models: vec![], keys: HashMap::new(), model: "gemini-2.5-flash".into(), temp: 0.5, system: String::new(),
            history: vec![], input: TextBox::new(true), busy: false, stream: None, gen: 0, inbox: Inbox::new(),
            menu: false, menu_rows: vec![], menu_scroll: Scroll::default(), menu_dims: (0.0, 0.0),
            scroll: Scroll::default(), dims: (0.0, 0.0), sink: md::Sink::default(), copied: None, copied_at: 0.0,
            stick: true, ready: false,
        }
    }

    /// Where the conversation is kept between runs (state.json's neighbour).
    fn path() -> std::path::PathBuf { crate::conf::data_dir().join("ai.json") }

    /// localStorage.setItem('end4-pC.ai.history'): the last 100 messages.
    fn save(&self) {
        let start = self.history.len().saturating_sub(100);
        let v: Vec<Value> = self.history[start..].iter()
            .map(|m| json!({ "role": m.role.key(), "content": m.content, "model": m.model, "error": m.error })).collect();
        let _ = std::fs::write(Self::path(), json!({ "history": v }).to_string());
    }

    fn load(&mut self) {
        let Some(v) = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return };
        self.history = v["history"].as_array().into_iter().flatten().filter_map(|m| Some(Msg {
            role: match m["role"].as_str()? { "user" => Role::User, "assistant" => Role::Assistant, _ => Role::Note },
            content: m["content"].as_str()?.to_string(),
            model: m["model"].as_str().unwrap_or("").to_string(),
            error: m["error"].as_bool().unwrap_or(false),
        })).collect();
    }

    fn model_name(&self) -> String { self.models.iter().find(|m| m.id == self.model).map_or_else(|| self.model.clone(), |m| m.name.clone()) }

    fn note(&mut self, s: String) {
        self.history.push(Msg { role: Role::Note, content: s, model: String::new(), error: false });
        self.stick = true;
        self.save();
    }

    /// bridge('ai-models'): the models the bridge offers and which providers have a key.
    fn load_models(&mut self, cx: &Ctx) {
        if sync() { return; }
        let inbox = self.inbox.clone();
        call(cx, "ai-models", vec![], move |v| inbox.push(Ev::Models(v)));
    }

    fn drain(&mut self) {
        for ev in self.inbox.take() {
            match ev {
                Ev::Models(v) => {
                    self.models = v["models"].as_array().into_iter().flatten().filter_map(|m| {
                        let id = m["id"].as_str()?.to_string();
                        let name = m["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id.as_str()).to_string();
                        Some(Model { id, name, provider: m["provider"].as_str().unwrap_or("custom").to_string() })
                    }).collect();
                    self.keys = v["keys"].as_object().into_iter().flatten().map(|(k, b)| (k.clone(), b.as_bool().unwrap_or(false))).collect();
                }
                Ev::Delta(gen, v) => {
                    if gen != self.gen || !self.busy { continue; }
                    let Some(reply) = self.history.last_mut() else { continue };
                    if let Some(d) = v["delta"].as_str() { reply.content.push_str(d); }
                    if let Some(e) = v["error"].as_str() {
                        if !reply.content.is_empty() { reply.content.push_str("\n\n"); }
                        reply.content.push_str(e);
                        reply.error = true;
                    }
                }
                Ev::End(gen) => {
                    if gen != self.gen { continue; }
                    self.busy = false;
                    self.stream = None;
                    if let Some(reply) = self.history.last_mut() {
                        if reply.content.is_empty() { reply.content = "No response.".into(); reply.error = true; }
                    }
                    self.stick = true;
                    self.save();
                }
                Ev::Key(prov, keyed, v) => {
                    self.keys.insert(prov.clone(), keyed);
                    let line = if let Some(e) = v["error"].as_str() {
                        format!("Couldn't save key: {e}")
                    } else if keyed {
                        format!("Saved the {prov} key (encrypted for your Windows account).")
                    } else {
                        format!("Removed the {prov} key.")
                    };
                    self.note(line);
                }
            }
        }
    }

    /// The last 30 real turns: no notes, no failed replies, and not the reply being written.
    fn convo(&self) -> Vec<Value> {
        let end = self.history.len().saturating_sub(1);
        let v: Vec<Value> = self.history[..end].iter().filter(|m| m.role != Role::Note && !m.error)
            .map(|m| json!({ "role": m.role.wire(), "content": m.content })).collect();
        v[v.len().saturating_sub(30)..].to_vec()
    }

    fn stop(&mut self) {
        self.gen += 1; // whatever the killed process already wrote is stale now
        self.busy = false;
        self.stream = None;
        if let Some(reply) = self.history.last_mut() {
            if reply.content.is_empty() { reply.content = "Stopped.".into(); }
        }
        self.save();
    }

    fn submit(&mut self, cx: &Ctx) {
        let text = self.input.text.trim().to_string();
        if text.is_empty() { return; }
        self.input.set("");
        self.stick = true;
        if text.starts_with('/') { return self.command(&text, cx); }
        let name = self.model_name();
        self.history.push(Msg { role: Role::User, content: text, model: String::new(), error: false });
        self.history.push(Msg { role: Role::Assistant, content: String::new(), model: name, error: false });
        self.gen += 1;
        let gen = self.gen;
        let body = json!({ "model": self.model, "messages": self.convo(), "system": self.system, "temperature": self.temp }).to_string();
        self.busy = true;
        self.save();
        let (lines, done) = (self.inbox.clone(), self.inbox.clone());
        let cfg = Config(cx.cfg.0.clone());
        self.stream = proc::stream_stdin(&cfg, "ai-chat", &[], &body,
            move |line| if let Ok(v) = serde_json::from_str::<Value>(line) { lines.push(Ev::Delta(gen, v)); wake(); },
            move || { done.push(Ev::End(gen)); wake(); });
        if self.stream.is_none() {
            self.busy = false;
            self.note("The bridge didn't start: check pythonPath and bridgePath in config.json.".into());
        }
    }

    fn command(&mut self, line: &str, cx: &Ctx) {
        let mut words = line[1..].split_whitespace();
        let cmd = words.next().unwrap_or("").to_lowercase();
        let rest: Vec<&str> = words.collect();
        let arg = rest.join(" ");
        match cmd.as_str() {
            "clear" => { self.history.clear(); self.save(); }
            "help" => self.note(HELP.iter().map(|(c, d)| format!("{c} \u{2014} {d}")).collect::<Vec<_>>().join("\n")),
            "key" => {
                let prov = rest.first().copied().unwrap_or("");
                if !PROVIDERS.contains(&prov) { return self.note(format!("Usage: /key <{}> <key>", PROVIDERS.join("|"))); }
                let key = rest.iter().skip(1).copied().collect::<String>();
                let keyed = !key.is_empty();
                let (inbox, prov) = (self.inbox.clone(), prov.to_string());
                call(cx, "secret-set", vec![prov.clone(), key], move |v| inbox.push(Ev::Key(prov, keyed, v)));
            }
            "model" => {
                let q = arg.to_lowercase();
                let hit = self.models.iter().find(|m| m.id == arg)
                    .or_else(|| self.models.iter().find(|m| m.name.to_lowercase().contains(&q)))
                    .map(|m| m.id.clone());
                match hit {
                    Some(id) => self.pick(&id, cx),
                    None => {
                        let ids = self.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", ");
                        self.note(format!("No model matching \"{arg}\". Available: {ids}"));
                    }
                }
            }
            "prompt" => {
                self.system = arg.clone();
                config_set(cx, "ai.systemPrompt", json!(arg));
                self.note(if arg.is_empty() { "System prompt reset.".into() } else { "System prompt set.".into() });
            }
            "temp" => match arg.parse::<f64>() {
                Ok(t) if (0.0..=2.0).contains(&t) => {
                    self.temp = t;
                    config_set(cx, "ai.temperature", json!(t));
                    self.note(format!("Temperature set to {t}."));
                }
                _ => self.note("Temperature must be between 0 and 2.".into()),
            },
            _ => self.note(format!("Unknown command /{cmd}. Type /help.")),
        }
    }

    fn pick(&mut self, id: &str, cx: &Ctx) {
        self.model = id.to_string();
        config_set(cx, "ai.model", json!(id));
        self.menu = false;
        self.note(format!("Model: {}", self.model_name()));
    }

    fn pill(c: &mut Canvas, cx: &Ctx, head: R, label: &str) {
        let t = &cx.theme;
        let f = txt(13.0, 400);
        let lw = c.g.measure(label, f).0.ceil();
        let pill = R::new(head.x, head.y, (14.0 + 24.0 + lw + 26.0).min(head.w - 44.0), 32.0);
        c.rect(pill, 16.0, t.c("secondaryContainer"));
        if c.hit(pill, MODEL) { c.rect(pill, 16.0, alpha(t.c("onSurface"), 0.08)); }
        c.icon("neurology", R::new(pill.x + 12.0, pill.y, 18.0, 32.0), ico(18.0, false), t.c("onSecondaryContainer"));
        c.text(label, R::new(pill.x + 34.0, pill.y, (pill.w - 58.0).max(0.0), 32.0), f, t.c("onSecondaryContainer"), Align::Left);
        c.icon("expand_more", R::new(pill.x + pill.w - 24.0, pill.y, 18.0, 32.0), ico(18.0, false), t.c("onSecondaryContainer"));
    }

    fn empty_state(c: &mut Canvas, cx: &Ctx, msgs: R, model: &str) {
        let t = &cx.theme;
        let w = (msgs.w - 60.0).max(60.0);
        c.icon("neurology", R::new(msgs.x, msgs.y + 8.0, msgs.w, 56.0), ico(48.0, false), t.c("primary"));
        c.text("Large language models", R::new(msgs.x, msgs.y + 70.0, msgs.w, 24.0), txt(17.0, 500), t.c("onSurface"), Align::Center);
        let hint = format!("Chat with {model}. Type /key gemini YOUR_KEY to add a key, or run Ollama for local models.");
        let hh = c.g.measure_wrapped(&hint, txt(12.5, 400), w);
        c.para(&hint, R::new(msgs.x + 30.0, msgs.y + 96.0, w, hh + 4.0), txt(12.5, 400), t.c("onSurfaceVariant"), Align::Center);
        let mut y = msgs.y + 104.0 + hh;
        for (cmd, d) in HELP {
            if y + 30.0 > msgs.y + msgs.h { break; }
            c.text(cmd, R::new(msgs.x + 6.0, y, msgs.w - 12.0, 15.0), txt(11.5, 400), t.c("primary"), Align::Left);
            c.text(d, R::new(msgs.x + 6.0, y + 15.0, msgs.w - 12.0, 14.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Left);
            y += 32.0;
        }
    }
}

impl Tab for Ai {
    fn show(&mut self, cx: &Ctx) {
        if self.ready { return; }
        self.ready = true;
        self.model = Some(cx.cfg.str("ai.model")).filter(|s| !s.is_empty()).unwrap_or("gemini-2.5-flash").to_string();
        self.temp = cx.cfg.get("ai.temperature").as_f64().unwrap_or(0.5);
        self.system = cx.cfg.str("ai.systemPrompt").to_string();
        self.load();
        if std::env::var_os("END4_RENDER_DEMO").is_some() {
            self.models = vec![
                Model { id: "gemini-2.5-flash".into(), name: "Gemini 2.5 Flash".into(), provider: "gemini".into() },
                Model { id: "gpt-4o-mini".into(), name: "GPT-4o mini".into(), provider: "openai".into() },
            ];
            self.keys.insert("gemini".into(), true);
            if self.history.is_empty() {
                self.history.push(Msg { role: Role::User, content: "What's a good way to start a Rust project?".into(), model: String::new(), error: false });
                self.history.push(Msg { role: Role::Assistant, content: "Use **cargo**:\n\n```sh\ncargo new app\n```\n\nThen `cargo run`.".into(), model: "Gemini 2.5 Flash".into(), error: false });
            }
            return;
        }
        self.load_models(cx);
    }

    fn frame_ms(&self) -> u32 { if self.busy { 100 } else { 500 } }
    fn captures_esc(&self) -> bool { self.menu }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx, r: R) {
        self.drain();
        let t = &cx.theme;
        let f = txt(13.5, 400);
        let tw = (r.w - 68.0).max(40.0);
        let th = if self.input.text.is_empty() { 20.0 } else { c.g.measure_wrapped(&self.input.text, f, tw).max(20.0) };
        let input_h = (th + 22.0).clamp(44.0, 150.0);
        let head = R::new(r.x, r.y, r.w, 36.0);
        let input = R::new(r.x, r.y + r.h - input_h, r.w, input_h);
        let msgs = R::new(r.x, r.y + 36.0 + GAP, r.w, (input.y - GAP - r.y - 36.0 - GAP).max(60.0));

        let label = fit(c, &self.model_name(), f, (r.w - 130.0).max(40.0));
        Self::pill(c, cx, head, &label);
        ui::icon_btn(c, cx, R::new(head.x + head.w - 36.0, head.y - 2.0, 36.0, 36.0), CLEAR, "delete_sweep", IconKind::Plain, 20.0, false);

        // the messages, laid out top to bottom
        let bw = msgs.w * 0.92;
        let mut lines: Vec<Line> = Vec::new();
        let mut y = 0.0f32;
        for m in &self.history {
            match m.role {
                Role::Note => {
                    let h = c.g.measure_wrapped(&m.content, txt(11.5, 400), (bw - 40.0).max(40.0)) + 10.0;
                    lines.push(Line { kind: Kind::Note, y, w: bw, h, laid: None });
                    y += h + GAP;
                }
                Role::User => {
                    let w = (c.g.measure(&m.content, f).0 + 30.0).min(bw);
                    let h = c.g.measure_wrapped(&m.content, f, (w - 30.0).max(20.0)) + 22.0;
                    lines.push(Line { kind: Kind::User, y, w, h, laid: None });
                    y += h + GAP;
                }
                Role::Assistant => {
                    let laid = md::layout(&c.g, &m.content, (bw - 30.0).max(60.0));
                    let h = laid.h + 46.0;
                    lines.push(Line { kind: Kind::Bot, y, w: bw, h, laid: Some(laid) });
                    y += h + GAP;
                }
            }
        }
        let content = y.max(1.0);
        self.dims = (content, msgs.h);
        if self.stick { self.scroll.y = (content - msgs.h).max(0.0); }

        if self.history.is_empty() {
            Self::empty_state(c, cx, msgs, &self.model_name());
        } else {
            let mut sink = std::mem::take(&mut self.sink);
            let copied = self.copied.filter(|_| hub::now_ms() - self.copied_at < 1200.0);
            let (view, busy) = ((msgs.y, msgs.y + msgs.h), self.busy);
            {
                let Ai { scroll, history, .. } = self;
                scroll.region(c, cx, msgs, content, |c, top| {
                    for (i, it) in lines.iter().enumerate() {
                        if top + it.y + it.h < view.0 || top + it.y > view.1 { continue; }
                        let m = &history[i];
                        match it.kind {
                            Kind::Note => {
                                let w = (it.w - 8.0).min(c.g.measure(&m.content, txt(11.5, 400)).0 + 26.0);
                                let rr = R::new(msgs.x + (msgs.w - w) / 2.0, top + it.y, w, it.h);
                                c.rect(rr, it.h / 2.0, t.c("surfaceContainerLow"));
                                c.para(&m.content, R::new(rr.x + 13.0, rr.y + 5.0, rr.w - 26.0, rr.h - 10.0), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Center);
                            }
                            Kind::User => {
                                let rr = R::new(msgs.x + msgs.w - it.w, top + it.y, it.w, it.h);
                                c.rect(rr, 17.0, t.c("secondaryContainer"));
                                c.para(&m.content, R::new(rr.x + 15.0, rr.y + 11.0, rr.w - 30.0, rr.h - 22.0), f, t.c("onSecondaryContainer"), Align::Left);
                            }
                            Kind::Bot => {
                                let rr = R::new(msgs.x, top + it.y, it.w, it.h);
                                c.rect(rr, 17.0, if m.error { alpha(t.c("error"), 0.16) } else { t.c("surfaceContainerLow") });
                                c.icon("neurology", R::new(rr.x + 15.0, rr.y + 12.0, 14.0, 14.0), ico(14.0, false), t.c("primary"));
                                c.text(&m.model, R::new(rr.x + 33.0, rr.y + 10.0, rr.w - 48.0, 16.0), txt(11.0, 400), t.c("primary"), Align::Left);
                                if m.content.is_empty() {
                                    if busy { dots(c, cx, rr.x + rr.w / 2.0, rr.y + rr.h / 2.0 + 8.0); }
                                } else if let Some(l) = &it.laid {
                                    md::draw(c, cx, l, rr.x + 15.0, rr.y + 34.0, &mut sink, view, copied);
                                }
                            }
                        }
                    }
                });
            }
            self.sink = sink;
        }

        // the model menu, over the messages
        if self.menu {
            let mr = R::new(msgs.x, msgs.y, msgs.w, msgs.h);
            c.shadow(4.0, 16.0, 0.35, |c, col| if let Some(col) = col { c.rect(mr, 17.0, col) });
            c.rect(mr, 17.0, t.c("surfaceContainerHigh"));
            c.hit(mr, MENU);
            let inner = R::new(mr.x + 6.0, mr.y + 6.0, mr.w - 12.0, mr.h - 12.0);
            let mut rows: Vec<(bool, String, String, String)> = Vec::new();
            let mut providers: Vec<&str> = Vec::new();
            for m in &self.models { if !providers.contains(&m.provider.as_str()) { providers.push(&m.provider); } }
            for p in providers {
                let status = if p == "ollama" { String::new() }
                    else if self.keys.get(p).copied().unwrap_or(false) { " \u{00b7} key set".into() }
                    else { " \u{00b7} no key".into() };
                rows.push((true, format!("{p}{status}"), String::new(), String::new()));
                for m in self.models.iter().filter(|m| m.provider == p) {
                    rows.push((false, m.name.clone(), m.id.clone(), m.id.clone()));
                }
            }
            let total: f32 = rows.iter().map(|r| if r.0 { 28.0 } else { 44.0 }).sum();
            self.menu_dims = (total, inner.h);
            if rows.is_empty() {
                c.text("Loading models\u{2026}", R::new(inner.x + 8.0, inner.y, inner.w, 40.0), txt(12.0, 400), t.c("onSurfaceVariant"), Align::Left);
            }
            let cur = self.model.clone();
            let menu_scroll = &mut self.menu_scroll;
            menu_scroll.region(c, cx, inner, total.max(1.0), |c, top| {
                let mut y = top;
                for (i, (header, text, sub, id)) in rows.iter().enumerate() {
                    let h = if *header { 28.0 } else { 44.0 };
                    if y + h < inner.y || y > inner.y + inner.h { y += h; continue; }
                    let rr = R::new(inner.x, y, inner.w - 6.0, h);
                    if *header {
                        c.text(text, R::new(rr.x + 8.0, rr.y, rr.w - 12.0, h), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Left);
                    } else {
                        if *id == cur { c.rect(rr, 12.0, t.c("secondaryContainer")); }
                        if c.hit(rr, ITEM0 + i as u32) && *id != cur { c.rect(rr, 12.0, alpha(t.c("onSurface"), 0.08)); }
                        c.text(text, R::new(rr.x + 12.0, rr.y + 6.0, rr.w - 24.0, 18.0), txt(13.0, 400), t.c("onSurface"), Align::Left);
                        c.text(sub, R::new(rr.x + 12.0, rr.y + 23.0, rr.w - 24.0, 16.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Left);
                    }
                    y += h;
                }
            });
            self.menu_rows = rows.iter().map(|r| r.3.clone()).collect();
        }

        // the composer
        c.rect(input, 22.0, t.c("surfaceContainer"));
        c.hit(input, INPUT);
        self.input.draw(c, cx, R::new(input.x + 16.0, input.y + 11.0, tw, input_h - 22.0), f, "Message the model\u{2026}  (/ for commands)", !self.menu);
        ui::icon_btn(c, cx, R::new(input.x + input.w - 44.0, input.y + (input_h - 36.0) / 2.0, 36.0, 36.0), SEND,
            if self.busy { "stop" } else { "arrow_upward" }, IconKind::Filled, 20.0, false);
    }

    fn input(&mut self, ev: &Input, cx: &mut Ctx, _hover: u32) {
        let mut submit = false;
        match ev {
            Input::Click(MODEL, _) => {
                self.menu = !self.menu;
                self.menu_scroll.y = 0.0;
                if self.menu && self.models.is_empty() { self.load_models(cx); }
            }
            Input::Click(CLEAR, _) => { self.history.clear(); self.save(); }
            Input::Click(SEND, _) => if self.busy { self.stop(); } else { submit = true; },
            Input::Click(id, _) if (ITEM0..ITEM0 + 64).contains(id) => {
                if let Some(m) = self.menu_rows.get((*id - ITEM0) as usize).filter(|s| !s.is_empty()).cloned() { self.pick(&m, cx); }
            }
            Input::Click(id, _) if *id >= md::LINK0 && *id < md::COPY0 => {
                if let Some(u) = self.sink.links.get((*id - md::LINK0) as usize).cloned() { open_url(&u); }
            }
            Input::Click(id, _) if *id >= md::COPY0 && *id < md::COPY0 + 400 => {
                if let Some(s) = self.sink.copies.get((*id - md::COPY0) as usize) {
                    util::clipboard_set(s);
                    self.copied = Some(*id);
                    self.copied_at = hub::now_ms();
                }
            }
            Input::Click(MENU, _) => {}
            Input::Click(..) if self.menu => self.menu = false,
            Input::Drag(INPUT, x, y, Phase::Start) => { self.menu = false; self.input.press(*x, *y); }
            Input::Key { vk: 0x1B, .. } => self.menu = false,
            Input::Key { vk, ctrl, shift, alt } if !self.menu => {
                if self.input.key(*vk, *ctrl, *shift, *alt) == Edit::Submit { submit = true; }
            }
            Input::Char(ch) if !self.menu => { self.input.ch(*ch); },
            Input::Wheel(_, d) => {
                if self.menu {
                    self.menu_scroll.wheel(*d, self.menu_dims.0, self.menu_dims.1);
                } else {
                    self.scroll.wheel(*d, self.dims.0, self.dims.1);
                    self.stick = self.scroll.y >= (self.dims.0 - self.dims.1).max(0.0) - 1.0;
                }
            }
            _ => {}
        }
        if submit { self.submit(cx); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> Msg { Msg { role, content: content.into(), model: String::new(), error: false } }

    #[test]
    fn the_conversation_leaves_out_notes_errors_and_the_open_reply() {
        let mut a = Ai::new();
        a.history = vec![
            msg(Role::User, "hi"),
            msg(Role::Note, "Model: x"),
            msg(Role::Assistant, "hello"),
            Msg { error: true, ..msg(Role::Assistant, "boom") },
            msg(Role::User, "again"),
            msg(Role::Assistant, ""),
        ];
        let c = a.convo();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0]["role"], "user");
        assert_eq!(c[1]["content"], "hello");
        assert_eq!(c[2]["content"], "again");
    }

    #[test]
    fn only_the_last_thirty_turns_are_sent() {
        let mut a = Ai::new();
        for i in 0..40 { a.history.push(msg(if i % 2 == 0 { Role::User } else { Role::Assistant }, "x")); }
        a.history.push(msg(Role::Assistant, "")); // the reply being written
        assert_eq!(a.convo().len(), 30);
        a.history.clear();
        assert!(a.convo().is_empty());
    }
}
