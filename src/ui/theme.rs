//! Themes. The active palette is process-wide; views read it through the
//! role functions below, so switching themes only needs a window refresh.

use gpui::{App, Hsla, rgb};
use gpui_component::{Theme, ThemeMode};
use serde::Deserialize;
use std::{path::Path, sync::RwLock};

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub bg_rail: u32,
    pub bg_sidebar: u32,
    pub bg_chat: u32,
    pub bg_input: u32,
    pub bg_hover: u32,
    pub bg_active: u32,
    pub bg_code: u32,
    pub border: u32,
    pub text: u32,
    pub text_bright: u32,
    pub text_muted: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub accent_text: u32,
    pub link: u32,
    pub danger: u32,
    pub warning: u32,
    pub online: u32,
    pub idle: u32,
    pub dnd: u32,
    pub offline: u32,
}

/// The default theme: plum-dark surfaces with pink accents.
pub const fn blossom() -> Palette {
    Palette {
        id: String::new(),
        name: String::new(),
        dark: true,
        bg_rail: 0x130d13,
        bg_sidebar: 0x1a141b,
        bg_chat: 0x221d27,
        bg_input: 0x2c2533,
        bg_hover: 0x2a2330,
        bg_active: 0x3a2f41,
        bg_code: 0x18131c,
        border: 0x30283a,
        text: 0xe6dcea,
        text_bright: 0xf9f8fb,
        text_muted: 0xa797b0,
        accent: 0xd4387c,
        on_accent: 0xfff5fa,
        accent_text: 0xf7a8cb,
        link: 0xf28dc0,
        danger: 0xf0506e,
        warning: 0xf0b232,
        online: 0x3ba55d,
        idle: 0xf0b232,
        dnd: 0xed4245,
        offline: 0x82748a,
    }
}

pub const fn blossom_light() -> Palette {
    Palette {
        id: String::new(),
        name: String::new(),
        dark: false,
        bg_rail: 0xead3ee,
        bg_sidebar: 0xf3e4f5,
        bg_chat: 0xfcf6fc,
        bg_input: 0xf4e7f6,
        bg_hover: 0xf3e6f5,
        bg_active: 0xe8d0ec,
        bg_code: 0xf5e9f7,
        border: 0xe4cde8,
        text: 0x501854,
        text_bright: 0x3a0f3f,
        text_muted: 0x8d6492,
        accent: 0xa23b67,
        on_accent: 0xffffff,
        accent_text: 0xac1668,
        link: 0xac1668,
        danger: 0xc62a47,
        warning: 0xb7791f,
        online: 0x2d8a4b,
        idle: 0xc08a12,
        dnd: 0xd13438,
        offline: 0x9b86a0,
    }
}

const fn midnight() -> Palette {
    Palette {
        id: String::new(),
        name: String::new(),
        dark: true,
        bg_rail: 0x131519,
        bg_sidebar: 0x1a1c22,
        bg_chat: 0x21242b,
        bg_input: 0x2a2e37,
        bg_hover: 0x272a32,
        bg_active: 0x333844,
        bg_code: 0x16181d,
        border: 0x111216,
        text: 0xd9dde4,
        text_bright: 0xf4f6f9,
        text_muted: 0x8b93a1,
        accent: 0x34b3a0,
        on_accent: 0x0c1a18,
        accent_text: 0x8fe3d6,
        link: 0x58a6ff,
        danger: 0xe5484d,
        warning: 0xf0b232,
        online: 0x3ba55d,
        idle: 0xf0b232,
        dnd: 0xed4245,
        offline: 0x747f8d,
    }
}

const fn paper() -> Palette {
    Palette {
        id: String::new(),
        name: String::new(),
        dark: false,
        bg_rail: 0xe3e5e8,
        bg_sidebar: 0xf2f3f5,
        bg_chat: 0xffffff,
        bg_input: 0xebedef,
        bg_hover: 0xf2f3f5,
        bg_active: 0xdfe1e5,
        bg_code: 0xf2f3f5,
        border: 0xdcdfe3,
        text: 0x2e3338,
        text_bright: 0x060607,
        text_muted: 0x5c6470,
        accent: 0x3b6fd4,
        on_accent: 0xffffff,
        accent_text: 0x2a56ad,
        link: 0x0067c0,
        danger: 0xd13438,
        warning: 0xb7791f,
        online: 0x2d8a4b,
        idle: 0xc08a12,
        dnd: 0xd13438,
        offline: 0x80848e,
    }
}

