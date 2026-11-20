use super::{theme, widgets::*};
use crate::{
    credentials,
    store::{Connection, Store},
};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::assets::IconName;

pub struct LoginView {
    store: Entity<Store>,
    token: Entity<InputState>,
    is_bot: bool,
    _subs: Vec<Subscription>,
}

impl LoginView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let token = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Token"));
        let subs = vec![
            cx.subscribe_in(&token, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit(window, cx);
                }
            }),
            cx.observe(&store, |_, _, cx| cx.notify()),
        ];
        Self { store, token, is_bot: false, _subs: subs }
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let token = self.token.read(cx).value().trim().trim_matches('"').to_string();
        if token.is_empty() {
            return;
        }
        let is_bot = self.is_bot || token.starts_with("Bot ");
        let token = token.trim_start_matches("Bot ").to_string();
        credentials::save(&token, is_bot);
        self.store.update(cx, |store, cx| store.login(token, is_bot, cx));
    }
}

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = match &self.store.read(cx).connection {
            Connection::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let is_bot = self.is_bot;

        div().size_full().flex().items_center().justify_center().bg(theme::bg_rail()).child(
            v_flex()
                .w(px(420.))
                .p_8()
                .gap_4()
                .rounded_xl()
                .bg(theme::bg_sidebar())
                .border_1()
                .border_color(theme::border())
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(24.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::text_bright())
                                .child("Ferrocord"),
                        )
                        .child(
                            div()
                                .text_color(theme::text_muted())
                                .child("A fast, native Discord client. Sign in with a token."),
                        ),
                )
                .child(Input::new(&self.token).mask_toggle())
                .child(
                    h_flex()
                        .id("bot-toggle")
                        .gap_2()
                        .cursor_pointer()
                        .child(
                            div()
                                .size(px(18.))
                                .rounded_sm()
                                .border_1()
                                .border_color(if is_bot { theme::accent() } else { theme::text_muted() })
                                .when(is_bot, |d| d.bg(theme::accent()))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(is_bot, |d| d.child(icon(IconName::Check, 13., theme::bg_rail()))),
                        )
                        .child("This is a bot token")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.is_bot = !this.is_bot;
                            cx.notify();
                        })),
                )
                .when_some(error, |d, error| {
                    d.child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .text_color(theme::danger())
                            .child(icon(IconName::CircleAlert, 16., theme::danger()))
                            .child(div().flex_1().min_w_0().child(error)),
                    )
                })
                .child(
                    Button::new("login")
                        .primary()
                        .label("Log in")
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                )
                .child(
                    div().text_size(px(12.)).text_color(theme::text_muted()).child(
                        "The token is stored in your operating system's credential store. \
                         Using a personal account token with a third-party client is against \
                         Discord's terms of service; a bot token is the sanctioned option.",
                    ),
                ),
        )
    }
}
