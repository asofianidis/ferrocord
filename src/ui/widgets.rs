//! Small shared UI building blocks.

use super::theme;
use crate::discord::{Status, channel_kind};
use gpui::{prelude::*, *};
use gpui_component::tooltip::Tooltip;
use gpui_kit::assets::IconName;

pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

pub fn v_flex() -> Div {
    div().flex().flex_col()
}

pub fn icon(name: IconName, size: f32, color: Hsla) -> Svg {
    svg().path(name.path()).size(px(size)).flex_shrink_0().text_color(color)
}

pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

pub fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(3).collect::<String>().to_uppercase()
}

/// A round image with a neutral placeholder while loading or on failure.
pub fn avatar(url: impl Into<SharedString>, size: f32) -> impl IntoElement {
    let url: SharedString = url.into();
    img(url)
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .object_fit(ObjectFit::Cover)
        .with_fallback(move || div().size(px(size)).rounded_full().bg(theme::bg_active()).into_any_element())
        .with_loading(move || div().size(px(size)).rounded_full().bg(theme::bg_active()).into_any_element())
}

pub fn status_color(status: Status) -> Hsla {
    match status {
        Status::Online => theme::online(),
        Status::Idle => theme::idle(),
        Status::Dnd => theme::dnd(),
        Status::Offline => theme::offline(),
    }
}

/// An avatar with a presence dot in its corner.
pub fn avatar_with_status(url: impl Into<SharedString>, size: f32, status: Status, ring: Hsla) -> impl IntoElement {
    div().relative().size(px(size)).flex_shrink_0().child(avatar(url, size)).child(
        div()
            .absolute()
            .right(px(-2.))
            .bottom(px(-2.))
            .size(px(12.))
            .rounded_full()
            .border_2()
            .border_color(ring)
            .bg(status_color(status)),
    )
}

pub fn badge(count: u32) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .min_w(px(16.))
        .h(px(16.))
        .px(px(4.))
        .rounded_full()
        .bg(theme::danger())
        .text_color(gpui::white())
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(if count > 99 { "99+".to_string() } else { count.to_string() })
}

/// A square, icon-only button. Attach `on_click` at the call site.
pub fn icon_button(id: impl Into<ElementId>, name: IconName, tooltip: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme::bg_active()))
        .child(icon(name, 16., theme::text_muted()))
        .tooltip(tip(tooltip))
}

pub fn channel_icon(kind: u8) -> IconName {
    use channel_kind::*;
    match kind {
        VOICE => IconName::Volume2,
        STAGE => IconName::Radio,
        ANNOUNCEMENT => IconName::Megaphone,
        FORUM | MEDIA => IconName::MessagesSquare,
        ANNOUNCEMENT_THREAD | PUBLIC_THREAD | PRIVATE_THREAD => IconName::MessageSquare,
        DM | GROUP_DM => IconName::AtSign,
        _ => IconName::Hash,
    }
}

pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}