fn named(id: &str, name: &str, mut palette: Palette) -> Palette {
    palette.id = id.to_string();
    palette.name = name.to_string();
    palette
}

pub const DEFAULT_THEME: &str = "blossom";

pub fn builtin() -> Vec<Palette> {
    vec![
        named(DEFAULT_THEME, "Blossom", blossom()),
        named("blossom-light", "Blossom Light", blossom_light()),
        named("midnight", "Midnight", midnight()),
        named("paper", "Paper", paper()),
    ]
}

static CURRENT: RwLock<Palette> = RwLock::new(blossom());

fn get(f: impl Fn(&Palette) -> u32) -> Hsla {
    c(f(&CURRENT.read().unwrap()))
}

pub fn current_id() -> String {
    CURRENT.read().unwrap().id.clone()
}

pub fn bg_rail() -> Hsla { get(|p| p.bg_rail) }
pub fn bg_sidebar() -> Hsla { get(|p| p.bg_sidebar) }
pub fn bg_chat() -> Hsla { get(|p| p.bg_chat) }
pub fn bg_input() -> Hsla { get(|p| p.bg_input) }
pub fn bg_hover() -> Hsla { get(|p| p.bg_hover) }
pub fn bg_active() -> Hsla { get(|p| p.bg_active) }
pub fn bg_code() -> Hsla { get(|p| p.bg_code) }
pub fn border() -> Hsla { get(|p| p.border) }
pub fn text() -> Hsla { get(|p| p.text) }
pub fn text_bright() -> Hsla { get(|p| p.text_bright) }
pub fn text_muted() -> Hsla { get(|p| p.text_muted) }
pub fn accent() -> Hsla { get(|p| p.accent) }
/// Text and icons drawn on top of the accent colour.
pub fn on_accent() -> Hsla { get(|p| p.on_accent) }
pub fn accent_text() -> Hsla { get(|p| p.accent_text) }
pub fn link() -> Hsla { get(|p| p.link) }
pub fn danger() -> Hsla { get(|p| p.danger) }
pub fn warning() -> Hsla { get(|p| p.warning) }
pub fn online() -> Hsla { get(|p| p.online) }
pub fn idle() -> Hsla { get(|p| p.idle) }
pub fn dnd() -> Hsla { get(|p| p.dnd) }
pub fn offline() -> Hsla { get(|p| p.offline) }

/// Makes `palette` the active theme and restyles the shared components
/// (inputs, menus, tooltips, notifications) to match.
pub fn apply(palette: &Palette, cx: &mut App) {
    *CURRENT.write().unwrap() = palette.clone();
    Theme::change(if palette.dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);

    let p = palette;
    let theme = Theme::global_mut(cx);
    theme.background = c(p.bg_chat);
    theme.foreground = c(p.text);
    theme.border = c(p.border);
    theme.input = c(p.border);
    theme.popover = c(p.bg_sidebar);
    theme.popover_foreground = c(p.text);
    theme.muted = c(p.bg_hover);
    theme.muted_foreground = c(p.text_muted);
    theme.accent = c(p.bg_active);
    theme.accent_foreground = c(p.text_bright);
    theme.secondary = c(p.bg_input);
    theme.secondary_hover = c(p.bg_hover);
    theme.secondary_active = c(p.bg_active);
    theme.secondary_foreground = c(p.text);
    theme.primary = c(p.accent);
    theme.primary_hover = c(p.accent).opacity(0.88);
    theme.primary_active = c(p.accent).opacity(0.76);
    theme.primary_foreground = c(p.on_accent);
    theme.button_primary = c(p.accent);
    theme.button_primary_hover = c(p.accent).opacity(0.88);
    theme.button_primary_active = c(p.accent).opacity(0.76);
    theme.button_primary_foreground = c(p.on_accent);
    theme.ring = c(p.accent);
    theme.caret = c(p.text_bright);
    theme.selection = c(p.accent).opacity(0.35);
    theme.link = c(p.link);
    theme.list_hover = c(p.bg_hover);
    theme.list_active = c(p.bg_active);
    theme.scrollbar_thumb = c(p.text_muted).opacity(0.35);
    theme.scrollbar_thumb_hover = c(p.text_muted).opacity(0.6);
    theme.danger = c(p.danger);
    cx.refresh_windows();
}

// ---------------------------------------------------------------- custom themes

