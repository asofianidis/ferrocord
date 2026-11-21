use super::{theme, widgets::*};
use crate::{
    credentials,
    store::{AuthStep, Connection, QrStep, Store},
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_kit::assets::IconName;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Qr,
    Password,
    Token,
}

/// Side length of the drawn QR code, in pixels (before rounding to whole modules).
const QR_SIZE: f32 = 196.0;

/// Draws `url` as a QR code: dark modules on white, with a quiet zone.
fn qr_code(url: &str) -> AnyElement {
    let Ok(code) = qrcode::QrCode::new(url) else {
        return div().text_color(theme::danger()).child("Could not draw the code.").into_any_element();
    };
    let width = code.width();
    let dark: Vec<bool> = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    // Whole-pixel modules keep the edges crisp, which helps phone cameras.
    let module = (QR_SIZE / width as f32).floor().max(1.0);
    let side = module * width as f32;
    div()
        .p_3()
        .rounded_lg()
        .bg(gpui::white())
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    for (i, _) in dark.iter().enumerate().filter(|(_, d)| **d) {
                        let (x, y) = ((i % width) as f32, (i / width) as f32);
                        let origin = point(bounds.origin.x + px(module * x), bounds.origin.y + px(module * y));
                        window.paint_quad(fill(Bounds::new(origin, size(px(module), px(module))), gpui::black()));
                    }
                },
            )
            .size(px(side)),
        )
        .into_any_element()
}

pub struct LoginView {
    store: Entity<Store>,
    mode: Mode,
    login: Entity<InputState>,
    password: Entity<InputState>,
    code: Entity<InputState>,
    token: Entity<InputState>,
    is_bot: bool,
    _subs: Vec<Subscription>,
}

