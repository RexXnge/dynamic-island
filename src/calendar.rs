// Calendar: the month around today, opened by clicking the clock in the top row. The arrows or
// the wheel step through months; clicking the month's name goes back to this month.
use super::*;
use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK, LOCALE_SSHORTESTDAYNAME1};

const TITLE_Y: f32 = 58.0; // month name, centre
const HEAD_Y: f32 = 84.0; // weekday names, centre
const GRID_Y: f32 = 96.0; // first week's top
const ROW: f32 = 28.0;
const PAD: f32 = 24.0;

#[derive(Default)]
pub struct Cal {
    year: i32,
    month: u32, // 1..=12
    wheel: f32, // notches not yet turned into a month, so touchpads don't race
}

/// 0 = Monday … 6 = Sunday (Sakamoto's method, shifted from its 0 = Sunday).
fn weekday(y: i32, m: u32, d: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[m as usize - 1] + d as i32 + 6).rem_euclid(7)) as u32
}

fn days_in(y: i32, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn locale_info(kind: u32) -> String {
    let mut buf = [0u16; 32];
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), kind, Some(&mut buf)) };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { String::new() }
}

/// The user's first day of the week, 0 = Monday like `weekday`.
fn first_day() -> u32 {
    locale_info(LOCALE_IFIRSTDAYOFWEEK).parse().unwrap_or(0) % 7
}

/// A date in the user's language with a GetDateFormatEx picture like "MMMM yyyy".
pub fn date_text(y: i32, m: u32, d: u32, picture: &str) -> String {
    let st = SYSTEMTIME { wYear: y as u16, wMonth: m as u16, wDay: d as u16, ..Default::default() };
    let mut buf = [0u16; 64];
    let n = unsafe { GetDateFormatEx(PCWSTR::null(), Default::default(), Some(&st), &HSTRING::from(picture), Some(&mut buf), PCWSTR::null()) };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { format!("{y}-{m:02}-{d:02}") }
}

impl App {
    pub fn open_calendar(&mut self) {
        let st = unsafe { GetLocalTime() };
        self.cal = Cal { year: st.wYear as i32, month: st.wMonth as u32, wheel: 0.0 };
        self.set_view(View::Calendar);
    }

    /// Blank days before the 1st, and the number of week rows.
    fn cal_grid(&self) -> (u32, u32) {
        let (y, m) = (self.cal.year, self.cal.month);
        let lead = (weekday(y, m, 1) + 7 - first_day()) % 7;
        (lead, (lead + days_in(y, m)).div_ceil(7))
    }

    pub fn calendar_size(&self) -> (f32, f32, f32) {
        (WIDE, GRID_Y + self.cal_grid().1 as f32 * ROW + 14.0, 36.0)
    }

    fn cal_step(&mut self, months: i32) {
        let i = self.cal.year * 12 + self.cal.month as i32 - 1 + months;
        self.cal.year = i.div_euclid(12);
        self.cal.month = i.rem_euclid(12) as u32 + 1;
        self.set_view(View::Calendar); // resizes when the month needs another week row
    }

    pub fn calendar_wheel(&mut self, notches: f32) -> bool {
        if self.view != View::Calendar || self.expand.to < 0.5 {
            return false;
        }
        self.cal.wheel += notches;
        while self.cal.wheel.abs() >= 1.0 {
            let up = self.cal.wheel > 0.0;
            self.cal.wheel -= if up { 1.0 } else { -1.0 };
            self.cal_step(if up { -1 } else { 1 });
        }
        true
    }

    /// Centres of the previous and next month arrows.
    fn cal_arrows(l: &Layout) -> [(f32, f32); 2] {
        let cy = l.y + TITLE_Y;
        [(l.x + PAD + 8.0, cy), (l.x + l.w - PAD - 8.0, cy)]
    }

    pub fn calendar_click(&mut self, x: f32, y: f32) -> bool {
        if self.view != View::Calendar {
            return false;
        }
        let l = self.layout();
        let [prev, next] = Self::cal_arrows(&l);
        let near = |(cx, cy): (f32, f32)| (x - cx).abs() < 16.0 && (y - cy).abs() < 14.0;
        if near(prev) {
            self.cal_step(-1);
        } else if near(next) {
            self.cal_step(1);
        } else if (y - prev.1).abs() < 14.0 && (x - (l.x + l.w / 2.0)).abs() < 90.0 {
            self.open_calendar(); // back to this month
        }
        true
    }

    pub unsafe fn draw_calendar(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let text = |s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F| dc.DrawText(&s.encode_utf16().collect::<Vec<_>>(), f, &r, br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let (y, m) = (self.cal.year, self.cal.month);
        let st = GetLocalTime();
        let this_month = st.wYear as i32 == y && st.wMonth as u32 == m;

        br.SetColor(&fg(a));
        text(&date_text(y, m, 1, "MMMM yyyy"), &g.f_wtitle, rect(l.x + 60.0, l.y + TITLE_Y - 11.0, l.w - 120.0, 22.0));
        br.SetColor(&fg(a * 0.7));
        for ((cx, cy), dir) in Self::cal_arrows(l).into_iter().zip([-1.0f32, 1.0]) {
            let pt = |dx: f32, dy: f32| D2D_POINT_2F { x: cx + dx * dir, y: cy + dy };
            dc.DrawLine(pt(-2.5, -5.0), pt(2.5, 0.0), br, 1.8, &g.caps);
            dc.DrawLine(pt(2.5, 0.0), pt(-2.5, 5.0), br, 1.8, &g.caps);
        }

        let cw = (l.w - 2.0 * PAD) / 7.0;
        let col_x = |c: u32| l.x + PAD + cw * (c as f32 + 0.5);
        let first = first_day();
        br.SetColor(&fg(a * 0.45));
        for c in 0..7 {
            let name = locale_info(LOCALE_SSHORTESTDAYNAME1 + (first + c) % 7);
            text(&name, &g.f_seg, rect(col_x(c) - cw / 2.0, l.y + HEAD_Y - 8.0, cw, 16.0));
        }

        let (lead, _) = self.cal_grid();
        for d in 1..=days_in(y, m) {
            let i = lead + d - 1;
            let (cx, cy) = (col_x(i % 7), l.y + GRID_Y + (i / 7) as f32 * ROW + ROW / 2.0);
            let r = rect(cx - 15.0, cy - 8.0, 30.0, 16.0);
            if this_month && d == st.wDay as u32 {
                br.SetColor(&fg(a));
                dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: 12.0, radiusY: 12.0 }, br);
                br.SetColor(&bg(a));
            } else {
                br.SetColor(&fg(a * 0.85));
            }
            text(&d.to_string(), &g.f_btn, r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekdays_and_month_lengths() {
        assert_eq!(weekday(2026, 10, 25), 6); // a Sunday
        assert_eq!(weekday(2024, 2, 29), 3); // a Thursday
        assert_eq!(weekday(2000, 1, 1), 5); // a Saturday
        assert_eq!((days_in(2024, 2), days_in(2026, 2), days_in(1900, 2), days_in(2000, 2)), (29, 28, 28, 29));
    }
}
