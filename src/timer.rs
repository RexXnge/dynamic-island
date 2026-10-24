// Timer: iOS-style wheel picker, a countdown in the compact pill, and an expanded run view.
use super::*;
use std::time::Duration;

/// Which expanded view is showing.
#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Player,
    Pick,
    Run,
    Clips,    // clipboard history (src/clips.rs)
    Settings, // src/settings.rs
    Shelf,    // src/shelf.rs
    Calc,     // src/calc.rs
    Welcome,  // src/welcome.rs, first run only
}

pub const PICK: (f32, f32, f32) = (340.0, 214.0, 42.0);
pub const RUN: (f32, f32, f32) = (340.0, 96.0, 40.0);
pub const T_TICK: usize = 5;
pub const T_DONE: usize = 6;
const DONE_MS: u32 = 10_000; // a finished timer clears itself after this
const ROW: f32 = 23.0; // DIPs per wheel row at the selection band
const WHEEL_N: [f32; 3] = [24.0, 60.0, 60.0];
const UNITS: [&str; 3] = ["hours", "min", "sec"];
// Press-animation ids, continuing after the player's buttons.
const B_PCANCEL: usize = 6;
const B_PSTART: usize = 7;
const B_RPAUSE: usize = 8;
const B_RCANCEL: usize = 9;

pub struct Timer {
    pub total: f64,
    end: Instant,
    paused: Option<f64>, // seconds left while paused
    pub done: bool,
}

impl Timer {
    pub fn left(&self) -> f64 {
        if self.done {
            return 0.0;
        }
        self.paused.unwrap_or_else(|| self.end.saturating_duration_since(Instant::now()).as_secs_f64())
    }
    fn running(&self) -> bool {
        !self.done && self.paused.is_none()
    }
}

pub struct WheelDrag {
    col: usize,
    y0: f32,
    pos0: f32,
    last_pos: f32,
    last_t: Instant,
    vel: f32, // rows per second, smoothed, for the fling
}

