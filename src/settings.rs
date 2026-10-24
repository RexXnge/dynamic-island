// Settings view behind the gear: six pages of toggles, segmented pickers, sliders and a drag-to-rank
// list. Every change applies live and is saved to disk (see config.rs).
use super::*;
use crate::config::*;

pub const SETTINGS: (f32, f32, f32) = (340.0, 266.0, 36.0);
const TABS: [&str; 6] = ["Look", "Theme", "Motion", "Pills", "Priority", "Music"];
const TAB_Y: f32 = 46.0; // tab bar centre, from the island top
const ROWS_Y: f32 = 64.0; // first row's top
const ROW: f32 = 24.0;
const RANK_ROW: f32 = 20.0;

pub enum Drag {
    Slider(usize),  // 0 corners, 1 top gap, 2 speed, 3 bounce
    Rank(u8, f32),  // kind being moved, pointer offset inside its row
    Color(usize),   // custom theme strip: colour * 2 + (0 hue, 1 lightness)
}

pub struct SetUi {
    page: usize,
    tab: Spring,
    tog: [Spring; 12],
    seg: [Spring; 4], // size, display, feel, music mode (x = selected index)
    sl: [Slider; 4],
    pub drag: Option<Drag>,
    ry: [Spring; 7], // priority row position per kind, in rows
    pv: Spring,      // motion preview pill, 0 narrow .. 1 wide
    pub mons: Vec<HMONITOR>,
    th: Spring,       // theme picker ring position
    reset_armed: bool, // first click on Reset asks for a second
    gen: [Spring; 3],  // Look page switches: hide in fullscreen, start with Windows, mic dot
    pub startup: bool, // read from the registry when settings open
}

impl SetUi {
    pub fn new(c: &Config) -> Self {
        let mut ry = [spring(0.0); 7];
        for (i, &k) in c.rank.iter().enumerate() {
            ry[k as usize] = spring(i as f32);
        }
        SetUi {
            page: 0,
            tab: spring(0.0),
            tog: [spring(0.0); 12],
            seg: [spring(0.0); 4],
            sl: [Slider::new(0.0), Slider::new(0.0), Slider::new(0.0), Slider::new(0.0)],
            drag: None,
            ry,
            pv: spring(0.0),
            mons: Vec::new(),
            th: spring(c.theme as f32),
            reset_armed: false,
            gen: [spring(c.hide_fs as u8 as f32), spring(0.0), spring(c.mic_dot as u8 as f32)],
            startup: false,
        }
    }
}

/// Main display first, then the others in Windows' order.
pub fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn add(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        (*(lp.0 as *mut Vec<HMONITOR>)).push(m);
        TRUE
    }
    let mut v: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(HDC::default(), None, Some(add), LPARAM(&mut v as *mut _ as isize));
        let main = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        v.sort_by_key(|&m| m != main);
    }
    v
}

fn display_labels(n: usize) -> Vec<String> {
    let mut v = vec!["Main".to_string(), "Mouse".to_string()];
    v.extend((2..=n).map(|i| format!("#{i}")));
    v
}

/// Display setting <-> segment index: Main = 0, Mouse = 1, other displays from 2.
fn display_seg(d: i32) -> usize {
    match d {
        -1 => 1,
        0 => 0,
        n => n as usize + 1,
    }
}

impl App {
    fn slider_value(&self, i: usize) -> f32 {
        let c = &self.cfg;
        match i {
            0 => (c.round - 0.3) / 0.7,
            1 => c.top / 40.0,
            2 => (c.speed - 0.5) / 1.1,
            _ => c.bounce,
        }
    }

    fn set_slider_value(&mut self, i: usize, v: f32) {
        let v = clamp01(v);
        let c = &mut self.cfg;
        match i {
            0 => c.round = 0.3 + 0.7 * v,
            1 => c.top = (40.0 * v).round(),
            2 => c.speed = 0.5 + 1.1 * v,
            _ => c.bounce = v,
        }
    }

