//! Updates from GitHub releases.
//!
//! Each release carries `latest.json`, naming the version and the signed
//! installer. Ferrocord checks it, verifies the installer's signature against
//! the public key built in here, and runs it; the installer replaces the app
//! and starts it again.

use anyhow::{Result, anyhow};
use cargo_packager_updater::{Config, Update, WindowsConfig, WindowsUpdateInstallMode};

const ENDPOINT: &str = "https://github.com/asofianidis/ferrocord/releases/latest/download/latest.json";
/// Only installers signed with the matching private key are accepted.
const PUBLIC_KEY: &str = include_str!("../assets/update-key.pub");

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn config() -> Result<Config> {
    Ok(Config {
        endpoints: vec![ENDPOINT.parse()?],
        pubkey: PUBLIC_KEY.trim().to_string(),
        windows: Some(WindowsConfig {
            installer_args: None,
            // A small progress window, no questions asked.
            install_mode: Some(WindowsUpdateInstallMode::Passive),
        }),
    })
}

/// Asks GitHub for a newer release. Blocking: call it off the UI thread.
pub fn check() -> Result<Option<Update>> {
    let current = semver::Version::parse(current_version())?;
    cargo_packager_updater::check_update(current, config()?).map_err(|e| anyhow!("{e}"))
}

/// Downloads, verifies and runs the installer, then exits so it can replace
/// the app. Blocking; only returns if something went wrong.
pub fn install(update: &Update) -> Result<()> {
    update.download_and_install().map_err(|e| anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_config_is_valid() {
        let config = config().unwrap();
        assert!(config.endpoints[0].as_str().ends_with("/latest.json"));
        assert!(!config.pubkey.is_empty() && !config.pubkey.contains("PRIVATE"));
        assert!(semver::Version::parse(current_version()).is_ok());
    }
}
