// Background thread that owns the Windows media session (SMTC) and publishes snapshots to the UI.
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use windows::core::*;
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Media::Control::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::WinRT::CreateStreamOverRandomAccessStream;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const ART: u32 = 128;
pub const WM_MEDIA: u32 = 0x8001; // WM_APP + 1

pub struct Art {
    pub pixels: Vec<u8>, // ART x ART premultiplied BGRA
    pub accent: [f32; 3],
}

#[derive(Clone)]
pub struct Media {
    pub title: String,
    pub artist: String,
    pub art: Option<Arc<Art>>,
    pub playing: bool,
    pub pos: f64,    // seconds at `pos_at`
    pub dur: f64,    // seconds
    pub pos_at: i64, // FILETIME ticks (100ns since 1601)
}

pub enum Cmd {
    Refresh,
    Prev,
    Toggle,
    Next,
    Seek(f64), // seconds from track start
}

pub type Shared = Arc<Mutex<Option<Media>>>;
/// Music filter from settings: (mode, per-app choices); see `config::music_allows`.
pub type Filter = Arc<Mutex<(usize, Vec<(String, bool)>)>>;
/// Ids of every media app seen, for the settings page to list.
pub type Seen = Arc<Mutex<Vec<String>>>;

pub fn start(hwnd: isize, filter: Filter) -> (Shared, Sender<Cmd>, Seen) {
    let shared: Shared = Arc::new(Mutex::new(None));
    let seen: Seen = Default::default();
    let (tx, rx) = channel();
    let (s, t, sn) = (shared.clone(), tx.clone(), seen.clone());
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Err(e) = run(hwnd, s, t, rx, filter, sn) {
            eprintln!("media thread stopped: {e:?}");
        }
    });
    (shared, tx, seen)
}

/// The session to show: Windows' current one if the filter allows it, else the first allowed one,
/// preferring one that's playing.
fn pick(mgr: &GlobalSystemMediaTransportControlsSessionManager, filter: &Filter, seen: &Seen) -> Option<GlobalSystemMediaTransportControlsSession> {
    let id = |s: &GlobalSystemMediaTransportControlsSession| s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
    let all: Vec<_> = mgr.GetSessions().ok()?.into_iter().collect();
    {
        let mut sn = seen.lock().unwrap();
        for s in &all {
            let i = id(s);
            if !i.is_empty() && !sn.contains(&i) {
                sn.push(i);
            }
        }
    }
    let (mode, apps) = filter.lock().unwrap().clone();
    let ok = |s: &GlobalSystemMediaTransportControlsSession| crate::config::music_allows(mode, &apps, &id(s));
    let playing = |s: &GlobalSystemMediaTransportControlsSession| {
        s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).is_ok_and(|st| st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
    };
    mgr.GetCurrentSession().ok().filter(|s| ok(s)).or_else(|| {
        let allowed: Vec<_> = all.into_iter().filter(|s| ok(s)).collect();
        allowed.iter().find(|s| playing(s)).or(allowed.first()).cloned()
    })
}

unsafe fn run(hwnd: isize, shared: Shared, tx: Sender<Cmd>, rx: Receiver<Cmd>, filter: Filter, seen: Seen) -> Result<()> {
    let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
    let t = tx.clone();
    mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Cmd::Refresh);
        Ok(())
    }))?;
    let t = tx.clone();
    mgr.SessionsChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Cmd::Refresh);
        Ok(())
    }))?;
    let _ = tx.send(Cmd::Refresh);

    let mut hooked: Option<(GlobalSystemMediaTransportControlsSession, [EventRegistrationToken; 3])> = None;
    let mut last_key = String::new();
    let mut last_art: Option<Arc<Art>> = None;

    while let Ok(first) = rx.recv() {
        // Apps report a track switch as a burst (title, artist, art, state); wait for it to settle
        // so the UI animates once to the final state instead of restarting on every piece.
        if matches!(first, Cmd::Refresh) {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        let mut refresh = false;
        for cmd in std::iter::once(first).chain(rx.try_iter()) {
            let s = hooked.as_ref().map(|h| h.0.clone()); // controls act on the session we show
            match (cmd, &s) {
                (Cmd::Refresh, _) => refresh = true,
                (Cmd::Prev, Some(s)) => drop(s.TrySkipPreviousAsync()),
                (Cmd::Toggle, Some(s)) => drop(s.TryTogglePlayPauseAsync()),
                (Cmd::Next, Some(s)) => drop(s.TrySkipNextAsync()),
                (Cmd::Seek(t), Some(s)) => {
                    let start = s.GetTimelineProperties().and_then(|tl| tl.StartTime()).map_or(0, |d| d.Duration);
                    drop(s.TryChangePlaybackPositionAsync(start + (t * 1e7) as i64));
                }
                _ => {}
            }
        }
        if !refresh {
            continue;
        }

        let session = pick(&mgr, &filter, &seen);
        let id = |s: &GlobalSystemMediaTransportControlsSession| s.SourceAppUserModelId().unwrap_or_default();
        if hooked.as_ref().map(|h| id(&h.0)) != session.as_ref().map(id) {
            if let Some((old, tk)) = hooked.take() {
                let _ = old.RemoveMediaPropertiesChanged(tk[0]);
                let _ = old.RemovePlaybackInfoChanged(tk[1]);
                let _ = old.RemoveTimelinePropertiesChanged(tk[2]);
            }
            if let Some(s) = &session {
                macro_rules! h {
                    () => {{
                        let t = tx.clone();
                        &TypedEventHandler::new(move |_, _| {
                            let _ = t.send(Cmd::Refresh);
                            Ok(())
                        })
                    }};
                }
                let tk = [s.MediaPropertiesChanged(h!())?, s.PlaybackInfoChanged(h!())?, s.TimelinePropertiesChanged(h!())?];
                hooked = Some((s.clone(), tk));
            }
        }

        let media = session.and_then(|s| snapshot(&s, &wic, &mut last_key, &mut last_art).ok());
        *shared.lock().unwrap() = media;
        let _ = PostMessageW(HWND(hwnd as _), WM_MEDIA, WPARAM(0), LPARAM(0));
    }
    Ok(())
}

