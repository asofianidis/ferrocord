//! User settings, persisted as JSON in the platform config directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    /// Channels and servers muted on this device.
    pub muted: Vec<u64>,
    /// Audio devices by name; `None` follows the system default.
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// Voice activity threshold in dBFS.
    pub input_sensitivity_db: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: crate::ui::theme::DEFAULT_THEME.to_string(),
            muted: Vec::new(),
            input_device: None,
            output_device: None,
            input_sensitivity_db: -50.0,
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ferrocord"))
}

pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("themes"))
}

impl Settings {
    pub fn voice_options(&self) -> crate::voice::VoiceOptions {
        crate::voice::VoiceOptions {
            input_device: self.input_device.clone(),
            output_device: self.output_device.clone(),
            sensitivity_db: self.input_sensitivity_db,
        }
    }

    pub fn load() -> Self {
        config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Loads the settings, applies `change` and writes them back.
    pub fn update(change: impl FnOnce(&mut Settings)) {
        let mut settings = Self::load();
        change(&mut settings);
        settings.save();
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
