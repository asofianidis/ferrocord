//! Slash commands: flattening Discord's command tree into directly runnable
//! entries, and turning typed arguments into an interaction payload.

use super::model::Id;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct CommandOption {
    pub kind: u8,
    pub name: String,
    pub required: bool,
}

/// One runnable command: a top-level command or a single subcommand of one.
#[derive(Clone, Debug)]
pub struct SlashCommand {
    /// The command object as Discord sent it; echoed back when it is run.
    pub raw: Value,
    pub id: Id,
    pub application_id: Id,
    pub app_name: String,
    /// Subcommand group and subcommand names, if any.
    pub path: Vec<String>,
    pub description: String,
    pub options: Vec<CommandOption>,
}

const SUBCOMMAND: u64 = 1;
const SUBCOMMAND_GROUP: u64 = 2;

impl SlashCommand {
    /// What the user types, without the leading slash: `"poll create"`.
    pub fn full_name(&self) -> String {
        let mut name = self.raw["name"].as_str().unwrap_or_default().to_string();
        for part in &self.path {
            name.push(' ');
            name.push_str(part);
        }
        name
    }

    pub fn usage(&self) -> String {
        self.options
            .iter()
            .map(|o| if o.required { format!("{}:", o.name) } else { format!("[{}:]", o.name) })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn id_of(v: &Value) -> Option<Id> {
    v.as_str().and_then(|s| s.parse().ok()).map(Id)
}

fn leaf_options(list: &Value) -> Vec<CommandOption> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|o| CommandOption {
            kind: o["type"].as_u64().unwrap_or(3) as u8,
            name: o["name"].as_str().unwrap_or_default().to_string(),
            required: o["required"].as_bool().unwrap_or(false),
        })
        .collect()
}

/// Expands one chat-input command into an entry per runnable (sub)command.
pub fn flatten(raw: &Value, app_name: &str) -> Vec<SlashCommand> {
    let (Some(id), Some(application_id)) = (id_of(&raw["id"]), id_of(&raw["application_id"])) else {
        return Vec::new();
    };
    if raw["type"].as_u64().unwrap_or(1) != 1 {
        return Vec::new(); // user and message context-menu commands
    }
    let entry = |path: Vec<String>, source: &Value| SlashCommand {
        raw: raw.clone(),
        id,
        application_id,
        app_name: app_name.to_string(),
        path,
        description: source["description"].as_str().unwrap_or_default().to_string(),
        options: leaf_options(&source["options"]),
    };
    let name = |v: &Value| v["name"].as_str().unwrap_or_default().to_string();
    let mut out = Vec::new();
    for option in raw["options"].as_array().into_iter().flatten() {
        match option["type"].as_u64() {
            Some(SUBCOMMAND) => out.push(entry(vec![name(option)], option)),
            Some(SUBCOMMAND_GROUP) => {
                for sub in option["options"].as_array().into_iter().flatten() {
                    out.push(entry(vec![name(option), name(sub)], sub));
                }
            }
            _ => {}
        }
    }
    if out.is_empty() {
        out.push(entry(Vec::new(), raw));
    }
    out
}

/// Parses the command index Discord returns for a guild or DM.
pub fn parse_index(index: &Value) -> Vec<SlashCommand> {
    let app_name = |id: &Value| {
        index["applications"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["id"] == *id)
            .and_then(|a| a["name"].as_str())
            .unwrap_or("App")
            .to_string()
    };
    let mut commands: Vec<SlashCommand> = index["application_commands"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|c| flatten(c, &app_name(&c["application_id"])))
        .collect();
    commands.sort_by_key(|c| c.full_name());
    commands
}

fn convert(option: &CommandOption, text: &str) -> Result<Value, String> {
    let bad = |what: &str| format!("`{}` needs {what}, but got \"{text}\".", option.name);
    Ok(match option.kind {
        4 => json!(text.parse::<i64>().map_err(|_| bad("a whole number"))?),
        10 => json!(text.parse::<f64>().map_err(|_| bad("a number"))?),
        5 => match text.to_lowercase().as_str() {
            "true" | "yes" | "y" | "1" | "on" => json!(true),
            "false" | "no" | "n" | "0" | "off" => json!(false),
            _ => return Err(bad("true or false")),
        },
        // Users, channels, roles and mentionables are sent as bare ids.
        6..=9 => {
            let digits: String = text.chars().filter(char::is_ascii_digit).collect();
            if digits.is_empty() {
                return Err(bad("a mention or an id"));
            }
            json!(digits)
        }
        11 => return Err(format!("`{}` takes a file, which is not supported here.", option.name)),
        _ => json!(text),
    })
}

