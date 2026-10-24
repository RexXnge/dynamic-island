// Control Center, Apple-style: scroll down on the island to open it, scroll up (or move away)
// to close. A tile of round toggles (Wi-Fi, Bluetooth, microphone, sound), tall brightness and
// volume sliders, and a wide Dark mode tile. Everything applies live.
use super::*;
use windows::Devices::Radios::{Radio, RadioKind, RadioState};

pub const CONTROL: (f32, f32, f32) = (340.0, 212.0, 36.0);
pub const WM_RADIO: u32 = 0x8012; // wparam bits: 1 Wi-Fi present, 2 on, 4 Bluetooth present, 8 on
const PAD: f32 = 14.0;
const TILE_H: f32 = 124.0;
const B_CC: usize = 40; // press-scale ids 40..=47, above the welcome buttons
const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

#[derive(Default)]
pub struct Cc {
    wifi: Option<bool>, // None: no such radio (or not read yet)
    bt: Option<bool>,
    dark: bool,
    pub drag: Option<usize>, // slider held: 0 brightness, 1 volume
    on: [Spring; 5],     // toggle fades: Wi-Fi, Bluetooth, mic, sound, dark
    lv: [Spring; 2],     // slider fills: brightness, volume
    grip: [Spring; 2],   // slider swell while held
}

/// Reads the radios (and first switches one) on a worker thread; the answer comes as WM_RADIO.
fn radios(hwnd: HWND, set: Option<(RadioKind, bool)>) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let bits = (|| -> Result<usize> {
            let _ = Radio::RequestAccessAsync()?.get();
            let mut bits = 0;
            for r in Radio::GetRadiosAsync()?.get()? {
                let kind = r.Kind()?;
                if let Some((_, on)) = set.filter(|(k, _)| *k == kind) {
                    let _ = r.SetStateAsync(if on { RadioState::On } else { RadioState::Off })?.get();
                }
                let on = (r.State()? == RadioState::On) as usize;
                if kind == RadioKind::WiFi {
                    bits |= 1 | on << 1;
                } else if kind == RadioKind::Bluetooth {
                    bits |= 4 | on << 3;
                }
            }
            Ok(bits)
        })()
        .unwrap_or(0);
        let _ = PostMessageW(HWND(hwnd as _), WM_RADIO, WPARAM(bits), LPARAM(0));
    });
}

/// Windows' app theme is dark ("AppsUseLightTheme" = 0).
fn dark_now() -> bool {
    let (mut v, mut n) = (1u32, 4u32);
    unsafe {
        let _ = RegGetValueW(HKEY_CURRENT_USER, PERSONALIZE, w!("AppsUseLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as _), Some(&mut n));
    }
    v == 0
}

/// Switches apps and the taskbar together, then tells open windows to restyle (off-thread:
/// a hung window would otherwise stall the island).
fn set_dark(on: bool) {
    let v = (!on) as u32;
    unsafe {
        for name in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, PERSONALIZE, name, REG_DWORD.0, Some(&v as *const u32 as _), 4);
        }
    }
    std::thread::spawn(|| unsafe {
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(w!("ImmersiveColorSet").as_ptr() as isize), SMTO_ABORTIFHUNG, 300, None);
    });
}

/// Where everything sits: toggle centres, slider rects, the Dark mode tile.
fn spots(l: &Layout) -> ([(f32, f32); 4], [D2D_RECT_F; 2], D2D_RECT_F) {
    let (x0, y0) = (l.x + PAD, l.y + PAD);
    let tw = (l.w - PAD * 3.0) / 2.0; // left column (toggle tile) and right column (sliders)
    let (c0, c1) = (x0 + tw * 0.26, x0 + tw * 0.74);
    let toggles = [(c0, y0 + 34.0), (c1, y0 + 34.0), (c0, y0 + 90.0), (c1, y0 + 90.0)];
    let sx = x0 + tw + PAD;
    let sw = (tw - PAD) / 2.0;
    let sliders = [rect(sx, y0, sw, TILE_H), rect(sx + sw + PAD, y0, sw, TILE_H)];
    let ty = y0 + TILE_H + 12.0;
    (toggles, sliders, rect(x0, ty, l.w - PAD * 2.0, 48.0))
}

fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

impl App {
    pub fn open_control(&mut self) {
        self.cc.dark = dark_now();
        radios(self.hwnd, None);
        self.set_view(View::Control);
        self.expand.to = 1.0;
    }

    pub fn on_radio(&mut self, bits: usize) {
        self.cc.wifi = (bits & 1 != 0).then_some(bits & 2 != 0);
        self.cc.bt = (bits & 4 != 0).then_some(bits & 8 != 0);
    }

    pub fn control_open(&self) -> bool {
        self.view == View::Control && self.expand.to > 0.5
    }

    fn cc_targets(&self) -> ([f32; 5], [f32; 2]) {
        let b = |x: bool| x as u8 as f32;
        let on = [b(self.cc.wifi == Some(true)), b(self.cc.bt == Some(true)), b(!self.mic_muted), b(!self.muted), b(self.cc.dark)];
        (on, [self.bright_level.unwrap_or(0.0), if self.muted { 0.0 } else { self.vol_level }])
    }

    pub fn control_busy(&self) -> bool {
        if self.view != View::Control {
            return false;
        }
        let (on, lv) = self.cc_targets();
        let grip = [0, 1].map(|i| (self.cc.drag == Some(i)) as u8 as f32);
        let pairs = self.cc.on.iter().zip(on).chain(self.cc.lv.iter().zip(lv)).chain(self.cc.grip.iter().zip(grip));
        pairs.into_iter().any(|(s, t)| !s.settled() || s.to != t)
    }

    pub fn control_step(&mut self, dt: f32) {
        let (on, lv) = self.cc_targets();
        for (s, t) in self.cc.on.iter_mut().zip(on) {
            s.to = t;
            s.step(dt, 0.3, 1.0);
        }
        for (i, s) in self.cc.grip.iter_mut().enumerate() {
            s.to = (self.cc.drag == Some(i)) as u8 as f32;
            s.step(dt, 0.25, 1.0);
        }
        let held = self.cc.drag;
        for (i, (s, t)) in self.cc.lv.iter_mut().zip(lv).enumerate() {
            s.to = t;
            // Follows the pointer tightly while dragged, eases when changed from elsewhere.
            s.step(dt, if held == Some(i) { 0.08 } else { 0.3 }, 1.0);
        }
    }

    fn press_id(&mut self, id: usize) {
        self.pressed = Some(B_CC + id);
        self.press = Spring { x: 1.0, v: 0.0, to: 0.0 };
    }

    pub fn toggle_mic(&mut self) {
        self.mic_muted = !self.mic_muted; // optimistic; the endpoint callback confirms
        if let Some((m, _)) = &self.mic {
            let _ = unsafe { m.SetMute(self.mic_muted, std::ptr::null()) };
        }
    }

    pub fn control_click(&mut self, x: f32, y: f32) {
        let (toggles, sliders, tile) = spots(&self.layout());
        if let Some(i) = toggles.iter().position(|&(cx, cy)| (x - cx).hypot(y - cy) < 23.0) {
            self.press_id(i);
            match i {
                0 => {
                    if let Some(on) = self.cc.wifi {
                        self.cc.wifi = Some(!on);
                        radios(self.hwnd, Some((RadioKind::WiFi, !on)));
                    }
                }
                1 => {
                    if let Some(on) = self.cc.bt {
                        self.cc.bt = Some(!on);
                        radios(self.hwnd, Some((RadioKind::Bluetooth, !on)));
                    }
                }
                2 => self.toggle_mic(),
                _ => {
                    self.muted = !self.muted;
                    if let Some((v, _)) = &self.endpoint {
                        let _ = unsafe { v.SetMute(self.muted, std::ptr::null()) };
                    }
                }
            }
            return;
        }
        if let Some(i) = sliders.iter().position(|r| inside(r, x, y)) {
            if i == 0 && self.bright_level.is_none() {
                return; // this monitor doesn't take brightness over DDC/CI
            }
            self.cc.drag = Some(i);
            unsafe { SetCapture(self.hwnd) };
            self.control_move(x, y);
            return;
        }
        if inside(&tile, x, y) {
            self.cc.dark = !self.cc.dark;
            set_dark(self.cc.dark);
        }
    }