    pub fn settings_step(&mut self, dt: f32) {
        let c = self.cfg.clone();
        let u = &mut self.su;
        u.tab.to = u.page as f32;
        u.tab.step(dt, 0.35, 0.8);
        for (i, t) in u.tog.iter_mut().enumerate() {
            t.to = c.pills[i] as u8 as f32;
            t.step(dt, 0.3, 0.75);
        }
        let targets = [c.size as f32, display_seg(c.display) as f32, c.feel().map_or(-1.0, |f| f as f32), c.music as f32];
        for (s, to) in u.seg.iter_mut().zip(targets) {
            s.to = to;
            s.step(dt, 0.35, 0.8);
        }
        let held = match u.drag {
            Some(Drag::Rank(k, _)) => Some(k),
            _ => None,
        };
        for (i, &k) in c.rank.iter().enumerate() {
            let s = &mut u.ry[k as usize];
            s.to = i as f32;
            if held != Some(k) {
                s.step(dt, 0.3, 0.85);
            }
        }
        u.pv.step(dt, 0.42, 0.72);
        u.th.to = c.theme as f32;
        u.th.step(dt, 0.35, 0.8);
        for (s, on) in u.gen.iter_mut().zip([c.hide_fs, u.startup, c.mic_dot]) {
            s.to = on as u8 as f32;
            s.step(dt, 0.3, 0.75);
        }
        let vals = [0, 1, 2, 3].map(|i| self.slider_value(i));
        for (s, v) in self.su.sl.iter_mut().zip(vals) {
            s.step(dt, v);
        }
    }

    pub fn settings_busy(&self) -> bool {
        let u = &self.su;
        u.drag.is_some()
            || u.sl.iter().any(Slider::busy)
            || [u.tab, u.pv, u.th].iter().chain(&u.gen).chain(&u.tog).chain(&u.seg).chain(&u.ry).any(|s| !s.settled())
    }

    /// Applies what changed since `old`, then saves.
    fn apply(&mut self, old: Config) {
        let c = self.cfg.clone();
        set_feel(c.speed, c.bounce);
        set_theme(c.colors());
        if c.hide_fs != old.hide_fs {
            self.on_fullscreen(system::fullscreen_now(self.hwnd));
        }
        if c.size != old.size {
            let _ = unsafe { self.resize(self.sys_dpi) };
        }
        if c.display != old.display || c.top != old.top {
            self.reposition();
        }
        if c.music != old.music || c.apps != old.apps || c.pills[P_MUSIC] != old.pills[P_MUSIC] {
            *self.filter.lock().unwrap() = (c.music, c.apps.clone());
            let _ = self.tx.send(Cmd::Refresh);
            self.on_media(); // applies the Music switch right away
        }
        if !c.pills[P_TIMER] && self.timer.is_some() {
            self.cancel_timer();
        }
        if c.rank != old.rank {
            self.presence();
        }
        if c.pills[P_LYRICS] != old.pills[P_LYRICS] {
            self.want_lyrics();
        }
        if !matches!(self.su.drag, Some(Drag::Slider(_) | Drag::Color(_))) {
            c.save(); // drags save once, on release
        }
    }

    // ---- geometry ----

    fn tab_w(l: &Layout) -> f32 {
        (l.w - 32.0) / TABS.len() as f32
    }

    fn controls(l: &Layout) -> (f32, f32) {
        (l.x + 116.0, l.x + l.w - 22.0)
    }

    fn row_cy(l: &Layout, i: usize) -> f32 {
        l.y + ROWS_Y + i as f32 * ROW + ROW / 2.0
    }

    fn slider_track(l: &Layout, row: usize) -> (f32, f32, f32) {
        let (x0, x1) = Self::controls(l);
        (x0 + 4.0, x1 - x0 - 8.0, Self::row_cy(l, row))
    }

    /// Rows that hold the four sliders: (page, row).
    const SLIDERS: [(usize, usize); 4] = [(0, 1), (0, 2), (2, 1), (2, 2)];

    /// Custom theme strip for `color` (row 1..3 on the Theme page): hue on the left half,
    /// lightness on the right. Returns (left, right, centre y).
    fn strip(l: &Layout, color: usize, light: bool) -> (f32, f32, f32) {
        let (x0, x1) = Self::controls(l);
        let mid = (x0 + x1) / 2.0;
        let cy = Self::row_cy(l, color + 1);
        if light { (mid + 8.0, x1 - 4.0, cy) } else { (x0 + 4.0, mid - 8.0, cy) }
    }

