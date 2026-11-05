//! The buddies you can pick in Settings > Buddies: how each one is coloured and what they say.
//! gojo.rs draws all of them with one chibi figure; a cast only changes its palette, hair and lines.

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
    /// A blindfold that lifts on hover; only a blindfolded buddy has Infinity on right-click.
    pub blindfold: bool,
    pub bun: bool,
    /// The two orbs of the double-click move and the colour they merge into.
    pub orbs: (Color, Color, Color),
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
}

/// What a line to the partner is: the partner's answer has to fit it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tone { Tease, Invite, Love, Talk }

const PUBLIC: [Cast; 3] = [
    Cast {
        id: "gojo", label: "Gojo",
        hair: 0xFFF5F7FC, hair_sh: 0xFFC3CDE2, coat: 0xFF1E2133, coat_hi: 0xFF323858, pants: 0xFF151724, eye: 0xFF8FE3FF, eye_d: 0xFF2A9EE6,
        blindfold: true, bun: false, orbs: (0xFF4FB4FF, 0xFFFF4A5A, 0xFFB060FF),
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
    },
    Cast {
        id: "geto", label: "Geto",
        hair: 0xFF1B1B22, hair_sh: 0xFF3A3A48, coat: 0xFF23232E, coat_hi: 0xFFE8E4F0, pants: 0xFF1A1A22, eye: 0xFFB0A0FF, eye_d: 0xFF6A50D0,
        blindfold: false, bun: true, orbs: (0xFF3A2A5A, 0xFF5BBF7A, 0xFF4A3A7A),
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
    },
    Cast {
        id: "yuji", label: "Yuji",
        hair: 0xFFF08AA0, hair_sh: 0xFFC25A72, coat: 0xFF1E2A4A, coat_hi: 0xFFD33A3A, pants: 0xFF161C30, eye: 0xFFFFC27A, eye_d: 0xFFD98A2A,
        blindfold: false, bun: false, orbs: (0xFFFF8A3A, 0xFFFFE05A, 0xFFFF5A2A),
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
            assert!(c.blindfold || !c.right.is_empty(), "{} needs right-click lines without Infinity", c.id);
            assert!(c.to.iter().all(|(_, l)| l.contains("{p}")), "{} should name the partner", c.id);
        }
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
