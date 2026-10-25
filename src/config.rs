// User settings, saved as plain `key=value` lines in %APPDATA%\DynamicIsland\settings.ini.
use std::path::PathBuf;

pub const SIZES: [(&str, f32); 3] = [("Compact", 0.88), ("Default", 1.0), ("Large", 1.15)];
/// Animation presets: (name, speed = response multiplier, bounce 0..1; 0.5 = the hand-tuned defaults).
pub const FEELS: [(&str, f32, f32); 4] = [("Snappy", 0.75, 0.3), ("iOS", 1.0, 0.5), ("Bouncy", 1.05, 0.85), ("Calm", 1.4, 0.12)];
/// Colour themes: (name, [island, text, accent]). The last one is built from `Config::custom`.
pub const THEMES: [(&str, [[f32; 3]; 3]); 6] = [
    ("Dark", [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1.0, 1.0, 1.0]]),
    ("Light", [[0.96, 0.96, 0.97], [0.07, 0.07, 0.08], [0.0, 0.48, 1.0]]),
    ("Midnight", [[0.04, 0.06, 0.16], [0.86, 0.9, 1.0], [0.42, 0.6, 1.0]]),
    ("Rose", [[0.16, 0.05, 0.09], [1.0, 0.9, 0.93], [1.0, 0.42, 0.6]]),
    ("Forest", [[0.04, 0.12, 0.07], [0.88, 1.0, 0.9], [0.38, 0.86, 0.5]]),
    ("Custom", [[0.0; 3]; 3]),
];
pub const CUSTOM: usize = 5;
pub const MUSIC_MODES: [&str; 4] = ["All apps", "Spotify", "No browsers", "Choose"];

/// Pill switches, in the order the Pills page lists them.
pub const PILLS: [&str; 13] = ["Music", "Timer", "Copied", "Screenshot", "Colour", "Link", "Volume", "Brightness", "Focus", "Downloads", "Devices", "Lyrics", "Updates"];
pub const P_MUSIC: usize = 0;
pub const P_TIMER: usize = 1;
pub const P_COPIED: usize = 2;
pub const P_SHOT: usize = 3;
pub const P_COLOUR: usize = 4;
pub const P_LINK: usize = 5;
pub const P_VOLUME: usize = 6;
#[allow(dead_code)] // the wheel no longer sets brightness; kept so saved pill indices stay put
pub const P_BRIGHT: usize = 7;
pub const P_FOCUS: usize = 8;
pub const P_DOWNLOADS: usize = 9;
pub const P_EARBUDS: usize = 10;
pub const P_LYRICS: usize = 11; // off by default: sends track names to lrclib.net
pub const P_UPDATES: usize = 12; // off: no update pill and no check against GitHub at all

/// Things that compete for the island, for the Priority page. Lower rank wins.
pub const KINDS: [&str; 7] = ["Volume", "Copied", "Downloads", "Earbuds", "Focus", "Timer", "Music"];
pub const K_VOLUME: u8 = 0;
pub const K_COPIED: u8 = 1;
pub const K_DOWNLOADS: u8 = 2;
pub const K_EARBUDS: u8 = 3;
pub const K_FOCUS: u8 = 4;
pub const K_TIMER: u8 = 5;
pub const K_MUSIC: u8 = 6;

#[derive(Clone, PartialEq)]
pub struct Config {
    pub size: usize,  // index into SIZES
    pub round: f32,   // 0.3 (squarish) .. 1.0 (capsule) times each shape's corner radius
    pub top: f32,     // DIPs from the top of the screen, 0..40
    pub notch: bool,  // flush with the screen top, square top corners; ignores `top`
    pub display: i32, // -1 follows the mouse, 0 the main display, n the n-th other one
    pub speed: f32,   // spring response multiplier, 0.5..1.6
    pub bounce: f32,  // 0 none .. 1 a lot
    pub pills: [bool; 13],
    pub rank: [u8; 7], // kinds, best first
    pub music: usize,  // index into MUSIC_MODES
    pub apps: Vec<(String, bool)>, // media apps seen so far, and whether "Choose" allows them
    pub hide_fs: bool,             // hide while a fullscreen app covers the island's display
    pub mic_dot: bool,             // dot while another app records from a microphone
    pub welcomed: bool,            // the first-run welcome has been seen
    pub theme: usize,              // index into THEMES
    pub custom: [[f32; 2]; 3],     // Custom theme: (hue, lightness) for island, text, accent
    pub cover_tint: bool,          // island takes on the cover colour while a song plays
    pub tint: f32,                 // how much of it, 0..1
}

