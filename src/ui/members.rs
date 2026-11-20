//! The member list on the right-hand side of guild channels.

use super::{theme, widgets::*};
use crate::store::{MemberRow, Store, StoreEvent};
use gpui::{prelude::*, *};
use std::ops::Range;

pub struct Members {
    store: Entity<Store>,
    _subs: Vec<Subscription>,
}

impl Members {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.subscribe(&store, |_, store, event: &StoreEvent, cx| match event {
            StoreEvent::Members(guild) if store.read(cx).selected_guild == Some(*guild) => cx.notify(),
            StoreEvent::Selection => cx.notify(),
            _ => {}
        })];
        Self { store, _subs: subs }
    }

    fn rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let store = self.store.read(cx);
        let Some(list) = store.selected_guild.and_then(|g| store.member_lists.get(&g)) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(range.len());
        for (offset, row) in list.get(range.clone()).unwrap_or_default().iter().enumerate() {
            let ix = range.start + offset;
            out.push(match row {
                MemberRow::Group { name, count } => h_flex()
                    .h(px(42.))
                    .px_4()
                    .pt_3()
                    .text_size(px(11.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_muted())
                    .child(div().truncate().child(format!("{} \u{2014} {count}", name.to_uppercase())))
                    .into_any_element(),
                MemberRow::Placeholder => h_flex()
                    .h(px(42.))
                    .px_4()
                    .gap_3()
                    .child(div().size(px(32.)).rounded_full().bg(theme::bg_hover()))
                    .child(div().h(px(12.)).w(px(110.)).rounded_md().bg(theme::bg_hover()))
                    .into_any_element(),
                MemberRow::Member { user, nick, color, status } => {
                    let name = nick.clone().unwrap_or_else(|| user.display_name().to_string());
                    let offline = *status == crate::discord::Status::Offline;
                    let user_id = user.id;
                    div()
                        .id(("member", ix))
                        .w_full()
                        .h(px(42.))
                        .px_2()
                        .flex()
                        .items_center()
                        .child(
                            h_flex()
                                .w_full()
                                .h(px(40.))
                                .px_2()
                                .gap_3()
                                .rounded_md()
                                .cursor_pointer()
                                .when(offline, |d| d.opacity(0.45))
                                .hover(|s| s.bg(theme::bg_hover()))
                                .child(avatar_with_status(user.avatar_url(64), 32., *status, theme::bg_sidebar()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(color.map(theme::c).unwrap_or_else(theme::text))
                                        .child(name),
                                )
                                .when(user.bot, |d| d.child(bot_tag())),
                        )
                        .tooltip(tip("Click to message"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.store.update(cx, |store, cx| store.open_dm(user_id, cx));
                        }))
                        .into_any_element()
                }
            });
        }
        out
    }
}

pub fn bot_tag() -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px(px(4.))
        .rounded_sm()
        .bg(theme::accent())
        .text_color(theme::on_accent())
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .child("APP")
}

impl Render for Members {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let count = store.selected_guild.and_then(|g| store.member_lists.get(&g)).map_or(0, |l| l.len());
        let key = store.selected_guild.map_or(0, |g| g.0);

        v_flex().w(px(240.)).h_full().flex_shrink_0().bg(theme::bg_sidebar()).child(if count == 0 {
            div()
                .p_4()
                .text_color(theme::text_muted())
                .child("Members appear here once the server sends them.")
                .into_any_element()
        } else {
            uniform_list(("members", key), count, cx.processor(Self::rows)).flex_1().into_any_element()
        })
    }
}
