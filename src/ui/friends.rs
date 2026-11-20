//! The friends page: friends, pending requests and blocked users.

use super::{theme, widgets::*};
use crate::{
    discord::{Id, Status},
    store::{Store, StoreEvent},
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_kit::assets::IconName;
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Friends,
    Pending,
    Blocked,
}

pub struct FriendsView {
    store: Entity<Store>,
    tab: Tab,
    add: Entity<InputState>,
    /// Friends ticked for a new group conversation.
    picked: HashSet<Id>,
    _subs: Vec<Subscription>,
}

impl FriendsView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let add = cx.new(|cx| InputState::new(window, cx).placeholder("Add a friend by username"));
        let subs = vec![
            cx.subscribe(&store, |_, _, event: &StoreEvent, cx| {
                if matches!(event, StoreEvent::Friends | StoreEvent::Selection | StoreEvent::Connection) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&add, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.send_request(window, cx);
                }
            }),
        ];
        Self { store, tab: Tab::Friends, add, picked: HashSet::new(), _subs: subs }
    }

    fn send_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.add.read(cx).value().trim().trim_start_matches('@').to_string();
        if username.is_empty() {
            return;
        }
        self.add.update(cx, |s, cx| s.set_value("", window, cx));
        self.store.update(cx, |store, cx| store.add_friend(username, cx));
    }

    fn action(id: impl Into<ElementId>, label: &'static str, primary: bool) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(13.))
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
}

impl Render for FriendsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let tab = self.tab;
        let wanted: &[u8] = match tab {
            Tab::Friends => &[1],
            Tab::Pending => &[3, 4],
            Tab::Blocked => &[2],
        };
        let mut people: Vec<_> = store
            .relationships
            .iter()
            .filter(|(_, kind)| wanted.contains(kind))
            .filter_map(|(id, kind)| Some((store.users.get(id)?.clone(), *kind, store.status_of(*id))))
            .collect();
        people.sort_by_key(|(user, _, status)| (*status == Status::Offline, user.display_name().to_lowercase()));
        let pending = store.relationships.iter().filter(|(_, kind)| *kind == 3).count();
        self.picked.retain(|id| store.relationship(*id) == Some(1));
        let picked = self.picked.len();

        let tab_button = |id: &'static str, label: String, value: Tab, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .when(tab == value, |d| d.bg(theme::bg_active()).text_color(theme::text_bright()))
                .hover(|s| s.bg(theme::bg_hover()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = value;
                    cx.notify();
                }))
        };

        let empty = match tab {
            Tab::Friends => "No friends to show yet. Add one by username above.",
            Tab::Pending => "No pending friend requests.",
            Tab::Blocked => "You have not blocked anyone.",
        };

        let rows = people.into_iter().enumerate().map(|(ix, (user, kind, status))| {
            let id = user.id;
            let is_picked = self.picked.contains(&id);
            let subtitle = match kind {
                3 => "Incoming friend request".to_string(),
                4 => "Outgoing friend request".to_string(),
                2 => "Blocked".to_string(),
                _ => status.label().replace("Invisible", "Offline"),
            };
            let mut actions = h_flex().gap_2();
            match kind {
                1 => {
                    actions = actions
                        .child(Self::action(("message", ix), "Message", true).on_click(cx.listener(
                            move |this, _, _, cx| this.store.update(cx, |store, cx| store.open_dm(id, cx)),
                        )))
                        .child(Self::action(("remove", ix), "Remove", false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.store.update(cx, |store, cx| store.remove_relationship(id, cx))
                            },
                        )))
                        .child(Self::action(("block", ix), "Block", false).on_click(cx.listener(
                            move |this, _, _, cx| this.store.update(cx, |store, cx| store.block_user(id, cx)),
                        )));
                }
                3 => {
                    actions = actions
                        .child(Self::action(("accept", ix), "Accept", true).on_click(cx.listener(
                            move |this, _, _, cx| this.store.update(cx, |store, cx| store.accept_friend(id, cx)),
                        )))
                        .child(Self::action(("ignore", ix), "Ignore", false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.store.update(cx, |store, cx| store.remove_relationship(id, cx))
                            },
                        )));
                }
                _ => {
                    let label = if kind == 2 { "Unblock" } else { "Cancel" };
                    actions = actions.child(Self::action(("undo", ix), label, false).on_click(cx.listener(
                        move |this, _, _, cx| this.store.update(cx, |store, cx| store.remove_relationship(id, cx)),
                    )));
                }
            }
            h_flex()
                .id(("friend", ix))
                .h(px(58.))
                .px_3()
                .gap_3()
                .rounded_lg()
                .hover(|s| s.bg(theme::bg_hover()))
                .when(kind == 1, |d| {
                    d.child(
                        div()
                            .id(("pick", ix))
                            .size(px(18.))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .border_1()
                            .cursor_pointer()
                            .border_color(if is_picked { theme::accent() } else { theme::text_muted() })
                            .when(is_picked, |d| {
                                d.bg(theme::accent()).child(icon(IconName::Check, 13., theme::on_accent()))
                            })
                            .tooltip(tip("Select for a group conversation"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.picked.remove(&id) {
                                    this.picked.insert(id);
                                }
                                cx.notify();
                            })),
                    )
                })
                .child(avatar_with_status(user.avatar_url(64), 36., status, theme::bg_chat()))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::text_bright())
                                .child(user.display_name().to_string()),
                        )
                        .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(subtitle)),
                )
                .child(actions)
        });
        let rows: Vec<_> = rows.collect();

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme::bg_chat())
            .child(
                h_flex()
                    .h(px(48.))
                    .px_4()
                    .gap_2()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(icon(IconName::Users, 18., theme::text_muted()))
                    .child(
                        div()
                            .pr_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_bright())
                            .child("Friends"),
                    )
                    .child(tab_button("tab-friends", "All".into(), Tab::Friends, cx))
                    .child(tab_button(
                        "tab-pending",
                        if pending > 0 { format!("Pending ({pending})") } else { "Pending".into() },
                        Tab::Pending,
                        cx,
                    ))
                    .child(tab_button("tab-blocked", "Blocked".into(), Tab::Blocked, cx))
                    .child(div().flex_1())
                    .when(picked >= 2, |d| {
                        d.child(Self::action("new-group", "New group conversation", true).on_click(cx.listener(
                            |this, _, _, cx| {
                                let users: Vec<Id> = this.picked.drain().collect();
                                this.store.update(cx, |store, cx| store.create_group_dm(users, cx));
                            },
                        )))
                    }),
            )
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .gap_2()
                    .flex_shrink_0()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.add)))
                    .child(
                        Self::action("send-request", "Send request", true)
                            .on_click(cx.listener(|this, _, window, cx| this.send_request(window, cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("friends-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .when(rows.is_empty(), |d| d.child(div().p_3().text_color(theme::text_muted()).child(empty)))
                    .children(rows),
            )
    }
}
