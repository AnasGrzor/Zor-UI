//! The buddies you can pick in Settings > Buddies: how each one is coloured and what they say.
//! buddy.rs draws all of them with one chibi figure; a cast only changes its palette, hair and lines.

use crate::{conf, gfx::Color};

#[derive(Clone)]
pub struct Cast {
    pub id: &'static str,
    pub label: &'static str,
    pub hair: Color,
    pub hair_sh: Color,
    pub coat: Color,
    pub coat_hi: Color,
    pub pants: Color,
    pub eye: Color,
    pub eye_d: Color,
    /// A blindfold drawn over the eyes that lifts on hover.
    pub blindfold: bool,
    pub bun: bool,
    /// The move's colours: the first and second orb of `Move::Orbs` and the colour they merge into; `Move::Beam` and
    /// `Move::Aura` use the first and third.
    pub orbs: (Color, Color, Color),
    /// What a double-click and a right-click do. A right-click that does nothing says a line from `right` instead.
    pub special: Move,
    pub alt: Move,
    pub greet: &'static str,
    /// Said on a double-click.
    pub shout: &'static str,
    pub click: &'static [&'static str],
    /// Said when the pointer rests on them.
    pub peek: &'static [&'static str],
    pub chat: &'static [&'static str],
    /// Said on a right-click when there is no Infinity.
    pub right: &'static [&'static str],
    /// What they say to their partner ("{p}" is the partner's name), each tagged with its tone.
    pub to: &'static [(Tone, &'static str)],
    /// What they answer, by the tone of the line they heard (indexed by `Tone as usize`).
    pub reply: [&'static [&'static str]; 4],
    /// Everything else they say: reactions to the desktop, to being picked up, and so on.
    pub say: Say,
}

/// A move a buddy can play: Orbs (two orbs gather, meet and fire, Hollow Purple), Beam (a ball charges at the hand and
/// shoots across), Aura (rings and sparks around the body), Shield (the pointer can't get closer than 60 px for 10 s,
/// Infinity) or None.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Move { None, Orbs, Beam, Aura, Shield }

impl Move {
    /// None for an unknown name; names are lowercase: "none", "orbs", "beam", "aura", "shield".
    pub fn parse(s: &str) -> Option<Move> {
        Some(match s.trim().to_lowercase().as_str() { "none" => Move::None, "orbs" => Move::Orbs, "beam" => Move::Beam, "aura" => Move::Aura, "shield" => Move::Shield, _ => return None })
    }
}

/// What a line to the partner is: the partner's answer has to fit it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tone { Tease, Invite, Love, Talk }

/// The lines a buddy says outside clicks and chat. Single lines may be empty, which means say nothing.
/// Templates: `{pct}` is a percentage, `{app}` an app's name, `{track}` a track's title.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Say {
    /// Clicked while asleep.
    pub wake_click: &'static str,
    /// Picked up.
    pub held: &'static [&'static str],
    /// Dropped below the bar, and landing on a ledge.
    pub teleport: &'static str,
    pub landed: &'static str,
    /// You came back from being away.
    pub back: &'static str,
    /// A move of `Move::Shield` turns the shield (Infinity) on or off.
    pub shield_on: &'static str,
    pub shield_off: &'static str,
    /// Chat after midnight.
    pub late: &'static str,
    /// Tips after "N minutes straight. Break time!".
    pub break_tips: &'static [&'static str],
    pub game_start: &'static [&'static str],
    pub game_end: &'static str,
    /// The app in front: an editor, a video, social media, a chat, a game.
    pub focus_code: &'static [&'static str],
    pub focus_video: &'static [&'static str],
    pub focus_social: &'static [&'static str],
    pub focus_chat: &'static [&'static str],
    pub focus_game: &'static [&'static str],
    // reactions to the desktop
    pub low_battery: &'static str,
    pub claude_high: &'static str,
    pub claude_near: &'static str,
    pub note: &'static str,
    pub track: &'static str,
    pub music_on: &'static str,
    pub music_off: &'static str,
    pub muted: &'static str,
    pub unmuted: &'static str,
    pub loud: &'static str,
    pub quiet: &'static str,
    pub online: &'static str,
    pub offline: &'static str,
    pub charging: &'static str,
    pub unplugged: &'static str,
    pub thunder: &'static str,
    pub snow: &'static str,
    pub rain: &'static str,
    pub busy: &'static str,
}

