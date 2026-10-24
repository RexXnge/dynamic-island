// Colour picker: the eyedropper in the top row. While picking, an invisible window covers every
// display so the click doesn't reach the app underneath; it reads the pixel under the cursor and
// the island shows a live swatch and hex code. Click copies the hex, Esc or right-click cancels.
// Both windows are excluded from screen capture while picking, so neither ends up in the sample.
use super::*;
use std::sync::atomic::{AtomicIsize, Ordering};

pub const WM_PICK: u32 = 0x800F; // wparam: PICK_MOVE / PICK_DONE / PICK_CANCEL, lparam: COLORREF
const PICK_MOVE: usize = 0;
const PICK_DONE: usize = 1;
const PICK_CANCEL: usize = 2;

static ISLAND: AtomicIsize = AtomicIsize::new(0);
static OVERLAY: AtomicIsize = AtomicIsize::new(0);

/// Hex code for a COLORREF (0x00BBGGRR).
fn hex(c: u32) -> String {
    format!("#{:02X}{:02X}{:02X}", c & 0xff, (c >> 8) & 0xff, (c >> 16) & 0xff)
}

fn rgb(c: u32) -> [f32; 4] {
    [(c & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, ((c >> 16) & 0xff) as f32 / 255.0, 1.0]
}

/// Pixel under the cursor, or None off-screen.
unsafe fn sample() -> Option<u32> {
    let mut pt = POINT::default();
    GetCursorPos(&mut pt).ok()?;
    let dc = GetDC(None);
    let c = GetPixel(dc, pt.x, pt.y);
    ReleaseDC(None, dc);
    (c.0 != CLR_INVALID).then_some(c.0)
}

unsafe fn finish(kind: usize, color: u32) {
    let overlay = HWND(OVERLAY.swap(0, Ordering::Relaxed) as _);
    if overlay.is_invalid() {
        return; // already finished (a cancel can follow a click on the way out)
    }
    let island = HWND(ISLAND.load(Ordering::Relaxed) as _);
    let _ = SetWindowDisplayAffinity(island, WDA_NONE);
    let _ = DestroyWindow(overlay);
    let _ = PostMessageW(island, WM_PICK, WPARAM(kind), LPARAM(color as isize));
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let island = HWND(ISLAND.load(Ordering::Relaxed) as _);
    match msg {
        WM_MOUSEMOVE => {
            if let Some(c) = sample() {
                let _ = PostMessageW(island, WM_PICK, WPARAM(PICK_MOVE), LPARAM(c as isize));
            }
        }
        WM_LBUTTONDOWN => match sample() {
            Some(c) => finish(PICK_DONE, c),
            None => finish(PICK_CANCEL, 0),
        },
        WM_RBUTTONDOWN => finish(PICK_CANCEL, 0),
        WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => finish(PICK_CANCEL, 0),
        // Alt-Tab or anything else taking focus ends picking rather than leaving a screen-wide window up.
        WM_ACTIVATE if wp.0 & 0xffff == WA_INACTIVE as usize => finish(PICK_CANCEL, 0),
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

/// Covers all displays with the near-invisible picking window.
unsafe fn start(island: HWND) -> Result<()> {
    ISLAND.store(island.0 as isize, Ordering::Relaxed);
    let inst: HINSTANCE = GetModuleHandleW(None)?.into();
    let class = w!("DynamicIslandPicker");
    // Registering twice fails harmlessly; the class from the first pick is reused.
    RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(overlay_proc), hInstance: inst, lpszClassName: class, hCursor: LoadCursorW(None, IDC_CROSS)?, ..Default::default() });
    let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
    let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
    let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW, class, w!("Pick a colour"), WS_POPUP, x, y, w, h, None, None, inst, None)?;
    // Alpha 1 still receives the mouse but is invisible; excluded from capture so it isn't sampled.
    SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA)?;
    let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    let _ = SetWindowDisplayAffinity(island, WDA_EXCLUDEFROMCAPTURE);
    OVERLAY.store(hwnd.0 as isize, Ordering::Relaxed);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd); // for Esc
    Ok(())
}

impl App {
    /// Eyedropper clicked: the island folds into the live colour pill and picking starts.
    pub fn start_pick(&mut self) {
        if let Err(e) = unsafe { start(self.hwnd) } {
            eprintln!("colour picker failed: {e:?}");
            return;
        }
        self.expand.to = 0.0;
        self.note = Note::Pick;
        self.shot = None;
        self.link = None;
        self.swatch = unsafe { sample() }.map(rgb);
        self.clip_text = unsafe { sample() }.map_or_else(String::new, hex);
        if self.clip.to < 0.5 {
            self.check = Spring { x: -0.35, v: 0.0, to: 1.0 };
        }
        self.clip.to = 1.0;
        self.vol.to = 0.0;
        unsafe {
            let _ = KillTimer(self.hwnd, T_CLIP); // stays up until a pick or cancel
        }
    }

    pub fn on_pick(&mut self, kind: usize, color: u32) {
        match kind {
            PICK_MOVE if self.note == Note::Pick => {
                self.swatch = Some(rgb(color));
                self.clip_text = hex(color);
            }
            PICK_DONE => {
                let h = hex(color);
                let _ = unsafe { clips::set_clipboard_text(self.hwnd, &h) };
                self.record_clip(h.clone()); // our own copy skips the clipboard watcher
                self.note = Note::Copy;
                self.swatch = Some(rgb(color));
                self.clip_text = h;
                unsafe { SetTimer(self.hwnd, T_CLIP, CLIP_MS, None) };
            }
            PICK_CANCEL => self.clip.to = 0.0,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref() {
        assert_eq!(hex(0x00FF8000), "#0080FF"); // COLORREF is BGR
        assert_eq!(rgb(0x000000FF), [1.0, 0.0, 0.0, 1.0]);
    }
}