    fn set_level(&mut self, i: usize, v: f32) {
        let v = clamp01(v);
        if i == 0 {
            self.bright_level = Some(v);
            let _ = self.ddc.send(v);
        } else {
            self.vol_level = v;
            if let Some((ep, _)) = &self.endpoint {
                unsafe {
                    let _ = ep.SetMasterVolumeLevelScalar(v, std::ptr::null());
                    if self.muted && v > 0.0 {
                        self.muted = false; // dragging up unmutes, like iOS
                        let _ = ep.SetMute(false, std::ptr::null());
                    }
                }
            }
        }
    }

    pub fn control_move(&mut self, _x: f32, y: f32) {
        let Some(i) = self.cc.drag else { return };
        let r = spots(&self.layout()).1[i];
        self.set_level(i, (r.bottom - y) / (r.bottom - r.top));
    }

    pub fn control_release(&mut self) -> bool {
        if self.cc.drag.take().is_none() {
            return false;
        }
        unsafe {
            let _ = ReleaseCapture();
        }
        true
    }

    /// Wheel while open: scrolling up leaves. Returns false when Control Center isn't open, so the
    /// wheel does its usual thing.
    pub fn control_wheel(&mut self, notches: f32) -> bool {
        if !self.control_open() {
            return false;
        }
        if notches > 0.0 {
            // Back to the expanded player, like pushing Control Center away; nothing to show: collapse.
            if self.media.is_some() {
                self.set_view(View::Player);
            } else {
                self.expand.to = 0.0;
            }
        }
        true
    }

    // ---- drawing ----

    pub unsafe fn draw_control(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (toggles, sliders, tile) = spots(l);
        let dy = (1.0 - a) * 8.0; // the panel drops in a touch as it fades in
        let sh = |r: D2D_RECT_F| rect(r.left, r.top - dy, r.right - r.left, r.bottom - r.top);
        let press = |id: usize| if self.pressed == Some(B_CC + id) { 1.0 - 0.12 * self.press.x } else { 1.0 };
        // A toggle's disc: faint when off, solid white when on; its icon flips to the island colour.
        let disc = |cx: f32, cy: f32, r: f32, on: f32, al: f32, icon: &dyn Fn(f32)| {
            let c = D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: r, radiusY: r };
            br.SetColor(&fg(al * lerp(0.12, 1.0, on)));
            dc.FillEllipse(&c, br);
            if on < 1.0 {
                icon(al * (1.0 - on));
            }
            if on > 0.0 {
                settings::swap_fg_bg(|| icon(al * on));
            }
        };

        // Toggle tile.
        let tw = (l.w - PAD * 3.0) / 2.0;
        br.SetColor(&surface(a));
        dc.FillRoundedRectangle(&rrect(l.x + PAD, l.y + PAD - dy, tw, TILE_H, 22.0), br);
        for (i, &(cx, cy)) in toggles.iter().enumerate() {
            let cy = cy - dy;
            let on = clamp01(self.cc.on[i].x);
            let off_limits = (i == 0 && self.cc.wifi.is_none()) || (i == 1 && self.cc.bt.is_none());
            let al = if off_limits { a * 0.35 } else { a };
            let k = press(i);
            disc(cx, cy, 21.0 * k, on, al, &|ia| match i {
                0 => self.wifi_icon(cx, cy, k, ia),
                1 => self.bt_icon(cx, cy, k, ia),
                2 => self.mic_icon(cx, cy, k, ia, 1.0 - on),
                _ => self.speaker_icon(cx, cy, k, ia, 1.0 - on),
            });
        }

