//! QR-code sign-in, using Discord's remote authentication: the same scheme
//! the official desktop app uses to let a signed-in phone approve a login.
//!
//! 1. Connect to the remote-auth gateway and send a fresh RSA public key.
//! 2. Prove we hold the private key by decrypting a nonce the gateway sends.
//! 3. The gateway hands out a fingerprint, shown as a QR code linking to
//!    `https://discord.com/ra/<fingerprint>`.
//! 4. When the phone scans it, the gateway sends who scanned (encrypted to us).
//! 5. When the phone approves, the gateway sends a ticket, which the REST API
//!    exchanges for the session token, again encrypted to our key.
//!
//! No password, two-factor code or captcha is involved; approval happens in
//! Discord's own app.

use anyhow::{Context as _, Result, anyhow};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use futures::{SinkExt as _, StreamExt as _};
use rsa::{Oaep, RsaPrivateKey, pkcs8::EncodePublicKey as _, rand_core::OsRng};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::time::Duration;
use tokio_tungstenite::tungstenite::{Message as WsMessage, client::IntoClientRequest as _, http::HeaderValue};

const GATEWAY: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
const LOGIN: &str = "https://discord.com/api/v9/users/@me/remote-auth/login";
const USER_AGENT: &str = concat!("Ferrocord/", env!("CARGO_PKG_VERSION"));

/// Progress reported to the login screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// Show this address as a QR code.
    Code(String),
    /// Someone scanned the code and is being asked to approve on their phone.
    Scanned { username: String, avatar_url: Option<String> },
    /// Approved: the session token.
    Token(String),
    /// The phone declined.
    Cancelled,
    /// The code timed out before anyone approved it.
    Expired,
    Failed(String),
}

/// Who scanned the code, from the decrypted `pending_ticket` payload
/// (`id:discriminator:avatar:username`).
pub fn parse_user(payload: &str) -> Option<(String, Option<String>)> {
    let mut parts = payload.splitn(4, ':');
    let (id, _discriminator, avatar, username) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let avatar_url = (!avatar.is_empty() && avatar != "0")
        .then(|| format!("https://cdn.discordapp.com/avatars/{id}/{avatar}.png?size=128"));
    Some((username.to_string(), avatar_url))
}

/// The answer to the gateway's challenge: SHA-256 of the decrypted nonce,
/// URL-safe base64 without padding.
pub fn nonce_proof(nonce: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(nonce))
}

fn decrypt(key: &RsaPrivateKey, encoded: &str) -> Result<Vec<u8>> {
    let bytes = STANDARD.decode(encoded).context("bad base64 from Discord")?;
    key.decrypt(Oaep::new::<Sha256>(), &bytes).map_err(|e| anyhow!("could not decrypt Discord's reply: {e}"))
}

/// Runs one QR sign-in until it succeeds, is declined, expires or fails.
/// Progress goes to `updates`; dropping the receiver stops it.
pub async fn run(updates: async_channel::Sender<Update>) {
    if let Err(e) = attempt(&updates).await {
        let _ = updates.send(Update::Failed(e.to_string())).await;
    }
}

