//! The buddies you can pick in Settings > Buddies: how each one is coloured and what they say.
//! gojo.rs draws all of them with one chibi figure; a cast only changes its palette, hair and lines.

use crate::gfx::Color;

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
    /// What they say to their partner ("{p}" is the partner's name), and what they answer.
    pub to: &'static [&'static str],
    pub reply: &'static [&'static str],
}

pub const CASTS: [Cast; 3] = [
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
        to: &["Hey {p}, bet I can beat you.", "{p}, you're quiet. Boring~", "Wanna grab sweets, {p}?"],
        reply: &["Don't act like I'm your sidekick.", "Tch. Obviously.", "Sure. You're buying."],
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
        to: &["{p}, try not to cause trouble.", "{p}, we should sit for a while.", "Remember when it was just us, {p}?"],
        reply: &["Heh. You're no fun.", "Fine. Five minutes.", "Don't get sentimental on me."],
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
        to: &["{p}! Race you to the end of the bar!", "{p}, you hungry? I'm starving.", "Hey {p}, what're you thinking about?"],
        reply: &["Eh? Sure, why not!", "Always. Let's go.", "Nothing, really. Just vibing."],
    },
];

/// The cast with this id; Gojo for anything unknown.
pub fn get(id: &str) -> &'static Cast { CASTS.iter().find(|c| c.id == id).unwrap_or(&CASTS[0]) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookup_and_lines() {
        assert_eq!(get("geto").label, "Geto");
        assert_eq!(get("nobody").id, "gojo", "an unknown id falls back to Gojo");
        for c in &CASTS {
            assert!(!c.click.is_empty() && !c.peek.is_empty() && !c.chat.is_empty() && !c.to.is_empty() && !c.reply.is_empty(), "{} lacks lines", c.id);
            assert!(c.blindfold || !c.right.is_empty(), "{} needs right-click lines without Infinity", c.id);
            assert!(c.to.iter().all(|l| l.contains("{p}")), "{} should name the partner", c.id);
        }
    }
}
