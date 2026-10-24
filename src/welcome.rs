// First-run welcome: the island opens by itself into a tour of everything it does. Page one
// greets you with "hello" in a few languages; the others show how to use music, Control Center,
// the tools, the shelf, notices and settings, each with a small animated illustration. Pages slide and crossfade; the last one
// links to the GitHub repo for a star. Skip or Get started
// collapses it for good (`welcomed` in settings.ini).
use super::*;

pub const WELCOME: (f32, f32, f32) = (340.0, 214.0, 40.0);
const PAGES: usize = 7;
const B_NEXT: usize = 30; // press-scale ids; above every top-row button id
const B_SKIP: usize = 31;
const B_STAR: usize = 32;
const REPO: &str = "https://github.com/RexXnge/dynamic-island";
const FOOT: f32 = 30.0; // footer row centre, from the bottom edge

/// Ease-out with a little overshoot, for things that pop in.
fn back_out(t: f32) -> f32 {
    let t = clamp01(t) - 1.0;
    1.0 + t * t * (2.7 * t + 1.7)
}

fn smooth(t: f32) -> f32 {
    let t = clamp01(t);
    t * t * (3.0 - 2.0 * t)
}

impl App {
    pub fn open_welcome(&mut self) {
        self.wpage = spring(0.0);
        self.w_t0 = self.t;
        self.set_view(View::Welcome);
        self.expand.to = 1.0;
    }

    fn close_welcome(&mut self) {
        self.cfg.welcomed = true;
        self.cfg.save();
        self.expand.to = 0.0;
    }

    pub fn welcome_busy(&self) -> bool {
        // Shimmer and waveform move continuously while the tour is open.
        self.view == View::Welcome && (self.expand.to > 0.5 || !self.expand.settled()) || !self.wpage.settled()
    }

    /// Next, Skip, the "Star on GitHub" link that takes Skip's place on the last page, and the row's centre.
    fn footer(l: &Layout) -> (D2D_RECT_F, D2D_RECT_F, D2D_RECT_F, f32) {
        let cy = l.y + l.h - FOOT;
        let next = rect(l.x + l.w - 22.0 - 104.0, cy - 15.0, 104.0, 30.0);
        let skip = rect(l.x + 14.0, cy - 15.0, 64.0, 30.0);
        let star = rect(l.x + 10.0, cy - 15.0, 112.0, 30.0);
        (next, skip, star, cy)
    }

    pub fn welcome_click(&mut self, x: f32, y: f32) {
        let (next, skip, star, _) = Self::footer(&self.layout());
        let inside = |r: &D2D_RECT_F| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
        let page = self.wpage.to as usize;
        if inside(&next) {
            self.pressed = Some(B_NEXT);
            self.press = Spring { x: 1.0, v: 0.0, to: 0.0 };
            if page + 1 < PAGES {
                self.wpage.to = (page + 1) as f32;
                self.w_t0 = self.t; // replay the page's entrance
            } else {
                self.close_welcome();
            }
        } else if inside(&skip) && page + 1 < PAGES {
            self.pressed = Some(B_SKIP);
            self.press = Spring { x: 1.0, v: 0.0, to: 0.0 };
            self.close_welcome();
        } else if inside(&star) && page + 1 == PAGES {
            self.pressed = Some(B_STAR);
            self.press = Spring { x: 1.0, v: 0.0, to: 0.0 };
            unsafe {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                ShellExecuteW(None, w!("open"), &HSTRING::from(REPO), None, None, SW_SHOWNORMAL);
            }
        }
    }

    pub fn welcome_step(&mut self, dt: f32) {
        self.wpage.step(dt, 0.5, 0.86);
    }

