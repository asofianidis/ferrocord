//! Channel list (or direct-message list) and the current-user panel.

use super::{
    call,
    form::{self, Field, FormSpec},
    theme,
    widgets::*,
};
use crate::{
    discord::{Id, channel_kind, permissions},
    store::{ChannelRow, RowKind, Store, StoreEvent},
};
use gpui::{prelude::*, *};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
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
                    | StoreEvent::Voice
                    | StoreEvent::Friends
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
            if row.kind == RowKind::VoiceUser {
                let guild = store.selected_guild;
                let (name, url) = match store.users.get(&id) {
                    Some(user) => (store.display_name(guild, user), user.avatar_url(64)),
                    None => ("Unknown user".to_string(), String::new()),
                };
                let speaking = store.speaking.contains(&id);
                let flags = store.voice_member(id).map(call::voice_flags);
                out.push(
                    h_flex()
                        .w_full()
                        .h(px(32.))
                        .pl(px(38.))
                        .pr_2()
                        .gap_1p5()
                        .text_size(px(13.))
                        .text_color(if speaking { theme::text_bright() } else { theme::text_muted() })
                        .child(call::voice_avatar(url, 18., speaking))
                        .child(div().flex_1().min_w_0().truncate().child(name))
                        .children(flags)
                        .into_any_element(),
                );
                continue;
            }
            let Some(channel) = store.channels.get(&id) else {
                out.push(div().h(px(32.)).into_any_element());
                continue;
            };
            let name = channel.name.clone().unwrap_or_default();
            if row.kind == RowKind::Category {
                let collapsed = store.collapsed.contains(&id);
                let category_admin =
                    channel.guild_id.filter(|g| store.can(*g, permissions::MANAGE_CHANNELS));
                let can_override = channel.guild_id.is_some_and(|g| store.can(g, permissions::MANAGE_ROLES));
                let row =
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
                        }));
                out.push(match category_admin {
                    Some(guild) => {
                        let store = self.store.clone();
                        row.context_menu(move |menu, _, _| {
                            let create = store.clone();
                            let menu = menu.item(PopupMenuItem::new("Create Channel").on_click(move |_, window, cx| {
                                form::open_form(create_channel_form(create.clone(), guild, Some(id)), window, cx);
                            }));
                            channel_admin_items(menu, store.clone(), id, true, can_override)
                        })
                        .into_any_element()
                    }
                    None => row.into_any_element(),
                });
                continue;
            }
            // A forum stays highlighted while one of its posts is open.
            let selected = store.selected_channel == Some(id)
                || store
                    .selected_channel
                    .and_then(|open| store.channels.get(&open))
                    .is_some_and(|open| open.is_thread() && open.parent_id == Some(id));
            let unread = !selected && store.is_unread(id);
            let mentions = store.mentions(id);
            let is_thread = row.kind == RowKind::Thread;
            let muted = store.muted.contains(&id);
            let is_voice = channel.is_voice();
            let can_manage = channel.guild_id.is_some_and(|g| store.can(g, permissions::MANAGE_CHANNELS));
            let can_override = channel.guild_id.is_some_and(|g| store.can(g, permissions::MANAGE_ROLES));
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
                            .when(muted && !selected, |d| d.opacity(0.45))
                            .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .when(selected, |d| d.bg(theme::bg_active()))
                            .when(!selected, |d| d.hover(|s| s.bg(theme::bg_hover()).text_color(theme::text())))
                            .child(icon(channel_icon(channel.kind), 16., theme::text_muted()))
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .when(mentions > 0, |d| d.child(badge(mentions))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.store.update(cx, |store, cx| {
                            store.select_channel(id, cx);
                            if is_voice {
                                store.join_voice(id, cx);
                            }
                        });
                    }))
                    .context_menu({
                        let store = self.store.clone();
                        move |menu, _, _| {
                            let store = store.clone();
                            let (mute, invite, read) = (store.clone(), store.clone(), store.clone());
                            let mut menu = menu.item(PopupMenuItem::new("Mark as Read").on_click(move |_, _, cx| {
                                read.update(cx, |store, cx| store.mark_read(id, cx));
                            }))
                            .item(PopupMenuItem::new(if muted { "Unmute Channel" } else { "Mute Channel" }).on_click(
                                move |_, _, cx| mute.update(cx, |store, cx| store.toggle_mute(id, cx)),
                            ))
                            .item(PopupMenuItem::new("Copy Invite Link").on_click(move |_, _, cx| {
                                invite.update(cx, |store, cx| store.copy_invite(id, cx));
                            }))
                            .separator()
                            .item(PopupMenuItem::new("Copy Channel ID").on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(id.to_string()));
                            }));
                            if can_manage {
                                menu = channel_admin_items(menu, store, id, false, can_override);
                            }
                            menu
                        }
                    })
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
                    .child(icon(IconName::Users, 16., theme::on_accent()))
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
                    .context_menu({
                        let store = self.store.clone();
                        let muted = store.read(cx).muted.contains(&id);
                        let is_group = channel.kind == channel_kind::GROUP_DM;
                        move |menu, _, _| {
                            let (read, mute, close) = (store.clone(), store.clone(), store.clone());
                            menu.item(PopupMenuItem::new("Mark as Read").on_click(move |_, _, cx| {
                                read.update(cx, |store, cx| store.mark_read(id, cx));
                            }))
                            .item(PopupMenuItem::new(if muted { "Unmute" } else { "Mute" }).on_click(
                                move |_, _, cx| mute.update(cx, |store, cx| store.toggle_mute(id, cx)),
                            ))
                            .item(PopupMenuItem::new(if is_group { "Leave Group" } else { "Close Conversation" }).on_click(
                                move |_, _, cx| close.update(cx, |store, cx| store.close_dm(id, cx)),
                            ))
                        }
                    })
                    .into_any_element(),
            );
        }
        out
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let voice_panel = call::connection_panel(&self.store, cx);
        let store = self.store.read(cx);
        let (self_mute, self_deaf) = (store.self_mute, store.self_deaf);
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
                        continue;
                    }
                    let occupants = store.voice.get(&row.id);
                    if !hidden
                        || store.selected_channel == Some(row.id)
                        || store.is_unread(row.id)
                        || occupants.is_some()
                    {
                        rows.push(*row);
                        for user in occupants.into_iter().flatten() {
                            rows.push(ChannelRow { id: user.user, kind: RowKind::VoiceUser });
                        }
                    }
                }
                self.rows = rows;
                let id: Id = guild.guild.id;
                let upcoming = store.upcoming_events(id);
                let live = upcoming.iter().any(|e| e.is_live());
                let show_events = !upcoming.is_empty() || store.can(id, super::events::MANAGE_EVENTS);
                let events_row = h_flex()
                    .id("events-row")
                    .mx_2()
                    .mt_2()
                    .px_2()
                    .h(px(32.))
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(if live { theme::online() } else { theme::text_muted() })
                    .hover(|s| s.bg(theme::bg_hover()).text_color(theme::text_bright()))
                    .child(icon(IconName::Calendar, 16., if live { theme::online() } else { theme::text_muted() }))
                    .child(div().flex_1().child(if live { "Event happening now" } else { "Events" }))
                    .when(!upcoming.is_empty(), |d| d.child(div().text_size(px(12.)).child(upcoming.len().to_string())))
                    .on_click(move |_, _, cx| form::open_events(id, cx));
                (
                    guild.guild.name.clone(),
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .when(show_events, |d| d.child(events_row))
                        .child(
                            uniform_list(("channels", id.0), self.rows.len(), cx.processor(Self::channel_rows)).flex_1(),
                        )
                        .into_any_element(),
                )
            }
            None => (
                "Direct Messages".to_string(),
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child({
                        let active = store.show_friends;
                        let pending = store.relationships.iter().filter(|(_, kind)| *kind == 3).count() as u32;
                        div().px_2().pt_2().pb_1().flex_shrink_0().child(
                            h_flex()
                                .id("friends-entry")
                                .h(px(38.))
                                .px_2()
                                .gap_3()
                                .rounded_md()
                                .cursor_pointer()
                                .text_color(if active { theme::text_bright() } else { theme::text_muted() })
                                .when(active, |d| d.bg(theme::bg_active()))
                                .when(!active, |d| d.hover(|s| s.bg(theme::bg_hover()).text_color(theme::text())))
                                .child(icon(IconName::Users, 18., theme::text_muted()))
                                .child(div().flex_1().child("Friends"))
                                .when(pending > 0, |d| d.child(badge(pending)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.store.update(cx, |store, cx| store.open_friends(cx));
                                })),
                        )
                    })
                    .child(if store.dms.is_empty() {
                    div()
                        .flex_1()
                        .p_4()
                        .text_color(theme::text_muted())
                        .child("No direct messages yet.")
                        .into_any_element()
                } else {
                    uniform_list("dms", store.dms.len(), cx.processor(Self::dm_rows)).flex_1().into_any_element()
                    })
                    .into_any_element(),
            ),
        };

        let me = store.me.clone();
        let my_status = store.my_status;
        let header_guild = store.selected_guild.filter(|g| store.guild(*g).is_some());
        let can_create = header_guild.is_some_and(|g| store.can(g, permissions::MANAGE_CHANNELS));
        let can_settings = header_guild.is_some_and(|g| {
            store.guild_permissions(g)
                & (permissions::MANAGE_GUILD | permissions::MANAGE_ROLES | permissions::BAN_MEMBERS)
                != 0
        });

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
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .when_some(header_guild.filter(|_| can_create), |d, guild| {
                        let store = self.store.clone();
                        d.child(icon_button("create-channel", IconName::Plus, "Create channel").on_click(
                            move |_, window, cx| form::open_form(create_channel_form(store.clone(), guild, None), window, cx),
                        ))
                    })
                    .when_some(header_guild.filter(|_| can_settings), |d, guild| {
                        d.child(icon_button("server-settings", IconName::Settings, "Server settings").on_click(
                            move |_, window, cx| form::open_server_settings(guild, window, cx),
                        ))
                    }),
            )
            .child(list)
            .children(voice_panel)
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
                            my_status,
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
                        .child(
                            icon_button_tinted(
                                "self-mute",
                                if self_mute { IconName::MicOff } else { IconName::Mic },
                                if self_mute { "Unmute" } else { "Mute" },
                                if self_mute { theme::danger() } else { theme::text_muted() },
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.store.update(cx, |store, cx| store.toggle_self_mute(cx));
                            })),
                        )
                        .child(
                            icon_button_tinted(
                                "self-deaf",
                                if self_deaf { IconName::HeadphoneOff } else { IconName::Headphones },
                                if self_deaf { "Undeafen" } else { "Deafen" },
                                if self_deaf { theme::danger() } else { theme::text_muted() },
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.store.update(cx, |store, cx| store.toggle_self_deaf(cx));
                            })),
                        )
                        .child(icon_button("settings", IconName::Settings, "Settings (Ctrl+,)").on_click(
                            |_, window, cx| window.dispatch_action(Box::new(super::ToggleSettings), cx),
                        )),
                )
            })
    }
}

