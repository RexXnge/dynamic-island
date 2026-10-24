// Auto-update from GitHub releases: checks the latest release 30 s after launch and every 6 h.
// A newer tag shows an "Update available" pill; clicking it downloads the release's installer,
// checks its SHA-256 against the digest GitHub publishes for the asset, and runs it silently
// (the installer stops the island and starts the new one). Nothing runs unless the hash matches.
// The Updates switch (Pills page) turns all of this off, including the check itself.
// shortcut: the repo must be public; a private repo's API answers 404, so no update is found.
use super::*;
use std::os::windows::process::CommandExt;
use windows::Data::Json::JsonObject;
use windows::Foundation::Uri;
use windows::Security::Cryptography::Core::{HashAlgorithmNames, HashAlgorithmProvider};
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Web::Http::HttpClient;

pub const WM_UPDATE: u32 = 0x8010; // wparam = UP_FOUND (lparam Box<Update>) or UP_FAILED
pub const T_UPDATE: usize = 10;
const UP_FOUND: usize = 0;
const UP_FAILED: usize = 1;
const API: &str = "https://api.github.com/repos/RexXnge/dynamic-island/releases/latest";
const FIRST_MS: u32 = 30_000;
const EVERY_MS: u32 = 6 * 3600 * 1000;
const RETRY_MS: u32 = 60_000; // island busy with something else: show the pill a bit later

pub struct Update {
    pub tag: String,
    url: String,
    sha256: String, // lowercase hex
}

/// "v1", "1.2" or "v1.2.3" as [major, minor, patch]; missing parts are 0.
fn version(s: &str) -> [u32; 3] {
    let mut v = [0; 3];
    for (slot, part) in v.iter_mut().zip(s.trim().trim_start_matches(['v', 'V']).split('.')) {
        *slot = part.trim().parse().unwrap_or(0);
    }
    v
}

fn newer(tag: &str, current: &str) -> bool {
    version(tag) > version(current)
}

fn http() -> Result<HttpClient> {
    let http = HttpClient::new()?;
    http.DefaultRequestHeaders()?.UserAgent()?.TryParseAdd(&HSTRING::from(concat!("DynamicIsland/", env!("CARGO_PKG_VERSION"))))?; // GitHub rejects requests without one
    Ok(http)
}

/// The latest release, if it's newer than this build and ships an installer with a digest.
fn latest() -> Result<Option<Update>> {
    let body = http()?.GetStringAsync(&Uri::CreateUri(&HSTRING::from(API))?)?.get()?;
    let rel = JsonObject::Parse(&body)?;
    let tag = rel.GetNamedString(&HSTRING::from("tag_name"))?.to_string();
    if !newer(&tag, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let assets = rel.GetNamedArray(&HSTRING::from("assets"))?;
    for i in 0..assets.Size()? {
        let a = assets.GetObjectAt(i)?;
        if !a.GetNamedString(&HSTRING::from("name"))?.to_string().ends_with("Setup.exe") {
            continue;
        }
        // No digest, no update: the hash is what makes running a downloaded exe safe.
        let Ok(digest) = a.GetNamedString(&HSTRING::from("digest")) else { continue };
        let Some(sha) = digest.to_string().strip_prefix("sha256:").map(str::to_lowercase) else { continue };
        let url = a.GetNamedString(&HSTRING::from("browser_download_url"))?.to_string();
        return Ok(Some(Update { tag, url, sha256: sha }));
    }
    Ok(None)
}

fn check(hwnd: HWND) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        if let Ok(Some(u)) = latest() {
            let p = Box::into_raw(Box::new(u));
            if PostMessageW(HWND(hwnd as _), WM_UPDATE, WPARAM(UP_FOUND), LPARAM(p as isize)).is_err() {
                drop(Box::from_raw(p));
            }
        }
    });
}

