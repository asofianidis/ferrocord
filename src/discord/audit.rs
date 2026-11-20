//! Server audit log entries, and plain-language descriptions of them.

use super::model::{Id, User};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub id: Id,
    /// Who made the change, if Discord says.
    pub user: Option<User>,
    pub action: u32,
    pub target: Option<Id>,
    pub reason: Option<String>,
    /// Names of the changed fields, such as `name` or `permissions`.
    pub changed: Vec<String>,
}

/// What kind of thing an action's target is, so the caller can name it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetKind {
    Server,
    Channel,
    User,
    Role,
    Invite,
    Webhook,
    Emoji,
    Integration,
    Stage,
    Event,
    Sticker,
    Thread,
    Command,
    AutoMod,
    Other,
}

/// A short past-tense description of an action, and what its target is.
pub fn describe(action: u32) -> (&'static str, TargetKind) {
    use TargetKind::*;
    match action {
        1 => ("updated the server", Server),
        10 => ("created channel", Channel),
        11 => ("updated channel", Channel),
        12 => ("deleted channel", Channel),
        13 => ("added a permission override to", Channel),
        14 => ("changed a permission override on", Channel),
        15 => ("removed a permission override from", Channel),
        20 => ("kicked", User),
        21 => ("pruned inactive members", Other),
        22 => ("banned", User),
        23 => ("unbanned", User),
        24 => ("updated member", User),
        25 => ("changed the roles of", User),
        26 => ("moved members between voice channels", Other),
        27 => ("disconnected a member from voice", Other),
        28 => ("added the bot", User),
        30 => ("created role", Role),
        31 => ("updated role", Role),
        32 => ("deleted role", Role),
        40 => ("created an invite", Invite),
        41 => ("updated an invite", Invite),
        42 => ("deleted an invite", Invite),
        50 => ("created a webhook", Webhook),
        51 => ("updated a webhook", Webhook),
        52 => ("deleted a webhook", Webhook),
        60 => ("added emoji", Emoji),
        61 => ("updated emoji", Emoji),
        62 => ("deleted emoji", Emoji),
        72 => ("deleted a message by", User),
        73 => ("bulk deleted messages", Channel),
        74 => ("pinned a message by", User),
        75 => ("unpinned a message by", User),
        80 => ("added an integration", Integration),
        81 => ("updated an integration", Integration),
        82 => ("removed an integration", Integration),
        83 => ("started a stage", Stage),
        84 => ("updated a stage", Stage),
        85 => ("ended a stage", Stage),
        90 => ("created a sticker", Sticker),
        91 => ("updated a sticker", Sticker),
        92 => ("deleted a sticker", Sticker),
        100 => ("created an event", Event),
        101 => ("updated an event", Event),
        102 => ("cancelled an event", Event),
        110 => ("created thread", Thread),
        111 => ("updated thread", Thread),
        112 => ("deleted thread", Thread),
        121 => ("changed command permissions", Command),
        140 => ("created an AutoMod rule", AutoMod),
        141 => ("updated an AutoMod rule", AutoMod),
        142 => ("deleted an AutoMod rule", AutoMod),
        143 => ("had a message blocked by AutoMod", AutoMod),
        144 => ("had a message flagged by AutoMod", AutoMod),
        145 => ("was timed out by AutoMod", AutoMod),
        _ => ("made a change", Other),
    }
}

fn id_of(v: &Value) -> Option<Id> {
    v.as_str().and_then(|s| s.parse().ok()).map(Id)
}

/// Parses the response of `GET /guilds/{id}/audit-logs`, newest first.
pub fn parse(log: &Value) -> Vec<AuditEntry> {
    let users: Vec<User> = log["users"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| serde_json::from_value(u.clone()).ok())
        .collect();
    log["audit_log_entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let id = id_of(&e["id"])?;
            let user_id = id_of(&e["user_id"]);
            Some(AuditEntry {
                id,
                user: user_id.and_then(|uid| users.iter().find(|u| u.id == uid).cloned()),
                action: e["action_type"].as_u64().unwrap_or(0) as u32,
                target: id_of(&e["target_id"]),
                reason: e["reason"].as_str().filter(|r| !r.is_empty()).map(str::to_string),
                changed: e["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["key"].as_str().map(str::to_string))
                    .collect(),
            })
        })
        .collect()
}

/// The users an audit log mentions, so targets can be named.
pub fn users(log: &Value) -> Vec<User> {
    log["users"].as_array().into_iter().flatten().filter_map(|u| serde_json::from_value(u.clone()).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entries_are_parsed_with_their_authors() {
        let log = json!({
            "users": [{ "id": "1", "username": "ada" }, { "id": "2", "username": "bob" }],
            "audit_log_entries": [
                { "id": "900", "user_id": "1", "target_id": "2", "action_type": 22, "reason": "spam" },
                { "id": "800", "user_id": "1", "target_id": "50", "action_type": 31,
                  "changes": [{ "key": "name", "old_value": "a", "new_value": "b" }, { "key": "color" }] },
                { "id": "700", "action_type": 9999, "reason": "" },
            ],
        });
        let entries = parse(&log);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].user.as_ref().unwrap().username, "ada");
        assert_eq!(entries[0].reason.as_deref(), Some("spam"));
        assert_eq!(describe(entries[0].action), ("banned", TargetKind::User));
        assert_eq!(entries[1].changed, ["name", "color"]);
        assert_eq!(entries[2].reason, None);
        assert_eq!(describe(entries[2].action).0, "made a change");
        assert_eq!(users(&log).len(), 2);
    }
}
