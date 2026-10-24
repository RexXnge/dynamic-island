// Windows integration: hide while a fullscreen app (game, video) covers the island's display,
// and start with Windows through the per-user Run key.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};

pub const WM_FULLSCREEN: u32 = 0x800E; // wparam = a fullscreen app now covers the island's display
const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const NAME: PCWSTR = w!("DynamicIsland");

static ISLAND: AtomicIsize = AtomicIsize::new(0);
static LAST: AtomicBool = AtomicBool::new(false);

/// Is the foreground window a captionless window covering the whole display the island is on?
pub fn fullscreen_now(island: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == island {
            return false;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(fg, &mut cls) as usize;
        if matches!(String::from_utf16_lossy(&cls[..n]).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return false; // the desktop itself spans the display
        }
        // Maximized windows keep their title bar; fullscreen games and videos don't.
        if GetWindowLongW(fg, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0 {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL);
        if mon.is_invalid() || mon != MonitorFromWindow(island, MONITOR_DEFAULTTONEAREST) {
            return false;
        }
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut r = RECT::default();
        if !GetMonitorInfoW(mon, &mut mi).as_bool() || GetWindowRect(fg, &mut r).is_err() {
            return false;
        }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// Foreground changes and the foreground window resizing (a video going fullscreen) re-check;
/// the island only hears about it when the answer changes.
unsafe extern "system" fn on_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, _: i32, _: u32, _: u32) {
    if event == EVENT_OBJECT_LOCATIONCHANGE && (object != OBJID_WINDOW.0 || hwnd != GetForegroundWindow()) {
        return; // the cursor and every other window move constantly
    }
    let island = HWND(ISLAND.load(Ordering::Relaxed) as _);
    let fs = fullscreen_now(island);
    if LAST.swap(fs, Ordering::Relaxed) != fs {
        let _ = PostMessageW(island, WM_FULLSCREEN, WPARAM(fs as usize), LPARAM(0));
    }
}

/// Hooks run on this thread's message loop; no polling.
pub unsafe fn watch_fullscreen(island: HWND) {
    ISLAND.store(island.0 as isize, Ordering::Relaxed);
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    for e in [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_LOCATIONCHANGE] {
        SetWinEventHook(e, e, None, Some(on_event), 0, 0, flags);
    }
}

pub fn startup_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, None, None).is_ok() }
}

/// Adds or removes this exe in the per-user Run key (no admin needed).
pub fn set_startup(on: bool) -> Result<()> {
    unsafe {
        if !on {
            return RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME).ok();
        }
        let exe = std::env::current_exe().map_err(|_| Error::from(E_FAIL))?;
        let v: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
        RegSetKeyValueW(HKEY_CURRENT_USER, RUN, NAME, REG_SZ.0, Some(v.as_ptr() as _), (v.len() * 2) as u32).ok()
    }
}

impl App {
    pub fn on_fullscreen(&mut self, fs: bool) {
        let hide = fs && self.cfg.hide_fs;
        if hide == self.hidden {
            return;
        }
        self.hidden = hide;
        // Hiding animates first; the frame loop hides the window once the island has shrunk away.
        self.shown.to = if hide { 0.0 } else { 1.0 };
        if !hide {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        self.last = Instant::now(); // don't jump animations on the way back
    }
}

