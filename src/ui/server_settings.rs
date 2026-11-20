//! Server settings: overview, roles, bans and the audit log.

use super::{form, theme, widgets::*};
use crate::{
    discord::{
        Ban, Id, Webhook,
        audit::{self, TargetKind},
        permissions as p,
        rest::Upload,
    },
    store::{AuditLog, Store, StoreEvent, image_mime},
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};
use gpui_kit::assets::IconName;

pub enum ServerSettingsEvent {
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Overview,
    Roles,
    Emoji,
    Stickers,
    Webhooks,
    Bans,
    AuditLog,
}

const ROLE_COLORS: [u32; 12] = [
    0, 0x1abc9c, 0x2ecc71, 0x3498db, 0x9b59b6, 0xe91e63, 0xf1c40f, 0xe67e22, 0xe74c3c, 0x95a5a6, 0x607d8b, 0xd4387c,
];

pub(super) const PERMISSIONS: [(&str, u64); 32] = [
    ("Administrator", p::ADMINISTRATOR),
    ("View Channels", p::VIEW_CHANNEL),
    ("Manage Channels", p::MANAGE_CHANNELS),
    ("Manage Roles", p::MANAGE_ROLES),
    ("Manage Server", p::MANAGE_GUILD),
    ("View Audit Log", 1 << 7),
    ("Manage Webhooks", 1 << 29),
    ("Manage Expressions", 1 << 30),
    ("Create Invite", 1 << 0),
    ("Change Nickname", 1 << 26),
    ("Manage Nicknames", p::MANAGE_NICKNAMES),
    ("Kick Members", p::KICK_MEMBERS),
    ("Ban Members", p::BAN_MEMBERS),
    ("Timeout Members", p::MODERATE_MEMBERS),
    ("Send Messages", p::SEND_MESSAGES),
    ("Send Messages in Threads", 1 << 38),
    ("Create Public Threads", 1 << 35),
    ("Embed Links", 1 << 14),
    ("Attach Files", 1 << 15),
    ("Add Reactions", 1 << 6),
    ("Mention Everyone", 1 << 17),
    ("Manage Messages", p::MANAGE_MESSAGES),
    ("Read Message History", 1 << 16),
    ("Use Application Commands", 1 << 31),
    ("Connect", p::CONNECT),
    ("Speak", p::SPEAK),
    ("Video", 1 << 9),
    ("Priority Speaker", 1 << 8),
    ("Mute Members", p::MUTE_MEMBERS),
    ("Deafen Members", p::DEAFEN_MEMBERS),
    ("Move Members", p::MOVE_MEMBERS),
    ("Request to Speak", 1 << 32),
];

