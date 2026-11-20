//! Discord gateway (websocket) connection: identify, heartbeat, resume and
//! automatic reconnection. Events are forwarded to the UI over a channel.

use super::model::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Message as WsMessage, protocol::WebSocketConfig};

const DEFAULT_GATEWAY: &str = "wss://gateway.discord.gg";

mod intents {
    pub const GUILDS: u64 = 1 << 0;
    pub const GUILD_MESSAGES: u64 = 1 << 9;
    pub const GUILD_MESSAGE_REACTIONS: u64 = 1 << 10;
    pub const GUILD_MESSAGE_TYPING: u64 = 1 << 11;
    pub const DIRECT_MESSAGES: u64 = 1 << 12;
    pub const DIRECT_MESSAGE_REACTIONS: u64 = 1 << 13;
    pub const DIRECT_MESSAGE_TYPING: u64 = 1 << 14;
    pub const MESSAGE_CONTENT: u64 = 1 << 15;
}

#[derive(Debug, Default)]
pub struct Ready {
    pub user: User,
    pub guilds: Vec<Guild>,
    /// The current user's role ids, per guild.
    pub own_roles: Vec<(Id, Vec<Id>)>,
    pub private_channels: Vec<Channel>,
    pub read_states: Vec<ReadState>,
    pub presences: Vec<(Id, Status)>,
}

#[derive(Debug)]
pub enum Event {
    Connecting,
    Ready(Box<Ready>),
    Resumed,
    /// The connection dropped; a reconnect is in progress.
    Reconnecting,
    /// The session cannot continue (bad token, etc).
    Fatal(String),
    GuildCreate(Box<Guild>),
    GuildDelete(Id),
    ChannelUpsert(Box<Channel>),
    ChannelDelete(Id),
    MessageCreate(Box<Message>),
    /// Partial message object.
    MessageUpdate(Value),
    MessageDelete { channel_id: Id, ids: Vec<Id> },
    ReactionAdd { channel_id: Id, message_id: Id, user_id: Id, emoji: Emoji },
    ReactionRemove { channel_id: Id, message_id: Id, user_id: Id, emoji: Emoji },
    ReactionClear { channel_id: Id, message_id: Id },
    Typing { channel_id: Id, user_id: Id, member: Option<Member> },
    Presence { user_id: Id, status: Status },
    MessageAck { channel_id: Id, message_id: Id },
    MemberList(Value),
    MembersChunk { guild_id: Id, members: Vec<Member>, presences: Vec<(Id, Status)> },
    OwnMemberUpdate { guild_id: Id, roles: Vec<Id> },
}

#[derive(Debug)]
pub enum Command {
    /// Ask the gateway for the member list of a guild channel.
    SubscribeMembers { guild_id: Id, channel_id: Id },
    Shutdown,
}

enum Outcome {
    Shutdown,
    Fatal(String),
    Reconnect { resumable: bool, reason: String },
    /// The privileged message-content intent was refused; retry without it.
    DropPrivilegedIntents,
}

struct Session {
    id: String,
    resume_url: String,
}

struct State {
    token: String,
    is_bot: bool,
    intents: u64,
    seq: Option<u64>,
    session: Option<Session>,
    user_id: Id,
}

