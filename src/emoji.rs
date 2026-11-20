//! A compact set of common Unicode emoji with Discord-style shortcodes.

pub const CATEGORIES: &[(&str, &[(&str, &str)])] = &[
    ("Smileys", &[
        ("grinning", "😀"), ("smiley", "😃"), ("smile", "😄"), ("grin", "😁"), ("laughing", "😆"),
        ("sweat_smile", "😅"), ("joy", "😂"), ("rofl", "🤣"), ("slight_smile", "🙂"), ("upside_down", "🙃"),
        ("wink", "😉"), ("blush", "😊"), ("innocent", "😇"), ("heart_eyes", "😍"), ("star_struck", "🤩"),
        ("kissing_heart", "😘"), ("yum", "😋"), ("stuck_out_tongue", "😛"), ("zany", "🤪"), ("thinking", "🤔"),
        ("neutral", "😐"), ("expressionless", "😑"), ("smirk", "😏"), ("unamused", "😒"), ("rolling_eyes", "🙄"),
        ("grimacing", "😬"), ("relieved", "😌"), ("pensive", "😔"), ("sleepy", "😪"), ("sleeping", "😴"),
        ("mask", "😷"), ("sunglasses", "😎"), ("nerd", "🤓"), ("confused", "😕"), ("worried", "😟"),
        ("open_mouth", "😮"), ("flushed", "😳"), ("pleading", "🥺"), ("cry", "😢"), ("sob", "😭"),
        ("scream", "😱"), ("tired", "😫"), ("angry", "😠"), ("rage", "😡"), ("skull", "💀"),
        ("clown", "🤡"), ("ghost", "👻"), ("robot", "🤖"), ("partying", "🥳"), ("melting", "🫠"),
    ]),
    ("People", &[
        ("thumbsup", "👍"), ("thumbsdown", "👎"), ("ok_hand", "👌"), ("wave", "👋"), ("clap", "👏"),
        ("raised_hands", "🙌"), ("pray", "🙏"), ("handshake", "🤝"), ("muscle", "💪"), ("point_up", "☝️"),
        ("point_down", "👇"), ("point_left", "👈"), ("point_right", "👉"), ("v", "✌️"), ("crossed_fingers", "🤞"),
        ("metal", "🤘"), ("call_me", "🤙"), ("fist", "✊"), ("punch", "👊"), ("eyes", "👀"),
        ("brain", "🧠"), ("shrug", "🤷"), ("facepalm", "🤦"), ("salute", "🫡"), ("heart_hands", "🫶"),
    ]),
    ("Symbols", &[
        ("heart", "❤️"), ("orange_heart", "🧡"), ("yellow_heart", "💛"), ("green_heart", "💚"), ("blue_heart", "💙"),
        ("purple_heart", "💜"), ("black_heart", "🖤"), ("broken_heart", "💔"), ("sparkling_heart", "💖"), ("fire", "🔥"),
        ("sparkles", "✨"), ("star", "⭐"), ("zap", "⚡"), ("boom", "💥"), ("100", "💯"),
        ("check", "✅"), ("x", "❌"), ("warning", "⚠️"), ("question", "❓"), ("exclamation", "❗"),
        ("zzz", "💤"), ("sweat_drops", "💦"), ("dash", "💨"), ("speech_balloon", "💬"), ("thought_balloon", "💭"),
    ]),
    ("Objects", &[
        ("tada", "🎉"), ("confetti_ball", "🎊"), ("gift", "🎁"), ("trophy", "🏆"), ("medal", "🏅"),
        ("rocket", "🚀"), ("bulb", "💡"), ("bell", "🔔"), ("lock", "🔒"), ("key", "🔑"),
        ("hammer", "🔨"), ("wrench", "🔧"), ("gear", "⚙️"), ("computer", "💻"), ("keyboard", "⌨️"),
        ("phone", "📱"), ("camera", "📷"), ("books", "📚"), ("memo", "📝"), ("pushpin", "📌"),
        ("link", "🔗"), ("calendar", "📅"), ("hourglass", "⏳"), ("alarm_clock", "⏰"), ("moneybag", "💰"),
        ("video_game", "🎮"), ("dart", "🎯"), ("musical_note", "🎵"), ("headphones", "🎧"), ("art", "🎨"),
    ]),
    ("Nature & Food", &[
        ("dog", "🐶"), ("cat", "🐱"), ("fox", "🦊"), ("bear", "🐻"), ("panda", "🐼"),
        ("frog", "🐸"), ("monkey", "🐵"), ("penguin", "🐧"), ("crab", "🦀"), ("butterfly", "🦋"),
        ("cherry_blossom", "🌸"), ("rose", "🌹"), ("sunflower", "🌻"), ("seedling", "🌱"), ("four_leaf_clover", "🍀"),
        ("sun", "☀️"), ("moon", "🌙"), ("rainbow", "🌈"), ("snowflake", "❄️"), ("ocean", "🌊"),
        ("coffee", "☕"), ("tea", "🍵"), ("beer", "🍺"), ("pizza", "🍕"), ("hamburger", "🍔"),
        ("taco", "🌮"), ("cake", "🍰"), ("cookie", "🍪"), ("popcorn", "🍿"), ("apple", "🍎"),
    ]),
];

/// Emoji whose shortcode contains `query`, best matches first.
pub fn search(query: &str, limit: usize) -> Vec<(&'static str, &'static str)> {
    let query = query.to_lowercase();
    let all = CATEGORIES.iter().flat_map(|(_, list)| list.iter().copied());
    let mut hits: Vec<_> = all.filter(|(name, _)| name.contains(&query)).collect();
    hits.sort_by_key(|(name, _)| (!name.starts_with(&query), name.len()));
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcodes_are_unique_and_searchable() {
        let mut names: Vec<_> = CATEGORIES.iter().flat_map(|(_, l)| l.iter().map(|(n, _)| *n)).collect();
        let total = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), total);
        assert_eq!(search("thumbsu", 5)[0], ("thumbsup", "👍"));
        assert_eq!(search("heart", 3)[0].0, "heart");
    }
}
