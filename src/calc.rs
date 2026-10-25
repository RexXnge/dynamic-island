// Calculator and unit converter: type `12*7` or `5 km to mi` into the island. The island takes
// keyboard focus only while this view is open, and hands it back when it closes.
use super::*;

pub const CALC: (f32, f32, f32) = (WIDE, 112.0, 36.0);
const MAX_INPUT: usize = 80;

/// Evaluates `input` as a unit conversion ("5 km to mi") or else as arithmetic.
pub fn answer(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(r) = convert(s) {
        return Some(r);
    }
    let v = eval(s)?;
    v.is_finite().then(|| fmt_num(v))
}

// ---- arithmetic: + - * / ^, %, parentheses, pi, e, a few functions ----

pub fn eval(s: &str) -> Option<f64> {
    // A comma is a decimal point too (German keyboards), and × ÷ work like * /.
    let s: Vec<char> = s.chars().map(|c| match c {
        ',' => '.',
        '×' => '*',
        '÷' => '/',
        c => c,
    }).filter(|c| !c.is_whitespace()).collect();
    let mut p = Parser { s: &s, i: 0 };
    let v = p.expr()?;
    (p.i == s.len()).then_some(v)
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: char) -> bool {
        let hit = self.peek() == Some(c);
        self.i += hit as usize;
        hit
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat('+') {
                v += self.term()?;
            } else if self.eat('-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat('*') {
                v *= self.unary()?;
            } else if self.eat('/') {
                v /= self.unary()?;
            } else if matches!(self.peek(), Some('(') | Some('a'..='z')) {
                v *= self.unary()?; // implicit: 2pi, 3(4+1)
            } else {
                return Some(v);
            }
        }
    }
    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            return Some(-self.unary()?); // -3^2 = -(3^2)
        }
        if self.eat('+') {
            return self.unary();
        }
        self.power()
    }
    fn power(&mut self) -> Option<f64> {
        let mut b = self.primary()?;
        while self.eat('%') {
            b /= 100.0;
        }
        if self.eat('^') {
            return Some(b.powf(self.unary()?)); // right-associative, allows 2^-1
        }
        Some(b)
    }
    fn primary(&mut self) -> Option<f64> {
        if self.eat('(') {
            let v = self.expr()?;
            self.eat(')'); // a missing close paren is fine while typing
            return Some(v);
        }
        let start = self.i;
        if matches!(self.peek(), Some('0'..='9' | '.')) {
            while matches!(self.peek(), Some('0'..='9' | '.')) {
                self.i += 1;
            }
            return self.s[start..self.i].iter().collect::<String>().parse().ok();
        }
        while matches!(self.peek(), Some('a'..='z' | 'A'..='Z')) {
            self.i += 1;
        }
        let name: String = self.s[start..self.i].iter().collect::<String>().to_ascii_lowercase();
        let f: fn(f64) -> f64 = match name.as_str() {
            "pi" => return Some(std::f64::consts::PI),
            "e" => return Some(std::f64::consts::E),
            "sqrt" => f64::sqrt,
            "sin" => f64::sin,
            "cos" => f64::cos,
            "tan" => f64::tan,
            "ln" => f64::ln,
            "log" => f64::log10,
            "abs" => f64::abs,
            "round" => f64::round,
            _ => return None,
        };
        Some(f(self.unary()?))
    }
}