pub async fn run(
    token: String,
    is_bot: bool,
    events: async_channel::Sender<Event>,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    use intents::*;
    let mut state = State {
        token,
        is_bot,
        intents: GUILDS
            | GUILD_MESSAGES
            | GUILD_MESSAGE_REACTIONS
            | GUILD_MESSAGE_TYPING
            | DIRECT_MESSAGES
            | DIRECT_MESSAGE_REACTIONS
            | DIRECT_MESSAGE_TYPING
            | MESSAGE_CONTENT,
        seq: None,
        session: None,
        user_id: Id(0),
    };
    let mut backoff = Duration::from_secs(1);

    loop {
        let _ = events.send(Event::Connecting).await;
        let started = tokio::time::Instant::now();
        match connect(&mut state, &events, &mut commands).await {
            Outcome::Shutdown => return,
            Outcome::Fatal(msg) => {
                let _ = events.send(Event::Fatal(msg)).await;
                return;
            }
            Outcome::DropPrivilegedIntents => {
                log::warn!("message content intent refused; continuing without it");
                state.intents &= !MESSAGE_CONTENT;
                state.session = None;
                state.seq = None;
                continue;
            }
            Outcome::Reconnect { resumable, reason } => {
                log::warn!("gateway disconnected: {reason} (resumable: {resumable})");
                if !resumable {
                    state.session = None;
                    state.seq = None;
                }
                if events.send(Event::Reconnecting).await.is_err() {
                    return;
                }
            }
        }
        // A connection that lasted a while earns a fresh backoff.
        if started.elapsed() > Duration::from_secs(30) {
            backoff = Duration::from_secs(1);
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn connect(
    state: &mut State,
    events: &async_channel::Sender<Event>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
) -> Outcome {
    let base = state
        .session
        .as_ref()
        .map(|s| s.resume_url.clone())
        .unwrap_or_else(|| DEFAULT_GATEWAY.to_string());
    let url = format!("{}/?v=10&encoding=json", base.trim_end_matches('/'));
    let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);

    let reconnect = |reason: String| Outcome::Reconnect { resumable: true, reason };

    let mut ws = match tokio_tungstenite::connect_async_with_config(url, Some(config), false).await {
        Ok((ws, _)) => ws,
        Err(e) => return reconnect(format!("connect failed: {e}")),
    };

    // The first frame must be Hello (op 10).
    let interval_ms = loop {
        match ws.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                let v: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                if v["op"] == 10 {
                    break v["d"]["heartbeat_interval"].as_u64().unwrap_or(41_250);
                }
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return reconnect(format!("handshake failed: {e}")),
            None => return reconnect("closed during handshake".into()),
        }
    };

    let hello = match &state.session {
        Some(session) => json!({
            "op": 6,
            "d": { "token": auth_token(state), "session_id": session.id, "seq": state.seq }
        }),
        None => identify(state),
    };
    if let Err(e) = ws.send(WsMessage::text(hello.to_string())).await {
        return reconnect(format!("identify failed: {e}"));
    }

    let mut heartbeat = tokio::time::interval(Duration::from_millis(interval_ms));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await; // the first tick completes immediately
    let mut acked = true;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if !acked {
                    return reconnect("heartbeat timed out".into());
                }
                acked = false;
                let beat = json!({ "op": 1, "d": state.seq });
                if let Err(e) = ws.send(WsMessage::text(beat.to_string())).await {
                    return reconnect(format!("send failed: {e}"));
                }
            }
            command = commands.recv() => {
                match command {
                    None | Some(Command::Shutdown) => {
                        let _ = ws.close(None).await;
                        return Outcome::Shutdown;
                    }
                    Some(Command::SubscribeMembers { guild_id, channel_id }) => {
                        let payload = if state.is_bot {
                            json!({ "op": 8, "d": {
                                "guild_id": guild_id.to_string(), "query": "", "limit": 100, "presences": false
                            }})
                        } else {
                            let mut channels = serde_json::Map::new();
                            channels.insert(channel_id.to_string(), json!([[0, 99]]));
                            json!({ "op": 14, "d": {
                                "guild_id": guild_id.to_string(),
                                "typing": true, "activities": false, "threads": true,
                                "channels": channels
                            }})
                        };
                        if let Err(e) = ws.send(WsMessage::text(payload.to_string())).await {
                            return reconnect(format!("send failed: {e}"));
                        }
                    }
                }
            }
            frame = ws.next() => {
                let text = match frame {
                    Some(Ok(WsMessage::Text(text))) => text,
                    Some(Ok(WsMessage::Close(frame))) => {
                        let (code, why) = frame
                            .map(|f| (u16::from(f.code), f.reason.to_string()))
                            .unwrap_or((1000, String::new()));
                        return close_outcome(code, &why);
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return reconnect(format!("socket error: {e}")),
                    None => return reconnect("socket closed".into()),
                };
                let mut v: Value = match serde_json::from_str(text.as_str()) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("unparseable gateway frame: {e}");
                        continue;
                    }
                };
                match v["op"].as_u64().unwrap_or(u64::MAX) {
                    0 => {
                        if let Some(seq) = v["s"].as_u64() {
                            state.seq = Some(seq);
                        }
                        let kind = v["t"].as_str().unwrap_or("").to_string();
                        let data = v["d"].take();
                        for event in dispatch(state, &kind, data) {
                            if events.send(event).await.is_err() {
                                return Outcome::Shutdown;
                            }
                        }
                    }
                    1 => {
                        let beat = json!({ "op": 1, "d": state.seq });
                        let _ = ws.send(WsMessage::text(beat.to_string())).await;
                    }
                    7 => return reconnect("server requested reconnect".into()),
                    9 => {
                        let resumable = v["d"].as_bool().unwrap_or(false);
                        // Discord asks clients to wait briefly before re-identifying.
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        return Outcome::Reconnect { resumable, reason: "invalid session".into() };
                    }
                    11 => acked = true,
                    _ => {}
                }
            }
        }
    }
}

