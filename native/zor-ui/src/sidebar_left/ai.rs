//! AI tab (tab-ai.js): streaming chat through the bridge `ai-chat` command (the conversation goes in as one JSON
//! line on stdin), model picker, `/key` `/model` `/prompt` `/temp` `/clear` commands, Markdown replies.
//! The history lives in %LOCALAPPDATA%\zor-ui\ai-history.json.

use super::{call, config_set, md, sync, wake, Inbox, Tab};
use crate::{
    app::Ctx,
    chat_ui::{input, models::{self, ModelMenu}, transcript::{Clicked, Message, Role, Transcript}},
    conf, gfx::{ico, txt, Align, Canvas, R},
    proc,
    ui::{self, Edit, IconKind, TextBox},
    win::{Input, Phase},
};
use serde_json::{json, Value};

const PROVIDERS: [&str; 5] = ["gemini", "anthropic", "openai", "mistral", "openrouter"];
const HELP: [(&str, &str); 5] = [
    ("/key <provider> <key>", "Save an API key (gemini, anthropic, openai, mistral, openrouter). Without a key it removes it."),
    ("/model <name>", "Switch model (or click the model chip)."),
    ("/prompt <text>", "Set the system prompt. \"/prompt\" alone resets it."),
    ("/temp <0-2>", "Set the temperature."),
    ("/clear", "Clear the conversation."),
];
const CLEAR: u32 = 202;
const VIEW: u32 = 205;

#[derive(Clone, Debug, PartialEq)]
pub struct Msg { pub role: String, pub content: String, pub model: String, pub error: bool }


enum Ev { Delta(u32, String), Err(u32, String), Exit(u32), Models(Value), Note(String) }

pub struct Ai {
    history: Vec<Msg>,
    mm: ModelMenu,
    model: String,
    temperature: f64,
    system: String,
    input: TextBox,
    stream: Option<proc::Stream>,
    sid: u32,
    inbox: Inbox<Ev>,
    tr: Transcript,
    ready: bool,
}

/// The stdin line for `ai-chat`: the last 30 finished user / assistant turns (`history` without the reply being written).
pub fn payload(model: &str, history: &[Msg], system: &str, temperature: f64) -> String {
    let turns: Vec<Value> = history.iter().filter(|m| (m.role == "user" || m.role == "assistant") && !m.error).map(|m| json!({ "role": m.role, "content": m.content })).collect();
    let skip = turns.len().saturating_sub(30);
    json!({ "model": model, "messages": turns[skip..], "system": system, "temperature": temperature }).to_string()
}

fn file() -> std::path::PathBuf { conf::data_dir().join("ai-history.json") }

fn load_history() -> Vec<Msg> {
    let v: Value = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    v.as_array().into_iter().flatten().map(|m| Msg {
        role: m["role"].as_str().unwrap_or("note").into(), content: m["content"].as_str().unwrap_or("").into(),
        model: m["model"].as_str().unwrap_or("").into(), error: m["error"].as_bool().unwrap_or(false),
    }).collect()
}


impl Ai {
    pub fn new() -> Ai {
        let mut input = TextBox::new(true);
        input.enter_submits = true;
        Ai { history: vec![], mm: ModelMenu::default(), model: String::new(), temperature: 0.5, system: String::new(), input, stream: None, sid: 0, inbox: Inbox::new(),
            tr: Transcript::new(), ready: false }
    }

    fn save(&self) {
        if sync() { return; }
        let keep = self.history.len().saturating_sub(100);
        let v: Vec<Value> = self.history[keep..].iter().map(|m| json!({ "role": m.role, "content": m.content, "model": m.model, "error": m.error })).collect();
        // through a temp file, so a crash mid-write never empties the saved history
        crate::bridge::common::save_json(&file(), &Value::from(v));
    }

    fn info(&self) -> models::Model { self.mm.info(&self.model) }

    fn note(&mut self, t: &str) {
        self.history.push(Msg { role: "note".into(), content: t.into(), model: String::new(), error: false });
        self.save();
        self.tr.stick = true;
    }

    fn busy(&self) -> bool { self.stream.is_some() }

    fn pick_model(&mut self, id: &str, cx: &Ctx) {
        self.model = id.into();
        config_set(cx, "ai.model", Value::from(id));
        self.mm.open = false;
        let n = self.info().name;
        self.note(&format!("Model: {n}"));
    }

