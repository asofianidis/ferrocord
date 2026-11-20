#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod credentials;
mod discord;
mod emoji;
mod http;
mod markdown;
mod settings;
mod store;
mod voice;
mod ui;

use gpui::{prelude::*, *};
use std::sync::Arc;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Networking runs on Tokio; the UI runs on GPUI's own executor.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to start the async runtime");
    let handle = runtime.handle().clone();
    let http_client = Arc::new(http::ReqwestClient::new(handle.clone()));

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .with_http_client(http_client)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            let saved = settings::Settings::load();
            let themes = ui::theme::load_all(settings::themes_dir().as_deref());
            let palette = themes.iter().find(|t| t.id == saved.theme).unwrap_or(&themes[0]);
            ui::theme::apply(palette, cx);

            let store = cx.new(|_| store::Store::new(handle.clone()));
            if std::env::args().any(|arg| arg.starts_with("--demo")) {
                store.update(cx, |store, cx| store.load_demo(cx));
            } else if let Some((token, is_bot)) = credentials::load() {
                store.update(cx, |store, cx| store.login(token, is_bot, cx));
            }

            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(820.), px(480.))),
                titlebar: Some(TitlebarOptions { title: Some("Ferrocord".into()), ..Default::default() }),
                ..Default::default()
            };
            let (window, view) =
                gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| ui::AppView::new(store, window, cx)))
                    .expect("failed to open the main window");

            let settings_view = view.clone();
            // Registered globally so the shortcuts work wherever focus is.
            cx.bind_keys([
                KeyBinding::new("ctrl-k", ui::ToggleSwitcher, None),
                #[cfg(target_os = "macos")]
                KeyBinding::new("cmd-k", ui::ToggleSwitcher, None),
            ]);
            cx.on_action(move |_: &ui::ToggleSwitcher, cx| {
                let view = view.clone();
                // The action arrives while the window is mid-update; act once it is released.
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        view.update(cx, |view, cx| view.toggle_switcher(window, cx));
                    });
                });
            });
            cx.bind_keys([KeyBinding::new("ctrl-,", ui::ToggleSettings, None)]);
            cx.on_action(move |_: &ui::ToggleSettings, cx| {
                let view = settings_view.clone();
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        view.update(cx, |view, cx| view.toggle_settings(window, cx));
                    });
                });
            });
            if std::env::args().any(|arg| arg == "--demo-switcher") {
                cx.dispatch_action(&ui::ToggleSwitcher);
            }
            if std::env::args().any(|arg| arg == "--demo-settings") {
                cx.dispatch_action(&ui::ToggleSettings);
            }
            cx.activate(true);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        });
}
