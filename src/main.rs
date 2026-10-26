#![windows_subsystem = "windows"]
mod buds;
mod calc;
mod clips;
mod config;
mod downloads;
mod extras;
mod lyrics;
mod media;
mod settings;
mod shelf;
mod shot;
mod picker;
mod system;
mod welcome;
mod timer;
mod updater;
mod usb;
mod control;
mod mixer;
mod ai;
mod calendar;

use media::{Cmd, Media, Shared, ART, WM_MEDIA};
use timer::{Timer, View, WheelDrag, T_DONE, T_TICK};
use extras::{FAV, WM_BRIGHT, WM_FAVICON, WM_FOCUS};
use buds::WM_BUDS;
use usb::WM_USB;
use control::WM_RADIO;
use ai::WM_AI;
use lyrics::WM_LYRICS;
use shelf::{WM_SHELF_ADD, WM_SHELF_HOVER};
use picker::WM_PICK;
use updater::{T_UPDATE, WM_UPDATE};
use system::WM_FULLSCREEN;
use config::*;
use settings::{accent, bg, fg, fg_rgb, surface, surface_rgb, SetUi};
use downloads::{DlEvent, Download, T_DL, WM_DL, WM_DRAG};
use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::sync::mpsc::Sender;
use std::time::Instant;
use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Registry::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTimePreciseAsFileTime};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DragQueryFileW, ShellExecuteW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

// Window canvas in DIPs; the island is drawn top-centered inside it.
const WIDE: f32 = 400.0; // width of every expanded view
const CW: f32 = WIDE + 40.0;
const CH: f32 = 330.0; // fits the tallest view (timer picker, settings) at the largest top gap
// Island sizes (w, h, radius): resting pill, compact music, expanded music.
const IDLE: (f32, f32, f32) = (150.0, 40.0, 20.0); // tall enough for the clock and date
const COMPACT: (f32, f32, f32) = (180.0, 36.0, 18.0);
const EXPANDED: (f32, f32, f32) = (WIDE, 164.0, 40.0);
const NOTCH_EAR: f32 = 8.0; // radius of the notch style's outward curves at the screen edge
const TOPBAR: f32 = 22.0; // expanded view's icon row (timer, mic, settings) above the player

const WM_MOUSELEAVE: u32 = 0x02A3;
const T_HOVER: usize = 1;
const T_LEAVE: usize = 2;
const T_CLIP: usize = 3;
const T_MON: usize = 8; // follow-the-mouse display check
const T_CLOCK: usize = 9; // wakes the expanded island when the clock's minute turns
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
// "Copied" notice: (w, h, radius) and how long it stays.
const CLIP: (f32, f32, f32) = (210.0, 40.0, 20.0);
const CLIP_MS: u32 = 2200;
const SHOT_MS: u32 = 4000; // screenshots stay longer: there's something to click
// Volume pill: shown on any system volume change, stays while hovered or dragged.
const VOL: (f32, f32, f32) = (220.0, 40.0, 20.0);
const T_VOL: usize = 4;
const VOL_MS: u32 = 1600;
const WM_VOLUME: u32 = 0x8002; // WM_APP + 2: wparam = level * 10000, lparam = muted
const WM_MIC: u32 = 0x8003; // default mic mute changed: lparam = muted
const WM_MIC_USE: u32 = 0x8004; // some app started/stopped using the mic: wparam = in use
// Top-row button indices, after the transport controls (0..3).
const B_TIMER: usize = 3;
const B_MIC: usize = 4;
const B_GEAR: usize = 5;
const B_CLIPS: usize = 10; // after the timer views' buttons (6..9)
const B_SHELF: usize = 11;
const B_CALC: usize = 12;
const B_POWER: usize = 13;
const B_PICK: usize = 14; // colour picker
const B_MIXER: usize = 15; // Sound Mixer
const B_AI: usize = 16;
// State fades in `App::fx`: things that used to pop in or out now ease.
const FX_MIC: usize = 0; // recording dot
const FX_MOON: usize = 1; // Focus moon on the resting pill
const FX_MUTE: usize = 2; // speaker muted
const FX_MICMUTE: usize = 3; // mic muted
const FX_SHELF: usize = 4; // shelf holds files
const FX_TIMER: usize = 5; // a timer is running
const FX_N: usize = 6;

/// What the notice pill is showing.
#[derive(Clone, Copy, PartialEq)]
enum Note {
    Copy,
    Focus(bool),
    Download,
    Buds(Option<u8>), // battery %
    Usb(bool),        // plugged in (false: pulled out)
    Pick,             // colour picker: live swatch under the cursor
    Update,           // a newer release is available (src/updater.rs)
}
const ORANGE: (f32, f32, f32) = (1.0, 0.62, 0.04); // iOS system orange
const GREEN: (f32, f32, f32) = (0.2, 0.84, 0.29);

/// Critically-tunable spring; response in seconds, damping ratio <1 bounces.
#[derive(Clone, Copy, Default)]
struct Spring {
    x: f32,
    v: f32,
    to: f32,
}
impl Spring {
    fn step(&mut self, dt: f32, response: f32, damping: f32) {
        let (response, damping) = settings::feel(response, damping);
        let k = (std::f32::consts::TAU / response).powi(2);
        let c = 2.0 * damping * k.sqrt();
        let n = (dt / (1.0 / 240.0)).ceil().max(1.0);
        let h = dt / n;
        for _ in 0..n as usize {
            self.v += (-k * (self.x - self.to) - c * self.v) * h;
            self.x += self.v * h;
        }
        if self.settled() {
            self.x = self.to;
            self.v = 0.0;
        }
    }
    fn settled(&self) -> bool {
        (self.x - self.to).abs() < 1e-3 && self.v.abs() < 1e-3
    }
}
fn spring(x: f32) -> Spring {
    Spring { x, v: 0.0, to: x }
}

/// Seek bar ported from React Bits' ElasticSlider: thickens and brightens when active,
/// rubber-bands past either end while dragging, then wobbles back.
const SL_STRETCH: f32 = 18.0;
struct Slider {
    p: Spring, // fill fraction
    a: Spring, // active (hover/drag) 0..1
    s: Spring, // stretch in DIPs past an end
    side: f32, // +1 stretching right end, -1 left end
    held: bool,
    drag: bool,
    hover: bool,
}
/// Outgoing track, kept around while it animates out.
struct Prev {
    title: String,
    artist: String,
    art: Option<ID2D1BitmapBrush1>,
}

fn rubber(d: f32, limit: f32) -> f32 {
    2.0 * (1.0 / (1.0 + (-d / limit).exp()) - 0.5) * limit
}

impl Slider {
    fn new(p: f32) -> Self {
        Slider { p: spring(p), a: spring(0.0), s: spring(0.0), side: 1.0, held: false, drag: false, hover: false }
    }

    fn busy(&self) -> bool {
        self.drag || [self.p, self.a, self.s].iter().any(|s| !s.settled())
    }

    /// Pointer x -> fill target; past either end the track rubber-bands instead.
    fn follow(&mut self, x: f32, bx: f32, len: f32) {
        let f = (x - bx) / len;
        let over = if f < 0.0 { -f * len } else if f > 1.0 { (f - 1.0) * len } else { 0.0 };
        if over > 0.0 {
            self.held = true;
            self.side = if f < 0.0 { -1.0 } else { 1.0 };
            self.s.x = rubber(over, SL_STRETCH);
            self.s.v = 0.0;
        } else {
            self.held = false;
        }
        self.p.to = clamp01(f);
    }

    /// ElasticSlider's spring constants; `value` is what the fill shows when not dragged.
    fn step(&mut self, dt: f32, value: f32) {
        self.a.to = if self.hover || self.drag { 1.0 } else { 0.0 };
        self.a.step(dt, 0.32, 0.9);
        if self.drag {
            self.p.step(dt, 0.09, 1.0);
        } else {
            self.p.to = value;
            self.p.step(dt, 0.3, 0.86);
        }
        if !self.held {
            self.s.step(dt, 0.42, 1.0 - 0.9 * 0.5);
        }
    }

    /// Draws track and fill centered on `cy`. Returns how far the left/right ends are pushed out,
    /// so icons or labels beside the track can ride the stretch.
    unsafe fn draw(&self, dc: &ID2D1DeviceContext, br: &ID2D1SolidColorBrush, bx: f32, len: f32, cy: f32, op: f32, rest: f32) -> (f32, f32) {
        // Stretch scales the track along its length from the far end and squashes it across.
        let along = 1.0 + self.s.x / len;
        let across = 1.0 - 0.22 * (self.s.x / SL_STRETCH).clamp(-1.0, 1.0);
        let at_end = self.side > 0.0;
        let thick = (rest + 5.0 * self.a.x) * across; // grows by 5 when active
        let tl = len * along;
        let tx0 = if at_end { bx } else { bx + len - tl };
        br.SetColor(&fg(op * 0.2));
        dc.FillRoundedRectangle(&rrect(tx0, cy - thick / 2.0, tl, thick, thick / 2.0), br);
        let fw = clamp01(self.p.x) * tl;
        if fw > 0.5 {
            br.SetColor(&fg(op));
            dc.FillRoundedRectangle(&rrect(tx0, cy - thick / 2.0, fw, thick, (thick / 2.0).min(fw / 2.0)), br);
        }
        if at_end { (0.0, self.s.x) } else { (self.s.x, 0.0) }
    }
}

/// Core Audio calls this on its own thread whenever the endpoint volume or mute changes.
#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatch(isize, u32); // hwnd, message to post

impl IAudioEndpointVolumeCallback_Impl for VolumeWatch_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        unsafe {
            if let Some(d) = data.as_ref() {
                let level = (d.fMasterVolume.clamp(0.0, 1.0) * 10000.0) as usize;
                let _ = PostMessageW(HWND(self.0 as _), self.1, WPARAM(level), LPARAM(d.bMuted.as_bool() as isize));
            }
        }
        Ok(())
    }
}

/// Volume control of the default output (eRender) or mic (eCapture), with changes posted to `hwnd` as `msg`.
/// shortcut: binds the device at startup only; switching devices (e.g. to headphones) needs a restart until we watch device changes.
unsafe fn volume_endpoint(hwnd: HWND, flow: EDataFlow, msg: u32) -> Result<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let vol: IAudioEndpointVolume = en.GetDefaultAudioEndpoint(flow, eMultimedia)?.Activate(CLSCTX_ALL, None)?;
    let cb: IAudioEndpointVolumeCallback = VolumeWatch(hwnd.0 as isize, msg).into();
    vol.RegisterControlChangeNotify(&cb)?;
    Ok((vol, cb))
}

struct Gfx {
    swap: IDXGISwapChain1,
    dc: ID2D1DeviceContext,
    dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual, // scaled while the island appears or hides
    brush: ID2D1SolidColorBrush,
    tri: ID2D1PathGeometry1,
    caps: ID2D1StrokeStyle1, // round caps/joins for line icons
    mic_cradle: ID2D1PathGeometry1, // U under the mic capsule, 24-unit box
    speaker: ID2D1PathGeometry1,
    waves: [ID2D1PathGeometry1; 3],
    f_title_s: IDWriteTextFormat,
    f_artist_s: IDWriteTextFormat,
    f_title_l: IDWriteTextFormat,
    f_artist_l: IDWriteTextFormat,
    f_time: IDWriteTextFormat,
    f_time_r: IDWriteTextFormat,
    factory: ID2D1Factory1, // per-frame geometry (timer ring)
    dw: IDWriteFactory,     // per-frame text layouts (lyric highlight)
    f_wheel: IDWriteTextFormat,
    f_wheel_label: IDWriteTextFormat,
    f_btn: IDWriteTextFormat,
    f_big: IDWriteTextFormat,
    f_cd: IDWriteTextFormat,
    f_row: IDWriteTextFormat,
    f_hello: IDWriteTextFormat,  // welcome: "hello", centred (measures the word)
    f_hello_l: IDWriteTextFormat, // welcome: one letter of it, left-aligned
    f_wtitle: IDWriteTextFormat, // welcome page titles
    f_wbody: IDWriteTextFormat,  // welcome page text, wraps
    f_wbtn: IDWriteTextFormat,   // welcome buttons and subtitle, top-row clock
    f_seg: IDWriteTextFormat,
    f_chat: IDWriteTextFormat, // AI messages, wrapped
    f_line: IDWriteTextFormat, // AI input line
    f_calc: IDWriteTextFormat,
    f_calc_r: IDWriteTextFormat,
    f_lyric: IDWriteTextFormat,
    f_lyric_s: IDWriteTextFormat,
}

