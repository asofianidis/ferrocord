//! The app's colour palette.

use gpui::{Hsla, rgb};

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn bg_rail() -> Hsla { c(0x131519) }
pub fn bg_sidebar() -> Hsla { c(0x1a1c22) }
pub fn bg_chat() -> Hsla { c(0x21242b) }
pub fn bg_input() -> Hsla { c(0x2a2e37) }
pub fn bg_hover() -> Hsla { c(0x272a32) }
pub fn bg_active() -> Hsla { c(0x333844) }
pub fn bg_code() -> Hsla { c(0x16181d) }
pub fn border() -> Hsla { c(0x111216) }
pub fn text() -> Hsla { c(0xd9dde4) }
pub fn text_bright() -> Hsla { c(0xf4f6f9) }
pub fn text_muted() -> Hsla { c(0x8b93a1) }
pub fn accent() -> Hsla { c(0x34b3a0) }
pub fn accent_text() -> Hsla { c(0x8fe3d6) }
pub fn link() -> Hsla { c(0x58a6ff) }
pub fn danger() -> Hsla { c(0xe5484d) }
pub fn online() -> Hsla { c(0x3ba55d) }
pub fn idle() -> Hsla { c(0xf0b232) }
pub fn dnd() -> Hsla { c(0xed4245) }
pub fn offline() -> Hsla { c(0x747f8d) }