    fn reset_button(l: &Layout) -> D2D_RECT_F {
        let cy = Self::row_cy(l, 7);
        rect(l.x + l.w / 2.0 - 80.0, cy - 10.0, 160.0, 20.0)
    }

    // ---- input ----

    pub fn settings_click(&mut self, x: f32, y: f32) -> bool {
        if self.view != View::Settings {
            return false;
        }
        let l = self.layout();
        let old = self.cfg.clone();
        if (y - (l.y + TAB_Y)).abs() < 12.0 {
            let i = ((x - l.x - 16.0) / Self::tab_w(&l)).floor();
            if (0.0..TABS.len() as f32).contains(&i) {
                self.su.page = i as usize;
                self.su.drag = None;
                self.su.reset_armed = false;
            }
            return true;
        }
        let (x0, x1) = Self::controls(&l);
        let seg_at = |n: usize, a: f32, b: f32| -> Option<usize> { (x >= a && x <= b).then(|| (((x - a) / (b - a) * n as f32) as usize).min(n - 1)) };
        let row = ((y - l.y - ROWS_Y) / ROW).floor();
        if row < 0.0 {
            return true;
        }
        let row = row as usize;
        if let Some(i) = Self::SLIDERS.iter().position(|&(p, r)| p == self.su.page && r == row) {
            self.su.drag = Some(Drag::Slider(i));
            self.su.sl[i].drag = true;
            unsafe { SetCapture(self.hwnd) };
            self.settings_move(x, y);
            return true;
        }
        match (self.su.page, row) {
            (0, 0) => {
                if let Some(i) = seg_at(SIZES.len(), x0, x1) {
                    self.cfg.size = i;
                }
            }
            (0, 4) => self.cfg.hide_fs = !self.cfg.hide_fs,
            (0, 5) => {
                let on = !self.su.startup;
                if system::set_startup(on).is_ok() {
                    self.su.startup = on;
                }
            }
            (0, 6) => self.cfg.mic_dot = !self.cfg.mic_dot,
            (0, 7) => {
                let b = Self::reset_button(&l);
                if x >= b.left && x <= b.right {
                    if self.su.reset_armed {
                        self.cfg = self.cfg.reset();
                    }
                    self.su.reset_armed = !self.su.reset_armed;
                }
            }
            (1, 0) => {
                if let Some(i) = seg_at(THEMES.len(), x0, x1) {
                    self.cfg.theme = i;
                }
            }
            (1, 1..=3) => {
                let (x0, x1) = Self::controls(&l);
                if x >= x0 && x <= x1 {
                    let light = x > (x0 + x1) / 2.0;
                    self.su.drag = Some(Drag::Color((row - 1) * 2 + light as usize));
                    unsafe { SetCapture(self.hwnd) };
                    self.settings_move(x, y);
                    return true;
                }
            }
            (0, 3) => {
                if let Some(i) = seg_at(display_labels(self.su.mons.len()).len(), x0, x1) {
                    self.cfg.display = match i {
                        0 => 0,
                        1 => -1,
                        n => n as i32 - 1,
                    };
                }
            }
            (2, 0) => {
                if let Some(i) = seg_at(FEELS.len(), x0, x1) {
                    (self.cfg.speed, self.cfg.bounce) = (FEELS[i].1, FEELS[i].2);
                    self.su.pv.to = 1.0 - self.su.pv.to; // show the new feel
                }
            }
            (2, 3) => self.su.pv.to = 1.0 - self.su.pv.to,
            (3, r) => {
                let i = r + if x > l.x + l.w / 2.0 { 6 } else { 0 };
                if i < PILLS.len() {
                    self.cfg.pills[i] = !self.cfg.pills[i];
                }
            }
            (4, _) => {
                let idx = ((y - l.y - ROWS_Y) / RANK_ROW).floor();
                if (0.0..KINDS.len() as f32).contains(&idx) {
                    let kind = self.cfg.rank[idx as usize];
                    self.su.drag = Some(Drag::Rank(kind, y - (l.y + ROWS_Y + idx * RANK_ROW)));
                    unsafe { SetCapture(self.hwnd) };
                }
            }
            (5, 0) => {
                if let Some(i) = seg_at(MUSIC_MODES.len(), l.x + 20.0, l.x + l.w - 20.0) {
                    self.cfg.music = i;
                }
            }
            (5, r) if self.cfg.music == 3 && r <= 5 && r - 1 < self.cfg.apps.len() => {
                let a = &mut self.cfg.apps[r - 1];
                a.1 = !a.1;
            }
            _ => {}
        }
        if self.cfg != old {
            self.apply(old);
        }
        true
    }

