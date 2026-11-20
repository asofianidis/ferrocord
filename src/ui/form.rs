//! A small reusable form dialog (rename, create channel, nickname, ...),
//! and helpers that let any view open dialogs owned by the app window.

use super::{theme, widgets::*};
use gpui::{prelude::*, *};
use gpui_component::input::{self, Input, InputEvent, InputState};
use gpui_kit::assets::IconName;

pub struct Field {
    pub label: String,
    pub value: String,
    pub placeholder: String,
    /// Mask the text, for passwords.
    pub secret: bool,
}

impl Field {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self { label: label.into(), value: value.into(), placeholder: String::new(), secret: false }
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
}

pub struct Choice {
    pub label: String,
    pub options: Vec<String>,
    pub selected: usize,
}

pub struct FormResult {
    pub values: Vec<String>,
    pub choice: usize,
}

pub struct FormSpec {
    pub title: String,
    pub description: Option<String>,
    pub fields: Vec<Field>,
    pub choice: Option<Choice>,
    pub submit: String,
    pub danger: bool,
    pub on_submit: Box<dyn FnOnce(FormResult, &mut Window, &mut App)>,
}

impl FormSpec {
    pub fn new(
        title: impl Into<String>,
        submit: impl Into<String>,
        on_submit: impl FnOnce(FormResult, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            description: None,
            fields: Vec::new(),
            choice: None,
            submit: submit.into(),
            danger: false,
            on_submit: Box::new(on_submit),
        }
    }

    pub fn field(mut self, field: Field) -> Self {
        self.fields.push(field);
        self
    }

    pub fn choice(mut self, label: impl Into<String>, options: &[&str], selected: usize) -> Self {
        self.choice = Some(Choice {
            label: label.into(),
            options: options.iter().map(|o| o.to_string()).collect(),
            selected,
        });
        self
    }

    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }
}

pub enum FormEvent {
    Close,
}

pub struct FormDialog {
    title: SharedString,
    description: Option<SharedString>,
    inputs: Vec<(SharedString, Entity<InputState>)>,
    choice: Option<Choice>,
    submit: SharedString,
    danger: bool,
    on_submit: Option<Box<dyn FnOnce(FormResult, &mut Window, &mut App)>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<FormEvent> for FormDialog {}

impl FormDialog {
    pub fn new(spec: FormSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subs = Vec::new();
        let inputs: Vec<_> = spec
            .fields
            .into_iter()
            .map(|field| {
                let placeholder = field.placeholder.clone();
                let value = field.value.clone();
                let secret = field.secret;
                let state = cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder(placeholder).masked(secret);
                    s.set_value(value, window, cx);
                    s
                });
                subs.push(cx.subscribe_in(&state, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit(window, cx);
                    }
                }));
                (SharedString::from(field.label), state)
            })
            .collect();
        if let Some((_, first)) = inputs.first() {
            first.update(cx, |s, cx| s.focus(window, cx));
        }
        Self {
            title: spec.title.into(),
            description: spec.description.map(Into::into),
            inputs,
            choice: spec.choice,
            submit: spec.submit.into(),
            danger: spec.danger,
            on_submit: Some(spec.on_submit),
            _subs: subs,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(on_submit) = self.on_submit.take() else { return };
        let values = self.inputs.iter().map(|(_, s)| s.read(cx).value().trim().to_string()).collect();
        let choice = self.choice.as_ref().map_or(0, |c| c.selected);
        cx.emit(FormEvent::Close);
        on_submit(FormResult { values, choice }, window, cx);
    }
}

impl Render for FormDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (bg, fg) = if self.danger {
            (theme::danger(), gpui::white())
        } else {
            (theme::accent(), theme::on_accent())
        };
        div()
            .id("form-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.55))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(FormEvent::Close)))
            .capture_action(cx.listener(|_, _: &input::Escape, _, cx| {
                cx.emit(FormEvent::Close);
                cx.stop_propagation();
            }))
            .child(
                v_flex()
                    .id("form")
                    .w(px(440.))
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
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_bright())
                                    .child(self.title.clone()),
                            )
                            .child(
                                icon_button("form-close", IconName::X, "Close")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FormEvent::Close))),
                            ),
                    )
                    .children(self.description.clone().map(|d| div().text_color(theme::text_muted()).child(d)))
                    .children(self.inputs.iter().map(|(label, state)| {
                        v_flex()
                            .gap_1()
                            .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(label.clone()))
                            .child(Input::new(state))
                    }))
                    .when_some(self.choice.as_ref(), |d, choice| {
                        let selected = choice.selected;
                        d.child(div().text_size(px(12.)).text_color(theme::text_muted()).child(choice.label.clone()))
                            .child(h_flex().flex_wrap().gap_2().children(choice.options.iter().enumerate().map(
                                |(ix, option)| {
                                    div()
                                        .id(("form-choice", ix))
                                        .px_3()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .border_1()
                                        .border_color(if ix == selected { theme::accent() } else { theme::border() })
                                        .when(ix == selected, |d| d.bg(theme::accent().opacity(0.15)))
                                        .child(option.clone())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(choice) = this.choice.as_mut() {
                                                choice.selected = ix;
                                            }
                                            cx.notify();
                                        }))
                                },
                            )))
                    })
                    .child(
                        h_flex().justify_end().gap_2().child(
                            div()
                                .id("form-submit")
                                .px_4()
                                .py_1p5()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(bg)
                                .text_color(fg)
                                .font_weight(FontWeight::MEDIUM)
                                .hover(|s| s.opacity(0.85))
                                .child(self.submit.clone())
                                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                        ),
                    ),
            )
    }
}

// ---------------------------------------------------------------- app-level dialogs

/// Lets any view reach the app window's dialog host.
pub struct AppHandle(pub WeakEntity<super::AppView>);

impl Global for AppHandle {}

fn with_app(cx: &mut App, f: impl FnOnce(&mut super::AppView, &mut Context<super::AppView>)) {
    let Some(app) = cx.try_global::<AppHandle>().and_then(|h| h.0.upgrade()) else { return };
    app.update(cx, f);
}

pub fn open_form(spec: FormSpec, window: &mut Window, cx: &mut App) {
    with_app(cx, |app, cx| app.show_form(spec, window, cx));
}

pub fn open_server_settings(guild: crate::discord::Id, window: &mut Window, cx: &mut App) {
    with_app(cx, |app, cx| app.show_server_settings(guild, window, cx));
}

pub fn open_channel_permissions(channel: crate::discord::Id, cx: &mut App) {
    with_app(cx, |app, cx| app.show_channel_permissions(channel, cx));
}

/// Asks before doing something destructive.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    title: &str,
    detail: &str,
    button: &str,
    on_yes: impl FnOnce(&mut App) + 'static,
) {
    let answer = window.prompt(PromptLevel::Warning, title, Some(detail), &[button, "Cancel"], cx);
    cx.spawn(async move |cx| {
        if answer.await == Ok(0) {
            let _ = cx.update(on_yes);
        }
    })
    .detach();
}