    fn send(&mut self, text: &str, cx: &Ctx) {
        self.history.push(Msg { role: "user".into(), content: text.into(), model: String::new(), error: false });
        let convo = payload(&self.model, &self.history, &self.system, self.temperature);
        self.history.push(Msg { role: "assistant".into(), content: String::new(), model: self.info().name, error: false });
        self.tr.stick = true;
        self.sid += 1;
        let (id, ib, ib2) = (self.sid, self.inbox.clone(), self.inbox.clone());
        let cfg = conf::Config(cx.cfg.0.clone());
        self.stream = proc::stream_stdin(&cfg, "ai-chat", &[], &convo, move |line| {
            let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
            if let Some(d) = v["delta"].as_str() { ib.push(Ev::Delta(id, d.into())); wake(); }
            if let Some(e) = v["error"].as_str() { ib.push(Ev::Err(id, e.into())); wake(); }
        }, move || { ib2.push(Ev::Exit(id)); wake(); });
        if self.stream.is_none() {
            if let Some(m) = self.history.last_mut() { m.content = "Couldn't start the AI request.".into(); m.error = true; }
        }
    }

    fn command(&mut self, line: &str, cx: &Ctx) {
        let mut parts = line[1..].split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        let arg = rest.join(" ");
        match cmd {
            "clear" => { self.history.clear(); self.save(); }
            "help" => self.note(&HELP.iter().map(|h| format!("{} \u{2014} {}", h.0, h.1)).collect::<Vec<_>>().join("\n")),
            "key" => {
                let prov = rest.first().copied().unwrap_or("");
                if !PROVIDERS.contains(&prov) && prov != "wallhaven" { return self.note(&format!("Usage: /key <{}> <key>", PROVIDERS.join("|"))); }
                let key = rest[1..].join("");
                let (ib, p, set) = (self.inbox.clone(), prov.to_string(), !key.is_empty());
                call(cx, "secret-set", vec![prov.into(), key], move |v| {
                    ib.push(Ev::Note(match v["error"].as_str() {
                        Some(e) => format!("Couldn't save key: {e}"),
                        None if set => format!("Saved the {p} key (encrypted for your Windows account)."),
                        None => format!("Removed the {p} key."),
                    }));
                });
                self.mm.keys[prov] = Value::Bool(set);
            }
            "model" => {
                let l = arg.to_lowercase();
                match self.mm.models.iter().find(|m| m.id == arg).or_else(|| self.mm.models.iter().find(|m| m.name.to_lowercase().contains(&l))).cloned() {
                    Some(m) => self.pick_model(&m.id, cx),
                    None => { let ids = self.mm.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", "); self.note(&format!("No model matching \"{arg}\". Available: {ids}")) }
                }
            }
            "prompt" => { self.system = arg.clone(); config_set(cx, "ai.systemPrompt", Value::from(arg.clone())); self.note(if arg.is_empty() { "System prompt reset." } else { "System prompt set." }); }
            "temp" => match arg.parse::<f64>() {
                Ok(t) if (0.0..=2.0).contains(&t) => { self.temperature = t; config_set(cx, "ai.temperature", json!(t)); self.note(&format!("Temperature set to {t}.")); }
                _ => self.note("Temperature must be between 0 and 2."),
            },
            _ => self.note(&format!("Unknown command /{cmd}. Type /help.")),
        }
    }

    fn submit(&mut self, cx: &Ctx) {
        if self.busy() { self.stream = None; return; } // stop: killing the child ends the stream (Exit finishes the reply)
        let t = self.input.text.trim().to_string();
        if t.is_empty() { return; }
        self.input.set("");
        if t.starts_with('/') { self.command(&t, cx) } else { self.send(&t, cx) }
    }

    fn drain(&mut self) {
        for ev in self.inbox.take() {
            match ev {
                Ev::Delta(id, d) if id == self.sid => if let Some(m) = self.history.last_mut() { m.content += &d; },
                Ev::Err(id, e) if id == self.sid => if let Some(m) = self.history.last_mut() { if !m.content.is_empty() { m.content += "\n\n"; } m.content += &e; m.error = true; },
                Ev::Exit(id) if id == self.sid => {
                    self.stream = None;
                    if let Some(m) = self.history.last_mut() { if m.content.is_empty() { m.content = "No response.".into(); m.error = true; } }
                    self.save();
                }
                Ev::Models(v) => {
                    self.mm.set(&v);
                }
                Ev::Note(t) => self.note(&t),
                _ => {}
            }
        }
    }
}