pub struct ServerSettings {
    store: Entity<Store>,
    guild: Id,
    tab: Tab,
    name: Entity<InputState>,
    role_name: Entity<InputState>,
    role: Option<Id>,
    color: u32,
    permissions: u64,
    hoist: bool,
    bans: Option<Vec<Ban>>,
    audit: Option<AuditLog>,
    webhooks: Option<Vec<Webhook>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ServerSettingsEvent> for ServerSettings {}

impl ServerSettings {
    pub fn new(store: Entity<Store>, guild: Id, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = store.read(cx).guild(guild).map(|g| g.guild.name.clone()).unwrap_or_default();
        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(current, window, cx);
            s
        });
        let role_name = cx.new(|cx| InputState::new(window, cx).placeholder("Role name"));
        let subs = vec![cx.subscribe(&store, move |this, store, event: &StoreEvent, cx| {
            if matches!(event, StoreEvent::Guilds) && store.read(cx).guild(this.guild).is_none() {
                cx.emit(ServerSettingsEvent::Close);
            }
            cx.notify();
        })];
        Self {
            store,
            guild,
            tab: Tab::Overview,
            name,
            role_name,
            role: None,
            color: 0,
            permissions: 0,
            hoist: false,
            bans: None,
            audit: None,
            webhooks: None,
            _subs: subs,
        }
    }

    fn select_role(&mut self, role: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(found) = self.store.read(cx).guild(self.guild).and_then(|g| g.guild.roles.iter().find(|r| r.id == role)).cloned()
        else {
            return;
        };
        self.role = Some(role);
        self.color = found.color;
        self.permissions = found.permissions;
        self.hoist = found.hoist;
        self.role_name.update(cx, |s, cx| s.set_value(found.name, window, cx));
        cx.notify();
    }

    fn open_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == Tab::Webhooks && self.webhooks.is_none() && self.store.read(cx).can(self.guild, MANAGE_WEBHOOKS) {
            let this = cx.weak_entity();
            let guild = self.guild;
            self.store.update(cx, |store, cx| {
                store.fetch_webhooks(guild, cx, move |hooks, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.webhooks = Some(hooks);
                        cx.notify();
                    });
                });
            });
        }
        if tab == Tab::AuditLog && self.audit.is_none() {
            let this = cx.weak_entity();
            let guild = self.guild;
            self.store.update(cx, |store, cx| {
                store.fetch_audit_log(guild, cx, move |log, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.audit = Some(log);
                        cx.notify();
                    });
                });
            });
        }
        if tab == Tab::Bans && self.bans.is_none() {
            let this = cx.weak_entity();
            let guild = self.guild;
            self.store.update(cx, |store, cx| {
                store.fetch_bans(guild, cx, move |bans, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.bans = Some(bans);
                        cx.notify();
                    });
                });
            });
        }
        cx.notify();
    }

    fn pick_icon(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose icon".into()),
        });
        let (store, guild) = (self.store.clone(), self.guild);
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let bytes = cx.background_executor().spawn(async move { std::fs::read(path) }).await;
            let _ = store.update(cx, |store, cx| match bytes {
                Ok(bytes) => match image_mime(&bytes) {
                    Some(mime) => store.set_guild_icon(guild, Some((mime.to_string(), bytes)), cx),
                    None => cx.emit(StoreEvent::Error("Choose a PNG, JPEG, GIF or WebP image.".into())),
                },
                Err(e) => cx.emit(StoreEvent::Error(format!("Could not read the image: {e}"))),
            });
        })
        .detach();
    }

    fn button(id: impl Into<ElementId>, label: &'static str, primary: bool) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .map(|d| {
                if primary {
                    d.bg(theme::accent()).text_color(theme::on_accent())
                } else {
                    d.bg(theme::bg_input()).text_color(theme::text())
                }
            })
            .hover(|s| s.opacity(0.85))
            .child(label)
    }

    fn render_overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let Some(guild) = store.guild(self.guild) else { return div().into_any_element() };
        let can_manage = store.can(self.guild, p::MANAGE_GUILD);
        let icon = guild.guild.icon_url(128);
        let initials_text = initials(&guild.guild.name);
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        div()
                            .size(px(80.))
                            .flex_shrink_0()
                            .rounded_full()
                            .overflow_hidden()
                            .bg(theme::bg_chat())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(22.))
                            .child(match icon {
                                Some(url) => img(url).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                                None => div().child(initials_text).into_any_element(),
                            }),
                    )
                    .when(can_manage, |d| {
                        d.child(
                            v_flex()
                                .gap_2()
                                .child(Self::button("icon-change", "Change Icon", false).on_click(cx.listener(
                                    |this, _, _, cx| this.pick_icon(cx),
                                )))
                                .child(Self::button("icon-remove", "Remove Icon", false).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        let guild = this.guild;
                                        this.store.update(cx, |s, cx| s.set_guild_icon(guild, None, cx));
                                    },
                                ))),
                        )
                    }),
            )
            .child(div().text_size(px(12.)).text_color(theme::text_muted()).child("Server name"))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.name).disabled(!can_manage)))
                    .when(can_manage, |d| {
                        d.child(Self::button("rename", "Save", true).on_click(cx.listener(|this, _, _, cx| {
                            let name = this.name.read(cx).value().trim().to_string();
                            if !name.is_empty() {
                                let guild = this.guild;
                                this.store.update(cx, |s, cx| s.rename_guild(guild, name, cx));
                            }
                        })))
                    }),
            )
            .when(!can_manage, |d| {
                d.child(div().text_color(theme::text_muted()).child("You need Manage Server to change these."))
            })
            .into_any_element()
    }

    fn render_roles(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let Some(guild) = store.guild(self.guild) else { return div().into_any_element() };
        let can_manage = store.can(self.guild, p::MANAGE_ROLES);
        let top = store.top_role_position(self.guild);
        let everyone = self.guild;
        let mut roles = store.ordered_roles(self.guild);
        roles.extend(guild.guild.roles.iter().find(|r| r.id == everyone).cloned());
        let movable = roles.len().saturating_sub(1);
        let editing = self.role.and_then(|id| roles.iter().find(|r| r.id == id)).cloned();
        let editable = editing.as_ref().is_some_and(|r| can_manage && (r.position < top || r.id == everyone));

        let list = v_flex()
            .id("role-list")
            .w(px(190.))
            .flex_shrink_0()
            .gap_1()
            .overflow_y_scroll()
            .when(can_manage, |d| {
                d.child(Self::button("role-create", "Create Role", true).mb_2().on_click(cx.listener(
                    |this, _, _, cx| {
                        let this_entity = cx.weak_entity();
                        let guild = this.guild;
                        this.store.update(cx, |s, cx| {
                            s.create_role(guild, cx, move |role, cx| {
                                if let Some(window) = cx.active_window() {
                                    let _ = window.update(cx, |_, window, cx| {
                                        let _ = this_entity.update(cx, |this, cx| this.select_role(role, window, cx));
                                    });
                                }
                            });
                        });
                    },
                )))
            })
            .children(roles.iter().enumerate().map(|(ix, role)| {
                let id = role.id;
                let selected = self.role == Some(id);
                let label = if id == everyone { "@everyone".to_string() } else { role.name.clone() };
                // Arrows show on the selected role when it can move and stays below ours.
                let can_move = |other: Option<&crate::discord::Role>| {
                    can_manage && id != everyone && role.position < top && other.is_some_and(|o| o.position < top)
                };
                let up = selected && ix > 0 && can_move(roles.get(ix - 1));
                let down = selected && ix + 1 < movable && can_move(roles.get(ix + 1));
                h_flex()
                    .id(("role", id.0))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::bg_active()))
                    .hover(|s| s.bg(theme::bg_hover()))
                    .child(div().size(px(10.)).rounded_full().flex_shrink_0().bg(if role.color == 0 {
                        theme::text_muted()
                    } else {
                        theme::c(role.color)
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when(up, |d| {
                        d.child(
                            icon_button(("role-up", id.0), IconName::ChevronUp, "Move up").size(px(20.)).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let guild = this.guild;
                                    this.store.update(cx, |s, cx| s.move_role(guild, id, true, cx));
                                }),
                            ),
                        )
                    })
                    .when(down, |d| {
                        d.child(
                            icon_button(("role-down", id.0), IconName::ChevronDown, "Move down").size(px(20.)).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let guild = this.guild;
                                    this.store.update(cx, |s, cx| s.move_role(guild, id, false, cx));
                                }),
                            ),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.select_role(id, window, cx)))
            }));

        let editor = match editing {
            None => div().text_color(theme::text_muted()).child("Choose a role to see its settings.").into_any_element(),
            Some(role) => {
                let role_id = role.id;
                let is_everyone = role_id == everyone;
                let mut col = v_flex().flex_1().min_w_0().gap_2();
                if !editable {
                    col = col.child(div().text_color(theme::warning()).child(if can_manage {
                        "This role is at or above your highest role, so you cannot change it."
                    } else {
                        "You need Manage Roles to change roles."
                    }));
                }
                if !is_everyone {
                    col = col
                        .child(div().text_size(px(12.)).text_color(theme::text_muted()).child("Name"))
                        .child(Input::new(&self.role_name).disabled(!editable))
                        .child(div().text_size(px(12.)).text_color(theme::text_muted()).child("Colour"))
                        .child(h_flex().flex_wrap().gap_1p5().children(swatches(role.color).into_iter().enumerate().map(|(ix, color)| {
                            let selected = self.color == color;
                            div()
                                .id(("role-color", ix))
                                .size(px(24.))
                                .rounded_md()
                                .cursor_pointer()
                                .border_2()
                                .border_color(if selected { theme::text_bright() } else { gpui::transparent_black() })
                                .bg(if color == 0 { theme::text_muted() } else { theme::c(color) })
                                .when(editable, |d| {
                                    d.on_click(cx.listener(move |this, _, _, cx| {
                                        this.color = color;
                                        cx.notify();
                                    }))
                                })
                        })))
                        .child(
                            h_flex()
                                .id("role-hoist")
                                .gap_2()
                                .cursor_pointer()
                                .child(checkbox(self.hoist))
                                .child("Show members separately in the member list")
                                .when(editable, |d| {
                                    d.on_click(cx.listener(|this, _, _, cx| {
                                        this.hoist = !this.hoist;
                                        cx.notify();
                                    }))
                                }),
                        );
                }
                col = col
                    .child(div().pt_1().text_size(px(12.)).text_color(theme::text_muted()).child("Permissions"))
                    .child(h_flex().flex_wrap().gap_y_1().children(PERMISSIONS.iter().enumerate().map(|(ix, (label, bit))| {
                        let bit = *bit;
                        let on = self.permissions & bit == bit;
                        h_flex()
                            .id(("perm", ix))
                            .w(px(205.))
                            .gap_2()
                            .cursor_pointer()
                            .child(checkbox(on))
                            .child(div().text_size(px(13.)).child(*label))
                            .when(editable, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.permissions ^= bit;
                                    cx.notify();
                                }))
                            })
                    })));
                if editable {
                    col = col.child(
                        h_flex()
                            .pt_2()
                            .gap_2()
                            .child(Self::button("role-save", "Save Changes", true).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let name = if is_everyone {
                                        "@everyone".to_string()
                                    } else {
                                        this.role_name.read(cx).value().trim().to_string()
                                    };
                                    let (guild, color, perms, hoist) = (this.guild, this.color, this.permissions, this.hoist);
                                    this.store.update(cx, |s, cx| s.save_role(guild, role_id, name, color, perms, hoist, cx));
                                },
                            )))
                            .when(!is_everyone, |d| {
                                d.child(Self::button("role-delete", "Delete Role", false).on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        let (store, guild) = (this.store.clone(), this.guild);
                                        this.role = None;
                                        form::confirm(window, cx, "Delete role?", "Members lose this role.", "Delete", move |cx| {
                                            store.update(cx, |s, cx| s.delete_role(guild, role_id, cx));
                                        });
                                    },
                                )))
                            }),
                    );
                }
                col.into_any_element()
            }
        };
        h_flex().items_start().gap_4().h(px(430.)).child(list).child(
            div().id("role-editor").flex_1().min_w_0().h_full().overflow_y_scroll().child(editor),
        ).into_any_element()
    }

    fn render_bans(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_ban = self.store.read(cx).can(self.guild, p::BAN_MEMBERS);
        let Some(bans) = &self.bans else {
            return div().text_color(theme::text_muted()).child("Loading bans\u{2026}").into_any_element();
        };
        if bans.is_empty() {
            return div().text_color(theme::text_muted()).child("Nobody is banned.").into_any_element();
        }
        v_flex()
            .gap_1()
            .children(bans.iter().map(|ban| {
                let user = ban.user.id;
                h_flex()
                    .id(("ban", user.0))
                    .px_2()
                    .py_1p5()
                    .gap_3()
                    .rounded_md()
                    .hover(|s| s.bg(theme::bg_hover()))
                    .child(avatar(ban.user.avatar_url(64), 28.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_color(theme::text_bright()).child(ban.user.tag()))
                            .children(ban.reason.clone().map(|r| {
                                div().truncate().text_size(px(12.)).text_color(theme::text_muted()).child(r)
                            })),
                    )
                    .when(can_ban, |d| {
                        d.child(Self::button(("unban", user.0), "Unban", false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let this_entity = cx.weak_entity();
                                let guild = this.guild;
                                this.store.update(cx, |s, cx| {
                                    s.unban(guild, user, cx, move |cx| {
                                        let _ = this_entity.update(cx, |this, cx| {
                                            if let Some(bans) = this.bans.as_mut() {
                                                bans.retain(|b| b.user.id != user);
                                            }
                                            cx.notify();
                                        });
                                    });
                                });
                            },
                        )))
                    })
            }))
            .into_any_element()
    }
}