const CHANNEL_TYPES: [(&str, u8); 6] = [
    ("Text", channel_kind::TEXT),
    ("Voice", channel_kind::VOICE),
    ("Forum", channel_kind::FORUM),
    ("Stage", channel_kind::STAGE),
    ("Announcement", channel_kind::ANNOUNCEMENT),
    ("Category", channel_kind::CATEGORY),
];

fn create_channel_form(store: Entity<Store>, guild: Id, parent: Option<Id>) -> FormSpec {
    let labels: Vec<&str> = CHANNEL_TYPES.iter().map(|(label, _)| *label).collect();
    FormSpec::new("Create Channel", "Create", move |result, _, cx| {
        let name = result.values[0].clone();
        if name.is_empty() {
            return;
        }
        let kind = CHANNEL_TYPES[result.choice].1;
        // Categories cannot sit inside other categories.
        let parent = if kind == channel_kind::CATEGORY { None } else { parent };
        store.update(cx, |s, cx| s.create_channel(guild, name, kind, parent, cx));
    })
    .field(Field::new("Name", "").placeholder("new-channel"))
    .choice("Type", &labels, 0)
}

/// Edit and delete entries for a channel or category menu.
fn channel_admin_items(
    menu: gpui_component::menu::PopupMenu,
    store: Entity<Store>,
    id: Id,
    is_category: bool,
    can_override: bool,
) -> gpui_component::menu::PopupMenu {
    let (edit, delete) = (store.clone(), store);
    let noun = if is_category { "Category" } else { "Channel" };
    menu.separator()
        .when(can_override, |m| {
            m.item(PopupMenuItem::new("Permissions").on_click(move |_, window, cx| form::open_channel_permissions(id, window, cx)))
        })
        .item(PopupMenuItem::new(format!("Edit {noun}")).on_click(move |_, window, cx| {
            let Some(channel) = edit.read(cx).channels.get(&id).cloned() else { return };
            let has_topic = channel.is_text_like() && !channel.is_voice() && !is_category;
            let store = edit.clone();
            let mut spec = FormSpec::new(format!("Edit {noun}"), "Save", move |result, _, cx| {
                let name = result.values[0].clone();
                if name.is_empty() {
                    return;
                }
                let topic = result.values.get(1).cloned();
                store.update(cx, |s, cx| s.edit_channel(id, name, topic, cx));
            })
            .field(Field::new("Name", channel.name.clone().unwrap_or_default()));
            if has_topic {
                spec = spec.field(Field::new("Topic", channel.topic.clone().unwrap_or_default()));
            }
            form::open_form(spec, window, cx);
        }))
        .item(PopupMenuItem::new(format!("Delete {noun}")).on_click(move |_, window, cx| {
            let name = delete.read(cx).channels.get(&id).and_then(|c| c.name.clone()).unwrap_or_default();
            let store = delete.clone();
            form::confirm(window, cx, &format!("Delete {name}?"), "This cannot be undone.", "Delete", move |cx| {
                store.update(cx, |s, cx| s.delete_channel(id, cx));
            });
        }))
}