/// The history as the shared list draws it. A reply that is still empty shows the busy dots while the stream runs.
fn messages(history: &[Msg], busy: bool) -> Vec<Message> {
    history.iter().map(|m| Message {
        role: match m.role.as_str() { "user" => Role::User, "note" => Role::Note, _ => Role::Assistant },
        text: m.content.clone(), who: m.model.clone(), error: m.error, pending: busy, ..Message::default()
    }).collect()
}

impl Tab for Ai {
    // VIEW is the message list and MENU the model menu panel: neither takes a click
    fn hand(&self, id: u32) -> bool { !matches!(id, 0 | VIEW) && !ModelMenu::is_background(id) }

    fn show(&mut self, cx: &Ctx) {
        if self.ready { return; }
        self.ready = true;
        self.model = cx.cfg.str("ai.model").into();
        self.system = cx.cfg.str("ai.systemPrompt").into();
        self.temperature = cx.cfg.get("ai.temperature").as_f64().unwrap_or(0.5);
        self.history = load_history();
        if sync() && std::env::var_os("ZORUI_RENDER_DEMO").is_some() && self.history.is_empty() {
            let m = |role: &str, content: &str, error: bool| Msg { role: role.into(), content: content.into(), model: "Gemini 2.5 Flash".into(), error };
            self.history = vec![m("user", "How do I read a file in Rust?", false), m("assistant", "# Reading files\nUse **std::fs** for small files:\n\n- `read_to_string` gives a `String`\n- `read` gives bytes\n1. open it\n2. read it\n\n```rust\nlet text = std::fs::read_to_string(\"a.txt\")?;\nprintln!(\"{text}\");\n```\n> Handle the `Result`, see [the docs](https://doc.rust-lang.org/std/fs/).", false),
                m("note", "Model: Gemini 2.5 Flash", false), m("assistant", "No API key for gemini. Type /key gemini YOUR_KEY", true)];
        }
        let ib = self.inbox.clone();
        call(cx, "ai-models", vec![], move |v| ib.push(Ev::Models(v)));
    }