impl ServerSettings {
    fn render_audit(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        if !store.can(self.guild, 1 << 7) {
            return div().text_color(theme::text_muted()).child("You need View Audit Log to see this.").into_any_element();
        }
        let Some(log) = &self.audit else {
            return div().text_color(theme::text_muted()).child("Loading the audit log\u{2026}").into_any_element();
        };
        if log.entries.is_empty() {
            return div().text_color(theme::text_muted()).child("Nothing has been logged yet.").into_any_element();
        }
        let guild = store.guild(self.guild);
        let name_target = |entry: &audit::AuditEntry, kind: TargetKind| -> Option<String> {
            let target = entry.target?;
            match kind {
                TargetKind::Channel | TargetKind::Thread => store
                    .channels
                    .get(&target)
                    .and_then(|c| c.name.clone())
                    .map(|n| format!("#{n}"))
                    .or(Some("a deleted channel".into())),
                TargetKind::Role => guild
                    .and_then(|g| g.guild.roles.iter().find(|r| r.id == target))
                    .map(|r| format!("@{}", r.name))
                    .or(Some("a deleted role".into())),
                TargetKind::User => log
                    .users
                    .get(&target)
                    .or_else(|| store.users.get(&target))
                    .map(|u| u.display_name().to_string()),
                _ => None,
            }
        };
        v_flex()
            .gap_1()
            .children(log.entries.iter().map(|entry| {
                let (verb, kind) = audit::describe(entry.action);
                let who = entry.user.as_ref().map_or_else(|| "Someone".to_string(), |u| u.display_name().to_string());
                let target = name_target(entry, kind);
                let mut line = format!("{who} {verb}");
                if let Some(target) = target {
                    line.push(' ');
                    line.push_str(&target);
                }
                let mut detail = vec![entry.id.time().format("%b %-d, %H:%M").to_string()];
                if !entry.changed.is_empty() {
                    detail.push(format!("changed {}", entry.changed.join(", ").replace('_', " ")));
                }
                if let Some(reason) = &entry.reason {
                    detail.push(format!("reason: {reason}"));
                }
                h_flex()
                    .px_2()
                    .py_1p5()
                    .gap_3()
                    .rounded_md()
                    .hover(|s| s.bg(theme::bg_hover()))
                    .child(avatar(entry.user.as_ref().map(|u| u.avatar_url(64)).unwrap_or_default(), 28.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_color(theme::text_bright()).child(line))
                            .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(detail.join(" \u{b7} "))),
                    )
            }))
            .into_any_element()
    }
}


