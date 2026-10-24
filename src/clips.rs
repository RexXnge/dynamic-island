// Clipboard history: the last few text copies, listed in an expanded view; clicking one pastes it
// back into the app you're in. Kept in memory only, and only for copies that allow history.
use super::*;
use std::collections::VecDeque;

pub const HIST: usize = 7;
const ROW: f32 = 26.0;
const LIST_Y: f32 = 38.0; // first row's top, below the icon row
const MAX_CHARS: usize = 100_000;

pub struct Clip {
    pub text: String,
    at: Instant,
    swatch: Option<[f32; 4]>,
    link: bool,
}

/// Full clipboard text for history, or None when it's not text or the copying app opted out of
/// clipboard history (call only after `clipboard_preview` allowed the copy at all).
pub unsafe fn clipboard_text(hwnd: HWND) -> Option<String> {
    OpenClipboard(hwnd).ok()?;
    let text = (|| {
        if let Ok(h) = GetClipboardData(RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"))) {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
            let hide = !p.is_null() && *p == 0;
            let _ = GlobalUnlock(HGLOBAL(h.0));
            if hide {
                return None;
            }
        }
        let h = GetClipboardData(13).ok()?; // CF_UNICODETEXT
        let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
        if p.is_null() {
            return None;
        }
        let mut n = 0;
        while n < MAX_CHARS && *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        let _ = GlobalUnlock(HGLOBAL(h.0));
        Some(s)
    })();
    let _ = CloseClipboard();
    text.filter(|t| !t.trim().is_empty())
}

pub unsafe fn set_clipboard_text(hwnd: HWND, text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    OpenClipboard(hwnd)?;
    let r = (|| {
        EmptyClipboard()?;
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
        let p = GlobalLock(mem) as *mut u16;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        let _ = GlobalUnlock(mem);
        SetClipboardData(13, HANDLE(mem.0))?; // the clipboard owns `mem` from here
        Ok(())
    })();
    let _ = CloseClipboard();
    r
}

/// Ctrl+V into whatever window has focus; the island never takes focus, so that's the user's app.
unsafe fn send_paste() {
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() } },
    };
    SendInput(&[key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)], std::mem::size_of::<INPUT>() as i32);
}

fn age(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

impl App {
    pub fn record_clip(&mut self, text: String) {
        self.history.retain(|c| c.text != text);
        self.history.push_front(Clip { swatch: extras::parse_color(&text), link: extras::parse_link(&text).is_some(), text, at: Instant::now() });
        self.history.truncate(HIST);
    }

    pub fn clips_size(&self) -> (f32, f32, f32) {
        let rows = self.history.len().max(1) as f32; // empty list shows one "nothing yet" row
        (EXPANDED.0, LIST_Y + rows * ROW + 12.0, 36.0)
    }

    fn row_at(&self, l: &Layout, x: f32, y: f32) -> Option<usize> {
        let i = ((y - l.y - LIST_Y) / ROW).floor();
        (x > l.x + 10.0 && x < l.x + l.w - 10.0 && i >= 0.0 && (i as usize) < self.history.len()).then_some(i as usize)
    }

    /// Clicking a row pastes it. Returns true when the click belonged to this view.
    pub fn clips_click(&mut self, x: f32, y: f32) -> bool {
        if self.view != View::Clips {
            return false;
        }
        let l = self.layout();
        if let Some(i) = self.row_at(&l, x, y) {
            let clip = self.history.remove(i).unwrap();
            let ok = unsafe { set_clipboard_text(self.hwnd, &clip.text) }.is_ok();
            self.history.push_front(Clip { at: Instant::now(), ..clip });
            self.expand.to = 0.0;
            if ok {
                unsafe { send_paste() };
            }
        }
        true
    }

    pub unsafe fn draw_clips(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let dy = (1.0 - a) * 6.0;
        if self.history.is_empty() {
            br.SetColor(&fg(a * 0.45));
            dc.DrawText(&"Nothing copied yet".encode_utf16().collect::<Vec<_>>(), &g.f_artist_s, &rect(l.x + 24.0, l.y + LIST_Y + dy, l.w - 48.0, ROW), br, opts, mm);
            return;
        }
        let hot = self.hover.then(|| self.row_at(l, self.mouse.0, self.mouse.1)).flatten();
        for (i, c) in self.history.iter().enumerate() {
            let y = l.y + LIST_Y + i as f32 * ROW + dy;
            if hot == Some(i) {
                br.SetColor(&fg(a * 0.1));
                dc.FillRoundedRectangle(&rrect(l.x + 12.0, y + 1.0, l.w - 24.0, ROW - 2.0, 9.0), br);
            }
            let cy = y + ROW / 2.0;
            let tx = l.x + 44.0;
            if let Some([r, gg, b, al]) = c.swatch {
                br.SetColor(&rgba(r, gg, b, al * a));
                dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: l.x + 28.0, y: cy }, radiusX: 6.0, radiusY: 6.0 }, br);
            } else if c.link {
                self.globe(l.x + 28.0, cy, 0.6, a * 0.75);
            } else {
                br.SetColor(&fg(a * 0.35));
                dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: l.x + 28.0, y: cy }, radiusX: 2.5, radiusY: 2.5 }, br);
            }
            // Whole text on one line, whitespace collapsed; the format trims it with an ellipsis.
            let line: String = c.text.split_whitespace().take(60).collect::<Vec<_>>().join(" ");
            br.SetColor(&fg(a));
            dc.DrawText(&line.encode_utf16().collect::<Vec<_>>(), &g.f_row, &rect(tx, y, l.x + l.w - 64.0 - tx, ROW), br, opts, mm);
            br.SetColor(&fg(a * 0.45));
            dc.DrawText(&age(c.at).encode_utf16().collect::<Vec<_>>(), &g.f_time_r, &rect(l.x + l.w - 64.0, y, 40.0, ROW), br, opts, mm);
        }
    }
}

pub type History = VecDeque<Clip>;
