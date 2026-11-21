//! The settings dialog: themes, presence and account.

use super::{
    form::{self, Field, FormSpec},
    theme::{self, Palette},
    widgets::*,
};
use crate::{
    credentials,
    discord::Status,
    settings,
    store::{Profile, Store, StoreEvent, image_mime},
};
use gpui::{prelude::*, *};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::assets::IconName;

pub enum SettingsEvent {
    Close,
}

pub struct SettingsView {
    store: Entity<Store>,
    themes: Vec<Palette>,
    saved: settings::Settings,
    /// Audio devices, listed in the background when the dialog opens.
    devices: Option<crate::voice::Devices>,
    input_volume: Entity<SliderState>,
    output_volume: Entity<SliderState>,
    sensitivity: Entity<SliderState>,
    /// Listens to the microphone while settings are open, for the level meter.
    mic_test: Option<crate::voice::MicTest>,
    mic_problem: Option<String>,
    /// Whether Windows starts the app at sign-in, read from the registry.
    start_with_windows: bool,
    /// Why the last change to that could not be made.
    autostart_problem: Option<String>,
    _subs: Vec<Subscription>,
}

/// The sensitivity slider and level meter span this range, in dBFS.
const METER_FLOOR_DB: f32 = -80.0;

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        store.update(cx, |store, cx| store.load_my_profile(cx));
        cx.spawn(async move |this, cx| {
            let devices = cx.background_executor().spawn(async { crate::voice::list_devices() }).await;
            let _ = this.update(cx, |this, cx| {
                this.devices = Some(devices);
                cx.notify();
            });
        })
        .detach();
        let saved = settings::Settings::load();
        let mut percent = |v: f32| cx.new(|_| SliderState::new().min(0.).max(200.).step(1.).default_value((v * 100.).round()));
        let input_volume = percent(saved.input_volume);
        let output_volume = percent(saved.output_volume);
        let sensitivity = cx.new(|_| {
            SliderState::new().min(METER_FLOOR_DB).max(0.).step(1.).default_value(saved.input_sensitivity_db.clamp(METER_FLOOR_DB, 0.))
        });
        let subs = vec![
            cx.subscribe(&input_volume, |this, _, event: &SliderEvent, cx| {
                let (value, done) = slider_value(event);
                let volume = value / 100.;
                this.saved.input_volume = volume;
                this.store.update(cx, |s, _| s.set_input_volume(volume));
                if let Some(test) = &this.mic_test {
                    test.set_volume(volume);
                }
                if done {
                    settings::Settings::update(|s| s.input_volume = volume);
                }
                cx.notify();
            }),
            cx.subscribe(&output_volume, |this, _, event: &SliderEvent, cx| {
                let (value, done) = slider_value(event);
                let volume = value / 100.;
                this.saved.output_volume = volume;
                this.store.update(cx, |s, _| s.set_output_volume(volume));
                if done {
                    settings::Settings::update(|s| s.output_volume = volume);
                }
                cx.notify();
            }),
            cx.subscribe(&sensitivity, |this, _, event: &SliderEvent, cx| {
                let (db, done) = slider_value(event);
                this.saved.input_sensitivity_db = db;
                this.store.update(cx, |s, _| s.set_input_sensitivity(db));
                if done {
                    settings::Settings::update(|s| s.input_sensitivity_db = db);
                }
                cx.notify();
            }),
        ];
        // Redraw the level meter while the dialog is open.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.mic_test.is_some() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut view = Self {
            store,
            themes: theme::load_all(settings::themes_dir().as_deref()),
            saved,
            devices: None,
            input_volume,
            output_volume,
            sensitivity,
            mic_test: None,
            mic_problem: None,
            start_with_windows: crate::autostart::is_enabled(),
            autostart_problem: None,
            _subs: subs,
        };
        view.restart_mic_test();
        view
    }

    fn restart_mic_test(&mut self) {
        self.mic_test = None;
        match crate::voice::MicTest::start(self.saved.input_device.as_deref(), self.saved.input_volume) {
            Ok(test) => {
                self.mic_test = Some(test);
                self.mic_problem = None;
            }
            Err(problem) => self.mic_problem = Some(problem),
        }
    }

    fn label_row(label: &'static str, value: String) -> Div {
        h_flex()
            .text_size(px(12.))
            .text_color(theme::text_muted())
            .child(div().flex_1().child(label))
            .child(div().text_color(theme::text()).child(value))
    }

    fn slider(state: &Entity<SliderState>) -> Slider {
        Slider::new(state).bg(theme::accent()).text_color(theme::text_bright())
    }

    /// The microphone's live level, with the part above the sensitivity
    /// threshold (what would be sent) in green.
    fn level_meter(&self) -> AnyElement {
        let Some(test) = &self.mic_test else {
            let note = self.mic_problem.clone().unwrap_or_else(|| "Starting the microphone\u{2026}".into());
            return div().text_size(px(12.)).text_color(theme::text_muted()).child(note).into_any_element();
        };
        let span = -METER_FLOOR_DB;
        let level = ((test.level_db() - METER_FLOOR_DB) / span).clamp(0., 1.);
        let threshold = ((self.saved.input_sensitivity_db - METER_FLOOR_DB) / span).clamp(0., 1.);
        let fill_color = if level >= threshold { theme::online() } else { theme::warning() };
        let (track, marker) = (theme::bg_active(), theme::text_bright());
        let meter = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let width = bounds.size.width;
                let height = bounds.size.height;
                window.paint_quad(fill(bounds, track).corner_radii(height / 2.));
                if level > 0. {
                    let filled = Bounds::new(bounds.origin, size(width * level, height));
                    window.paint_quad(fill(filled, fill_color).corner_radii(height / 2.));
                }
                let x = bounds.origin.x + width * threshold - px(1.);
                window.paint_quad(fill(Bounds::new(point(x, bounds.origin.y), size(px(2.), height)), marker));
            },
        )
        .size_full();
        div().w_full().h(px(8.)).flex_shrink_0().rounded_full().bg(theme::bg_active()).child(meter).into_any_element()
    }

    fn choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(palette) = self.themes.get(ix) else { return };
        theme::apply(palette, cx);
        let id = palette.id.clone();
        self.saved.theme = id.clone();
        settings::Settings::update(|s| s.theme = id);
        cx.notify();
    }

    fn set_device(&mut self, input: bool, name: Option<String>, cx: &mut Context<Self>) {
        if input {
            self.saved.input_device = name.clone();
            self.restart_mic_test();
        } else {
            self.saved.output_device = name.clone();
        }
        settings::Settings::update(|s| {
            if input {
                s.input_device = name;
            } else {
                s.output_device = name;
            }
        });
        self.store.update(cx, |store, cx| store.restart_voice_audio(cx));
        cx.notify();
    }

    fn device_list(&self, input: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(devices) = &self.devices else {
            return div().text_color(theme::text_muted()).child("Looking for audio devices\u{2026}").into_any_element();
        };
        let names = if input { &devices.inputs } else { &devices.outputs };
        let current = if input { &self.saved.input_device } else { &self.saved.output_device };
        let key = if input { "in" } else { "out" };
        let mut options = vec![(None, "System default".to_string())];
        options.extend(names.iter().map(|n| (Some(n.clone()), n.clone())));
        h_flex()
            .flex_wrap()
            .gap_2()
            .children(options.into_iter().enumerate().map(|(ix, (value, label))| {
                let selected = *current == value;
                Self::pill(SharedString::from(format!("{key}-device-{ix}")), label, selected)
                    .max_w(px(280.))
                    .truncate()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_device(input, value.clone(), cx)))
            }))
            .into_any_element()
    }

    fn edit_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let Some(me) = store.me.clone() else { return };
        let profile = store.my_profile.clone().unwrap_or_default();
        let handle = self.store.clone();
        let spec = FormSpec::new("Edit Profile", "Save", move |result, _, cx| {
            let [name, pronouns, bio] = <[String; 3]>::try_from(result.values).unwrap_or_default();
            handle.update(cx, |s, cx| s.edit_profile(name, Profile { bio, pronouns }, cx));
        })
        .field(Field::new("Display name", me.global_name.unwrap_or_default()).placeholder(me.username))
        .field(Field::new("Pronouns", profile.pronouns))
        .field(Field::new("About me", profile.bio));
        form::open_form(spec, window, cx);
    }

    fn change_username(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let Some(me) = store.me.clone() else { return };
        let bot = store.is_bot();
        let handle = self.store.clone();
        let mut spec = FormSpec::new("Change Username", "Change", move |result, _, cx| {
            let mut values = result.values.into_iter();
            let username = values.next().unwrap_or_default();
            let password = values.next().unwrap_or_default();
            if username.is_empty() {
                return;
            }
            handle.update(cx, |s, cx| s.change_username(username, password, cx));
        })
        .field(Field::new("Username", me.username));
        if !bot {
            spec = spec
                .description("Discord asks for your password to change your username.")
                .field(Field::new("Current password", "").secret());
        }
        form::open_form(spec, window, cx);
    }

    fn pick_avatar(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose avatar".into()),
        });
        let store = self.store.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let bytes = cx.background_executor().spawn(async move { std::fs::read(path) }).await;
            let _ = store.update(cx, |store, cx| match bytes {
                Ok(bytes) => match image_mime(&bytes) {
                    Some(mime) => store.set_avatar(Some((mime.to_string(), bytes)), cx),
                    None => cx.emit(StoreEvent::Error("Choose a PNG, JPEG, GIF or WebP image.".into())),
                },
                Err(e) => cx.emit(StoreEvent::Error(format!("Could not read the image: {e}"))),
            });
        })
        .detach();
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