/// Up to 10 significant digits, no trailing zeros; scientific for huge or tiny values.
pub fn fmt_num(v: f64) -> String {
    let a = v.abs();
    if a != 0.0 && !(1e-6..1e15).contains(&a) {
        return format!("{v:.6e}").replace("e", "×10^").replace(".000000", "");
    }
    let decimals = if a == 0.0 { 0 } else { (9 - a.log10().floor() as i32).clamp(0, 10) as usize };
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

// ---- units ----

#[derive(PartialEq, Clone, Copy)]
enum Dim {
    Length,
    Mass,
    Volume,
    Time,
    Data,
    Speed,
    Area,
    Temp,
}

/// (names, dimension, size in the dimension's base unit). Temperatures are handled apart.
const UNITS: &[(&[&str], Dim, f64)] = &[
    (&["mm", "millimeter", "millimetre"], Dim::Length, 0.001),
    (&["cm", "centimeter", "centimetre"], Dim::Length, 0.01),
    (&["m", "meter", "metre"], Dim::Length, 1.0),
    (&["km", "kilometer", "kilometre"], Dim::Length, 1000.0),
    (&["in", "inch", "inches", "\""], Dim::Length, 0.0254),
    (&["ft", "foot", "feet", "'"], Dim::Length, 0.3048),
    (&["yd", "yard"], Dim::Length, 0.9144),
    (&["mi", "mile"], Dim::Length, 1609.344),
    (&["nmi"], Dim::Length, 1852.0),
    (&["mg"], Dim::Mass, 1e-6),
    (&["g", "gram"], Dim::Mass, 0.001),
    (&["kg", "kilo", "kilogram"], Dim::Mass, 1.0),
    (&["t", "ton", "tonne"], Dim::Mass, 1000.0),
    (&["oz", "ounce"], Dim::Mass, 0.028349523125),
    (&["lb", "lbs", "pound"], Dim::Mass, 0.45359237),
    (&["st", "stone"], Dim::Mass, 6.35029318),
    (&["ml", "milliliter", "millilitre"], Dim::Volume, 0.001),
    (&["cl"], Dim::Volume, 0.01),
    (&["dl"], Dim::Volume, 0.1),
    (&["l", "liter", "litre"], Dim::Volume, 1.0),
    (&["gal", "gallon"], Dim::Volume, 3.785411784),
    (&["qt", "quart"], Dim::Volume, 0.946352946),
    (&["pt", "pint"], Dim::Volume, 0.473176473),
    (&["cup"], Dim::Volume, 0.2365882365),
    (&["floz"], Dim::Volume, 0.0295735295625),
    (&["tbsp"], Dim::Volume, 0.01478676478125),
    (&["tsp"], Dim::Volume, 0.00492892159375),
    (&["ms", "millisecond"], Dim::Time, 0.001),
    (&["s", "sec", "second"], Dim::Time, 1.0),
    (&["min", "minute"], Dim::Time, 60.0),
    (&["h", "hr", "hour"], Dim::Time, 3600.0),
    (&["d", "day"], Dim::Time, 86400.0),
    (&["wk", "week"], Dim::Time, 604800.0),
    (&["yr", "year"], Dim::Time, 31557600.0),
    (&["bit"], Dim::Data, 0.125),
    (&["b", "byte"], Dim::Data, 1.0),
    (&["kb"], Dim::Data, 1e3),
    (&["mb"], Dim::Data, 1e6),
    (&["gb"], Dim::Data, 1e9),
    (&["tb"], Dim::Data, 1e12),
    (&["kib"], Dim::Data, 1024.0),
    (&["mib"], Dim::Data, 1048576.0),
    (&["gib"], Dim::Data, 1073741824.0),
    (&["m/s", "mps"], Dim::Speed, 1.0),
    (&["km/h", "kmh", "kph"], Dim::Speed, 1.0 / 3.6),
    (&["mph"], Dim::Speed, 0.44704),
    (&["kn", "knot"], Dim::Speed, 0.514444),
    (&["m2", "m²", "sqm"], Dim::Area, 1.0),
    (&["km2", "km²"], Dim::Area, 1e6),
    (&["ft2", "ft²", "sqft"], Dim::Area, 0.09290304),
    (&["ha", "hectare"], Dim::Area, 1e4),
    (&["acre"], Dim::Area, 4046.8564224),
];

/// Kind and size of a unit name; temperatures come back with size 0 and their letter in `Temp`.
fn unit(name: &str) -> Option<(Dim, f64, char)> {
    let n = name.trim().to_lowercase();
    let n = n.trim_start_matches('°');
    match n {
        "c" | "celsius" => return Some((Dim::Temp, 0.0, 'c')),
        "f" | "fahrenheit" => return Some((Dim::Temp, 0.0, 'f')),
        "k" | "kelvin" => return Some((Dim::Temp, 0.0, 'k')),
        _ => {}
    }
    let find = |n: &str| UNITS.iter().find(|u| u.0.contains(&n)).map(|u| (u.1, u.2, ' '));
    find(n).or_else(|| n.strip_suffix('s').and_then(find)) // plurals: miles, hours
}

fn to_kelvin(v: f64, u: char) -> f64 {
    match u {
        'c' => v + 273.15,
        'f' => (v - 32.0) * 5.0 / 9.0 + 273.15,
        _ => v,
    }
}

fn from_kelvin(v: f64, u: char) -> f64 {
    match u {
        'c' => v - 273.15,
        'f' => (v - 273.15) * 9.0 / 5.0 + 32.0,
        _ => v,
    }
}

/// "5 km to mi", "100f in c", "2.5 hours to min".
fn convert(s: &str) -> Option<String> {
    let lower = s.to_lowercase();
    let (left, target) = lower.rsplit_once(" to ").or_else(|| lower.rsplit_once(" in "))?;
    let (dim_to, k_to, t_to) = unit(target)?;
    // The longest unit name at the end of `left` whose remainder is a number or expression.
    let (v, (dim, k, t)) = left.char_indices().find_map(|(i, _)| Some((eval(&left[..i])?, unit(&left[i..])?)))?;
    if dim != dim_to {
        return None;
    }
    let r = if dim == Dim::Temp { from_kelvin(to_kelvin(v, t), t_to) } else { v * k / k_to };
    r.is_finite().then(|| format!("{} {}", fmt_num(r), target.trim()))
}

impl App {
    /// Opens the calculator and takes keyboard focus, remembering who had it.
    pub fn open_calc(&mut self) {
        self.set_view(View::Calc);
        self.expand.to = 1.0;
        unsafe {
            let prev = GetForegroundWindow();
            if prev != self.hwnd {
                self.calc_prev = prev;
            }
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(self.hwnd);
        }
    }

    pub fn close_calc(&mut self) {
        self.expand.to = 0.0;
        self.close_calc_focus();
    }

    /// Gives focus back to the app the user was in.
    pub fn close_calc_focus(&mut self) {
        unsafe {
            if GetForegroundWindow() == self.hwnd && !self.calc_prev.is_invalid() {
                let _ = SetForegroundWindow(self.calc_prev);
            }
        }
    }

    pub fn calc_char(&mut self, c: u32) {
        if self.view != View::Calc || self.expand.to < 0.5 {
            return;
        }
        match c {
            8 => {
                self.calc_in.pop();
            }
            13 => {
                // Enter copies the answer (just the number for conversions' value too).
                if let Some(r) = answer(&self.calc_in) {
                    let _ = unsafe { clips::set_clipboard_text(self.hwnd, &r) };
                    self.calc_copied = Spring { x: 1.0, v: 0.0, to: 0.0 };
                }
            }
            27 => self.close_calc(),
            22 => {
                // Ctrl+V
                if let Some(t) = unsafe { clips::clipboard_text(self.hwnd) } {
                    self.calc_in.extend(t.chars().filter(|c| !c.is_control()).take(MAX_INPUT - self.calc_in.chars().count().min(MAX_INPUT)));
                }
            }
            127 => self.calc_in.clear(), // Ctrl+Backspace
            c => {
                if let Some(ch) = char::from_u32(c).filter(|ch| !ch.is_control()) {
                    if self.calc_in.chars().count() < MAX_INPUT {
                        self.calc_in.push(ch);
                    }
                }
            }
        }
    }

    pub unsafe fn draw_calc(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let dy = (1.0 - a) * 6.0;
        let (x0, x1) = (l.x + 24.0, l.x + l.w - 24.0);
        let caret_w = 10.0;
        // Input, right-aligned like a calculator, with a caret after it.
        let iy = l.y + 36.0 + dy;
        let (txt, al) = if self.calc_in.is_empty() { ("12*7  or  5 km to mi", 0.35) } else { (self.calc_in.as_str(), 1.0) };
        br.SetColor(&fg(a * al));
        dc.DrawText(&txt.encode_utf16().collect::<Vec<_>>(), &g.f_calc, &rect(x0, iy, x1 - x0 - caret_w, 34.0), br, opts, mm);
        if GetForegroundWindow() == self.hwnd {
            let (r, gg, b) = accent();
            br.SetColor(&rgba(r, gg, b, a));
            dc.FillRoundedRectangle(&rrect(x1 - 4.0, iy + 7.0, 2.0, 20.0, 1.0), br);
        }
        // Answer below, in the accent colour; flashes "Copied" after Enter.
        let ry = l.y + 72.0 + dy;
        if let Some(r) = answer(&self.calc_in) {
            let (ar, ag, ab) = accent();
            let k = clamp01(self.calc_copied.x);
            br.SetColor(&rgba(ar, ag, ab, a * (1.0 - k)));
            dc.DrawText(&format!("= {r}").encode_utf16().collect::<Vec<_>>(), &g.f_calc_r, &rect(x0, ry, x1 - x0 - caret_w, 26.0), br, opts, mm);
            br.SetColor(&rgba(ar, ag, ab, a * k));
            dc.DrawText(&"Copied".encode_utf16().collect::<Vec<_>>(), &g.f_calc_r, &rect(x0, ry, x1 - x0 - caret_w, 26.0), br, opts, mm);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        assert_eq!(answer("12*7").as_deref(), Some("84"));
        assert_eq!(answer("1 + 2 * 3").as_deref(), Some("7"));
        assert_eq!(answer("(1+2)*3").as_deref(), Some("9"));
        assert_eq!(answer("2^3^2").as_deref(), Some("512"));
        assert_eq!(answer("-3^2").as_deref(), Some("-9"));
        assert_eq!(answer("10/4").as_deref(), Some("2.5"));
        assert_eq!(answer("2,5*2").as_deref(), Some("5"));
        assert_eq!(answer("20% * 80").as_deref(), Some("16"));
        assert_eq!(answer("2pi").as_deref(), Some("6.283185307"));
        assert_eq!(answer("sqrt(16)+1").as_deref(), Some("5"));
        assert_eq!(answer("0.1+0.2").as_deref(), Some("0.3"));
        assert_eq!(answer("(2+3").as_deref(), Some("5"));
        assert_eq!(answer("1/0"), None);
        assert_eq!(answer("12*"), None);
        assert_eq!(answer("hello"), None);
    }

    #[test]
    fn units() {
        assert_eq!(answer("5 km to mi").as_deref(), Some("3.106855961 mi"));
        assert_eq!(answer("100f to c").as_deref(), Some("37.77777778 c"));
        assert_eq!(answer("0 c in f").as_deref(), Some("32 f"));
        assert_eq!(answer("2.5 hours to min").as_deref(), Some("150 min"));
        assert_eq!(answer("1 gb in mb").as_deref(), Some("1000 mb"));
        assert_eq!(answer("12 in to cm").as_deref(), Some("30.48 cm"));
        assert_eq!(answer("3*2 kg to lb").as_deref(), Some("13.22773573 lb"));
        assert_eq!(answer("100 km/h to mph").as_deref(), Some("62.13711922 mph"));
        assert_eq!(answer("5 km to kg"), None);
    }
}
