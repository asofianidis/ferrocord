//! Minimal Discord REST client with rate-limit handling.

use super::model::{Ban, Channel, Emoji, Gif, Id, Message, Role, StickerItem, User, Webhook};
use anyhow::{Result, anyhow};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const API: &str = "https://discord.com/api/v10";
const USER_AGENT: &str = concat!("DiscordBot (native-client, ", env!("CARGO_PKG_VERSION"), ")");

#[derive(Clone)]
pub struct Rest {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    auth: String,
    is_bot: bool,
}

pub use reqwest::Method as HttpMethod;

pub struct Upload {
    pub filename: String,
    pub bytes: Vec<u8>,
}

impl Rest {
    pub fn new(token: &str, is_bot: bool) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        let token = token.trim();
        let auth = if is_bot && !token.starts_with("Bot ") {
            format!("Bot {token}")
        } else {
            token.to_string()
        };
        Ok(Self { inner: Arc::new(Inner { http, auth, is_bot }) })
    }

    pub fn is_bot(&self) -> bool {
        self.inner.is_bot
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.inner
            .http
            .request(method, format!("{API}{path}"))
            .header("Authorization", &self.inner.auth)
    }

    /// Sends a request, retrying when rate limited.
    async fn send(&self, build: impl Fn() -> RequestBuilder) -> Result<Response> {
        let mut attempts = 0;
        loop {
            let resp = build().send().await?;
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS && attempts < 4 {
                attempts += 1;
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                let wait = body["retry_after"].as_f64().unwrap_or(1.0).clamp(0.05, 30.0);
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                continue;
            }
            if !status.is_success() {
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                let msg = body["message"].as_str().unwrap_or("request failed");
                return Err(anyhow!("{msg} (HTTP {})", status.as_u16()));
            }
            return Ok(resp);
        }
    }

    pub async fn messages(&self, channel: Id, before: Option<Id>, limit: u32) -> Result<Vec<Message>> {
        let mut path = format!("/channels/{channel}/messages?limit={limit}");
        if let Some(before) = before {
            path.push_str(&format!("&before={before}"));
        }
        let raw: Vec<Value> = self.send(|| self.request(Method::GET, &path)).await?.json().await?;
        // Newest first on the wire; skip any message we fail to understand.
        Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
    }

    pub async fn pins(&self, channel: Id) -> Result<Vec<Message>> {
        let path = format!("/channels/{channel}/pins");
        let raw: Vec<Value> = self.send(|| self.request(Method::GET, &path)).await?.json().await?;
        Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
    }

    fn message_payload(content: &str, reply_to: Option<Id>, nonce: Id) -> Value {
        let mut body = json!({ "content": content, "nonce": nonce.to_string(), "tts": false });
        if let Some(id) = reply_to {
            body["message_reference"] = json!({ "message_id": id.to_string() });
            body["allowed_mentions"] = json!({ "parse": ["users", "roles", "everyone"], "replied_user": true });
        }
        body
    }

    pub async fn send_message(
        &self,
        channel: Id,
        content: &str,
        reply_to: Option<Id>,
        nonce: Id,
        uploads: Vec<Upload>,
    ) -> Result<Message> {
        let path = format!("/channels/{channel}/messages");
        let payload = Self::message_payload(content, reply_to, nonce);
        let resp = if uploads.is_empty() {
            self.send(|| self.request(Method::POST, &path).json(&payload)).await?
        } else {
            let mut form = reqwest::multipart::Form::new().text("payload_json", payload.to_string());
            for (i, up) in uploads.into_iter().enumerate() {
                let part = reqwest::multipart::Part::bytes(up.bytes).file_name(up.filename);
                form = form.part(format!("files[{i}]"), part);
            }
            let resp = self.request(Method::POST, &path).multipart(form).send().await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                let msg = body["message"].as_str().unwrap_or("upload failed");
                return Err(anyhow!("{msg} (HTTP {})", status.as_u16()));
            }
            resp
        };
        Ok(resp.json().await?)
    }

    pub async fn edit_message(&self, channel: Id, message: Id, content: &str) -> Result<()> {
        let path = format!("/channels/{channel}/messages/{message}");
        let body = json!({ "content": content });
        self.send(|| self.request(Method::PATCH, &path).json(&body)).await?;
        Ok(())
    }

    pub async fn delete_message(&self, channel: Id, message: Id) -> Result<()> {
        let path = format!("/channels/{channel}/messages/{message}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    pub async fn set_pinned(&self, channel: Id, message: Id, pinned: bool) -> Result<()> {
        let path = format!("/channels/{channel}/pins/{message}");
        let method = if pinned { Method::PUT } else { Method::DELETE };
        self.send(|| self.request(method.clone(), &path).body("")).await?;
        Ok(())
    }

    pub async fn set_reaction(&self, channel: Id, message: Id, emoji: &str, on: bool) -> Result<()> {
        let path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}/@me",
            percent_encode(emoji)
        );
        let method = if on { Method::PUT } else { Method::DELETE };
        self.send(|| self.request(method.clone(), &path).body("")).await?;
        Ok(())
    }

    pub async fn typing(&self, channel: Id) -> Result<()> {
        let path = format!("/channels/{channel}/typing");
        self.send(|| self.request(Method::POST, &path).body("")).await?;
        Ok(())
    }

    /// Marks a channel as read up to `message`. User accounts only.
    pub async fn ack(&self, channel: Id, message: Id) -> Result<()> {
        if self.inner.is_bot {
            return Ok(());
        }
        let path = format!("/channels/{channel}/messages/{message}/ack");
        let body = json!({ "token": null });
        self.send(|| self.request(Method::POST, &path).json(&body)).await?;
        Ok(())
    }

    /// Full-text search. Only available to user accounts.
    pub async fn search(&self, guild: Option<Id>, channel: Id, query: &str) -> Result<Vec<Message>> {
        let scope = match guild {
            Some(guild) => format!("/guilds/{guild}"),
            None => format!("/channels/{channel}"),
        };
        let path = format!("{scope}/messages/search?content={}", percent_encode(query));
        let body: Value = self.send(|| self.request(Method::GET, &path)).await?.json().await?;
        let mut hits = Vec::new();
        for group in body["messages"].as_array().into_iter().flatten() {
            let group = group.as_array().cloned().unwrap_or_default();
            let hit = group.iter().find(|m| m["hit"].as_bool() == Some(true)).or(group.first());
            if let Some(m) = hit.and_then(|m| serde_json::from_value(m.clone()).ok()) {
                hits.push(m);
            }
        }
        Ok(hits)
    }

    /// Archived posts of a forum (active ones arrive over the gateway).
    pub async fn forum_threads(&self, forum: Id) -> Result<Vec<Channel>> {
        let path = if self.inner.is_bot {
            format!("/channels/{forum}/threads/archived/public?limit=50")
        } else {
            format!(
                "/channels/{forum}/threads/search?archived=true&sort_by=last_message_time&sort_order=desc&limit=25&offset=0"
            )
        };
        let mut body: Value = self.send(|| self.request(Method::GET, &path)).await?.json().await?;
        let threads = match body["threads"].take() {
            Value::Array(list) => list.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
            _ => Vec::new(),
        };
        Ok(threads)
    }

    pub async fn create_thread(&self, channel: Id, message: Id, name: &str) -> Result<Channel> {
        let path = format!("/channels/{channel}/messages/{message}/threads");
        let body = json!({ "name": name, "auto_archive_duration": 1440 });
        Ok(self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?)
    }

    pub async fn create_post(&self, forum: Id, title: &str, content: &str) -> Result<Channel> {
        let path = format!("/channels/{forum}/threads");
        let body = json!({ "name": title, "auto_archive_duration": 4320, "message": { "content": content } });
        Ok(self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?)
    }

    /// Replaces the current user's votes on a poll. User accounts only.
    pub async fn vote_poll(&self, channel: Id, message: Id, answers: &[u32]) -> Result<()> {
        let path = format!("/channels/{channel}/polls/{message}/answers/@me");
        let ids: Vec<String> = answers.iter().map(|a| a.to_string()).collect();
        let body = json!({ "answer_ids": ids });
        self.send(|| self.request(Method::PUT, &path).json(&body)).await?;
        Ok(())
    }

    /// Sends an interaction (button press, select choice, slash command).
    /// User accounts only; this endpoint is not part of the documented API.
    pub async fn interact(&self, payload: Value, uploads: Vec<Upload>) -> Result<()> {
        if uploads.is_empty() {
            self.send(|| self.request(Method::POST, "/interactions").json(&payload)).await?;
            return Ok(());
        }
        let mut form = reqwest::multipart::Form::new().text("payload_json", payload.to_string());
        for (i, up) in uploads.into_iter().enumerate() {
            form = form.part(format!("files[{i}]"), reqwest::multipart::Part::bytes(up.bytes).file_name(up.filename));
        }
        let resp = self.request(Method::POST, "/interactions").multipart(form).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            let msg = body["message"].as_str().unwrap_or("upload failed");
            return Err(anyhow!("{msg} (HTTP {})", status.as_u16()));
        }
        Ok(())
    }

    pub async fn add_friend(&self, username: &str) -> Result<()> {
        let body = json!({ "username": username });
        self.send(|| self.request(Method::POST, "/users/@me/relationships").json(&body)).await?;
        Ok(())
    }

    /// Accepts a friend request (or sends one, by id).
    pub async fn accept_friend(&self, user: Id) -> Result<()> {
        let path = format!("/users/@me/relationships/{user}");
        self.send(|| self.request(Method::PUT, &path).json(&json!({}))).await?;
        Ok(())
    }

    pub async fn block_user(&self, user: Id) -> Result<()> {
        let path = format!("/users/@me/relationships/{user}");
        self.send(|| self.request(Method::PUT, &path).json(&json!({ "type": 2 }))).await?;
        Ok(())
    }

    /// Removes a friend, block or pending request.
    pub async fn remove_relationship(&self, user: Id) -> Result<()> {
        let path = format!("/users/@me/relationships/{user}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    pub async fn create_group_dm(&self, users: &[Id]) -> Result<Channel> {
        let ids: Vec<String> = users.iter().map(|u| u.to_string()).collect();
        let body = json!({ "recipients": ids });
        Ok(self
            .send(|| self.request(Method::POST, "/users/@me/channels").json(&body))
            .await?
            .json()
            .await?)
    }

    /// Closes a DM, or leaves a group DM.
    pub async fn close_dm(&self, channel: Id) -> Result<()> {
        let path = format!("/channels/{channel}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    pub async fn leave_guild(&self, guild: Id) -> Result<()> {
        let path = format!("/users/@me/guilds/{guild}");
        self.send(|| self.request(Method::DELETE, &path).json(&json!({ "lurking": false }))).await?;
        Ok(())
    }

    /// Creates a one-day invite and returns its code.
    pub async fn create_invite(&self, channel: Id) -> Result<String> {
        let path = format!("/channels/{channel}/invites");
        let body = json!({ "max_age": 86400, "max_uses": 0, "temporary": false });
        let invite: Value = self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?;
        invite["code"].as_str().map(str::to_string).ok_or_else(|| anyhow!("no invite code returned"))
    }

    /// Mutes or unmutes a guild, or one channel inside it. User accounts only.
    pub async fn set_muted(&self, guild: Option<Id>, channel: Option<Id>, muted: bool) -> Result<()> {
        let scope = guild.map_or_else(|| "@me".to_string(), |g| g.to_string());
        let path = format!("/users/@me/guilds/{scope}/settings");
        let body = match channel {
            Some(channel) => {
                let mut overrides = serde_json::Map::new();
                overrides.insert(channel.to_string(), json!({ "muted": muted }));
                json!({ "channel_overrides": overrides })
            }
            None => json!({ "muted": muted }),
        };
        self.send(|| self.request(Method::PATCH, &path).json(&body)).await?;
        Ok(())
    }

    /// Searches GIFs, or lists trending ones for an empty query. User accounts only.
    pub async fn gifs(&self, query: &str) -> Result<Vec<Gif>> {
        let path = if query.is_empty() {
            "/gifs/trending-gifs?provider=tenor&locale=en-US&media_format=gif".to_string()
        } else {
            format!("/gifs/search?q={}&media_format=gif&provider=tenor&locale=en-US", percent_encode(query))
        };
        let body: Value = self.send(|| self.request(Method::GET, &path)).await?.json().await?;
        let list = body.as_array().cloned().unwrap_or_default();
        Ok(list.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
    }

    pub async fn send_sticker(&self, channel: Id, sticker: Id, nonce: Id) -> Result<Message> {
        let path = format!("/channels/{channel}/messages");
        let body = json!({ "content": "", "sticker_ids": [sticker.to_string()], "nonce": nonce.to_string() });
        Ok(self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?)
    }

    /// The slash commands available in a guild, or in a DM channel. User accounts only.
    pub async fn command_index(&self, guild: Option<Id>, channel: Id) -> Result<Value> {
        let path = match guild {
            Some(guild) => format!("/guilds/{guild}/application-command-index"),
            None => format!("/channels/{channel}/application-command-index"),
        };
        Ok(self.send(|| self.request(Method::GET, &path)).await?.json().await?)
    }

    pub async fn forward(&self, target: Id, source: Id, message: Id, guild: Option<Id>, nonce: Id) -> Result<Message> {
        let path = format!("/channels/{target}/messages");
        let mut reference = json!({ "type": 1, "message_id": message.to_string(), "channel_id": source.to_string() });
        if let Some(guild) = guild {
            reference["guild_id"] = json!(guild.to_string());
        }
        let body = json!({ "message_reference": reference, "nonce": nonce.to_string() });
        Ok(self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?)
    }

    /// Rings the other people in a direct-message call.
    pub async fn ring(&self, channel: Id) -> Result<()> {
        let path = format!("/channels/{channel}/call/ring");
        self.send(|| self.request(Method::POST, &path).json(&json!({ "recipients": null }))).await?;
        Ok(())
    }

    /// Updates a stage voice state: `user` is `None` for ourselves.
    pub async fn update_voice_state(&self, guild: Id, user: Option<Id>, body: Value) -> Result<()> {
        let who = user.map_or_else(|| "@me".to_string(), |u| u.to_string());
        let path = format!("/guilds/{guild}/voice-states/{who}");
        self.send(|| self.request(Method::PATCH, &path).json(&body)).await?;
        Ok(())
    }

    pub async fn start_stage(&self, channel: Id, topic: &str) -> Result<()> {
        let body = json!({ "channel_id": channel.to_string(), "topic": topic });
        self.send(|| self.request(Method::POST, "/stage-instances").json(&body)).await?;
        Ok(())
    }

    pub async fn end_stage(&self, channel: Id) -> Result<()> {
        let path = format!("/stage-instances/{channel}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    /// Changes a member: nickname, roles, server mute or deafen, voice channel, timeout.
    pub async fn modify_member(&self, guild: Id, user: Id, body: Value) -> Result<()> {
        let path = format!("/guilds/{guild}/members/{user}");
        self.send(|| self.request(Method::PATCH, &path).json(&body)).await?;
        Ok(())
    }

    // ------------------------------------------------------------ administration

    pub async fn modify_guild(&self, guild: Id, body: Value) -> Result<()> {
        let path = format!("/guilds/{guild}");
        self.send(|| self.request(Method::PATCH, &path).json(&body)).await?;
        Ok(())
    }

    pub async fn create_channel(&self, guild: Id, body: Value) -> Result<Channel> {
        let path = format!("/guilds/{guild}/channels");
        Ok(self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?)
    }

    pub async fn modify_channel(&self, channel: Id, body: Value) -> Result<Channel> {
        let path = format!("/channels/{channel}");
        Ok(self.send(|| self.request(Method::PATCH, &path).json(&body)).await?.json().await?)
    }

    pub async fn delete_channel(&self, channel: Id) -> Result<()> {
        let path = format!("/channels/{channel}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    /// Moves roles; `body` is a list of `{ id, position }`.
    pub async fn reorder_roles(&self, guild: Id, body: Value) -> Result<Vec<Role>> {
        let path = format!("/guilds/{guild}/roles");
        Ok(self.send(|| self.request(Method::PATCH, &path).json(&body)).await?.json().await?)
    }

    /// Creates or replaces a channel's permission override for a role or member.
    pub async fn set_overwrite(&self, channel: Id, target: Id, body: Value) -> Result<()> {
        let path = format!("/channels/{channel}/permissions/{target}");
        self.send(|| self.request(Method::PUT, &path).json(&body)).await?;
        Ok(())
    }

    pub async fn delete_overwrite(&self, channel: Id, target: Id) -> Result<()> {
        let path = format!("/channels/{channel}/permissions/{target}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    pub async fn create_emoji(&self, guild: Id, name: &str, image_uri: &str) -> Result<Emoji> {
        let path = format!("/guilds/{guild}/emojis");
        let body = json!({ "name": name, "image": image_uri });
        Ok(self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?)
    }

    pub async fn rename_emoji(&self, guild: Id, emoji: Id, name: &str) -> Result<Emoji> {
        let path = format!("/guilds/{guild}/emojis/{emoji}");
        let body = json!({ "name": name });
        Ok(self.send(|| self.request(Method::PATCH, &path).json(&body)).await?.json().await?)
    }

    pub async fn delete_emoji(&self, guild: Id, emoji: Id) -> Result<()> {
        let path = format!("/guilds/{guild}/emojis/{emoji}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    /// Uploads a sticker; `tags` is the emoji it is suggested for.
    pub async fn create_sticker(&self, guild: Id, name: &str, tags: &str, file: Upload) -> Result<StickerItem> {
        let path = format!("/guilds/{guild}/stickers");
        let form = reqwest::multipart::Form::new()
            .text("name", name.to_string())
            .text("description", String::new())
            .text("tags", tags.to_string())
            .part("file", reqwest::multipart::Part::bytes(file.bytes).file_name(file.filename));
        let resp = self.request(Method::POST, &path).multipart(form).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            let msg = body["message"].as_str().unwrap_or("upload failed");
            return Err(anyhow!("{msg} (HTTP {})", status.as_u16()));
        }
        Ok(resp.json().await?)
    }

    pub async fn rename_sticker(&self, guild: Id, sticker: Id, name: &str) -> Result<StickerItem> {
        let path = format!("/guilds/{guild}/stickers/{sticker}");
        let body = json!({ "name": name });
        Ok(self.send(|| self.request(Method::PATCH, &path).json(&body)).await?.json().await?)
    }

    pub async fn delete_sticker(&self, guild: Id, sticker: Id) -> Result<()> {
        let path = format!("/guilds/{guild}/stickers/{sticker}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    pub async fn webhooks(&self, guild: Id) -> Result<Vec<Webhook>> {
        let path = format!("/guilds/{guild}/webhooks");
        Ok(self.send(|| self.request(Method::GET, &path)).await?.json().await?)
    }

    pub async fn create_webhook(&self, channel: Id, name: &str) -> Result<Webhook> {
        let path = format!("/channels/{channel}/webhooks");
        let body = json!({ "name": name });
        Ok(self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?)
    }

    pub async fn modify_webhook(&self, webhook: Id, body: Value) -> Result<Webhook> {
        let path = format!("/webhooks/{webhook}");
        Ok(self.send(|| self.request(Method::PATCH, &path).json(&body)).await?.json().await?)
    }

    pub async fn delete_webhook(&self, webhook: Id) -> Result<()> {
        let path = format!("/webhooks/{webhook}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    /// A request whose JSON reply (or empty reply, as `Null`) is handled by the caller.
    pub async fn call(&self, method: Method, path: String, body: Option<Value>) -> Result<Value> {
        let resp = self
            .send(|| {
                let req = self.request(method.clone(), &path);
                match &body {
                    Some(b) => req.json(b),
                    None => req,
                }
            })
            .await?;
        let text = resp.text().await?;
        Ok(if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text)? })
    }

    pub async fn audit_log(&self, guild: Id) -> Result<Value> {
        let path = format!("/guilds/{guild}/audit-logs?limit=100");
        Ok(self.send(|| self.request(Method::GET, &path)).await?.json().await?)
    }

    pub async fn create_role(&self, guild: Id, body: Value) -> Result<Role> {
        let path = format!("/guilds/{guild}/roles");
        Ok(self.send(|| self.request(Method::POST, &path).json(&body)).await?.json().await?)
    }

    pub async fn modify_role(&self, guild: Id, role: Id, body: Value) -> Result<Role> {
        let path = format!("/guilds/{guild}/roles/{role}");
        Ok(self.send(|| self.request(Method::PATCH, &path).json(&body)).await?.json().await?)
    }

    pub async fn delete_role(&self, guild: Id, role: Id) -> Result<()> {
        let path = format!("/guilds/{guild}/roles/{role}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    pub async fn set_member_role(&self, guild: Id, user: Id, role: Id, on: bool) -> Result<()> {
        let path = format!("/guilds/{guild}/members/{user}/roles/{role}");
        let method = if on { Method::PUT } else { Method::DELETE };
        self.send(|| self.request(method.clone(), &path).body("")).await?;
        Ok(())
    }

    /// Changes a nickname; `user` is `None` for our own.
    pub async fn set_nickname(&self, guild: Id, user: Option<Id>, nick: Option<&str>) -> Result<()> {
        let who = user.map_or_else(|| "@me".to_string(), |u| u.to_string());
        let path = format!("/guilds/{guild}/members/{who}");
        let body = json!({ "nick": nick });
        self.send(|| self.request(Method::PATCH, &path).json(&body)).await?;
        Ok(())
    }

    pub async fn kick(&self, guild: Id, user: Id) -> Result<()> {
        let path = format!("/guilds/{guild}/members/{user}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    pub async fn ban(&self, guild: Id, user: Id, delete_message_seconds: u32) -> Result<()> {
        let path = format!("/guilds/{guild}/bans/{user}");
        let body = json!({ "delete_message_seconds": delete_message_seconds });
        self.send(|| self.request(Method::PUT, &path).json(&body)).await?;
        Ok(())
    }

    pub async fn unban(&self, guild: Id, user: Id) -> Result<()> {
        let path = format!("/guilds/{guild}/bans/{user}");
        self.send(|| self.request(Method::DELETE, &path)).await?;
        Ok(())
    }

    pub async fn bans(&self, guild: Id) -> Result<Vec<Ban>> {
        let path = format!("/guilds/{guild}/bans?limit=1000");
        Ok(self.send(|| self.request(Method::GET, &path)).await?.json().await?)
    }

    /// Changes the account's username, display name or avatar.
    pub async fn modify_me(&self, body: Value) -> Result<User> {
        Ok(self.send(|| self.request(Method::PATCH, "/users/@me").json(&body)).await?.json().await?)
    }

    /// The account's profile: about-me text and pronouns (user accounts only).
    pub async fn my_profile(&self, me: Id) -> Result<Value> {
        let path = format!("/users/{me}/profile");
        Ok(self.send(|| self.request(Method::GET, &path)).await?.json().await?)
    }

    pub async fn modify_profile(&self, body: Value) -> Result<()> {
        self.send(|| self.request(Method::PATCH, "/users/@me/profile").json(&body)).await?;
        Ok(())
    }

    pub async fn open_dm(&self, user: Id) -> Result<Channel> {
        let body = json!({ "recipient_id": user.to_string() });
        Ok(self
            .send(|| self.request(Method::POST, "/users/@me/channels").json(&body))
            .await?
            .json()
            .await?)
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