/// Countdown text the way iOS shows it: rounds up, hours only when needed.
fn fmt_left(secs: f64) -> String {
    let s = secs.ceil() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl App {
    pub fn set_view(&mut self, v: View) {
        if self.view == View::Calc && v != View::Calc {
            self.close_calc_focus();
        }
        self.view = v;
        let d = match v {
            View::Player => self.player_size(),
            View::Pick => PICK,
            View::Run => RUN,
            View::Clips => self.clips_size(),
            View::Settings => settings::SETTINGS,
            View::Shelf => self.shelf_size(),
            View::Calc => calc::CALC,
            View::Welcome => welcome::WELCOME,
        };
        self.xd[0].to = d.0;
        self.xd[1].to = d.1;
        self.xd[2].to = d.2;
        for (i, s) in self.va.iter_mut().enumerate() {
            s.to = if i == v as usize { 1.0 } else { 0.0 };
        }
        // While collapsed, jump straight to the new view so expanding doesn't morph from the old one.
        if self.expand.x < 0.05 {
            for s in self.xd.iter_mut().chain(self.va.iter_mut()) {
                s.x = s.to;
                s.v = 0.0;
            }
        }
    }

    /// The pill shows whenever there's media or a timer.
    pub fn presence(&mut self) {
        self.music.to = if self.media.is_some() || self.timer.is_some() { 1.0 } else { 0.0 };
        // Both active: the compact pill shows whichever ranks higher on the Priority page.
        let timer_wins = self.media.is_none() || self.cfg.rank_of(K_TIMER) < self.cfg.rank_of(K_MUSIC);
        self.tc.to = if self.timer.is_some() && timer_wins { 1.0 } else { 0.0 };
    }

    /// The view hovering or clicking the pill should open.
    pub fn default_view(&self) -> View {
        let music_first = self.media.is_some() && self.cfg.rank_of(K_MUSIC) < self.cfg.rank_of(K_TIMER);
        if self.timer.is_some() && !music_first {
            View::Run
        } else if self.media.is_some() {
            View::Player
        } else {
            View::Clips
        }
    }

    fn wheel_value(&self, c: usize) -> u32 {
        self.wheel[c].to.round().rem_euclid(WHEEL_N[c]) as u32
    }

    fn picked(&self) -> f64 {
        self.wheel_value(0) as f64 * 3600.0 + self.wheel_value(1) as f64 * 60.0 + self.wheel_value(2) as f64
    }

    fn start_timer(&mut self, total: f64) {
        self.timer = Some(Timer { total, end: Instant::now() + Duration::from_secs_f64(total), paused: None, done: false });
        unsafe {
            let _ = KillTimer(self.hwnd, T_DONE);
            let _ = PlaySoundW(PCWSTR::null(), HMODULE::default(), SND_FLAGS(0)); // stop a ringing alarm
        }
        self.presence();
        self.set_view(View::Run);
        self.tick();
    }

    pub fn cancel_timer(&mut self) {
        self.timer = None;
        unsafe {
            let _ = KillTimer(self.hwnd, T_TICK);
            let _ = KillTimer(self.hwnd, T_DONE);
            let _ = PlaySoundW(PCWSTR::null(), HMODULE::default(), SND_FLAGS(0));
        }
        self.presence();
        if self.media.is_some() && self.hover {
            self.set_view(View::Player);
        } else {
            self.expand.to = 0.0;
        }
    }

    fn toggle_pause(&mut self) {
        let Some(t) = &mut self.timer else { return };
        if t.done {
            let total = t.total;
            return self.start_timer(total); // restart the same duration
        }
        match t.paused {
            Some(left) => {
                t.end = Instant::now() + Duration::from_secs_f64(left);
                t.paused = None;
            }
            None => t.paused = Some(t.left()),
        }
        self.tick();
    }

    /// Wakes exactly when the shown second changes, so a running timer costs one frame per second.
    pub fn tick(&mut self) {
        let Some(t) = &self.timer else { return };
        if !t.running() {
            return;
        }
        let left = t.left();
        if left <= 0.0 {
            return self.finish();
        }
        let ms = (left * 1000.0) as u32 % 1000 + 5;
        unsafe { SetTimer(self.hwnd, T_TICK, ms, None) };
    }

    fn finish(&mut self) {
        if let Some(t) = &mut self.timer {
            t.done = true;
        }
        self.bump.v = 20.0;
        self.set_view(View::Run);
        self.expand.to = 1.0;
        unsafe {
            let _ = PlaySoundW(w!("Notification.Looping.Alarm"), HMODULE::default(), SND_ALIAS | SND_ASYNC);
            SetTimer(self.hwnd, T_DONE, DONE_MS, None);
        }
    }

    pub fn timer_busy(&self) -> bool {
        self.xd.iter().chain(&self.va).chain(&self.wheel).chain([&self.tc]).any(|s| !s.settled())
            || self.wdrag.is_some()
            || self.timer.as_ref().is_some_and(|t| t.done) // pulsing
    }

    pub fn timer_step(&mut self, dt: f32) {
        for s in &mut self.xd {
            s.step(dt, 0.4, 0.82);
        }
        for s in &mut self.va {
            s.step(dt, 0.3, 1.0);
        }
        self.tc.step(dt, 0.35, 1.0);
        let held = self.wdrag.as_ref().map(|d| d.col);
        for (i, w) in self.wheel.iter_mut().enumerate() {
            if held != Some(i) {
                w.step(dt, 0.32, 0.92);
            }
        }
    }

    // ---- picker geometry ----

    fn band_y(l: &Layout) -> f32 {
        l.y + 84.0
    }

    /// Right edge of each column's numbers; the unit label sits just right of it.
    fn pick_cols(l: &Layout) -> [f32; 3] {
        let ox = l.x + (l.w - PICK.0) / 2.0;
        [ox + 70.0, ox + 176.0, ox + 276.0]
    }

    fn pick_buttons(l: &Layout) -> [(f32, f32); 2] {
        let y = l.y + l.h - 32.0;
        [(l.x + 48.0, y), (l.x + l.w - 48.0, y)]
    }

    fn run_buttons(l: &Layout) -> [(f32, f32); 2] {
        let y = l.y + l.h / 2.0;
        [(l.x + l.w - 104.0, y), (l.x + l.w - 48.0, y)]
    }

    fn col_at(l: &Layout, x: f32, y: f32) -> Option<usize> {
        if (y - Self::band_y(l)).abs() > 74.0 {
            return None;
        }
        let cols = Self::pick_cols(l);
        (0..3).find(|&c| (x - (cols[c] + 12.0)).abs() < 52.0)
    }

    fn press_btn(&mut self, id: usize) {
        self.pressed = Some(id);
        self.press = Spring { x: 1.0, v: 0.0, to: 0.0 };
    }

    // ---- input ----

    /// Clicks in the timer views. Returns false when the player view should handle it.
    pub fn timer_click(&mut self, x: f32, y: f32) -> bool {
        let l = self.layout();
        let hit = |(cx, cy): (f32, f32)| (x - cx).hypot(y - cy) < 25.0;
        match self.view {
            View::Player | View::Clips | View::Settings | View::Shelf | View::Calc | View::Welcome => return false,
            View::Pick => {
                let [cancel, start] = Self::pick_buttons(&l);
                if hit(cancel) {
                    self.press_btn(B_PCANCEL);
                    if self.media.is_some() {
                        self.set_view(View::Player);
                    } else {
                        self.expand.to = 0.0;
                    }
                } else if hit(start) {
                    let total = self.picked();
                    if total > 0.0 {
                        self.press_btn(B_PSTART);
                        self.start_timer(total);
                    }
                } else if let Some(col) = Self::col_at(&l, x, y) {
                    let pos = self.wheel[col].x;
                    self.wdrag = Some(WheelDrag { col, y0: y, pos0: pos, last_pos: pos, last_t: Instant::now(), vel: 0.0 });
                    unsafe { SetCapture(self.hwnd) };
                }
            }
            View::Run => {
                let [pause, cancel] = Self::run_buttons(&l);
                if hit(pause) {
                    self.press_btn(B_RPAUSE);
                    self.toggle_pause();
                } else if hit(cancel) {
                    self.press_btn(B_RCANCEL);
                    self.cancel_timer();
                }
            }
        }
        true
    }

    /// Dragging a wheel: content follows the pointer; velocity is kept for the fling.
    pub fn wheel_move(&mut self, y: f32) {
        let Some(d) = &mut self.wdrag else { return };
        let pos = d.pos0 - (y - d.y0) / ROW;
        let now = Instant::now();
        let dt = now.duration_since(d.last_t).as_secs_f32();
        if dt > 0.001 {
            d.vel = 0.6 * (pos - d.last_pos) / dt + 0.4 * d.vel;
            d.last_t = now;
            d.last_pos = pos;
        }
        let w = &mut self.wheel[d.col];
        w.x = pos;
        w.to = pos;
        w.v = 0.0;
    }

    /// Ends a wheel drag with a fling that coasts and snaps to a whole number.
    pub fn wheel_release(&mut self) -> bool {
        let Some(d) = self.wdrag.take() else { return false };
        let vel = if d.last_t.elapsed().as_secs_f32() > 0.08 { 0.0 } else { d.vel.clamp(-40.0, 40.0) };
        let w = &mut self.wheel[d.col];
        w.to = (w.x + vel * 0.18).round();
        w.v = vel;
        unsafe {
            let _ = ReleaseCapture();
        }
        true
    }

    /// Mouse wheel over a column steps it one number per notch.
    pub fn wheel_scroll(&mut self, x: f32, y: f32, notches: f32) {
        if self.view != View::Pick || self.expand.x < 0.9 {
            return;
        }
        let l = self.layout();
        if let Some(c) = Self::col_at(&l, x, y) {
            let w = &mut self.wheel[c];
            w.to = w.to.round() - notches.round();
        }
    }

    // ---- drawing ----

    pub unsafe fn draw_timer(&self, l: &Layout) {
        let cf = clamp01(1.0 - self.clip.x * 2.0) * clamp01(1.0 - self.vol.x * 2.0);
        let base = clamp01((self.music.x.max(l.e) - 0.4) / 0.6) * cf;
        let e = clamp01(l.e);
        let small = clamp01(1.0 - e * 3.0) * base * self.tc.x;
        let big = clamp01((e - 0.4) / 0.6) * base;
        self.g.dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        if small > 0.0 {
            self.draw_timer_compact(l, small);
        }
        if big * self.va[1].x > 0.0 {
            self.draw_picker(l, big * self.va[1].x);
        }
        if big * self.va[2].x > 0.0 {
            self.draw_run(l, big * self.va[2].x);
        }
        self.g.dc.PopAxisAlignedClip();
    }

    fn pulse(&self, t: &Timer) -> f32 {
        if t.done { 0.55 + 0.45 * (self.t * 5.0).cos() } else { 1.0 }
    }

    /// Orange ring that empties as time runs out, plus the countdown on the right.
    unsafe fn draw_timer_compact(&self, l: &Layout, a: f32) {
        let Some(t) = &self.timer else { return };
        let (dc, br) = (&self.g.dc, &self.g.brush);
        let (cx, cy) = (l.x + 18.0, l.y + l.h / 2.0);
        let (r, g, b) = ORANGE;
        let a = a * self.pulse(t);
        br.SetColor(&rgba(r, g, b, a * 0.3));
        dc.DrawEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: 8.5, radiusY: 8.5 }, br, 2.6, None);
        let frac = if t.total > 0.0 { (t.left() / t.total) as f32 } else { 0.0 };
        br.SetColor(&rgba(r, g, b, a));
        self.arc(cx, cy, 8.5, frac, 2.6);
        let text = if t.done { "Done".to_string() } else { fmt_left(t.left()) };
        let dim = if t.paused.is_some() { 0.55 } else { 1.0 };
        br.SetColor(&rgba(r, g, b, a * dim));
        dc.DrawText(&utf16(&text), &self.g.f_cd, &rect(l.x + 40.0, l.y, l.w - 54.0, l.h), br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    }

    /// Clockwise arc from 12 o'clock covering `frac` of the circle.
    pub unsafe fn arc(&self, cx: f32, cy: f32, r: f32, frac: f32, w: f32) {
        let (dc, br) = (&self.g.dc, &self.g.brush);
        if frac >= 0.999 {
            dc.DrawEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: r, radiusY: r }, br, w, None);
            return;
        }
        if frac <= 0.001 {
            return;
        }
        let Ok(geom) = self.g.factory.CreatePathGeometry() else { return };
        let Ok(sink) = geom.Open() else { return };
        let end = -std::f32::consts::FRAC_PI_2 + frac * std::f32::consts::TAU;
        sink.BeginFigure(D2D_POINT_2F { x: cx, y: cy - r }, D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: D2D_POINT_2F { x: cx + r * end.cos(), y: cy + r * end.sin() },
            size: D2D_SIZE_F { width: r, height: r },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        if sink.Close().is_ok() {
            dc.DrawGeometry(&geom, br, w, &self.g.caps);
        }
    }

    /// Three looping wheels on a drum, a selection band with unit labels, Cancel and Start.
    unsafe fn draw_picker(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let cy = Self::band_y(l);
        br.SetColor(&surface(a));
        dc.FillRoundedRectangle(&rrect(l.x + 16.0, cy - 16.0, l.w - 32.0, 32.0, 9.0), br);

        dc.PushAxisAlignedClip(&rect(l.x, cy - 74.0, l.w, 148.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        let cols = Self::pick_cols(l);
        for c in 0..3 {
            let pos = self.wheel[c].x;
            let base = pos.floor();
            for k in -4..=5 {
                let i = base + k as f32;
                let d = i - pos;
                let th = d * 0.36; // rows sit on a cylinder: angle per row
                if th.abs() > 1.45 {
                    continue;
                }
                let (y, sc) = (cy + 64.0 * th.sin(), th.cos());
                let lum = lerp(0.6, 1.0, clamp01(1.0 - d.abs())); // brighter at the band
                br.SetColor(&rgba(lum, lum, lum, a * (0.2 + 0.8 * sc * sc)));
                dc.SetTransform(&mat(1.0, sc, 0.0, y * (1.0 - sc))); // squash vertically about the row
                let val = i.rem_euclid(WHEEL_N[c]) as u32;
                dc.DrawText(&utf16(&val.to_string()), &g.f_wheel, &rect(cols[c] - 60.0, y - 14.0, 60.0, 28.0), br, opts, mm);
            }
            dc.SetTransform(&Matrix3x2::identity());
            br.SetColor(&fg(a));
            dc.DrawText(&utf16(UNITS[c]), &g.f_wheel_label, &rect(cols[c] + 6.0, cy - 14.0, 80.0, 28.0), br, opts, mm);
        }
        dc.PopAxisAlignedClip();

        let [cancel, start] = Self::pick_buttons(l);
        let enabled = self.picked() > 0.0;
        let (gr, gg, gb) = GREEN;
        for (id, (cx, by), fill, (tr, tg, tb), label, on) in [
            (B_PCANCEL, cancel, surface_rgb(), fg_rgb(), "Cancel", true),
            (B_PSTART, start, (0.07, 0.25, 0.11), (gr, gg, gb), "Start", enabled),
        ] {
            let k = if self.pressed == Some(id) { 1.0 - 0.15 * self.press.x } else { 1.0 };
            let dim = if on { 1.0 } else { 0.45 };
            br.SetColor(&rgba(fill.0, fill.1, fill.2, a * dim));
            dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: by }, radiusX: 25.0 * k, radiusY: 25.0 * k }, br);
            br.SetColor(&rgba(tr, tg, tb, a * dim));
            dc.DrawText(&utf16(label), &g.f_btn, &rect(cx - 30.0, by - 10.0, 60.0, 20.0), br, opts, mm);
        }
    }

    /// Big countdown on the left, pause/resume and cancel on the right.
    unsafe fn draw_run(&self, l: &Layout, a: f32) {
        let Some(t) = &self.timer else { return };
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let (r, gg, b) = ORANGE;
        let label = if t.done { "Timer done" } else if t.paused.is_some() { "Paused" } else { "Timer" };
        br.SetColor(&rgba(r, gg, b, a));
        dc.DrawText(&utf16(label), &g.f_title_s, &rect(l.x + 28.0, l.y + 14.0, 160.0, 18.0), br, opts, mm);
        br.SetColor(&fg(a * self.pulse(t)));
        dc.DrawText(&utf16(&fmt_left(t.left())), &g.f_big, &rect(l.x + 25.0, l.y + 32.0, 200.0, 48.0), br, opts, mm);

        let [(px, py), (xx, xy)] = Self::run_buttons(l);
        let k = |id| if self.pressed == Some(id) { 1.0 - 0.15 * self.press.x } else { 1.0 };
        let circle = |x: f32, y: f32, r: f32| D2D1_ELLIPSE { point: D2D_POINT_2F { x, y }, radiusX: r, radiusY: r };
        // Pause / resume (play also restarts a finished timer).
        let kp = k(B_RPAUSE);
        br.SetColor(&rgba(0.35, 0.21, 0.0, a));
        dc.FillEllipse(&circle(px, py, 23.0 * kp), br);
        br.SetColor(&rgba(r, gg, b, a));
        if t.running() {
            for ox in [-6.0, 1.8] {
                dc.FillRoundedRectangle(&rrect(px + ox * kp, py - 8.5 * kp, 4.2 * kp, 17.0 * kp, 1.6 * kp), br);
            }
        } else {
            self.triangle(px - 6.0 * kp, py - 9.0 * kp, 18.0 * kp, false);
        }
        // Cancel.
        let kc = k(B_RCANCEL);
        br.SetColor(&surface(a));
        dc.FillEllipse(&circle(xx, xy, 23.0 * kc), br);
        br.SetColor(&fg(a));
        let d = 6.0 * kc;
        dc.DrawLine(D2D_POINT_2F { x: xx - d, y: xy - d }, D2D_POINT_2F { x: xx + d, y: xy + d }, br, 2.6, &g.caps);
        dc.DrawLine(D2D_POINT_2F { x: xx + d, y: xy - d }, D2D_POINT_2F { x: xx - d, y: xy + d }, br, 2.6, &g.caps);
    }
}