/// Gojo's own words, and what every built-in buddy says.
pub const GOJO_SAY: Say = Say {
    wake_click: "Five more minutes...",
    held: &["Hey! Put me down!", "Whoa, whoa!", "Careful, I'm precious."],
    teleport: "Teleport~",
    landed: "Perfect landing.",
    back: "Hm? Oh, you're back.",
    shield_on: "Infinity. You can't touch me~",
    shield_off: "Fine, you can touch me.",
    late: "It's late. Even I sleep sometimes.",
    break_tips: &["Stand up, stretch, drink some water.", "Look away from the screen for a minute.", "Take a walk. The code will wait."],
    game_start: &["A game! I'll watch from here. Go win.", "Game time. I'm on the other screen, cheering."],
    game_end: "Good game? Welcome back.",
    focus_code: &["Coding? Don't let a bug beat you.", "Writing code again. I'll be here if it breaks."],
    focus_video: &["Watching something? Don't forget to blink.", "Is it any good? Move over."],
    focus_social: &["Scrolling again? Just a little, okay?", "Social media time. Don't lose an hour."],
    focus_chat: &["Chatting with someone? Say hi from me.", "Busy talking, I see."],
    focus_game: &["A game! Show them what you've got.", "Game time. Go win."],
    low_battery: "Battery's low. Find a plug, would you?",
    claude_high: "Claude's at {pct}%. Even I can't beat a rate limit.",
    claude_near: "Claude's at {pct}% of the limit. Pace yourself.",
    note: "Word from {app}.",
    track: "{track}, huh? Good taste.",
    music_on: "Music's on. Turn it up~",
    music_off: "Aw, the music stopped.",
    muted: "Muted? I can't hear a thing.",
    unmuted: "Sound's back.",
    loud: "That's loud. I approve.",
    quiet: "Too quiet in here.",
    online: "We're back online.",
    offline: "The internet's gone. Not my doing.",
    charging: "Charging. Good.",
    unplugged: "On battery? Better be quick.",
    thunder: "Thunder outside? Even I'd stay in.",
    snow: "It's snowing. Cute.",
    rain: "It's raining out there. Stay in.",
    busy: "Whatever's running is working hard.",
};

/// Plain words with no character in them: what a pack says for anything it doesn't write itself.
pub const NEUTRAL_SAY: Say = Say {
    wake_click: "Okay, I'm up.",
    held: &["Hey! Put me down!", "Whoa!", "Careful!"],
    teleport: "Back up I go.",
    landed: "Landed.",
    back: "Oh, you're back.",
    shield_on: "Shield up.",
    shield_off: "Shield down.",
    late: "It's late. Time to rest?",
    break_tips: &["Stand up and stretch.", "Drink some water.", "Look away from the screen for a minute."],
    game_start: &["A game! I'll watch from the other screen.", "Game time. Good luck."],
    game_end: "Good game? Welcome back.",
    focus_code: &["Coding? Good luck.", "Writing code again."],
    focus_video: &["Watching something?", "Enjoy the show."],
    focus_social: &["Scrolling for a while?", "Don't lose an hour."],
    focus_chat: &["Chatting with someone?", "Busy talking, I see."],
    focus_game: &["A game! Have fun.", "Game time."],
    low_battery: "The battery is low. Find a plug.",
    claude_high: "Claude is at {pct}%. Almost at the limit.",
    claude_near: "Claude is at {pct}% of the limit.",
    note: "A message from {app}.",
    track: "{track}. Nice.",
    music_on: "Music is on.",
    music_off: "The music stopped.",
    muted: "Muted.",
    unmuted: "Sound is back.",
    loud: "That's loud.",
    quiet: "That's quiet.",
    online: "Back online.",
    offline: "The internet is gone.",
    charging: "Charging.",
    unplugged: "Running on battery.",
    thunder: "Thunder outside.",
    snow: "It's snowing.",
    rain: "It's raining outside.",
    busy: "Something is working hard.",
};

/// A character with no voice of its own: the base of a pack that names no `base`.
const NEUTRAL: Cast = Cast {
    id: "neutral", label: "Buddy",
    hair: 0xFF50505C, hair_sh: 0xFF383842, coat: 0xFF3A3F55, coat_hi: 0xFF5A6080, pants: 0xFF262833, eye: 0xFF9FD0FF, eye_d: 0xFF3A7ACC,
    blindfold: false, bun: false, orbs: (0xFF4FB4FF, 0xFFFF6A5A, 0xFFB080FF), special: Move::Aura, alt: Move::None,
    greet: "Hi.", shout: "Special move!",
    click: &["Hello!", "What's up?", "Need something?", "I'm here."],
    peek: &["Oh, hi.", "Looking for me?", "Hello there."],
    chat: &["Nice view from up here.", "Quiet today.", "Need help? Just click me."],
    right: &["Hey!", "Please don't.", "Not now."],
    to: &[(Tone::Tease, "Hey {p}, race you."), (Tone::Invite, "Want to sit for a while, {p}?"), (Tone::Talk, "How's it going, {p}?")],
    reply: [&["Ha. Not this time.", "You wish."], &["Sure, why not.", "Sounds good."], &["Thanks. Same to you.", "Aw, thanks."], &["Not much. You?", "Just relaxing."]],
    say: NEUTRAL_SAY,
};