/// A slider event's value, and whether the user has let go (time to save).
fn slider_value(event: &SliderEvent) -> (f32, bool) {
    match event {
        SliderEvent::Change(v) => (v.start(), false),
        SliderEvent::Release(v) => (v.start(), true),
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let me = store.me.clone();
        let bot = store.is_bot();
        let profile = store.my_profile.clone().unwrap_or_default();
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
                    .child(Self::section("VOICE"))
                    .child(div().text_size(px(12.)).text_color(theme::text_muted()).child("Microphone"))
                    .child(self.device_list(true, cx))
                    .child(div().text_size(px(12.)).text_color(theme::text_muted()).child("Output"))
                    .child(self.device_list(false, cx))
                    .child(Self::label_row("Input volume", format!("{:.0}%", self.saved.input_volume * 100.)))
                    .child(Self::slider(&self.input_volume))
                    .child(Self::label_row(
                        "Input sensitivity",
                        format!("{:.0} dB", self.saved.input_sensitivity_db),
                    ))
                    .child(self.level_meter())
                    .child(Self::slider(&self.sensitivity))
                    .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(
                        "Speak normally: your voice is sent while the bar is green. Move the slider right if \
                         background noise turns it green, left if your voice does not.",
                    ))
                    .child(Self::label_row("Output volume", format!("{:.0}%", self.saved.output_volume * 100.)))
                    .child(Self::slider(&self.output_volume))
                    .child(h_flex().gap_2().pt_1().children([(true, "Play voice sounds"), (false, "No voice sounds")].into_iter().map(
                        |(value, label)| {
                            Self::pill(SharedString::from(format!("sounds-{value}")), label, self.saved.voice_sounds == value)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.saved.voice_sounds = value;
                                    settings::Settings::update(|s| s.voice_sounds = value);
                                    if value {
                                        crate::sounds::play(crate::sounds::Sound::Join);
                                    }
                                    cx.notify();
                                }))
                        },
                    )))
                    .when(cfg!(windows), |d| {
                        let to_tray = self.saved.close_to_tray;
                        d.child(Self::section("WHEN YOU CLOSE THE WINDOW"))
                            .child(h_flex().gap_2().children([(true, "Keep running in the system tray"), (false, "Quit Ferrocord")].into_iter().map(
                                |(value, label)| {
                                    Self::pill(SharedString::from(format!("close-{value}")), label, to_tray == value).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.saved.close_to_tray = value;
                                            settings::Settings::update(|s| s.close_to_tray = value);
                                            cx.notify();
                                        }),
                                    )
                                },
                            )))
                            .child(Self::section("WHEN YOU SIGN IN TO WINDOWS"))
                            .child(h_flex().gap_2().children([(true, "Start Ferrocord in the tray"), (false, "Don't start")].into_iter().map(
                                |(value, label)| {
                                    Self::pill(SharedString::from(format!("autostart-{value}")), label, self.start_with_windows == value)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            match crate::autostart::set(value) {
                                                Ok(()) => this.autostart_problem = None,
                                                Err(e) => {
                                                    log::warn!("could not change the sign-in entry: {e}");
                                                    this.autostart_problem = Some(format!("Could not change this: {e}"));
                                                }
                                            }
                                            this.start_with_windows = crate::autostart::is_enabled();
                                            cx.notify();
                                        }))
                                },
                            )))
                            .when_some(self.autostart_problem.clone(), |d, problem| d.child(div().pt_1().text_size(px(12.)).text_color(theme::text_muted()).child(problem)))
                    })
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
                    .child(Self::section("UPDATES"))
                    .child({
                        let state = self.store.read(cx).update_state.clone();
                        let status = match &state {
                            crate::store::UpdateState::Idle => String::new(),
                            crate::store::UpdateState::Checking => "Checking\u{2026}".into(),
                            crate::store::UpdateState::UpToDate => "You have the latest version.".into(),
                            crate::store::UpdateState::Available { version, .. } => format!("Version {version} is available."),
                            crate::store::UpdateState::Installing => "Downloading the update\u{2026}".into(),
                            crate::store::UpdateState::Failed(e) => e.clone(),
                        };
                        let available = matches!(state, crate::store::UpdateState::Available { .. });
                        h_flex()
                            .gap_3()
                            .child(div().text_color(theme::text_bright()).child(format!("Ferrocord {}", crate::updater::current_version())))
                            .child(
                                Self::pill(
                                    "update-action",
                                    if available { "Restart to update" } else { "Check for updates" },
                                    available,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.store.update(cx, |s, cx| {
                                        if available { s.install_update(cx) } else { s.check_for_updates(cx) }
                                    })
                                })),
                            )
                            .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(theme::text_muted()).child(status))
                    })
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
                                            div().text_size(px(12.)).text_color(theme::text_muted()).child(
                                                if profile.pronouns.is_empty() {
                                                    me.tag()
                                                } else {
                                                    format!("{} \u{b7} {}", me.tag(), profile.pronouns)
                                                },
                                            ),
                                        )
                                        .when(!profile.bio.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .pt_1()
                                                    .text_size(px(12.))
                                                    .text_color(theme::text())
                                                    .child(profile.bio.clone()),
                                            )
                                        }),
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
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .when(!bot, |d| {
                                    d.child(Self::pill("edit-profile", "Edit profile", false).on_click(
                                        cx.listener(|this, _, window, cx| this.edit_profile(window, cx)),
                                    ))
                                })
                                .child(Self::pill("change-username", "Change username", false).on_click(
                                    cx.listener(|this, _, window, cx| this.change_username(window, cx)),
                                ))
                                .child(
                                    Self::pill("change-avatar", "Change avatar", false)
                                        .on_click(cx.listener(|this, _, _, cx| this.pick_avatar(cx))),
                                )
                                .when(me.avatar.is_some(), |d| {
                                    d.child(Self::pill("remove-avatar", "Remove avatar", false).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.store.update(cx, |s, cx| s.set_avatar(None, cx));
                                        }),
                                    ))
                                }),
                        )
                    }),
            )
    }
}
