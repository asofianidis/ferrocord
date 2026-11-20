//! Scheduled server events.

use super::model::Id;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde::Deserialize;
use serde_json::{Value, json};

pub const STAGE: u8 = 1;
pub const VOICE: u8 = 2;
pub const EXTERNAL: u8 = 3;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ScheduledEvent {
    pub id: Id,
    #[serde(default)]
    pub guild_id: Option<Id>,
    #[serde(default)]
    pub channel_id: Option<Id>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub scheduled_start_time: String,
    #[serde(default)]
    pub scheduled_end_time: Option<String>,
    /// 1 scheduled, 2 active, 3 completed, 4 cancelled.
    #[serde(default)]
    pub status: u8,
    #[serde(default)]
    pub entity_type: u8,
    #[serde(default)]
    pub entity_metadata: Option<Value>,
    #[serde(default)]
    pub user_count: Option<u32>,
}

impl ScheduledEvent {
    pub fn start(&self) -> Option<DateTime<Local>> {
        DateTime::parse_from_rfc3339(&self.scheduled_start_time).ok().map(|t| t.with_timezone(&Local))
    }

    pub fn end(&self) -> Option<DateTime<Local>> {
        self.scheduled_end_time.as_deref().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Local))
    }

    pub fn location(&self) -> Option<String> {
        self.entity_metadata.as_ref().and_then(|m| m["location"].as_str()).map(str::to_string)
    }

    pub fn is_live(&self) -> bool {
        self.status == 2
    }

    /// Still worth showing: scheduled or running.
    pub fn is_upcoming(&self) -> bool {
        matches!(self.status, 1 | 2)
    }
}

/// Parses `2026-12-24 18:30` in local time.
pub fn parse_local(text: &str) -> Option<DateTime<Local>> {
    let naive = NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M").ok()?;
    Local.from_local_datetime(&naive).single()
}

/// Where a new event happens.
pub enum Place {
    Channel { id: Id, stage: bool },
    Elsewhere { location: String, end: DateTime<Local> },
}

pub fn create_body(name: &str, description: &str, start: DateTime<Local>, place: &Place) -> Value {
    let mut body = json!({
        "name": name,
        "description": description,
        "privacy_level": 2,
        "scheduled_start_time": start.to_rfc3339(),
    });
    match place {
        Place::Channel { id, stage } => {
            body["entity_type"] = json!(if *stage { STAGE } else { VOICE });
            body["channel_id"] = json!(id.to_string());
        }
        Place::Elsewhere { location, end } => {
            body["entity_type"] = json!(EXTERNAL);
            body["entity_metadata"] = json!({ "location": location });
            body["scheduled_end_time"] = json!(end.to_rfc3339());
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_parse_and_build() {
        let event: ScheduledEvent = serde_json::from_value(json!({
            "id": "5", "name": "Release party", "scheduled_start_time": "2026-12-24T18:30:00+00:00",
            "status": 1, "entity_type": 3, "entity_metadata": { "location": "The pub" }, "user_count": 4,
        }))
        .unwrap();
        assert_eq!(event.location().as_deref(), Some("The pub"));
        assert!(event.start().is_some() && event.end().is_none());
        assert!(event.is_upcoming() && !event.is_live());

        let start = parse_local("2026-12-24 18:30").unwrap();
        assert!(parse_local("tomorrow").is_none());
        let body = create_body("Party", "", start, &Place::Elsewhere { location: "Pub".into(), end: start + chrono::Duration::hours(2) });
        assert_eq!(body["entity_type"], 3);
        assert_eq!(body["entity_metadata"]["location"], "Pub");
        let voice = create_body("Call", "", start, &Place::Channel { id: Id(9), stage: false });
        assert_eq!((voice["entity_type"].as_u64(), voice["channel_id"].as_str()), (Some(2), Some("9")));
    }
}