const PUBLIC: [Cast; 3] = [
    Cast {
        id: "gojo", label: "Gojo",
        hair: 0xFFF5F7FC, hair_sh: 0xFFC3CDE2, coat: 0xFF1E2133, coat_hi: 0xFF323858, pants: 0xFF151724, eye: 0xFF8FE3FF, eye_d: 0xFF2A9EE6,
        blindfold: true, bun: false, orbs: (0xFF4FB4FF, 0xFFFF4A5A, 0xFFB060FF), special: Move::Orbs, alt: Move::Shield,
        greet: "Yo~", shout: "Hollow Purple.",
        click: &[
            "Relax. I'm the strongest.",
            "Nah, I'd win.",
            "Throughout heaven and earth, I alone am the honored one.",
            "You look tired. Want a kikufuku?",
            "Stop poking me. Infinity's off, you know.",
            "Working hard? Take a break~",
            "Don't worry. I'm here.",
            "Sukuna? Never heard of him.",
        ],
        peek: &["Oh? You wanted to see my eyes?", "Six Eyes: watching your tabs.", "Don't stare too long. You'll fall for me."],
        chat: &["Hmm~ I'm bored.", "Got any sweets?", "This bar's a nice place to hang out.", "Need help? Just click me."],
        right: &[],
        to: &[(Tone::Tease, "Hey {p}, bet I can beat you."), (Tone::Tease, "{p}, you're quiet. Boring~"), (Tone::Invite, "Wanna grab sweets, {p}?")],
        reply: [
            &["Don't act like I'm your sidekick.", "Tch. Obviously."],
            &["Sure. You're buying.", "Sweets? Now you're talking."],
            &["Aw, I know. Everybody does.", "Don't get sappy. ...Me too."],
            &["Thinking about sweets. Obviously.", "Me? Just being the strongest."],
        ],
        say: GOJO_SAY,
    },
    Cast {
        id: "geto", label: "Geto",
        hair: 0xFF1B1B22, hair_sh: 0xFF3A3A48, coat: 0xFF23232E, coat_hi: 0xFFE8E4F0, pants: 0xFF1A1A22, eye: 0xFFB0A0FF, eye_d: 0xFF6A50D0,
        blindfold: false, bun: true, orbs: (0xFF3A2A5A, 0xFF5BBF7A, 0xFF4A3A7A), special: Move::Orbs, alt: Move::None,
        greet: "Hello.", shout: "Maximum: Uzumaki.",
        click: &[
            "Take a break. You've earned one.",
            "I'm just keeping the peace.",
            "Tea? I'll make some.",
            "Don't mind me. I'm thinking.",
            "You look like you carry a lot.",
        ],
        peek: &["Hm? Can I help you?", "You're welcome to sit with me.", "Careful. I notice things."],
        chat: &["Quiet today.", "I could stay here all day.", "Some days I miss the old classroom."],
        right: &["Please don't.", "I'd rather you didn't.", "Not now."],
        to: &[(Tone::Tease, "{p}, try not to cause trouble."), (Tone::Invite, "{p}, we should sit for a while."), (Tone::Talk, "Remember when it was just us, {p}?")],
        reply: [
            &["Heh. You're no fun.", "Me? Trouble? Never."],
            &["Fine. Five minutes.", "Sure. Let's sit."],
            &["That's kind of you to say.", "Thank you. I mean it."],
            &["I remember. Those were good days.", "Don't get sentimental on me."],
        ],
        say: GOJO_SAY,
    },
    Cast {
        id: "yuji", label: "Yuji",
        hair: 0xFFF08AA0, hair_sh: 0xFFC25A72, coat: 0xFF1E2A4A, coat_hi: 0xFFD33A3A, pants: 0xFF161C30, eye: 0xFFFFC27A, eye_d: 0xFFD98A2A,
        blindfold: false, bun: false, orbs: (0xFFFF8A3A, 0xFFFFE05A, 0xFFFF5A2A), special: Move::Orbs, alt: Move::None,
        greet: "Yo!", shout: "Divergent Fist!",
        click: &[
            "Wanna watch a movie later?",
            "I'll eat anything. Try me.",
            "Don't worry, I'm on it!",
            "Black Flash, someday!",
            "Hey, you okay? You can tell me.",
        ],
        peek: &["Oh, hey!", "Need something?", "You've got a tab open for way too long."],
        chat: &["I'm hungry.", "Wonder what's for dinner.", "I should stretch. You should too!"],
        right: &["Ow! Hey!", "What was that for?", "Whoa, easy!"],
        to: &[(Tone::Invite, "{p}! Race you to the end of the bar!"), (Tone::Invite, "{p}, you hungry? I'm starving."), (Tone::Talk, "Hey {p}, what're you thinking about?")],
        reply: [
            &["Hey! I'll beat you next time!", "Oh, you're on!"],
            &["Eh? Sure, why not!", "Always. Let's go."],
            &["Haha, thanks! Same here!", "Aw, you're the best."],
            &["Nothing, really. Just vibing.", "Food, mostly."],
        ],
        say: GOJO_SAY,
    },
];

// Private buddies live in cast_personal.rs, which is gitignored. build.rs sets `personal` only
// when that file exists and ZORUI_PUBLIC is unset, so a clone or the release exe never has them.
#[cfg(personal)]
#[path = "cast_personal.rs"]
mod personal;

pub static CASTS: std::sync::LazyLock<Vec<Cast>> = std::sync::LazyLock::new(|| {
    let mut v = PUBLIC.to_vec();
    #[cfg(personal)]
    v.extend(personal::PERSONAL);
    v
});

/// The cast with this id; Gojo for anything unknown.
pub fn get(id: &str) -> &'static Cast { CASTS.iter().find(|c| c.id == id).unwrap_or(&CASTS[0]) }

