//! AI tab (tab-ai.js): streaming chat through the bridge `ai-chat` command (the conversation goes in as one JSON
//! line on stdin), model picker, `/key` `/model` `/prompt` `/temp` `/clear` commands, Markdown replies.
//! The history lives in %LOCALAPPDATA%\end4-pC\ai-history.json (the Zebar tab keeps its own in localStorage).

use super::{call, config_set, dots, md, sync, wake, Inbox, Tab};
use crate::{
    app::Ctx,
    conf, gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    hub, proc, util,
    ui::{self, Edit, IconKind, Scroll, TextBox},
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
const MODEL: u32 = 201;
const CLEAR: u32 = 202;
const SEND: u32 = 203;
const TEXT: u32 = 204;
const VIEW: u32 = 205;
const MENU: u32 = 206;
const MENU0: u32 = 220;

#[derive(Clone, Debug, PartialEq)]
pub struct Msg { pub role: String, pub content: String, pub model: String, pub error: bool }

#[derive(Clone)]
struct Model { id: String, name: String, provider: String }

enum Ev { Delta(u32, String), Err(u32, String), Exit(u32), Models(Value), Note(String) }

/// A message laid out for the current width.
enum Lay { Note { w: f32, h: f32 }, User { w: f32, h: f32 }, Asst { laid: md::Laid, w: f32, h: f32 } }
impl Lay { fn size(&self) -> (f32, f32) { match self { Lay::Note { w, h } | Lay::User { w, h } | Lay::Asst { w, h, .. } => (*w, *h) } }}

pub struct Ai {
    history: Vec<Msg>,
    models: Vec<Model>,
    keys: Value,
    model: String,
    temperature: f64,
    system: String,
    input: TextBox,
    stream: Option<proc::Stream>,
    sid: u32,
    inbox: Inbox<Ev>,
    scroll: Scroll,
    stick: bool,
    menu: bool,
    menu_scroll: Scroll,
    dims: (f32, f32),
    menu_dims: (f32, f32),
    lays: Vec<Option<(usize, bool, u32, Lay)>>,
    sink: md::Sink,
    copied: Option<(u32, f64)>,
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

fn capitalize(s: &str) -> String { let mut c = s.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }

impl Ai {
    pub fn new() -> Ai {
        let mut input = TextBox::new(true);
        input.enter_submits = true;
        Ai { history: vec![], models: vec![], keys: Value::Null, model: String::new(), temperature: 0.5, system: String::new(), input, stream: None, sid: 0, inbox: Inbox::new(),
            scroll: Scroll::default(), stick: true, menu: false, menu_scroll: Scroll::default(), dims: (0.0, 0.0), menu_dims: (0.0, 0.0), lays: vec![], sink: Default::default(), copied: None, ready: false }
    }

    fn save(&self) {
        if sync() { return; }
        let keep = self.history.len().saturating_sub(100);
        let v: Vec<Value> = self.history[keep..].iter().map(|m| json!({ "role": m.role, "content": m.content, "model": m.model, "error": m.error })).collect();
        let _ = std::fs::write(file(), Value::from(v).to_string());
    }

    fn info(&self) -> Model { self.models.iter().find(|m| m.id == self.model).cloned().unwrap_or(Model { id: self.model.clone(), name: self.model.clone(), provider: "?".into() }) }

    fn note(&mut self, t: &str) {
        self.history.push(Msg { role: "note".into(), content: t.into(), model: String::new(), error: false });
        self.save();
        self.stick = true;
    }

    fn busy(&self) -> bool { self.stream.is_some() }

    fn pick_model(&mut self, id: &str, cx: &Ctx) {
        self.model = id.into();
        config_set(cx, "ai.model", Value::from(id));
        self.menu = false;
        let n = self.info().name;
        self.note(&format!("Model: {n}"));
    }

    fn send(&mut self, text: &str, cx: &Ctx) {
        self.history.push(Msg { role: "user".into(), content: text.into(), model: String::new(), error: false });
        let convo = payload(&self.model, &self.history, &self.system, self.temperature);
        self.history.push(Msg { role: "assistant".into(), content: String::new(), model: self.info().name, error: false });
        self.stick = true;
        self.sid += 1;
        let (id, ib, ib2) = (self.sid, self.inbox.clone(), self.inbox.clone());
        let cfg = conf::Config(cx.cfg.0.clone());
        self.stream = proc::stream_stdin(&cfg, "ai-chat", &[], &convo, move |line| {
            let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
            if let Some(d) = v["delta"].as_str() { ib.push(Ev::Delta(id, d.into())); wake(); }
            if let Some(e) = v["error"].as_str() { ib.push(Ev::Err(id, e.into())); wake(); }
        }, move || { ib2.push(Ev::Exit(id)); wake(); });
        if self.stream.is_none() {
            if let Some(m) = self.history.last_mut() { m.content = "Couldn't start the bridge (check pythonPath / bridgePath in config.json).".into(); m.error = true; }
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
                self.keys[prov] = Value::Bool(set);
            }
            "model" => {
                let l = arg.to_lowercase();
                match self.models.iter().find(|m| m.id == arg).or_else(|| self.models.iter().find(|m| m.name.to_lowercase().contains(&l))).cloned() {
                    Some(m) => self.pick_model(&m.id, cx),
                    None => { let ids = self.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", "); self.note(&format!("No model matching \"{arg}\". Available: {ids}")) }
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
                    self.models = v["models"].as_array().into_iter().flatten().map(|m| Model { id: m["id"].as_str().unwrap_or("").into(), name: m["name"].as_str().unwrap_or("").into(), provider: m["provider"].as_str().unwrap_or("").into() }).collect();
                    self.keys = v["keys"].clone();
                }
                Ev::Note(t) => self.note(&t),
                _ => {}
            }
        }
    }

    fn lay(&mut self, g: &Gfx, i: usize, vw: f32) {
        let maxw = ((vw - 4.0) * 0.92).floor();
        let inner = maxw - 28.0;
        let m = &self.history[i];
        let tag = (m.content.len(), m.error, maxw as u32);
        if self.lays.len() < self.history.len() { self.lays.resize_with(self.history.len(), || None); }
        if self.lays[i].as_ref().is_some_and(|l| (l.0, l.1, l.2) == tag) { return; }
        let l = match m.role.as_str() {
            "note" => { let w = (g.measure(&m.content, txt(11.5, 400)).0.ceil() + 26.0).min(vw - 4.0); Lay::Note { w, h: g.measure_wrapped(&m.content, txt(11.5, 400), w - 24.0) + 10.0 } }
            "user" => { let tw = (g.measure(&m.content, txt(13.5, 400)).0.ceil() + 2.0).min(inner); Lay::User { w: tw + 28.0, h: g.measure_wrapped(&m.content, txt(13.5, 400), tw) + 20.0 } }
            _ => {
                let laid = md::layout(g, &m.content, inner);
                let who = g.measure(&m.model, txt(11.0, 400)).0 + 18.0;
                let w = (laid.w.ceil().max(who) + 28.0).max(60.0);
                let h = 20.0 + 18.0 + laid.h.max(if m.content.is_empty() { 20.0 } else { 0.0 });
                Lay::Asst { laid, w, h }
            }
        };
        self.lays[i] = Some((tag.0, tag.1, tag.2, l));
    }
}

