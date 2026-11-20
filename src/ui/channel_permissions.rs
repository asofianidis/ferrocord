//! Per-channel permission overrides: for each role or member, allow, deny or
//! inherit each permission.

use super::{server_settings::PERMISSIONS, theme, widgets::*};
use crate::{
    discord::{Id, Overwrite, permissions as p},
    store::{Store, StoreEvent},
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_kit::assets::IconName;

pub enum ChannelPermissionsEvent {
    Close,
}

/// Permissions that only make sense server-wide, so channels cannot override them.
const SERVER_ONLY: [u64; 8] = [
    p::ADMINISTRATOR,
    p::MANAGE_GUILD,
    1 << 7,  // View Audit Log
    1 << 26, // Change Nickname
    p::MANAGE_NICKNAMES,
    p::KICK_MEMBERS,
    p::BAN_MEMBERS,
    p::MODERATE_MEMBERS,
];

pub struct ChannelPermissions {
    store: Entity<Store>,
    channel: Id,
    /// The role or member being edited, with its unsaved allow and deny bits.
    editing: Option<Overwrite>,
    /// Search box for adding a member override.
    member_search: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ChannelPermissionsEvent> for ChannelPermissions {}

impl ChannelPermissions {
    pub fn new(store: Entity<Store>, channel: Id, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let member_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search members"));
        let subs = vec![
            cx.subscribe(&store, move |this, store, event: &StoreEvent, cx| {
                if matches!(event, StoreEvent::Channels) && !store.read(cx).channels.contains_key(&this.channel) {
                    cx.emit(ChannelPermissionsEvent::Close);
                }
                cx.notify();
            }),
            cx.subscribe(&member_search, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let mut this = Self { store, channel, editing: None, member_search, _subs: subs };
        // Start on @everyone, which every channel can override.
        if let Some(guild) = this.guild(cx) {
            this.select(guild, 0, cx);
        }
        this
    }

    fn guild(&self, cx: &App) -> Option<Id> {
        self.store.read(cx).channels.get(&self.channel).and_then(|c| c.guild_id)
    }

    fn select(&mut self, target: Id, kind: u8, cx: &mut Context<Self>) {
        let existing = self
            .store
            .read(cx)
            .channels
            .get(&self.channel)
            .and_then(|c| c.permission_overwrites.iter().find(|o| o.id == target).cloned());
        self.editing = Some(existing.unwrap_or(Overwrite { id: target, kind, allow: 0, deny: 0 }));
        cx.notify();
    }

    /// Cycles one permission: inherit, then allow, then deny.
    fn set(&mut self, bit: u64, state: Tri, cx: &mut Context<Self>) {
        let Some(o) = self.editing.as_mut() else { return };
        o.allow &= !bit;
        o.deny &= !bit;
        match state {
            Tri::Allow => o.allow |= bit,
            Tri::Deny => o.deny |= bit,
            Tri::Inherit => {}
        }
        cx.notify();
    }

    fn target_name(&self, target: Id, kind: u8, cx: &App) -> String {
        let store = self.store.read(cx);
        let guild = self.guild(cx);
        if kind == 0 {
            if Some(target) == guild {
                return "@everyone".into();
            }
            return guild
                .and_then(|g| store.guild(g))
                .and_then(|g| g.guild.roles.iter().find(|r| r.id == target))
                .map_or_else(|| "Deleted role".into(), |r| r.name.clone());
        }
        store.users.get(&target).map_or_else(|| format!("Member {target}"), |u| u.display_name().to_string())
    }

    fn row(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
        h_flex()
            .id(id)
            .px_2()
            .py_1()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::bg_active()))
            .hover(|s| s.bg(theme::bg_hover()))
    }

    fn render_targets(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let Some(guild) = self.guild(cx) else { return div().into_any_element() };
        let overwrites = store.channels.get(&self.channel).map(|c| c.permission_overwrites.clone()).unwrap_or_default();
        let selected = self.editing.as_ref().map(|o| o.id);
        let has = |id: Id| overwrites.iter().any(|o| o.id == id);

        // @everyone first, then roles and members that already have overrides.
        let mut targets: Vec<(Id, u8)> = vec![(guild, 0)];
        targets.extend(store.ordered_roles(guild).iter().filter(|r| has(r.id)).map(|r| (r.id, 0)));
        targets.extend(overwrites.iter().filter(|o| o.kind == 1).map(|o| (o.id, 1)));
        // Roles that could get an override.
        let addable: Vec<(Id, String)> =
            store.ordered_roles(guild).into_iter().filter(|r| !has(r.id)).map(|r| (r.id, r.name)).collect();
        let query = self.member_search.read(cx).value().trim().to_string();
        let members: Vec<(Id, String)> = if query.is_empty() {
            Vec::new()
        } else {
            store
                .mention_candidates(self.channel, &query, 8)
                .into_iter()
                .filter(|(u, _)| !has(u.id))
                .map(|(u, name)| (u.id, name))
                .take(5)
                .collect()
        };

        v_flex()
            .id("override-targets")
            .w(px(200.))
            .flex_shrink_0()
            .gap_1()
            .overflow_y_scroll()
            .child(div().text_size(px(12.)).text_color(theme::text_muted()).child("ROLES AND MEMBERS"))
            .children(targets.into_iter().map(|(id, kind)| {
                let name = self.target_name(id, kind, cx);
                let marker = if kind == 0 { IconName::AtSign } else { IconName::Users };
                Self::row(("override", id.0), selected == Some(id))
                    .child(icon(marker, 14., theme::text_muted()))
                    .child(div().min_w_0().truncate().child(name))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(id, kind, cx)))
            }))
            .when(!addable.is_empty(), |d| {
                d.child(div().pt_3().text_size(px(12.)).text_color(theme::text_muted()).child("ADD A ROLE"))
                    .children(addable.into_iter().map(|(id, name)| {
                        Self::row(("add-override", id.0), selected == Some(id))
                            .text_color(theme::text_muted())
                            .child(icon(IconName::Plus, 14., theme::text_muted()))
                            .child(div().min_w_0().truncate().child(name))
                            .on_click(cx.listener(move |this, _, _, cx| this.select(id, 0, cx)))
                    }))
            })
            .child(div().pt_3().pb_1().text_size(px(12.)).text_color(theme::text_muted()).child("ADD A MEMBER"))
            .child(Input::new(&self.member_search))
            .children(members.into_iter().map(|(id, name)| {
                Self::row(("add-member-override", id.0), selected == Some(id))
                    .text_color(theme::text_muted())
                    .child(icon(IconName::Plus, 14., theme::text_muted()))
                    .child(div().min_w_0().truncate().child(name))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(id, 1, cx)))
            }))
            .into_any_element()
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editing) = self.editing.clone() else {
            return div().text_color(theme::text_muted()).child("Choose a role or member.").into_any_element();
        };
        let exists = self
            .store
            .read(cx)
            .channels
            .get(&self.channel)
            .is_some_and(|c| c.permission_overwrites.iter().any(|o| o.id == editing.id));
        let is_everyone = Some(editing.id) == self.guild(cx);
        let name = self.target_name(editing.id, editing.kind, cx);

        let rows = PERMISSIONS.iter().filter(|(_, bit)| !SERVER_ONLY.contains(bit)).enumerate().map(|(ix, (label, bit))| {
            let bit = *bit;
            let current = if editing.allow & bit != 0 {
                Tri::Allow
            } else if editing.deny & bit != 0 {
                Tri::Deny
            } else {
                Tri::Inherit
            };
            // "Manage Roles" on a channel means managing its permissions.
            let label = if bit == p::MANAGE_ROLES { "Manage Permissions" } else { *label };
            h_flex()
                .py_0p5()
                .gap_2()
                .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).child(label))
                .child(h_flex().rounded_md().border_1().border_color(theme::border()).overflow_hidden().children(
                    [Tri::Deny, Tri::Inherit, Tri::Allow].into_iter().map(|state| {
                        let on = state == current;
                        let (glyph, color) = match state {
                            Tri::Deny => (Some(IconName::X), theme::danger()),
                            Tri::Inherit => (None, theme::text_muted()),
                            Tri::Allow => (Some(IconName::Check), theme::online()),
                        };
                        div()
                            .id(SharedString::from(format!("perm-{ix}-{state:?}")))
                            .w(px(30.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .when(on, |d| d.bg(color.opacity(0.25)))
                            .hover(|s| s.bg(theme::bg_hover()))
                            .map(|d| match glyph {
                                Some(g) => d.child(icon(g, 14., if on { color } else { theme::text_muted() })),
                                None => d.text_color(if on { theme::text_bright() } else { theme::text_muted() }).child("/"),
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.set(bit, state, cx)))
                    }),
                ))
        });

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_1()
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text_bright()).child(name))
            .child(
                div()
                    .pb_1()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child("\u{2715} denies, / follows the server role settings, \u{2713} allows."),
            )
            .child(div().id("override-rows").flex_1().min_h_0().pr_2().overflow_y_scroll().children(rows))
            .child(
                h_flex()
                    .pt_3()
                    .gap_2()
                    .child(button("override-save", "Save Changes", true).on_click(cx.listener(move |this, _, _, cx| {
                        let Some(overwrite) = this.editing.clone() else { return };
                        let channel = this.channel;
                        this.store.update(cx, |s, cx| s.save_overwrite(channel, overwrite, cx));
                    })))
                    .when(exists && !is_everyone, |d| {
                        d.child(button("override-remove", "Remove Override", false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let (channel, target) = (this.channel, editing.id);
                                this.store.update(cx, |s, cx| s.delete_overwrite(channel, target, cx));
                                if let Some(guild) = this.guild(cx) {
                                    this.select(guild, 0, cx);
                                }
                            },
                        )))
                    }),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tri {
    Deny,
    Inherit,
    Allow,
}

fn button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
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

impl Render for ChannelPermissions {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self
            .store
            .read(cx)
            .channels
            .get(&self.channel)
            .and_then(|c| c.name.clone())
            .map_or_else(|| "Channel permissions".to_string(), |n| format!("Permissions \u{2014} {n}"));
        div()
            .id("channel-perms-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.55))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(ChannelPermissionsEvent::Close)))
            .child(
                v_flex()
                    .id("channel-perms")
                    .w(px(680.))
                    .h(px(600.))
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
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_bright())
                                    .child(title),
                            )
                            .child(
                                icon_button("channel-perms-close", IconName::X, "Close")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChannelPermissionsEvent::Close))),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .items_stretch()
                            .gap_4()
                            .child(self.render_targets(cx))
                            .child(self.render_editor(cx)),
                    ),
            )
    }
}
