//! Token persistence in the operating system's credential store.

const SERVICE: &str = "ferrocord";
const ACCOUNT: &str = "token";

fn entry() -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).ok()
}

pub fn save(token: &str, is_bot: bool) {
    let value = format!("{}:{token}", if is_bot { "bot" } else { "user" });
    if let Some(Err(e)) = entry().map(|e| e.set_password(&value)) {
        log::warn!("could not save token to the credential store: {e}");
    }
}

/// Returns the saved token and whether it is a bot token.
pub fn load() -> Option<(String, bool)> {
    let value = entry()?.get_password().ok()?;
    let (kind, token) = value.split_once(':')?;
    (!token.is_empty()).then(|| (token.to_string(), kind == "bot"))
}

pub fn clear() {
    if let Some(entry) = entry() {
        let _ = entry.delete_credential();
    }
}
