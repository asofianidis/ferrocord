//! Quick switcher: jump to any channel or conversation by name.

use super::{theme, widgets::*};
use crate::{
    discord::Id,
    store::{RowKind, Store},
};
use gpui::{prelude::*, *};
use gpui_component::input::{self, Input, InputEvent, InputState};

const MAX_RESULTS: usize = 10;

pub enum SwitcherEvent {
    Dismiss,
}

struct Target {
    channel: Id,
    kind: u8,
    label: String,
    detail: String,
    unread: bool,
}

pub struct Switcher {
    store: Entity<Store>,
    input: Entity<InputState>,
    results: Vec<Target>,
    selected: usize,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SwitcherEvent> for Switcher {}

impl Switcher {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Jump to a channel or conversation"));
        let subs = vec![cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => this.refilter(cx),
            InputEvent::PressEnter { .. } => this.confirm(this.selected, cx),
            _ => {}
        })];
        input.update(cx, |s, cx| s.focus(window, cx));
        let mut this = Self { store, input, results: Vec::new(), selected: 0, _subs: subs };
        this.refilter(cx);
        this
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().trim_start_matches(['#', '@']).to_lowercase();
        let store = self.store.read(cx);
        let mut scored: Vec<(u8, Target)> = Vec::new();
        let mut consider = |channel: Id, label: String, detail: String, kind: u8| {
            let unread = store.is_unread(channel);
            let lower = label.to_lowercase();
            let score = if query.is_empty() {
                // With no query, surface what needs attention.
                if unread { 0 } else { 2 }
            } else if lower.starts_with(&query) {
                0
            } else if lower.contains(&query) {
                1
            } else {
                return;
            };
            scored.push((score, Target { channel, kind, label, detail, unread }));
        };
        for id in &store.dms {
            if let Some(channel) = store.channels.get(id) {
                consider(*id, store.channel_title(channel), "Direct message".into(), channel.kind);
            }
        }
        for guild in &store.guilds {
            for row in guild.rows.iter().filter(|r| r.kind != RowKind::Category) {
                if let Some(channel) = store.channels.get(&row.id).filter(|c| c.is_text_like()) {
                    consider(row.id, store.channel_title(channel), guild.guild.name.clone(), channel.kind);
                }
            }
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.len().cmp(&b.1.label.len())));
        scored.truncate(MAX_RESULTS);
        self.results = scored.into_iter().map(|(_, target)| target).collect();
        self.selected = 0;
        cx.notify();
    }

    fn confirm(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(target) = self.results.get(ix) {
            let channel = target.channel;
            self.store.update(cx, |store, cx| store.select_channel(channel, cx));
        }
        cx.emit(SwitcherEvent::Dismiss);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let len = self.results.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        cx.notify();
    }
}

impl Render for Switcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        div()
            .id("switcher-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .justify_center()
            .bg(gpui::black().opacity(0.55))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SwitcherEvent::Dismiss)))
            .capture_action(cx.listener(|_, _: &input::Escape, _, cx| {
                cx.emit(SwitcherEvent::Dismiss);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::MoveUp, _, cx| {
                this.step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::MoveDown, _, cx| {
                this.step(1, cx);
                cx.stop_propagation();
            }))
            .child(
                v_flex()
                    .id("switcher")
                    .mt(px(96.))
                    .w(px(540.))
                    .max_h(px(520.))
                    .p_3()
                    .gap_2()
                    .rounded_xl()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
                    .shadow_lg()
                    // Clicks inside the panel must not reach the backdrop.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(Input::new(&self.input))
                    .when(self.results.is_empty(), |d| {
                        d.child(div().p_2().text_color(theme::text_muted()).child("No matches."))
                    })
                    .children(self.results.iter().enumerate().map(|(ix, target)| {
                        h_flex()
                            .id(("switcher-result", ix))
                            .h(px(36.))
                            .px_2()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(ix == selected, |d| d.bg(theme::bg_active()))
                            .hover(|s| s.bg(theme::bg_hover()))
                            .child(icon(channel_icon(target.kind), 16., theme::text_muted()))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w(px(280.))
                                    .truncate()
                                    .text_color(theme::text_bright())
                                    .when(target.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .child(target.label.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child(target.detail.clone()),
                            )
                            .when(target.unread, |d| {
                                d.child(div().size(px(8.)).rounded_full().bg(theme::text_bright()))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.confirm(ix, cx);
                            }))
                    }))
                    .child(
                        div()
                            .px_2()
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child("\u{2191}\u{2193} to navigate \u{00b7} Enter to open \u{00b7} Esc to close"),
                    ),
            )
    }
}