    pub fn settings_move(&mut self, x: f32, y: f32) {
        let l = self.layout();
        let old = self.cfg.clone();
        match self.su.drag {
            Some(Drag::Slider(i)) => {
                let (bx, len, _) = Self::slider_track(&l, Self::SLIDERS[i].1);
                self.su.sl[i].follow(x, bx, len);
                let v = self.su.sl[i].p.to;
                self.set_slider_value(i, v);
            }
            Some(Drag::Rank(kind, grab)) => {
                // Live reorder: the dragged row takes the slot under its centre.
                let to = (((y - grab - l.y - ROWS_Y) / RANK_ROW) + 0.5).floor().clamp(0.0, KINDS.len() as f32 - 1.0) as usize;
                self.su.ry[kind as usize].x = (y - grab - l.y - ROWS_Y) / RANK_ROW;
                let from = self.cfg.rank_of(kind);
                if from != to {
                    let mut r = self.cfg.rank.to_vec();
                    let k = r.remove(from);
                    r.insert(to, k);
                    self.cfg.rank.copy_from_slice(&r);
                }
            }
            Some(Drag::Color(i)) => {
                let (a, b, _) = Self::strip(&l, i / 2, i % 2 == 1);
                self.cfg.custom[i / 2][i % 2] = clamp01((x - a) / (b - a));
                self.cfg.theme = CUSTOM;
            }
            None => return,
        }
        if self.cfg != old {
            self.apply(old);
        }
    }

    pub fn settings_release(&mut self) -> bool {
        let Some(d) = self.su.drag.take() else { return false };
        if let Drag::Slider(i) = d {
            self.su.sl[i].drag = false;
            self.su.sl[i].held = false;
            if i >= 2 {
                self.su.pv.to = 1.0 - self.su.pv.to; // preview the new spring
            }
        }
        self.cfg.save();
        unsafe {
            let _ = ReleaseCapture();
        }
        true
    }

    // ---- drawing ----