/// Hairstyle drawn behind and on top of the head. Chosen per buddy in Settings; Spiky is the original.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Hair { #[default] Spiky, Short, Long, Mohawk }

/// (config value, name) for the Settings picker.
pub const HAIR_STYLES: [(&str, &str); 4] = [("spiky", "Spiky"), ("short", "Short"), ("long", "Long"), ("mohawk", "Mohawk")];

impl Hair {
    /// None for empty or unknown, so the buddy keeps its own style.
    pub fn parse(s: &str) -> Option<Hair> {
        Some(match s { "spiky" => Hair::Spiky, "short" => Hair::Short, "long" => Hair::Long, "mohawk" => Hair::Mohawk, _ => return None })
    }
}

/// Casts need 'static names; each distinct custom name is leaked once, not once per config read.
fn intern(s: &str) -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut v) = NAMES.lock() else { return "Buddy" };
    if let Some(n) = v.iter().find(|n| **n == s) { return n; }
    let n: &'static str = Box::leak(s.to_string().into_boxed_str());
    v.push(n);
    n
}

/// A list of lines as one `'static` slice, leaked once per distinct content (config is re-read every second).
fn leak_list(v: &[String]) -> &'static [&'static str] {
    static KEPT: std::sync::Mutex<Vec<(String, &'static [&'static str])>> = std::sync::Mutex::new(Vec::new());
    let key = v.join("\u{1}");
    let Ok(mut k) = KEPT.lock() else { return &[] };
    if let Some((_, l)) = k.iter().find(|(s, _)| *s == key) { return l; }
    let l: &'static [&'static str] = Box::leak(v.iter().map(|s| intern(s)).collect::<Vec<_>>().into_boxed_slice());
    k.push((key, l));
    l
}

/// "tease: Hey {p}" is a Tease line; no known tone before the colon means Talk. None without "{p}", which the partner's name fills.
fn to_line(s: &str) -> Option<(Tone, String)> {
    let (tone, text) = match s.split_once(':') {
        Some((t, rest)) => match t.trim().to_lowercase().as_str() {
            "tease" => (Tone::Tease, rest), "invite" => (Tone::Invite, rest), "love" => (Tone::Love, rest), "talk" => (Tone::Talk, rest),
            _ => (Tone::Talk, s),
        },
        None => (Tone::Talk, s),
    };
    text.contains("{p}").then(|| (tone, text.trim().to_string()))
}

fn leak_to(v: &[(Tone, String)]) -> &'static [(Tone, &'static str)] {
    static KEPT: std::sync::Mutex<Vec<(String, &'static [(Tone, &'static str)])>> = std::sync::Mutex::new(Vec::new());
    let key = v.iter().map(|(t, s)| format!("{}{s}", *t as u8)).collect::<Vec<_>>().join("\u{1}");
    let Ok(mut k) = KEPT.lock() else { return &[] };
    if let Some((_, l)) = k.iter().find(|(s, _)| *s == key) { return l; }
    let l: &'static [(Tone, &'static str)] = Box::leak(v.iter().map(|(t, s)| (*t, intern(s))).collect::<Vec<_>>().into_boxed_slice());
    k.push((key, l));
    l
}

impl Cast {
    /// Replaces the lines the pack writes (`lines` by category; see `Say` and the fields above): a lone line keeps
    /// the first entry, even an empty one (silence); a list keeps its non-empty entries and is ignored if none are
    /// left, so a category can never end up with nothing to pick. `to` entries are "tone: text with {p}";
    /// `reply_tease`, `reply_invite`, `reply_love` and `reply_talk` answer each tone.
    pub fn apply_lines(&mut self, lines: &std::collections::HashMap<String, Vec<String>>) {
        let one = |d: &mut &'static str, k: &str| if let Some(s) = lines.get(k).and_then(|v| v.first()) { *d = intern(s); };
        let many = |d: &mut &'static [&'static str], k: &str| {
            let v: Vec<String> = lines.get(k).into_iter().flatten().filter(|s| !s.is_empty()).cloned().collect();
            if !v.is_empty() { let l = leak_list(&v); if !l.is_empty() { *d = l; } }
        };
        one(&mut self.greet, "greet");
        one(&mut self.shout, "shout");
        let s = &mut self.say;
        for (k, d) in [("wake_click", &mut s.wake_click), ("teleport", &mut s.teleport), ("landed", &mut s.landed), ("back", &mut s.back), ("shield_on", &mut s.shield_on),
            ("shield_off", &mut s.shield_off), ("late", &mut s.late), ("game_end", &mut s.game_end), ("low_battery", &mut s.low_battery), ("claude_high", &mut s.claude_high),
            ("claude_near", &mut s.claude_near), ("note", &mut s.note), ("track", &mut s.track), ("music_on", &mut s.music_on), ("music_off", &mut s.music_off),
            ("muted", &mut s.muted), ("unmuted", &mut s.unmuted), ("loud", &mut s.loud), ("quiet", &mut s.quiet), ("online", &mut s.online), ("offline", &mut s.offline),
            ("charging", &mut s.charging), ("unplugged", &mut s.unplugged), ("thunder", &mut s.thunder), ("snow", &mut s.snow), ("rain", &mut s.rain), ("busy", &mut s.busy)] {
            one(d, k);
        }
        for (k, d) in [("held", &mut s.held), ("break_tips", &mut s.break_tips), ("game_start", &mut s.game_start), ("focus_code", &mut s.focus_code), ("focus_video", &mut s.focus_video),
            ("focus_social", &mut s.focus_social), ("focus_chat", &mut s.focus_chat), ("focus_game", &mut s.focus_game)] {
            many(d, k);
        }
        for (k, d) in [("click", &mut self.click), ("peek", &mut self.peek), ("chat", &mut self.chat), ("right", &mut self.right)] { many(d, k); }
        let [r0, r1, r2, r3] = &mut self.reply;
        for (k, d) in [("reply_tease", r0), ("reply_invite", r1), ("reply_love", r2), ("reply_talk", r3)] { many(d, k); }
        let to: Vec<(Tone, String)> = lines.get("to").into_iter().flatten().filter_map(|s| to_line(s)).collect();
        if !to.is_empty() { self.to = leak_to(&to); }
    }
}