        // Sliders: the fill rises from the bottom inside a rounded tile; icon near the bottom.
        for (i, r) in sliders.iter().enumerate() {
            let r = sh(*r);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let off_limits = i == 0 && self.bright_level.is_none();
            let al = if off_limits { a * 0.35 } else { a };
            let held = 3.0 * self.cc.grip[i].x; // swells while held
            let full = rrect(r.left - held, r.top - held, w + 2.0 * held, h + 2.0 * held, 20.0 + held);
            br.SetColor(&surface(al));
            dc.FillRoundedRectangle(&full, br);
            let v = clamp01(self.cc.lv[i].x);
            dc.PushAxisAlignedClip(&rect(r.left - 2.0, r.bottom - h * v, w + 4.0, h * v + 2.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            br.SetColor(&fg(al * 0.92));
            dc.FillRoundedRectangle(&full, br);
            dc.PopAxisAlignedClip();
            let (icx, icy) = ((r.left + r.right) / 2.0, r.bottom - 22.0);
            // Dark icon once the fill has risen past it, light below.
            let covered = clamp01((h * v - 12.0) / 20.0);
            for (col_a, dark) in [(1.0 - covered, false), (covered, true)] {
                if col_a <= 0.0 {
                    continue;
                }
                if i == 0 {
                    self.sun_colored(icx, icy, al * col_a, dark, v);
                } else {
                    self.speaker_colored(icx, icy, al * col_a, dark, clamp01(self.fx[FX_MUTE].x));
                }
            }
        }

        // Dark mode tile.
        let opts = D2D1_DRAW_TEXT_OPTIONS_NONE;
        let mm = DWRITE_MEASURING_MODE_NATURAL;
        let r = sh(tile);
        // No press scale or spin: only the disc fades, like the toggles.
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let cy = (r.top + r.bottom) / 2.0;
        br.SetColor(&surface(a));
        dc.FillRoundedRectangle(&rrect(r.left, r.top, w, h, 16.0), br);
        let on = clamp01(self.cc.on[4].x);
        let dx = r.left + 26.0;
        disc(dx, cy, 15.0, on, a, &|ia| self.dark_icon(dx, cy, 1.0, ia));
        br.SetColor(&fg(a));
        dc.DrawText(&"Dark mode".encode_utf16().collect::<Vec<_>>(), &g.f_title_s, &rect(r.left + 48.0, cy - 16.0, w - 54.0, 18.0), br, opts, mm);
        br.SetColor(&fg(a * 0.55));
        let state = if self.cc.on[4].to > 0.5 { "On" } else { "Off" };
        dc.DrawText(&state.encode_utf16().collect::<Vec<_>>(), &g.f_artist_s, &rect(r.left + 48.0, cy + 1.0, w - 54.0, 15.0), br, opts, mm);
    }

    unsafe fn wifi_icon(&self, cx: f32, cy: f32, k: f32, a: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        let p = |x: f32, y: f32| D2D_POINT_2F { x: cx + x * k, y: cy + y * k };
        br.SetColor(&fg(a));
        dc.FillEllipse(&D2D1_ELLIPSE { point: p(0.0, 6.0), radiusX: 1.8 * k, radiusY: 1.8 * k }, br);
        for r in [5.0, 9.5] {
            // Arcs over the dot, -135° to -45°.
            let mut prev = None;
            for s in 0..=10 {
                let t = (-135.0 + 9.0 * s as f32).to_radians();
                let q = p(r * t.cos(), 6.0 + r * t.sin());
                if let Some(o) = prev {
                    dc.DrawLine(o, q, br, 2.0 * k, caps);
                }
                prev = Some(q);
            }
        }
    }

    unsafe fn bt_icon(&self, cx: f32, cy: f32, k: f32, a: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        let p = |x: f32, y: f32| D2D_POINT_2F { x: cx + x * k, y: cy + y * k };
        br.SetColor(&fg(a));
        let w = 1.8 * k;
        dc.DrawLine(p(0.0, -8.0), p(0.0, 8.0), br, w, caps);
        dc.DrawLine(p(0.0, -8.0), p(4.2, -4.0), br, w, caps);
        dc.DrawLine(p(4.2, -4.0), p(-4.2, 4.0), br, w, caps);
        dc.DrawLine(p(0.0, 8.0), p(4.2, 4.0), br, w, caps);
        dc.DrawLine(p(4.2, 4.0), p(-4.2, -4.0), br, w, caps);
    }

    /// Speaker with a cross growing in as `mute` rises.
    unsafe fn speaker_icon(&self, cx: f32, cy: f32, k: f32, a: f32, mute: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let s = 0.85 * k;
        dc.SetTransform(&mat(s, s, cx - 11.0 * s, cy - 12.0 * s));
        br.SetColor(&fg(a));
        dc.FillGeometry(&g.speaker, br, None);
        if mute < 1.0 {
            br.SetColor(&fg(a * (1.0 - mute)));
            dc.DrawGeometry(&g.waves[0], br, 1.8, &g.caps);
            dc.DrawGeometry(&g.waves[1], br, 1.8, &g.caps);
        }
        if mute > 0.0 {
            let h = 3.0 * (0.4 + 0.6 * mute);
            let pt = |dx: f32, dy: f32| D2D_POINT_2F { x: 17.5 + dx * h, y: 12.0 + dy * h };
            br.SetColor(&fg(a * mute));
            dc.DrawLine(pt(-1.0, -1.0), pt(1.0, 1.0), br, 1.8, &g.caps);
            dc.DrawLine(pt(1.0, -1.0), pt(-1.0, 1.0), br, 1.8, &g.caps);
        }
        dc.SetTransform(&Matrix3x2::identity());
    }

    /// The level icons draw in the text colour; on the slider fill they need the island colour.
    unsafe fn with_dark(&self, dark: bool, f: impl FnOnce()) {
        if dark {
            settings::swap_fg_bg(f);
        } else {
            f();
        }
    }

    unsafe fn sun_colored(&self, cx: f32, cy: f32, a: f32, dark: bool, level: f32) {
        self.with_dark(dark, || self.sun(cx, cy, 0.85, a, level));
    }

    unsafe fn speaker_colored(&self, cx: f32, cy: f32, a: f32, dark: bool, mute: f32) {
        self.with_dark(dark, || self.speaker_icon(cx, cy, 1.0, a, mute));
    }

    /// Microphone: capsule, cradle and stand; a slash grows in as `mute` rises.
    unsafe fn mic_icon(&self, cx: f32, cy: f32, k: f32, a: f32, mute: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        let p = |x: f32, y: f32| D2D_POINT_2F { x: cx + x * k, y: cy + y * k };
        br.SetColor(&fg(a));
        dc.FillRoundedRectangle(&rrect(cx - 3.2 * k, cy - 8.5 * k, 6.4 * k, 11.0 * k, 3.2 * k), br);
        let mut prev = p(-6.0, -1.0);
        for s in 1..=10 {
            let t = std::f32::consts::PI * s as f32 / 10.0;
            let q = p(-6.0 * t.cos(), -1.0 + 6.0 * t.sin());
            dc.DrawLine(prev, q, br, 1.7 * k, caps);
            prev = q;
        }
        dc.DrawLine(p(0.0, 5.0), p(0.0, 8.5), br, 1.7 * k, caps);
        if mute > 0.0 {
            br.SetColor(&fg(a * mute));
            let m = 0.4 + 0.6 * mute;
            dc.DrawLine(p(-7.0 * m, -8.0 * m), p(7.0 * m, 8.0 * m), br, 1.8 * k, caps);
        }
    }

    /// Half-filled circle.
    unsafe fn dark_icon(&self, cx: f32, cy: f32, k: f32, a: f32) {
        let (dc, br, f) = (&self.g.dc, &self.g.brush, &self.g.factory);
        let r = 7.0 * k;
        let c = D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: r, radiusY: r };
        br.SetColor(&fg(a));
        dc.DrawEllipse(&c, br, 1.8 * k, None);
        // Left half as geometry (not a clip) so it follows a rotation transform.
        let (Ok(disc), Ok(half), Ok(path)) = (f.CreateEllipseGeometry(&c), f.CreateRectangleGeometry(&rect(cx - r - 1.0, cy - r - 1.0, r + 1.0, 2.0 * r + 2.0)), f.CreatePathGeometry()) else { return };
        let Ok(sink) = path.Open() else { return };
        if disc.CombineWithGeometry(&half, D2D1_COMBINE_MODE_INTERSECT, None, 0.1, &sink).is_ok() && sink.Close().is_ok() {
            dc.FillGeometry(&path, br, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // reads this PC's radios
    fn radios_live() {
        println!("access {:?}", Radio::RequestAccessAsync().unwrap().get());
        for r in Radio::GetRadiosAsync().unwrap().get().unwrap() {
            println!("{:?} {:?}", r.Kind().unwrap(), r.State().unwrap());
        }
        println!("dark {}", dark_now());
    }
}
