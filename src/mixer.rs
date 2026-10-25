// Sound Mixer: the system volume and one slider per app that's playing on the default output,
// like Windows' volume mixer. Click a name to mute it. Apps are listed when the view opens; only
// ones playing right now, plus System sounds.
use super::*;
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};

const LIST_Y: f32 = 42.0; // first row's top, below the icon row
const ROW: f32 = 32.0;
const MAX_APPS: usize = 6;
const NAME_W: f32 = 136.0; // name column, from the island's left edge
const PCT_W: f32 = 44.0; // percent column at the right

pub struct Row {
    name: String,
    vols: Vec<ISimpleAudioVolume>, // empty: the System row, which drives the output device itself
    level: f32,
    muted: bool,
    sl: Slider,
}

#[derive(Default)]
pub struct Mixer {
    rows: Vec<Row>,
    pub drag: Option<usize>,
}

/// "spotify.exe" -> "Spotify". shortcut: uses the exe name, not its friendly description.
fn pretty(exe: &str) -> String {
    let stem = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let stem = stem.strip_suffix(".exe").or_else(|| stem.strip_suffix(".EXE")).unwrap_or(stem);
    match stem.to_ascii_lowercase().as_str() {
        "msedge" => "Edge".into(),
        "firefox" => "Firefox".into(),
        "wallpaper32" | "wallpaper64" => "Wallpaper Engine".into(),
        _ => {
            let mut c = stem.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        }
    }
}

unsafe fn exe_name(pid: u32) -> Option<String> {
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = [0u16; 260];
    let mut n = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
    let _ = CloseHandle(h);
    ok.then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// Live sessions on the default output, one row per app (an app's sessions share a slider).
unsafe fn app_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return rows };
    let Ok(mgr) = en.GetDefaultAudioEndpoint(eRender, eMultimedia).and_then(|d| d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)) else { return rows };
    let Ok(list) = mgr.GetSessionEnumerator() else { return rows };
    let me = std::process::id();
    for j in 0..list.GetCount().unwrap_or(0) {
        let Ok(s) = list.GetSession(j) else { continue };
        let (Ok(s2), Ok(vol)) = (s.cast::<IAudioSessionControl2>(), s.cast::<ISimpleAudioVolume>()) else { continue };
        let name = if s2.IsSystemSoundsSession() == S_OK {
            "System sounds".to_string()
        } else {
            // Only apps playing right now; idle streams (RGB tools, launchers) would just be clutter.
            if !s.GetState().is_ok_and(|st| st == AudioSessionStateActive) {
                continue;
            }
            let pid = s2.GetProcessId().unwrap_or(0);
            if pid == me || pid == 0 {
                continue;
            }
            match exe_name(pid) {
                Some(exe) => pretty(&exe),
                None => continue,
            }
        };
        if let Some(r) = rows.iter_mut().find(|r| r.name == name) {
            r.vols.push(vol);
            continue;
        }
        let level = vol.GetMasterVolume().unwrap_or(1.0);
        let muted = vol.GetMute().is_ok_and(|m| m.as_bool());
        rows.push(Row { name, vols: vec![vol], level, muted, sl: Slider::new(if muted { 0.0 } else { level }) });
    }
    rows.truncate(MAX_APPS);
    rows
}

impl App {
    pub fn open_mixer(&mut self) {
        let mut rows = vec![Row { name: "System".into(), vols: Vec::new(), level: self.vol_level, muted: self.muted, sl: Slider::new(self.vol_level) }];
        rows.extend(unsafe { app_rows() });
        self.mx = Mixer { rows, drag: None };
        self.set_view(View::Mixer);
    }

    pub fn mixer_open(&self) -> bool {
        self.view == View::Mixer && self.expand.to > 0.5
    }

    pub fn mixer_size(&self) -> (f32, f32, f32) {
        let n = self.mx.rows.len().max(2) as f32; // with no apps, a line saying so takes the second row
        (WIDE, LIST_Y + n * ROW + 10.0, 36.0)
    }

    /// Slider track for row `i`: (left x, length, centre y).
    fn mix_track(l: &Layout, i: usize) -> (f32, f32, f32) {
        let bx = l.x + NAME_W;
        (bx, l.x + l.w - PCT_W - 12.0 - bx, l.y + LIST_Y + i as f32 * ROW + ROW / 2.0)
    }

    fn mix_row_at(&self, l: &Layout, y: f32) -> Option<usize> {
        let i = ((y - l.y - LIST_Y) / ROW).floor();
        (i >= 0.0 && (i as usize) < self.mx.rows.len()).then_some(i as usize)
    }

    pub fn mixer_step(&mut self, dt: f32) {
        // The System row follows the device, so changes from the keyboard or tray show up here too.
        let (lv, mu) = (self.vol_level, self.muted);
        if let Some(r) = self.mx.rows.first_mut().filter(|r| r.vols.is_empty()) {
            (r.level, r.muted) = (lv, mu);
        }
        for r in &mut self.mx.rows {
            r.sl.step(dt, if r.muted { 0.0 } else { r.level });
        }
    }