/// A user theme file. Every colour is optional; anything left out is taken
/// from Blossom (or Blossom Light when `dark` is false).
#[derive(Deserialize)]
struct ThemeFile {
    name: Option<String>,
    dark: Option<bool>,
    #[serde(flatten)]
    colors: std::collections::HashMap<String, String>,
}

fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok(),
        3 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some((r * 17) << 16 | (g * 17) << 8 | b * 17)
        }
        _ => None,
    }
}

pub fn parse_theme(id: &str, json: &str) -> Result<Palette, String> {
    let file: ThemeFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let dark = file.dark.unwrap_or(true);
    let mut p = named(id, file.name.as_deref().unwrap_or(id), if dark { blossom() } else { blossom_light() });
    for (key, value) in &file.colors {
        let slot = match key.as_str() {
            "bg_rail" => &mut p.bg_rail,
            "bg_sidebar" => &mut p.bg_sidebar,
            "bg_chat" => &mut p.bg_chat,
            "bg_input" => &mut p.bg_input,
            "bg_hover" => &mut p.bg_hover,
            "bg_active" => &mut p.bg_active,
            "bg_code" => &mut p.bg_code,
            "border" => &mut p.border,
            "text" => &mut p.text,
            "text_bright" => &mut p.text_bright,
            "text_muted" => &mut p.text_muted,
            "accent" => &mut p.accent,
            "on_accent" => &mut p.on_accent,
            "accent_text" => &mut p.accent_text,
            "link" => &mut p.link,
            "danger" => &mut p.danger,
            "warning" => &mut p.warning,
            "online" => &mut p.online,
            "idle" => &mut p.idle,
            "dnd" => &mut p.dnd,
            "offline" => &mut p.offline,
            other => return Err(format!("unknown key \"{other}\"")),
        };
        *slot = parse_hex(value).ok_or_else(|| format!("\"{key}\" is not a hex colour: {value}"))?;
    }
    Ok(p)
}

/// Built-in themes followed by any valid `*.json` theme in `dir`.
pub fn load_all(dir: Option<&Path>) -> Vec<Palette> {
    let mut themes = builtin();
    let Some(entries) = dir.and_then(|d| std::fs::read_dir(d).ok()) else { return themes };
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let id = format!("custom-{stem}");
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| parse_theme(&id, &s)) {
            Ok(palette) => themes.push(palette),
            Err(e) => log::warn!("skipping theme {}: {e}", path.display()),
        }
    }
    themes
}

pub const EXAMPLE_THEME: &str = r##"{
  "name": "My Theme",
  "dark": true,
  "bg_rail": "#130d13",
  "bg_sidebar": "#1a141b",
  "bg_chat": "#221d27",
  "bg_input": "#2c2533",
  "bg_hover": "#2a2330",
  "bg_active": "#3a2f41",
  "bg_code": "#18131c",
  "border": "#30283a",
  "text": "#e6dcea",
  "text_bright": "#f9f8fb",
  "text_muted": "#a797b0",
  "accent": "#d4387c",
  "on_accent": "#fff5fa",
  "accent_text": "#f7a8cb",
  "link": "#f28dc0",
  "danger": "#f0506e",
  "warning": "#f0b232",
  "online": "#3ba55d",
  "idle": "#f0b232",
  "dnd": "#ed4245",
  "offline": "#82748a"
}
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_theme_round_trips_to_blossom() {
        let parsed = parse_theme("x", EXAMPLE_THEME).unwrap();
        assert_eq!(parsed, named("x", "My Theme", blossom()));
    }

    #[test]
    fn partial_light_theme_falls_back_to_blossom_light() {
        let p = parse_theme("mine", r##"{ "dark": false, "accent": "#f0a" }"##).unwrap();
        assert_eq!(p.accent, 0xff00aa);
        assert_eq!(p.bg_chat, blossom_light().bg_chat);
        assert_eq!(p.name, "mine");
    }

    #[test]
    fn bad_themes_are_rejected() {
        assert!(parse_theme("x", r##"{ "accent": "pink" }"##).is_err());
        assert!(parse_theme("x", r##"{ "acent": "#ffffff" }"##).is_err());
        assert!(parse_theme("x", "not json").is_err());
    }

    #[test]
    fn builtin_ids_are_unique_and_include_the_default() {
        let themes = builtin();
        let mut ids: Vec<_> = themes.iter().map(|t| t.id.clone()).collect();
        ids.dedup();
        assert_eq!(ids.len(), themes.len());
        assert_eq!(themes[0].id, DEFAULT_THEME);
    }
}
