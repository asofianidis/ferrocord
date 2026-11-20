//! The settings dialog: themes, presence and account.

use super::{
    theme::{self, Palette},
    widgets::*,
};
use crate::{credentials, discord::Status, settings, store::Store};
use gpui::{prelude::*, *};
use gpui_kit::assets::IconName;

pub enum SettingsEvent {
    Close,
}

pub struct SettingsView {
    store: Entity<Store>,
    themes: Vec<Palette>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self { store, themes: theme::load_all(settings::themes_dir().as_deref()) }
    }

    fn choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(palette) = self.themes.get(ix) else { return };
        theme::apply(palette, cx);
        let id = palette.id.clone();
        settings::Settings::update(|s| s.theme = id);
        cx.notify();
    }

    fn open_themes_folder(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = settings::themes_dir() else { return };
        let readme = format!(
            "Put theme files here as <name>.json, then press Reload in Settings.\n\
             Every colour is optional; missing ones come from Blossom\n\
             (or Blossom Light when \"dark\" is false).\n\nExample:\n\n{}",
            theme::EXAMPLE_THEME
        );
        let prepared = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("README.txt"), readme));
        if let Err(e) = prepared.and_then(|_| open::that(&dir)) {
            log::warn!("could not open the themes folder: {e}");
        }
        cx.notify();
    }

    fn section(title: &'static str) -> Div {
        div()
            .pt_2()
            .text_size(px(11.))
            .font_weight(FontWeight::BOLD)
            .text_color(theme::text_muted())
            .child(title)
    }

    fn pill(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .border_1()
            .border_color(if selected { theme::accent() } else { theme::border() })
            .when(selected, |d| d.bg(theme::accent().opacity(0.15)).text_color(theme::text_bright()))
            .hover(|s| s.bg(theme::bg_hover()))
            .child(label.into())
    }

    /// A miniature of the app drawn in the theme's own colours.
    fn preview(p: &Palette) -> impl IntoElement {
        let c = theme::c;
        let line = |w: f32, color: u32| div().h(px(5.)).w(px(w)).rounded_full().bg(c(color));
        h_flex()
            .h(px(84.))
            .w_full()
            .items_stretch()
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(c(p.border))
            .child(
                v_flex()
                    .w(px(16.))
                    .py_1p5()
                    .gap_1()
                    .items_center()
                    .bg(c(p.bg_rail))
                    .child(div().size(px(8.)).rounded_full().bg(c(p.accent)))
                    .child(div().size(px(8.)).rounded_full().bg(c(p.bg_active)))
                    .child(div().size(px(8.)).rounded_full().bg(c(p.bg_active))),
            )
            .child(
                v_flex()
                    .w(px(44.))
                    .p_1p5()
                    .gap_1p5()
                    .bg(c(p.bg_sidebar))
                    .child(line(26., p.text_muted))
                    .child(div().h(px(9.)).w_full().rounded_sm().bg(c(p.bg_active)))
                    .child(line(22., p.text_muted))
                    .child(line(28., p.text_muted)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_2()
                    .gap_1p5()
                    .bg(c(p.bg_chat))
                    .child(line(34., p.accent_text))
                    .child(line(88., p.text))
                    .child(line(60., p.text))
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .h(px(14.))
                            .px_1()
                            .justify_end()
                            .rounded_sm()
                            .bg(c(p.bg_input))
                            .child(div().size(px(8.)).rounded_sm().bg(c(p.accent))),
                    ),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let me = store.me.clone();
        let status = store.my_status;
        let current = theme::current_id();

        let themes = h_flex().flex_wrap().items_start().gap_3().children(self.themes.iter().enumerate().map(
            |(ix, palette)| {
                let selected = palette.id == current;
                v_flex()
                    .id(("theme", ix))
                    .w(px(176.))
                    .p_1p5()
                    .gap_1p5()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_2()
                    .border_color(if selected { theme::accent() } else { theme::border() })
                    .hover(|s| s.bg(theme::bg_hover()))
                    .child(Self::preview(palette))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .px_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme::text_bright())
                                    .child(palette.name.clone()),
                            )
                            .when(selected, |d| d.child(icon(IconName::Check, 14., theme::accent()))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(ix, cx)))
            },
        ));

        div()
            .id("settings-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.55))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))
            .child(
                v_flex()
                    .id("settings")
                    .w(px(640.))
                    .max_h(px(620.))
                    .p_5()
                    .gap_3()
                    .rounded_xl()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
                    .shadow_lg()
                    .overflow_y_scroll()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_bright())
                                    .child("Settings"),
                            )
                            .child(
                                icon_button("close-settings", IconName::X, "Close")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                            ),
                    )
                    .child(Self::section("THEME"))
                    .child(themes)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Self::pill("themes-folder", "Open themes folder", false)
                                    .on_click(cx.listener(|this, _, _, cx| this.open_themes_folder(cx))),
                            )
                            .child(Self::pill("themes-reload", "Reload", false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.themes = theme::load_all(settings::themes_dir().as_deref());
                                    // Re-apply in case the active theme's file changed.
                                    let current = theme::current_id();
                                    if let Some(palette) = this.themes.iter().find(|t| t.id == current).cloned() {
                                        theme::apply(&palette, cx);
                                    }
                                    cx.notify();
                                },
                            )))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child("Custom themes are JSON files; the folder has an example."),
                            ),
                    )
                    .child(Self::section("STATUS"))
                    .child(h_flex().gap_2().children(
                        [Status::Online, Status::Idle, Status::Dnd, Status::Offline].into_iter().enumerate().map(
                            |(ix, option)| {
                                Self::pill(("status", ix), option.label(), option == status)
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().size(px(8.)).rounded_full().bg(status_color(option)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.store.update(cx, |store, cx| store.set_status(option, cx));
                                    }))
                            },
                        ),
                    ))
                    .child(Self::section("ACCOUNT"))
                    .when_some(me, |d, me| {
                        d.child(
                            h_flex()
                                .gap_3()
                                .child(avatar(me.avatar_url(64), 36.))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(theme::text_bright())
                                                .child(me.display_name().to_string()),
                                        )
                                        .child(
                                            div().text_size(px(12.)).text_color(theme::text_muted()).child(me.tag()),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("settings-logout")
                                        .px_3()
                                        .py_1p5()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .bg(theme::danger())
                                        .text_color(gpui::white())
                                        .child("Log out")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            credentials::clear();
                                            this.store.update(cx, |store, cx| store.logout(cx));
                                            cx.emit(SettingsEvent::Close);
                                        })),
                                ),
                        )
                    }),
            )
    }
}
