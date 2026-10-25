// AI chat over Groq's free API. The key is pasted once and kept encrypted for this Windows user
// (DPAPI) in %APPDATA%\DynamicIsland\ai\key. Chats are JSON files in ai\chats on this PC only;
// the one thing that leaves the machine is the conversation sent to Groq to get each answer.
use super::*;
use std::path::{Path, PathBuf};
use windows::Data::Json::{JsonArray, JsonObject, JsonValue};
use windows::Foundation::Uri;
use windows::Storage::Streams::UnicodeEncoding;
use windows::Web::Http::{HttpClient, HttpMethod, HttpRequestMessage, HttpStringContent};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

pub const WM_AI: u32 = 0x8013; // lparam = Box<(PathBuf chat file, Result<String answer, String error>)>
pub const AI: (f32, f32, f32) = (WIDE, 290.0, 36.0);
const URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const MODELS_URL: &str = "https://api.groq.com/openai/v1/models";
/// Best first; the first one Groq offers this key is used. Groq retires models often, so anything
/// else that chats is the fallback.
const PREFERRED: [&str; 4] = ["openai/gpt-oss-120b", "llama-3.3-70b-versatile", "openai/gpt-oss-20b", "llama-3.1-8b-instant"];
/// The model picked from Groq's list, kept for the rest of the run.
static MODEL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
const SYSTEM: &str = "You are a helpful assistant inside a small Dynamic Island at the top of the user's Windows desktop. Answer briefly, in plain text without markdown.";
const KEYS_URL: &str = "https://console.groq.com/keys";
const MAX_INPUT: usize = 2000;
const HEAD_Y: f32 = 48.0; // header row centre: model name, New chat, Change key
const LIST_TOP: f32 = 62.0; // messages area, from the island top
const INPUT_H: f32 = 30.0;
const PAD: f32 = 18.0;

#[derive(Default)]
pub struct Ai {
    loaded: bool,
    key: Option<String>,
    chat: Vec<(bool, String)>, // (from the user, text)
    file: Option<PathBuf>,     // this chat's file, made with its first message
    input: String,
    waiting: bool,
    error: Option<String>,
    scroll: f32, // DIPs scrolled up from the newest message
}

fn dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("DynamicIsland").join("ai"))
}

/// DPAPI: encrypts (or decrypts) so only this Windows user on this PC can read it back.
unsafe fn dpapi(data: &[u8], decrypt: bool) -> Option<Vec<u8>> {
    let inp = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    let ok = if decrypt {
        CryptUnprotectData(&inp, None, None, None, None, 0, &mut out)
    } else {
        CryptProtectData(&inp, w!("Dynamic Island AI key"), None, None, None, 0, &mut out)
    };
    ok.ok()?;
    let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
    let _ = LocalFree(HLOCAL(out.pbData as _));
    Some(v)
}

fn load_key() -> Option<String> {
    let enc = std::fs::read(dir()?.join("key")).ok()?;
    String::from_utf8(unsafe { dpapi(&enc, true) }?).ok()
}

fn save_key(key: Option<&str>) -> std::io::Result<()> {
    let p = dir().ok_or(std::io::ErrorKind::NotFound)?.join("key");
    *MODEL.lock().unwrap() = None; // another key may be allowed other models
    match key {
        Some(k) => {
            let enc = unsafe { dpapi(k.as_bytes(), false) }.ok_or(std::io::ErrorKind::Other)?;
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(p, enc)
        }
        None => std::fs::remove_file(p).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }),
    }
}

fn to_json(chat: &[(bool, String)]) -> Result<JsonArray> {
    let arr = JsonArray::new()?;
    for (user, text) in chat {
        arr.Append(&message(if *user { "user" } else { "assistant" }, text)?)?;
    }
    Ok(arr)
}

fn message(role: &str, text: &str) -> Result<JsonObject> {
    let m = JsonObject::new()?;
    m.SetNamedValue(h!("role"), &JsonValue::CreateStringValue(&HSTRING::from(role))?)?;
    m.SetNamedValue(h!("content"), &JsonValue::CreateStringValue(&HSTRING::from(text))?)?;
    Ok(m)
}

fn save_chat(path: &Path, chat: &[(bool, String)]) {
    if let Ok(s) = to_json(chat).and_then(|a| a.Stringify()) {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, s.to_string());
    }
}