impl Default for Config {
    fn default() -> Self {
        Config {
            size: 1,
            round: 1.0,
            top: 8.0,
            notch: false,
            display: 0,
            speed: 1.0,
            bounce: 0.5,
            pills: std::array::from_fn(|i| i != P_LYRICS),
            rank: [0, 1, 2, 3, 4, 5, 6],
            music: 0,
            apps: Vec::new(),
            hide_fs: true,
            mic_dot: true,
            welcomed: false,
            theme: 0,
            custom: [[0.62, 0.1], [0.62, 0.95], [0.55, 0.6]],
            cover_tint: false,
            tint: 0.45,
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("DynamicIsland").join("settings.ini"))
}

impl Config {
    pub fn feel(&self) -> Option<usize> {
        FEELS.iter().position(|f| (f.1 - self.speed).abs() < 0.01 && (f.2 - self.bounce).abs() < 0.01)
    }

    /// [island, text, accent] colours of the current theme.
    pub fn colors(&self) -> [[f32; 3]; 3] {
        match self.theme {
            CUSTOM => self.custom.map(|[h, l]| hsl(h, l)),
            t => THEMES[t].1,
        }
    }

    /// Defaults, but keeps the list of players seen (all allowed again).
    pub fn reset(&self) -> Config {
        Config { apps: self.apps.iter().map(|(id, _)| (id.clone(), true)).collect(), ..Config::default() }
    }

    pub fn rank_of(&self, kind: u8) -> usize {
        self.rank.iter().position(|&k| k == kind).unwrap_or(0)
    }

    /// Unknown keys and bad values are skipped, so an old or hand-edited file still loads.
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let f = |lo: f32, hi: f32| v.parse::<f32>().ok().filter(|x| x.is_finite()).map(|x| x.clamp(lo, hi));
            match k {
                "size" => c.size = v.parse::<usize>().ok().filter(|&s| s < SIZES.len()).unwrap_or(c.size),
                "round" => c.round = f(0.3, 1.0).unwrap_or(c.round),
                "top" => c.top = f(0.0, 40.0).unwrap_or(c.top),
                "display" => c.display = v.parse::<i32>().ok().filter(|&d| d >= -1).unwrap_or(c.display),
                "speed" => c.speed = f(0.5, 1.6).unwrap_or(c.speed),
                "bounce" => c.bounce = f(0.0, 1.0).unwrap_or(c.bounce),
                "notch" => c.notch = v == "1",
                "hide_fs" => c.hide_fs = v == "1",
                "mic_dot" => c.mic_dot = v == "1",
                "cover_tint" => c.cover_tint = v == "1",
                "tint" => c.tint = f(0.0, 1.0).unwrap_or(c.tint),
                "welcomed" => c.welcomed = v == "1",
                "theme" => c.theme = v.parse::<usize>().ok().filter(|&t| t < THEMES.len()).unwrap_or(c.theme),
                "custom" => {
                    let n: Vec<f32> = v.split(',').filter_map(|x| x.parse::<f32>().ok()).filter(|x| x.is_finite()).collect();
                    if n.len() == 6 {
                        for (i, c2) in c.custom.iter_mut().enumerate() {
                            *c2 = [n[2 * i].clamp(0.0, 1.0), n[2 * i + 1].clamp(0.0, 1.0)];
                        }
                    }
                }
                "music" => c.music = v.parse::<usize>().ok().filter(|&m| m < MUSIC_MODES.len()).unwrap_or(c.music),
                "pills" if v.len() <= PILLS.len() => {
                    // Older files have fewer switches; the newer ones keep their defaults.
                    for (p, ch) in c.pills.iter_mut().zip(v.chars()) {
                        *p = ch == '1';
                    }
                }
                "rank" => {
                    let r: Vec<u8> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                    // Must be a permutation of all kinds.
                    let mut sorted = r.clone();
                    sorted.sort();
                    if sorted == (0..KINDS.len() as u8).collect::<Vec<_>>() {
                        c.rank.copy_from_slice(&r);
                    }
                }
                "app" => {
                    if let Some((id, on)) = v.rsplit_once('|') {
                        c.apps.push((id.to_string(), on == "1"));
                    }
                }
                _ => {}
            }
        }
        c
    }

    pub fn load() -> Config {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Config::default, |t| Config::parse(&t))
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, self.to_text());
    }

    fn to_text(&self) -> String {
        let b = |x: bool| if x { "1" } else { "0" };
        let mut s = format!(
            "size={}\nround={}\ntop={}\nnotch={}\ndisplay={}\nspeed={}\nbounce={}\nmusic={}\npills={}\nrank={}\ntheme={}\ncustom={}\nhide_fs={}\nmic_dot={}\nwelcomed={}\ncover_tint={}\ntint={}\n",
            self.size,
            self.round,
            self.top,
            b(self.notch),
            self.display,
            self.speed,
            self.bounce,
            self.music,
            self.pills.iter().map(|&p| b(p)).collect::<String>(),
            self.rank.iter().map(|k| k.to_string()).collect::<Vec<_>>().join(","),
            self.theme,
            self.custom.iter().flatten().map(|x| x.to_string()).collect::<Vec<_>>().join(","),
            b(self.hide_fs),
            b(self.mic_dot),
            b(self.welcomed),
            b(self.cover_tint),
            self.tint,
        );
        for (id, on) in &self.apps {
            s += &format!("app={id}|{}\n", b(*on));
        }
        s
    }
}

