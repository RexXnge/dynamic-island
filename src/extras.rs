// Small system integrations for the notice pills: copied colours and links, monitor brightness
// over DDC/CI, and Windows Focus.
use super::*;
use std::sync::mpsc::channel;
use windows::Foundation::{EventRegistrationToken, TypedEventHandler, Uri};
use windows::Storage::Streams::DataReader;
use windows::Web::Http::HttpClient;
use windows::Win32::Devices::Display::*;
use windows::Win32::Graphics::Imaging::*;
use windows::UI::Shell::FocusSessionManager;

pub const WM_FAVICON: u32 = 0x8005; // lparam = Box<(u64 generation, Vec<u8> FAV×FAV PBGRA)>
pub const WM_BRIGHT: u32 = 0x8006; // wparam = level * 10000, lparam = 1 for the silent initial read
pub const WM_FOCUS: u32 = 0x8007; // wparam = focus active, lparam = 1 for the silent initial read
pub const FAV: u32 = 32;
pub const FOCUS_TINT: (f32, f32, f32) = (0.37, 0.36, 0.9); // iOS Focus indigo

// ---- colours ----

/// CSS-style colour text: #rgb, #rgba, #rrggbb, #rrggbbaa, rgb()/rgba(), hsl()/hsla().
pub fn parse_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        let v: Vec<f32> = match hex.len() {
            3 | 4 => hex.chars().map(|c| c.to_digit(16).map(|d| (d * 17) as f32 / 255.0)).collect::<Option<_>>()?,
            6 | 8 => (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok().map(|b| b as f32 / 255.0)).collect::<Option<_>>()?,
            _ => return None,
        };
        return Some([v[0], v[1], v[2], v.get(3).copied().unwrap_or(1.0)]);
    }
    let (name, rest) = s.split_once('(')?;
    let args: Vec<&str> = rest.strip_suffix(')')?.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|a| !a.is_empty()).collect();
    if args.len() != 3 && args.len() != 4 {
        return None;
    }
    // Plain numbers scale by `max`; percentages are always out of 100.
    let num = |a: &str, max: f32| -> Option<f32> {
        match a.strip_suffix('%') {
            Some(p) => p.parse::<f32>().ok().map(|v| v / 100.0),
            None => a.strip_suffix("deg").unwrap_or(a).parse::<f32>().ok().map(|v| v / max),
        }
    };
    let alpha = match args.get(3) {
        Some(a) => num(a, 1.0)?.clamp(0.0, 1.0),
        None => 1.0,
    };
    match name {
        "rgb" | "rgba" => Some([num(args[0], 255.0)?.clamp(0.0, 1.0), num(args[1], 255.0)?.clamp(0.0, 1.0), num(args[2], 255.0)?.clamp(0.0, 1.0), alpha]),
        "hsl" | "hsla" => {
            let h = (num(args[0], 360.0)? * 360.0).rem_euclid(360.0);
            let (s, l) = (num(args[1], 100.0)?.clamp(0.0, 1.0), num(args[2], 100.0)?.clamp(0.0, 1.0));
            let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
            let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
            let (r, g, b) = match (h / 60.0) as u32 {
                0 => (c, x, 0.0),
                1 => (x, c, 0.0),
                2 => (0.0, c, x),
                3 => (0.0, x, c),
                4 => (x, 0.0, c),
                _ => (c, 0.0, x),
            };
            let m = l - c / 2.0;
            Some([r + m, g + m, b + m, alpha])
        }
        _ => None,
    }
}

// ---- links ----

/// `(url, host)` when the copied text is a single http(s) link. Only these schemes are ever opened.
pub fn parse_link(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"))?;
    // Browsers read `\` as `/`, so `https://evil\@bank.com` would be labelled bank.com but open evil.
    if s.chars().any(|c| c.is_whitespace() || c == '\\') {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?; // drop any user:pass@ and :port
    if host.is_empty() {
        return None;
    }
    Some((s.to_string(), host.strip_prefix("www.").unwrap_or(host).to_string()))
}

/// The host to ask for a favicon: https links to public domain names only, so copying a link never
/// sends a plain-http request or one to localhost, a LAN device or a raw IP address.
/// shortcut: a public name that resolves to a LAN address still gets fetched; resolve and check the IP if that matters.
fn favicon_host(url: &str) -> Option<String> {
    let rest = url.get(..8).filter(|p| p.eq_ignore_ascii_case("https://")).map(|_| &url[8..])?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?.to_ascii_lowercase(); // no user:pass@ or :port
    let tld = host.rsplit_once('.')?.1;
    let local = ["local", "lan", "home", "internal", "localhost", "arpa", "test", "invalid"].contains(&tld);
    (tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()) && !local).then_some(host)
}

