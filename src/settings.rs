//! User settings, persisted as JSON in the platform config directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { theme: crate::ui::theme::DEFAULT_THEME.to_string() }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ferrocord"))
}

pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("themes"))
}

impl Settings {
    pub fn load() -> Self {
        config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(dir) = config_dir() else { return };
        let result = std::fs::create_dir_all(&dir).and_then(|_| {
            std::fs::write(dir.join("settings.json"), serde_json::to_string_pretty(self).unwrap_or_default())
        });
        if let Err(e) = result {
            log::warn!("could not save settings: {e}");
        }
    }
}
