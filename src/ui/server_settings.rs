//! Server settings: overview, roles and bans.

use super::{form, theme, widgets::*};
use crate::{
    discord::{Ban, Id, permissions as p},
    store::{Store, StoreEvent, image_mime},
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
    Bans,
}

const ROLE_COLORS: [u32; 12] = [
    0, 0x1abc9c, 0x2ecc71, 0x3498db, 0x9b59b6, 0xe91e63, 0xf1c40f, 0xe67e22, 0xe74c3c, 0x95a5a6, 0x607d8b, 0xd4387c,
];

const PERMISSIONS: [(&str, u64); 32] = [
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
        let mut roles = guild.guild.roles.clone();
        roles.sort_by_key(|r| std::cmp::Reverse(r.position));
        let everyone = self.guild;
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
            .children(roles.iter().map(|role| {
                let id = role.id;
                let selected = self.role == Some(id);
                let label = if id == everyone { "@everyone".to_string() } else { role.name.clone() };
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
                    .child(div().min_w_0().truncate().child(label))
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
        let tabs = [("Overview", Tab::Overview), ("Roles", Tab::Roles), ("Bans", Tab::Bans)];
        let body = match tab {
            Tab::Overview => self.render_overview(cx),
            Tab::Roles => self.render_roles(cx),
            Tab::Bans => self.render_bans(cx),
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
                    .w(px(760.))
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
                                    .px_3()
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