    pub unsafe fn draw_settings(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let u = &self.su;
        let text = |s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, al: f32| {
            br.SetColor(&fg(al));
            dc.DrawText(&s.encode_utf16().collect::<Vec<_>>(), f, &r, br, opts, mm);
        };
        // Tabs with a sliding highlight.
        let (tw, ty) = (Self::tab_w(l), l.y + TAB_Y);
        br.SetColor(&fg(a * 0.12));
        dc.FillRoundedRectangle(&rrect(l.x + 16.0 + u.tab.x * tw + 2.0, ty - 11.0, tw - 4.0, 22.0, 11.0), br);
        for (i, t) in TABS.iter().enumerate() {
            let on = 1.0 - (u.tab.x - i as f32).abs().min(1.0);
            text(t, &g.f_seg, rect(l.x + 16.0 + i as f32 * tw, ty - 10.0, tw, 20.0), a * (0.5 + 0.5 * on));
        }

        let (x0, x1) = Self::controls(l);
        let label = |row: usize, s: &str| text(s, &g.f_row, rect(l.x + 24.0, Self::row_cy(l, row) - 10.0, 90.0, 20.0), a * 0.85);
        let toggle = |right: f32, cy: f32, s: f32, al: f32| {
            let (w, h) = (32.0, 19.0);
            let track = rrect(right - w, cy - h / 2.0, w, h, h / 2.0);
            br.SetColor(&fg(al * 0.22));
            dc.FillRoundedRectangle(&track, br);
            let (ar, ag, ab) = accent();
            br.SetColor(&rgba(ar, ag, ab, al * clamp01(s)));
            dc.FillRoundedRectangle(&track, br);
            br.SetColor(&rgba(1.0, 1.0, 1.0, al)); // knob stays white on every theme
            let kx = right - w + h / 2.0 + (w - h) * s;
            dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: kx, y: cy }, radiusX: h / 2.0 - 2.0, radiusY: h / 2.0 - 2.0 }, br);
        };
        let segmented = |a0: f32, a1: f32, cy: f32, labels: &[&str], sel: f32| {
            let n = labels.len() as f32;
            let sw = (a1 - a0) / n;
            br.SetColor(&fg(a * 0.1));
            dc.FillRoundedRectangle(&rrect(a0, cy - 10.0, a1 - a0, 20.0, 10.0), br);
            if sel >= 0.0 {
                br.SetColor(&fg(a * 0.28));
                dc.FillRoundedRectangle(&rrect(a0 + sel * sw + 1.5, cy - 8.5, sw - 3.0, 17.0, 8.5), br);
            }
            for (i, s) in labels.iter().enumerate() {
                let on = if sel >= 0.0 { 1.0 - (sel - i as f32).abs().min(1.0) } else { 0.0 };
                text(s, &g.f_seg, rect(a0 + i as f32 * sw, cy - 9.0, sw, 18.0), a * (0.6 + 0.4 * on));
            }
        };
        let slider = |i: usize, row: usize| {
            let (bx, len, cy) = Self::slider_track(l, row);
            let s = &u.sl[i];
            s.draw(dc, br, bx, len, cy, a * (0.7 + 0.3 * s.a.x), 4.0);
        };

        match u.page {
            0 => {
                label(0, "Size");
                segmented(x0, x1, Self::row_cy(l, 0), &SIZES.map(|s| s.0), u.seg[0].x);
                label(1, "Corners");
                slider(0, 1);
                label(2, "Top gap");
                slider(1, 2);
                label(3, "Display");
                let d = display_labels(u.mons.len());
                segmented(x0, x1, Self::row_cy(l, 3), &d.iter().map(String::as_str).collect::<Vec<_>>(), u.seg[1].x);
                for (row, name) in [(4, "Hide in fullscreen"), (5, "Start with Windows"), (6, "Microphone dot")] {
                    text(name, &g.f_row, rect(l.x + 24.0, Self::row_cy(l, row) - 10.0, 200.0, 20.0), a * 0.85);
                    toggle(x1, Self::row_cy(l, row), u.gen[row - 4].x, a);
                }
                let b = Self::reset_button(l);
                br.SetColor(&fg(a * 0.1));
                dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: b, radiusX: 10.0, radiusY: 10.0 }, br);
                let (msg, al) = if u.reset_armed { ("Click again to reset", a) } else { ("Reset to defaults", a * 0.8) };
                br.SetColor(&rgba(1.0, 0.27, 0.23, al));
                dc.DrawText(&msg.encode_utf16().collect::<Vec<_>>(), &g.f_seg, &b, br, opts, mm);
            }
            1 => self.draw_theme_page(l, a),
            2 => {
                label(0, "Feel");
                segmented(x0, x1, Self::row_cy(l, 0), &FEELS.map(|f| f.0), u.seg[2].x);
                label(1, "Speed");
                slider(2, 1);
                label(2, "Bounce");
                slider(3, 2);
                label(3, "Preview");
                // A pill that springs between two widths with the current feel; click to replay.
                let cy = Self::row_cy(l, 3);
                let w = lerp(40.0, x1 - x0, self.su.pv.x);
                br.SetColor(&fg(a * 0.3));
                dc.FillRoundedRectangle(&rrect(x0, cy - 8.0, w.max(16.0), 16.0, 8.0), br);
            }
            3 => {
                for (i, name) in PILLS.iter().enumerate() {
                    let (col, row) = (i / 6, i % 6);
                    let cx0 = l.x + 24.0 + col as f32 * (l.w / 2.0 - 12.0);
                    let right = cx0 + l.w / 2.0 - 44.0;
                    let cy = Self::row_cy(l, row);
                    text(name, &g.f_row, rect(cx0, cy - 10.0, right - cx0 - 36.0, 20.0), a * 0.85);
                    toggle(right, cy, u.tog[i].x, a);
                }
            }
            4 => {
                let held = match u.drag {
                    Some(Drag::Rank(k, _)) => Some(k),
                    _ => None,
                };
                // Others first so the dragged row draws on top.
                let mut order: Vec<u8> = self.cfg.rank.to_vec();
                order.sort_by_key(|&k| held == Some(k));
                for k in order {
                    let y = l.y + ROWS_Y + u.ry[k as usize].x * RANK_ROW;
                    let cy = y + RANK_ROW / 2.0;
                    if held == Some(k) {
                        br.SetColor(&fg(a * 0.14));
                        dc.FillRoundedRectangle(&rrect(l.x + 16.0, y + 1.0, l.w - 32.0, RANK_ROW - 2.0, 8.0), br);
                    }
                    let place = self.cfg.rank_of(k) + 1;
                    text(&place.to_string(), &g.f_time, rect(l.x + 26.0, cy - 8.0, 20.0, 16.0), a * 0.45);
                    text(KINDS[k as usize], &g.f_row, rect(l.x + 48.0, cy - 10.0, 200.0, 20.0), a);
                    // Grip lines on the right.
                    br.SetColor(&fg(a * 0.4));
                    for dy in [-3.5, 0.0, 3.5] {
                        dc.DrawLine(D2D_POINT_2F { x: x1 - 14.0, y: cy + dy }, D2D_POINT_2F { x: x1, y: cy + dy }, br, 1.5, &g.caps);
                    }
                }
            }
            _ => {
                segmented(l.x + 20.0, l.x + l.w - 20.0, Self::row_cy(l, 0), &MUSIC_MODES, u.seg[3].x);
                let c = &self.cfg;
                if c.apps.is_empty() {
                    text("Players you use will show up here", &g.f_artist_s, rect(l.x + 24.0, Self::row_cy(l, 1) - 9.0, l.w - 48.0, 18.0), a * 0.45);
                }
                for (i, (id, _)) in c.apps.iter().take(5).enumerate() {
                    let cy = Self::row_cy(l, i + 1);
                    let on = music_allows(c.music, &c.apps, id);
                    let al = if c.music == 3 { a } else { a * 0.4 }; // only editable in "Choose"
                    text(&app_name(id), &g.f_row, rect(l.x + 24.0, cy - 10.0, x1 - 60.0 - l.x - 24.0, 20.0), al);
                    toggle(x1, cy, on as u8 as f32, al);
                }
            }
        }
    }
}

