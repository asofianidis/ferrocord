//! The member list on the right-hand side of guild channels.

use super::{
    form::{self, Field, FormSpec},
    theme,
    widgets::*,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use crate::{
    discord::{Id, permissions},
    store::{MemberRow, Store, StoreEvent},
};
use gpui::{prelude::*, *};
use std::ops::Range;

/// Width of the member list, which is always at the window's right edge.
const PANEL_WIDTH: f32 = 240.;

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
        let Some(guild) = store.selected_guild else { return Vec::new() };
        let Some(list) = store.member_rows() else {
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
                                        .child(name.clone()),
                                )
                                .when(user.bot, |d| d.child(bot_tag())),
                        )
                        .on_click(move |event, window, cx| {
                            // Beside the member list, which runs down the window's right edge.
                            let left_of_list = window.viewport_size().width - px(PANEL_WIDTH);
                            let at = point(left_of_list, event.position().y);
                            super::form::open_profile(user_id, at, cx);
                        })
                        .context_menu({
                            let store = self.store.clone();
                            let display = name.clone();
                            move |menu, _, cx| member_menu(menu, store.clone(), guild, user_id, display.clone(), cx)
                        })
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
        let count = store.member_rows().map_or(0, |l| l.len());
        // A list per channel keeps each one's scroll position separate.
        let key = store.selected_channel.map_or(0, |c| c.0);

        v_flex().w(px(PANEL_WIDTH)).h_full().flex_shrink_0().bg(theme::bg_sidebar()).child(if count == 0 {
            div()
                .p_4()
                .text_color(theme::text_muted())
                .child("Loading members\u{2026}")
                .into_any_element()
        } else {
            uniform_list(("members", key), count, cx.processor(Self::rows)).flex_1().into_any_element()
        })
    }
}

/// The right-click menu for a member: nickname, roles and moderation, as
/// far as our permissions allow.
fn member_menu(
    menu: gpui_component::menu::PopupMenu,
    store: Entity<Store>,
    guild: Id,
    user: Id,
    name: String,
    cx: &App,
) -> gpui_component::menu::PopupMenu {
    let s = store.read(cx);
    let me = s.my_id();
    let perms = s.guild_permissions(guild);
    let has = |bit: u64| perms & bit == bit;
    let is_me = user == me;
    let is_owner = s.guild(guild).and_then(|g| g.guild.owner_id) == Some(user);
    let top = s.top_role_position(guild);
    let current_roles = s.member_roles(guild, user);
    let current_nick = s.member_info.get(&(guild, user)).and_then(|m| m.nick.clone()).unwrap_or_default();
    let mut assignable: Vec<_> = s
        .guild(guild)
        .map(|g| g.guild.roles.iter().filter(|r| r.id != guild && r.position < top).cloned().collect())
        .unwrap_or_default();
    assignable.sort_by_key(|r| std::cmp::Reverse(r.position));

    let mut menu = menu;
    let mut any = false;
    if (is_me && has(1 << 26)) || has(permissions::MANAGE_NICKNAMES) {
        any = true;
        let store = store.clone();
        menu = menu.item(PopupMenuItem::new("Change Nickname").on_click(move |_, window, cx| {
            let store = store.clone();
            let spec = FormSpec::new("Change Nickname", "Save", move |result, _, cx| {
                let nick = Some(result.values[0].clone()).filter(|n| !n.is_empty());
                store.update(cx, |s, cx| s.set_nickname(guild, user, nick, cx));
            })
            .description("Leave it empty to reset.")
            .field(Field::new("Nickname", current_nick.clone()));
            form::open_form(spec, window, cx);
        }));
    }
    if has(permissions::MANAGE_ROLES) && !assignable.is_empty() {
        any = true;
        menu = menu.separator().label("Roles");
        for role in assignable {
            let on = current_roles.contains(&role.id);
            let store = store.clone();
            menu = menu.item(PopupMenuItem::new(role.name.clone()).checked(on).on_click(move |_, _, cx| {
                store.update(cx, |s, cx| s.set_member_role(guild, user, role.id, !on, cx));
            }));
        }
    }
    if !is_me && !is_owner {
        if has(permissions::MODERATE_MEMBERS) {
            any = true;
            menu = menu.separator();
            for (label, minutes) in
                [("Timeout for 1 Hour", Some(60)), ("Timeout for 1 Day", Some(1440)), ("Timeout for 1 Week", Some(10080)), ("Remove Timeout", None)]
            {
                let store = store.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    store.update(cx, |s, cx| s.timeout_member(guild, user, minutes, cx));
                }));
            }
        }
        if has(permissions::KICK_MEMBERS) || has(permissions::BAN_MEMBERS) {
            any = true;
            menu = menu.separator();
        }
        if has(permissions::KICK_MEMBERS) {
            let (store, name) = (store.clone(), name.clone());
            menu = menu.item(PopupMenuItem::new(format!("Kick {name}")).on_click(move |_, window, cx| {
                let store = store.clone();
                form::confirm(window, cx, &format!("Kick {name}?"), "They can rejoin with a new invite.", "Kick", move |cx| {
                    store.update(cx, |s, cx| s.kick_member(guild, user, cx));
                });
            }));
        }
        if has(permissions::BAN_MEMBERS) {
            let (store, name) = (store.clone(), name.clone());
            menu = menu.item(PopupMenuItem::new(format!("Ban {name}")).on_click(move |_, window, cx| {
                let store = store.clone();
                form::confirm(
                    window,
                    cx,
                    &format!("Ban {name}?"),
                    "Their messages from the last day are deleted too.",
                    "Ban",
                    move |cx| store.update(cx, |s, cx| s.ban_member(guild, user, 86_400, cx)),
                );
            }));
        }
    }
    if !any {
        menu = menu.label("No actions available");
    }
    menu.separator().item(PopupMenuItem::new("Copy User ID").on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(user.to_string()));
    }))
}