unsafe fn snapshot(
    s: &GlobalSystemMediaTransportControlsSession,
    wic: &IWICImagingFactory,
    last_key: &mut String,
    last_art: &mut Option<Arc<Art>>,
) -> Result<Media> {
    let props = s.TryGetMediaPropertiesAsync()?.get()?;
    let title = props.Title()?.to_string();
    let artist = props.Artist()?.to_string();
    let key = format!("{title}\u{1}{artist}");
    // Decode art only on track change, or when the app sends the thumbnail after the title.
    if key != *last_key || last_art.is_none() {
        *last_key = key;
        *last_art = props.Thumbnail().ok().and_then(|t| decode(wic, &t).ok()).map(Arc::new);
    }
    let playing = s.GetPlaybackInfo()?.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
    let tl = s.GetTimelineProperties()?;
    let start = tl.StartTime()?.Duration;
    Ok(Media {
        title,
        artist,
        art: last_art.clone(),
        playing,
        pos: (tl.Position()?.Duration - start) as f64 / 1e7,
        dur: (tl.EndTime()?.Duration - start) as f64 / 1e7,
        pos_at: tl.LastUpdatedTime()?.UniversalTime,
    })
}

unsafe fn decode(wic: &IWICImagingFactory, r: &windows::Storage::Streams::IRandomAccessStreamReference) -> Result<Art> {
    let stream = r.OpenReadAsync()?.get()?;
    let istream: IStream = CreateStreamOverRandomAccessStream(&stream)?;
    let frame = wic.CreateDecoderFromStream(&istream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?.GetFrame(0)?;
    let (mut w, mut h) = (0, 0);
    frame.GetSize(&mut w, &mut h)?;
    // Scale so the short side is ART, then center-crop to a square (handles 16:9 video thumbnails).
    let k = ART as f64 / w.min(h).max(1) as f64;
    let (sw, sh) = (((w as f64 * k).round() as u32).max(ART), ((h as f64 * k).round() as u32).max(ART));
    let scaler = wic.CreateBitmapScaler()?;
    scaler.Initialize(&frame, sw, sh, WICBitmapInterpolationModeFant)?;
    let conv = wic.CreateFormatConverter()?;
    conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
    let mut pixels = vec![0u8; (ART * ART * 4) as usize];
    let rc = WICRect { X: ((sw - ART) / 2) as i32, Y: ((sh - ART) / 2) as i32, Width: ART as i32, Height: ART as i32 };
    conv.CopyPixels(&rc, ART * 4, &mut pixels)?;
    Ok(Art { accent: accent(&pixels), pixels })
}

/// Saturation-weighted average color, lifted so it reads on black. Returns RGB 0..1.
fn accent(px: &[u8]) -> [f32; 3] {
    let (mut acc, mut wsum) = ([0f32; 3], 0f32);
    for p in px.chunks_exact(4).step_by(3) {
        let (b, g, r) = (p[0] as f32 / 255., p[1] as f32 / 255., p[2] as f32 / 255.);
        let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
        let w = (mx - mn) * mx + 0.01;
        acc = [acc[0] + r * w, acc[1] + g * w, acc[2] + b * w];
        wsum += w;
    }
    let c = acc.map(|v| v / wsum);
    let mx = c[0].max(c[1]).max(c[2]).max(0.01);
    let lift = (0.9 / mx).min(4.0); // normalize brightness, keep hue
    c.map(|v| (v * lift).min(1.0) * 0.85 + 0.15)
}
