//! The right-click menu for a user, wherever they appear: message authors,
//! the member list, voice channel rows and call tiles.

use super::form::{self, Field, FormSpec};
use crate::{
    discord::{Id, permissions},
    store::{Store, VoiceModeration},
};
use gpui::*;
use gpui_component::menu::{PopupMenu, PopupMenuItem};

/// Per-user volume choices, as linear gains.
const VOLUMES: [(&str, f32); 5] = [("25%", 0.25), ("50%", 0.5), ("100%", 1.0), ("150%", 1.5), ("200%", 2.0)];

/// Builds the menu for `user`. `guild` is the server they are seen in, if any.
pub fn user_menu(menu: PopupMenu, store: Entity<Store>, user: Id, guild: Option<Id>, cx: &App) -> PopupMenu {
    let s = store.read(cx);
    let me = s.my_id();
    let is_me = user == me;
    let person = s.users.get(&user).cloned().unwrap_or_default();
    let name = s.display_name(guild, &person);
    let is_bot = person.bot;

    let mut menu = menu;
    menu = menu.item(PopupMenuItem::new("Profile").on_click(move |_, window, cx| {
        form::open_profile_beside(user, window.mouse_position(), cx);
    }));
    if s.selected_channel.is_some() {
        menu = menu.item(PopupMenuItem::new("Mention").on_click(move |_, window, cx| form::mention(user, window, cx)));
    }
    if !is_me {
        let store = store.clone();
        menu = menu.item(PopupMenuItem::new("Message").on_click(move |_, _, cx| {
            store.update(cx, |s, cx| s.open_dm(user, cx));
        }));
    }

    // Our own volume and mute for someone in our voice channel.
    let in_our_call = s
        .voice_session
        .as_ref()
        .is_some_and(|session| s.voice_members(session.channel).iter().any(|m| m.user == user));
    if in_our_call && !is_me {
        let muted = s.locally_muted(user);
        let volume = s.user_volume(user);
        let toggle = store.clone();
        menu = menu.separator().item(PopupMenuItem::new("Mute").checked(muted).on_click(move |_, _, cx| {
            toggle.update(cx, |s, cx| s.toggle_local_mute(user, cx));
        }));
        menu = menu.label("User Volume");
        for (label, gain) in VOLUMES {
            let store = store.clone();
            menu = menu.item(PopupMenuItem::new(label).checked((volume - gain).abs() < 0.01).on_click(move |_, _, cx| {
                store.update(cx, |s, cx| s.set_user_volume(user, gain, cx));
            }));
        }
    }

    // Friends and blocking (user accounts only).
    if !is_me && !is_bot && !s.is_bot() {
        menu = menu.separator();
        let relationship = s.relationship(user);
        let action = |label: &'static str, f: fn(&mut Store, Id, &mut Context<Store>)| {
            let store = store.clone();
            PopupMenuItem::new(label).on_click(move |_, _, cx| store.update(cx, |s, cx| f(s, user, cx)))
        };
        menu = match relationship {
            Some(1) => menu.item(action("Remove Friend", Store::remove_relationship)),
            Some(3) => menu
                .item(action("Accept Friend Request", Store::accept_friend))
                .item(action("Ignore Friend Request", Store::remove_relationship)),
            Some(4) => menu.item(action("Cancel Friend Request", Store::remove_relationship)),
            Some(2) => menu,
            _ => menu.item(action("Add Friend", Store::send_friend_request)),
        };
        menu = if relationship == Some(2) {
            menu.item(action("Unblock", Store::remove_relationship))
        } else {
            menu.item(action("Block", Store::block_user))
        };
    }

    if let Some(guild) = guild {
        menu = moderation_items(menu, store.clone(), guild, user, name, cx);
    }

    menu.separator().item(PopupMenuItem::new("Copy User ID").on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(user.to_string()));
    }))
}

/// Nickname, roles, timeouts, kicks, bans and voice moderation, as far as
/// our permissions in `guild` allow.
fn moderation_items(menu: PopupMenu, store: Entity<Store>, guild: Id, user: Id, name: String, cx: &App) -> PopupMenu {
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
    // Voice moderation applies to people in a voice channel of this server.
    let voice = s.voice_member(user).filter(|_| {
        s.voice.iter().any(|(channel, members)| {
            members.iter().any(|m| m.user == user) && s.channels.get(channel).and_then(|c| c.guild_id) == Some(guild)
        })
    });
    let voice = voice.cloned();

    let mut menu = menu;
    if (is_me && has(1 << 26)) || has(permissions::MANAGE_NICKNAMES) {
        let store = store.clone();
        menu = menu.separator().item(PopupMenuItem::new("Change Nickname").on_click(move |_, window, cx| {
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
        menu = menu.separator().label("Roles");
        for role in assignable {
            let on = current_roles.contains(&role.id);
            let store = store.clone();
            menu = menu.item(PopupMenuItem::new(role.name.clone()).checked(on).on_click(move |_, _, cx| {
                store.update(cx, |s, cx| s.set_member_role(guild, user, role.id, !on, cx));
            }));
        }
    }
    if let Some(member) = voice.filter(|_| !is_me) {
        let act = |change: VoiceModeration| {
            let store = store.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| store.update(cx, |s, cx| s.moderate_voice(guild, user, change, cx))
        };
        let mut any = false;
        if has(permissions::MUTE_MEMBERS) {
            any = true;
            menu = menu.separator().item(
                PopupMenuItem::new(if member.mute { "Server Unmute" } else { "Server Mute" })
                    .on_click(act(VoiceModeration::Mute(!member.mute))),
            );
        }
        if has(permissions::DEAFEN_MEMBERS) {
            if !any {
                menu = menu.separator();
            }
            any = true;
            menu = menu.item(
                PopupMenuItem::new(if member.deaf { "Server Undeafen" } else { "Server Deafen" })
                    .on_click(act(VoiceModeration::Deafen(!member.deaf))),
            );
        }
        if has(permissions::MOVE_MEMBERS) {
            if !any {
                menu = menu.separator();
            }
            menu = menu.item(PopupMenuItem::new("Disconnect").on_click(act(VoiceModeration::Disconnect)));
        }
    }
    if !is_me && !is_owner {
        if has(permissions::MODERATE_MEMBERS) {
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
    menu
}