/// The cast of a pack: the `base` built-in character's lines (neutral ones when it names none), then the pack's own
/// name, colours and lines on top.
pub fn from_pack(p: &crate::pack::Pack) -> Cast {
    let mut c = CASTS.iter().find(|c| c.id == p.base).cloned().unwrap_or(NEUTRAL);
    c.id = intern(&format!("pack:{}", p.id));
    c.label = intern(&p.name);
    let col = |k: &str| p.colors.get(k).map_or("", |s| s.as_str());
    c.dress(col("hair"), "", "", col("eye"), "");
    let orbs: Vec<Color> = col("orbs").split(',').filter_map(|s| conf::parse_hex(s.trim())).collect();
    if let [a, b, m] = orbs[..] { c.orbs = (a, b, m); }
    c.apply_lines(&p.lines);
    for (k, d) in [("special", &mut c.special), ("alt", &mut c.alt)] {
        if let Some(m) = p.moves.get(k).and_then(|s| Move::parse(s)) { *d = m; }
    }
    // a right-click with no move has to say something
    if c.alt == Move::None && c.right.is_empty() { c.right = NEUTRAL.right; }
    c
}

/// Feature flag: saved buddies, pictures and the glasses, ears and skin options. Off unless config.json has
/// "buddies": {"customEnabled": true}; while off, a saved buddy's id falls back to Gojo.
pub fn custom_on(cfg: &conf::Config) -> bool { cfg.get("buddies.customEnabled").as_bool().unwrap_or(false) }

/// The buddy with this id: a built-in one, or one saved with "Save as new buddy" (config buddies.custom).
/// A saved buddy keeps the lines of the character it was made from.
#[cfg(test)]
pub fn pick(cfg: &conf::Config, id: &str) -> (Cast, Hair) { pick_with(custom_on(cfg), cfg.get("buddies.custom"), id) }

/// `pick` with the saved buddies (`custom`, a JSON list) and whether they are enabled given directly, so Settings can ask
/// about values that haven't reached config.json yet.
pub fn pick_with(custom_on: bool, custom: &serde_json::Value, id: &str) -> (Cast, Hair) {
    // a pack folder is a deliberate act, so it needs no feature flag; one that went missing falls back to Gojo
    if let Some(p) = id.strip_prefix("pack:").and_then(crate::pack::load) { return (from_pack(&p), Hair::default()); }
    let Some(e) = custom_on.then(|| custom.as_array()).flatten().and_then(|a| a.iter().find(|e| e["id"] == id)) else { return (get(id).clone(), Hair::default()) };
    let s = |f: &str| e[f].as_str().unwrap_or("");
    let mut c = get(s("base")).clone();
    c.id = intern(id);
    c.label = intern(s("name"));
    c.dress(s("hair"), s("shirt"), s("pants"), s("eyes"), s("bun"));
    (c, Hair::parse(s("style")).unwrap_or_default())
}

/// Look options beyond the cast's palette: skin tone, glasses, cat ears, and a picture that replaces the
/// drawn figure. The fields of a buddies.custom entry or of a slot use the same names.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Trim { pub skin: Option<Color>, pub glasses: bool, pub ears: bool, pub image: String }

/// The extra look fields a slot can set and `save` copies into a new entry.
pub const TRIM_FIELDS: [&str; 4] = ["skin", "glasses", "ears", "image"];

impl Trim {
    /// `get` returns a field's text ("" = unset). Glasses and ears are "yes"/"no"; `base` is what an unset one keeps.
    pub fn read(base: &Trim, get: impl Fn(&str) -> String) -> Trim {
        let flag = |f: &str, was: bool| match get(f).as_str() { "yes" => true, "no" => false, _ => was };
        let image = get("image");
        Trim {
            skin: conf::parse_hex(get("skin").trim()).or(base.skin), glasses: flag("glasses", base.glasses), ears: flag("ears", base.ears),
            image: if image.is_empty() { base.image.clone() } else { image },
        }
    }
}