    pub unsafe fn draw_welcome(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let opts = D2D1_DRAW_TEXT_OPTIONS_NONE;
        let mm = DWRITE_MEASURING_MODE_NATURAL;
        let text = |s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F| dc.DrawText(&s.encode_utf16().collect::<Vec<_>>(), f, &r, br, opts, mm);
        let t = self.t - self.w_t0; // time since this page came in
        let p = self.wpage.x;

        for page in 0..PAGES {
            let d = page as f32 - p;
            let pa = a * clamp01(1.0 - d.abs() * 1.4);
            if pa <= 0.0 {
                continue;
            }
            // Pages slide sideways and settle; the incoming one also rises a little.
            let ox = d * 70.0;
            let entering = page as f32 == self.wpage.to;
            let rise = |delay: f32| if entering { (1.0 - smooth((t - delay) / 0.45)) * 10.0 } else { 0.0 };
            let fade = |delay: f32| if entering { smooth((t - delay) / 0.4) } else { 1.0 };
            let (x, w) = (l.x + 28.0 + ox, l.w - 56.0);
            let body_y = l.y + 112.0;
            let (title, body) = match page {
                0 => ("Welcome to Dynamic Island", "A quiet home for music, timers and quick tools, right at the top of your screen."),
                1 => ("Music, front and center", "Hover the pill to peek at what's playing. Click it for the full player to skip, pause or scrub."),
                2 => ("Control Center", "Scroll down on the island for Wi-Fi, Bluetooth, mic, sound, brightness and dark mode. Scroll up to go back."),
                3 => ("Your tools, one click away", "Clipboard history, the file shelf, a calculator and timers sit in the top row when it's open."),
                4 => ("A shelf for files", "Drag files onto the island to park them, then drag them out wherever you need them."),
                5 => ("Quick notices", "Copied text, screenshots, downloads, USB devices and earbuds pop up as small pills."),
                _ => ("Make it yours", "Pick a theme, size and motion in Settings, the gear at the top right. Any pill can be turned off there."),
            };
            match page {
                0 => self.hello(l.x + ox, l.y + 22.0 + rise(0.0), l.w, pa, t),
                1 => self.wave_art(l.x + l.w / 2.0 + ox, l.y + 50.0 + rise(0.0), pa * fade(0.0)),
                2 => self.control_art(l.x + l.w / 2.0 + ox, l.y + 50.0 + rise(0.0), pa * fade(0.0)),
                4 => self.shelf_art(l.x + l.w / 2.0 + ox, l.y + 50.0 + rise(0.0), pa * fade(0.0), t),
                3 | 5 => {
                    // Icons pop in one after another: the tools, or what shows up as a notice.
                    let n = if page == 3 { 4 } else { 3 };
                    for i in 0..n {
                        let k = if entering { back_out((t - 0.08 * i as f32) / 0.5) } else { 1.0 };
                        if k > 0.0 {
                            let (cx, cy, ka) = (l.x + l.w / 2.0 + ox + (i as f32 - (n - 1) as f32 / 2.0) * 44.0, l.y + 50.0, pa * clamp01(k));
                            br.SetColor(&surface(ka));
                            dc.FillRoundedRectangle(&rrect(cx - 17.0 * k, cy - 17.0 * k, 34.0 * k, 34.0 * k, 11.0 * k), br);
                            match (page, i) {
                                (3, _) => self.top_icon([B_CLIPS, B_SHELF, B_CALC, B_TIMER][i], cx, cy, 1.25 * k, ka),
                                (_, 0) => self.top_icon(B_CLIPS, cx, cy, 1.25 * k, ka),
                                (_, 1) => self.usb_icon(cx, cy, 0.85 * k, ka, true, 1.0),
                                _ => self.headphones(cx, cy, 0.85 * k, ka),
                            }
                        }
                    }
                }
                _ => {
                    // Gear turns into place as it pops in, then keeps a slow spin.
                    let k = if entering { back_out(t / 0.55) } else { 1.0 };
                    let (cx, cy) = (l.x + l.w / 2.0 + ox, l.y + 50.0);
                    br.SetColor(&surface(pa * clamp01(k)));
                    dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: 21.0 * k, radiusY: 21.0 * k }, br);
                    let spin = (1.0 - clamp01(k)) * -120.0 + self.t * 12.0;
                    dc.SetTransform(&Matrix3x2::rotation(spin, cx, cy));
                    self.top_icon(B_GEAR, cx, cy, 1.5 * k, pa * clamp01(k));
                    dc.SetTransform(&Matrix3x2::identity());
                }
            }
            if page > 0 {
                br.SetColor(&fg(pa * fade(0.1)));
                text(title, &g.f_wtitle, rect(x, l.y + 82.0 + rise(0.1), w, 24.0));
            } else {
                br.SetColor(&fg(pa * 0.6 * fade(0.7)));
                text(title, &g.f_wbtn, rect(x, l.y + 82.0 + rise(0.7), w, 22.0));
            }
            br.SetColor(&fg(pa * 0.55 * fade(if page == 0 { 0.85 } else { 0.2 })));
            text(body, &g.f_wbody, rect(x, body_y + rise(if page == 0 { 0.85 } else { 0.2 }), w, 40.0));
        }

        // Footer: page dots (the current one stretches into a pill), Skip, Next / Get started.
        let (next, skip, star, cy) = Self::footer(l);
        // Seven pages: smaller dots so the row fits between Skip and Next.
        let mut dx = l.x + l.w / 2.0 - ((PAGES - 1) as f32 * 10.0 + 15.0) / 2.0;
        for i in 0..PAGES {
            let on = clamp01(1.0 - (i as f32 - p).abs());
            let dw = 5.0 + 10.0 * on;
            br.SetColor(&fg(a * (0.25 + 0.65 * on)));
            dc.FillRoundedRectangle(&rrect(dx, cy - 2.5, dw, 5.0, 2.5), br);
            dx += dw + 5.0;
        }
        let last = clamp01(p - (PAGES - 2) as f32); // 0 -> 1 across the final page turn
        let press = |id: usize| if self.pressed == Some(id) { 1.0 - 0.08 * self.press.x } else { 1.0 };
        if last < 1.0 {
            br.SetColor(&fg(a * 0.5 * (1.0 - last)));
            text("Skip", &g.f_wbtn, skip);
        }
        if last > 0.0 {
            // Fades in where Skip was; brightens for a moment when clicked.
            let lit = if self.pressed == Some(B_STAR) { 0.4 * self.press.x } else { 0.0 };
            br.SetColor(&fg(a * (0.6 + lit) * last));
            text("★ Star on GitHub", &g.f_wbtn, star);
        }
        let k = press(B_NEXT);
        let (cx, ny) = ((next.left + next.right) / 2.0, (next.top + next.bottom) / 2.0);
        let (bw, bh) = ((next.right - next.left) * k, 30.0 * k);
        br.SetColor(&fg(a)); // white button, island-coloured label
        dc.FillRoundedRectangle(&rrect(cx - bw / 2.0, ny - bh / 2.0, bw, bh, bh / 2.0), br);
        // Label crossfades from "Next" to "Get started" on the last page.
        let r = rect(cx - bw / 2.0, ny - 11.0, bw, 22.0);
        br.SetColor(&bg(a * (1.0 - last)));
        text("Next", &g.f_wbtn, r);
        br.SetColor(&bg(a * last));
        text("Get started", &g.f_wbtn, r);
    }

    /// Apple-style greeting: a light "hello" whose letters rise in one after another, then drifts
    /// up and out for the next language. Plain white, like the Mac's.
    unsafe fn hello(&self, x: f32, y: f32, w: f32, a: f32, t: f32) {
        const WORDS: [&str; 7] = ["hello", "hola", "bonjour", "hallo", "ciao", "привет", "こんにちは"];
        const HOLD: f32 = 2.2; // seconds per word
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let local = t % HOLD;
        let word: Vec<u16> = WORDS[(t / HOLD) as usize % WORDS.len()].encode_utf16().collect();
        let Ok(lay) = g.dw.CreateTextLayout(&word, &g.f_hello, w, 56.0) else { return };
        // The word leaves together: fades and drifts up over the last part of its slot.
        let out = smooth((local - (HOLD - 0.4)) / 0.4);
        let at = |i: usize| {
            let (mut px, mut py, mut hm) = (0.0, 0.0, DWRITE_HIT_TEST_METRICS::default());
            let _ = lay.HitTestTextPosition(i as u32, false, &mut px, &mut py, &mut hm);
            px
        };
        for (i, ch) in word.iter().enumerate() {
            let k = smooth((local - 0.06 * i as f32) / 0.5); // letters arrive one after another
            let la = a * k * (1.0 - out);
            if la <= 0.0 {
                continue;
            }
            let (cx, cw) = (x + at(i), at(i + 1) - at(i));
            let cy = y + 12.0 * (1.0 - k) - 8.0 * out;
            let r = rect(cx - 2.0, cy, cw + 30.0, 56.0);
            let one = [*ch];
            let Ok(cl) = g.dw.CreateTextLayout(&one, &g.f_hello_l, cw + 30.0, 56.0) else { continue };
            br.SetColor(&fg(la));
            dc.DrawTextLayout(D2D_POINT_2F { x: r.left + 2.0, y: r.top }, &cl, br, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// A tiny Control Center: Wi-Fi on, Bluetooth off, and a level slider breathing up and down.
    unsafe fn control_art(&self, cx: f32, cy: f32, a: f32) {
        let (dc, br) = (&self.g.dc, &self.g.brush);
        br.SetColor(&surface(a));
        dc.FillRoundedRectangle(&rrect(cx - 62.0, cy - 22.0, 82.0, 44.0, 14.0), br);
        let disc = |x: f32, on: bool| {
            br.SetColor(&fg(a * if on { 1.0 } else { 0.12 }));
            dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x, y: cy }, radiusX: 14.0, radiusY: 14.0 }, br);
        };
        disc(cx - 41.0, true);
        settings::swap_fg_bg(|| self.wifi_icon(cx - 41.0, cy, 0.75, a));
        disc(cx - 1.0, false);
        self.bt_icon(cx - 1.0, cy, 0.75, a);
        let (sx, sw) = (cx + 28.0, 28.0);
        br.SetColor(&surface(a));
        dc.FillRoundedRectangle(&rrect(sx, cy - 22.0, sw, 44.0, 10.0), br);
        let v = 0.45 + 0.25 * (self.t * 1.6).sin();
        dc.PushAxisAlignedClip(&rect(sx, cy + 22.0 - 44.0 * v, sw, 44.0 * v), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        br.SetColor(&fg(a * 0.92));
        dc.FillRoundedRectangle(&rrect(sx, cy - 22.0, sw, 44.0, 10.0), br);
        dc.PopAxisAlignedClip();
    }

    /// The shelf with a file dropping into it, over and over.
    unsafe fn shelf_art(&self, cx: f32, cy: f32, a: f32, t: f32) {
        let (dc, br) = (&self.g.dc, &self.g.brush);
        br.SetColor(&surface(a));
        dc.FillRoundedRectangle(&rrect(cx - 17.0, cy - 11.0, 34.0, 34.0, 11.0), br);
        self.top_icon(B_SHELF, cx, cy + 6.0, 1.25, a);
        let ph = (t % 1.8) / 1.8;
        let fall = smooth(ph / 0.55);
        let fa = a * smooth(ph / 0.15) * (1.0 - smooth((ph - 0.55) / 0.2));
        if fa > 0.0 {
            let (fx, fy) = (cx - 6.0, cy - 30.0 + 22.0 * fall);
            br.SetColor(&fg(fa));
            dc.FillRoundedRectangle(&rrect(fx, fy, 12.0, 15.0, 2.5), br);
            br.SetColor(&bg(fa));
            dc.FillRectangle(&rect(fx + 3.0, fy + 5.0, 6.0, 1.4), br);
            dc.FillRectangle(&rect(fx + 3.0, fy + 8.5, 6.0, 1.4), br);
        }
    }

    /// Bouncing white waveform bars.
    unsafe fn wave_art(&self, cx: f32, cy: f32, a: f32) {
        let (dc, br) = (&self.g.dc, &self.g.brush);
        br.SetColor(&fg(a));
        let (bars, bw, gap) = (7, 4.0, 4.0);
        let x0 = cx - (bars as f32 * (bw + gap) - gap) / 2.0;
        for i in 0..bars {
            let f = i as f32;
            let n = 0.5 + 0.3 * (self.t * (5.3 + f * 0.9) + f * 1.3).sin() + 0.2 * (self.t * (8.1 - f * 0.7) + f * 2.1).sin();
            let env = 1.0 - ((f - 3.0) / 4.0).powi(2) * 0.6;
            let h = 6.0 + 28.0 * n * env;
            dc.FillRoundedRectangle(&rrect(x0 + f * (bw + gap), cy - h / 2.0, bw, h, bw / 2.0), br);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing() {
        assert_eq!(back_out(0.0), 0.0);
        assert!((back_out(1.0) - 1.0).abs() < 1e-6);
        assert!((1..10).any(|i| back_out(i as f32 / 10.0) > 1.0)); // overshoots
        assert_eq!(smooth(0.5), 0.5);
    }
}