/// A bubble of `col`; the corner next to the speaker is squarer (a patch of radius 6, opaque colours only).
fn bubble(c: &mut Canvas, r: R, col: u32, tail_right: bool) {
    c.rect(r, 23.0, col);
    let patch = if tail_right { R::new(r.x + r.w - 23.0, r.y + r.h - 23.0, 23.0, 23.0) } else { R::new(r.x, r.y + r.h - 23.0, 23.0, 23.0) };
    c.rect(patch, 6.0, col);
}

impl Tab for Ai {
    fn show(&mut self, cx: &Ctx) {
        if self.ready { return; }
        self.ready = true;
        self.model = cx.cfg.str("ai.model").into();
        self.system = cx.cfg.str("ai.systemPrompt").into();
        self.temperature = cx.cfg.get("ai.temperature").as_f64().unwrap_or(0.5);
        self.history = load_history();
        if sync() && std::env::var_os("END4_RENDER_DEMO").is_some() && self.history.is_empty() {
            let m = |role: &str, content: &str, error: bool| Msg { role: role.into(), content: content.into(), model: "Gemini 2.5 Flash".into(), error };
            self.history = vec![m("user", "How do I read a file in Rust?", false), m("assistant", "# Reading files\nUse **std::fs** for small files:\n\n- `read_to_string` gives a `String`\n- `read` gives bytes\n1. open it\n2. read it\n\n```rust\nlet text = std::fs::read_to_string(\"a.txt\")?;\nprintln!(\"{text}\");\n```\n> Handle the `Result`, see [the docs](https://doc.rust-lang.org/std/fs/).", false),
                m("note", "Model: Gemini 2.5 Flash", false), m("assistant", "No API key for gemini. Type /key gemini YOUR_KEY", true)];
        }
        let ib = self.inbox.clone();
        call(cx, "ai-models", vec![], move |v| ib.push(Ev::Models(v)));
    }