fn load_chat(path: &Path) -> Vec<(bool, String)> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(arr) = JsonArray::Parse(&HSTRING::from(text)) else { return Vec::new() };
    (0..arr.Size().unwrap_or(0))
        .filter_map(|i| {
            let m = arr.GetObjectAt(i).ok()?;
            let role = m.GetNamedString(h!("role")).ok()?.to_string();
            Some((role == "user", m.GetNamedString(h!("content")).ok()?.to_string()))
        })
        .collect()
}

/// Newest saved chat: files are named by creation time in ms, so the largest name wins.
fn latest_chat() -> Option<PathBuf> {
    std::fs::read_dir(dir()?.join("chats")).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).max()
}

fn new_chat_path() -> Option<PathBuf> {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    Some(dir()?.join("chats").join(format!("{ms:015}.json")))
}

/// Sends `req` with the key. Ok(Ok(json)) on success, Ok(Err(message)) when Groq says no.
fn call(key: &str, req: &HttpRequestMessage) -> Result<std::result::Result<JsonObject, String>> {
    req.Headers()?.TryAppendWithoutValidation(h!("Authorization"), &HSTRING::from(format!("Bearer {key}")))?;
    let resp = HttpClient::new()?.SendRequestAsync(req)?.get()?;
    let text = resp.Content()?.ReadAsStringAsync()?.get()?;
    let json = JsonObject::Parse(&text);
    if resp.IsSuccessStatusCode()? {
        return Ok(Ok(json?));
    }
    let detail = json.and_then(|j| j.GetNamedObject(h!("error"))?.GetNamedString(h!("message"))).map(|s| s.to_string());
    Ok(Err(detail.unwrap_or_else(|_| format!("Groq answered {}", resp.StatusCode().map_or(0, |c| c.0)))))
}

/// The id to chat with, from the models this key can use; never `skip`.
fn pick(ids: &[String], skip: Option<&str>) -> Option<String> {
    let ok = |id: &&String| Some(id.as_str()) != skip;
    let not_chat = ["whisper", "guard", "tts", "orpheus", "playai", "distil", "safeguard"];
    PREFERRED.iter().find_map(|p| ids.iter().filter(ok).find(|id| id.as_str() == *p)).or_else(|| ids.iter().filter(ok).find(|id| !not_chat.iter().any(|n| id.to_lowercase().contains(n)))).cloned()
}

fn list_models(key: &str) -> Result<std::result::Result<Vec<String>, String>> {
    let req = HttpRequestMessage::Create(&HttpMethod::Get()?, &Uri::CreateUri(&HSTRING::from(MODELS_URL))?)?;
    Ok(call(key, &req)?.map(|j| {
        let Ok(data) = j.GetNamedArray(h!("data")) else { return Vec::new() };
        (0..data.Size().unwrap_or(0)).filter_map(|i| data.GetObjectAt(i).ok()?.GetNamedString(h!("id")).ok().map(|s| s.to_string())).collect()
    }))
}

/// One chat completion. Err carries a message fit to show (Groq's own error text when it sends one).
fn complete(key: &str, chat: &[(bool, String)]) -> std::result::Result<String, String> {
    let ask = |model: &str| -> Result<std::result::Result<String, String>> {
        let msgs = to_json(chat)?;
        msgs.InsertAt(0, &message("system", SYSTEM)?)?;
        let body = JsonObject::new()?;
        body.SetNamedValue(h!("model"), &JsonValue::CreateStringValue(&HSTRING::from(model))?)?;
        body.SetNamedValue(h!("messages"), &msgs)?;
        let req = HttpRequestMessage::Create(&HttpMethod::Post()?, &Uri::CreateUri(&HSTRING::from(URL))?)?;
        req.SetContent(&HttpStringContent::CreateFromStringWithEncodingAndMediaType(&body.Stringify()?, UnicodeEncoding::Utf8, h!("application/json"))?)?;
        Ok(match call(key, &req)? {
            Ok(j) => Ok(j.GetNamedArray(h!("choices"))?.GetObjectAt(0)?.GetNamedObject(h!("message"))?.GetNamedString(h!("content"))?.to_string().trim().to_string()),
            Err(e) => Err(e),
        })
    };
    let run = || -> Result<std::result::Result<String, String>> {
        let mut skip = None;
        for _ in 0..2 {
            let cached = MODEL.lock().unwrap().clone();
            let model = match cached {
                Some(m) => m,
                None => match list_models(key)? {
                    Ok(ids) => match pick(&ids, skip.as_deref()) {
                        Some(m) => m,
                        None => return Ok(Err("Groq offers no chat model for this key".into())),
                    },
                    Err(e) => return Ok(Err(e)),
                },
            };
            *MODEL.lock().unwrap() = Some(model.clone());
            match ask(&model)? {
                // Retired or not allowed for this key: pick again from a fresh list, once.
                Err(e) if e.contains("does not exist") || e.contains("decommissioned") || e.contains("not have access") => {
                    *MODEL.lock().unwrap() = None;
                    skip = Some(model);
                }
                r => return Ok(r),
            }
        }
        Ok(Err("Groq rejected every model it offered; try again later".into()))
    };
    run().unwrap_or_else(|e| Err(format!("Couldn't reach Groq: {}", e.message())))
}

