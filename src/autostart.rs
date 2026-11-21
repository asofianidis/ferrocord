//! Starting ferrocord when the user signs in to Windows.
//!
//! The choice lives in the registry under the user's `Run` key, so it is read
//! from there rather than from the settings file and stays true to what Windows
//! will actually do. A launch from that entry passes [`FLAG`], which starts the
//! app hidden in the system tray.

/// Passed on the command line when Windows starts the app at sign-in.
pub const FLAG: &str = "--startup";

/// Whether this launch came from the sign-in entry.
pub fn launched_at_startup() -> bool {
    std::env::args().any(|arg| arg == FLAG)
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
mod imp {
    pub fn is_enabled() -> bool {
        false
    }
    pub fn set(_: bool) -> anyhow::Result<()> {
        anyhow::bail!("starting at sign-in is only supported on Windows")
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW},
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "Ferrocord";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Whether Windows will start the app at sign-in.
    pub fn is_enabled() -> bool {
        let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        result == ERROR_SUCCESS
    }

    /// Adds or removes the sign-in entry, pointing it at this executable.
    pub fn set(enabled: bool) -> anyhow::Result<()> {
        let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
        let result = if enabled {
            let exe = std::env::current_exe()?;
            let command = wide(&format!("\"{}\" {}", exe.display(), super::FLAG));
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    command.as_ptr().cast(),
                    (command.len() * 2) as u32,
                )
            }
        } else {
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                other => other,
            }
        };
        if result != ERROR_SUCCESS {
            anyhow::bail!("{}", std::io::Error::from_raw_os_error(result as i32));
        }
        Ok(())
    }
}