/// Colour from hue and lightness (0..1) at a fixed saturation.
/// shortcut: no saturation control; lightness near 0 or 1 still gives black or white.
pub fn hsl(h: f32, l: f32) -> [f32; 3] {
    let a = 0.7 * l.min(1.0 - l);
    let f = |n: f32| {
        let k = (n + h * 12.0) % 12.0;
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

/// Which media apps a mode lets through.
pub fn music_allows(mode: usize, apps: &[(String, bool)], id: &str) -> bool {
    let l = id.to_ascii_lowercase();
    // Firefox registers under a hash AUMID.
    let browser = ["chrome", "msedge", "brave", "firefox", "308046b0af4a39cb", "opera", "vivaldi", "zen", "arc"].iter().any(|b| l.contains(b));
    match mode {
        1 => l.contains("spotify"),
        2 => !browser,
        3 => apps.iter().find(|(a, _)| a == id).map_or(true, |(_, on)| *on), // new apps start allowed
        _ => true,
    }
}

/// Short readable name for a media app id, e.g. "Spotify.exe" -> "Spotify".
pub fn app_name(id: &str) -> String {
    let base = id.rsplit(['\\', '!']).next().unwrap_or(id);
    let base = base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base);
    let l = id.to_ascii_lowercase();
    match () {
        _ if l.contains("308046b0af4a39cb") => "Firefox".into(),
        _ if l.contains("spotify") => "Spotify".into(),
        _ if l.contains("msedge") => "Edge".into(),
        _ => {
            let mut c = base.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut c = Config::default();
        c.size = 2;
        c.round = 0.5;
        c.notch = true;
        c.display = -1;
        c.pills[P_LINK] = false;
        c.rank = [5, 6, 0, 1, 2, 3, 4];
        c.music = 3;
        c.apps = vec![("Spotify.exe".into(), true), ("chrome".into(), false)];
        c.theme = CUSTOM;
        c.hide_fs = false;
        c.mic_dot = false;
        c.welcomed = true;
        c.cover_tint = true;
        c.tint = 0.75;
        c.custom = [[0.5, 0.25], [0.1, 0.9], [0.0, 0.5]];
        assert!(Config::parse(&c.to_text()) == c);
        // Bad values fall back to defaults.
        let bad = Config::parse("size=9\nround=nan\nrank=1,1,2\ntop=500\n");
        assert_eq!(bad.size, 1);
        assert_eq!(bad.round, 1.0);
        assert_eq!(bad.rank, [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(bad.top, 40.0);
    }

    #[test]
    fn music_filter() {
        assert!(music_allows(1, &[], "Spotify.exe"));
        assert!(!music_allows(1, &[], "chrome"));
        assert!(!music_allows(2, &[], "308046B0AF4A39CB"));
        assert!(music_allows(2, &[], "Spotify.exe"));
        assert!(!music_allows(3, &[("vlc.exe".into(), false)], "vlc.exe"));
        assert!(music_allows(3, &[], "vlc.exe"));
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01);
        assert!(near(hsl(0.0, 0.5), [0.85, 0.15, 0.15]));
        assert!(near(hsl(1.0 / 3.0, 0.5), [0.15, 0.85, 0.15]));
        assert!(near(hsl(0.3, 1.0), [1.0, 1.0, 1.0]));
    }
}
