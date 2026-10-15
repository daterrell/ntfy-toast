//! Windows integration: toast identity, action URI scheme, autostart, single instance, console.
//! Every function is a no-op (or a plain error) on other platforms.

use anyhow::Result;

#[cfg_attr(not(windows), allow(dead_code))]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg_attr(not(windows), allow(dead_code))]
const RUN_VALUE: &str = "ntfy-toast";

#[cfg_attr(not(windows), allow(dead_code))]
fn protocol_key() -> String {
    format!(r"Software\Classes\{}", crate::action::SCHEME)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn aumid_key(app_id: &str) -> String {
    format!(r"Software\Classes\AppUserModelId\{app_id}")
}

/// GUI-subsystem exes have no console; borrow the parent's so CLI output is visible.
pub fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Register the AppUserModelID under HKCU so unpackaged toasts show our name and icon
/// (no Start Menu shortcut needed).
#[cfg(windows)]
pub fn register_app_id(app_id: &str, display_name: &str, icon: &std::path::Path) -> Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(aumid_key(app_id))?;
    key.set_value("DisplayName", &display_name)?;
    key.set_value("IconUri", &icon.display().to_string())?;
    key.set_value("IconBackgroundColor", &"FF338574")?;
    Ok(())
}

/// Route `ntfy-toast-action:` URIs (http action buttons) to `<this exe> action <uri>`.
#[cfg(windows)]
pub fn register_action_protocol() -> Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let exe = std::env::current_exe()?;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(protocol_key())?;
    key.set_value("", &"URL:ntfy-toast action")?;
    key.set_value("URL Protocol", &"")?;
    let (cmd, _) = key.create_subkey(r"shell\open\command")?;
    cmd.set_value("", &format!("\"{}\" action \"%1\"", exe.display()))?;
    Ok(())
}

#[cfg(windows)]
pub fn unregister_action_protocol() -> Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(protocol_key()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(windows)]
pub fn set_autostart(enable: bool) -> Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
    if enable {
        let exe = std::env::current_exe()?;
        key.set_value(RUN_VALUE, &format!("\"{}\" run", exe.display()))?;
    } else {
        match key.delete_value(RUN_VALUE) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn unregister_app_id(app_id: &str) -> Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(aumid_key(app_id)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Returns false if another daemon is already running for this user session. With `wait`,
/// keeps trying that long first (a restart after an update overlaps the old process).
#[cfg(windows)]
pub fn acquire_single_instance(name: &str, wait: std::time::Duration) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let deadline = std::time::Instant::now() + wait;
    loop {
        unsafe {
            match CreateMutexW(None, true, &HSTRING::from(name)) {
                Ok(handle) if GetLastError() == ERROR_ALREADY_EXISTS => {
                    let _ = CloseHandle(handle);
                }
                // The handle is intentionally leaked: it must live as long as the process.
                Ok(_handle) => return true,
                Err(_) => return true,
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Call `on_signal` each time another process signals the named event (see `signal`).
/// Blocks forever; run it on its own thread.
#[cfg(windows)]
pub fn wait_for_signals(name: &str, mut on_signal: impl FnMut()) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    let event = match unsafe { CreateEventW(None, false, false, &HSTRING::from(name)) } {
        Ok(e) => e,
        Err(e) => {
            log::warn!("could not create {name}: {e}");
            return;
        }
    };
    while unsafe { WaitForSingleObject(event, INFINITE) } == WAIT_OBJECT_0 {
        on_signal();
    }
}

/// Signal a running daemon's named event. Returns false if no daemon is listening.
#[cfg(windows)]
pub fn signal(name: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(name)) {
            Ok(event) => {
                let ok = SetEvent(event).is_ok();
                let _ = CloseHandle(event);
                ok
            }
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
pub fn register_app_id(_: &str, _: &str, _: &std::path::Path) -> Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn set_autostart(_enable: bool) -> Result<()> {
    anyhow::bail!("autostart is only supported on Windows")
}

#[cfg(not(windows))]
pub fn unregister_app_id(_: &str) -> Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn register_action_protocol() -> Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn unregister_action_protocol() -> Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn acquire_single_instance(_name: &str, _wait: std::time::Duration) -> bool {
    true
}

#[cfg(not(windows))]
pub fn wait_for_signals(_name: &str, _on_signal: impl FnMut()) {}

#[cfg(not(windows))]
pub fn signal(_name: &str) -> bool {
    false
}