/// Downloads the installer, verifies it, and starts it silently.
fn install(u: &Update) -> Result<()> {
    let buf = http()?.GetBufferAsync(&Uri::CreateUri(&HSTRING::from(u.url.as_str()))?)?.get()?;
    let hash = HashAlgorithmProvider::OpenAlgorithm(&HashAlgorithmNames::Sha256()?)?.HashData(&buf)?;
    if CryptographicBuffer::EncodeToHexString(&hash)?.to_string().to_lowercase() != u.sha256 {
        return Err(Error::new(E_FAIL, "installer hash mismatch"));
    }
    let mut bytes = windows::core::Array::<u8>::new();
    CryptographicBuffer::CopyToByteArray(&buf, &mut bytes)?;
    let path = std::env::temp_dir().join(format!("DynamicIsland-Setup-{}.exe", u.tag.replace(['/', '\\'], "")));
    std::fs::write(&path, &*bytes).map_err(|_| Error::from(E_FAIL))?;
    // Silent install: it stops this island and launches the new one when done.
    std::process::Command::new(&path)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|_| Error::from(E_FAIL))?;
    Ok(())
}

impl App {
    pub fn start_update_checks(&self) {
        unsafe { SetTimer(self.hwnd, T_UPDATE, FIRST_MS, None) };
    }

    /// T_UPDATE: show a found update again, or look for one.
    pub fn update_tick(&mut self) {
        if !self.cfg.pills[P_UPDATES] {
            self.update = None;
            unsafe {
                let _ = KillTimer(self.hwnd, T_UPDATE); // switching Updates back on restarts it
            }
        } else if self.update.is_some() {
            self.show_update();
        } else {
            check(self.hwnd);
            unsafe { SetTimer(self.hwnd, T_UPDATE, EVERY_MS, None) };
        }
    }

    pub fn on_update(&mut self, kind: usize, lp: isize) {
        match kind {
            UP_FOUND => {
                self.update = Some(*unsafe { Box::from_raw(lp as *mut Update) });
                self.show_update();
            }
            _ => {
                self.updating = false;
                self.note = Note::Update;
                self.clip_text = "Update failed, try again later".into();
                unsafe { SetTimer(self.hwnd, T_CLIP, CLIP_MS, None) };
            }
        }
    }

    /// The pill waits until the island isn't showing anything else.
    fn show_update(&mut self) {
        let Some(u) = &self.update else { return };
        if self.clip.to > 0.5 || self.vol.to > 0.5 || self.expand.to > 0.5 || self.updating {
            unsafe { SetTimer(self.hwnd, T_UPDATE, RETRY_MS, None) };
            return;
        }
        self.note = Note::Update;
        self.shot = None;
        self.swatch = None;
        self.link = None;
        self.clip_text = format!("{} · click to install", u.tag);
        self.clip.to = 1.0;
        unsafe {
            SetTimer(self.hwnd, T_CLIP, 8000, None);
            SetTimer(self.hwnd, T_UPDATE, EVERY_MS, None); // ignored: remind again later
        }
    }

    /// Update pill clicked: download, verify and install in the background.
    pub fn run_update(&mut self) {
        let Some(u) = self.update.take() else { return };
        self.updating = true;
        self.clip_text = format!("Downloading {}…", u.tag);
        unsafe {
            let _ = KillTimer(self.hwnd, T_CLIP); // stays up until the installer replaces us
        }
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || unsafe {
            if install(&u).is_err() {
                let _ = PostMessageW(HWND(hwnd as _), WM_UPDATE, WPARAM(UP_FAILED), LPARAM(0));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(version("v1"), [1, 0, 0]);
        assert_eq!(version("v1.2.3"), [1, 2, 3]);
        assert!(!newer("v1", "1.0.0"));
        assert!(newer("v1.0.1", "1.0.0"));
        assert!(newer("v2", "1.9.9"));
        assert!(!newer("v0.9", "1.0.0"));
        assert!(!newer("garbage", "1.0.0"));
    }

    #[test]
    #[ignore] // network
    fn latest_live() {
        println!("{:?}", latest().map(|u| u.map(|u| (u.tag, u.url, u.sha256))));
    }
}