struct App {
    hwnd: HWND,
    g: Gfx,
    dpi: f32,     // render dpi: the monitor's, times the size preset
    sys_dpi: f32, // the monitor's
    cfg: Config,
    su: SetUi,
    mon: HMONITOR, // display the island is on when following the mouse
    filter: media::Filter,
    seen: media::Seen,
    shared: Shared,
    tx: Sender<Cmd>,
    media: Option<Media>,
    art: Option<(ID2D1BitmapBrush1, std::sync::Arc<media::Art>)>,
    accent: [f32; 3],
    // Animation state.
    music: Spring,  // 0 idle pill -> 1 music
    expand: Spring, // 0 compact -> 1 expanded
    wave: Spring,   // waveform amplitude, 0 when paused
    art_in: Spring, // art fade-in on track change
    clip: Spring,   // 0 -> 1 morphs into the "Copied" notice
    check: Spring,  // clipboard clip snap-down progress; starts below 0 as a delay
    clip_text: String,
    shot: Option<(ID2D1BitmapBrush1, f32, f32)>, // copied image thumbnail and its pixel size
    swatch: Option<[f32; 4]>,                    // copied colour
    link: Option<(String, String)>,              // copied (url, host)
    favicon: Option<ID2D1Bitmap1>,
    fav_gen: u64,                                // drops favicons that arrive for an older copy
    note: Note,
    focus_active: bool,
    history: clips::History,
    mouse: (f32, f32), // last pointer position in canvas DIPs
    fx: [Spring; FX_N], // state fades, see FX_*
    hot: [Spring; 17],  // top-row button hover, by button id
    cur: [Spring; 17],  // top-row button is the open view, by button id
    dl: Option<Download>,
    dl_spin: Spring,                 // in-progress ring rotation, in turns
    dl_press: Option<(f32, f32)>,    // pressed on a finished download: click opens, drag moves it out
    _focus: Option<(windows::UI::Shell::FocusSessionManager, windows::Foundation::EventRegistrationToken)>,
    bright: bool, // the level pill shows brightness instead of volume
    bright_level: Option<f32>, // None until the monitor answers over DDC/CI
    ddc: Sender<f32>,
    vol: Spring, // 0 -> 1 morphs into the volume pill
    vsl: Slider, // volume slider
    vol_level: f32,
    muted: bool,
    endpoint: Option<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)>,
    mic: Option<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)>,
    mic_muted: bool,
    mic_rec: bool, // some app is capturing from a microphone right now
    press: Spring,  // button press scale
    pp: Spring,     // play/pause morph: 0 shows play, 1 shows pause
    skip: [Spring; 2], // prev/next arrow roll, 0 -> 1 per click
    pressed: Option<usize>,
    sl: Slider,
    bump: Spring,   // elastic size kick on track change
    change: Spring, // 0 -> 1 crossfade from `prev` to the current track
    prev: Option<Prev>,
    hover: bool,
    last: Instant,
    t: f32,
    rgn: (i32, i32, i32, i32),
    // Timer.
    view: View,
    xd: [Spring; 3], // expanded (w, h, r) for the current view
    va: [Spring; 12], // per-view content opacity, indexed by View
    wpage: Spring,   // welcome page, fractional while turning
    w_t0: f32,       // when the current welcome page came in, in `t` seconds
    // File shelf.
    shelf: Vec<shelf::Item>,
    shelf_drop: Spring, // files hovering over the island
    shelf_press: Option<(f32, f32, usize)>, // pressed on a tile: click opens, drag moves it out
    drag: Option<(std::path::PathBuf, bool)>, // file to drag out next, and whether it's from the shelf
    // Calculator.
    calc_in: String,
    calc_prev: HWND, // had focus before the calculator took it
    calc_copied: Spring,
    // Lyrics.
    lyrics: Option<(String, lyrics::Lines)>, // for track key
    lyrics_pending: String,
    lyric_idx: Option<usize>,
    lyric_prev: Option<usize>,
    lyric_in: Spring,
    ly_c: Spring, // compact pill's lyric row, 0 -> 1
    hidden: bool, // out of the way of a fullscreen app
    update: Option<updater::Update>, // newer release found, not installed yet
    updating: bool,                  // its installer is downloading
    cc: control::Cc,                 // Control Center state
    mx: mixer::Mixer,                // Sound Mixer rows
    ai: ai::Ai,                      // AI chat
    cal: calendar::Cal,              // month the calendar shows
    clock_min: std::cell::Cell<u16>, // minute the top-row clock last drew, so a new one redraws
    shown: Spring, // 0 hidden -> 1 shown: the whole island pops in or shrinks away
    scaled: bool,  // the composition visual still carries a show/hide transform
    timer: Option<Timer>,
    tc: Spring, // compact pill shows the countdown instead of music
    wheel: [Spring; 3], // picker positions in rows: hours, min, sec
    wdrag: Option<WheelDrag>,
}

thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        OleInitialize(None)?; // STA COM plus OLE drag and drop (dragging downloads out)
        let inst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("DynamicIsland");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Dynamic Island"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            inst,
            None,
        )?;
        let cfg = Config::load();
        settings::set_feel(cfg.speed, cfg.bounce);
        settings::set_theme(cfg.colors());
        let sys_dpi = GetDpiForWindow(hwnd) as f32;
        let dpi = sys_dpi * SIZES[cfg.size].1;
        let g = gfx(hwnd, dpi)?;
        let filter: media::Filter = std::sync::Arc::new(std::sync::Mutex::new((cfg.music, cfg.apps.clone())));
        let (shared, tx, seen) = media::start(hwnd.0 as isize, filter.clone());
        let endpoint = volume_endpoint(hwnd, eRender, WM_VOLUME).ok();
        let mic = volume_endpoint(hwnd, eCapture, WM_MIC).ok();
        let mic_muted = mic.as_ref().is_some_and(|(m, _)| m.GetMute().is_ok_and(|b| b.as_bool()));
        watch_mic_use(hwnd);
        downloads::watch(hwnd);
        buds::watch(hwnd);
        usb::watch(hwnd);
        let (vol_level, muted) = endpoint.as_ref().map_or((0.0, false), |(v, _)| {
            (v.GetMasterVolumeLevelScalar().unwrap_or(0.0), v.GetMute().is_ok_and(|m| m.as_bool()))
        });
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                hwnd,
                g,
                dpi,
                sys_dpi,
                su: SetUi::new(&cfg),
                cfg,
                mon: MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
                filter,
                seen,
                shared,
                tx,
                media: None,
                art: None,
                accent: [1.0, 1.0, 1.0],
                music: spring(0.0),
                expand: spring(0.0),
                wave: spring(0.0),
                art_in: spring(1.0),
                clip: spring(0.0),
                check: spring(0.0),
                clip_text: String::new(),
                shot: None,
                swatch: None,
                link: None,
                favicon: None,
                fav_gen: 0,
                note: Note::Copy,
                focus_active: false,
                history: Default::default(),
                mouse: (0.0, 0.0),
                fx: [spring(0.0); FX_N],
                hot: [spring(0.0); 17],
                cur: [spring(0.0); 17],
                dl: None,
                dl_spin: spring(0.0),
                dl_press: None,
                _focus: extras::watch_focus(hwnd),
                bright: false,
                bright_level: None,
                ddc: extras::start_ddc(hwnd),
                vol: spring(0.0),
                vsl: Slider::new(vol_level),
                vol_level,
                muted,
                endpoint,
                mic,
                mic_muted,
                mic_rec: false,
                press: spring(0.0),
                pp: spring(0.0),
                skip: [spring(1.0); 2],
                pressed: None,
                sl: Slider::new(0.0),
                bump: spring(0.0),
                change: spring(1.0),
                prev: None,
                hover: false,
                last: Instant::now(),
                t: 0.0,
                rgn: (0, 0, 0, 0),
                view: View::Player,
                xd: [spring(EXPANDED.0), spring(EXPANDED.1), spring(EXPANDED.2)],
                va: [spring(1.0), spring(0.0), spring(0.0), spring(0.0), spring(0.0), spring(0.0), spring(0.0), spring(0.0), spring(0.0), spring(0.0), spring(0.0), spring(0.0)],
                wpage: spring(0.0),
                w_t0: 0.0,
                shelf: Vec::new(),
                shelf_drop: spring(0.0),
                shelf_press: None,
                drag: None,
                calc_in: String::new(),
                calc_prev: HWND::default(),
                calc_copied: spring(0.0),
                lyrics: None,
                lyrics_pending: String::new(),
                lyric_idx: None,
                lyric_prev: None,
                lyric_in: spring(1.0),
                ly_c: spring(0.0),
                hidden: false,
                update: None,
                updating: false,
                cc: Default::default(),
                mx: Default::default(),
                ai: Default::default(),
                cal: Default::default(),
                clock_min: std::cell::Cell::new(u16::MAX),
                shown: Spring { x: 0.0, v: 0.0, to: 1.0 }, // pops in on launch
                scaled: false,
                timer: None,
                tc: spring(0.0),
                wheel: [spring(0.0), spring(5.0), spring(0.0)],
                wdrag: None,
            })
        });
        APP.with(|a| a.borrow_mut().as_mut().map(App::reposition));
        let _ = AddClipboardFormatListener(hwnd);
        let _ = shelf::register(hwnd);
        system::watch_fullscreen(hwnd);
        APP.with(|a| a.borrow().as_ref().map(App::start_update_checks));
        APP.with(|a| a.borrow_mut().as_mut().map(|app| app.on_fullscreen(system::fullscreen_now(hwnd))));
        if APP.with(|a| a.borrow().as_ref().is_some_and(|app| !app.hidden)) {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            APP.with(|a| a.borrow_mut().as_mut().filter(|app| !app.cfg.welcomed).map(App::open_welcome));
        }

        // Render only while something moves; otherwise block in GetMessage (0% CPU).
        let mut msg = MSG::default();
        loop {
            let busy = APP.with(|a| a.borrow().as_ref().is_some_and(App::animating));
            if busy {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return Ok(());
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                APP.with(|a| a.borrow_mut().as_mut().map(App::frame));
            } else {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    return Ok(());
                }
                // The follow-the-mouse poll changes nothing on screen unless it moved the island.
                let poll = msg.message == WM_TIMER && msg.wParam.0 == T_MON;
                let mon = APP.with(|a| a.borrow().as_ref().map(|app| app.mon));
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                APP.with(|a| a.borrow_mut().as_mut().map(|app| {
                    app.last = Instant::now(); // don't jump animations after idling
                    if !(app.hidden && app.shown.settled()) && !(poll && Some(app.mon) == mon) {
                        app.frame(); // nothing to draw behind a fullscreen app
                    }
                }));
            }
        }
    }
}

/// Size the window for the current DPI and pin it to the top-center of `mon`.
unsafe fn place(hwnd: HWND, dpi: f32, mon: HMONITOR) {
    let s = dpi / 96.0;
    let (w, h) = ((CW * s).ceil() as i32, (CH * s).ceil() as i32);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(mon, &mut mi);
    let r = mi.rcMonitor;
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, r.left + (r.right - r.left - w) / 2, r.top, w, h, SWP_NOACTIVATE);
}