/// Splits typed arguments into option values.
///
/// Arguments may be named (`city: Paris days: 3`) or positional, in which
/// case the last option takes whatever text remains.
pub fn parse_args(options: &[CommandOption], text: &str) -> Result<Vec<Value>, String> {
    let text = text.trim();
    let mut values: Vec<(usize, String)> = Vec::new();

    // Find `name:` markers that start a word.
    let mut markers: Vec<(usize, usize, usize)> = Vec::new(); // (start, value start, option index)
    for (ix, option) in options.iter().enumerate() {
        let needle = format!("{}:", option.name);
        let mut from = 0;
        while let Some(found) = text[from..].find(&needle) {
            let start = from + found;
            if start == 0 || text[..start].ends_with(char::is_whitespace) {
                markers.push((start, start + needle.len(), ix));
                break;
            }
            from = start + needle.len();
        }
    }
    markers.sort();

    if !markers.is_empty() {
        for (i, (_, value_start, ix)) in markers.iter().enumerate() {
            let end = markers.get(i + 1).map_or(text.len(), |next| next.0);
            values.push((*ix, text[*value_start..end].trim().to_string()));
        }
    } else if !text.is_empty() {
        match options.len() {
            0 => {}
            1 => values.push((0, text.to_string())),
            n => {
                let mut rest = text;
                for ix in 0..n {
                    rest = rest.trim_start();
                    if rest.is_empty() {
                        break;
                    }
                    if ix == n - 1 {
                        values.push((ix, rest.to_string()));
                    } else {
                        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                        values.push((ix, rest[..end].to_string()));
                        rest = &rest[end..];
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    for (ix, option) in options.iter().enumerate() {
        match values.iter().find(|(i, v)| *i == ix && !v.is_empty()) {
            Some((_, value)) => {
                out.push(json!({ "type": option.kind, "name": option.name, "value": convert(option, value)? }))
            }
            None if option.required => return Err(format!("Missing required option `{}`.", option.name)),
            None => {}
        }
    }
    Ok(out)
}

/// Builds the `data` object of a chat-input interaction.
pub fn interaction_data(command: &SlashCommand, args: &str) -> Result<Value, String> {
    let mut options = Value::Array(parse_args(&command.options, args)?);
    for (depth, name) in command.path.iter().rev().enumerate() {
        // The innermost path element is the subcommand; any outer one is its group.
        let kind = if depth == 0 { SUBCOMMAND } else { SUBCOMMAND_GROUP };
        options = json!([{ "type": kind, "name": name, "options": options }]);
    }
    Ok(json!({
        "version": command.raw["version"],
        "id": command.id.to_string(),
        "name": command.raw["name"],
        "type": 1,
        "options": options,
        "application_command": command.raw,
        "attachments": [],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Value {
        json!({
            "applications": [{ "id": "7", "name": "Weather" }],
            "application_commands": [
                { "id": "1", "application_id": "7", "version": "9", "type": 1, "name": "forecast",
                  "description": "Forecast", "options": [
                    { "type": 3, "name": "city", "description": "", "required": true },
                    { "type": 4, "name": "days", "description": "" },
                ]},
                { "id": "2", "application_id": "7", "version": "9", "type": 1, "name": "alerts", "description": "",
                  "options": [
                    { "type": 1, "name": "list", "description": "List alerts", "options": [] },
                    { "type": 2, "name": "rules", "description": "", "options": [
                        { "type": 1, "name": "add", "description": "Add", "options": [
                            { "type": 7, "name": "channel", "required": true },
                            { "type": 5, "name": "loud" },
                        ]},
                    ]},
                ]},
                { "id": "3", "application_id": "7", "type": 2, "name": "Inspect user" },
            ],
        })
    }

    #[test]
    fn index_is_flattened_and_sorted() {
        let names: Vec<_> = parse_index(&index()).iter().map(SlashCommand::full_name).collect();
        assert_eq!(names, ["alerts list", "alerts rules add", "forecast"]);
        assert_eq!(parse_index(&index())[2].app_name, "Weather");
        assert_eq!(parse_index(&index())[2].usage(), "city: [days:]");
    }

    #[test]
    fn named_and_positional_arguments() {
        let forecast = &parse_index(&index())[2];
        let named = parse_args(&forecast.options, "days: 3 city: New York").unwrap();
        assert_eq!(named, vec![
            json!({ "type": 3, "name": "city", "value": "New York" }),
            json!({ "type": 4, "name": "days", "value": 3 }),
        ]);
        let positional = parse_args(&forecast.options, "Paris 5").unwrap();
        assert_eq!(positional[0]["value"], "Paris");
        assert_eq!(positional[1]["value"], 5);
        assert_eq!(parse_args(&forecast.options, "Paris").unwrap().len(), 1);
    }

    #[test]
    fn argument_errors_are_reported() {
        let forecast = &parse_index(&index())[2];
        assert!(parse_args(&forecast.options, "").unwrap_err().contains("city"));
        assert!(parse_args(&forecast.options, "city: Rome days: soon").unwrap_err().contains("whole number"));
    }

    #[test]
    fn subcommands_nest_in_the_payload() {
        let add = &parse_index(&index())[1];
        let data = interaction_data(add, "channel: <#555> loud: yes").unwrap();
        assert_eq!(data["name"], "alerts");
        let group = &data["options"][0];
        assert_eq!((group["type"].as_u64(), group["name"].as_str()), (Some(2), Some("rules")));
        let sub = &group["options"][0];
        assert_eq!((sub["type"].as_u64(), sub["name"].as_str()), (Some(1), Some("add")));
        assert_eq!(sub["options"][0]["value"], "555");
        assert_eq!(sub["options"][1]["value"], true);
    }
}