// ---------------------------------------------------------------- emoji, stickers and webhooks

const MANAGE_EXPRESSIONS: u64 = 1 << 30;
const MANAGE_WEBHOOKS: u64 = 1 << 29;

/// Asks for an image file and hands its name and bytes to `then`.
fn pick_image(
    prompt: &'static str,
    cx: &mut App,
    then: impl FnOnce(String, Vec<u8>, &mut App) + 'static,
) {
    let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(prompt.into()) });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = picked.await else { return };
        let Some(path) = paths.into_iter().next() else { return };
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = cx.background_executor().spawn(async move { std::fs::read(path) }).await;
        let _ = cx.update(|cx| match bytes {
            Ok(bytes) => then(name, bytes, cx),
            Err(e) => log::warn!("could not read the image: {e}"),
        });
    })
    .detach();
}

/// Discord emoji names: 2-32 letters, digits or underscores.
fn emoji_name(raw: &str) -> String {
    let mut name: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(32).collect();
    while name.len() < 2 {
        name.push('_');
    }
    name
}

impl ServerSettings {
    fn item_row(id: impl Into<ElementId>) -> Stateful<Div> {
        h_flex().id(id).px_2().py_1p5().gap_3().rounded_md().hover(|s| s.bg(theme::bg_hover()))
    }

