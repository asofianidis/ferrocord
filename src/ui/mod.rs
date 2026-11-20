//! Top-level window content.

mod call;
mod channel_permissions;
mod chat;
mod events;
pub mod form;
mod friends;
mod login;
mod members;
mod rail;
mod rich;
mod server_settings;
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
    friends: Entity<friends::FriendsView>,
    switcher: Option<Entity<switcher::Switcher>>,
    _switcher_sub: Option<Subscription>,
    settings: Option<Entity<settings_view::SettingsView>>,
    _settings_sub: Option<Subscription>,
    lightbox: Option<String>,
    form: Option<Entity<form::FormDialog>>,
    _form_sub: Option<Subscription>,
    server_settings: Option<Entity<server_settings::ServerSettings>>,
    _server_settings_sub: Option<Subscription>,
    channel_permissions: Option<Entity<channel_permissions::ChannelPermissions>>,
    _channel_permissions_sub: Option<Subscription>,
    events: Option<Entity<events::EventsView>>,
    _events_sub: Option<Subscription>,
    _subs: Vec<Subscription>,
}

impl AppView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let login = cx.new(|cx| login::LoginView::new(store.clone(), window, cx));
        let rail = cx.new(|cx| rail::Rail::new(store.clone(), cx));
        let sidebar = cx.new(|cx| sidebar::Sidebar::new(store.clone(), cx));
        let chat = cx.new(|cx| chat::ChatView::new(store.clone(), window, cx));
        let members = cx.new(|cx| members::Members::new(store.clone(), cx));
        let friends = cx.new(|cx| friends::FriendsView::new(store.clone(), window, cx));

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
            StoreEvent::IncomingCall { channel, title } => {
                let channel = *channel;
                let store = this.store.clone();
                let note = Notification::info(format!("{title} is calling you. Click to join."))
                    .title("Incoming call")
                    .autohide(false)
                    .on_click(move |_, _, cx| {
                        store.update(cx, |store, cx| {
                            store.select_channel(channel, cx);
                            store.join_voice(channel, cx);
                        });
                    });
                window.push_notification(note, cx);
            }
            StoreEvent::Notice(message) => {
                window.push_notification(Notification::success(message.clone()), cx);
            }
            StoreEvent::Clipboard { text, notice } => {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                window.push_notification(Notification::success(notice.clone()), cx);
            }
            StoreEvent::Mention { title, body } => {
                window.push_notification(Notification::info(body.clone()).title(title.clone()), cx);
            }
            _ => {}
        })];

        let mut subs = subs;
        subs.push(cx.subscribe_in(&chat, window, |this, _, event: &chat::ChatEvent, window, cx| match event {
            chat::ChatEvent::OpenImage(url) => {
                this.lightbox = Some(url.clone());
                cx.notify();
            }
            chat::ChatEvent::Forward { channel, message } => {
                this.switcher = None;
                this.open_switcher(Some((*channel, *message)), window, cx);
            }
        }));

        Self {
            store,
            login,
            rail,
            sidebar,
            chat,
            members,
            friends,
            switcher: None,
            _switcher_sub: None,
            settings: None,
            _settings_sub: None,
            lightbox: None,
            form: None,
            _form_sub: None,
            server_settings: None,
            _server_settings_sub: None,
            channel_permissions: None,
            _channel_permissions_sub: None,
            events: None,
            _events_sub: None,
            _subs: subs,
        }
    }

    pub fn show_form(&mut self, spec: form::FormSpec, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| form::FormDialog::new(spec, window, cx));
        self._form_sub = Some(cx.subscribe(&dialog, |this, _, _: &form::FormEvent, cx| {
            this.form = None;
            this._form_sub = None;
            cx.notify();
        }));
        self.form = Some(dialog);
        cx.notify();
    }

    pub fn show_server_settings(&mut self, guild: crate::discord::Id, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| server_settings::ServerSettings::new(self.store.clone(), guild, window, cx));
        self._server_settings_sub = Some(cx.subscribe(&view, |this, _, _: &server_settings::ServerSettingsEvent, cx| {
            this.server_settings = None;
            this._server_settings_sub = None;
            cx.notify();
        }));
        self.server_settings = Some(view);
        cx.notify();
    }

    pub fn show_channel_permissions(&mut self, channel: crate::discord::Id, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| channel_permissions::ChannelPermissions::new(self.store.clone(), channel, window, cx));
        self._channel_permissions_sub =
            Some(cx.subscribe(&view, |this, _, _: &channel_permissions::ChannelPermissionsEvent, cx| {
                this.channel_permissions = None;
                this._channel_permissions_sub = None;
                cx.notify();
            }));
        self.channel_permissions = Some(view);
        cx.notify();
    }

    pub fn show_events(&mut self, guild: crate::discord::Id, cx: &mut Context<Self>) {
        let view = cx.new(|cx| events::EventsView::new(self.store.clone(), guild, cx));
        self._events_sub = Some(cx.subscribe(&view, |this, _, _: &events::EventsEvent, cx| {
            this.events = None;
            this._events_sub = None;
            cx.notify();
        }));
        self.events = Some(view);
        cx.notify();
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
        self.open_switcher(None, window, cx);
    }

    fn open_switcher(
        &mut self,
        forward: Option<(crate::discord::Id, crate::discord::Id)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.store.read(cx).me.is_none() {
            return;
        }
        let switcher = cx.new(|cx| switcher::Switcher::new(self.store.clone(), forward, window, cx));
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
                if this.lightbox.take().is_some()
                    || this.form.take().is_some()
                    || this.channel_permissions.take().is_some()
                    || this.events.take().is_some()
                    || this.server_settings.take().is_some()
                    || this.settings.take().is_some()
                {
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
                            .map(|d| {
                                if store.show_friends && store.selected_guild.is_none() {
                                    d.child(self.friends.clone())
                                } else {
                                    d.child(self.chat.clone())
                                }
                            })
                            .when(show_members, |d| d.child(self.members.clone())),
                    )
            }
        };
        body.children(self.switcher.clone())
            .children(self.settings.clone())
            .children(self.server_settings.clone())
            .children(self.channel_permissions.clone())
            .children(self.events.clone())
            .children(self.form.clone())
            .children(lightbox)
    }
}