    fn frame_ms(&self) -> u32 { if self.busy() { 100 } else { 500 } }
    fn captures_esc(&self) -> bool { self.mm.open }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx, r: R) {
        self.drain();
        let t = &cx.theme;
        // header: model chip + clear
        let name = self.info().name;
        ModelMenu::chip(c, cx, r.x, r.y + 2.0, r.w * 0.75, &name);
        ui::icon_btn(c, cx, R::new(r.x + r.w - 36.0, r.y, 36.0, 36.0), CLEAR, "delete_sweep", IconKind::Plain, 22.0, false);

        // input bar
        let busy = self.busy();
        let bar = input::paint(c, cx, r, &mut self.input, "Message the model\u{2026}  (/ for commands)", !self.mm.open, busy, &[]);

        // messages
        let view = R::new(r.x, r.y + 46.0, r.w, (bar.y - 10.0 - (r.y + 46.0)).max(40.0));
        c.hit(view, VIEW);
        let n = self.history.len();
        let cw = view.w - crate::tokens::BAR_ROOM; // content width: the scrollbar keeps the right BAR_ROOM
        if n == 0 {
            let mut sink = md::Sink::default();
            let mut sc = std::mem::take(&mut self.tr.scroll);
            let mut ops = vec![];
            let mut links = vec![];
            let model = self.info().name;
            let spans = [md::Span::Text(format!("Chat with {model}. Type ")), md::Span::Code("/key gemini YOUR_KEY".into()), md::Span::Text(" to add a key, or run Ollama for local models.".into())];
            let (ph, _) = md::flow(c.g, &spans, 0.0, 0.0, cw - 28.0, 13.5, 400, md::Col::Muted, true, &mut ops, &mut links);
            let desc_h: Vec<f32> = HELP.iter().map(|h| c.g.measure_wrapped(h.1, txt(12.0, 400), cw - 20.0)).collect();
            let content = 48.0 + 6.0 + 24.0 + 8.0 + ph + 12.0 + desc_h.iter().map(|h| 8.0 + 17.0 + 2.0 + h + 8.0 + 6.0).sum::<f32>();
            self.tr.dims = (content, view.h);
            sc.region(c, cx, view, content, |c, top| {
                let mut y = top.max(view.y + (view.h - content).max(0.0) / 2.0 * 0.0) + (view.h - content).max(0.0) / 2.0;
                c.icon("neurology", R::new(view.x, y, cw, 48.0), ico(48.0, false), t.c("primary"));
                y += 54.0;
                c.text("Large language models", R::new(view.x, y, cw, 24.0), txt(17.0, 500), t.c("onSurface"), Align::Center);
                y += 32.0;
                let laid = md::Laid { w: cw, h: ph, ops, links };
                md::draw(c, cx, &laid, view.x + 14.0, y, &mut sink, (view.y, view.y + view.h), None);
                y += ph + 12.0;
                for (h, dh) in HELP.iter().zip(&desc_h) {
                    let card = R::new(view.x, y, cw, 8.0 + 17.0 + 2.0 + dh + 8.0);
                    ui::card(c, cx, card, 12.0);
                    c.text(h.0, R::new(card.x + 10.0, card.y + 8.0, card.w - 20.0, 17.0), txt(12.0, 500), t.c("onSurface"), Align::Left);
                    c.para(h.1, R::new(card.x + 10.0, card.y + 27.0, card.w - 20.0, *dh), txt(12.0, 400), t.c("onSurfaceVariant"), Align::Left);
                    y += card.h + 6.0;
                }
            });
            self.tr.scroll = sc;
        } else {
            let msgs = messages(&self.history, busy);
            self.tr.paint(c, cx, view, &msgs, "neurology");
        }

        // model menu, on top
        let cur = self.model.clone();
        self.mm.paint(c, cx, R::new(r.x, r.y + 42.0, r.w, 0.0), c.h * 0.55, &cur);
    }

    fn input(&mut self, ev: &Input, cx: &mut Ctx, _hover: u32) {
        match ev {
            Input::Click(id, _) if { let (took, pick) = self.mm.click(*id); if let Some(m) = &pick { self.pick_model(m, cx); } took } => {}
            Input::Click(CLEAR, _) => { self.history.clear(); self.save(); }
            Input::Click(input::SEND, _) => self.submit(cx),
            Input::Click(id, _) if self.tr.click(*id).is_some_and(|k| { if let Clicked::Link(u) = k { call(cx, "open", vec![u], |_| {}); } true }) => {}
            Input::Drag(input::TEXT, x, y, Phase::Start) => { self.mm.open = false; self.input.press(*x, *y); }
            Input::Key { vk: 0x1B, .. } => self.mm.open = false,
            Input::Key { vk, ctrl, shift, alt } => if self.input.key(*vk, *ctrl, *shift, *alt) == Edit::Submit { self.submit(cx); },
            Input::Char(ch) => { self.input.ch(*ch); }
            Input::Wheel(_, d) if self.mm.open => self.mm.wheel(*d),
            Input::Wheel(_, d) => self.tr.wheel(*d),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(role: &str, content: &str, error: bool) -> Msg { Msg { role: role.into(), content: content.into(), model: String::new(), error } }

    #[test]
    fn chat_payload() {
        let h = vec![m("user", "hi", false), m("assistant", "boom", true), m("note", "n", false), m("assistant", "hello", false), m("user", "again", false)];
        let v: Value = serde_json::from_str(&payload("gemini-2.5-flash", &h, "be brief", 0.7)).unwrap();
        assert_eq!(v["model"], "gemini-2.5-flash");
        assert_eq!(v["system"], "be brief");
        assert_eq!(v["temperature"], 0.7);
        let roles: Vec<&str> = v["messages"].as_array().unwrap().iter().map(|x| x["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user"]); // no errors, no notes
        assert_eq!(v["messages"][1]["content"], "hello");
    }

    #[test]
    fn payload_keeps_last_30() {
        let h: Vec<Msg> = (0..45).map(|i| m(if i % 2 == 0 { "user" } else { "assistant" }, &i.to_string(), false)).collect();
        let v: Value = serde_json::from_str(&payload("x", &h, "", 0.5)).unwrap();
        assert_eq!(v["messages"].as_array().unwrap().len(), 30);
        assert_eq!(v["messages"][0]["content"], "15");
    }
}
