//! Discord API data models. Deserialization is deliberately lenient: every
//! field that is not strictly required is defaulted so that payload changes on
//! Discord's side do not break the client.

// Models mirror the wire format; not every field is consumed yet.
#![allow(dead_code)]

use chrono::{DateTime, Local, TimeZone, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// A Discord snowflake. Serialized as a string on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(pub u64);

const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

impl Id {
    pub fn timestamp_ms(self) -> u64 {
        (self.0 >> 22) + DISCORD_EPOCH_MS
    }

    pub fn time(self) -> DateTime<Local> {
        Utc.timestamp_millis_opt(self.timestamp_ms() as i64)
            .single()
            .unwrap_or_else(Utc::now)
            .with_timezone(&Local)
    }

    /// A snowflake for "now", usable as a message nonce.
    pub fn now() -> Id {
        let ms = Utc::now().timestamp_millis().max(0) as u64;
        Id(ms.saturating_sub(DISCORD_EPOCH_MS) << 22)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.0)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            S(String),
            N(u64),
        }
        match Raw::deserialize(d)? {
            Raw::S(s) => s.parse().map(Id).map_err(serde::de::Error::custom),
            Raw::N(n) => Ok(Id(n)),
        }
    }
}

/// Accepts a u64 encoded as either a JSON string or number (permission bitfields).
fn de_bits<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        S(String),
        N(u64),
        Null,
    }
    Ok(match Raw::deserialize(d)? {
        Raw::S(s) => s.parse().unwrap_or(0),
        Raw::N(n) => n,
        Raw::Null => 0,
    })
}

/// Treats JSON `null` as the type's default value.
fn de_null<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

pub fn parse_time(s: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Local))
}

const CDN: &str = "https://cdn.discordapp.com";

#[derive(Clone, Debug, Default, Deserialize)]
pub struct User {
    pub id: Id,
    #[serde(default, deserialize_with = "de_null")]
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub discriminator: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default, deserialize_with = "de_null")]
    pub bot: bool,
}

impl User {
    pub fn display_name(&self) -> &str {
        match &self.global_name {
            Some(n) if !n.is_empty() => n,
            _ => &self.username,
        }
    }

    pub fn avatar_url(&self, size: u32) -> String {
        match &self.avatar {
            Some(hash) => format!("{CDN}/avatars/{}/{hash}.png?size={size}", self.id),
            None => {
                let index = match self.discriminator.as_deref() {
                    Some(d) if d != "0" && !d.is_empty() => d.parse::<u64>().unwrap_or(0) % 5,
                    _ => (self.id.0 >> 22) % 6,
                };
                format!("{CDN}/embed/avatars/{index}.png")
            }
        }
    }

