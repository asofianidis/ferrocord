//! Top-level window content.

mod chat;
mod login;
mod members;
mod rail;
mod rich;
mod settings_view;
mod sidebar;
mod switcher;
pub mod theme;
mod widgets;

use crate::{
    credentials,
    store::{Connection, Store, StoreEvent},
};
use gpui::{prelude::*, *};
use gpui_component::{WindowExt as _, input, notification::Notification};
use widgets::*;

actions!(ferrocord, [ToggleSwitcher, ToggleSettings]);

pub struct AppView {
    store: Entity<Store>,
    login: Entity<login::LoginView>,
    rail: Entity<rail::Rail>,
    sidebar: Entity<sidebar::Sidebar>,
    chat: Entity<chat::ChatView>,
    members: Entity<members::Members>,
    switcher: Option<Entity<switcher::Switcher>>,
    _switcher_sub: Option<Subscription>,
    settings: Option<Entity<settings_view::SettingsView>>,
    _settings_sub: Option<Subscription>,
    lightbox: Option<String>,
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

        let mut subs = subs;
        subs.push(cx.subscribe(&chat, |this, _, event: &chat::ChatEvent, cx| match event {
            chat::ChatEvent::OpenImage(url) => {
                this.lightbox = Some(url.clone());
                cx.notify();
            }
        }));

        Self {
            store,
            login,
            rail,
            sidebar,
            chat,
            members,
            switcher: None,
            _switcher_sub: None,
            settings: None,
            _settings_sub: None,
            lightbox: None,
            _subs: subs,
        }
    }

    pub fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self._settings_sub = None;
            cx.notify();
            return;
        }
        let view = cx.new(|cx| settings_view::SettingsView::new(self.store.clone(), cx));
        self._settings_sub = Some(cx.subscribe_in(
            &view,
            window,
            |this, _, _: &settings_view::SettingsEvent, _, cx| {
                this.settings = None;
                this._settings_sub = None;
                cx.notify();
            },
        ));
        self.settings = Some(view);
        cx.notify();
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
        let lightbox = self.lightbox.clone().map(|url| {
            let open = url.clone();
            div()
                .id("lightbox")
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .p_12()
                .bg(gpui::black().opacity(0.88))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.lightbox = None;
                    cx.notify();
                }))
                .child(img(url).size_full().object_fit(ObjectFit::Contain))
                .child(
                    div()
                        .id("lightbox-open")
                        .absolute()
                        .top_4()
                        .right_4()
                        .px_3()
                        .py_1p5()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(theme::bg_sidebar())
                        .text_color(theme::text())
                        .child("Open in browser")
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            cx.open_url(&open);
                        }),
                )
        });
        let store = self.store.read(cx);
        let root = div()
            .relative()
            .size_full()
            // Full-window overlays close first on Escape.
            .capture_action(cx.listener(|this, _: &input::Escape, _, cx| {
                if this.lightbox.take().is_some() || this.settings.take().is_some() {
                    this._settings_sub = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .bg(theme::bg_chat())
            .text_color(theme::text())
            .text_size(px(14.))
            .line_height(px(20.));

        let body = match (&store.connection, store.me.is_some()) {
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
                                .bg(theme::warning())
                                .text_color(gpui::black())
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
            }
        };
        body.children(self.switcher.clone()).children(self.settings.clone()).children(lightbox)
    }
}