    fn render_emoji(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let can = store.can(self.guild, MANAGE_EXPRESSIONS);
        let emojis = store.guild(self.guild).map(|g| g.guild.emojis.clone()).unwrap_or_default();
        v_flex()
            .gap_1()
            .child(div().pb_1().text_size(px(12.)).text_color(theme::text_muted()).child(
                "PNG, JPEG, GIF or WebP, at most 256 KB. The file name becomes the emoji name.",
            ))
            .when(can, |d| {
                d.child(Self::button("emoji-upload", "Upload Emoji", true).mb_2().w(px(140.)).on_click(cx.listener(
                    |this, _, _, cx| {
                        let (store, guild) = (this.store.clone(), this.guild);
                        pick_image("Choose emoji image", cx, move |name, bytes, cx| {
                            store.update(cx, |s, cx| match image_mime(&bytes) {
                                Some(mime) => s.create_emoji(guild, emoji_name(&name), mime.to_string(), bytes, cx),
                                None => cx.emit(StoreEvent::Error("Choose a PNG, JPEG, GIF or WebP image.".into())),
                            });
                        });
                    },
                )))
            })
            .when(emojis.is_empty(), |d| d.child(div().text_color(theme::text_muted()).child("No custom emoji yet.")))
            .children(emojis.into_iter().filter_map(|emoji| {
                let id = emoji.id?;
                let name = emoji.name.clone().unwrap_or_default();
                let url = emoji.image_url(64).unwrap_or_default();
                Some(
                    Self::item_row(("emoji", id.0))
                        .child(img(url).size(px(28.)).object_fit(ObjectFit::Contain))
                        .child(div().flex_1().min_w_0().truncate().text_color(theme::text_bright()).child(format!(":{name}:")))
                        .when(can, |d| {
                            d.child(Self::button(("emoji-rename", id.0), "Rename", false).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    let (store, guild) = (this.store.clone(), this.guild);
                                    let spec = form::FormSpec::new("Rename Emoji", "Save", move |r, _, cx| {
                                        let name = emoji_name(&r.values[0]);
                                        store.update(cx, |s, cx| s.rename_emoji(guild, id, name, cx));
                                    })
                                    .field(form::Field::new("Name", name.clone()));
                                    form::open_form(spec, window, cx);
                                },
                            )))
                            .child(Self::button(("emoji-delete", id.0), "Delete", false).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    let (store, guild) = (this.store.clone(), this.guild);
                                    form::confirm(window, cx, "Delete emoji?", "It disappears from every message that uses it.", "Delete", move |cx| {
                                        store.update(cx, |s, cx| s.delete_emoji(guild, id, cx));
                                    });
                                },
                            )))
                        }),
                )
            }))
            .into_any_element()
    }

    fn render_stickers(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let can = store.can(self.guild, MANAGE_EXPRESSIONS);
        let stickers = store.guild(self.guild).map(|g| g.guild.stickers.clone()).unwrap_or_default();
        v_flex()
            .gap_1()
            .child(div().pb_1().text_size(px(12.)).text_color(theme::text_muted()).child(
                "PNG, APNG or GIF, 320 \u{d7} 320 pixels, at most 512 KB.",
            ))
            .when(can, |d| {
                d.child(Self::button("sticker-upload", "Upload Sticker", true).mb_2().w(px(140.)).on_click(cx.listener(
                    |this, _, window, cx| {
                        let (store, guild) = (this.store.clone(), this.guild);
                        let spec = form::FormSpec::new("Upload Sticker", "Choose File", move |r, _, cx| {
                            let name = r.values[0].clone();
                            let tags = if r.values[1].is_empty() { "\u{2b50}".to_string() } else { r.values[1].clone() };
                            if name.len() < 2 {
                                return;
                            }
                            pick_image("Choose sticker image", cx, move |file, bytes, cx| {
                                store.update(cx, |s, cx| match image_mime(&bytes) {
                                    Some("image/png" | "image/gif") => {
                                        let ext = if image_mime(&bytes) == Some("image/gif") { "gif" } else { "png" };
                                        let upload = Upload { filename: format!("{file}.{ext}"), bytes };
                                        s.create_sticker(guild, name, tags, upload, cx);
                                    }
                                    _ => cx.emit(StoreEvent::Error("Stickers must be PNG, APNG or GIF images.".into())),
                                });
                            });
                        })
                        .description("Name the sticker and pick the emoji it is suggested for, then choose the file.")
                        .field(form::Field::new("Name", "").placeholder("Ferris wave"))
                        .field(form::Field::new("Related emoji", "").placeholder("\u{2b50}"));
                        form::open_form(spec, window, cx);
                    },
                )))
            })
            .when(stickers.is_empty(), |d| d.child(div().text_color(theme::text_muted()).child("No stickers yet.")))
            .children(stickers.into_iter().map(|sticker| {
                let id = sticker.id;
                let name = sticker.name.clone();
                Self::item_row(("sticker", id.0))
                    .child(match sticker.image_url() {
                        Some(url) => img(url).size(px(40.)).object_fit(ObjectFit::Contain).into_any_element(),
                        None => div().size(px(40.)).into_any_element(),
                    })
                    .child(div().flex_1().min_w_0().truncate().text_color(theme::text_bright()).child(name.clone()))
                    .when(can, |d| {
                        d.child(Self::button(("sticker-rename", id.0), "Rename", false).on_click(cx.listener(
                            move |this, _, window, cx| {
                                let (store, guild) = (this.store.clone(), this.guild);
                                let spec = form::FormSpec::new("Rename Sticker", "Save", move |r, _, cx| {
                                    let name = r.values[0].clone();
                                    if name.len() >= 2 {
                                        store.update(cx, |s, cx| s.rename_sticker(guild, id, name, cx));
                                    }
                                })
                                .field(form::Field::new("Name", name.clone()));
                                form::open_form(spec, window, cx);
                            },
                        )))
                        .child(Self::button(("sticker-delete", id.0), "Delete", false).on_click(cx.listener(
                            move |this, _, window, cx| {
                                let (store, guild) = (this.store.clone(), this.guild);
                                form::confirm(window, cx, "Delete sticker?", "This cannot be undone.", "Delete", move |cx| {
                                    store.update(cx, |s, cx| s.delete_sticker(guild, id, cx));
                                });
                            },
                        )))
                    })
            }))
            .into_any_element()
    }

    /// The server's text channels, for choosing where a webhook posts.
    fn text_channels(&self, cx: &App) -> Vec<(Id, String)> {
        let store = self.store.read(cx);
        let mut channels: Vec<_> = store
            .channels
            .values()
            .filter(|c| c.guild_id == Some(self.guild) && (c.kind == 0 || c.kind == 5))
            .map(|c| (c.id, c.name.clone().unwrap_or_default()))
            .collect();
        channels.sort_by(|a, b| a.1.cmp(&b.1));
        channels
    }

    fn set_webhook(&mut self, hook: Webhook, cx: &mut Context<Self>) {
        let list = self.webhooks.get_or_insert_with(Vec::new);
        match list.iter_mut().find(|h| h.id == hook.id) {
            Some(existing) => *existing = hook,
            None => list.push(hook),
        }
        cx.notify();
    }

    /// A form for a new webhook (`hook` is `None`) or for renaming or moving one.
    fn webhook_form(&mut self, hook: Option<Webhook>, window: &mut Window, cx: &mut Context<Self>) {
        let channels = self.text_channels(cx);
        if channels.is_empty() {
            return;
        }
        let selected = hook
            .as_ref()
            .and_then(|h| channels.iter().position(|(id, _)| Some(*id) == h.channel_id))
            .unwrap_or(0);
        let labels: Vec<String> = channels.iter().map(|(_, n)| format!("#{n}")).collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let (store, this) = (self.store.clone(), cx.weak_entity());
        let editing = hook.as_ref().map(|h| h.id);
        let spec = form::FormSpec::new(if editing.is_some() { "Edit Webhook" } else { "Create Webhook" }, "Save", move |r, _, cx| {
            let name = r.values[0].clone();
            let Some((channel, _)) = channels.get(r.choice).cloned() else { return };
            if name.is_empty() {
                return;
            }
            let done = move |hook: Webhook, cx: &mut App| {
                let _ = this.update(cx, |this, cx| this.set_webhook(hook, cx));
            };
            store.update(cx, |s, cx| match editing {
                Some(id) => s.edit_webhook(id, name, channel, cx, done),
                None => s.create_webhook(channel, name, cx, done),
            });
        })
        .field(form::Field::new("Name", hook.and_then(|h| h.name).unwrap_or_else(|| "Webhook".into())))
        .choice("Channel", &label_refs, selected);
        form::open_form(spec, window, cx);
    }

    fn render_webhooks(&self, cx: &mut Context<Self>) -> AnyElement {
        let can = self.store.read(cx).can(self.guild, MANAGE_WEBHOOKS);
        if !can {
            return div().text_color(theme::text_muted()).child("You need Manage Webhooks to see this.").into_any_element();
        }
        let Some(hooks) = self.webhooks.clone() else {
            return div().text_color(theme::text_muted()).child("Loading webhooks\u{2026}").into_any_element();
        };
        let channel_name = |id: Option<Id>| {
            id.and_then(|id| self.store.read(cx).channels.get(&id).and_then(|c| c.name.clone()))
                .map_or_else(|| "unknown channel".into(), |n| format!("#{n}"))
        };
        let rows: Vec<_> = hooks.iter().map(|h| (h.clone(), channel_name(h.channel_id))).collect();
        v_flex()
            .gap_1()
            .child(div().pb_1().text_size(px(12.)).text_color(theme::text_muted()).child(
                "Apps and services post to a channel through a webhook address. Treat the address like a password.",
            ))
            .child(Self::button("webhook-create", "Create Webhook", true).mb_2().w(px(150.)).on_click(cx.listener(
                |this, _, window, cx| this.webhook_form(None, window, cx),
            )))
            .when(rows.is_empty(), |d| d.child(div().text_color(theme::text_muted()).child("No webhooks yet.")))
            .children(rows.into_iter().map(|(hook, channel)| {
                let id = hook.id;
                let url = hook.url();
                let edit = hook.clone();
                Self::item_row(("webhook", id.0))
                    .child(avatar(hook.avatar_url().unwrap_or_default(), 28.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_color(theme::text_bright()).child(hook.name.clone().unwrap_or_default()))
                            .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(format!("Posts to {channel}"))),
                    )
                    .when_some(url, |d, url| {
                        d.child(Self::button(("webhook-copy", id.0), "Copy URL", false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let text = url.clone();
                                this.store.update(cx, |_, cx| {
                                    cx.emit(StoreEvent::Clipboard { text, notice: "Webhook URL copied.".into() })
                                });
                            },
                        )))
                    })
                    .child(Self::button(("webhook-edit", id.0), "Edit", false).on_click(cx.listener(
                        move |this, _, window, cx| this.webhook_form(Some(edit.clone()), window, cx),
                    )))
                    .child(Self::button(("webhook-delete", id.0), "Delete", false).on_click(cx.listener(
                        move |this, _, window, cx| {
                            let (store, this_entity) = (this.store.clone(), cx.weak_entity());
                            form::confirm(window, cx, "Delete webhook?", "Anything posting to it will stop working.", "Delete", move |cx| {
                                store.update(cx, |s, cx| {
                                    s.delete_webhook(id, cx, move |cx| {
                                        let _ = this_entity.update(cx, |this, cx| {
                                            if let Some(list) = this.webhooks.as_mut() {
                                                list.retain(|h| h.id != id);
                                            }
                                            cx.notify();
                                        });
                                    });
                                });
                            });
                        },
                    )))
            }))
            .into_any_element()
    }
}