/// Fetches https://host/favicon.ico on a short-lived thread. Only the host of the copied link is
/// contacted, never its path or query.
pub fn fetch_favicon(hwnd: HWND, url: &str, generation: u64) {
    let Some(host) = favicon_host(url) else { return };
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Ok(px) = favicon(&format!("https://{host}/favicon.ico")) {
            let msg = Box::into_raw(Box::new((generation, px)));
            if PostMessageW(HWND(hwnd as _), WM_FAVICON, WPARAM(0), LPARAM(msg as isize)).is_err() {
                drop(Box::from_raw(msg));
            }
        }
    });
}

unsafe fn favicon(url: &str) -> Result<Vec<u8>> {
    let buf = HttpClient::new()?.GetBufferAsync(&Uri::CreateUri(&HSTRING::from(url))?)?.get()?;
    let mut bytes = vec![0u8; buf.Length()? as usize];
    DataReader::FromBuffer(&buf)?.ReadBytes(&mut bytes)?;
    let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let stream = wic.CreateStream()?;
    stream.InitializeFromMemory(&bytes)?;
    let dec = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
    // .ico files hold several sizes; take the largest.
    let mut best: Option<(u32, IWICBitmapFrameDecode)> = None;
    for i in 0..dec.GetFrameCount()? {
        let f = dec.GetFrame(i)?;
        let (mut w, mut h) = (0, 0);
        f.GetSize(&mut w, &mut h)?;
        if best.as_ref().map_or(true, |(bw, _)| w > *bw) {
            best = Some((w, f));
        }
    }
    let (_, frame) = best.ok_or_else(|| Error::from(E_FAIL))?;
    let scaler = wic.CreateBitmapScaler()?;
    scaler.Initialize(&frame, FAV, FAV, WICBitmapInterpolationModeFant)?;
    let conv = wic.CreateFormatConverter()?;
    conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
    let mut px = vec![0u8; (FAV * FAV * 4) as usize];
    conv.CopyPixels(std::ptr::null(), FAV * 4, &mut px)?;
    Ok(px)
}

// ---- brightness ----

/// DDC/CI brightness of the primary monitor's screens on a worker thread (each call takes ~50 ms).
/// Send it 0..1 levels; while it's busy, only the newest one is applied.
/// shortcut: primary monitor only, bound at startup; laptop panels (WMI) aren't handled.
pub fn start_ddc(hwnd: HWND) -> Sender<f32> {
    let (tx, rx) = channel::<f32>();
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let mon = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        let mut n = 0;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(mon, &mut n).is_err() || n == 0 {
            return;
        }
        let mut pms = vec![PHYSICAL_MONITOR::default(); n as usize];
        if GetPhysicalMonitorsFromHMONITOR(mon, &mut pms).is_err() {
            return;
        }
        let (mut min, mut cur, mut max) = (0, 0, 0);
        if GetMonitorBrightness(pms[0].hPhysicalMonitor, &mut min, &mut cur, &mut max) != 0 && max > min {
            let level = (cur - min) as f32 / (max - min) as f32;
            let _ = PostMessageW(HWND(hwnd as _), WM_BRIGHT, WPARAM((level * 10000.0) as usize), LPARAM(1));
            while let Ok(mut t) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    t = newer;
                }
                let v = min + (t.clamp(0.0, 1.0) * (max - min) as f32).round() as u32;
                for pm in &pms {
                    SetMonitorBrightness(pm.hPhysicalMonitor, v);
                }
            }
        }
        let _ = DestroyPhysicalMonitors(&pms);
    });
    tx
}

// ---- focus ----