    pub fn tag(&self) -> String {
        match self.discriminator.as_deref() {
            Some(d) if d != "0" && !d.is_empty() => format!("{}#{d}", self.username),
            _ => self.username.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub user_id: Option<Id>,
    #[serde(default)]
    pub nick: Option<String>,
    #[serde(default, deserialize_with = "de_null")]
    pub roles: Vec<Id>,
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Role {
    pub id: Id,
    #[serde(default, deserialize_with = "de_null")]
    pub name: String,
    #[serde(default, deserialize_with = "de_null")]
    pub color: u32,
    #[serde(default, deserialize_with = "de_null")]
    pub position: i32,
    #[serde(default, deserialize_with = "de_bits")]
    pub permissions: u64,
    #[serde(default, deserialize_with = "de_null")]
    pub hoist: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Overwrite {
    pub id: Id,
    /// 0 = role, 1 = member
    #[serde(default, rename = "type", deserialize_with = "de_overwrite_kind")]
    pub kind: u8,
    #[serde(default, deserialize_with = "de_bits")]
    pub allow: u64,
    #[serde(default, deserialize_with = "de_bits")]
    pub deny: u64,
}

fn de_overwrite_kind<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        N(u8),
        S(String),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::N(n) => n,
        Raw::S(s) => match s.as_str() {
            "member" | "1" => 1,
            _ => 0,
        },
    })
}

pub mod channel_kind {
    pub const TEXT: u8 = 0;
    pub const DM: u8 = 1;
    pub const VOICE: u8 = 2;
    pub const GROUP_DM: u8 = 3;
    pub const CATEGORY: u8 = 4;
    pub const ANNOUNCEMENT: u8 = 5;
    pub const ANNOUNCEMENT_THREAD: u8 = 10;
    pub const PUBLIC_THREAD: u8 = 11;
    pub const PRIVATE_THREAD: u8 = 12;
    pub const STAGE: u8 = 13;
    pub const FORUM: u8 = 15;
    pub const MEDIA: u8 = 16;
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Channel {
    pub id: Id,
    #[serde(default, rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub guild_id: Option<Id>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default, deserialize_with = "de_null")]
    pub position: i32,
    #[serde(default)]
    pub parent_id: Option<Id>,
    #[serde(default, deserialize_with = "de_null")]
    pub permission_overwrites: Vec<Overwrite>,
    #[serde(default)]
    pub last_message_id: Option<Id>,
    #[serde(default, deserialize_with = "de_null")]
    pub recipients: Vec<User>,
    #[serde(default, deserialize_with = "de_null")]
    pub recipient_ids: Vec<Id>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub owner_id: Option<Id>,
    #[serde(default, deserialize_with = "de_null")]
    pub message_count: u32,
}

impl Channel {
    pub fn is_text_like(&self) -> bool {
        use channel_kind::*;
        matches!(
            self.kind,
            TEXT | DM | GROUP_DM | ANNOUNCEMENT | ANNOUNCEMENT_THREAD | PUBLIC_THREAD | PRIVATE_THREAD | VOICE | STAGE
        )
    }

    pub fn is_forum(&self) -> bool {
        matches!(self.kind, channel_kind::FORUM | channel_kind::MEDIA)
    }

    pub fn is_thread(&self) -> bool {
        use channel_kind::*;
        matches!(self.kind, ANNOUNCEMENT_THREAD | PUBLIC_THREAD | PRIVATE_THREAD)
    }

    pub fn is_dm(&self) -> bool {
        matches!(self.kind, channel_kind::DM | channel_kind::GROUP_DM)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Emoji {
    #[serde(default)]
    pub id: Option<Id>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "de_null")]
    pub animated: bool,
}

impl Emoji {
    /// The identifier used in reaction REST routes.
    pub fn api_name(&self) -> String {
        match (self.id, &self.name) {
            (Some(id), Some(name)) => format!("{name}:{id}"),
            (Some(id), None) => format!("_:{id}"),
            (None, Some(name)) => name.clone(),
            (None, None) => String::new(),
        }
    }

    pub fn image_url(&self, size: u32) -> Option<String> {
        self.id.map(|id| {
            let ext = if self.animated { "gif" } else { "png" };
            format!("{CDN}/emojis/{id}.{ext}?size={size}")
        })
    }