/// The look options of the buddy `id` (a saved one; built-in ones have none).
#[cfg(test)]
pub fn trim(cfg: &conf::Config, id: &str) -> Trim { trim_with(custom_on(cfg), cfg.get("buddies.custom"), id) }

/// `trim` with the saved buddies given directly (see `pick_with`).
pub fn trim_with(custom_on: bool, custom: &serde_json::Value, id: &str) -> Trim {
    let Some(e) = custom_on.then(|| custom.as_array()).flatten().and_then(|a| a.iter().find(|e| e["id"] == id)) else { return Trim::default() };
    Trim::read(&Trim::default(), |f| e[f].as_str().unwrap_or("").to_string())
}

/// The buddies.custom entry for a new buddy made from the character `cur` (built-in or already saved in `list`)
/// plus the non-empty `look` fields (hair, shirt, pants, eyes, bun, style) the slot had set.
pub fn save(list: &[serde_json::Value], cur: &str, look: &[(&str, &str)], name: &str, id: &str) -> serde_json::Value {
    let mut e = list.iter().find(|e| e["id"] == cur).cloned().unwrap_or_else(|| serde_json::json!({ "base": if cur.is_empty() { "gojo" } else { cur } }));
    for (f, v) in look.iter().filter(|(_, v)| !v.is_empty()) { e[*f] = serde_json::json!(v); }
    e["id"] = id.into();
    e["name"] = if name.trim().is_empty() { format!("My buddy {}", list.len() + 1) } else { name.trim().to_string() }.into();
    e
}

/// Gives the saved buddy `id` a new name; false if there is no such buddy or the name is blank.
pub fn rename(list: &mut [serde_json::Value], id: &str, name: &str) -> bool {
    let name = name.trim();
    match list.iter_mut().find(|e| e["id"] == id) {
        Some(e) if !name.is_empty() => { e["name"] = name.into(); true }
        _ => false,
    }
}

/// Drops the saved buddy `id`; false if there was none.
pub fn remove(list: &mut Vec<serde_json::Value>, id: &str) -> bool {
    let n = list.len();
    list.retain(|e| e["id"] != id);
    list.len() != n
}

/// (id, name) of every buddy that can be picked: built-in ones, pack folders, then saved ones.
pub fn all(custom: &serde_json::Value) -> Vec<(String, String)> { all_with(custom, crate::pack::list()) }

fn all_with(custom: &serde_json::Value, packs: Vec<(String, String)>) -> Vec<(String, String)> {
    let saved = custom.as_array().into_iter().flatten().filter_map(|e| Some((e["id"].as_str()?.to_string(), e["name"].as_str()?.to_string())));
    CASTS.iter().map(|c| (c.id.to_string(), c.label.to_string())).chain(packs).chain(saved).collect()
}

/// Each channel scaled by `f`; above 1.0 it brightens, but only up to white.
fn shade(c: Color, f: f32) -> Color {
    let ch = |s: u32| ((((c >> s) & 0xFF) as f32 * f).min(255.0)) as u32;
    (c & 0xFF00_0000) | ch(16) << 16 | ch(8) << 8 | ch(0)
}

