//! Signing in with an email (or phone number) and password, including the
//! two-factor step. Only the session token that results is ever kept; the
//! password goes to Discord and nowhere else.

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::time::Duration;

const API: &str = "https://discord.com/api/v9";
const USER_AGENT: &str = concat!("Ferrocord/", env!("CARGO_PKG_VERSION"));

/// What Discord said to a login attempt.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Token(String),
    /// A second step is needed; `ticket` identifies this attempt.
    Mfa(Mfa),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mfa {
    pub ticket: String,
    /// An authenticator app (backup codes also work here).
    pub totp: bool,
    pub sms: bool,
    /// Security keys and passkeys, which this client cannot use.
    pub webauthn: bool,
}

pub fn parse_login(v: &Value) -> Result<Outcome> {
    if let Some(token) = v["token"].as_str().filter(|t| !t.is_empty()) {
        return Ok(Outcome::Token(token.to_string()));
    }
    if v["mfa"].as_bool() == Some(true) {
        let ticket = v["ticket"].as_str().ok_or_else(|| anyhow!("Discord asked for a second step but sent no ticket."))?;
        return Ok(Outcome::Mfa(Mfa {
            ticket: ticket.to_string(),
            totp: v["totp"].as_bool().unwrap_or(false),
            sms: v["sms"].as_bool().unwrap_or(false),
            webauthn: !v["webauthn"].is_null() && v["webauthn"] != json!(false),
        }));
    }
    Err(anyhow!("Discord's reply had no session token."))
}

/// Finds the first human-readable message in a Discord error body.
fn first_message(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => {
            if let Some(Value::Array(errors)) = map.get("_errors") {
                if let Some(m) = errors.iter().find_map(|e| e["message"].as_str()) {
                    return Some(m.to_string());
                }
            }
            map.iter().filter(|(k, _)| k.as_str() != "message").find_map(|(_, child)| first_message(child))
        }
        Value::Array(items) => items.iter().find_map(first_message),
        _ => None,
    }
}

/// Turns a failed reply into something to show the user.
pub fn describe_error(v: &Value, status: u16) -> String {
    if v.get("captcha_key").is_some() || v.get("captcha_sitekey").is_some() {
        return "Discord wants a captcha check before signing in, which Ferrocord cannot show. \
                Wait a while and try again, or sign in with a token instead."
            .into();
    }
    if status == 429 {
        let wait = v["retry_after"].as_f64().map_or(String::new(), |s| format!(" Try again in {} seconds.", s.ceil()));
        return format!("Too many attempts.{wait}");
    }
    if let Some(m) = first_message(&v["errors"]) {
        return m;
    }
    v["message"].as_str().map_or_else(|| format!("Signing in failed (HTTP {status})."), str::to_string)
}

pub struct Auth {
    http: reqwest::Client,
}

impl Auth {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http })
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let resp = self.http.post(format!("{API}{path}")).json(&body).send().await?;
        let status = resp.status();
        let value: Value = resp.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(value)
        } else {
            Err(anyhow!(describe_error(&value, status.as_u16())))
        }
    }

    /// `login` is an email address or phone number.
    pub async fn login(&self, login: &str, password: &str) -> Result<Outcome> {
        let body = json!({
            "login": login, "password": password, "undelete": false,
            "login_source": null, "gift_code_sku_id": null,
        });
        parse_login(&self.post("/auth/login", body).await?)
    }

    /// Finishes with an authenticator code or a backup code.
    pub async fn totp(&self, ticket: &str, code: &str) -> Result<String> {
        let body = json!({ "code": code, "ticket": ticket, "login_source": null, "gift_code_sku_id": null });
        token_of(&self.post("/auth/mfa/totp", body).await?)
    }

    /// Texts a code to the account's phone; returns the (masked) number.
    pub async fn send_sms(&self, ticket: &str) -> Result<String> {
        let reply = self.post("/auth/mfa/sms/send", json!({ "ticket": ticket })).await?;
        Ok(reply["phone"].as_str().unwrap_or("your phone").to_string())
    }

    pub async fn sms(&self, ticket: &str, code: &str) -> Result<String> {
        let body = json!({ "code": code, "ticket": ticket, "login_source": null, "gift_code_sku_id": null });
        token_of(&self.post("/auth/mfa/sms", body).await?)
    }
}

fn token_of(v: &Value) -> Result<String> {
    match parse_login(v)? {
        Outcome::Token(token) => Ok(token),
        Outcome::Mfa(_) => Err(anyhow!("Discord asked for another step after the code.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_replies() {
        assert_eq!(parse_login(&json!({ "user_id": "1", "token": "abc" })).unwrap(), Outcome::Token("abc".into()));
        let mfa = parse_login(&json!({
            "user_id": "1", "mfa": true, "sms": false, "ticket": "t1", "backup": true, "totp": true, "webauthn": null,
        }))
        .unwrap();
        assert_eq!(mfa, Outcome::Mfa(Mfa { ticket: "t1".into(), totp: true, sms: false, webauthn: false }));
        let keys = parse_login(&json!({ "mfa": true, "ticket": "t", "webauthn": "{\"publicKey\":{}}" })).unwrap();
        assert!(matches!(keys, Outcome::Mfa(Mfa { webauthn: true, totp: false, .. })));
        assert!(parse_login(&json!({})).is_err());
    }

    #[test]
    fn error_messages() {
        let invalid = json!({ "code": 50035, "message": "Invalid Form Body", "errors": {
            "login": { "_errors": [{ "code": "INVALID_LOGIN", "message": "Login or password is invalid." }] },
        }});
        assert_eq!(describe_error(&invalid, 400), "Login or password is invalid.");
        let captcha = json!({ "captcha_key": ["captcha-required"], "captcha_sitekey": "x", "captcha_service": "hcaptcha" });
        assert!(describe_error(&captcha, 400).contains("captcha"));
        assert_eq!(describe_error(&json!({ "retry_after": 4.2 }), 429), "Too many attempts. Try again in 5 seconds.");
        assert_eq!(describe_error(&json!({ "message": "Invalid two-factor code", "code": 60008 }), 400), "Invalid two-factor code");
        assert_eq!(describe_error(&Value::Null, 500), "Signing in failed (HTTP 500).");
    }
}
