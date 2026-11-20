//! Keeping ferrocord running in the background on Windows.
//!
//! Closing the window hides it and leaves an icon in the system tray, which
//! brings the window back or quits. Only one copy runs at a time: starting
//! the app again shows the running copy instead.

use futures::channel::mpsc::UnboundedSender;

/// What the tray icon, its menu or a second launch asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Show,
    Quit,
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct Tray;

    pub fn create(_: UnboundedSender<Command>) -> Option<Tray> {
        None
    }
    pub fn hide(_: &gpui::Window) {}
    pub fn show(window: &gpui::Window) {
        window.activate_window();
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use tray_icon::{
        Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow};

    /// The tray icon; it disappears when this is dropped.
    pub struct Tray(#[allow(dead_code)] TrayIcon);

    /// Puts the icon in the tray. Its clicks and menu choices arrive on `tx`.
    pub fn create(tx: UnboundedSender<Command>) -> Option<Tray> {
        let menu = Menu::new();
        let open = MenuItem::with_id("open", "Open Ferrocord", true, None);
        let quit = MenuItem::with_id("quit", "Quit Ferrocord", true, None);
        let built = menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]);
        if let Err(e) = built {
            log::warn!("could not build the tray menu: {e}");
        }

        let menu_tx = tx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id.0.as_str() {
                "open" => Command::Show,
                "quit" => Command::Quit,
                _ => return,
            };
            let _ = menu_tx.unbounded_send(command);
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let left_click = matches!(
                event,
                TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick { .. }
            );
            if left_click {
                let _ = tx.unbounded_send(Command::Show);
            }
        }));

        let icon = icon()?;
        match TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Ferrocord")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
        {
            Ok(tray) => Some(Tray(tray)),
            Err(e) => {
                log::warn!("could not add the tray icon: {e}");
                None
            }
        }
    }

    /// The app icon embedded in the executable by build.rs, or a plain
    /// Blossom-pink square if that is missing.
    fn icon() -> Option<Icon> {
        Icon::from_resource(1, Some((32, 32))).ok().or_else(|| {
            let pixel = [0xd4, 0x38, 0x7c, 0xff];
            Icon::from_rgba(pixel.repeat(32 * 32), 32, 32).ok()
        })
    }

    fn hwnd(window: &gpui::Window) -> Option<windows_sys::Win32::Foundation::HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as _),
            _ => None,
        }
    }

    pub fn hide(window: &gpui::Window) {
        if let Some(hwnd) = hwnd(window) {
            unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
    }

    pub fn show(window: &gpui::Window) {
        let Some(hwnd) = hwnd(window) else { return };
        unsafe {
            ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(hwnd);
        }
        window.activate_window();
    }
}

/// Making sure only one copy runs.
pub mod single_instance {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
        path::PathBuf,
        time::Duration,
    };

    const HELLO: &str = "ferrocord-show";
    const ACK: &str = "ok";

    fn port_file() -> Option<PathBuf> {
        crate::settings::config_dir().map(|d| d.join("instance.port"))
    }

    /// Asks an already running copy to show its window. Returns true if one answered.
    fn wake_running() -> bool {
        let Some(port) = port_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| s.trim().parse::<u16>().ok())
        else {
            return false;
        };
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else { return false };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
        if writeln!(stream, "{HELLO}").is_err() {
            return false;
        }
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).is_ok() && reply.trim() == ACK
    }

    /// Returns false if another copy is running (it has been asked to show
    /// itself, and this one should exit). Otherwise listens for later
    /// launches and forwards them to `tx` as [`Command::Show`].
    pub fn claim(tx: UnboundedSender<Command>) -> bool {
        if wake_running() {
            return false;
        }
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("could not listen for other launches: {e}");
                return true;
            }
        };
        if let (Some(file), Ok(addr)) = (port_file(), listener.local_addr()) {
            let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&file, addr.port().to_string()));
            if let Err(e) = written {
                log::warn!("could not record the instance port: {e}");
            }
        }
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
                    let mut line = String::new();
                    let mut reader = BufReader::new(&stream);
                    if reader.read_line(&mut line).is_ok() && line.trim() == HELLO {
                        let _ = writeln!(&stream, "{ACK}");
                        let _ = tx.unbounded_send(Command::Show);
                    }
                }
            })
            .ok();
        true
    }
}