fn auth_token(state: &State) -> String {
    let token = state.token.trim();
    if state.is_bot && !token.starts_with("Bot ") {
        format!("Bot {token}")
    } else {
        token.to_string()
    }
}

fn identify(state: &State) -> Value {
    let properties = json!({
        "os": std::env::consts::OS,
        "browser": "native-client",
        "device": "native-client",
    });
    if state.is_bot {
        json!({ "op": 2, "d": {
            "token": auth_token(state),
            "intents": state.intents,
            "properties": properties,
            "large_threshold": 50,
        }})
    } else {
        json!({ "op": 2, "d": {
            "token": auth_token(state),
            "properties": properties,
            "compress": false,
            "large_threshold": 50,
            "presence": { "status": "online", "since": 0, "activities": [], "afk": false },
        }})
    }
}

fn close_outcome(code: u16, why: &str) -> Outcome {
    let reason = format!("closed with code {code} {why}");
    match code {
        4004 => Outcome::Fatal("Authentication failed. The token is invalid or has expired.".into()),
        4014 => Outcome::DropPrivilegedIntents,
        4010 | 4011 | 4012 | 4013 => Outcome::Fatal(format!("The gateway rejected the connection ({code}).")),
        4007 | 4009 => Outcome::Reconnect { resumable: false, reason },
        _ => Outcome::Reconnect { resumable: true, reason },
    }
}

fn id_of(v: &Value) -> Option<Id> {
    match v {
        Value::String(s) => s.parse().ok().map(Id),
        Value::Number(n) => n.as_u64().map(Id),
        _ => None,
    }
}

fn parse<T: serde::de::DeserializeOwned>(v: Value, what: &str) -> Option<T> {
    match serde_json::from_value(v) {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("failed to parse {what}: {e}");
            None
        }
    }
}