    fn frame_ms(&self) -> u32 { if self.busy() { 100 } else { 500 } }
    fn captures_esc(&self) -> bool { self.menu }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx, r: R) {
        self.drain();
        let t = &cx.theme;
        // header: model chip + clear
        let name = self.info().name;
        let nw = c.g.measure(&name, txt(13.0, 400)).0.ceil();
        let cw = (14.0 + 18.0 + 6.0 + nw + 6.0 + 18.0 + 14.0).min(r.w * 0.75);
        let chip = R::new(r.x, r.y + 2.0, cw, 32.0);
        c.rect(chip, 16.0, t.c("secondaryContainer"));
        if c.hit(chip, MODEL) { c.rect(chip, 16.0, alpha(t.c("onSecondaryContainer"), 0.08)); }
        let fg = t.c("onSecondaryContainer");
        c.icon("neurology", R::new(chip.x + 14.0, chip.y, 18.0, 32.0), ico(18.0, false), fg);
        c.text(&name, R::new(chip.x + 38.0, chip.y, cw - 38.0 - 38.0, 32.0), txt(13.0, 400), fg, Align::Left);
        c.icon("expand_more", R::new(chip.x + cw - 32.0, chip.y, 18.0, 32.0), ico(18.0, false), fg);
        ui::icon_btn(c, cx, R::new(r.x + r.w - 36.0, r.y, 36.0, 36.0), CLEAR, "delete_sweep", IconKind::Plain, 22.0, false);

        // input bar
        let tw = r.w - 16.0 - 6.0 - 36.0 - 6.0;
        let th = c.g.measure_wrapped(if self.input.text.is_empty() { "x" } else { &self.input.text }, txt(13.5, 400), tw).clamp(17.0, 160.0);
        let bar_h = (th + 18.0).max(36.0) + 12.0;
        let bar = R::new(r.x, r.y + r.h - bar_h, r.w, bar_h);
        c.rect(bar, 30.0, t.c("surfaceContainer"));
        c.hit(bar, TEXT);
        self.input.draw(c, cx, R::new(bar.x + 16.0, bar.y + 6.0 + 9.0, tw, bar_h - 12.0 - 18.0), txt(13.5, 400), "Message the model\u{2026}  (/ for commands)", !self.menu);
        let sb = R::new(bar.x + bar.w - 6.0 - 36.0, bar.y + bar.h - 6.0 - 36.0, 36.0, 36.0);
        if self.busy() {
            c.rect(sb, 18.0, t.c("error"));
            if c.hit(sb, SEND) { c.rect(sb, 18.0, alpha(t.c("onError"), 0.1)); }
            c.icon("stop", sb, ico(24.0, false), t.c("onError"));
        } else { ui::icon_btn(c, cx, sb, SEND, "arrow_upward", IconKind::Filled, 24.0, false); }

        // messages
        let view = R::new(r.x, r.y + 46.0, r.w, (bar.y - 10.0 - (r.y + 46.0)).max(40.0));
        c.hit(view, VIEW);
        let n = self.history.len();
        for i in 0..n { self.lay(c.g, i, view.w); }
        let mut sink = md::Sink::default();
        let copied = self.copied.filter(|x| hub::now_ms() - x.1 < 1200.0).map(|x| x.0);
        let busy = self.busy();
        let mut sc = std::mem::take(&mut self.scroll);
        if self.stick { sc.y = f32::MAX; }
        if n == 0 {
            let mut ops = vec![];
            let mut links = vec![];
            let model = self.info().name;
            let spans = [md::Span::Text(format!("Chat with {model}. Type ")), md::Span::Code("/key gemini YOUR_KEY".into()), md::Span::Text(" to add a key, or run Ollama for local models.".into())];
            let (ph, _) = md::flow(c.g, &spans, 0.0, 0.0, view.w - 28.0, 13.5, 400, md::Col::Muted, true, &mut ops, &mut links);
            let desc_h: Vec<f32> = HELP.iter().map(|h| c.g.measure_wrapped(h.1, txt(12.0, 400), view.w - 20.0)).collect();
            let content = 48.0 + 6.0 + 24.0 + 8.0 + ph + 12.0 + desc_h.iter().map(|h| 8.0 + 17.0 + 2.0 + h + 8.0 + 6.0).sum::<f32>();
            self.dims = (content, view.h);
            sc.region(c, cx, view, content, |c, top| {
                let mut y = top.max(view.y + (view.h - content).max(0.0) / 2.0 * 0.0) + (view.h - content).max(0.0) / 2.0;
                c.icon("neurology", R::new(view.x, y, view.w, 48.0), ico(48.0, false), t.c("primary"));
                y += 54.0;
                c.text("Large language models", R::new(view.x, y, view.w, 24.0), txt(17.0, 500), t.c("onSurface"), Align::Center);
                y += 32.0;
                let laid = md::Laid { w: view.w, h: ph, ops, links };
                md::draw(c, cx, &laid, view.x + 14.0, y, &mut sink, (view.y, view.y + view.h), None);
                y += ph + 12.0;
                for (h, dh) in HELP.iter().zip(&desc_h) {
                    let card = R::new(view.x, y, view.w, 8.0 + 17.0 + 2.0 + dh + 8.0);
                    ui::card(c, cx, card, 12.0);
                    c.text(h.0, R::new(card.x + 10.0, card.y + 8.0, card.w - 20.0, 17.0), txt(12.0, 500), t.c("onSurface"), Align::Left);
                    c.para(h.1, R::new(card.x + 10.0, card.y + 27.0, card.w - 20.0, *dh), txt(12.0, 400), t.c("onSurfaceVariant"), Align::Left);
                    y += card.h + 6.0;
                }
            });
        } else {
            let sizes: Vec<(f32, f32)> = self.lays.iter().take(n).map(|l| l.as_ref().map_or((0.0, 0.0), |l| l.3.size())).collect();
            let content = sizes.iter().map(|s| s.1 + 10.0).sum::<f32>() + 4.0 - 10.0;
            self.dims = (content, view.h);
            let (hist, lays) = (&self.history, &self.lays);
            sc.region(c, cx, view, content, |c, top| {
                let mut y = top + 2.0;
                for (i, (w, h)) in sizes.iter().enumerate() {
                    let Some((_, _, _, lay)) = &lays[i] else { continue };
                    let m = &hist[i];
                    if y + h >= view.y && y <= view.y + view.h {
                        match lay {
                            Lay::Note { .. } => {
                                let b = R::new(view.x + (view.w - w) / 2.0, y, *w, *h);
                                c.rect(b, h / 2.0, t.c("surfaceContainerLow"));
                                c.para(&m.content, R::new(b.x + 12.0, b.y + 5.0, b.w - 24.0, b.h - 10.0), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Center);
                            }
                            Lay::User { .. } => {
                                let b = R::new(view.x + view.w - 2.0 - w, y, *w, *h);
                                bubble(c, b, t.c("secondaryContainer"), true);
                                c.para(&m.content, R::new(b.x + 14.0, b.y + 10.0, b.w - 28.0, b.h - 20.0), txt(13.5, 400), t.c("onSecondaryContainer"), Align::Left);
                            }
                            Lay::Asst { laid, .. } => {
                                let b = R::new(view.x + 2.0, y, *w, *h);
                                if m.error { c.rect(b, 23.0, t.c("surfaceContainerLow")); c.rect(b, 23.0, alpha(t.c("error"), 0.16)); } else { bubble(c, b, t.c("surfaceContainerLow"), false); }
                                c.icon("neurology", R::new(b.x + 14.0, b.y + 10.0, 14.0, 14.0), ico(14.0, false), t.c("primary"));
                                c.text(&m.model, R::new(b.x + 32.0, b.y + 10.0, b.w - 46.0, 14.0), txt(11.0, 400), t.c("primary"), Align::Left);
                                if m.content.is_empty() && busy { dots(c, cx, b.x + 14.0 + 16.0, b.y + 38.0 + 10.0); }
                                md::draw(c, cx, laid, b.x + 14.0, b.y + 38.0, &mut sink, (view.y, view.y + view.h), copied);
                            }
                        }
                    } else if let Lay::Asst { laid, .. } = lay {
                        // off-screen: still number its links and code blocks so hit ids stay in step
                        md::draw(c, cx, laid, 0.0, y, &mut sink, (-1.0e9, -1.0e9), None);
                    }
                    y += h + 10.0;
                }
            });
        }
        self.scroll = sc;
        self.sink = sink;

        // model menu, on top
        if self.menu {
            let mut groups: Vec<(String, Vec<usize>)> = vec![];
            for (i, m) in self.models.iter().enumerate() {
                match groups.iter_mut().find(|g| g.0 == m.provider) { Some(g) => g.1.push(i), None => groups.push((m.provider.clone(), vec![i])) }
            }
            let content: f32 = if groups.is_empty() { 40.0 } else { groups.iter().map(|g| 28.0 + g.1.len() as f32 * 46.0).sum() };
            let mr = R::new(r.x, r.y + 42.0, r.w, (content + 12.0).min(c.h * 0.55));
            c.shadow(6.0, 24.0, 0.35, |c, col| if let Some(col) = col { c.rect(mr, 23.0, col) });
            c.rect(mr, 23.0, t.c("surfaceContainerHigh"));
            c.hit(mr, MENU);
            let inner = R::new(mr.x + 6.0, mr.y + 6.0, mr.w - 12.0, mr.h - 12.0);
            self.menu_dims = (content, inner.h);
            let (models, keys, cur) = (&self.models, &self.keys, &self.model);
            self.menu_scroll.region(c, cx, inner, content, |c, top| {
                if groups.is_empty() { c.text("Loading models\u{2026}", R::new(inner.x + 12.0, top, inner.w, 40.0), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left); }
                let mut y = top;
                for (p, idxs) in &groups {
                    let head = format!("{}", capitalize(p));
                    c.text(&head, R::new(inner.x + 12.0, y + 10.0, 200.0, 16.0), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Left);
                    if p != "ollama" {
                        let ok = keys[p.as_str()].as_bool().unwrap_or(false);
                        let hw = c.g.measure(&head, txt(11.5, 400)).0;
                        c.text(if ok { " \u{00B7} key set" } else { " \u{00B7} no key" }, R::new(inner.x + 12.0 + hw, y + 10.0, 120.0, 16.0), txt(11.5, 400), if ok { t.c("primary") } else { t.c("error") }, Align::Left);
                    }
                    y += 28.0;
                    for &i in idxs {
                        let m = &models[i];
                        let b = R::new(inner.x, y, inner.w - 6.0, 46.0);
                        if y + 46.0 >= inner.y && y <= inner.y + inner.h {
                            let sel = &m.id == cur;
                            if sel { c.rect(b, 17.0, t.c("secondaryContainer")); }
                            if c.hit(b, MENU0 + i as u32) && !sel { c.rect(b, 17.0, alpha(t.c("onSurface"), 0.08)); }
                            c.text(&m.name, R::new(b.x + 12.0, b.y + 6.0, b.w - 24.0, 18.0), txt(13.5, 400), t.c("onSurface"), Align::Left);
                            c.text(&m.id, R::new(b.x + 12.0, b.y + 25.0, b.w - 24.0, 14.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Left);
                        }
                        y += 46.0;
                    }
                }
            });
        }
    }

    fn input(&mut self, ev: &Input, cx: &mut Ctx, _hover: u32) {
        match ev {
            Input::Click(MODEL, _) => { self.menu = !self.menu; self.menu_scroll.y = 0.0; }
            Input::Click(CLEAR, _) => { self.history.clear(); self.save(); }
            Input::Click(SEND, _) => self.submit(cx),
            Input::Click(MENU, _) => {}
            Input::Click(id, _) if (MENU0..MENU0 + 400).contains(id) => {
                if let Some(m) = self.models.get((*id - MENU0) as usize).cloned() { self.pick_model(&m.id, cx); }
            }
            Input::Click(id, _) if (md::LINK0..md::LINK0 + 5000).contains(id) => {
                if let Some(u) = self.sink.links.get((*id - md::LINK0) as usize).cloned() { call(cx, "open", vec![u], |_| {}); }
            }
            Input::Click(id, _) if (md::COPY0..md::COPY0 + 5000).contains(id) => {
                if let Some(code) = self.sink.copies.get((*id - md::COPY0) as usize) { util::clipboard_set(code); self.copied = Some((*id, hub::now_ms())); }
            }
            Input::Click(..) if self.menu => self.menu = false,
            Input::Drag(TEXT, x, y, Phase::Start) => { self.menu = false; self.input.press(*x, *y); }
            Input::Key { vk: 0x1B, .. } => self.menu = false,
            Input::Key { vk, ctrl, shift, alt } => if self.input.key(*vk, *ctrl, *shift, *alt) == Edit::Submit { self.submit(cx); },
            Input::Char(ch) => { self.input.ch(*ch); }
            Input::Wheel(..) if self.menu => if let Input::Wheel(_, d) = ev { self.menu_scroll.wheel(*d, self.menu_dims.0, self.menu_dims.1) },
            Input::Wheel(_, d) => {
                self.stick = false;
                self.scroll.wheel(*d, self.dims.0, self.dims.1);
                self.stick = self.scroll.y >= self.dims.0 - self.dims.1 - 1.0;
            }
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
