// Synced lyrics from LRCLIB (lrclib.net, free, no account). Off by default: turning it on sends
// each track's title, artist and length there. Shows the current line under the player.
use super::*;
use windows::Data::Json::{JsonArray, JsonObject};
use windows::Foundation::Uri;
use windows::Web::Http::HttpClient;

pub const WM_LYRICS: u32 = 0x800D; // lparam = Box<(String track key, Vec<(f64, String)>)>
pub const LYRICS_H: f32 = 26.0; // extra player height for the lyric line
pub const COMPACT_LY: (f32, f32, f32) = (250.0, 58.0, 22.0); // compact pill with its lyric row

pub type Lines = Vec<(f64, String)>; // (start second, text), sorted

/// Fetches on a worker thread and posts WM_LYRICS (an empty list when there are none).
pub fn fetch(hwnd: HWND, key: String, title: String, artist: String, dur: f64) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let lines = get(&title, &artist, dur).unwrap_or_default();
        let msg = Box::into_raw(Box::new((key, lines)));
        if PostMessageW(HWND(hwnd as _), WM_LYRICS, WPARAM(0), LPARAM(msg as isize)).is_err() {
            drop(Box::from_raw(msg));
        }
    });
}

fn get(title: &str, artist: &str, dur: f64) -> Result<Lines> {
    let esc = |s: &str| Uri::EscapeComponent(&HSTRING::from(s)).map(|h| h.to_string());
    let (t, a) = (esc(title)?, esc(artist)?);
    let http = HttpClient::new()?;
    let fetch = |url: String| -> Result<String> { Ok(http.GetStringAsync(&Uri::CreateUri(&HSTRING::from(url))?)?.get()?.to_string()) };
    // Exact match first (title, artist, length within a few seconds), then a looser search.
    let mut url = format!("https://lrclib.net/api/get?track_name={t}&artist_name={a}");
    if dur > 0.0 {
        url += &format!("&duration={}", dur.round());
    }
    if let Ok(body) = fetch(url) {
        if let Ok(s) = JsonObject::Parse(&HSTRING::from(body)).and_then(|o| o.GetNamedString(h!("syncedLyrics"))) {
            return Ok(parse_lrc(&s.to_string()));
        }
    }
    let body = fetch(format!("https://lrclib.net/api/search?track_name={t}&artist_name={a}"))?;
    let arr = JsonArray::Parse(&HSTRING::from(body))?;
    for i in 0..arr.Size()? {
        if let Ok(s) = arr.GetObjectAt(i).and_then(|o| o.GetNamedString(h!("syncedLyrics"))) {
            return Ok(parse_lrc(&s.to_string()));
        }
    }
    Ok(Vec::new())
}

