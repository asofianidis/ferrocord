//! Top-level window content.

mod chat;
mod login;
mod members;
mod rail;
mod rich;
mod sidebar;
mod switcher;
pub mod theme;
mod widgets;

use crate::{
    credentials,
    store::{Connection, Store, StoreEvent},
};
use gpui::{prelude::*, *};
use gpui_component::{WindowExt as _, notification::Notification};
use widgets::*;

actions!(ferrocord, [ToggleSwitcher]);

pub struct AppView {
    store: Entity<Store>,
    login: Entity<login::LoginView>,
    rail: Entity<rail::Rail>,
    sidebar: Entity<sidebar::Sidebar>,
    chat: Entity<chat::ChatView>,
    members: Entity<members::Members>,
    switcher: Option<Entity<switcher::Switcher>>,
    _switcher_sub: Option<Subscription>,
    _subs: Vec<Subscription>,
}

impl AppView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let login = cx.new(|cx| login::LoginView::new(store.clone(), window, cx));
        let rail = cx.new(|cx| rail::Rail::new(store.clone(), cx));
        let sidebar = cx.new(|cx| sidebar::Sidebar::new(store.clone(), cx));
        let chat = cx.new(|cx| chat::ChatView::new(store.clone(), window, cx));
        let members = cx.new(|cx| members::Members::new(store.clone(), cx));

        let subs = vec![cx.subscribe_in(&store, window, |this, store, event: &StoreEvent, window, cx| match event {
            StoreEvent::Connection => {
                if matches!(store.read(cx).connection, Connection::Failed(_)) {
                    // Do not keep retrying a token the gateway rejected.
                    credentials::clear();
                }
                cx.notify();
            }
            StoreEvent::Selection | StoreEvent::Channels => {
                this.update_title(window, cx);
                cx.notify();
            }
            StoreEvent::Error(message) => {
                window.push_notification(Notification::error(message.clone()), cx);
            }
            StoreEvent::Mention { title, body } => {
                window.push_notification(Notification::info(body.clone()).title(title.clone()), cx);
            }
            _ => {}
        })];

        Self { store, login, rail, sidebar, chat, members, switcher: None, _switcher_sub: None, _subs: subs }
    }

    pub fn toggle_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_some() {
            self.close_switcher(window, cx);
            return;
        }
        if self.store.read(cx).me.is_none() {
            return;
        }
        let switcher = cx.new(|cx| switcher::Switcher::new(self.store.clone(), window, cx));
        self._switcher_sub = Some(cx.subscribe_in(
            &switcher,
            window,
            |this, _, _: &switcher::SwitcherEvent, window, cx| this.close_switcher(window, cx),
        ));
        self.switcher = Some(switcher);
        cx.notify();
    }

    fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        self._switcher_sub = None;
        self.chat.update(cx, |chat, cx| chat.focus_composer(window, cx));
        cx.notify();
    }

    fn update_title(&self, window: &mut Window, cx: &App) {
        let store = self.store.read(cx);
        let title = match store.selected_channel.and_then(|c| store.channels.get(&c)) {
            Some(channel) => {
                let name = store.channel_title(channel);
                match channel.guild_id.and_then(|g| store.guild(g)) {
                    Some(guild) => format!("#{name} \u{2014} {} \u{2014} Ferrocord", guild.guild.name),
                    None => format!("{name} \u{2014} Ferrocord"),
                }
            }
            None => "Ferrocord".to_string(),
        };
        window.set_window_title(&title);
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let root = div()
            .relative()
            .size_full()
            .bg(theme::bg_chat())
            .text_color(theme::text())
            .text_size(px(14.))
            .line_height(px(20.));

        match (&store.connection, store.me.is_some()) {
            (Connection::LoggedOut | Connection::Failed(_), _) => root.child(self.login.clone()),
            (Connection::Connecting, false) => root.child(
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .bg(theme::bg_rail())
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_bright())
                            .child("Ferrocord"),
                    )
                    .child(div().text_color(theme::text_muted()).child("Connecting…")),
            ),
            (connection, _) => {
                let reconnecting = *connection != Connection::Ready;
                let show_members = store.show_members
                    && store.selected_guild.is_some()
                    && store.selected_channel.is_some();
                root.flex()
                    .flex_col()
                    .when(reconnecting, |d| {
                        d.child(
                            div()
                                .w_full()
                                .py_1()
                                .flex()
                                .flex_shrink_0()
                                .justify_center()
                                .bg(theme::idle())
                                .text_color(theme::bg_rail())
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Connection lost. Reconnecting…"),
                        )
                    })
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .items_stretch()
                            .child(self.rail.clone())
                            .child(self.sidebar.clone())
                            .child(self.chat.clone())
                            .when(show_members, |d| d.child(self.members.clone())),
                    )
                    .children(self.switcher.clone())
            }
        }
    }
}