impl App {
    /// Opens the chat and takes keyboard focus like the calculator does.
    pub fn open_ai(&mut self) {
        if !self.ai.loaded {
            self.ai.loaded = true;
            self.ai.key = load_key();
            self.ai.file = latest_chat();
            self.ai.chat = self.ai.file.as_deref().map(load_chat).unwrap_or_default();
        }
        self.set_view(View::Ai);
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

    pub fn ai_busy(&self) -> bool {
        self.view == View::Ai && self.ai.waiting
    }

    fn header_buttons(l: &Layout) -> (D2D_RECT_F, D2D_RECT_F) {
        let cy = l.y + HEAD_Y;
        let r = l.x + l.w - PAD;
        (rect(r - 150.0, cy - 10.0, 70.0, 20.0), rect(r - 76.0, cy - 10.0, 76.0, 20.0)) // New chat, Change key
    }

    fn input_box(l: &Layout) -> D2D_RECT_F {
        rect(l.x + PAD, l.y + l.h - 12.0 - INPUT_H, l.w - 2.0 * PAD, INPUT_H)
    }

    fn ai_send(&mut self) {
        let text = self.ai.input.trim().to_string();
        if text.is_empty() || self.ai.waiting {
            return;
        }
        let Some(key) = self.ai.key.clone() else { return };
        let Some(path) = self.ai.file.clone().or_else(new_chat_path) else { return };
        self.ai.file = Some(path.clone());
        self.ai.chat.push((true, text));
        self.ai.input.clear();
        self.ai.error = None;
        self.ai.waiting = true;
        self.ai.scroll = 0.0;
        save_chat(&path, &self.ai.chat);
        let (hwnd, chat) = (self.hwnd.0 as isize, self.ai.chat.clone());
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let msg = Box::into_raw(Box::new((path, complete(&key, &chat))));
            if PostMessageW(HWND(hwnd as _), WM_AI, WPARAM(0), LPARAM(msg as isize)).is_err() {
                drop(Box::from_raw(msg));
            }
        });
    }

    pub fn on_ai(&mut self, path: PathBuf, answer: std::result::Result<String, String>) {
        let current = self.ai.file.as_ref() == Some(&path);
        if current {
            self.ai.waiting = false;
        }
        match answer {
            Ok(text) if current => {
                self.ai.chat.push((false, text));
                self.ai.scroll = 0.0;
                save_chat(&path, &self.ai.chat);
            }
            Ok(text) => {
                // Answer for a chat that's no longer open: it still belongs in that chat's file.
                let mut chat = load_chat(&path);
                chat.push((false, text));
                save_chat(&path, &chat);
            }
            Err(e) if current => self.ai.error = Some(e),
            Err(_) => {}
        }
    }

    pub fn ai_char(&mut self, c: u32) {
        if self.view != View::Ai || self.expand.to < 0.5 {
            return;
        }
        let a = &mut self.ai;
        match c {
            8 => {
                a.input.pop();
            }
            13 if a.key.is_none() => {
                let k = a.input.trim().to_string();
                if k.is_empty() {
                    return;
                }
                match save_key(Some(&k)) {
                    Ok(()) => {
                        a.key = Some(k);
                        a.error = None;
                    }
                    Err(e) => a.error = Some(format!("Couldn't save the key: {e}")),
                }
                a.input.clear();
            }
            13 => self.ai_send(),
            27 => self.close_calc(),
            22 => {
                // Ctrl+V; a pasted key loses any stray spaces or line breaks.
                if let Some(t) = unsafe { clips::clipboard_text(self.hwnd) } {
                    let key_mode = a.key.is_none();
                    let room = MAX_INPUT.saturating_sub(a.input.chars().count());
                    a.input.extend(t.chars().filter(|c| if key_mode { !c.is_whitespace() } else { !c.is_control() || *c == '\n' }).take(room));
                }
            }
            127 => a.input.clear(), // Ctrl+Backspace
            c => {
                if let Some(ch) = char::from_u32(c).filter(|ch| !ch.is_control()) {
                    if a.input.chars().count() < MAX_INPUT {
                        a.input.push(ch);
                    }
                }
            }
        }
    }

    pub fn ai_click(&mut self, x: f32, y: f32) -> bool {
        if self.view != View::Ai {
            return false;
        }
        let l = self.layout();
        let inside = |r: &D2D_RECT_F| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
        let (new, key) = Self::header_buttons(&l);
        if self.ai.key.is_some() && inside(&new) {
            if !self.ai.chat.is_empty() {
                self.ai = Ai { loaded: true, key: self.ai.key.take(), ..Default::default() };
            }
        } else if self.ai.key.is_some() && inside(&key) {
            let _ = save_key(None);
            self.ai.key = None;
            self.ai.input.clear();
            self.ai.error = None;
        } else if self.ai.key.is_none() && y > l.y + LIST_TOP && y < Self::input_box(&l).top - 4.0 {
            unsafe {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                ShellExecuteW(None, w!("open"), &HSTRING::from(KEYS_URL), None, None, SW_SHOWNORMAL);
            }
            return true;
        }
        unsafe {
            let _ = SetForegroundWindow(self.hwnd); // typing goes here again after clicking elsewhere
            let _ = SetFocus(self.hwnd);
        }
        true
    }

    /// Wrapped height of each shown message at the list width, oldest first.
    fn ai_rows(&self, l: &Layout) -> Vec<(bool, String, f32)> {
        let w = l.w - 2.0 * PAD;
        let mut rows: Vec<(bool, String)> = self.ai.chat.clone();
        if self.ai.waiting {
            rows.push((false, "…".into()));
        }
        if let Some(e) = &self.ai.error {
            rows.push((false, format!("⚠ {e}")));
        }
        rows.into_iter()
            .map(|(user, text)| {
                let tw = if user { w * 0.78 - 20.0 } else { w };
                let h = self.measure(&text, tw).1 + if user { 12.0 } else { 0.0 };
                (user, text, h)
            })
            .collect()
    }

    /// (width, height) of `text` wrapped at `w`.
    fn measure(&self, text: &str, w: f32) -> (f32, f32) {
        let t: Vec<u16> = text.encode_utf16().collect();
        let Ok(lay) = (unsafe { self.g.dw.CreateTextLayout(&t, &self.g.f_chat, w, 10_000.0) }) else { return (w, 16.0) };
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = unsafe { lay.GetMetrics(&mut m) };
        (m.widthIncludingTrailingWhitespace, m.height)
    }

    pub fn ai_scroll(&mut self, notches: f32) {
        let l = self.layout();
        let total: f32 = self.ai_rows(&l).iter().map(|r| r.2 + 10.0).sum();
        let room = Self::input_box(&l).top - 8.0 - (l.y + LIST_TOP);
        self.ai.scroll = (self.ai.scroll + notches * 40.0).clamp(0.0, (total - room).max(0.0));
    }

    pub unsafe fn draw_ai(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let text = |s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F| dc.DrawText(&s.encode_utf16().collect::<Vec<_>>(), f, &r, br, opts, mm);
        let (x0, w) = (l.x + PAD, l.w - 2.0 * PAD);
        let ib = Self::input_box(l);
        let focused = GetForegroundWindow() == self.hwnd;

        if self.ai.key.is_some() {
            br.SetColor(&fg(a * 0.4));
            let model = MODEL.lock().unwrap().as_deref().map(|m| format!(" · {}", m.rsplit('/').next().unwrap_or(m))).unwrap_or_default();
            text(&format!("Groq{model} · saved on this PC"), &g.f_artist_s, rect(x0, l.y + HEAD_Y - 8.0, w - 160.0, 16.0));
            let (new, key) = Self::header_buttons(l);
            for (r, s) in [(new, "New chat"), (key, "Change key")] {
                br.SetColor(&fg(a * 0.1));
                dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: 10.0, radiusY: 10.0 }, br);
                br.SetColor(&fg(a * 0.8));
                text(s, &g.f_seg, rect(r.left, r.top + 2.0, r.right - r.left, 16.0));
            }
            // Messages, newest at the bottom; the wheel scrolls back through older ones.
            let (top, bottom) = (l.y + LIST_TOP, ib.top - 8.0);
            dc.PushAxisAlignedClip(&rect(l.x, top, l.w, bottom - top), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let mut y = bottom + self.ai.scroll;
            for (user, s, h) in self.ai_rows(l).into_iter().rev() {
                let row_top = y - h;
                if row_top < bottom && y > top {
                    if user {
                        let tw = self.measure(&s, w * 0.78 - 20.0).0;
                        let bx = x0 + w - tw - 20.0;
                        br.SetColor(&fg(a * 0.14));
                        dc.FillRoundedRectangle(&rrect(bx, row_top, tw + 20.0, h, 12.0f32.min(h / 2.0)), br);
                        br.SetColor(&fg(a));
                        text(&s, &g.f_chat, rect(bx + 10.0, row_top + 6.0, tw + 1.0, h));
                    } else {
                        let warn = s.starts_with('⚠');
                        br.SetColor(&if warn { rgba(1.0, 0.45, 0.4, a) } else { fg(a * 0.9) });
                        text(&s, &g.f_chat, rect(x0, row_top, w, h));
                    }
                }
                y = row_top - 10.0;
                if y < top {
                    break;
                }
            }
            if self.ai.chat.is_empty() && !self.ai.waiting && self.ai.error.is_none() {
                br.SetColor(&fg(a * 0.4));
                text("Ask anything. Chats stay on this PC.", &g.f_wbody, rect(x0, top + 40.0, w, 40.0));
            }
            dc.PopAxisAlignedClip();
        } else {
            br.SetColor(&fg(a));
            text("Connect Groq", &g.f_wtitle, rect(x0, l.y + 64.0, w, 24.0));
            br.SetColor(&fg(a * 0.55));
            text("Paste a free API key from console.groq.com/keys (click here to open it), then press Enter. It's stored encrypted, only on this PC.", &g.f_wbody, rect(x0 + 10.0, l.y + 96.0, w - 20.0, 60.0));
            if let Some(e) = &self.ai.error {
                br.SetColor(&rgba(1.0, 0.45, 0.4, a));
                text(e, &g.f_artist_s, rect(x0, ib.top - 22.0, w, 16.0));
            }
        }

        // Input line, with a caret while the island has the keyboard.
        br.SetColor(&fg(a * 0.1));
        dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: ib, radiusX: INPUT_H / 2.0, radiusY: INPUT_H / 2.0 }, br);
        let inner = rect(ib.left + 14.0, ib.top + 6.0, ib.right - ib.left - 28.0, 18.0);
        let shown = if self.ai.key.is_none() && !self.ai.input.is_empty() {
            // Never draw the key itself: dots and its last four characters.
            let n = self.ai.input.chars().count();
            format!("{}{}", "•".repeat(n.saturating_sub(4).min(24)), self.ai.input.chars().skip(n.saturating_sub(4)).collect::<String>())
        } else {
            // One line: the tail of what's typed, newlines shown as spaces.
            let t: String = self.ai.input.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
            let n = t.chars().count();
            t.chars().skip(n.saturating_sub(60)).collect()
        };
        let (s, al) = match (shown.is_empty(), self.ai.key.is_some()) {
            (false, _) => (shown.as_str(), 1.0),
            (true, true) => (if self.ai.waiting { "Thinking…" } else { "Message" }, 0.35),
            (true, false) => ("Paste key with Ctrl+V", 0.35),
        };
        br.SetColor(&fg(a * al));
        text(s, &g.f_line, inner);
        if focused {
            let cx = if shown.is_empty() { inner.left } else { (inner.left + self.measure(&shown, 2000.0).0 + 1.0).min(inner.right) };
            let (r, gg, b) = accent();
            br.SetColor(&rgba(r, gg, b, a));
            dc.FillRoundedRectangle(&rrect(cx, ib.top + 7.0, 2.0, 16.0, 1.0), br);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_a_chat_model() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let list = ids(&["whisper-large-v3", "llama-3.1-8b-instant", "openai/gpt-oss-20b"]);
        assert_eq!(pick(&list, None).as_deref(), Some("openai/gpt-oss-20b"));
        assert_eq!(pick(&list, Some("openai/gpt-oss-20b")).as_deref(), Some("llama-3.1-8b-instant"));
        assert_eq!(pick(&ids(&["whisper-large-v3", "qwen/qwen3-32b"]), None).as_deref(), Some("qwen/qwen3-32b"));
        assert_eq!(pick(&ids(&["whisper-large-v3"]), None), None);
    }
}