async fn attempt(updates: &async_channel::Sender<Update>) -> Result<()> {
    // Key generation takes a moment; keep it off the async workers.
    let key = tokio::task::spawn_blocking(|| RsaPrivateKey::new(&mut OsRng, 2048)).await??;
    let public = key.to_public_key().to_public_key_der()?;

    let mut request = GATEWAY.into_client_request()?;
    // The gateway only accepts connections that say they come from discord.com.
    request.headers_mut().insert("Origin", HeaderValue::from_static("https://discord.com"));
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.context("could not reach Discord")?;

    let mut heartbeat = tokio::time::interval(Duration::from_secs(40));
    heartbeat.tick().await; // The first tick fires immediately.
    loop {
        let message = tokio::select! {
            _ = heartbeat.tick() => {
                ws.send(WsMessage::text(json!({ "op": "heartbeat" }).to_string())).await?;
                continue;
            }
            _ = updates.closed() => return Ok(()), // The login screen went away.
            message = ws.next() => message,
        };
        let text = match message {
            Some(Ok(WsMessage::Text(text))) => text,
            Some(Ok(WsMessage::Close(frame))) => {
                // The gateway closes the socket when the code expires.
                log::info!("remote auth closed: {frame:?}");
                updates.send(Update::Expired).await.ok();
                return Ok(());
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(anyhow!("connection to Discord failed: {e}")),
            None => {
                updates.send(Update::Expired).await.ok();
                return Ok(());
            }
        };
        let v: Value = serde_json::from_str(&text)?;
        match v["op"].as_str().unwrap_or_default() {
            "hello" => {
                if let Some(ms) = v["heartbeat_interval"].as_u64() {
                    heartbeat = tokio::time::interval(Duration::from_millis(ms.max(1000)));
                    heartbeat.tick().await;
                }
                let init = json!({ "op": "init", "encoded_public_key": STANDARD.encode(public.as_bytes()) });
                ws.send(WsMessage::text(init.to_string())).await?;
            }
            "nonce_proof" => {
                let nonce = decrypt(&key, v["encrypted_nonce"].as_str().unwrap_or_default())?;
                let proof = json!({ "op": "nonce_proof", "proof": nonce_proof(&nonce) });
                ws.send(WsMessage::text(proof.to_string())).await?;
            }
            "pending_remote_init" => {
                let fingerprint = v["fingerprint"].as_str().ok_or_else(|| anyhow!("Discord sent no QR code"))?;
                updates.send(Update::Code(format!("https://discord.com/ra/{fingerprint}"))).await.ok();
            }
            "pending_ticket" => {
                let payload = decrypt(&key, v["encrypted_user_payload"].as_str().unwrap_or_default())?;
                let (username, avatar_url) =
                    parse_user(&String::from_utf8_lossy(&payload)).unwrap_or_else(|| ("your account".into(), None));
                updates.send(Update::Scanned { username, avatar_url }).await.ok();
            }
            "pending_login" => {
                let ticket = v["ticket"].as_str().ok_or_else(|| anyhow!("Discord sent no login ticket"))?;
                let encrypted = exchange_ticket(ticket).await?;
                let token = String::from_utf8(decrypt(&key, &encrypted)?)?;
                updates.send(Update::Token(token)).await.ok();
                return Ok(());
            }
            // Older gateways sent the encrypted token directly.
            "pending_finish" | "finish" => {
                let encrypted = v["encrypted_token"].as_str().ok_or_else(|| anyhow!("Discord sent no token"))?;
                let token = String::from_utf8(decrypt(&key, encrypted)?)?;
                updates.send(Update::Token(token)).await.ok();
                return Ok(());
            }
            "cancel" => {
                updates.send(Update::Cancelled).await.ok();
                return Ok(());
            }
            _ => {} // heartbeat_ack and anything new
        }
    }
}

/// Swaps the approval ticket for the encrypted session token.
async fn exchange_ticket(ticket: &str) -> Result<String> {
    let http = reqwest::Client::builder().user_agent(USER_AGENT).timeout(Duration::from_secs(30)).build()?;
    let resp = http.post(LOGIN).json(&json!({ "ticket": ticket })).send().await?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        log::warn!("remote auth login failed with HTTP {}: {body}", status.as_u16());
        return Err(anyhow!(super::auth::describe_error(&body, status.as_u16())));
    }
    body["encrypted_token"].as_str().map(str::to_string).ok_or_else(|| anyhow!("Discord sent no token"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanned_user_payload() {
        let (name, avatar) = parse_user("123:0:abc:ada").unwrap();
        assert_eq!(name, "ada");
        assert_eq!(avatar.as_deref(), Some("https://cdn.discordapp.com/avatars/123/abc.png?size=128"));
        assert_eq!(parse_user("123:0:0:grace").unwrap(), ("grace".into(), None));
        assert_eq!(parse_user("123:0::name:with:colons").unwrap().0, "name:with:colons");
        assert!(parse_user("nonsense").is_none());
    }

    #[test]
    fn nonce_round_trip() {
        let key = RsaPrivateKey::new(&mut OsRng, 1024).unwrap();
        let nonce = b"discord-nonce";
        let encrypted = key.to_public_key().encrypt(&mut OsRng, Oaep::new::<Sha256>(), nonce).unwrap();
        let decrypted = decrypt(&key, &STANDARD.encode(encrypted)).unwrap();
        assert_eq!(decrypted, nonce);
        let proof = nonce_proof(&decrypted);
        assert_eq!(proof.len(), 43, "32 bytes, unpadded URL-safe base64");
        assert!(!proof.contains(['+', '/', '=']));
    }
}