impl LoginView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let login = cx.new(|cx| InputState::new(window, cx).placeholder("Email or phone number"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Password"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("6-digit code"));
        let token = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Token"));
        let mut subs = Vec::new();
        for input in [&login, &password, &code, &token] {
            subs.push(cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit(window, cx);
                }
            }));
        }
        let focus_code = code.clone();
        subs.push(cx.observe_in(&store, window, move |this, store, window, cx| {
            // Put the cursor in the code box when the second step appears.
            if matches!(store.read(cx).auth_step, AuthStep::Code { .. }) && !focus_code.read(cx).focus_handle(cx).is_focused(window) {
                focus_code.update(cx, |s, cx| s.focus(window, cx));
            }
            this.sync_qr(cx);
            cx.notify();
        }));
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = this.update(cx, |this, cx| this.sync_qr(cx));
        });
        Self { store, mode: Mode::Qr, login, password, code, token, is_bot: false, _subs: subs }
    }

    /// Runs a QR sign-in exactly while the QR tab is showing on the login screen.
    fn sync_qr(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let on_login_screen = matches!(store.connection, Connection::LoggedOut | Connection::Failed(_));
        let wanted = on_login_screen && self.mode == Mode::Qr;
        let running = store.qr_step != QrStep::Off;
        if wanted && !running {
            self.store.update(cx, |s, cx| s.start_qr_login(cx));
        } else if !wanted && running {
            self.store.update(cx, |s, cx| s.stop_qr_login(cx));
        }
    }

    fn value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Qr => {}
            Mode::Token => self.submit_token(cx),
            Mode::Password => match self.store.read(cx).auth_step {
                AuthStep::Working => {}
                AuthStep::Code { .. } => {
                    let code = Self::value(&self.code, cx);
                    self.store.update(cx, |s, cx| s.submit_auth_code(code, cx));
                }
                AuthStep::Idle => {
                    let login = Self::value(&self.login, cx);
                    let password = self.password.read(cx).value().to_string();
                    // The password is not needed after this; clear it from the screen.
                    self.password.update(cx, |s, cx| s.set_value("", window, cx));
                    self.store.update(cx, |s, cx| s.password_login(login, password, cx));
                }
            },
        }
    }

    fn submit_token(&mut self, cx: &mut Context<Self>) {
        let token = self.token.read(cx).value().trim().trim_matches('"').to_string();
        if token.is_empty() {
            return;
        }
        let is_bot = self.is_bot || token.starts_with("Bot ");
        let token = token.trim_start_matches("Bot ").to_string();
        credentials::save(&token, is_bot);
        self.store.update(cx, |store, cx| store.login(token, is_bot, cx));
    }

    fn primary_button(id: &'static str, label: &'static str, busy: bool) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(theme::accent())
            .text_color(theme::on_accent())
            .font_weight(FontWeight::SEMIBOLD)
            .when(busy, |d| d.opacity(0.6))
            .when(!busy, |d| d.cursor_pointer().hover(|s| s.opacity(0.9)))
            .child(label)
    }

    fn link(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
        div()
            .id(id)
            .text_size(px(13.))
            .text_color(theme::link())
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(label.into())
    }

    fn tab(&self, id: &'static str, label: &'static str, mode: Mode, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.mode == mode;
        div()
            .id(id)
            .flex_1()
            .py_1p5()
            .flex()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(13.))
            .when(selected, |d| d.bg(theme::bg_active()).text_color(theme::text_bright()))
            .when(!selected, |d| d.text_color(theme::text_muted()).hover(|s| s.bg(theme::bg_hover())))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.mode = mode;
                this.store.update(cx, |s, cx| s.cancel_auth(cx));
                this.sync_qr(cx);
                cx.notify();
            }))
    }

    fn render_qr(&self, cx: &mut Context<Self>) -> Div {
        let step = self.store.read(cx).qr_step.clone();
        let centered = v_flex().gap_3().items_center().py_2();
        match step {
            QrStep::Off | QrStep::Starting => centered
                .child(div().size(px(QR_SIZE + 24.)).flex().items_center().justify_center().rounded_lg().bg(theme::bg_input()).child(
                    div().text_color(theme::text_muted()).child("Preparing a code\u{2026}"),
                ))
                .child(div().text_size(px(13.)).text_color(theme::text_muted()).child(" ")),
            QrStep::Code(url) => centered.child(qr_code(&url)).child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child("Open the Discord app on your phone, go to Settings and choose Scan QR Code."),
            ),
            QrStep::Scanned { username, avatar_url } => centered
                .child(avatar(avatar_url.unwrap_or_default(), 72.))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text_bright()).child("Check your phone"))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme::text_muted())
                        .child(format!("Approve the login for {username} in the Discord app.")),
                )
                .child(Self::link("qr-restart", "Not you? Get a new code").on_click(cx.listener(|this, _, _, cx| {
                    this.store.update(cx, |s, cx| s.start_qr_login(cx));
                }))),
            QrStep::Ended(reason) => centered
                .child(div().text_color(theme::text()).child(reason))
                .child(
                    Self::primary_button("qr-new", "Get a new code", false)
                        .w(px(200.))
                        .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.start_qr_login(cx)))),
                ),
        }
    }

    fn render_password(&self, cx: &mut Context<Self>) -> Div {
        let step = self.store.read(cx).auth_step.clone();
        match step {
            AuthStep::Code { texted_to, totp, sms, .. } => {
                let prompt = match &texted_to {
                    Some(phone) => format!("Enter the code Discord texted to {phone}."),
                    None => "Enter the 6-digit code from your authenticator app, or an 8-digit backup code.".into(),
                };
                v_flex()
                    .gap_3()
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text_bright()).child("Two-factor authentication"))
                    .child(div().text_size(px(13.)).text_color(theme::text_muted()).child(prompt))
                    .child(Input::new(&self.code))
                    .child(
                        Self::primary_button("verify", "Verify", false)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .child(Self::link("auth-back", "Back").on_click(cx.listener(|this, _, _, cx| {
                                this.store.update(cx, |s, cx| s.cancel_auth(cx));
                            })))
                            .when(totp && sms && texted_to.is_none(), |d| {
                                d.child(Self::link("auth-sms", "Text me a code instead").on_click(cx.listener(
                                    |this, _, _, cx| this.store.update(cx, |s, cx| s.text_auth_code(cx)),
                                )))
                            }),
                    )
            }
            step => {
                let busy = step == AuthStep::Working;
                v_flex()
                    .gap_3()
                    .child(Input::new(&self.login))
                    .child(Input::new(&self.password).mask_toggle())
                    .child(
                        Self::primary_button("login-password", if busy { "Logging in\u{2026}" } else { "Log in" }, busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    )
            }
        }
    }

    fn render_token(&self, cx: &mut Context<Self>) -> Div {
        let is_bot = self.is_bot;
        v_flex()
            .gap_3()
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
                            .when(is_bot, |d| d.child(icon(IconName::Check, 13., theme::on_accent()))),
                    )
                    .child("This is a bot token")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.is_bot = !this.is_bot;
                        cx.notify();
                    })),
            )
            .child(
                Self::primary_button("login-token", "Log in", false)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
    }
}

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let error = match (&store.connection, &store.auth_error) {
            (_, Some(auth)) if self.mode == Mode::Password => Some(auth.clone()),
            (Connection::Failed(reason), _) => Some(reason.clone()),
            _ => None,
        };
        let body = match self.mode {
            Mode::Qr => self.render_qr(cx),
            Mode::Password => self.render_password(cx),
            Mode::Token => self.render_token(cx),
        };
        let note = match self.mode {
            Mode::Qr => {
                "You approve the login in Discord's own app, so no password or two-factor code is \
                 needed here. Ferrocord keeps the session token in your operating system's \
                 credential store. Using a personal account with a third-party client is against \
                 Discord's terms of service."
            }
            Mode::Password => {
                "Your password goes only to Discord and is never saved; Ferrocord keeps the session \
                 token in your operating system's credential store. Using a personal account with a \
                 third-party client is against Discord's terms of service."
            }
            Mode::Token => {
                "The token is stored in your operating system's credential store. Using a personal \
                 account token with a third-party client is against Discord's terms of service; a bot \
                 token is the sanctioned option."
            }
        };

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
                        .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).text_color(theme::text_bright()).child("Ferrocord"))
                        .child(div().text_color(theme::text_muted()).child("A fast, native Discord client.")),
                )
                .child(
                    h_flex()
                        .p_1()
                        .gap_1()
                        .rounded_lg()
                        .bg(theme::bg_input())
                        .child(self.tab("mode-qr", "QR code", Mode::Qr, cx))
                        .child(self.tab("mode-password", "Password", Mode::Password, cx))
                        .child(self.tab("mode-token", "Token", Mode::Token, cx)),
                )
                .child(body)
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
                .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(note)),
        )
    }
}