fn parse_presences(v: &Value) -> Vec<(Id, Status)> {
    v.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    let id = id_of(&p["user"]["id"]).or_else(|| id_of(&p["user_id"]))?;
                    Some((id, Status::parse(p["status"].as_str().unwrap_or("offline"))))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn dispatch(state: &mut State, kind: &str, mut d: Value) -> Vec<Event> {
    let one = |e: Option<Event>| e.into_iter().collect::<Vec<_>>();
    match kind {
        "READY" => {
            if let (Some(id), Some(url)) = (d["session_id"].as_str(), d["resume_gateway_url"].as_str()) {
                state.session = Some(Session { id: id.to_string(), resume_url: url.to_string() });
            }
            let ready = parse_ready(d);
            state.user_id = ready.user.id;
            vec![Event::Ready(Box::new(ready))]
        }
        "RESUMED" => vec![Event::Resumed],
        "GUILD_CREATE" | "GUILD_UPDATE" => {
            let Some(mut guild) = Guild::from_value(d) else { return vec![] };
            if kind == "GUILD_UPDATE" || guild.unavailable {
                // Updates carry no channel list; the store merges metadata only.
                guild.unavailable = kind != "GUILD_UPDATE";
                if guild.unavailable {
                    return vec![];
                }
            }
            for c in guild.channels.iter_mut().chain(guild.threads.iter_mut()) {
                c.guild_id = Some(guild.id);
            }
            let mut out = Vec::new();
            if let Some(me) = guild.members.iter().find(|m| m.user.as_ref().map(|u| u.id) == Some(state.user_id)) {
                out.push(Event::OwnMemberUpdate { guild_id: guild.id, roles: me.roles.clone() });
            }
            out.insert(0, Event::GuildCreate(Box::new(guild)));
            out
        }
        "GUILD_DELETE" => {
            if d["unavailable"].as_bool().unwrap_or(false) {
                return vec![];
            }
            one(id_of(&d["id"]).map(Event::GuildDelete))
        }
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" | "THREAD_CREATE" | "THREAD_UPDATE" => {
            one(parse::<Channel>(d, kind).map(|c| Event::ChannelUpsert(Box::new(c))))
        }
        "CHANNEL_DELETE" | "THREAD_DELETE" => one(id_of(&d["id"]).map(Event::ChannelDelete)),
        "MESSAGE_CREATE" => one(parse::<Message>(d, kind).map(|m| Event::MessageCreate(Box::new(m)))),
        "MESSAGE_UPDATE" => vec![Event::MessageUpdate(d)],
        "MESSAGE_DELETE" => match (id_of(&d["channel_id"]), id_of(&d["id"])) {
            (Some(channel_id), Some(id)) => vec![Event::MessageDelete { channel_id, ids: vec![id] }],
            _ => vec![],
        },
        "MESSAGE_DELETE_BULK" => {
            let ids = d["ids"].as_array().map(|a| a.iter().filter_map(id_of).collect()).unwrap_or_default();
            one(id_of(&d["channel_id"]).map(|channel_id| Event::MessageDelete { channel_id, ids }))
        }
        "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
            let (Some(channel_id), Some(message_id), Some(user_id)) =
                (id_of(&d["channel_id"]), id_of(&d["message_id"]), id_of(&d["user_id"]))
            else {
                return vec![];
            };
            let Some(emoji) = parse::<Emoji>(d["emoji"].take(), "emoji") else { return vec![] };
            if kind == "MESSAGE_REACTION_ADD" {
                vec![Event::ReactionAdd { channel_id, message_id, user_id, emoji }]
            } else {
                vec![Event::ReactionRemove { channel_id, message_id, user_id, emoji }]
            }
        }
        "MESSAGE_REACTION_REMOVE_ALL" => match (id_of(&d["channel_id"]), id_of(&d["message_id"])) {
            (Some(channel_id), Some(message_id)) => vec![Event::ReactionClear { channel_id, message_id }],
            _ => vec![],
        },
        "TYPING_START" => match (id_of(&d["channel_id"]), id_of(&d["user_id"])) {
            (Some(channel_id), Some(user_id)) => {
                let member = serde_json::from_value(d["member"].take()).ok();
                vec![Event::Typing { channel_id, user_id, member }]
            }
            _ => vec![],
        },
        "PRESENCE_UPDATE" => one(id_of(&d["user"]["id"]).map(|user_id| Event::Presence {
            user_id,
            status: Status::parse(d["status"].as_str().unwrap_or("offline")),
        })),
        "MESSAGE_ACK" => match (id_of(&d["channel_id"]), id_of(&d["message_id"])) {
            (Some(channel_id), Some(message_id)) => vec![Event::MessageAck { channel_id, message_id }],
            _ => vec![],
        },
        "GUILD_MEMBER_LIST_UPDATE" => vec![Event::MemberList(d)],
        "GUILD_MEMBERS_CHUNK" => {
            let Some(guild_id) = id_of(&d["guild_id"]) else { return vec![] };
            let presences = parse_presences(&d["presences"]);
            let members = parse::<Vec<Member>>(d["members"].take(), "members").unwrap_or_default();
            vec![Event::MembersChunk { guild_id, members, presences }]
        }
        "GUILD_MEMBER_UPDATE" => {
            if id_of(&d["user"]["id"]) != Some(state.user_id) {
                return vec![];
            }
            let Some(guild_id) = id_of(&d["guild_id"]) else { return vec![] };
            let roles = d["roles"].as_array().map(|a| a.iter().filter_map(id_of).collect()).unwrap_or_default();
            vec![Event::OwnMemberUpdate { guild_id, roles }]
        }
        _ => vec![],
    }
}

