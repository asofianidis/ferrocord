//! AutoMod rules: reading them, describing them, and building the JSON to
//! create or change keyword rules.

use super::model::Id;
use serde_json::{Value, json};

pub const KEYWORD: u8 = 1;
pub const SPAM: u8 = 3;
pub const KEYWORD_PRESET: u8 = 4;
pub const MENTION_SPAM: u8 = 5;
pub const MEMBER_PROFILE: u8 = 6;

/// What a keyword rule does when it matches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Response {
    Block,
    /// Block the message and time the author out for this many seconds.
    BlockAndTimeout(u32),
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub id: Id,
    pub name: String,
    pub trigger: u8,
    pub enabled: bool,
    pub keywords: Vec<String>,
    pub allow: Vec<String>,
    pub presets: Vec<u8>,
    pub mention_limit: u32,
    pub actions: Vec<Value>,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect()
}

pub fn parse(v: &Value) -> Option<Rule> {
    let id = v["id"].as_str()?.parse().ok().map(Id)?;
    let meta = &v["trigger_metadata"];
    Some(Rule {
        id,
        name: v["name"].as_str().unwrap_or_default().to_string(),
        trigger: v["trigger_type"].as_u64().unwrap_or(0) as u8,
        enabled: v["enabled"].as_bool().unwrap_or(false),
        keywords: strings(&meta["keyword_filter"]),
        allow: strings(&meta["allow_list"]),
        presets: meta["presets"].as_array().into_iter().flatten().filter_map(|p| p.as_u64()).map(|p| p as u8).collect(),
        mention_limit: meta["mention_total_limit"].as_u64().unwrap_or(0) as u32,
        actions: v["actions"].as_array().cloned().unwrap_or_default(),
    })
}

pub fn parse_list(v: &Value) -> Vec<Rule> {
    v.as_array().into_iter().flatten().filter_map(parse).collect()
}

impl Rule {
    pub fn kind(&self) -> &'static str {
        match self.trigger {
            KEYWORD => "Custom words",
            SPAM => "Suspected spam",
            KEYWORD_PRESET => "Commonly flagged words",
            MENTION_SPAM => "Mention spam",
            MEMBER_PROFILE => "Member profile words",
            _ => "Other rule",
        }
    }

    /// What the rule looks for, in a few words.
    pub fn summary(&self) -> String {
        match self.trigger {
            KEYWORD | MEMBER_PROFILE => {
                let mut s = match self.keywords.len() {
                    0 => "No words yet".to_string(),
                    n if n <= 4 => self.keywords.join(", "),
                    n => format!("{} and {} more", self.keywords[..3].join(", "), n - 3),
                };
                if !self.allow.is_empty() {
                    s.push_str(&format!(" (allowing {})", self.allow.join(", ")));
                }
                s
            }
            KEYWORD_PRESET => {
                let names: Vec<_> = self
                    .presets
                    .iter()
                    .map(|p| match p {
                        1 => "profanity",
                        2 => "sexual content",
                        3 => "slurs",
                        _ => "other",
                    })
                    .collect();
                if names.is_empty() { "No lists chosen".into() } else { names.join(", ") }
            }
            MENTION_SPAM => format!("More than {} mentions in one message", self.mention_limit),
            SPAM => "Messages Discord thinks are spam".into(),
            _ => String::new(),
        }
    }

    /// The response of a keyword rule, as far as this client can edit it.
    pub fn response(&self) -> Response {
        self.actions
            .iter()
            .find(|a| a["type"].as_u64() == Some(3))
            .and_then(|a| a["metadata"]["duration_seconds"].as_u64())
            .map_or(Response::Block, |s| Response::BlockAndTimeout(s as u32))
    }

    pub fn does(&self) -> String {
        let mut parts = Vec::new();
        for action in &self.actions {
            match action["type"].as_u64() {
                Some(1) => parts.push("blocks the message".to_string()),
                Some(2) => parts.push("alerts moderators".to_string()),
                Some(3) => {
                    let mins = action["metadata"]["duration_seconds"].as_u64().unwrap_or(60) / 60;
                    parts.push(format!("times the author out for {mins} min"));
                }
                Some(4) => parts.push("blocks member interactions".to_string()),
                _ => {}
            }
        }
        if parts.is_empty() { "does nothing".into() } else { parts.join(", ") }
    }
}

/// Splits user input such as `"spoiler, leak ,  *scam*"` into keywords.
pub fn split_words(text: &str) -> Vec<String> {
    text.split([',', '\n']).map(str::trim).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn actions(response: Response) -> Value {
    let block = json!({ "type": 1, "metadata": { "custom_message": "This message was blocked by AutoMod." } });
    match response {
        Response::Block => json!([block]),
        Response::BlockAndTimeout(seconds) => json!([block, { "type": 3, "metadata": { "duration_seconds": seconds } }]),
    }
}

/// The body for creating a keyword rule.
pub fn new_keyword_rule(name: &str, keywords: &[String], allow: &[String], response: Response) -> Value {
    json!({
        "name": name,
        "event_type": 1,
        "trigger_type": KEYWORD,
        "trigger_metadata": { "keyword_filter": keywords, "allow_list": allow },
        "actions": actions(response),
        "enabled": true,
    })
}

/// The body for changing a keyword rule's name, words and response.
pub fn edit_keyword_rule(name: &str, keywords: &[String], allow: &[String], response: Response) -> Value {
    json!({
        "name": name,
        "trigger_metadata": { "keyword_filter": keywords, "allow_list": allow },
        "actions": actions(response),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_parsed_and_described() {
        let list = json!([
            { "id": "1", "name": "No leaks", "trigger_type": 1, "enabled": true,
              "trigger_metadata": { "keyword_filter": ["leak", "spoiler"], "allow_list": ["leaky"] },
              "actions": [{ "type": 1 }, { "type": 3, "metadata": { "duration_seconds": 300 } }] },
            { "id": "2", "name": "Presets", "trigger_type": 4, "enabled": false,
              "trigger_metadata": { "presets": [1, 3] }, "actions": [{ "type": 1 }] },
            { "id": "3", "name": "Mentions", "trigger_type": 5, "trigger_metadata": { "mention_total_limit": 8 },
              "actions": [{ "type": 2, "metadata": { "channel_id": "9" } }] },
            { "name": "broken" },
        ]);
        let rules = parse_list(&list);
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].summary(), "leak, spoiler (allowing leaky)");
        assert_eq!(rules[0].response(), Response::BlockAndTimeout(300));
        assert_eq!(rules[0].does(), "blocks the message, times the author out for 5 min");
        assert_eq!(rules[1].summary(), "profanity, slurs");
        assert!(!rules[1].enabled);
        assert_eq!(rules[2].summary(), "More than 8 mentions in one message");
        assert_eq!(rules[2].does(), "alerts moderators");
    }

    #[test]
    fn keyword_bodies() {
        let words = split_words("leak, spoiler ,, *scam*\nfree nitro");
        assert_eq!(words, ["leak", "spoiler", "*scam*", "free nitro"]);
        let body = new_keyword_rule("Leaks", &words, &[], Response::BlockAndTimeout(60));
        assert_eq!(body["trigger_type"], 1);
        assert_eq!(body["trigger_metadata"]["keyword_filter"][3], "free nitro");
        assert_eq!(body["actions"][1]["metadata"]["duration_seconds"], 60);
        let edit = edit_keyword_rule("Leaks", &words, &["leaky".into()], Response::Block);
        assert_eq!(edit["actions"].as_array().unwrap().len(), 1);
        assert!(edit.get("trigger_type").is_none());
    }
}