    pub fn mixer_busy(&self) -> bool {
        self.view == View::Mixer && (self.mx.drag.is_some() || self.mx.rows.iter().any(|r| r.sl.busy()))
    }

    /// Sets row `i`'s volume (and unmutes it when dragged up from zero).
    fn mix_apply(&mut self, i: usize) {
        let r = &mut self.mx.rows[i];
        unsafe {
            if r.vols.is_empty() {
                if let Some((v, _)) = &self.endpoint {
                    let _ = v.SetMasterVolumeLevelScalar(r.level, std::ptr::null());
                    let _ = v.SetMute(r.muted, std::ptr::null());
                }
                self.vol_level = r.level;
                self.muted = r.muted;
            } else {
                for v in &r.vols {
                    let _ = v.SetMasterVolume(r.level, std::ptr::null());
                    let _ = v.SetMute(r.muted, std::ptr::null());
                }
            }
        }
    }

    pub fn mixer_click(&mut self, x: f32, y: f32) -> bool {
        if self.view != View::Mixer {
            return false;
        }
        let l = self.layout();
        let Some(i) = self.mix_row_at(&l, y) else { return true };
        let (bx, len, _) = Self::mix_track(&l, i);
        if x >= bx - 8.0 && x <= bx + len + 8.0 {
            self.mx.drag = Some(i);
            self.mx.rows[i].sl.drag = true;
            unsafe { SetCapture(self.hwnd) };
            self.mixer_move(x, y);
        } else if x < bx {
            let r = &mut self.mx.rows[i];
            r.muted = !r.muted;
            self.mix_apply(i);
        }
        true
    }

    pub fn mixer_move(&mut self, x: f32, y: f32) {
        if self.view != View::Mixer {
            return;
        }
        let l = self.layout();
        match self.mx.drag {
            Some(i) => {
                let (bx, len, _) = Self::mix_track(&l, i);
                let r = &mut self.mx.rows[i];
                r.sl.follow(x, bx, len);
                r.level = r.sl.p.to;
                r.muted = false;
                self.mix_apply(i);
            }
            None => {
                let hot = self.mix_row_at(&l, y).filter(|&i| {
                    let (bx, len, _) = Self::mix_track(&l, i);
                    x >= bx - 8.0 && x <= bx + len + 8.0
                });
                for (i, r) in self.mx.rows.iter_mut().enumerate() {
                    r.sl.hover = hot == Some(i);
                }
            }
        }
    }

    pub fn mixer_release(&mut self) -> bool {
        let Some(i) = self.mx.drag.take() else { return false };
        let r = &mut self.mx.rows[i];
        r.sl.drag = false;
        r.sl.held = false;
        unsafe {
            let _ = ReleaseCapture();
        }
        true
    }

    pub unsafe fn draw_mixer(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let text = |s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F| dc.DrawText(&s.encode_utf16().collect::<Vec<_>>(), f, &r, br, opts, mm);
        for (i, r) in self.mx.rows.iter().enumerate() {
            let (bx, len, cy) = Self::mix_track(l, i);
            let on = if r.muted { 0.45 } else { 1.0 };
            br.SetColor(&fg(a * 0.9 * on));
            text(&r.name, &g.f_row, rect(l.x + 22.0, cy - 10.0, NAME_W - 30.0, 20.0));
            r.sl.draw(dc, br, bx, len, cy, a * (0.7 + 0.3 * r.sl.a.x) * on, 4.0);
            br.SetColor(&fg(a * 0.6 * on));
            let pct = if r.muted { "Muted".to_string() } else { format!("{}%", (clamp01(r.sl.p.x) * 100.0).round()) };
            text(&pct, &g.f_time_r, rect(l.x + l.w - PCT_W - 16.0, cy - 8.0, PCT_W, 16.0));
            if i == 0 && self.mx.rows.len() > 1 {
                br.SetColor(&fg(a * 0.1)); // System apart from the apps
                dc.FillRectangle(&rect(l.x + 22.0, cy + ROW / 2.0 - 0.5, l.w - 44.0, 1.0), br);
            }
        }
        if self.mx.rows.len() == 1 {
            br.SetColor(&fg(a * 0.45));
            text("No apps are playing sound", &g.f_artist_s, rect(l.x + 22.0, l.y + LIST_Y + ROW + ROW / 2.0 - 8.0, l.w - 44.0, 16.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(pretty(r"C:\Program Files\Spotify\Spotify.exe"), "Spotify");
        assert_eq!(pretty(r"C:\x\chrome.exe"), "Chrome");
        assert_eq!(pretty(r"C:\x\msedge.exe"), "Edge");
        assert_eq!(pretty(r"C:\x\wallpaper32.exe"), "Wallpaper Engine");
    }
}