impl App {
    /// Theme page: preset swatches, then hue and lightness strips for the custom theme.
    unsafe fn draw_theme_page(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let text = |s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, al: f32| {
            br.SetColor(&fg(al));
            dc.DrawText(&s.encode_utf16().collect::<Vec<_>>(), f, &r, br, opts, mm);
        };
        let label = |row: usize, s: &str| text(s, &g.f_row, rect(l.x + 24.0, Self::row_cy(l, row) - 10.0, 90.0, 20.0), a * 0.85);
        let circle = |x: f32, y: f32, r: f32| D2D1_ELLIPSE { point: D2D_POINT_2F { x, y }, radiusX: r, radiusY: r };
        let (x0, x1) = Self::controls(l);
        let c = &self.cfg;

        label(0, THEMES[c.theme].0);
        let sw = (x1 - x0) / THEMES.len() as f32;
        let cy = Self::row_cy(l, 0);
        for (i, t) in THEMES.iter().enumerate() {
            let [b, _, ac] = if i == CUSTOM { c.custom.map(|[h, lt]| hsl(h, lt)) } else { t.1 };
            let x = x0 + sw * (i as f32 + 0.5);
            br.SetColor(&rgba(b[0], b[1], b[2], a));
            dc.FillEllipse(&circle(x, cy, 8.0), br);
            br.SetColor(&fg(a * 0.35));
            dc.DrawEllipse(&circle(x, cy, 8.0), br, 1.0, None);
            br.SetColor(&rgba(ac[0], ac[1], ac[2], a));
            dc.FillEllipse(&circle(x, cy, 3.2), br);
        }
        br.SetColor(&fg(a));
        dc.DrawEllipse(&circle(x0 + sw * (self.su.th.x + 0.5), cy, 11.0), br, 1.6, None);

        let custom = c.theme == CUSTOM;
        let al = if custom { a } else { a * 0.45 };
        for (i, name) in ["Island", "Text", "Accent"].iter().enumerate() {
            label(i + 1, name);
            let [h, lt] = c.custom[i];
            for light in [false, true] {
                let (s0, s1, scy) = Self::strip(l, i, light);
                let stops: Vec<[f32; 3]> = if light { vec![hsl(h, 0.0), hsl(h, 0.5), hsl(h, 1.0)] } else { (0..=6).map(|k| hsl(k as f32 / 6.0, 0.5)).collect() };
                if let Some(gb) = self.gradient(s0, s1, &stops) {
                    gb.SetOpacity(al);
                    dc.FillRoundedRectangle(&rrect(s0, scy - 4.0, s1 - s0, 8.0, 4.0), &gb);
                }
                let v = if light { lt } else { h };
                let knob = if light { hsl(h, lt) } else { hsl(h, 0.5) };
                let kx = lerp(s0, s1, v);
                br.SetColor(&rgba(knob[0], knob[1], knob[2], al));
                dc.FillEllipse(&circle(kx, scy, 6.5), br);
                br.SetColor(&rgba(1.0, 1.0, 1.0, al));
                dc.DrawEllipse(&circle(kx, scy, 6.5), br, 1.8, None);
            }
        }
        if !custom {
            text("Drag a colour to make your own", &g.f_artist_s, rect(l.x + 24.0, Self::row_cy(l, 4) - 9.0, l.w - 48.0, 18.0), a * 0.45);
        }
    }