/// Posts WM_FOCUS whenever Windows Focus turns on or off. Reading Focus is open to any app;
/// starting or ending it is a Limited Access Feature, so the island only observes it.
pub fn watch_focus(hwnd: HWND) -> Option<(FocusSessionManager, EventRegistrationToken)> {
    if !FocusSessionManager::IsSupported().unwrap_or(false) {
        return None;
    }
    let m = FocusSessionManager::GetDefault().ok()?;
    let h = hwnd.0 as isize;
    let post = move |active: bool, silent: bool| unsafe {
        let _ = PostMessageW(HWND(h as _), WM_FOCUS, WPARAM(active as usize), LPARAM(silent as isize));
    };
    let tok = m
        .IsFocusActiveChanged(&TypedEventHandler::new(move |s: &Option<FocusSessionManager>, _| {
            if let Some(s) = s {
                post(s.IsFocusActive()?, false);
            }
            Ok(())
        }))
        .ok()?;
    post(m.IsFocusActive().unwrap_or(false), true);
    Some((m, tok))
}

// ---- glyphs (24-unit box centred on cx, cy, scaled by k) ----

impl App {
    pub unsafe fn moon(&self, cx: f32, cy: f32, k: f32, a: f32) {
        let (dc, br) = (&self.g.dc, &self.g.brush);
        let c = |x: f32, y: f32, r: f32| D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx + (x - 12.0) * k, y: cy + (y - 12.0) * k }, radiusX: r * k, radiusY: r * k };
        let (r, g, b) = FOCUS_TINT;
        br.SetColor(&rgba(r, g, b, a));
        dc.FillEllipse(&c(12.0, 12.0, 8.0), br);
        br.SetColor(&bg(a)); // bite out of the disc on the black pill
        dc.FillEllipse(&c(16.5, 8.0, 7.0), br);
    }

    pub unsafe fn globe(&self, cx: f32, cy: f32, k: f32, a: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        let p = |x: f32, y: f32| D2D_POINT_2F { x: cx + (x - 12.0) * k, y: cy + (y - 12.0) * k };
        br.SetColor(&fg(a));
        dc.DrawEllipse(&D2D1_ELLIPSE { point: p(12.0, 12.0), radiusX: 8.5 * k, radiusY: 8.5 * k }, br, 1.6 * k, None);
        dc.DrawEllipse(&D2D1_ELLIPSE { point: p(12.0, 12.0), radiusX: 3.8 * k, radiusY: 8.5 * k }, br, 1.4 * k, None);
        dc.DrawLine(p(3.5, 12.0), p(20.5, 12.0), br, 1.4 * k, caps);
    }

    pub unsafe fn headphones(&self, cx: f32, cy: f32, k: f32, a: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        let p = |x: f32, y: f32| D2D_POINT_2F { x: cx + (x - 12.0) * k, y: cy + (y - 12.0) * k };
        br.SetColor(&fg(a));
        // Band: a half ellipse over the top, as two quarter arcs drawn with lines for simplicity.
        let mut prev = p(4.5, 14.0);
        for i in 1..=16 {
            let t = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
            let next = p(12.0 + 7.5 * t.cos(), 14.0 + 9.0 * t.sin());
            dc.DrawLine(prev, next, br, 1.8 * k, caps);
            prev = next;
        }
        for x in [3.0, 16.5] {
            let o = p(x, 12.5);
            dc.FillRoundedRectangle(&rrect(o.x, o.y, 4.5 * k, 8.0 * k, 2.0 * k), br);
        }
    }

    /// USB plug, pointing up. `plugged`: slides up into place; pulled out: drops away.
    pub unsafe fn usb_icon(&self, cx: f32, cy: f32, k: f32, a: f32, plugged: bool, t: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        let dy = if plugged { 3.0 * (1.0 - t) } else { -3.0 * (1.0 - t) };
        let p = |x: f32, y: f32| D2D_POINT_2F { x: cx + (x - 12.0) * k, y: cy + (y - 12.0 + dy) * k };
        let rr = |x: f32, y: f32, w: f32, h: f32, r: f32| {
            let o = p(x, y);
            rrect(o.x, o.y, w * k, h * k, r * k)
        };
        br.SetColor(&fg(a));
        dc.DrawRoundedRectangle(&rr(8.5, 3.0, 7.0, 6.5, 0.8), br, 1.5 * k, None); // metal tip
        dc.FillRectangle(&rect(p(10.0, 5.0).x, p(10.0, 5.0).y, 1.4 * k, 1.4 * k), br);
        dc.FillRectangle(&rect(p(12.6, 5.0).x, p(12.6, 5.0).y, 1.4 * k, 1.4 * k), br);
        dc.FillRoundedRectangle(&rr(7.0, 9.5, 10.0, 8.0, 2.2), br); // grip
        dc.DrawLine(p(12.0, 17.5), p(12.0, 21.5), br, 1.8 * k, caps); // cable
    }

    /// Down arrow in a ring: the ring spins in quarter turns while downloading, closes when done.
    pub unsafe fn download_icon(&self, cx: f32, cy: f32, k: f32, a: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        let done = self.dl.as_ref().is_some_and(|d| d.done);
        let r = 9.5 * k;
        let ring = D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: r, radiusY: r };
        let (gr, gg, gb) = fg_rgb();
        br.SetColor(&rgba(gr, gg, gb, a * if done { 1.0 } else { 0.25 }));
        dc.DrawEllipse(&ring, br, 2.2 * k, None);
        if !done {
            // A quarter arc, rotated by the spin spring.
            br.SetColor(&fg(a));
            dc.SetTransform(&Matrix3x2::rotation(self.dl_spin.x * 360.0, cx, cy));
            self.arc(cx, cy, r, 0.28, 2.2 * k);
            dc.SetTransform(&Matrix3x2::identity());
        }
        br.SetColor(&rgba(gr, gg, gb, a));
        let p = |x: f32, y: f32| D2D_POINT_2F { x: cx + x * k, y: cy + y * k };
        dc.DrawLine(p(0.0, -4.5), p(0.0, 4.0), br, 2.0 * k, caps);
        dc.DrawLine(p(-3.5, 0.8), p(0.0, 4.3), br, 2.0 * k, caps);
        dc.DrawLine(p(3.5, 0.8), p(0.0, 4.3), br, 2.0 * k, caps);
    }

    /// Sun whose rays lengthen with `level`.
    pub unsafe fn sun(&self, cx: f32, cy: f32, k: f32, a: f32, level: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        br.SetColor(&fg(a));
        dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: 4.2 * k, radiusY: 4.2 * k }, br);
        let len = 1.2 + 2.2 * clamp01(level);
        for i in 0..8 {
            let ang = i as f32 * std::f32::consts::FRAC_PI_4;
            let (c, s) = (ang.cos() * k, ang.sin() * k);
            dc.DrawLine(D2D_POINT_2F { x: cx + c * 6.6, y: cy + s * 6.6 }, D2D_POINT_2F { x: cx + c * (6.6 + len), y: cy + s * (6.6 + len) }, br, 1.8 * k, caps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01);
        assert!(near(parse_color("#ff9f0a").unwrap(), [1.0, 0.623, 0.039, 1.0]));
        assert!(near(parse_color(" #F00 ").unwrap(), [1.0, 0.0, 0.0, 1.0]));
        assert!(near(parse_color("#00ff0080").unwrap(), [0.0, 1.0, 0.0, 0.5]));
        assert!(near(parse_color("rgb(255, 0, 0)").unwrap(), [1.0, 0.0, 0.0, 1.0]));
        assert!(near(parse_color("rgba(0 0 255 / 50%)").unwrap(), [0.0, 0.0, 1.0, 0.5]));
        assert!(near(parse_color("hsl(120, 100%, 50%)").unwrap(), [0.0, 1.0, 0.0, 1.0]));
        assert!(parse_color("#ggg").is_none());
        assert!(parse_color("decade").is_none());
        assert!(parse_color("rgb(1,2)").is_none());
    }

    #[test]
    fn links() {
        assert_eq!(parse_link("https://www.github.com/a?b").unwrap().1, "github.com");
        assert_eq!(parse_link("http://user:pw@localhost:3000/x").unwrap().1, "localhost");
        assert!(parse_link("see https://x.com").is_none());
        assert!(parse_link("ftp://x.com").is_none());
        assert!(parse_link("javascript:alert(1)").is_none());
        assert!(parse_link("https://evil.example\\@bank.com/login").is_none());
        assert_eq!(favicon_host("HTTPS://user:pw@GitHub.com:8443/a").as_deref(), Some("github.com"));
        for url in ["http://github.com", "https://localhost:3000", "https://192.168.1.1/x", "https://127.1", "https://router", "https://nas.local", "https://[::1]/"] {
            assert!(favicon_host(url).is_none(), "{url}");
        }
    }
}
