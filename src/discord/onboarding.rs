//! Server onboarding: the questions new members answer to pick roles and
//! channels, plus the channels everyone starts in.
//!
//! The settings are kept as the JSON Discord sends, edited in place, and sent
//! back whole, so fields this client does not know about survive a save.

use super::model::Id;
use serde_json::{Value, json};

fn ids(v: &Value) -> Vec<Id> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str()?.parse().ok()).map(Id).collect()
}

fn set_ids(v: &mut Value, list: &[Id]) {
    *v = Value::Array(list.iter().map(|i| json!(i.to_string())).collect());
}

/// Adds `id` to a JSON id list, or removes it if present.
fn toggle_in(v: &mut Value, id: Id) {
    let mut list = ids(v);
    match list.iter().position(|x| *x == id) {
        Some(ix) => {
            list.remove(ix);
        }
        None => list.push(id),
    }
    set_ids(v, &list);
}

pub fn enabled(o: &Value) -> bool {
    o["enabled"].as_bool().unwrap_or(false)
}

pub fn set_enabled(o: &mut Value, on: bool) {
    o["enabled"] = json!(on);
}

pub fn default_channels(o: &Value) -> Vec<Id> {
    ids(&o["default_channel_ids"])
}

pub fn toggle_default_channel(o: &mut Value, channel: Id) {
    toggle_in(&mut o["default_channel_ids"], channel);
}

pub struct PromptView {
    pub title: String,
    pub single_select: bool,
    pub required: bool,
    pub options: Vec<OptionView>,
}

pub struct OptionView {
    pub title: String,
    pub roles: Vec<Id>,
    pub channels: Vec<Id>,
}

pub fn prompts(o: &Value) -> Vec<PromptView> {
    o["prompts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| PromptView {
            title: p["title"].as_str().unwrap_or_default().to_string(),
            single_select: p["single_select"].as_bool().unwrap_or(false),
            required: p["required"].as_bool().unwrap_or(false),
            options: p["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|opt| OptionView {
                    title: opt["title"].as_str().unwrap_or_default().to_string(),
                    roles: ids(&opt["role_ids"]),
                    channels: ids(&opt["channel_ids"]),
                })
                .collect(),
        })
        .collect()
}

/// Appends a question with the given answers. Ids are made up locally, as
/// Discord expects of new prompts.
pub fn add_prompt(o: &mut Value, title: &str, answers: &[String], single_select: bool) {
    let mut next = Id::now().0;
    let mut id = || {
        next += 1;
        next.to_string()
    };
    let options: Vec<Value> = answers
        .iter()
        .map(|a| json!({ "id": id(), "title": a, "description": "", "role_ids": [], "channel_ids": [] }))
        .collect();
    let prompt = json!({
        "id": id(), "type": 0, "title": title, "options": options,
        "single_select": single_select, "required": false, "in_onboarding": true,
    });
    if !o["prompts"].is_array() {
        o["prompts"] = json!([]);
    }
    o["prompts"].as_array_mut().map(|p| p.push(prompt));
}

pub fn remove_prompt(o: &mut Value, ix: usize) {
    if let Some(list) = o["prompts"].as_array_mut().filter(|l| ix < l.len()) {
        list.remove(ix);
    }
}

/// Gives an answer a role (or takes it away).
pub fn toggle_option_role(o: &mut Value, prompt: usize, option: usize, role: Id) {
    let slot = &mut o["prompts"][prompt]["options"][option];
    if slot.is_object() {
        toggle_in(&mut slot["role_ids"], role);
    }
}

pub fn toggle_option_channel(o: &mut Value, prompt: usize, option: usize, channel: Id) {
    let slot = &mut o["prompts"][prompt]["options"][option];
    if slot.is_object() {
        toggle_in(&mut slot["channel_ids"], channel);
    }
}

pub fn toggle_required(o: &mut Value, prompt: usize) {
    let slot = &mut o["prompts"][prompt];
    if slot.is_object() {
        let now = slot["required"].as_bool().unwrap_or(false);
        slot["required"] = json!(!now);
    }
}

/// What to send to `PUT /guilds/{id}/onboarding`.
pub fn save_body(o: &Value) -> Value {
    json!({
        "prompts": o["prompts"].as_array().cloned().unwrap_or_default(),
        "default_channel_ids": o["default_channel_ids"].as_array().cloned().unwrap_or_default(),
        "enabled": enabled(o),
        "mode": o["mode"].as_u64().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_onboarding() {
        let mut o = json!({ "guild_id": "1", "enabled": false, "default_channel_ids": ["10"], "prompts": [], "mode": 0 });
        toggle_default_channel(&mut o, Id(11));
        toggle_default_channel(&mut o, Id(10));
        assert_eq!(default_channels(&o), [Id(11)]);

        add_prompt(&mut o, "What brings you here?", &["Gaming".into(), "Rust".into()], true);
        toggle_option_role(&mut o, 0, 1, Id(50));
        toggle_option_channel(&mut o, 0, 0, Id(11));
        toggle_required(&mut o, 0);
        let p = &prompts(&o)[0];
        assert_eq!(p.title, "What brings you here?");
        assert!(p.single_select && p.required);
        assert_eq!(p.options[1].roles, [Id(50)]);
        assert_eq!(p.options[0].channels, [Id(11)]);
        // Option and prompt ids are distinct.
        assert_ne!(o["prompts"][0]["id"], o["prompts"][0]["options"][0]["id"]);

        set_enabled(&mut o, true);
        let body = save_body(&o);
        assert_eq!(body["enabled"], true);
        assert!(body.get("guild_id").is_none());
        remove_prompt(&mut o, 0);
        assert!(prompts(&o).is_empty());
    }
}