    pub fn same_as(&self, other: &Emoji) -> bool {
        match (self.id, other.id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.name == other.name,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Guild {
    pub id: Id,
    #[serde(default, deserialize_with = "de_null")]
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub owner_id: Option<Id>,
    #[serde(default, deserialize_with = "de_null")]
    pub roles: Vec<Role>,
    #[serde(default, deserialize_with = "de_null")]
    pub channels: Vec<Channel>,
    #[serde(default, deserialize_with = "de_null")]
    pub threads: Vec<Channel>,
    #[serde(default, deserialize_with = "de_null")]
    pub members: Vec<Member>,
    #[serde(default, deserialize_with = "de_null")]
    pub emojis: Vec<Emoji>,
    #[serde(default, deserialize_with = "de_null")]
    pub member_count: u32,
    #[serde(default, deserialize_with = "de_null")]
    pub unavailable: bool,
    #[serde(default, deserialize_with = "de_null")]
    pub voice_states: Vec<VoiceState>,
    #[serde(default, deserialize_with = "de_null")]
    pub stickers: Vec<StickerItem>,
}

/// A GIF search result.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Gif {
    /// The link that is sent as the message.
    #[serde(default, deserialize_with = "de_null")]
    pub url: String,
    #[serde(default, deserialize_with = "de_null")]
    pub gif_src: String,
    #[serde(default, deserialize_with = "de_null")]
    pub src: String,
    #[serde(default, deserialize_with = "de_null")]
    pub preview: String,
    #[serde(default, deserialize_with = "de_null")]
    pub width: u32,
    #[serde(default, deserialize_with = "de_null")]
    pub height: u32,
}

impl Gif {
    pub fn thumbnail(&self) -> &str {
        [&self.preview, &self.gif_src, &self.src].into_iter().find(|s| !s.is_empty()).map_or("", |s| s.as_str())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct VoiceState {
    pub user_id: Id,
    #[serde(default)]
    pub channel_id: Option<Id>,
}

impl Guild {
    /// Parses a guild from either the bot-style payload (fields at the top
    /// level) or the user-account payload (fields nested under `properties`).
    pub fn from_value(mut v: serde_json::Value) -> Option<Guild> {
        if let Some(obj) = v.as_object_mut() {
            if let Some(serde_json::Value::Object(props)) = obj.remove("properties") {
                for (k, val) in props {
                    obj.entry(k).or_insert(val);
                }
            }
        }
        match serde_json::from_value(v) {
            Ok(g) => Some(g),
            Err(e) => {
                log::warn!("failed to parse guild: {e}");
                None
            }
        }
    }

    pub fn icon_url(&self, size: u32) -> Option<String> {
        self.icon
            .as_ref()
            .map(|hash| format!("{CDN}/icons/{}/{hash}.png?size={size}", self.id))
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Attachment {
    pub id: Id,
    #[serde(default, deserialize_with = "de_null")]
    pub filename: String,
    #[serde(default, deserialize_with = "de_null")]
    pub url: String,
    #[serde(default, deserialize_with = "de_null")]
    pub proxy_url: String,
    #[serde(default, deserialize_with = "de_null")]
    pub size: u64,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        if self.width.is_none() {
            return false;
        }
        match &self.content_type {
            Some(ct) => ct.starts_with("image/"),
            None => {
                let lower = self.filename.to_ascii_lowercase();
                [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| lower.ends_with(e))
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedMedia {
    #[serde(default, deserialize_with = "de_null")]
    pub url: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

impl EmbedMedia {
    pub fn best_url(&self) -> &str {
        self.proxy_url.as_deref().filter(|u| !u.is_empty()).unwrap_or(&self.url)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedAuthor {
    #[serde(default, deserialize_with = "de_null")]
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedFooter {
    #[serde(default, deserialize_with = "de_null")]
    pub text: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EmbedField {
    #[serde(default, deserialize_with = "de_null")]
    pub name: String,
    #[serde(default, deserialize_with = "de_null")]
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Embed {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub color: Option<u32>,
    #[serde(default)]
    pub author: Option<EmbedAuthor>,
    #[serde(default)]
    pub footer: Option<EmbedFooter>,
    #[serde(default)]
    pub image: Option<EmbedMedia>,
    #[serde(default)]
    pub thumbnail: Option<EmbedMedia>,
    #[serde(default, deserialize_with = "de_null")]
    pub fields: Vec<EmbedField>,
    #[serde(default)]
    pub provider: Option<EmbedAuthor>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Reaction {
    #[serde(default, deserialize_with = "de_null")]
    pub count: u32,
    #[serde(default, deserialize_with = "de_null")]
    pub me: bool,
    #[serde(default)]
    pub emoji: Emoji,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct StickerItem {
    pub id: Id,
    #[serde(default, deserialize_with = "de_null")]
    pub name: String,
    /// 1 = png, 2 = apng, 3 = lottie, 4 = gif
    #[serde(default, deserialize_with = "de_null")]
    pub format_type: u8,
}

impl StickerItem {
    pub fn image_url(&self) -> Option<String> {
        match self.format_type {
            1 | 2 => Some(format!("https://media.discordapp.net/stickers/{}.png?size=256", self.id)),
            4 => Some(format!("https://media.discordapp.net/stickers/{}.gif?size=256", self.id)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MessageReference {
    #[serde(default)]
    pub message_id: Option<Id>,
    #[serde(default)]
    pub channel_id: Option<Id>,
}

pub mod message_kind {
    pub const DEFAULT: u8 = 0;
    pub const RECIPIENT_ADD: u8 = 1;
    pub const RECIPIENT_REMOVE: u8 = 2;
    pub const CALL: u8 = 3;
    pub const CHANNEL_NAME_CHANGE: u8 = 4;
    pub const CHANNEL_ICON_CHANGE: u8 = 5;
    pub const PINNED: u8 = 6;
    pub const MEMBER_JOIN: u8 = 7;
    pub const BOOST: u8 = 8;
    pub const BOOST_TIER_1: u8 = 9;
    pub const BOOST_TIER_2: u8 = 10;
    pub const BOOST_TIER_3: u8 = 11;
    pub const THREAD_CREATED: u8 = 18;
    pub const REPLY: u8 = 19;
    pub const CHAT_INPUT_COMMAND: u8 = 20;
    pub const THREAD_STARTER: u8 = 21;
    pub const CONTEXT_MENU_COMMAND: u8 = 23;
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Message {
    pub id: Id,
    pub channel_id: Id,
    #[serde(default)]
    pub guild_id: Option<Id>,
    #[serde(default)]
    pub author: User,
    #[serde(default)]
    pub member: Option<Member>,
    #[serde(default, deserialize_with = "de_null")]
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default, deserialize_with = "de_null")]
    pub attachments: Vec<Attachment>,
    #[serde(default, deserialize_with = "de_null")]
    pub embeds: Vec<Embed>,
    #[serde(default, deserialize_with = "de_null")]
    pub reactions: Vec<Reaction>,
    #[serde(default, deserialize_with = "de_null")]
    pub mentions: Vec<User>,
    #[serde(default, deserialize_with = "de_null")]
    pub mention_roles: Vec<Id>,
    #[serde(default, deserialize_with = "de_null")]
    pub mention_everyone: bool,
    #[serde(default, deserialize_with = "de_null")]
    pub pinned: bool,
    #[serde(default, rename = "type", deserialize_with = "de_null")]
    pub kind: u8,
    #[serde(default)]
    pub referenced_message: Option<Box<Message>>,
    #[serde(default)]
    pub message_reference: Option<MessageReference>,
    #[serde(default, deserialize_with = "de_null")]
    pub sticker_items: Vec<StickerItem>,
    #[serde(default)]
    pub nonce: Option<serde_json::Value>,
    #[serde(default)]
    pub poll: Option<Poll>,
    #[serde(default, deserialize_with = "de_null")]
    pub message_snapshots: Vec<Snapshot>,
    #[serde(default, deserialize_with = "de_null")]
    pub components: Vec<Component>,
    #[serde(default, deserialize_with = "de_null")]
    pub flags: u64,
    #[serde(default)]
    pub application_id: Option<Id>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PollMedia {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub emoji: Option<Emoji>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PollAnswer {
    #[serde(default, deserialize_with = "de_null")]
    pub answer_id: u32,
    #[serde(default)]
    pub poll_media: PollMedia,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PollCount {
    #[serde(default, deserialize_with = "de_null")]
    pub id: u32,
    #[serde(default, deserialize_with = "de_null")]
    pub count: u32,
    #[serde(default, deserialize_with = "de_null")]
    pub me_voted: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PollResults {
    #[serde(default, deserialize_with = "de_null")]
    pub is_finalized: bool,
    #[serde(default, deserialize_with = "de_null")]
    pub answer_counts: Vec<PollCount>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Poll {
    #[serde(default)]
    pub question: PollMedia,
    #[serde(default, deserialize_with = "de_null")]
    pub answers: Vec<PollAnswer>,
    #[serde(default)]
    pub results: Option<PollResults>,
    #[serde(default, deserialize_with = "de_null")]
    pub allow_multiselect: bool,
}

impl Poll {
    pub fn count(&self, answer: u32) -> (u32, bool) {
        self.results
            .as_ref()
            .and_then(|r| r.answer_counts.iter().find(|c| c.id == answer))
            .map_or((0, false), |c| (c.count, c.me_voted))
    }

    /// Applies one vote change, creating the tally entry when needed.
    pub fn apply_vote(&mut self, answer: u32, added: bool, mine: bool) {
        let results = self.results.get_or_insert_with(Default::default);
        let index = match results.answer_counts.iter().position(|c| c.id == answer) {
            Some(index) => index,
            None => {
                results.answer_counts.push(PollCount { id: answer, ..Default::default() });
                results.answer_counts.len() - 1
            }
        };
        let entry = &mut results.answer_counts[index];
        entry.count = if added { entry.count + 1 } else { entry.count.saturating_sub(1) };
        if mine {
            entry.me_voted = added;
        }
    }
}

/// The content of a forwarded message.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SnapshotMessage {
    #[serde(default, deserialize_with = "de_null")]
    pub content: String,
    #[serde(default, deserialize_with = "de_null")]
    pub attachments: Vec<Attachment>,
    #[serde(default, deserialize_with = "de_null")]
    pub embeds: Vec<Embed>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub message: SnapshotMessage,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SelectOption {
    #[serde(default, deserialize_with = "de_null")]
    pub label: String,
    #[serde(default, deserialize_with = "de_null")]
    pub value: String,
}

pub mod component_kind {
    pub const ACTION_ROW: u8 = 1;
    pub const BUTTON: u8 = 2;
    pub const STRING_SELECT: u8 = 3;
    pub const TEXT_DISPLAY: u8 = 10;
    pub const LINK_STYLE: u8 = 5;
}

/// A message component: an action row, button, select menu or text block.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Component {
    #[serde(default, rename = "type", deserialize_with = "de_null")]
    pub kind: u8,
    #[serde(default, deserialize_with = "de_null")]
    pub components: Vec<Component>,
    #[serde(default, deserialize_with = "de_null")]
    pub style: u8,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub custom_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "de_null")]
    pub disabled: bool,
    #[serde(default)]
    pub emoji: Option<Emoji>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default, deserialize_with = "de_null")]
    pub options: Vec<SelectOption>,
    #[serde(default)]
    pub content: Option<String>,
}

impl Message {
    pub fn time(&self) -> DateTime<Local> {
        self.timestamp
            .as_deref()
            .and_then(parse_time)
            .unwrap_or_else(|| self.id.time())
    }

    pub fn is_system(&self) -> bool {
        use message_kind::*;
        !matches!(self.kind, DEFAULT | REPLY | CHAT_INPUT_COMMAND | CONTEXT_MENU_COMMAND | THREAD_STARTER)
    }

    pub fn mentions_user(&self, user: Id) -> bool {
        self.mention_everyone || self.mentions.iter().any(|u| u.id == user)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ReadState {
    pub id: Id,
    #[serde(default)]
    pub last_message_id: Option<Id>,
    #[serde(default, deserialize_with = "de_null")]
    pub mention_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    Online,
    Idle,
    Dnd,
    #[default]
    Offline,
}

impl Status {
    pub fn parse(s: &str) -> Status {
        match s {
            "online" => Status::Online,
            "idle" => Status::Idle,
            "dnd" => Status::Dnd,
            _ => Status::Offline,
        }
    }

    /// The value sent when setting our own presence.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Online => "online",
            Status::Idle => "idle",
            Status::Dnd => "dnd",
            Status::Offline => "invisible",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Online => "Online",
            Status::Idle => "Idle",
            Status::Dnd => "Do Not Disturb",
            Status::Offline => "Invisible",
        }
    }
}