/// "[01:23.45] text" lines, possibly several stamps per line; empty lines become "♪".
pub fn parse_lrc(s: &str) -> Lines {
    let mut out = Vec::new();
    for line in s.lines() {
        let mut rest = line.trim();
        let mut stamps = Vec::new();
        while let Some(r) = rest.strip_prefix('[') {
            let Some((stamp, after)) = r.split_once(']') else { break };
            let Some((m, sec)) = stamp.split_once(':') else { break };
            let (Ok(m), Ok(sec)) = (m.parse::<f64>(), sec.parse::<f64>()) else { break };
            stamps.push(m * 60.0 + sec);
            rest = after.trim_start();
        }
        let text = if rest.is_empty() { "♪".to_string() } else { rest.to_string() };
        out.extend(stamps.into_iter().map(|t| (t, text.clone())));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

impl App {
    /// The player's expanded size: taller when there's a lyric line to show.
    pub fn player_size(&self) -> (f32, f32, f32) {
        if self.has_lyrics() { (EXPANDED.0, EXPANDED.1 + LYRICS_H, EXPANDED.2) } else { EXPANDED }
    }

    /// Lyrics are on and fetched for the track that's playing now.
    fn has_lyrics(&self) -> bool {
        let now = self.media.as_ref().map(Self::track_key);
        self.cfg.pills[P_LYRICS] && self.lyrics.as_ref().is_some_and(|(k, l)| !l.is_empty() && now.as_ref() == Some(k))
    }

    fn track_key(m: &Media) -> String {
        format!("{}\u{1}{}", m.title, m.artist)
    }

    /// Asks for the current track's lyrics if they're on and not already fetched.
    pub fn want_lyrics(&mut self) {
        let Some(m) = self.media.as_ref().filter(|_| self.cfg.pills[P_LYRICS]) else { return };
        let key = Self::track_key(m);
        if self.lyrics.as_ref().is_some_and(|(k, _)| *k == key) || self.lyrics_pending == key || m.title.is_empty() {
            return;
        }
        self.lyrics_pending = key.clone();
        fetch(self.hwnd, key, m.title.clone(), m.artist.clone(), m.dur);
    }

    pub fn on_lyrics(&mut self, key: String, lines: Lines) {
        if self.media.as_ref().map(Self::track_key).as_ref() != Some(&key) {
            // The track changed while fetching: make sure the one playing now gets asked for.
            if self.lyrics_pending == key {
                self.lyrics_pending.clear();
            }
            return self.want_lyrics();
        }
        self.lyrics_pending.clear();
        self.lyrics = Some((key, lines));
        self.lyric_idx = None;
        if self.view == View::Player {
            self.set_view(View::Player); // grow or shrink for the lyric line
        }
    }

    /// Keeps the shown line in step with playback; a change slides the new line in.
    pub fn lyrics_step(&mut self, dt: f32) {
        self.lyric_in.step(dt, 0.4, 1.0);
        // The compact pill grows a lyric row while there are lyrics (and it's showing music).
        self.ly_c.to = (self.has_lyrics() && self.tc.to < 0.5) as u8 as f32;
        self.ly_c.step(dt, 0.42, 0.75);
        let Some((_, lines)) = self.lyrics.as_ref().filter(|_| self.has_lyrics()) else { return };
        let pos = self.pos() + 0.25; // a hair early reads as on time
        let idx = lines.iter().rposition(|(t, _)| *t <= pos);
        if idx != self.lyric_idx {
            self.lyric_prev = self.lyric_idx;
            self.lyric_idx = idx;
            self.lyric_in = Spring { x: 0.0, v: 0.0, to: 1.0 };
        }
    }

    /// Lyric line under the expanded player.
    pub unsafe fn draw_lyrics(&self, l: &Layout, a: f32) {
        self.lyric_row(l.x + 20.0, l.w - 40.0, l.y + EXPANDED.1 + LYRICS_H / 2.0 - 6.0, a, &self.g.f_lyric);
    }

    /// Lyric row inside the compact pill, under the song.
    pub unsafe fn draw_lyrics_compact(&self, l: &Layout, a: f32) {
        self.lyric_row(l.x + 14.0, l.w - 28.0, l.y + COMPACT.1 + 6.0, a * clamp01(self.ly_c.x), &self.g.f_lyric_s);
    }

    /// The current line centred in (x, w) at `cy`: the old line drifts up and out, the new one
    /// rises in dim, and a highlight sweeps across it word by word as it's sung.
    unsafe fn lyric_row(&self, x: f32, w: f32, cy: f32, a: f32, f: &IDWriteTextFormat) {
        let Some((_, lines)) = self.lyrics.as_ref().filter(|_| self.has_lyrics()) else { return };
        if a <= 0.0 {
            return;
        }
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let opts = D2D1_DRAW_TEXT_OPTIONS_NONE;
        let k = clamp01(self.lyric_in.x);
        let layout = |text: &[u16]| {
            let lay = g.dw.CreateTextLayout(text, f, w, 20.0).ok()?;
            let _ = lay.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP); // one row; long lines trim
            Some(lay)
        };
        if let Some((_, text)) = self.lyric_prev.and_then(|i| lines.get(i)) {
            let t: Vec<u16> = text.encode_utf16().collect();
            if let Some(lay) = layout(&t) {
                br.SetColor(&fg(a * (1.0 - k)));
                dc.DrawTextLayout(D2D_POINT_2F { x, y: cy - 10.0 - 10.0 * k }, &lay, br, opts);
            }
        }
        let Some((t0, text)) = self.lyric_idx.and_then(|i| lines.get(i)) else { return };
        let t: Vec<u16> = text.encode_utf16().collect();
        let Some(lay) = layout(&t) else { return };
        let origin = D2D_POINT_2F { x, y: cy - 10.0 + 10.0 * (1.0 - k) };
        br.SetColor(&fg(a * k * 0.35));
        dc.DrawTextLayout(origin, &lay, br, opts);
        let next = lines.get(self.lyric_idx.unwrap() + 1).map_or(t0 + 5.0, |n| n.0);
        let sx = sweep_x(&lay, &t, sung(*t0, next, t.len(), self.pos()));
        dc.PushAxisAlignedClip(&rect(origin.x, origin.y - 4.0, sx, 28.0), D2D1_ANTIALIAS_MODE_ALIASED);
        br.SetColor(&fg(a * k));
        dc.DrawTextLayout(origin, &lay, br, opts);
        dc.PopAxisAlignedClip();
    }
}

/// How many characters of a line are sung by `pos`, as a fraction 0..1. Lines only carry a start
/// time, so it's spread evenly over the line's time, capped so a line before a long gap still
/// finishes at singing pace. shortcut: even pace; word-timed LRC (A2 `<mm:ss>` tags) would be exact.
fn sung(t0: f64, next: f64, chars: usize, pos: f64) -> f32 {
    let dur = (next - t0).min(chars as f64 * 0.09 + 0.6).max(0.2);
    ((pos + 0.25 - t0) / dur).clamp(0.0, 1.0) as f32
}

/// x offset in the layout where the highlight ends: through whole words already sung, then
/// smoothly across the word being sung.
unsafe fn sweep_x(layout: &IDWriteTextLayout, w: &[u16], frac: f32) -> f32 {
    let at = |i: usize| {
        let (mut x, mut y, mut m) = (0.0, 0.0, DWRITE_HIT_TEST_METRICS::default());
        let _ = layout.HitTestTextPosition(i as u32, false, &mut x, &mut y, &mut m);
        x
    };
    if frac >= 1.0 || w.is_empty() {
        return at(w.len().saturating_sub(1)) + 40.0;
    }
    let f = frac * w.len() as f32;
    let i = f.floor() as usize;
    lerp(at(i), at(i + 1), f.fract())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc() {
        let l = parse_lrc("[ar: x]\n[00:12.50] Hello\n[01:02.00][00:05.00] Chorus\n[00:20.00]\n");
        assert_eq!(l, vec![(5.0, "Chorus".into()), (12.5, "Hello".into()), (20.0, "♪".into()), (62.0, "Chorus".into())]);
    }

    #[test]
    fn pace() {
        assert_eq!(sung(10.0, 14.0, 20, 9.0), 0.0);
        assert_eq!(sung(10.0, 14.0, 20, 20.0), 1.0);
        // 20 chars sing in 2.4 s even with a 30 s gap before the next line.
        assert!((sung(10.0, 40.0, 20, 11.2 - 0.25) - 0.5).abs() < 1e-3);
    }

    #[test]
    #[ignore] // network
    fn fetch_live() {
        let r = get("ot tebya pret", "koonki", 999.0);
        println!("{:?}", r.as_ref().map(|l| l.len()));
        assert!(r.unwrap().len() > 5);
    }
}
