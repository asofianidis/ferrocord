#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod credentials;
mod discord;
mod emoji;
mod http;
mod markdown;
mod settings;
mod sounds;
mod store;
mod tray;
mod updater;
mod video;
mod voice;
mod ui;

use futures::StreamExt as _;
use gpui::{prelude::*, *};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // The tray icon, its menu and later launches all send commands here.
    let (tray_tx, mut tray_rx) = futures::channel::mpsc::unbounded::<tray::Command>();
    let demo = std::env::args().any(|arg| arg.starts_with("--demo"));
    // Demo mode may run beside a real copy, so it skips the one-copy check.
    if !demo && !tray::single_instance::claim(tray_tx.clone()) {
        return; // Another copy is running and has been asked to show itself.
    }

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
            if demo {
                store.update(cx, |store, cx| store.load_demo(cx));
            } else if let Some((token, is_bot)) = credentials::load() {
                store.update(cx, |store, cx| store.login(token, is_bot, cx));
            }

            let tray = tray::create(tray_tx.clone());
            let has_tray = tray.is_some();
            cx.set_global(TrayHolder(tray));
            // Started by Windows at sign-in: wait in the tray until wanted.
            let start_hidden = has_tray && autostart::launched_at_startup();

            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(820.), px(480.))),
                titlebar: Some(TitlebarOptions { title: Some("Ferrocord".into()), ..Default::default() }),
                show: !start_hidden,
                focus: !start_hidden,
                ..Default::default()
            };
            let checker = store.clone();
            let (window, view) =
                gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| ui::AppView::new(store, window, cx)))
                    .expect("failed to open the main window");

            cx.set_global(ui::form::AppHandle(view.downgrade()));

            // Look for updates now and every six hours (not in development builds).
            if !cfg!(debug_assertions) && !demo {
                cx.spawn(async move |cx| {
                    loop {
                        let _ = checker.update(cx, |s, cx| s.check_for_updates(cx));
                        cx.background_executor().timer(std::time::Duration::from_secs(6 * 60 * 60)).await;
                    }
                })
                .detach();
            }

            // Closing the window hides it to the tray, unless that is turned off
            // in settings or the tray icon could not be created.
            let quitting = Arc::new(AtomicBool::new(false));
            let quitting_check = quitting.clone();
            let _ = window.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |window, _| {
                    if has_tray && !quitting_check.load(Ordering::SeqCst) && settings::Settings::load().close_to_tray {
                        tray::hide(window);
                        return false;
                    }
                    true
                });
            });
            cx.spawn(async move |cx| {
                while let Some(command) = tray_rx.next().await {
                    match command {
                        tray::Command::Show => {
                            let _ = window.update(cx, |_, window, _| tray::show(window));
                        }
                        tray::Command::Quit => {
                            quitting.store(true, Ordering::SeqCst);
                            let _ = cx.update(|cx| cx.quit());
                        }
                    }
                }
            })
            .detach();
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
            if !start_hidden {
                cx.activate(true);
            }
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        });
}

/// Keeps the tray icon alive for as long as the app runs.
struct TrayHolder(#[allow(dead_code)] Option<tray::Tray>);

impl Global for TrayHolder {}
