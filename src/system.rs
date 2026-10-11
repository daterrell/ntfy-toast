//! Windows integration: toast identity, autostart, single instance, console.
//! Every function is a no-op (or a plain error) on other platforms.

use anyhow::Result;

#[cfg_attr(not(windows), allow(dead_code))]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg_attr(not(windows), allow(dead_code))]
const RUN_VALUE: &str = "ntfy-toast";

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

/// Returns false if another daemon is already running for this user session.
#[cfg(windows)]
pub fn acquire_single_instance() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        match CreateMutexW(None, true, w!(r"Local\ntfy-toast-daemon")) {
            // The handle is intentionally leaked: it must live as long as the process.
            Ok(_handle) => GetLastError() != ERROR_ALREADY_EXISTS,
            Err(_) => true,
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
pub fn acquire_single_instance() -> bool {
    true
}