    /// Horizontal gradient brush across x0..x1 through evenly spaced `stops`.
    unsafe fn gradient(&self, x0: f32, x1: f32, stops: &[[f32; 3]]) -> Option<ID2D1LinearGradientBrush> {
        let n = (stops.len() - 1) as f32;
        let st: Vec<D2D1_GRADIENT_STOP> = stops.iter().enumerate().map(|(i, c)| D2D1_GRADIENT_STOP { position: i as f32 / n, color: rgba(c[0], c[1], c[2], 1.0) }).collect();
        let rt: &ID2D1RenderTarget = &self.g.dc;
        let coll = rt.CreateGradientStopCollection(&st, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: D2D_POINT_2F { x: x0, y: 0.0 }, endPoint: D2D_POINT_2F { x: x1, y: 0.0 } };
        rt.CreateLinearGradientBrush(&props, None, &coll).ok()
    }
}

thread_local! { static THEME: std::cell::Cell<[[f32; 3]; 3]> = const { std::cell::Cell::new(THEMES[0].1) }; }

/// Current theme colours, read by all drawing: [island, text, accent].
pub fn set_theme(c: [[f32; 3]; 3]) {
    THEME.with(|t| t.set(c));
}

fn theme() -> [[f32; 3]; 3] {
    THEME.with(|t| t.get())
}

/// Text and icon colour at opacity `a`.
pub fn fg(a: f32) -> D2D1_COLOR_F {
    let [_, [r, g, b], _] = theme();
    rgba(r, g, b, a)
}

pub fn fg_rgb() -> (f32, f32, f32) {
    let [_, [r, g, b], _] = theme();
    (r, g, b)
}

/// Island fill colour at opacity `a`; also cuts glyph outlines.
pub fn bg(a: f32) -> D2D1_COLOR_F {
    let [[r, g, b], _, _] = theme();
    rgba(r, g, b, a)
}

/// Raised surface (buttons, wheel band): the island colour lifted toward the text colour.
pub fn surface_rgb() -> (f32, f32, f32) {
    let [b, f, _] = theme();
    (lerp(b[0], f[0], 0.18), lerp(b[1], f[1], 0.18), lerp(b[2], f[2], 0.18))
}

pub fn surface(a: f32) -> D2D1_COLOR_F {
    let (r, g, b) = surface_rgb();
    rgba(r, g, b, a)
}

pub fn accent() -> (f32, f32, f32) {
    let [_, _, [r, g, b]] = theme();
    (r, g, b)
}

thread_local! { static FEEL: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((1.0, 0.5)) }; }

/// Global animation feel, read by every spring: (speed, bounce).
pub fn set_feel(speed: f32, bounce: f32) {
    FEEL.with(|f| f.set((speed, bounce)));
}

/// A spring's hand-tuned (response, damping) adjusted by the feel. Fades (damping >= 1) never bounce.
pub fn feel(response: f32, damping: f32) -> (f32, f32) {
    let (speed, bounce) = FEEL.with(|f| f.get());
    let d = if damping >= 1.0 {
        damping
    } else if bounce < 0.5 {
        lerp(1.0, damping, bounce / 0.5)
    } else {
        lerp(damping, damping * 0.55, (bounce - 0.5) / 0.5)
    };
    (response * speed, d)
}