/// The preset colours, plus the role's own colour if it is not one of them.
fn swatches(current: u32) -> Vec<u32> {
    let mut colors = ROLE_COLORS.to_vec();
    if !colors.contains(&current) {
        colors.push(current);
    }
    colors
}

fn checkbox(on: bool) -> impl IntoElement {
    div()
        .size(px(16.))
        .flex_shrink_0()
        .rounded_sm()
        .border_1()
        .border_color(if on { theme::accent() } else { theme::text_muted() })
        .when(on, |d| d.bg(theme::accent()))
        .flex()
        .items_center()
        .justify_center()
        .when(on, |d| d.child(icon(IconName::Check, 12., theme::on_accent())))
}

impl Render for ServerSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.store.read(cx).guild(self.guild).map(|g| g.guild.name.clone()).unwrap_or_default();
        let tab = self.tab;
        let tabs = [
            ("Overview", Tab::Overview),
            ("Roles", Tab::Roles),
            ("Emoji", Tab::Emoji),
            ("Stickers", Tab::Stickers),
            ("Webhooks", Tab::Webhooks),
            ("Bans", Tab::Bans),
            ("Audit Log", Tab::AuditLog),
        ];
        let body = match tab {
            Tab::Overview => self.render_overview(cx),
            Tab::Roles => self.render_roles(cx),
            Tab::Emoji => self.render_emoji(cx),
            Tab::Stickers => self.render_stickers(cx),
            Tab::Webhooks => self.render_webhooks(cx),
            Tab::Bans => self.render_bans(cx),
            Tab::AuditLog => self.render_audit(cx),
        };
        div()
            .id("server-settings-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.55))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(ServerSettingsEvent::Close)))
            .child(
                v_flex()
                    .id("server-settings")
                    .w(px(860.))
                    .max_h(px(640.))
                    .p_5()
                    .gap_3()
                    .rounded_xl()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_bright())
                                    .child(name),
                            )
                            .children(tabs.into_iter().enumerate().map(|(ix, (label, value))| {
                                div()
                                    .id(("server-tab", ix))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .when(tab == value, |d| d.bg(theme::bg_active()).text_color(theme::text_bright()))
                                    .hover(|s| s.bg(theme::bg_hover()))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| this.open_tab(value, cx)))
                            }))
                            .child(
                                icon_button("server-settings-close", IconName::X, "Close")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ServerSettingsEvent::Close))),
                            ),
                    )
                    .child(div().id("server-settings-body").flex_1().min_h_0().overflow_y_scroll().child(body)),
            )
    }
}
