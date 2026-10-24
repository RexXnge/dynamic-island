// Per-user installer for Dynamic Island: no admin rights, nothing outside the user's profile.
// Installs to %LOCALAPPDATA%\Programs\Dynamic Island, adds a Start menu shortcut and an entry in
// Settings > Apps. The installed copy of this exe is the uninstaller (`/uninstall`).
// `/S` skips the dialogs (for scripted installs and tests).
#![windows_subsystem = "windows"]

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::core::{w, Interface, HSTRING, PCWSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegDeleteTreeW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO};

static APP: &[u8] = include_bytes!("../../target/release/dynamic-island.exe");
const EXE: &str = "dynamic-island.exe";
const NAME: &str = "Dynamic Island";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const UNINSTALL_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\DynamicIsland");
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).map(|a| a.to_lowercase()).collect();
    let quiet = args.iter().any(|a| a == "/s");
    let ask = |text: &str| quiet || unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(NAME), MB_YESNO | MB_ICONQUESTION) } == IDYES;
    let tell = |text: &str, err: bool| {
        if !quiet || err {
            let icon = if err { MB_ICONERROR } else { MB_ICONINFORMATION };
            unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(NAME), MB_OK | icon) };
        }
    };
    if args.iter().any(|a| a == "/uninstall") {
        if ask("Uninstall Dynamic Island?\n\nYour settings in %APPDATA%\\DynamicIsland are kept.") {
            uninstall();
        }
        return;
    }
    if !ask(&format!("Install Dynamic Island {VERSION}?\n\nIt installs for your user only and starts right away.")) {
        return;
    }
    match install() {
        Ok(dir) => tell(&format!("Dynamic Island is installed and running.\n\n{}\n\nFind it in the Start menu; uninstall it from Settings > Apps.", dir.display()), false),
        Err(e) => tell(&format!("Install failed:\n{e}"), true),
    }
}

fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Programs").join(NAME)
}

fn shortcut_path() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Microsoft\\Windows\\Start Menu\\Programs").join(format!("{NAME}.lnk"))
}

/// A running island locks its exe; stop it before replacing or removing it.
fn stop_running() {
    let _ = Command::new("taskkill").args(["/F", "/IM", EXE]).creation_flags(CREATE_NO_WINDOW).status();
    std::thread::sleep(std::time::Duration::from_millis(300));
}

fn install() -> Result<PathBuf, String> {
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    stop_running();
    let exe = dir.join(EXE);
    std::fs::write(&exe, APP).map_err(|e| format!("write {}: {e}", exe.display()))?;
    let uninstaller = dir.join("uninstall.exe");
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    if me != uninstaller {
        std::fs::copy(&me, &uninstaller).map_err(|e| format!("write {}: {e}", uninstaller.display()))?;
    }
    unsafe { make_shortcut(&exe, &shortcut_path()) }.map_err(|e| format!("Start menu shortcut: {e}"))?;
    register(&dir, &exe, &uninstaller);
    Command::new(&exe).current_dir(&dir).spawn().map_err(|e| format!("start: {e}"))?;
    Ok(dir)
}

unsafe fn make_shortcut(target: &Path, lnk: &Path) -> windows::core::Result<()> {
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    link.SetPath(&HSTRING::from(target.as_os_str()))?;
    link.SetWorkingDirectory(&HSTRING::from(target.parent().unwrap_or(target).as_os_str()))?;
    link.SetDescription(&HSTRING::from("Apple-style Dynamic Island for Windows"))?;
    link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk.as_os_str()), true)
}

fn set_str(name: &str, value: &str) {
    let v: Vec<u16> = value.encode_utf16().chain([0]).collect();
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, UNINSTALL_KEY, &HSTRING::from(name), REG_SZ.0, Some(v.as_ptr() as _), (v.len() * 2) as u32);
    }
}

fn set_dword(name: &str, value: u32) {
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, UNINSTALL_KEY, &HSTRING::from(name), REG_DWORD.0, Some(&value as *const u32 as _), 4);
    }
}

/// Entry in Settings > Apps > Installed apps.
fn register(dir: &Path, exe: &Path, uninstaller: &Path) {
    set_str("DisplayName", NAME);
    set_str("DisplayVersion", VERSION);
    set_str("Publisher", "RexXnge");
    set_str("DisplayIcon", &exe.display().to_string());
    set_str("InstallLocation", &dir.display().to_string());
    set_str("UninstallString", &format!("\"{}\" /uninstall", uninstaller.display()));
    set_str("QuietUninstallString", &format!("\"{}\" /uninstall /S", uninstaller.display()));
    set_str("URLInfoAbout", "https://github.com/RexXnge/dynamic-island");
    set_dword("NoModify", 1);
    set_dword("NoRepair", 1);
    set_dword("EstimatedSize", (APP.len() / 1024) as u32 * 2);
}

fn uninstall() {
    stop_running();
    unsafe {
        // The app's own "Start with Windows" entry.
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"), w!("DynamicIsland"));
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, UNINSTALL_KEY);
    }
    let _ = std::fs::remove_file(shortcut_path());
    let dir = install_dir();
    let _ = std::fs::remove_file(dir.join(EXE));
    // This exe lives in the folder: a detached cmd removes the folder once this process exits.
    let _ = Command::new("cmd")
        .raw_arg(format!("/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display()))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
