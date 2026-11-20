//! Minimal Discord REST client with rate-limit handling.

use super::model::{Channel, Id, Message};
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
