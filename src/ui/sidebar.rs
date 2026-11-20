//! Channel list (or direct-message list) and the current-user panel.

use super::{theme, widgets::*};
use crate::{
    credentials,
    discord::{Id, channel_kind},
    store::{ChannelRow, RowKind, Store, StoreEvent},
};
use gpui::{prelude::*, *};
use gpui_kit::assets::IconName;
use std::ops::Range;

pub struct Sidebar {
    store: Entity<Store>,
    rows: Vec<ChannelRow>,
    _subs: Vec<Subscription>,
}

impl Sidebar {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.subscribe(&store, |_, _, event: &StoreEvent, cx| {
            if matches!(
                event,
                StoreEvent::Guilds
                    | StoreEvent::Channels
                    | StoreEvent::Selection
                    | StoreEvent::Unread
                    | StoreEvent::Connection
            ) {
                cx.notify();
            }
        })];
        Self { store, rows: Vec::new(), _subs: subs }
    }

    fn channel_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let store = self.store.read(cx);
        let mut out = Vec::with_capacity(range.len());
        for row in self.rows.get(range).unwrap_or_default() {
            let id = row.id;
            let Some(channel) = store.channels.get(&id) else {
                out.push(div().h(px(32.)).into_any_element());
                continue;
            };
            let name = channel.name.clone().unwrap_or_default();
            if row.kind == RowKind::Category {
                let collapsed = store.collapsed.contains(&id);
                out.push(
                    h_flex()
                        .id(("category", id.0))
                        .w_full()
                        .h(px(32.))
                        .px_2()
                        .pt_2()
                        .gap_1()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_muted())
                        .hover(|s| s.text_color(theme::text()))
                        .child(icon(
                            if collapsed { IconName::ChevronRight } else { IconName::ChevronDown },
                            12.,
                            theme::text_muted(),
                        ))
                        .child(div().flex_1().min_w_0().truncate().child(name.to_uppercase()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.store.update(cx, |store, cx| store.toggle_category(id, cx));
                        }))
                        .into_any_element(),
                );
                continue;
            }
            let selected = store.selected_channel == Some(id);
            let unread = !selected && store.is_unread(id);
            let mentions = store.mentions(id);
            let is_thread = row.kind == RowKind::Thread;
            let color = if selected || unread { theme::text_bright() } else { theme::text_muted() };
            out.push(
                div()
                    .id(("channel", id.0))
                    .relative()
                    .w_full()
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .when(unread, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(12.))
                                .w(px(4.))
                                .h(px(8.))
                                .rounded_r_md()
                                .bg(theme::text_bright()),
                        )
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(30.))
                            .px_2()
                            .when(is_thread, |d| d.pl_6())
                            .gap_1p5()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(color)
                            .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .when(selected, |d| d.bg(theme::bg_active()))
                            .when(!selected, |d| d.hover(|s| s.bg(theme::bg_hover()).text_color(theme::text())))
                            .child(icon(channel_icon(channel.kind), 16., theme::text_muted()))
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .when(mentions > 0, |d| d.child(badge(mentions))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.store.update(cx, |store, cx| store.select_channel(id, cx));
                    }))
                    .into_any_element(),
            );
        }
        out
    }

    fn dm_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let store = self.store.read(cx);
        let mut out = Vec::with_capacity(range.len());
        for id in store.dms.get(range).unwrap_or_default().iter().copied() {
            let Some(channel) = store.channels.get(&id) else {
                out.push(div().h(px(44.)).into_any_element());
                continue;
            };
            let selected = store.selected_channel == Some(id);
            let unread = !selected && store.is_unread(id);
            let title = store.channel_title(channel);
            let visual = match store.dm_peer(channel) {
                Some(peer) => {
                    let ring = if selected { theme::bg_active() } else { theme::bg_sidebar() };
                    avatar_with_status(peer.avatar_url(64), 32., store.status_of(peer.id), ring).into_any_element()
                }
                None => div()
                    .size(px(32.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(theme::accent())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(IconName::Users, 16., theme::bg_rail()))
                    .into_any_element(),
            };
            let subtitle = (channel.kind == channel_kind::GROUP_DM)
                .then(|| format!("{} members", channel.recipients.len() + 1));
            out.push(
                div()
                    .id(("dm", id.0))
                    .w_full()
                    .h(px(44.))
                    .px_2()
                    .flex()
                    .items_center()
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(42.))
                            .px_2()
                            .gap_3()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(if selected || unread { theme::text_bright() } else { theme::text_muted() })
                            .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .when(selected, |d| d.bg(theme::bg_active()))
                            .when(!selected, |d| d.hover(|s| s.bg(theme::bg_hover()).text_color(theme::text())))
                            .child(visual)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().truncate().child(title))
                                    .when_some(subtitle, |d, subtitle| {
                                        d.child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(theme::text_muted())
                                                .child(subtitle),
                                        )
                                    }),
                            )
                            .when(unread, |d| d.child(badge(store.mentions(id).max(1)))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.store.update(cx, |store, cx| store.select_channel(id, cx));
                    }))
                    .into_any_element(),
            );
        }
        out
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let guild = store.selected_guild.and_then(|id| store.guild(id));

        let (title, list) = match guild {
            Some(guild) => {
                // Collapsed categories still show the selected and unread channels.
                let mut rows = Vec::with_capacity(guild.rows.len());
                let mut hidden = false;
                for row in &guild.rows {
                    if row.kind == RowKind::Category {
                        hidden = store.collapsed.contains(&row.id);
                        rows.push(*row);
                    } else if !hidden || store.selected_channel == Some(row.id) || store.is_unread(row.id) {
                        rows.push(*row);
                    }
                }
                self.rows = rows;
                let id: Id = guild.guild.id;
                (
                    guild.guild.name.clone(),
                    uniform_list(("channels", id.0), self.rows.len(), cx.processor(Self::channel_rows))
                        .flex_1()
                        .into_any_element(),
                )
            }
            None => (
                "Direct Messages".to_string(),
                if store.dms.is_empty() {
                    div()
                        .flex_1()
                        .p_4()
                        .text_color(theme::text_muted())
                        .child("No direct messages yet.")
                        .into_any_element()
                } else {
                    uniform_list("dms", store.dms.len(), cx.processor(Self::dm_rows)).flex_1().into_any_element()
                },
            ),
        };

        let me = store.me.clone();
        let my_status = me.as_ref().map(|u| store.status_of(u.id)).unwrap_or_default();

        v_flex()
            .w(px(248.))
            .h_full()
            .flex_shrink_0()
            .bg(theme::bg_sidebar())
            .child(
                h_flex()
                    .h(px(48.))
                    .px_4()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme::border())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_bright())
                    .child(div().flex_1().min_w_0().truncate().child(title)),
            )
            .child(list)
            .when_some(me, |d, me| {
                d.child(
                    h_flex()
                        .h(px(54.))
                        .px_2()
                        .gap_2()
                        .flex_shrink_0()
                        .bg(theme::bg_rail())
                        .child(avatar_with_status(
                            me.avatar_url(64),
                            32.,
                            if my_status == Default::default() { crate::discord::Status::Online } else { my_status },
                            theme::bg_rail(),
                        ))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::text_bright())
                                        .child(me.display_name().to_string()),
                                )
                                .child(
                                    div().truncate().text_size(px(12.)).text_color(theme::text_muted()).child(me.tag()),
                                ),
                        )
                        .child(icon_button("logout", IconName::LogOut, "Log out").on_click(cx.listener(
                            |this, _, _, cx| {
                                credentials::clear();
                                this.store.update(cx, |store, cx| store.logout(cx));
                            },
                        ))),
                )
            })
    }
}