impl Cast {
    /// Applies the colours typed in Settings > Buddies ("#RRGGBB"). Empty or invalid keeps the character's own.
    /// The shadow and highlight shades follow the colour chosen; `bun` is "yes", "no", or anything else to keep.
    pub fn dress(&mut self, hair: &str, shirt: &str, pants: &str, eye: &str, bun: &str) {
        if let Some(c) = conf::parse_hex(hair.trim()) { self.hair = c; self.hair_sh = shade(c, 0.7); }
        if let Some(c) = conf::parse_hex(shirt.trim()) { self.coat = c; self.coat_hi = shade(c, 1.2); }
        if let Some(c) = conf::parse_hex(pants.trim()) { self.pants = c; }
        if let Some(c) = conf::parse_hex(eye.trim()) { self.eye = c; self.eye_d = shade(c, 0.6); }
        match bun { "yes" => self.bun = true, "no" => self.bun = false, _ => {} }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookup_and_lines() {
        assert_eq!(get("geto").label, "Geto");
        assert_eq!(get("nobody").id, "gojo", "an unknown id falls back to Gojo");
        for c in CASTS.iter() {
            assert!(!c.click.is_empty() && !c.peek.is_empty() && !c.chat.is_empty() && !c.to.is_empty() && c.reply.iter().all(|r| !r.is_empty()), "{} lacks lines", c.id);
            assert!(c.alt != Move::None || !c.right.is_empty(), "{} needs right-click lines when the right-click has no move", c.id);
            assert!(c.special != Move::None, "{} needs a double-click move", c.id);
            assert!(c.to.iter().all(|(_, l)| l.contains("{p}")), "{} should name the partner", c.id);
        }
    }

    #[test]
    fn saved_buddy_keeps_base_lines_and_own_look() {
        let cfg = conf::Config::parse(r##"{"buddies":{"customEnabled":true,"custom":[{"id":"mine1","name":"Rin","base":"yuji","hair":"#102030","style":"long","bun":"yes"}]}}"##);
        let (c, h) = pick(&cfg, "mine1");
        assert_eq!((c.label, c.id, c.hair, h, c.bun), ("Rin", "mine1", 0xFF10_2030, Hair::Long, true));
        assert_eq!(c.greet, get("yuji").greet);
        assert_eq!(pick(&cfg, "geto").1, Hair::Spiky, "built-in buddies start spiky");
        assert_eq!(all(cfg.get("buddies.custom")).last().unwrap(), &("mine1".to_string(), "Rin".to_string()));
        assert_eq!(Hair::parse("mohawk"), Some(Hair::Mohawk));
        assert_eq!(Hair::parse(""), None);
    }

    #[test]
    fn save_folds_the_slot_look_into_a_new_entry() {
        let first = save(&[], "yuji", &[("hair", "#FF0000"), ("style", "")], " Rin ", "c1");
        assert_eq!((first["base"].as_str(), first["name"].as_str(), first["hair"].as_str(), first.get("style")), (Some("yuji"), Some("Rin"), Some("#FF0000"), None));
        let second = save(&[first], "c1", &[("style", "mohawk")], "", "c2");
        assert_eq!((second["id"].as_str(), second["base"].as_str(), second["hair"].as_str(), second["style"].as_str(), second["name"].as_str()),
            (Some("c2"), Some("yuji"), Some("#FF0000"), Some("mohawk"), Some("My buddy 2")), "keeps what the first one had");
    }

    #[test]
    fn rename_and_remove_saved_buddies() {
        let mut l = vec![save(&[], "geto", &[], "A", "c1"), save(&[], "yuji", &[], "B", "c2")];
        assert!(rename(&mut l, "c2", " Zed "));
        assert_eq!(l[1]["name"], "Zed");
        assert!(!rename(&mut l, "c2", "  ") && !rename(&mut l, "nope", "X"), "blank or unknown changes nothing");
        assert!(remove(&mut l, "c1") && !remove(&mut l, "c1"));
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn custom_buddies_are_off_unless_flagged() {
        let off = conf::Config::parse(r##"{"buddies":{"custom":[{"id":"c1","name":"X","base":"yuji","ears":"yes","image":"a.png"}]}}"##);
        assert!(!custom_on(&off));
        assert_eq!(pick(&off, "c1").0.id, "gojo", "a saved id falls back to Gojo while the flag is off");
        assert_eq!(trim(&off, "c1"), Trim::default());
        assert!(custom_on(&conf::Config::parse(r#"{"buddies":{"customEnabled":true}}"#)));
    }

    #[test]
    fn trim_reads_saved_entry_and_slot_overrides() {
        let cfg = conf::Config::parse(r##"{"buddies":{"customEnabled":true,"custom":[{"id":"c1","name":"X","skin":"#102030","ears":"yes","image":"buddy-1.png"}]}}"##);
        let t = trim(&cfg, "c1");
        assert_eq!((t.skin, t.glasses, t.ears, t.image.as_str()), (Some(0xFF10_2030), false, true, "buddy-1.png"));
        assert_eq!(trim(&cfg, "gojo"), Trim::default(), "built-in buddies have no extras");
        let slot = |f: &str| match f { "glasses" => "yes", "ears" => "no", "skin" => "bad", _ => "" }.to_string();
        let o = Trim::read(&t, slot);
        assert_eq!((o.skin, o.glasses, o.ears, o.image.as_str()), (Some(0xFF10_2030), true, false, "buddy-1.png"), "a slot field overrides; unset or invalid keeps the saved one");
    }

    fn pack(json: &str) -> crate::pack::Pack { crate::pack::parse("goku", std::path::Path::new("x"), json, 1).unwrap() }

    #[test]
    fn a_pack_with_no_base_speaks_neutrally_not_like_gojo() {
        let c = from_pack(&pack(r#"{"name":"Goku","frames":{"stand":["a.png"]}}"#));
        assert_eq!((c.label, c.id), ("Goku", "pack:goku"));
        assert_eq!(c.say, NEUTRAL_SAY);
        assert!(!c.click.is_empty() && !c.blindfold && c.to.iter().all(|(_, l)| l.contains("{p}")), "neutral lines are complete");
        assert_ne!(c.say.teleport, GOJO_SAY.teleport);
    }

    #[test]
    fn a_pack_inherits_a_base_and_overrides_what_it_writes() {
        let c = from_pack(&pack(r##"{"base":"yuji","frames":{"stand":["a.png"]},"colors":{"hair":"#102030","eye":"#FFFFFF","orbs":"#111111,#222222,#333333"},
            "lines":{"greet":"Yo!","click":["Kamehameha!","", "Hungry."],"landed":"","teleport":"Instant Transmission!","held":["Hey!"],
                     "to":["tease: {p}, spar with me.","invite: Eat with me, {p}?","no partner name here","Plain {p}"],"reply_love":["Aw."]}}"##));
        assert_eq!(c.greet, "Yo!");
        assert_eq!(c.click, ["Kamehameha!", "Hungry."], "empty entries in a list are dropped");
        assert_eq!(c.say.landed, "", "a lone empty line is kept: silence");
        assert_eq!(c.say.teleport, "Instant Transmission!");
        assert_eq!(c.say.held, ["Hey!"]);
        assert_eq!(c.peek, get("yuji").peek, "what the pack leaves out is the base's");
        assert_eq!(c.say.back, GOJO_SAY.back);
        assert_eq!((c.hair, c.eye, c.orbs), (0xFF10_2030, 0xFFFF_FFFF, (0xFF11_1111, 0xFF22_2222, 0xFF33_3333)));
        assert_eq!(c.to.iter().map(|(t, l)| (*t, *l)).collect::<Vec<_>>(), [(Tone::Tease, "{p}, spar with me."), (Tone::Invite, "Eat with me, {p}?"), (Tone::Talk, "Plain {p}")],
            "a line without the partner's name is dropped; no tone means Talk");
        assert_eq!(c.reply[Tone::Love as usize], ["Aw."]);
        assert_eq!(c.reply[Tone::Tease as usize], get("yuji").reply[Tone::Tease as usize]);
    }

    #[test]
    fn moves_have_names_and_the_built_ins_keep_theirs() {
        assert_eq!([" Beam ", "AURA", "orbs", "none", "shield"].map(Move::parse), [Some(Move::Beam), Some(Move::Aura), Some(Move::Orbs), Some(Move::None), Some(Move::Shield)]);
        assert_eq!((Move::parse("laser"), Move::parse("")), (None, None));
        assert_eq!((get("gojo").special, get("gojo").alt), (Move::Orbs, Move::Shield), "Hollow Purple and Infinity");
        for id in ["geto", "yuji"] { assert_eq!((get(id).special, get(id).alt), (Move::Orbs, Move::None), "{id} had no Infinity"); }
    }

    #[test]
    fn a_pack_picks_its_moves_and_keeps_the_base_for_unknown_names() {
        let c = from_pack(&pack(r#"{"frames":{"stand":["a.png"]},"moves":{"special":"beam","alt":"aura"}}"#));
        assert_eq!((c.special, c.alt), (Move::Beam, Move::Aura));
        let c = from_pack(&pack(r#"{"base":"gojo","frames":{"stand":["a.png"]},"moves":{"special":"laser"}}"#));
        assert_eq!((c.special, c.alt), (Move::Orbs, Move::Shield), "an unknown name keeps the base's move");
        let c = from_pack(&pack(r#"{"base":"gojo","frames":{"stand":["a.png"]},"moves":{"alt":"none"}}"#));
        assert_eq!(c.alt, Move::None);
        assert_eq!(c.right, NEUTRAL.right, "Gojo has no right-click lines, so a pack that drops Infinity gets neutral ones");
        let c = from_pack(&pack(r#"{"frames":{"stand":["a.png"]}}"#));
        assert_eq!((c.special, c.alt), (Move::Aura, Move::None), "a neutral buddy glows");
    }

    #[test]
    fn a_list_with_nothing_usable_keeps_the_base_so_nothing_is_ever_empty() {
        let c = from_pack(&pack(r#"{"frames":{"stand":["a.png"]},"lines":{"click":["",""],"to":["no name"],"held":[""]}}"#));
        assert_eq!((c.click, c.say.held), (NEUTRAL.click, NEUTRAL_SAY.held));
        assert_eq!(c.to.len(), NEUTRAL.to.len());
    }

    #[test]
    fn the_same_lines_are_leaked_once() {
        let json = r#"{"frames":{"stand":["a.png"]},"lines":{"click":["Leak test A","Leak test B"],"teleport":"Leak test C"}}"#;
        let (a, b) = (from_pack(&pack(json)), from_pack(&pack(json)));
        assert_eq!((a.click.as_ptr(), a.say.teleport.as_ptr()), (b.click.as_ptr(), b.say.teleport.as_ptr()), "config is re-read every second: no new copy each time");
    }

    #[test]
    fn packs_are_listed_between_built_in_and_saved_buddies() {
        let saved = serde_json::json!([{"id": "c1", "name": "Mine"}]);
        let all = all_with(&saved, vec![("pack:goku".into(), "Goku".into())]);
        let ids: Vec<&str> = all.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(&ids[ids.len() - 2..], ["pack:goku", "c1"]);
        assert!(ids.contains(&"gojo"));
        assert_eq!(pick(&conf::Config::parse("{}"), "pack:no-such-folder-zorui").0.id, "gojo", "a pack that is gone is Gojo again");
        assert_eq!(pick(&conf::Config::parse("{}"), "pack:../x").0.id, "gojo", "a name that leaves the folder is never read");
    }

    #[test]
    fn dress_overrides_only_valid_colours() {
        let mut c = get("gojo").clone();
        c.dress("#102030", "nope", "", "#FFFFFF", "yes");
        assert_eq!(c.hair, 0xFF10_2030);
        assert_eq!(c.coat, get("gojo").coat, "an invalid colour keeps the original");
        assert_eq!(c.eye_d, 0xFF99_9999);
        assert!(c.bun);
    }
}

#[cfg(all(test, not(personal)))]
mod public_tests {
    #[test]
    fn release_has_no_private_buddies() {
        assert_eq!(super::CASTS.len(), super::PUBLIC.len());
    }
}