unsafe fn gfx(hwnd: HWND, dpi: f32) -> Result<Gfx> {
    let mut d3d = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut d3d),
        None,
        None,
    )?;
    let dxgi: IDXGIDevice1 = d3d.unwrap().cast()?;
    dxgi.SetMaximumFrameLatency(1)?; // Present blocks on vsync instead of queueing frames
    let s = dpi / 96.0;
    let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
    let swap = factory.CreateSwapChainForComposition(
        &dxgi,
        &DXGI_SWAP_CHAIN_DESC1 {
            Width: (CW * s).ceil() as u32,
            Height: (CH * s).ceil() as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        },
        None,
    )?;
    let d2f: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
    let dc = d2f.CreateDevice(&dxgi)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
    dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    bind_target(&swap, &dc, dpi)?;

    let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
    let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
    let visual = dcomp.CreateVisual()?;
    visual.SetContent(&swap)?;
    target.SetRoot(&visual)?;
    dcomp.Commit()?;

    // Unit right-pointing triangle with SF Symbols-style rounded corners; placed with transforms.
    let tri = d2f.CreatePathGeometry()?;
    let sink = tri.Open()?;
    let v = [(0.0f32, 0.0f32), (0.87, 0.5), (0.0, 1.0)];
    let r = 0.2; // corner cut along each edge, in unit space
    let toward = |a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let n = dx.hypot(dy);
        D2D_POINT_2F { x: a.0 + dx / n * r, y: a.1 + dy / n * r }
    };
    sink.BeginFigure(toward(v[0], v[1]), D2D1_FIGURE_BEGIN_FILLED);
    for i in [1, 2, 0] {
        let (p, c, n) = (v[(i + 2) % 3], v[i], v[(i + 1) % 3]);
        sink.AddLine(toward(c, p));
        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: D2D_POINT_2F { x: c.0, y: c.1 }, point2: toward(c, n) });
    }
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close()?;

    let caps = d2f.CreateStrokeStyle(
        &D2D1_STROKE_STYLE_PROPERTIES1 {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            ..Default::default()
        },
        None,
    )?;

    // Speaker glyph in a 24-unit box: filled body plus three sound-wave arcs.
    let speaker = d2f.CreatePathGeometry()?;
    let sink = speaker.Open()?;
    let pts = [(7.0, 9.3), (12.0, 5.3), (12.0, 18.7), (7.0, 14.7), (3.6, 14.7), (3.0, 14.1), (3.0, 9.9), (3.6, 9.3)];
    sink.BeginFigure(D2D_POINT_2F { x: pts[7].0, y: pts[7].1 }, D2D1_FIGURE_BEGIN_FILLED);
    for (x, y) in pts {
        sink.AddLine(D2D_POINT_2F { x, y });
    }
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close()?;
    let wave = |r: f32| -> Result<ID2D1PathGeometry1> {
        let geom = d2f.CreatePathGeometry()?;
        let sink = geom.Open()?;
        let (cx, cy, a) = (12.5, 12.0, 50f32.to_radians());
        sink.BeginFigure(D2D_POINT_2F { x: cx + r * a.cos(), y: cy - r * a.sin() }, D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: D2D_POINT_2F { x: cx + r * a.cos(), y: cy + r * a.sin() },
            size: D2D_SIZE_F { width: r, height: r },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_SMALL,
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        Ok(geom)
    };
    let waves = [wave(3.6)?, wave(6.6)?, wave(9.6)?];
    let mic_cradle = d2f.CreatePathGeometry()?;
    let sink = mic_cradle.Open()?;
    sink.BeginFigure(D2D_POINT_2F { x: 6.0, y: 10.5 }, D2D1_FIGURE_BEGIN_HOLLOW);
    sink.AddArc(&D2D1_ARC_SEGMENT {
        point: D2D_POINT_2F { x: 18.0, y: 10.5 },
        size: D2D_SIZE_F { width: 6.0, height: 6.7 },
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    });
    sink.EndFigure(D2D1_FIGURE_END_OPEN);
    sink.Close()?;

    let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
    let fmt = |size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT| -> Result<IDWriteTextFormat> {
        let f = dw.CreateTextFormat(
            w!("Segoe UI Variable Display"),
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        f.SetTextAlignment(align)?;
        let sign = dw.CreateEllipsisTrimmingSign(&f)?;
        f.SetTrimming(&DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, ..Default::default() }, &sign)?;
        Ok(f)
    };
    Ok(Gfx {
        brush: dc.CreateSolidColorBrush(&fg(1.0), None)?,
        swap,
        dc,
        dcomp,
        _target: target,
        visual,
        tri,
        caps,
        speaker,
        waves,
        mic_cradle,
        f_title_s: fmt(12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_artist_s: fmt(10.5, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_title_l: fmt(16.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_artist_l: fmt(14.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_time: fmt(11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_time_r: fmt(11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        f_wheel: fmt(21.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        f_wheel_label: fmt(16.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_btn: fmt(12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        f_big: fmt(34.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_cd: fmt(15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        f_row: fmt(13.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_hello: fmt(46.0, DWRITE_FONT_WEIGHT_LIGHT, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        f_hello_l: fmt(46.0, DWRITE_FONT_WEIGHT_LIGHT, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_wtitle: fmt(17.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        f_wbody: {
            let f = fmt(12.5, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
            f
        },
        f_wbtn: fmt(12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        f_seg: fmt(11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        f_chat: {
            let f = fmt(12.5, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
            f
        },
        f_line: fmt(12.5, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        f_calc: fmt(24.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        f_calc_r: fmt(17.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
        f_lyric: fmt(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        f_lyric_s: fmt(11.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        factory: d2f,
        dw,
    })
}

unsafe fn bind_target(swap: &IDXGISwapChain1, dc: &ID2D1DeviceContext, dpi: f32) -> Result<()> {
    let surface: IDXGISurface = swap.GetBuffer(0)?;
    let bmp = dc.CreateBitmapFromDxgiSurface(
        &surface,
        Some(&D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        }),
    )?;
    dc.SetTarget(&bmp);
    dc.SetDpi(dpi, dpi);
    Ok(())
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}
fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}
fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: rect(x, y, w, h), radiusX: r, radiusY: r }
}
fn mat(sx: f32, sy: f32, tx: f32, ty: f32) -> Matrix3x2 {
    Matrix3x2 { M11: sx, M12: 0.0, M21: 0.0, M22: sy, M31: tx, M32: ty }
}
/// Local time as the user formats it (no seconds), its minute of the day, and ms to the next minute.
fn clock() -> (String, u16, u32) {
    unsafe {
        let st = GetLocalTime();
        let mut buf = [0u16; 32];
        let n = GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&st), PCWSTR::null(), Some(&mut buf));
        let text = if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { format!("{:02}:{:02}", st.wHour, st.wMinute) };
        let ms = 60_000 - (st.wSecond as u32 * 1000 + st.wMilliseconds as u32);
        (text, st.wHour * 60 + st.wMinute, ms)
    }
}

/// What a top-row button does, shown when it's hovered.
fn top_label(id: usize) -> &'static str {
    match id {
        B_POWER => "Quit",
        B_TIMER => "Timer",
        B_CLIPS => "Clipboard",
        B_PICK => "Colour picker",
        B_CALC => "Calculator",
        B_SHELF => "Shelf",
        B_MIC => "Microphone",
        B_GEAR => "Settings",
        B_MIXER => "Sound Mixer",
        B_AI => "AI",
        _ => "",
    }
}

/// Today as short words in the user's language, e.g. "Sun 25 Oct".
fn today() -> String {
    unsafe {
        let st = GetLocalTime();
        let mut buf = [0u16; 32];
        let n = GetDateFormatEx(PCWSTR::null(), Default::default(), Some(&st), w!("ddd d MMM"), Some(&mut buf), PCWSTR::null());
        if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { format!("{:02}.{:02}.", st.wDay, st.wMonth) }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
fn now_ft() -> i64 {
    let ft = unsafe { GetSystemTimePreciseAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

/// Island geometry for the current animation state, in canvas DIPs.
struct Layout {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    e: f32,
}

impl App {
    fn animating(&self) -> bool {
        if self.hidden && self.shown.settled() {
            return false; // nothing to draw for while a fullscreen app has the screen
        }
        if !self.shown.settled() {
            return true;
        }
        [self.music, self.expand, self.wave, self.art_in, self.clip, self.check, self.vol, self.press, self.pp, self.skip[0], self.skip[1], self.bump, self.change]
            .iter()
            .any(|s| !s.settled())
            || self.wave.to > 0.0
            || self.sl.busy()
            || self.vsl.busy()
            || self.timer_busy()
            || !self.dl_spin.settled()
            || self.settings_busy()
            || self.mixer_busy()
            || self.ai_busy()
            || [self.shelf_drop, self.calc_copied, self.lyric_in, self.ly_c].iter().any(|s| !s.settled())
            || self.fx_busy()
            || self.welcome_busy()
            || self.control_busy()
            || ((self.expand.x > 0.5 || self.music.x < 0.5) && clock().1 != self.clock_min.get())
    }

    /// Where every state fade should head, from the state it shows.
    fn fx_targets(&self) -> ([f32; FX_N], [f32; 17], [f32; 17]) {
        let b = |x: bool| x as u8 as f32;
        let fx = [b(self.mic_live()), b(self.focus_active), b(self.muted), b(self.mic_muted), b(!self.shelf.is_empty()), b(self.timer.is_some())];
        let (mut hot, mut cur) = ([0.0; 17], [0.0; 17]);
        let l = self.layout();
        let (mx, my) = self.mouse;
        let live = self.hover && self.expanded_alpha(&l).0 > 0.5;
        if let Some(id) = self.top_hit(&l, mx, my).filter(|_| live) {
            hot[id] = 1.0;
        }
        for (id, _, _) in self.top_buttons(&l) {
            cur[id] = b(matches!((id, self.view), (B_CLIPS, View::Clips) | (B_GEAR, View::Settings) | (B_SHELF, View::Shelf) | (B_CALC, View::Calc) | (B_MIXER, View::Mixer) | (B_AI, View::Ai)));
        }
        (fx, hot, cur)
    }

    /// Checked while idle too, so a state change wakes the render loop.
    fn fx_busy(&self) -> bool {
        let (fx, hot, cur) = self.fx_targets();
        let pairs = self.fx.iter().zip(fx).chain(self.hot.iter().zip(hot)).chain(self.cur.iter().zip(cur));
        pairs.into_iter().any(|(s, t)| !s.settled() || s.to != t)
    }

    fn fx_step(&mut self, dt: f32) {
        let (fx, hot, cur) = self.fx_targets();
        for (s, t) in self.fx.iter_mut().zip(fx) {
            s.to = t;
            s.step(dt, 0.34, 0.78); // dots pop with a small overshoot
        }
        for (s, t) in self.hot.iter_mut().zip(hot).chain(self.cur.iter_mut().zip(cur)) {
            s.to = t;
            s.step(dt, 0.22, 1.0);
        }
    }

    /// Current playback position in seconds, extrapolated from the last timeline update.
    fn pos(&self) -> f64 {
        let Some(m) = &self.media else { return 0.0 };
        let p = if m.playing { m.pos + (now_ft() - m.pos_at) as f64 / 1e7 } else { m.pos };
        if m.dur > 0.0 { p.clamp(0.0, m.dur) } else { p.max(0.0) }
    }

    /// Seek track (left x, length, center y) at full expansion, before the fade-in offset.
    fn track(&self, l: &Layout) -> (f32, f32, f32) {
        (l.x + 62.0, l.w - 124.0, l.y + 88.0 + TOPBAR)
    }

    fn slider_hit(&self, x: f32, y: f32) -> bool {
        let l = self.layout();
        let (bx, len, py) = self.track(&l);
        l.e > 0.9 && self.view == View::Player && self.media.as_ref().is_some_and(|m| m.dur > 0.0) && (y - py).abs() < 12.0 && x > bx - 8.0 && x < bx + len + 8.0
    }

    fn follow(&mut self, x: f32) {
        let (bx, len, _) = self.track(&self.layout());
        self.sl.follow(x, bx, len);
    }

    /// Volume slider track (left x, length, center y).
    fn vol_track(&self, l: &Layout) -> (f32, f32, f32) {
        (l.x + 44.0, l.w - 44.0 - 20.0, l.y + l.h / 2.0)
    }

    fn vol_hit(&self, x: f32, y: f32) -> bool {
        let l = self.layout();
        let (bx, len, cy) = self.vol_track(&l);
        self.vol.to > 0.5 && self.vol.x > 0.8 && (y - cy).abs() < 14.0 && x > bx - 10.0 && x < bx + len + 10.0
    }

    /// Drag on the volume slider sets the system volume live.
    fn vol_follow(&mut self, x: f32) {
        let (bx, len, _) = self.vol_track(&self.layout());
        self.vsl.follow(x, bx, len);
        if self.bright {
            self.bright_level = Some(self.vsl.p.to);
            let _ = self.ddc.send(self.vsl.p.to);
            return;
        }
        self.vol_level = self.vsl.p.to;
        if let Some((v, _)) = &self.endpoint {
            let _ = unsafe { v.SetMasterVolumeLevelScalar(self.vol_level, std::ptr::null()) };
        }
    }

    fn on_volume(&mut self, level: f32, muted: bool) {
        if !self.vsl.drag && self.cc.drag != Some(1) && self.mx.drag != Some(0) {
            self.vol_level = level; // our own drag already set it; don't fight the pointer
        }
        self.muted = muted;
        if self.control_open() || self.mixer_open() {
            return; // Control Center's or the mixer's own slider shows it
        }
        if self.vsl.drag && self.bright {
            return; // keep the brightness pill under the pointer
        }
        if self.cfg.pills[P_VOLUME] {
            self.bright = false;
            self.show_level();
        }
    }

    fn show_level(&mut self) {
        if !self.may_show(K_VOLUME) {
            return;
        }
        self.vol.to = 1.0;
        self.clip.to = 0.0; // one notice at a time
        unsafe {
            let _ = KillTimer(self.hwnd, T_CLIP);
            SetTimer(self.hwnd, T_VOL, VOL_MS, None);
        }
    }

    fn release(&mut self) {
        if self.settings_release() || self.shelf_release() || self.wheel_release() || self.control_release() || self.mixer_release() {
            return;
        }
        if self.dl_press.take().is_some() {
            if let Some(d) = &self.dl {
                unsafe { downloads::open(&d.path) };
            }
            self.clip.to = 0.0;
            unsafe { let _ = ReleaseCapture(); }
            return;
        }
        if self.vsl.drag {
            self.vsl.drag = false;
            self.vsl.held = false;
            unsafe {
                let _ = ReleaseCapture();
                SetTimer(self.hwnd, T_VOL, VOL_MS, None);
            }
            return;
        }
        if !self.sl.drag {
            return;
        }
        self.sl.drag = false;
        self.sl.held = false;
        if let Some(m) = &mut self.media {
            // Optimistic: show the new position now; the next media snapshot confirms it.
            m.pos = self.sl.p.to as f64 * m.dur;
            m.pos_at = now_ft();
            let _ = self.tx.send(Cmd::Seek(m.pos));
        }
        unsafe {
            let _ = ReleaseCapture();
            if !self.hover {
                SetTimer(self.hwnd, T_LEAVE, 250, None);
            }
        }
    }

    fn layout(&self) -> Layout {
        let e = self.expand.x;
        let m = self.music.x.max(e.min(1.0)); // the picker can open from the resting pill
        let xd = (self.xd[0].x, self.xd[1].x, self.xd[2].x);
        let k = self.ly_c.x;
        let cp = (lerp(COMPACT.0, lyrics::COMPACT_LY.0, k), lerp(COMPACT.1, lyrics::COMPACT_LY.1, k), lerp(COMPACT.2, lyrics::COMPACT_LY.2, k));
        let dim = |i: fn(&(f32, f32, f32)) -> f32| i(&IDLE) + (i(&cp) - i(&IDLE)) * m + (i(&xd) - i(&cp)) * e;
        let (w, h, r) = (dim(|d| d.0) + 16.0 * self.bump.x, dim(|d| d.1) + 3.0 * self.bump.x, dim(|d| d.2).max(8.0));
        let c = self.clip.x; // the notice takes over whatever the island was showing
        let (w, h, r) = (lerp(w, CLIP.0, c), lerp(h, CLIP.1, c), lerp(r, CLIP.2, c));
        let v = self.vol.x;
        let (w, h, r) = (lerp(w, VOL.0, v), lerp(h, VOL.1, v), lerp(r, VOL.2, v));
        Layout { x: (CW - w) / 2.0, y: if self.cfg.notch { 0.0 } else { self.cfg.top }, w, h, r: (r * self.cfg.round).min(h / 2.0), e }
    }

    fn buttons(&self, l: &Layout) -> [(f32, f32); 3] {
        // Fixed from the top so the lyric line can sit below them.
        let (cx, cy) = (l.x + l.w / 2.0, l.y + EXPANDED.1 - 28.0);
        [(cx - 64.0, cy), (cx, cy), (cx + 64.0, cy)]
    }

    fn on_media(&mut self) {
        // New players show up on the Music settings page.
        let seen = self.seen.lock().unwrap().clone();
        let fresh: Vec<_> = seen.into_iter().filter(|id| !self.cfg.apps.iter().any(|(a, _)| a == id)).collect();
        if !fresh.is_empty() {
            self.cfg.apps.extend(fresh.into_iter().map(|id| (id, true)));
            self.cfg.save();
        }
        let m = if self.cfg.pills[P_MUSIC] { self.shared.lock().unwrap().clone() } else { None };
        let new_track = matches!((&self.media, &m), (Some(a), Some(b)) if a.title != b.title || a.artist != b.artist);
        // A second update mid-transition only swaps the incoming content; restarting would jump.
        if new_track && self.prev.is_none() {
            let old = self.media.as_ref().unwrap();
            self.prev = Some(Prev { title: old.title.clone(), artist: old.artist.clone(), art: self.art.as_ref().map(|(b, _)| b.clone()) });
            self.change = Spring { x: 0.0, v: 0.0, to: 1.0 };
            self.bump.v = 20.0; // impulse; the spring settles it back to 0
        }
        let changed = match (&m.as_ref().and_then(|m| m.art.clone()), &self.art) {
            (Some(a), Some((_, b))) => !std::sync::Arc::ptr_eq(a, b),
            (a, b) => a.is_some() != b.is_some(),
        };
        if changed {
            self.art = m.as_ref().and_then(|m| m.art.clone()).and_then(|a| unsafe { self.art_brush(a).ok() });
            // Art arriving with a new track pops in via `change`; art arriving late fades in over the placeholder.
            if self.art.is_some() && !new_track {
                self.art_in = Spring { x: 0.0, v: 0.0, to: 1.0 };
            }
            self.accent = self.art.as_ref().map_or([1.0, 1.0, 1.0], |(_, a)| a.accent);
        }
        self.wave.to = if m.as_ref().is_some_and(|m| m.playing) { 1.0 } else { 0.0 };
        self.pp.to = self.wave.to;
        if m.is_none() && self.view == View::Player {
            self.expand.to = 0.0;
        }
        self.media = m;
        self.presence();
        self.want_lyrics();
        if self.view == View::Player && self.xd[1].to != self.player_size().1 {
            self.set_view(View::Player); // lyric line comes or goes with the track
        }
    }

    unsafe fn art_brush(&self, a: std::sync::Arc<media::Art>) -> Result<(ID2D1BitmapBrush1, std::sync::Arc<media::Art>)> {
        let bmp = self.g.dc.CreateBitmap(
            D2D_SIZE_U { width: ART, height: ART },
            Some(a.pixels.as_ptr() as _),
            ART * 4,
            &D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            },
        )?;
        Ok((self.g.dc.CreateBitmapBrush(&bmp, None, None)?, a))
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(1.0 / 30.0);
        self.last = now;
        self.t += dt;
        // Expand: soft bounce. Collapse: a touch quicker and calmer, like iOS.
        let (resp, damp) = if self.expand.to > 0.5 { (0.4, 0.74) } else { (0.32, 0.88) };
        self.expand.step(dt, resp, damp);
        self.music.step(dt, 0.42, 0.72);
        self.wave.step(dt, 0.35, 1.0);
        self.art_in.step(dt, 0.35, 1.0);
        self.clip.step(dt, 0.42, if self.clip.to > 0.5 { 0.7 } else { 0.9 });
        self.check.step(dt, 0.45, 1.0);
        self.press.step(dt, 0.2, 0.6);
        self.pp.step(dt, 0.3, 0.75);
        for s in &mut self.skip {
            s.step(dt, 0.42, 0.85);
        }
        self.bump.step(dt, 0.5, 0.62); // one soft swell, a single small rebound
        self.change.step(dt, 0.45, 1.0);
        if self.change.settled() {
            self.prev = None;
        }
        self.vol.step(dt, 0.42, if self.vol.to > 0.5 { 0.7 } else { 0.9 });
        self.timer_step(dt);
        self.dl_spin.step(dt, 0.45, 1.0);
        self.settings_step(dt);
        self.shelf_drop.step(dt, 0.3, 0.9);
        self.calc_copied.step(dt, 1.4, 1.0);
        self.lyrics_step(dt);
        self.fx_step(dt);
        self.welcome_step(dt);
        self.control_step(dt);
        self.mixer_step(dt);
        // Appear: pop with a little overshoot. Hide: quick and calm, it's in a game's way.
        let (resp, damp) = if self.shown.to > 0.5 { (0.42, 0.7) } else { (0.26, 1.0) };
        self.shown.step(dt, resp, damp);
        let frac = self.media.as_ref().filter(|m| m.dur > 0.0).map_or(0.0, |m| (self.pos() / m.dur) as f32);
        self.sl.step(dt, frac);
        self.vsl.step(dt, if self.bright { self.bright_level.unwrap_or(0.0) } else { self.vol_level });
        if let Err(e) = unsafe { self.draw() } {
            eprintln!("draw failed: {e:?}");
        }
        if self.hidden && self.shown.settled() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Notch style as one outline, so no seams show: flat top flush with the screen edge, concave
    /// ears flaring into it like a MacBook notch, rounded bottom corners. False if it couldn't be built.
    unsafe fn fill_notch(&self, l: &Layout) -> bool {
        let (x, y, w, h, r) = (l.x, l.y, l.w, l.h, l.r);
        let e = NOTCH_EAR.min(h / 2.0);
        let p = |x: f32, y: f32| D2D_POINT_2F { x, y };
        let arc = |to: D2D_POINT_2F, rad: f32, dir| D2D1_ARC_SEGMENT { point: to, size: D2D_SIZE_F { width: rad, height: rad }, rotationAngle: 0.0, sweepDirection: dir, arcSize: D2D1_ARC_SIZE_SMALL };
        let (ccw, cw) = (D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE, D2D1_SWEEP_DIRECTION_CLOCKWISE);
        let Ok(geom) = self.g.factory.CreatePathGeometry() else { return false };
        let Ok(sink) = geom.Open() else { return false };
        sink.BeginFigure(p(x - e, y), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(p(x + w + e, y));
        sink.AddArc(&arc(p(x + w, y + e), e, ccw));
        sink.AddLine(p(x + w, y + h - r));
        sink.AddArc(&arc(p(x + w - r, y + h), r, cw));
        sink.AddLine(p(x + r, y + h));
        sink.AddArc(&arc(p(x, y + h - r), r, cw));
        sink.AddLine(p(x, y + e));
        sink.AddArc(&arc(p(x - e, y), e, ccw));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        if sink.Close().is_err() {
            return false;
        }
        self.g.dc.FillGeometry(&geom, &self.g.brush, None);
        true
    }

    unsafe fn draw(&mut self) -> Result<()> {
        let l = self.layout();
        let s = self.dpi / 96.0;
        // Hit-test region follows the island so the rest of the canvas is click-through. While the
        // island resizes, use the whole canvas: changing the region every frame makes DWM hitch.
        let resizing = [self.music, self.expand, self.bump, self.clip, self.vol, self.xd[0], self.xd[1], self.ly_c].iter().any(|s| !s.settled());
        let rgn = if resizing {
            (0, 0, (CW * s).ceil() as i32, (CH * s).ceil() as i32)
        } else {
            let ex = if self.cfg.notch { NOTCH_EAR + 1.0 } else { 1.0 }; // the region also clips drawing
            (((l.x - ex) * s) as i32, ((l.y - 1.0) * s) as i32, ((l.x + l.w + ex) * s).ceil() as i32, ((l.y + l.h + 1.0) * s).ceil() as i32)
        };
        if rgn != self.rgn {
            self.rgn = rgn;
            SetWindowRgn(self.hwnd, CreateRectRgn(rgn.0, rgn.1, rgn.2, rgn.3), false);
        }

        // Show/hide: the compositor scales the whole frame about the pill's top centre, rising as it
        // shrinks; the frame itself fades through a layer below. Free once it's back at rest.
        let k = self.shown.x;
        if k != 1.0 || self.scaled {
            let q = lerp(0.55, 1.0, k);
            let (px, py) = (CW / 2.0 * s, l.y * s);
            self.g.visual.SetTransform2(&mat(q, q, px * (1.0 - q), py * (1.0 - q) - (1.0 - k) * 8.0 * s))?;
            self.g.dcomp.Commit()?;
            self.scaled = k != 1.0;
        }

        let g = &self.g;
        let dc = &g.dc;
        let br = &g.brush;
        dc.BeginDraw();
        dc.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
        let fading = k < 1.0;
        if fading {
            let params = D2D1_LAYER_PARAMETERS1 { contentBounds: rect(-1e4, -1e4, 2e4, 2e4), opacity: clamp01(k), maskTransform: Matrix3x2::identity(), ..Default::default() };
            dc.PushLayer(&params, None);
        }
        dc.SetTransform(&Matrix3x2::identity());
        br.SetOpacity(1.0);
        // How much the player shows; gives way to the timer: its countdown when compact, its views when expanded.
        let music_a = clamp01((self.music.x - 0.4) / 0.6) * clamp01(1.0 - self.clip.x * 2.0) * clamp01(1.0 - self.vol.x * 2.0) * lerp(1.0 - self.tc.x, self.va[0].x, clamp01(l.e));
        // While a song plays, the island takes on a little of its cover colour, like the waveform.
        let [ar, ag, ab] = self.accent;
        // Stays on in other expanded views (Settings shows it live), but not behind the compact timer, copied or volume pills.
        let shown = clamp01((self.music.x - 0.4) / 0.6) * clamp01(1.0 - self.clip.x * 2.0) * clamp01(1.0 - self.vol.x * 2.0) * lerp(1.0 - self.tc.x, 1.0, clamp01(l.e));
        let on = self.cfg.cover_tint && self.media.is_some() && self.art.is_some();
        let t = if on { 0.4 * self.cfg.tint * shown * clamp01(self.wave.x) * clamp01(self.art_in.x) } else { 0.0 };
        settings::set_tint([ar, ag, ab, t]);
        br.SetColor(&bg(1.0));
        if !(self.cfg.notch && self.fill_notch(&l)) {
            dc.FillRoundedRectangle(&rrect(l.x, l.y, l.w, l.h, l.r), br);
        }

        if let Some(m) = &self.media {
            let e = clamp01(l.e);
            let alpha = music_a;
            let small = clamp01(1.0 - e * 3.0) * alpha; // compact-only content fades out fast
            let big = clamp01((e - 0.4) / 0.6) * alpha; // expanded content fades in late

            // Album art morphs between the compact and expanded spots.
            let (ax, ay, asz, ar) = (lerp(6.0, 18.0, l.e), lerp(6.0, 18.0 + TOPBAR, l.e), lerp(24.0, 52.0, l.e), lerp(6.0, 12.0, l.e));
            // Sliding text must not spill outside the island.
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let cw = self.change.x;
            // Art scaled about its center; placeholder tile when there is no art.
            let draw_art = |art: Option<&ID2D1BitmapBrush1>, a: f32, sc: f32, fade_in: f32| {
                let sz = asz * sc;
                let (x, y) = (l.x + ax + (asz - sz) / 2.0, l.y + ay + (asz - sz) / 2.0);
                let rr = rrect(x, y, sz, sz, ar * sc);
                // Placeholder tile sits under art that is still fading in, so late art never flashes empty.
                let fade = if art.is_some() { fade_in } else { 0.0 };
                if fade < 1.0 {
                    br.SetColor(&surface(a * (1.0 - fade)));
                    dc.FillRoundedRectangle(&rr, br);
                }
                if let Some(ab) = art {
                    let k = sz / ART as f32;
                    ab.SetTransform(&mat(k, k, x, y));
                    ab.SetOpacity(a * fade);
                    dc.FillRoundedRectangle(&rr, ab);
                }
            };
            // Track change: old art sinks away, new art pops in from smaller.
            if let Some(p) = &self.prev {
                draw_art(p.art.as_ref(), alpha * (1.0 - cw), 1.0 - 0.2 * cw, 1.0);
            }
            draw_art(self.art.as_ref().map(|(b, _)| b), alpha * cw, 0.75 + 0.25 * cw, self.art_in.x);

            // Waveform, anchored right; tinted with the art's accent color.
            let (bars, bw, gap) = (6, lerp(2.2, 3.2, l.e), lerp(1.8, 2.8, l.e));
            let ww = bars as f32 * bw + (bars - 1) as f32 * gap;
            let wx = l.x + l.w - lerp(12.0, 22.0, l.e) - ww;
            let wcy = l.y + lerp(18.0, 44.0 + TOPBAR, l.e);
            let maxh = lerp(12.0, 20.0, l.e);
            // Art tint, or the theme's text colour while there's no art.
            let [r, gg, b] = if self.art.is_some() { self.accent } else { let (r, g, b) = fg_rgb(); [r, g, b] };
            br.SetColor(&rgba(r, gg, b, alpha));
            for i in 0..bars {
                let f = i as f32;
                let n = 0.5 + 0.28 * (self.t * (6.1 + f * 1.3) + f * 1.7).sin() + 0.22 * (self.t * (9.7 - f * 0.9) + f * 2.9).sin();
                let env = 1.0 - ((f - 2.5) / 3.2).powi(2) * 0.5; // taller in the middle
                let h = bw + (maxh - bw) * n * env * self.wave.x;
                dc.FillRoundedRectangle(&rrect(wx + f * (bw + gap), wcy - h / 2.0, bw, h, bw / 2.0), br);
            }

            let opts = D2D1_DRAW_TEXT_OPTIONS_NONE;
            let mm = DWRITE_MEASURING_MODE_NATURAL;
            let dy = (1.0 - big) * 6.0; // slight rise as expanded content fades in
            // Track change: old title slides up and clears early, then the new one rises in, so the
            // two never read as stacked lines.
            let layers = match &self.prev {
                Some(p) => vec![
                    (p.title.as_str(), p.artist.as_str(), clamp01(1.0 - cw / 0.4), -9.0 * cw),
                    (m.title.as_str(), m.artist.as_str(), clamp01((cw - 0.3) / 0.7), 9.0 * (1.0 - cw)),
                ],
                None => vec![(m.title.as_str(), m.artist.as_str(), 1.0, 0.0)],
            };
            for (title, artist, la, oy) in layers {
                let title: Vec<u16> = title.encode_utf16().collect();
                let artist: Vec<u16> = artist.encode_utf16().collect();
                if small * la > 0.0 {
                    let tx = l.x + 36.0;
                    let tw = (wx - 10.0 - 8.0 * clamp01(self.fx[FX_MIC].x) - tx).max(0.0);
                    br.SetColor(&fg(small * la));
                    dc.DrawText(&title, &g.f_title_s, &rect(tx, l.y + 2.0 + oy, tw, 17.0), br, opts, mm);
                    br.SetColor(&fg(small * la * 0.55));
                    dc.DrawText(&artist, &g.f_artist_s, &rect(tx, l.y + 17.0 + oy * 1.3, tw, 15.0), br, opts, mm);
                }
                if big * la > 0.0 {
                    let tx = l.x + 82.0;
                    let tw = (wx - 12.0 - tx).max(0.0);
                    br.SetColor(&fg(big * la));
                    dc.DrawText(&title, &g.f_title_l, &rect(tx, l.y + 22.0 + TOPBAR + dy + oy, tw, 22.0), br, opts, mm);
                    br.SetColor(&fg(big * la * 0.55));
                    dc.DrawText(&artist, &g.f_artist_l, &rect(tx, l.y + 43.0 + TOPBAR + dy + oy * 1.3, tw, 20.0), br, opts, mm);
                }
            }
            if small > 0.0 {
                self.draw_lyrics_compact(&l, small);
            }
            if small > 0.0 {
                self.mic_dot(wx - 8.0, wcy, small);
            }
            if big > 0.0 {
                // Progress row.
                let sl = &self.sl;
                let pos = if sl.drag { sl.p.to as f64 * m.dur } else { self.pos() };
                let (bx, len, py) = self.track(&l);
                let py = py + dy;
                let op = big * (0.7 + 0.3 * sl.a.x); // restOpacity 0.7 -> 1 when active
                let (ls, rs) = sl.draw(dc, br, bx, len, py, op, 5.0); // labels ride the stretch like the icons
                br.SetColor(&fg(op * 0.6));
                dc.DrawText(&mmss(pos).encode_utf16().collect::<Vec<_>>(), &g.f_time, &rect(l.x + 20.0 - ls, py - 8.0, 40.0, 16.0), br, opts, mm);
                dc.DrawText(&format!("-{}", mmss(m.dur - pos)).encode_utf16().collect::<Vec<_>>(), &g.f_time_r, &rect(l.x + l.w - 60.0 + rs, py - 8.0, 40.0, 16.0), br, opts, mm);

                // Transport controls.
                br.SetColor(&fg(big));
                for (i, (cx, cy)) in self.buttons(&l).into_iter().enumerate() {
                    let k = if self.pressed == Some(i) { 1.0 - 0.18 * self.press.x } else { 1.0 };
                    let cy = cy + dy;
                    match i {
                        1 => {
                            // Play/pause morph: outgoing icon shrinks and fades as the incoming one grows in.
                            let p = clamp01(self.pp.x);
                            if p > 0.01 {
                                let k = k * (0.5 + 0.5 * p);
                                br.SetColor(&fg(big * p));
                                for ox in [-7.0, 2.0] {
                                    dc.FillRoundedRectangle(&rrect(cx + ox * k, cy - 11.0 * k, 5.0 * k, 22.0 * k, 2.2 * k), br);
                                }
                            }
                            if p < 0.99 {
                                let k = k * (1.0 - 0.5 * p);
                                br.SetColor(&fg(big * (1.0 - p)));
                                self.triangle(cx - 9.0 * k, cy - 12.0 * k, 24.0 * k, false);
                            }
                        }
                        _ => {
                            // Arrows roll one slot in the skip direction: the lead arrow slides out and
                            // fades, the trailing one takes its place, a new one grows in behind.
                            // At rest (t = 1) this is the same picture as before the click.
                            let flip = i == 0;
                            let t = clamp01(self.skip[i / 2].x);
                            let sz = 18.0 * k;
                            let w = sz * 0.87;
                            let step = sz * 0.78; // rounded tips leave room, so overlap slightly like SF Symbols
                            let x0 = cx - (step + w) / 2.0;
                            for (x, sc) in [(x0, t), (x0 + step * t, 1.0), (x0 + step * (1.0 + t), 1.0 - t)] {
                                if sc < 0.01 {
                                    continue;
                                }
                                let s2 = sz * sc;
                                let xs = x + (w - s2 * 0.87) / 2.0; // scale about the slot's center
                                let xs = if flip { 2.0 * cx - xs - s2 * 0.87 } else { xs }; // mirror for prev
                                br.SetColor(&fg(big * sc));
                                self.triangle(xs, cy - s2 / 2.0, s2, flip);
                            }
                        }
                    }
                }
            }
            if big > 0.0 {
                self.draw_lyrics(&l, big);
            }
            dc.PopAxisAlignedClip();
        }
        self.draw_timer(&l);
        let (ta, clips_a, set_a, shelf_a, calc_a) = self.expanded_alpha(&l);
        for (al, f) in [(shelf_a, Self::draw_shelf as unsafe fn(&Self, &Layout, f32)), (calc_a, Self::draw_calc)] {
            if al > 0.0 {
                dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                f(self, &l, al);
                dc.PopAxisAlignedClip();
            }
        }
        let cca = self.big_alpha(&l) * self.va[View::Control as usize].x;
        if cca > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_control(&l, cca);
            dc.PopAxisAlignedClip();
        }
        let ma = self.big_alpha(&l) * self.va[View::Mixer as usize].x;
        if ma > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_mixer(&l, ma);
            dc.PopAxisAlignedClip();
        }
        let aa = self.big_alpha(&l) * self.va[View::Ai as usize].x;
        if aa > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_ai(&l, aa);
            dc.PopAxisAlignedClip();
        }
        let cla = self.big_alpha(&l) * self.va[View::Calendar as usize].x;
        if cla > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_calendar(&l, cla);
            dc.PopAxisAlignedClip();
        }
        let wa = self.big_alpha(&l) * self.va[View::Welcome as usize].x;
        if wa > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_welcome(&l, wa);
            dc.PopAxisAlignedClip();
        }
        if clips_a > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_clips(&l, clips_a);
            dc.PopAxisAlignedClip();
        }
        if set_a > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_settings(&l, set_a);
            dc.PopAxisAlignedClip();
        }
        if ta > 0.0 {
            // Top row: timer and clipboard on the left, mic and settings on the right.
            let dy = (1.0 - ta) * 6.0;
            let buttons = self.top_buttons(&l);
            for &(id, cx, cy) in &buttons {
                let k = if self.pressed == Some(id) { 1.0 - 0.18 * self.press.x } else { 1.0 };
                self.top_icon(id, cx, cy + dy, k, ta);
            }
            // Hovered button: a pill grows from it toward the middle and names it; the others make room.
            for &(id, cx, cy) in &buttons {
                let h = clamp01(self.hot[id].x);
                if h <= 0.0 {
                    continue;
                }
                let k = if self.pressed == Some(id) { 1.0 - 0.18 * self.press.x } else { 1.0 };
                let name: Vec<u16> = top_label(id).encode_utf16().collect();
                let tw = self.label_w(id);
                let (r, cy) = ((11.0 + 2.0 * h) * k, cy + dy);
                let w = 2.0 * r + self.label_grow(id);
                let right = cx < l.x + l.w / 2.0;
                let pill = if right { rect(cx - r, cy - r, w, 2.0 * r) } else { rect(cx + r - w, cy - r, w, 2.0 * r) };
                let rr = D2D1_ROUNDED_RECT { rect: pill, radiusX: r, radiusY: r };
                br.SetColor(&fg(0.1 * h * ta));
                dc.FillRoundedRectangle(&rr, br);
                self.top_icon(id, cx, cy, k * (1.0 + 0.06 * h), ta);
                dc.PushAxisAlignedClip(&pill, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                let lx = if right { cx + r - 2.0 } else { cx - r - tw + 2.0 };
                br.SetColor(&fg(ta * h));
                dc.DrawText(&name, &g.f_seg, &rect(lx, cy - 8.0, tw, 16.0), br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                dc.PopAxisAlignedClip();
            }
            // Clock in the middle of the row, local time in the user's own format.
            let (now, min, ms) = clock();
            let lab = buttons.iter().map(|&(id, _, _)| clamp01(self.hot[id].x)).fold(0.0, f32::max);
            br.SetColor(&fg(ta * 0.9 * (1.0 - lab)));
            let tx: Vec<u16> = now.encode_utf16().collect();
            dc.DrawText(&tx, &g.f_wbtn, &rect(l.x + l.w / 2.0 - 50.0, l.y + 10.0 + dy, 100.0, 20.0), br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            if self.clock_min.replace(min) != min {
                SetTimer(self.hwnd, T_CLOCK, ms + 50, None); // the timer only wakes the loop; animating() sees the new minute
            }
        }
        // Resting pill: clock with the date under it, gone as soon as anything else takes the island.
        let ka = clamp01(1.0 - self.music.x * 2.0) * clamp01(1.0 - self.clip.x * 2.0) * clamp01(1.0 - self.vol.x * 2.0) * clamp01(1.0 - l.e * 3.0);
        if ka > 0.0 {
            let (now, min, ms) = clock();
            let (cx, cy) = (l.x + l.w / 2.0, l.y + l.h / 2.0);
            br.SetColor(&fg(ka));
            dc.DrawText(&now.encode_utf16().collect::<Vec<_>>(), &g.f_wbtn, &rect(cx - 50.0, cy - 16.0, 100.0, 17.0), br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            br.SetColor(&fg(ka * 0.6));
            dc.DrawText(&today().encode_utf16().collect::<Vec<_>>(), &g.f_seg, &rect(cx - 50.0, cy + 1.0, 100.0, 15.0), br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            if self.clock_min.replace(min) != min {
                SetTimer(self.hwnd, T_CLOCK, ms + 50, None);
            }
        }
        if self.media.is_none() && self.timer.is_none() {
            let a = clamp01(1.0 - self.clip.x * 2.0) * clamp01(1.0 - self.vol.x * 2.0) * clamp01(1.0 - l.e * 3.0);
            self.mic_dot(l.x + l.w - 12.0, l.y + l.h / 2.0, a);
            let mo = clamp01(self.fx[FX_MOON].x);
            if mo > 0.0 {
                self.moon(l.x + 14.0, l.y + l.h / 2.0, 0.55 * (0.6 + 0.4 * self.fx[FX_MOON].x), a * mo); // Focus is on: small moon on the resting pill
            }
        }
        let ca = clamp01((self.clip.x - 0.45) / 0.55) * clamp01(1.0 - self.vol.x * 2.0);
        if ca > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let k = 0.65 + 0.35 * ca;
            let (ccx, ccy) = (l.x + 6.0 + 12.0, l.y + l.h / 2.0);
            if matches!(self.note, Note::Focus(_)) {
                self.moon(ccx, ccy, k, ca);
            } else if self.note == Note::Download || self.note == Note::Update {
                self.download_icon(ccx, ccy, k, ca);
            } else if matches!(self.note, Note::Buds(_)) {
                self.headphones(ccx, ccy, k, ca);
            } else if let Note::Usb(on) = self.note {
                self.usb_icon(ccx, ccy, k, ca, on, ca);
            } else if self.note == Note::Pick {
                self.top_icon(B_PICK, ccx, ccy, 1.35 * k, ca);
            } else if self.link.is_some() {
                // Favicon once it arrives, a globe until then (or if the site has none).
                let fa = if self.favicon.is_some() { clamp01(self.check.x) } else { 0.0 };
                if fa < 1.0 {
                    self.globe(ccx, ccy, k, ca * (1.0 - fa));
                }
                if let Some(f) = &self.favicon {
                    // Light tile, like app icons on iOS, so dark favicons don't vanish on the black pill.
                    br.SetColor(&rgba(0.95, 0.95, 0.96, ca * fa));
                    dc.FillRoundedRectangle(&rrect(ccx - 12.0 * k, ccy - 12.0 * k, 24.0 * k, 24.0 * k, 6.0 * k), br);
                    let s = 16.0 * k;
                    dc.DrawBitmap(f, Some(&rect(ccx - s / 2.0, ccy - s / 2.0, s, s)), ca * fa, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
                }
            } else {
            // Clipboard glyph in the art slot (24-unit box): board pops in, then the clip snaps down onto it.
            let p = |x: f32, y: f32| D2D_POINT_2F { x: ccx + (x - 12.0) * k, y: ccy + (y - 12.0) * k };
            let rr = |x: f32, y: f32, w: f32, h: f32, r: f32| {
                let a = p(x, y);
                rrect(a.x, a.y, w * k, h * k, r * k)
            };
            br.SetColor(&fg(ca));
            dc.DrawRoundedRectangle(&rr(5.0, 4.5, 14.0, 17.0, 3.2), br, 1.7 * k, &g.caps);
            let snap = clamp01(self.check.x);
            dc.DrawLine(p(8.5, 12.0), p(15.5, 12.0), br, 1.6 * k, &g.caps);
            dc.DrawLine(p(8.5, 16.0), p(13.0, 16.0), br, 1.6 * k, &g.caps);
            let ty_clip = 2.5 - 4.0 * (1.0 - snap); // clip drops from above
            // Black backing cuts the board outline under the clip so it reads as a separate piece.
            br.SetColor(&bg(ca));
            dc.FillRoundedRectangle(&rr(7.6, ty_clip - 0.9, 8.8, 5.8, 2.2), br);
            br.SetColor(&fg(ca * snap.max(0.3)));
            dc.FillRoundedRectangle(&rr(8.6, ty_clip, 6.8, 4.0, 1.6), br);
            }
            let opts = D2D1_DRAW_TEXT_OPTIONS_NONE;
            let mm = DWRITE_MEASURING_MODE_NATURAL;
            let dy = (1.0 - ca) * 5.0;
            let tx = l.x + 36.0; // same text column as the compact music state
            let mut tw = (l.x + l.w - 14.0 - tx).max(0.0);
            if let Some((tb, pw, ph)) = &self.shot {
                // Thumbnail on the right, cropped to fill a rounded tile; pops in with the notice.
                let dh = (l.h - 12.0) * (0.75 + 0.25 * ca);
                let dw = (dh * pw / ph).clamp(dh * 0.8, dh * 2.0);
                let (x, y) = (l.x + l.w - 12.0 - dw, l.y + (l.h - dh) / 2.0);
                let k = (dw / pw).max(dh / ph);
                tb.SetTransform(&mat(k, k, x + (dw - pw * k) / 2.0, y + (dh - ph * k) / 2.0));
                tb.SetOpacity(ca);
                dc.FillRoundedRectangle(&rrect(x, y, dw, dh, 5.0), tb);
                tw = (x - 8.0 - tx).max(0.0);
            }
            if let Some([r, gg, b, a]) = self.swatch {
                // Colour swatch on the right; a faint ring keeps dark colours visible on black.
                let rad = 11.0 * k;
                let c = D2D1_ELLIPSE { point: D2D_POINT_2F { x: l.x + l.w - 12.0 - 11.0, y: ccy }, radiusX: rad, radiusY: rad };
                br.SetColor(&rgba(r, gg, b, a * ca));
                dc.FillEllipse(&c, br);
                br.SetColor(&fg(0.25 * ca));
                dc.DrawEllipse(&c, br, 1.0, None);
                tw = (l.x + l.w - 12.0 - 22.0 - 8.0 - tx).max(0.0);
            }
            if let Note::Buds(Some(pct)) = self.note {
                // Battery ring on the right, iOS-style: green, red when low, with the % beside it.
                let (bx, r) = (l.x + l.w - 12.0 - 10.0, 9.0 * k);
                let (cr, cg, cb) = if pct <= 20 { (1.0, 0.27, 0.23) } else { GREEN };
                br.SetColor(&rgba(cr, cg, cb, ca * 0.3));
                dc.DrawEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: bx, y: ccy }, radiusX: r, radiusY: r }, br, 2.6, None);
                br.SetColor(&rgba(cr, cg, cb, ca));
                self.arc(bx, ccy, r, pct as f32 / 100.0, 2.6);
                dc.DrawText(&format!("{pct}%").encode_utf16().collect::<Vec<_>>(), &g.f_time_r, &rect(bx - 16.0 - 44.0, ccy - 8.0 + dy, 44.0, 16.0), br, opts, mm);
                tw = (bx - 16.0 - 44.0 - tx).max(0.0);
            }
            // Title and subtitle per notice kind.
            let (title, sub): (&str, &str) = match self.note {
                Note::Copy => (
                    "Copied",
                    match (&self.shot, &self.link) {
                        (Some(_), _) => "Click to edit",
                        (_, Some((_, host))) => host,
                        _ => &self.clip_text,
                    },
                ),
                Note::Focus(_) => ("Focus", &self.clip_text),
                Note::Download => (if self.dl.as_ref().is_some_and(|d| d.done) { "Downloaded" } else { "Downloading" }, &self.clip_text),
                Note::Buds(_) => (&self.clip_text, "Connected"),
                Note::Usb(on) => (&self.clip_text, if on { "Plugged in" } else { "Unplugged" }),
                Note::Pick => ("Pick a colour", &self.clip_text),
                Note::Update => (if self.updating { "Installing update" } else { "Update available" }, &self.clip_text),
            };
            let has_preview = !sub.is_empty();
            br.SetColor(&fg(ca));
            let ty = if has_preview { l.y + l.h / 2.0 - 16.0 } else { l.y + l.h / 2.0 - 9.0 };
            dc.DrawText(&title.encode_utf16().collect::<Vec<_>>(), &g.f_title_s, &rect(tx, ty + dy, tw, 18.0), br, opts, mm);
            if has_preview {
                br.SetColor(&fg(ca * 0.55));
                dc.DrawText(&sub.encode_utf16().collect::<Vec<_>>(), &g.f_artist_s, &rect(tx, l.y + l.h / 2.0 - 1.0 + dy, tw, 15.0), br, opts, mm);
            }
            dc.PopAxisAlignedClip();
        }
        let va = clamp01((self.vol.x - 0.45) / 0.55);
        if va > 0.0 {
            dc.PushAxisAlignedClip(&rect(l.x, l.y, l.w, l.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let (bx, len, cy) = self.vol_track(&l);
            let vs = &self.vsl;
            let mute = if self.bright { 0.0 } else { clamp01(self.fx[FX_MUTE].x) };
            let op = va * (0.7 + 0.3 * vs.a.x) * lerp(1.0, 0.45, mute);
            let (ls, _) = vs.draw(dc, br, bx, len, cy, op, 6.0);
            // Icon rides the left stretch, like ElasticSlider's start icon.
            let k = (0.7 + 0.3 * va) * (1.0 + 0.08 * vs.a.x);
            let icx = l.x + 24.0 - ls;
            if self.bright {
                self.sun(icx, cy, k, va, vs.p.x);
            } else {
            dc.SetTransform(&mat(k, k, icx - 12.0 * k, cy - 12.0 * k));
            br.SetColor(&fg(va));
            dc.FillGeometry(&g.speaker, br, None);
            let lvl = clamp01(vs.p.x) * (1.0 - mute);
            for (i, w) in g.waves.iter().enumerate() {
                let a = clamp01((lvl - i as f32 * 0.33) / 0.1); // waves fill in as volume rises
                if a > 0.0 {
                    br.SetColor(&fg(va * a));
                    dc.DrawGeometry(w, br, 1.8, &g.caps);
                }
            }
            if mute > 0.0 {
                // Cross grows out from its centre.
                let (c, h) = ((18.25, 12.0), 3.25 * (0.4 + 0.6 * mute));
                let pt = |dx: f32, dy: f32| D2D_POINT_2F { x: c.0 + dx * h, y: c.1 + dy * h * 1.08 };
                br.SetColor(&fg(va * mute));
                dc.DrawLine(pt(-1.0, -1.0), pt(1.0, 1.0), br, 1.8, &g.caps);
                dc.DrawLine(pt(1.0, -1.0), pt(-1.0, 1.0), br, 1.8, &g.caps);
            }
            }
            dc.SetTransform(&Matrix3x2::identity());
            dc.PopAxisAlignedClip();
        }
        dc.SetTransform(&Matrix3x2::identity());
        if fading {
            dc.PopLayer();
        }
        dc.EndDraw(None, None)?;
        g.swap.Present(1, DXGI_PRESENT(0)).ok()
    }

    /// Top-row buttons (id, center): quit, timer and clipboard on the left; calculator, shelf, mic
    /// and settings on the right. With the Timer switched off, the clipboard button takes its spot.
    fn top_buttons(&self, l: &Layout) -> Vec<(usize, f32, f32)> {
        let cy = l.y + 20.0;
        let r = l.x + l.w;
        let mut v = vec![(B_POWER, l.x + 30.0, cy), (B_CLIPS, l.x + 78.0, cy), (B_AI, r - 126.0, cy), (B_CALC, r - 102.0, cy), (B_SHELF, r - 78.0, cy), (B_MIC, r - 54.0, cy), (B_GEAR, r - 30.0, cy)];
        if self.cfg.pills[P_TIMER] {
            v.push((B_TIMER, l.x + 54.0, cy));
        } else {
            v[1].1 = l.x + 54.0;
        }
        v.push((B_PICK, v[1].1 + 24.0, cy));
        v.push((B_MIXER, v[1].1 + 48.0, cy));
        // A hovered button's label pill pushes the buttons between it and the clock toward the middle.
        let (mid, base) = (l.x + l.w / 2.0, v.clone());
        for &(j, cj, _) in &base {
            let g = self.label_grow(j);
            if g > 0.0 {
                for (b, &(_, cb, _)) in v.iter_mut().zip(&base) {
                    if cj < mid && cb > cj && cb < mid {
                        b.1 += g;
                    } else if cj > mid && cb < cj && cb > mid {
                        b.1 -= g;
                    }
                }
            }
        }
        v
    }

    /// Room a top-row button's label takes, in DIPs: its text plus padding.
    fn label_w(&self, id: usize) -> f32 {
        let name: Vec<u16> = top_label(id).encode_utf16().collect();
        let w = unsafe { self.g.dw.CreateTextLayout(&name, &self.g.f_seg, 200.0, 20.0) }.ok().and_then(|t| {
            let mut m = DWRITE_TEXT_METRICS::default();
            unsafe { t.GetMetrics(&mut m) }.ok().map(|_| m.width)
        });
        w.unwrap_or(60.0) + 12.0
    }

    /// How far a button's label pill has grown out of it right now (eased hover).
    fn label_grow(&self, id: usize) -> f32 {
        let h = clamp01(self.hot[id].x);
        if h > 0.0 { self.label_w(id) * h * h * (3.0 - 2.0 * h) } else { 0.0 }
    }

    /// Button under (x, y): the hovered one anywhere on its label pill, else the nearest icon.
    fn top_hit(&self, l: &Layout, x: f32, y: f32) -> Option<usize> {
        let buttons = self.top_buttons(l);
        let mid = l.x + l.w / 2.0;
        let on_pill = buttons.iter().find(|&&(id, cx, cy)| {
            let g = self.label_grow(id);
            let (a, b) = if cx < mid { (cx - 13.0, cx + 13.0 + g) } else { (cx - 13.0 - g, cx + 13.0) };
            self.hot[id].to > 0.5 && x >= a && x <= b && (y - cy).abs() < 13.0
        });
        if let Some(&(id, _, _)) = on_pill {
            return Some(id);
        }
        // Only the nearest button lights up; neighbours are 24 DIPs apart.
        let near = buttons.iter().map(|&(id, cx, cy)| (id, (x - cx).hypot(y - cy))).min_by(|a, b| a.1.total_cmp(&b.1));
        near.filter(|&(_, d)| d < 13.0).map(|(id, _)| id)
    }

    /// Opacity of the expanded top row (shown over the player, the clipboard list and settings),
    /// of the list, and of settings.
    fn expanded_alpha(&self, l: &Layout) -> (f32, f32, f32, f32, f32) {
        let big = self.big_alpha(l);
        let player = if self.media.is_some() { self.va[View::Player as usize].x } else { 0.0 };
        let clips = big * self.va[View::Clips as usize].x;
        let set = big * self.va[View::Settings as usize].x;
        let shelf = big * self.va[View::Shelf as usize].x;
        let calc = big * self.va[View::Calc as usize].x;
        let mixer = big * self.va[View::Mixer as usize].x;
        let ai = big * self.va[View::Ai as usize].x;
        let cal = big * self.va[View::Calendar as usize].x;
        (big * player + clips + set + shelf + calc + mixer + ai + cal, clips, set, shelf, calc)
    }

    /// Opacity of expanded-only content: late in the expansion, and out of a notice's way.
    fn big_alpha(&self, l: &Layout) -> f32 {
        let cf = clamp01(1.0 - self.clip.x * 2.0) * clamp01(1.0 - self.vol.x * 2.0);
        clamp01((l.e - 0.4) / 0.6) * clamp01((self.music.x.max(l.e) - 0.4) / 0.6) * cf
    }

    /// Recording dot, unless switched off in settings.
    fn mic_live(&self) -> bool {
        self.mic_rec && self.cfg.mic_dot
    }

    /// Green while another app records, orange if the mic is muted (ORANGE mixed in by fade).
    fn dot_rgb(&self) -> (f32, f32, f32) {
        let m = clamp01(self.fx[FX_MICMUTE].x);
        (lerp(GREEN.0, ORANGE.0, m), lerp(GREEN.1, ORANGE.1, m), lerp(GREEN.2, ORANGE.2, m))
    }

    /// Small live-mic indicator, sized and faded by its state fade.
    unsafe fn mic_dot(&self, x: f32, y: f32, a: f32) {
        let k = self.fx[FX_MIC].x.max(0.0);
        if k < 0.01 || a <= 0.0 {
            return;
        }
        let (r, g, b) = self.dot_rgb();
        self.g.brush.SetColor(&rgba(r, g, b, a * clamp01(k)));
        self.g.dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x, y }, radiusX: 3.0 * k, radiusY: 3.0 * k }, &self.g.brush);
    }

    /// Top-row line icons, drawn in a 24-unit box scaled to 16 DIPs around (cx, cy).
    unsafe fn top_icon(&self, id: usize, cx: f32, cy: f32, k: f32, a: f32) {
        let (dc, br, caps) = (&self.g.dc, &self.g.brush, &self.g.caps);
        let s = 16.0 / 24.0 * k;
        let p = |x: f32, y: f32| D2D_POINT_2F { x: cx + (x - 12.0) * s, y: cy + (y - 12.0) * s };
        let line = |x0: f32, y0: f32, x1: f32, y1: f32, w: f32| dc.DrawLine(p(x0, y0), p(x1, y1), br, w * s, caps);
        let ring = |x: f32, y: f32, r: f32, w: f32| {
            dc.DrawEllipse(&D2D1_ELLIPSE { point: p(x, y), radiusX: r * s, radiusY: r * s }, br, w * s, None)
        };
        let mm = if id == B_MIC { clamp01(self.fx[FX_MICMUTE].x) } else { 0.0 };
        let tint = match id {
            B_MIC => mm,
            B_TIMER => clamp01(self.fx[FX_TIMER].x),
            _ => 0.0,
        };
        let a = a * (1.0 + clamp01(self.cur[id].x.max(self.hot[id].x)) / 3.0); // open view or hovered: full white
        let (fr, fg_, fb) = fg_rgb();
        let (r, g, b) = (lerp(fr, ORANGE.0, tint), lerp(fg_, ORANGE.1, tint), lerp(fb, ORANGE.2, tint));
        br.SetColor(&rgba(r, g, b, a * lerp(0.75, 1.0, tint)));
        match id {
            B_TIMER => {
                // Stopwatch: dial, hand, top button and side crown.
                ring(12.0, 13.5, 7.6, 2.0);
                line(12.0, 13.5, 12.0, 9.5, 2.0);
                line(10.0, 3.2, 14.0, 3.2, 2.0);
                line(12.0, 3.2, 12.0, 5.8, 2.0);
                line(18.2, 6.6, 19.6, 5.2, 2.0);
            }
            B_CLIPS => {
                // Clipboard: board, clip on top, two text lines.
                let o = p(5.0, 4.5);
                dc.DrawRoundedRectangle(&rrect(o.x, o.y, 14.0 * s, 17.0 * s, 3.2 * s), br, 2.0 * s, caps);
                let c = p(8.6, 2.6);
                dc.FillRoundedRectangle(&rrect(c.x, c.y, 6.8 * s, 4.4 * s, 1.6 * s), br);
                line(8.5, 12.0, 15.5, 12.0, 2.0);
                line(8.5, 16.0, 13.0, 16.0, 2.0);
            }
            B_MIC => {
                let cap = |x: f32, y: f32, w: f32, h: f32, r: f32| {
                    let o = p(x, y);
                    rrect(o.x, o.y, w * s, h * s, r * s)
                };
                dc.FillRoundedRectangle(&cap(8.8, 2.5, 6.4, 11.5, 3.2), br);
                // Cradle geometry is in the 24-unit box: place it with a transform.
                dc.SetTransform(&mat(s, s, cx - 12.0 * s, cy - 12.0 * s));
                dc.DrawGeometry(&self.g.mic_cradle, br, 2.0, caps);
                dc.SetTransform(&Matrix3x2::identity());
                line(12.0, 17.2, 12.0, 21.0, 2.0);
                line(9.0, 21.0, 15.0, 21.0, 2.0);
                if mm > 0.0 {
                    // Slash draws itself across; a black under-stroke cuts the glyph so it reads cleanly.
                    let (x1, y1) = (lerp(4.0, 20.0, mm), lerp(3.0, 21.0, mm));
                    br.SetColor(&bg(a));
                    line(4.0, 3.0, x1, y1, 4.6);
                    br.SetColor(&rgba(r, g, b, a));
                    line(4.0, 3.0, x1, y1, 2.0);
                }
                let d = self.fx[FX_MIC].x.max(0.0);
                if d > 0.01 {
                    let (dr, dg, db) = self.dot_rgb();
                    br.SetColor(&rgba(dr, dg, db, a * clamp01(d)));
                    dc.FillEllipse(&D2D1_ELLIPSE { point: p(20.5, 3.5), radiusX: 2.6 * k * d, radiusY: 2.6 * k * d }, br);
                }
            }
            B_SHELF => {
                // Inbox tray: slanted sides, a floor, and the dip in the front edge.
                let pts = [(3.5, 13.0), (8.0, 13.0), (9.5, 15.8), (14.5, 15.8), (16.0, 13.0), (20.5, 13.0)];
                for w in pts.windows(2) {
                    line(w[0].0, w[0].1, w[1].0, w[1].1, 2.0);
                }
                line(3.5, 13.0, 6.8, 5.0, 2.0);
                line(6.8, 5.0, 17.2, 5.0, 2.0);
                line(17.2, 5.0, 20.5, 13.0, 2.0);
                let o = p(3.5, 13.0);
                dc.DrawRoundedRectangle(&rrect(o.x, o.y, 17.0 * s, 7.0 * s, 2.0 * s), br, 2.0 * s, caps);
                let d = self.fx[FX_SHELF].x.max(0.0);
                if d > 0.01 {
                    let (ar, ag, ab) = settings::accent();
                    br.SetColor(&rgba(ar, ag, ab, a * clamp01(d) / 0.75));
                    dc.FillEllipse(&D2D1_ELLIPSE { point: p(20.5, 4.0), radiusX: 2.6 * k * d, radiusY: 2.6 * k * d }, br);
                }
            }
            B_AI => {
                // Sparkle: a four-pointed star with curved sides, and a small one top right.
                let star = |cx: f32, cy: f32, r: f32| {
                    let Ok(geom) = self.g.factory.CreatePathGeometry() else { return };
                    let Ok(sink) = geom.Open() else { return };
                    let q = |x: f32, y: f32, ex: f32, ey: f32| sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: p(x, y), point2: p(ex, ey) });
                    let k = r * 0.16;
                    sink.BeginFigure(p(cx, cy - r), D2D1_FIGURE_BEGIN_FILLED);
                    q(cx + k, cy - k, cx + r, cy);
                    q(cx + k, cy + k, cx, cy + r);
                    q(cx - k, cy + k, cx - r, cy);
                    q(cx - k, cy - k, cx, cy - r);
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    if sink.Close().is_ok() {
                        dc.FillGeometry(&geom, br, None);
                    }
                };
                star(10.5, 13.0, 8.5);
                star(19.0, 5.0, 3.6);
            }
            B_MIXER => {
                // Mixer: three channel strips, each with its fader knob at a different height.
                for (x, k) in [(6.0, 14.5), (12.0, 8.0), (18.0, 16.5)] {
                    line(x, 4.0, x, 20.0, 1.8);
                    line(x - 2.8, k, x + 2.8, k, 2.6);
                }
            }
            B_PICK => {
                // Eyedropper, SF Symbols style: drawn upright, then tilted so the bulb sits top right.
                let rr = |x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
                    let o = p(x0, y0);
                    rrect(o.x, o.y, (x1 - x0) * s, (y1 - y0) * s, r * s)
                };
                dc.SetTransform(&Matrix3x2::rotation(45.0, cx, cy));
                dc.FillRoundedRectangle(&rr(9.3, 1.2, 14.7, 7.4, 2.7), br); // squeeze bulb
                dc.FillRoundedRectangle(&rr(7.4, 7.6, 16.6, 9.8, 1.1), br); // collar
                dc.DrawRoundedRectangle(&rr(10.1, 10.6, 13.9, 18.4, 1.4), br, 1.7 * s, caps); // glass tube
                line(12.0, 19.2, 12.0, 22.2, 1.7); // tip
                dc.SetTransform(&Matrix3x2::identity());
            }
            B_POWER => {
                // Power symbol: a ring open at the top, and a stem through the gap.
                dc.SetTransform(&Matrix3x2::rotation(40.0, cx, cy));
                self.arc(cx, cy, 7.5 * s, 280.0 / 360.0, 2.0 * s);
                dc.SetTransform(&Matrix3x2::identity());
                line(12.0, 3.0, 12.0, 11.0, 2.0);
            }
            B_CALC => {
                // Calculator: body, display, four keys.
                let o = p(5.0, 2.5);
                dc.DrawRoundedRectangle(&rrect(o.x, o.y, 14.0 * s, 19.0 * s, 3.2 * s), br, 2.0 * s, caps);
                line(8.5, 7.5, 15.5, 7.5, 2.0);
                for (x, y) in [(9.0, 12.5), (15.0, 12.5), (9.0, 17.0), (15.0, 17.0)] {
                    dc.FillEllipse(&D2D1_ELLIPSE { point: p(x, y), radiusX: 1.3 * s, radiusY: 1.3 * s }, br);
                }
            }
            B_GEAR | _ => {
                // Gear: flat teeth around a thick ring (the ring's inside is the hole).
                for t in 0..8 {
                    let ang = t as f32 * std::f32::consts::FRAC_PI_4;
                    let (c, sn) = (ang.cos(), ang.sin());
                    dc.DrawLine(p(12.0 + c * 5.5, 12.0 + sn * 5.5), p(12.0 + c * 9.6, 12.0 + sn * 9.6), br, 3.4 * s, None);
                }
                ring(12.0, 12.0, 5.8, 3.0);
            }
        }
    }

    /// Filled triangle with softly rounded corners, `size` tall, pointing right (or left if `flip`).
    unsafe fn triangle(&self, x: f32, y: f32, size: f32, flip: bool) {
        let w = size * 0.87;
        let m = if flip { mat(-size, size, x + w, y) } else { mat(size, size, x, y) };
        self.g.dc.SetTransform(&m);
        self.g.dc.FillGeometry(&self.g.tri, &self.g.brush, None);
        self.g.dc.SetTransform(&Matrix3x2::identity());
    }

    /// `sys_dpi` is the monitor's; the size preset scales it into the render dpi.
    unsafe fn resize(&mut self, sys_dpi: f32) -> Result<()> {
        self.sys_dpi = sys_dpi;
        let dpi = sys_dpi * SIZES[self.cfg.size].1;
        self.dpi = dpi;
        self.g.dc.SetTarget(None);
        let s = dpi / 96.0;
        self.g.swap.ResizeBuffers(0, (CW * s).ceil() as u32, (CH * s).ceil() as u32, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        bind_target(&self.g.swap, &self.g.dc, dpi)?;
        self.rgn = (0, 0, 0, 0);
        self.reposition();
        Ok(())
    }

    /// Moves the window to the display chosen in settings; following the mouse polls the pointer.
    fn reposition(&mut self) {
        let primary = unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) };
        let mon = match self.cfg.display {
            -1 => self.mon,
            0 => primary,
            n => settings::monitors().get(n as usize).copied().unwrap_or(primary),
        };
        unsafe {
            place(self.hwnd, self.dpi, mon);
            if self.cfg.display == -1 {
                SetTimer(self.hwnd, T_MON, 400, None);
            } else {
                let _ = KillTimer(self.hwnd, T_MON);
            }
        }
    }

    fn follow_mouse(&mut self) {
        let mut pt = POINT::default();
        let mon = unsafe {
            let _ = GetCursorPos(&mut pt);
            MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
        };
        if mon != self.mon {
            self.mon = mon;
            self.reposition();
        } else {
            unsafe { SetTimer(self.hwnd, T_MON, 400, None) };
        }
    }

    /// Priority page: something may take the island unless a higher-ranked thing holds it.
    fn may_show(&self, kind: u8) -> bool {
        let r = self.cfg.rank_of(kind);
        let above = |k: u8| k != kind && self.cfg.rank_of(k) < r;
        let note = match self.note {
            Note::Copy | Note::Pick => K_COPIED,
            Note::Update => K_DOWNLOADS,
            Note::Focus(_) => K_FOCUS,
            Note::Download => K_DOWNLOADS,
            Note::Buds(_) | Note::Usb(_) => K_EARBUDS,
        };
        !((self.clip.to > 0.5 && above(note))
            || (self.vol.to > 0.5 && above(K_VOLUME))
            || (self.timer.is_some() && above(K_TIMER))
            || (self.media.as_ref().is_some_and(|m| m.playing) && above(K_MUSIC)))
    }

    fn on_clipboard(&mut self) {
        if unsafe { GetClipboardOwner() } == Ok(self.hwnd) {
            return; // our own paste from the history list
        }
        let Some(text) = (unsafe { clipboard_preview(self.hwnd) }) else { return };
        if let Some(full) = unsafe { clips::clipboard_text(self.hwnd) } {
            self.record_clip(full);
        }
        if !self.cfg.pills[P_COPIED] || !self.may_show(K_COPIED) {
            return;
        }
        let p = self.cfg.pills;
        // "Image" is what the preview says for a copied bitmap (e.g. Win+Shift+S); show the picture instead.
        self.shot = if text == "Image" && p[P_SHOT] { unsafe { shot::thumbnail(self.hwnd, &self.g.dc).ok() } } else { None };
        self.swatch = extras::parse_color(&text).filter(|_| p[P_COLOUR]);
        self.link = extras::parse_link(&text).filter(|_| p[P_LINK]);
        self.favicon = None;
        self.fav_gen += 1;
        if let Some((url, _)) = &self.link {
            extras::fetch_favicon(self.hwnd, url, self.fav_gen);
        }
        self.note = Note::Copy;
        self.clip_text = text;
        if self.clip.to < 0.5 {
            self.check = Spring { x: -0.35, v: 0.0, to: 1.0 }; // negative start = short delay before drawing
        }
        self.clip.to = 1.0;
        self.vol.to = 0.0; // one notice at a time
        let ms = if self.clickable() { SHOT_MS } else { CLIP_MS };
        unsafe { SetTimer(self.hwnd, T_CLIP, ms, None) }; // re-copying extends the notice
    }

    /// The notice does something when clicked: edit a screenshot, open a link or a finished download.
    fn clickable(&self) -> bool {
        match self.note {
            Note::Copy => self.shot.is_some() || self.link.is_some(),
            Note::Download => self.dl.as_ref().is_some_and(|d| d.done),
            Note::Update => self.update.is_some(),
            _ => false,
        }
    }

    /// Shows a non-clipboard notice; `ms` None keeps it up until told otherwise.
    fn notice(&mut self, note: Note, text: String, ms: Option<u32>) {
        if self.control_open() {
            return; // don't swap Control Center out from under the pointer
        }
        let (kind, pill) = match note {
            Note::Copy => (K_COPIED, P_COPIED),
            Note::Focus(_) => (K_FOCUS, P_FOCUS),
            Note::Download => (K_DOWNLOADS, P_DOWNLOADS),
            Note::Buds(_) | Note::Usb(_) => (K_EARBUDS, P_EARBUDS), // "Devices" pill
            Note::Pick => (K_COPIED, P_COPIED),
            Note::Update => (K_DOWNLOADS, P_DOWNLOADS),
        };
        if !self.cfg.pills[pill] || !self.may_show(kind) {
            return;
        }
        self.note = note;
        self.shot = None;
        self.swatch = None;
        self.link = None;
        self.clip_text = text;
        self.clip.to = 1.0;
        self.vol.to = 0.0;
        unsafe {
            match ms {
                Some(ms) => {
                    SetTimer(self.hwnd, T_CLIP, ms, None);
                }
                None => {
                    let _ = KillTimer(self.hwnd, T_CLIP);
                }
            }
        }
    }

    fn on_dl(&mut self, ev: DlEvent) {
        match ev {
            DlEvent::Start(path) => {
                self.dl = Some(Download { path, done: false });
                self.dl_tick();
            }
            DlEvent::Done(path) => {
                let name = downloads::display_name(&path);
                self.dl = Some(Download { path, done: true });
                self.notice(Note::Download, name, Some(SHOT_MS));
                self.bump.v = 20.0;
                unsafe { let _ = KillTimer(self.hwnd, T_DL); }
            }
            DlEvent::Cancel(path) => {
                if self.dl.as_ref().is_some_and(|d| d.path == path) {
                    self.dl = None;
                    unsafe { let _ = KillTimer(self.hwnd, T_DL); }
                    if self.note == Note::Download {
                        self.clip.to = 0.0;
                    }
                }
            }
        }
    }

    /// While downloading: size so far, ring steps a quarter turn, again in 500 ms.
    fn dl_tick(&mut self) {
        let Some(d) = self.dl.as_ref().filter(|d| !d.done) else { return };
        let size = std::fs::metadata(&d.path).map_or(0, |m| m.len());
        let text = format!("{} · {}", downloads::display_name(&d.path), downloads::size_text(size));
        if self.note == Note::Download || self.clip.to < 0.5 {
            self.notice(Note::Download, text, None); // don't cut off a copy or focus notice
        }
        self.dl_spin.to += 0.25;
        unsafe { SetTimer(self.hwnd, T_DL, 500, None) };
    }

    fn on_usb(&mut self, plugged: bool, name: String) {
        self.notice(Note::Usb(plugged), name, Some(CLIP_MS));
        if plugged {
            self.bump.v = 20.0;
        }
    }

    fn on_buds(&mut self, name: String, battery: Option<u8>) {
        self.notice(Note::Buds(battery), name, Some(SHOT_MS));
        self.bump.v = 20.0;
    }

    fn on_favicon(&mut self, generation: u64, px: Vec<u8>) {
        if generation != self.fav_gen {
            return;
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        self.favicon = unsafe { self.g.dc.CreateBitmap(D2D_SIZE_U { width: FAV, height: FAV }, Some(px.as_ptr() as _), FAV * 4, &props).ok() };
        self.check = Spring { x: 0.0, v: 0.0, to: 1.0 }; // crossfade the globe into the favicon
    }

    fn on_focus(&mut self, active: bool, silent: bool) {
        let changed = active != self.focus_active;
        self.focus_active = active;
        if silent || !changed {
            return;
        }
        self.notice(Note::Focus(active), if active { "On" } else { "Off" }.to_string(), Some(CLIP_MS));
    }

    fn click(&mut self, x: f32, y: f32) {
        if self.vol.to > 0.5 {
            if self.vol_hit(x, y) {
                self.vsl.drag = true; // jumpToClick
                unsafe { SetCapture(self.hwnd) };
                self.vol_follow(x);
            }
            return;
        }
        if self.clip.to > 0.5 && self.clickable() {
            if self.note == Note::Update {
                return self.run_update();
            }
            if self.note == Note::Download {
                self.dl_press = Some((x, y)); // decided on release (open) or on move (drag out)
                unsafe { SetCapture(self.hwnd) };
                return;
            }
            if let Some((url, _)) = &self.link {
                unsafe {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                    ShellExecuteW(None, w!("open"), &HSTRING::from(url.as_str()), None, None, SW_SHOWNORMAL);
                }
            } else if let Err(e) = unsafe { shot::edit_in_snipping_tool(self.hwnd) } {
                eprintln!("open screenshot failed: {e:?}");
            }
            self.clip.to = 0.0;
            unsafe { let _ = KillTimer(self.hwnd, T_CLIP); }
            return;
        }
        let l = self.layout();
        if l.e < 0.9 {
            // Click on compact expands; on the resting pill it opens the clipboard list.
            if self.expand.to < 0.5 {
                self.set_view(self.default_view());
            }
            self.expand.to = 1.0;
            return;
        }
        if self.view == View::Welcome {
            return self.welcome_click(x, y);
        }
        if self.view == View::Control {
            return self.control_click(x, y);
        }
        if self.expanded_alpha(&l).0 > 0.5 {
            if let Some(id) = self.top_hit(&l, x, y) {
                self.pressed = Some(id);
                self.press = Spring { x: 1.0, v: 0.0, to: 0.0 };
                match id {
                    B_MIC => self.toggle_mic(),
                    B_TIMER => self.set_view(if self.timer.is_some() { View::Run } else { View::Pick }),
                    // Toggles between the list and the player (or stays when nothing plays).
                    B_CLIPS => self.set_view(if self.view == View::Clips && self.media.is_some() { View::Player } else { View::Clips }),
                    B_POWER => unsafe { PostQuitMessage(0) },
                    B_PICK => self.start_pick(),
                    B_AI => {
                        if self.view == View::Ai {
                            self.set_view(self.default_view());
                        } else {
                            self.open_ai();
                        }
                    }
                    B_MIXER => {
                        if self.view == View::Mixer {
                            self.set_view(self.default_view());
                        } else {
                            self.open_mixer();
                        }
                    }
                    B_SHELF => self.set_view(if self.view == View::Shelf { self.default_view() } else { View::Shelf }),
                    B_CALC => {
                        if self.view == View::Calc {
                            self.set_view(self.default_view());
                        } else {
                            self.open_calc();
                        }
                    }
                    _ => {
                        if self.view == View::Settings {
                            self.set_view(self.default_view());
                        } else {
                            self.su.mons = settings::monitors();
                            self.su.startup = system::startup_enabled();
                            self.set_view(View::Settings);
                        }
                    }
                }
                return;
            }
            // The clock in the middle of the row opens the calendar.
            if (x - (l.x + l.w / 2.0)).abs() < 36.0 && (y - (l.y + 20.0)).abs() < 13.0 {
                return if self.view == View::Calendar { self.set_view(self.default_view()) } else { self.open_calendar() };
            }
        }
        if self.calendar_click(x, y) || self.ai_click(x, y) || self.mixer_click(x, y) || self.settings_click(x, y) || self.clips_click(x, y) || self.shelf_click(x, y) || self.view == View::Calc || self.timer_click(x, y) || self.media.is_none() {
            return;
        }
        if self.slider_hit(x, y) {
            self.sl.drag = true; // jumpToClick: fill snaps to the pointer
            unsafe { SetCapture(self.hwnd) };
            self.follow(x);
            return;
        }
        for (i, (cx, cy)) in self.buttons(&l).into_iter().enumerate() {
            if (x - cx).hypot(y - cy) < 24.0 {
                self.pressed = Some(i);
                self.press = Spring { x: 1.0, v: 0.0, to: 0.0 };
                match i {
                    1 => self.pp.to = 1.0 - self.pp.to, // optimistic; the next media snapshot confirms
                    _ => self.skip[i / 2] = Spring { x: 0.0, v: 0.0, to: 1.0 },
                }
                let _ = self.tx.send([Cmd::Prev, Cmd::Toggle, Cmd::Next].into_iter().nth(i).unwrap());
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DRAG {
        // Drag-and-drop runs its own modal loop; do it without holding the app so the island
        // keeps animating (it collapses) while the file is being dragged.
        let drag = APP.with(|a| {
            let mut a = a.try_borrow_mut().ok()?;
            let app = a.as_mut()?;
            app.clip.to = 0.0;
            if app.drag.as_ref().is_some_and(|d| d.1) {
                app.expand.to = 0.0; // out of the way while the shelf file is dragged
            }
            app.drag.take()
        });
        if let Some((p, from_shelf)) = drag {
            // A shelf file dropped somewhere has left the shelf.
            if downloads::drag_out(&p) == Ok(true) && from_shelf {
                APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(|app| app.shelf.retain(|it| it.path != p)));
            }
        }
        return LRESULT(0);
    }
    // try_borrow: SetWindowRgn etc. can re-enter while the app is borrowed for drawing.
    let handled = APP.with(|a| {
        let Ok(mut a) = a.try_borrow_mut() else { return None };
        let app = a.as_mut()?;
        let s = app.dpi / 96.0;
        match msg {
            WM_MEDIA => app.on_media(),
            WM_CLIPBOARDUPDATE => app.on_clipboard(),
            WM_VOLUME => app.on_volume(wp.0 as f32 / 10000.0, lp.0 != 0),
            WM_MIC => app.mic_muted = lp.0 != 0,
            WM_MIC_USE => app.mic_rec = wp.0 != 0,
            WM_FAVICON => {
                let (generation, px) = *Box::from_raw(lp.0 as *mut (u64, Vec<u8>));
                app.on_favicon(generation, px);
            }
            WM_BRIGHT => app.bright_level = Some(wp.0 as f32 / 10000.0),
            WM_FOCUS => app.on_focus(wp.0 != 0, lp.0 != 0),
            WM_DL => app.on_dl(*Box::from_raw(lp.0 as *mut DlEvent)),
            WM_SHELF_HOVER => app.on_shelf_hover(wp.0 != 0),
            WM_SHELF_ADD => app.on_shelf_add(*Box::from_raw(lp.0 as *mut Vec<std::path::PathBuf>)),
            WM_LYRICS => {
                let (key, lines) = *Box::from_raw(lp.0 as *mut (String, lyrics::Lines));
                app.on_lyrics(key, lines);
            }
            WM_CHAR => {
                app.calc_char(wp.0 as u32);
                app.ai_char(wp.0 as u32);
            }
            WM_FULLSCREEN => app.on_fullscreen(wp.0 != 0),
            WM_ACTIVATE => {
                if wp.0 & 0xffff == WA_INACTIVE as usize && matches!(app.view, View::Calc | View::Ai) {
                    app.expand.to = 0.0; // clicked away from the calculator or the chat
                }
                return None;
            }
            WM_PICK => app.on_pick(wp.0, lp.0 as u32),
            WM_UPDATE => app.on_update(wp.0, lp.0),
            WM_RADIO => app.on_radio(wp.0),
            WM_AI => {
                let (path, answer) = *Box::from_raw(lp.0 as *mut (std::path::PathBuf, std::result::Result<ai::Reply, String>));
                app.on_ai(path, answer);
            }
            WM_USB => app.on_usb(wp.0 == 1, *Box::from_raw(lp.0 as *mut String)),
            WM_BUDS => {
                let (name, battery) = *Box::from_raw(lp.0 as *mut (String, Option<u8>));
                app.on_buds(name, battery);
            }
            WM_MOUSEACTIVATE => return Some(LRESULT(MA_NOACTIVATE as isize)),
            WM_MOUSEMOVE => {
                let (x, y) = ((lp.0 & 0xffff) as i16 as f32 / s, ((lp.0 >> 16) & 0xffff) as i16 as f32 / s);
                let l = app.layout();
                // While resizing the hit region is the whole canvas; only the island itself counts as hover.
                if !app.hover && x >= l.x && x <= l.x + l.w && y >= l.y && y <= l.y + l.h {
                    app.hover = true;
                    let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    });
                    let _ = KillTimer(hwnd, T_LEAVE);
                    SetTimer(hwnd, T_HOVER, 80, None);
                }
                app.mouse = (x, y);
                if let Some((px, py, i)) = app.shelf_press {
                    if (x - px).hypot(y - py) > 5.0 {
                        app.shelf_press = None;
                        app.drag = app.shelf.get(i).map(|it| (it.path.clone(), true));
                        let _ = ReleaseCapture();
                        let _ = PostMessageW(hwnd, WM_DRAG, WPARAM(0), LPARAM(0));
                    }
                }
                if let Some((px, py)) = app.dl_press {
                    if (x - px).hypot(y - py) > 5.0 {
                        app.dl_press = None;
                        app.drag = app.dl.as_ref().map(|d| (d.path.clone(), false));
                        let _ = ReleaseCapture();
                        let _ = PostMessageW(hwnd, WM_DRAG, WPARAM(0), LPARAM(0));
                    }
                }
                app.sl.hover = app.slider_hit(x, y);
                app.vsl.hover = app.vol_hit(x, y);
                if app.sl.drag {
                    app.follow(x);
                }
                if app.vsl.drag {
                    app.vol_follow(x);
                }
                app.wheel_move(y);
                app.settings_move(x, y);
                app.control_move(x, y);
                app.mixer_move(x, y);
            }
            WM_MOUSEWHEEL => {
                let mut pt = POINT { x: (lp.0 & 0xffff) as i16 as i32, y: ((lp.0 >> 16) & 0xffff) as i16 as i32 };
                let _ = ScreenToClient(hwnd, &mut pt);
                let notches = ((wp.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
                let (x, y) = (pt.x as f32 / s, pt.y as f32 / s);
                if app.view == View::Pick && app.expand.x > 0.5 {
                    app.wheel_scroll(x, y, notches);
                } else if app.view == View::Ai && app.expand.to > 0.5 {
                    app.ai_scroll(notches);
                } else if app.calendar_wheel(notches) {
                } else if app.control_wheel(notches) {
                } else if notches < 0.0 && app.view != View::Welcome {
                    app.open_control(); // scroll down: Control Center, like pulling it down on iOS
                }
            }
            WM_MOUSELEAVE => {
                app.hover = false;
                app.sl.hover = false;
                app.vsl.hover = false;
                let _ = KillTimer(hwnd, T_HOVER);
                // The picker is fiddly; give the pointer longer to come back before closing it.
                SetTimer(hwnd, T_LEAVE, if app.view == View::Pick { 900 } else { 250 }, None);
            }
            WM_TIMER => {
                let _ = KillTimer(hwnd, wp.0);
                if wp.0 == T_UPDATE {
                    app.update_tick();
                } else if wp.0 == T_CLIP {
                    if app.hover && app.clickable() {
                        SetTimer(hwnd, T_CLIP, CLIP_MS, None); // don't vanish from under the pointer
                    } else if app.dl.as_ref().is_some_and(|d| !d.done) {
                        app.note = Note::Download; // a download is still running: go back to it
                        app.dl_tick();
                    } else {
                        app.clip.to = 0.0;
                    }
                } else if wp.0 == T_MON {
                    app.follow_mouse();
                } else if wp.0 == T_DL {
                    app.dl_tick();
                } else if wp.0 == T_VOL {
                    if app.hover || app.vsl.drag {
                        SetTimer(hwnd, T_VOL, VOL_MS, None); // keep it up while the user is on it
                    } else {
                        app.vol.to = 0.0;
                    }
                } else if wp.0 == T_TICK {
                    app.tick();
                } else if wp.0 == T_DONE {
                    app.cancel_timer();
                } else if wp.0 == T_HOVER && app.hover && app.vol.to < 0.5 && app.clip.to < 0.5 {
                    if app.expand.to < 0.5 {
                        app.set_view(app.default_view());
                    }
                    app.expand.to = 1.0;
                } else if wp.0 == T_LEAVE && !app.hover && !app.sl.drag && app.wdrag.is_none() && app.su.drag.is_none() && app.cc.drag.is_none() && app.shelf_press.is_none() && app.shelf_drop.to < 0.5
                    && !(matches!(app.view, View::Calc | View::Ai) && GetForegroundWindow() == hwnd)
                    && app.view != View::Welcome // the tour closes with its own buttons
                {
                    app.expand.to = 0.0;
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = ((lp.0 & 0xffff) as i16 as f32 / s, ((lp.0 >> 16) & 0xffff) as i16 as f32 / s);
                app.click(x, y);
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED => app.release(),
            WM_DPICHANGED => {
                if let Err(e) = app.resize((wp.0 & 0xffff) as f32) {
                    eprintln!("resize failed: {e:?}");
                }
            }
            WM_DISPLAYCHANGE => app.reposition(),
            _ => return None,
        }
        Some(LRESULT(0))
    });
    handled.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
}

/// One-line preview of the clipboard for the "Copied" notice. None means stay quiet: the copying
/// app (typically a password manager) asked clipboard monitors to ignore it.
unsafe fn clipboard_preview(hwnd: HWND) -> Option<String> {
    let fmt = |name: PCWSTR| RegisterClipboardFormatW(name);
    if IsClipboardFormatAvailable(fmt(w!("ExcludeClipboardContentFromMonitorProcessing"))).is_ok()
        || IsClipboardFormatAvailable(fmt(w!("Clipboard Viewer Ignore"))).is_ok()
    {
        return None;
    }
    if OpenClipboard(hwnd).is_err() {
        return Some(String::new()); // another app holds it; still confirm the copy
    }
    let text = (|| {
        // Marked as not for clipboard history (DWORD 0): confirm the copy, but don't show it.
        if let Ok(h) = GetClipboardData(fmt(w!("CanIncludeInClipboardHistory"))) {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
            let hide = !p.is_null() && *p == 0;
            let _ = GlobalUnlock(HGLOBAL(h.0));
            if hide {
                return String::new();
            }
        }
        if let Ok(h) = GetClipboardData(13) {
            // CF_UNICODETEXT: first non-blank line, capped, without copying the whole buffer.
            let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if p.is_null() {
                return String::new();
            }
            let mut line = Vec::new();
            for i in 0..100_000 {
                let c = *p.add(i);
                if c == 0 || line.len() >= 120 {
                    break;
                }
                let ws = matches!(c, 9 | 10 | 13 | 32);
                if line.is_empty() && ws {
                    continue;
                }
                if c == 10 || c == 13 {
                    break;
                }
                line.push(c);
            }
            let _ = GlobalUnlock(HGLOBAL(h.0));
            return String::from_utf16_lossy(&line);
        }
        if let Ok(h) = GetClipboardData(15) {
            // CF_HDROP: file name, or a count for several files.
            let drop = HDROP(h.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            if n != 1 {
                return format!("{n} files");
            }
            let mut buf = [0u16; 260];
            let len = DragQueryFileW(drop, 0, Some(&mut buf)) as usize;
            let path = String::from_utf16_lossy(&buf[..len]);
            return path.rsplit('\\').next().unwrap_or_default().to_string();
        }
        if IsClipboardFormatAvailable(8).is_ok() || IsClipboardFormatAvailable(2).is_ok() {
            return "Image".to_string(); // CF_DIB / CF_BITMAP
        }
        String::new()
    })();
    let _ = CloseClipboard();
    Some(text)
}

/// Any audio session event on a microphone; the watcher thread re-counts active recordings.
#[implement(IAudioSessionEvents, IAudioSessionNotification)]
struct MicEvents(std::sync::Mutex<std::sync::mpsc::Sender<Option<IAudioSessionControl>>>);

impl MicEvents_Impl {
    fn poke(&self, new: Option<IAudioSessionControl>) {
        let _ = self.0.lock().unwrap().send(new);
    }
}

impl IAudioSessionEvents_Impl for MicEvents_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(&self, _: f32, _: BOOL, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnChannelVolumeChanged(&self, _: u32, _: *const f32, _: u32, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnStateChanged(&self, _: AudioSessionState) -> Result<()> {
        self.poke(None);
        Ok(())
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> Result<()> {
        self.poke(None);
        Ok(())
    }
}

impl IAudioSessionNotification_Impl for MicEvents_Impl {
    fn OnSessionCreated(&self, new: Option<&IAudioSessionControl>) -> Result<()> {
        self.poke(new.cloned());
        Ok(())
    }
}

/// Watches every microphone's audio sessions and posts WM_MIC_USE whenever "some other app is
/// recording" flips. Event-driven: the thread sleeps on a channel between session events.
/// shortcut: mics plugged in after startup aren't watched until a restart.
fn watch_mic_use(hwnd: HWND) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };
        let Ok(devs) = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        let events: IAudioSessionEvents = MicEvents(std::sync::Mutex::new(tx)).into();
        let notify: IAudioSessionNotification = events.cast().unwrap();
        let mut mgrs = Vec::new();
        let mut watched: Vec<IAudioSessionControl> = Vec::new();
        for i in 0..devs.GetCount().unwrap_or(0) {
            let Ok(mgr) = devs.Item(i).and_then(|d| d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)) else { continue };
            // Listing sessions once is what turns on new-session notifications.
            if let Ok(list) = mgr.GetSessionEnumerator() {
                for j in 0..list.GetCount().unwrap_or(0) {
                    if let Ok(s) = list.GetSession(j) {
                        let _ = s.RegisterAudioSessionNotification(&events);
                        watched.push(s);
                    }
                }
            }
            let _ = mgr.RegisterSessionNotification(&notify);
            mgrs.push(mgr);
        }
        let me = std::process::id();
        let mut last = None;
        loop {
            let live = mgrs.iter().any(|m| {
                let Ok(list) = m.GetSessionEnumerator() else { return false };
                (0..list.GetCount().unwrap_or(0)).any(|j| {
                    let Ok(s) = list.GetSession(j) else { return false };
                    let ours = s.cast::<IAudioSessionControl2>().and_then(|s2| s2.GetProcessId()).is_ok_and(|pid| pid == me);
                    !ours && s.GetState().is_ok_and(|st| st == AudioSessionStateActive)
                })
            });
            if last != Some(live) {
                last = Some(live);
                let _ = PostMessageW(HWND(hwnd as _), WM_MIC_USE, WPARAM(live as usize), LPARAM(0));
            }
            let Ok(new) = rx.recv() else { break };
            for s in std::iter::once(new).chain(rx.try_iter()).flatten() {
                let _ = s.RegisterAudioSessionNotification(&events);
                watched.push(s);
            }
        }
    });
}