pub fn parse_ready(mut d: Value) -> Ready {
    let user: User = parse(d["user"].take(), "ready user").unwrap_or_default();

    // User accounts receive a deduplicated `users` array that private
    // channels reference through `recipient_ids`.
    let users: Vec<User> = serde_json::from_value(d["users"].take()).unwrap_or_default();

    let mut own_roles = Vec::new();
    let merged = d["merged_members"].take();
    let mut guilds = Vec::new();
    if let Value::Array(raw) = d["guilds"].take() {
        for (i, g) in raw.into_iter().enumerate() {
            if g["unavailable"].as_bool().unwrap_or(false) {
                continue; // bots: the real payload arrives as GUILD_CREATE
            }
            let Some(mut guild) = Guild::from_value(g) else { continue };
            for c in guild.channels.iter_mut().chain(guild.threads.iter_mut()) {
                c.guild_id = Some(guild.id);
            }
            let mut roles = guild
                .members
                .iter()
                .find(|m| m.user.as_ref().map(|u| u.id) == Some(user.id) || m.user_id == Some(user.id))
                .map(|m| m.roles.clone());
            if roles.is_none() {
                if let Some(list) = merged.get(i).and_then(|m| m.as_array()) {
                    roles = list
                        .iter()
                        .find(|m| id_of(&m["user_id"]) == Some(user.id) || id_of(&m["user"]["id"]) == Some(user.id))
                        .or(list.first())
                        .map(|m| m["roles"].as_array().map(|a| a.iter().filter_map(id_of).collect()).unwrap_or_default());
                }
            }
            own_roles.push((guild.id, roles.unwrap_or_default()));
            guilds.push(guild);
        }
    }

    let mut private_channels: Vec<Channel> =
        parse(d["private_channels"].take(), "private channels").unwrap_or_default();
    for channel in &mut private_channels {
        if channel.recipients.is_empty() {
            channel.recipients = channel
                .recipient_ids
                .iter()
                .filter_map(|id| users.iter().find(|u| u.id == *id).cloned())
                .collect();
        }
    }

    // `read_state` is either a bare array or `{ entries: [...] }`.
    let mut read_state = d["read_state"].take();
    if read_state.is_object() {
        read_state = read_state["entries"].take();
    }
    let read_states = match read_state {
        Value::Array(list) => list.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
        _ => Vec::new(),
    };

    Ready {
        user,
        guilds,
        own_roles,
        private_channels,
        read_states,
        presences: parse_presences(&d["presences"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State { token: "t".into(), is_bot: true, intents: 0, seq: None, session: None, user_id: Id(1) }
    }

    #[test]
    fn bot_ready_defers_guilds_to_guild_create() {
        let mut state = state();
        let events = dispatch(
            &mut state,
            "READY",
            json!({
                "user": { "id": "1", "username": "bot", "bot": true },
                "session_id": "abc",
                "resume_gateway_url": "wss://resume.example",
                "guilds": [{ "id": "10", "unavailable": true }],
            }),
        );
        let [Event::Ready(ready)] = events.as_slice() else { panic!("expected READY") };
        assert!(ready.guilds.is_empty());
        assert_eq!(state.session.as_ref().unwrap().resume_url, "wss://resume.example");

        let events = dispatch(
            &mut state,
            "GUILD_CREATE",
            json!({
                "id": "10",
                "name": "Guild",
                "roles": [{ "id": "10", "name": "@everyone", "permissions": "1024" }],
                "channels": [{ "id": "11", "type": 0, "name": "general", "permission_overwrites": [
                    { "id": "10", "type": 0, "allow": "0", "deny": "1024" }
                ]}],
                "members": [{ "user": { "id": "1", "username": "bot" }, "roles": ["12"] }],
            }),
        );
        assert_eq!(events.len(), 2);
        let Event::GuildCreate(guild) = &events[0] else { panic!("expected guild") };
        assert_eq!(guild.channels[0].guild_id, Some(Id(10)));
        assert_eq!(guild.channels[0].permission_overwrites[0].deny, 1024);
        assert!(matches!(&events[1], Event::OwnMemberUpdate { roles, .. } if roles == &vec![Id(12)]));
    }

    #[test]
    fn user_ready_handles_nested_properties_and_deduplicated_users() {
        let ready = parse_ready(json!({
            "user": { "id": "1", "username": "me", "global_name": "Me" },
            "users": [{ "id": "2", "username": "friend", "global_name": null }],
            "guilds": [{
                "id": "10",
                "properties": { "name": "Nested", "icon": "abc", "owner_id": "2" },
                "roles": [{ "id": "10", "name": "@everyone", "permissions": 1024 }],
                "channels": [{ "id": "11", "type": 0, "name": "general", "topic": null }],
                "threads": [{ "id": "12", "type": 11, "name": "thread", "parent_id": "11" }],
            }],
            "merged_members": [[{ "user_id": "1", "roles": ["20", "21"] }]],
            "private_channels": [{ "id": "30", "type": 1, "recipient_ids": ["2"], "last_message_id": "99" }],
            "read_state": { "version": 1, "partial": false, "entries": [
                { "id": "11", "last_message_id": "50", "mention_count": 3 },
                { "id": "30", "last_message_id": null }
            ]},
            "presences": [{ "user_id": "2", "status": "idle" }],
        }));
        assert_eq!(ready.user.display_name(), "Me");
        assert_eq!(ready.guilds[0].name, "Nested");
        assert_eq!(ready.guilds[0].owner_id, Some(Id(2)));
        assert_eq!(ready.guilds[0].threads[0].guild_id, Some(Id(10)));
        assert_eq!(ready.own_roles, vec![(Id(10), vec![Id(20), Id(21)])]);
        assert_eq!(ready.private_channels[0].recipients[0].username, "friend");
        assert_eq!(ready.read_states.len(), 2);
        assert_eq!(ready.read_states[0].mention_count, 3);
        assert_eq!(ready.presences, vec![(Id(2), Status::Idle)]);
    }

    #[test]
    fn message_create_tolerates_nulls_and_unknown_fields() {
        let mut state = state();
        let events = dispatch(
            &mut state,
            "MESSAGE_CREATE",
            json!({
                "id": "100", "channel_id": "11", "guild_id": "10",
                "author": { "id": "2", "username": "friend", "avatar": null, "discriminator": "0" },
                "member": { "nick": null, "roles": [] },
                "content": "hi", "timestamp": "2026-01-02T03:04:05.000000+00:00", "edited_timestamp": null,
                "attachments": [], "embeds": [], "mentions": [], "mention_roles": [], "type": 0,
                "referenced_message": null, "nonce": "123", "some_future_field": { "x": 1 },
            }),
        );
        let [Event::MessageCreate(m)] = events.as_slice() else { panic!("expected message") };
        assert_eq!(m.content, "hi");
        assert!(m.referenced_message.is_none());
        assert_eq!(m.time().timestamp(), 1_767_323_045);
    }

    #[test]
    fn close_codes_map_to_the_right_recovery() {
        assert!(matches!(close_outcome(4004, ""), Outcome::Fatal(_)));
        assert!(matches!(close_outcome(4014, ""), Outcome::DropPrivilegedIntents));
        assert!(matches!(close_outcome(4009, ""), Outcome::Reconnect { resumable: false, .. }));
        assert!(matches!(close_outcome(1006, ""), Outcome::Reconnect { resumable: true, .. }));
    }
}
