// AI chat over Groq's free API. The key is pasted once and kept encrypted for this Windows user
// (DPAPI) in %APPDATA%\DynamicIsland\ai\key. Chats are JSON files in ai\chats on this PC only;
// the one thing that leaves the machine is the conversation sent to Groq to get each answer.
// The AI can also start a timer, open an app or open a file; each one waits for a click on Allow,
// and app and file lookups happen on this PC (only the name the user asked for goes to Groq).
use super::*;
use std::path::{Path, PathBuf};
use windows::Data::Json::{JsonArray, JsonObject, JsonValue};
use windows::Foundation::Uri;
use windows::Storage::Streams::UnicodeEncoding;
use windows::Web::Http::{HttpClient, HttpMethod, HttpRequestMessage, HttpStringContent};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, SHGetKnownFolderItem, SHGetKnownFolderPath, FOLDERID_AppsFolder, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music,
    FOLDERID_Pictures, FOLDERID_Videos, KF_FLAG_DEFAULT, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
};

pub const WM_AI: u32 = 0x8013; // lparam = Box<(PathBuf chat file, Result<Reply, String error>)>
pub const AI: (f32, f32, f32) = (WIDE, 290.0, 36.0);
const URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const MODELS_URL: &str = "https://api.groq.com/openai/v1/models";
/// Best first; the first one Groq offers this key is used. Groq retires models often, so anything
/// else that chats is the fallback.
const PREFERRED: [&str; 4] = ["openai/gpt-oss-120b", "llama-3.3-70b-versatile", "openai/gpt-oss-20b", "llama-3.1-8b-instant"];
/// The model picked from Groq's list, kept for the rest of the run.
static MODEL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
const SYSTEM: &str = "You are a helpful assistant inside a small Dynamic Island at the top of the user's Windows desktop. Answer briefly, in plain text without markdown. \
You can start a timer, open an installed app, or find and open one of the user's files with the tools; the island asks the user before doing it, so just call the tool.";
const TOOLS: &str = r#"[
{"type":"function","function":{"name":"start_timer","description":"Start a countdown timer in the island.","parameters":{"type":"object","properties":{"minutes":{"type":"number","description":"Length in minutes, fractions allowed"}},"required":["minutes"]}}},
{"type":"function","function":{"name":"open_app","description":"Open an installed app, e.g. Spotify, Notepad or Settings.","parameters":{"type":"object","properties":{"name":{"type":"string","description":"The app's name"}},"required":["name"]}}},
{"type":"function","function":{"name":"open_file","description":"Find a file or folder in the user's Desktop, Documents, Downloads, Pictures, Music or Videos by part of its name, and open it.","parameters":{"type":"object","properties":{"name":{"type":"string","description":"Part of the file or folder name"}},"required":["name"]}}}
]"#;
const CARD_H: f32 = 46.0; // the Allow / Cancel card above the input line
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
    pending: Option<Action>, // waiting for Allow or Cancel
}

/// What came back for a message: an answer, or something to do once the user allows it.
pub enum Reply {
    Text(String),
    Ask(Action),
}

pub enum Action {
    Timer(u32), // seconds
    Open { label: String, detail: String, target: String }, // target goes to ShellExecute
}

impl Action {
    fn question(&self) -> String {
        match self {
            Action::Timer(s) => format!("Start a {} timer?", span(*s)),
            Action::Open { label, .. } => format!("Open {}?", short(label, 30)),
        }
    }
    fn detail(&self) -> String {
        match self {
            Action::Timer(_) => "Counts down in the island".into(),
            Action::Open { detail, .. } => short(detail, 44),
        }
    }
    fn verb(&self) -> &'static str {
        match self {
            Action::Timer(_) => "Start",
            Action::Open { .. } => "Open",
        }
    }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.into() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

/// "45 sec", "5 min", "1 h 30 min".
fn span(secs: u32) -> String {
    match (secs / 3600, secs / 60 % 60, secs % 60) {
        (0, 0, s) => format!("{s} sec"),
        (0, m, 0) => format!("{m} min"),
        (0, m, s) => format!("{m} min {s} sec"),
        (h, 0, _) => format!("{h} h"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

unsafe fn shell_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    let p = item.GetDisplayName(kind).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s
}

/// How well `name` answers `q` (both lowercase): exact, then prefix, then anywhere.
fn rank(name: &str, q: &str) -> Option<u8> {
    if name == q {
        Some(0)
    } else if name.starts_with(q) {
        Some(1)
    } else {
        name.contains(q).then_some(2)
    }
}

/// The Start menu's app (desktop or Store) that best matches `q`: (its name, what to open).
unsafe fn find_app(q: &str) -> Option<(String, String)> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, HANDLE::default()).ok()?;
    let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems).ok()?;
    let mut best: Option<((u8, usize), String, String)> = None;
    loop {
        let mut one = [None];
        let _ = items.Next(&mut one, None);
        let Some(item) = one[0].take() else { break };
        let (Some(name), Some(id)) = (shell_name(&item, SIGDN_NORMALDISPLAY), shell_name(&item, SIGDN_PARENTRELATIVEPARSING)) else { continue };
        let lower = name.to_lowercase();
        if lower.contains("uninstall") {
            continue;
        }
        // The id (e.g. Microsoft.WindowsNotepad_…!App) finds English names when Windows shows another language.
        let Some(r) = rank(&lower, &q).or_else(|| id.to_lowercase().contains(&q).then_some(3)) else { continue };
        if best.as_ref().map_or(true, |(k, ..)| (r, name.len()) < *k) {
            best = Some(((r, name.len()), name, id));
        }
    }
    best.map(|(_, name, id)| (name, format!("shell:AppsFolder\\{id}")))
}

unsafe fn known_folder(id: &GUID) -> Option<PathBuf> {
    let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, HANDLE::default()).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s.map(PathBuf::from)
}

/// The file or folder in the user's own folders that best matches `q`: the best name, then the newest.
fn find_file(q: &str) -> Option<PathBuf> {
    let q = q.trim().to_lowercase();
    if q.chars().count() < 2 {
        return None;
    }
    let roots = [FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures, FOLDERID_Music, FOLDERID_Videos];
    // Level by level, so files near the top of every folder are seen before deep trees use up the budget.
    let mut dirs: std::collections::VecDeque<(PathBuf, u32)> = roots.iter().filter_map(|id| unsafe { known_folder(id) }).map(|p| (p, 0)).collect();
    let mut best: Option<((std::cmp::Reverse<u8>, std::time::SystemTime), PathBuf)> = None;
    let mut seen = 0;
    // shortcut: stops after 50,000 entries and 6 levels deep; an index would be needed for more.
    while let Some((dir, depth)) = dirs.pop_front() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 50_000 {
                return best.map(|b| b.1);
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() && depth < 6 {
                dirs.push_back((e.path(), depth + 1)); // junctions report as symlinks, so they aren't followed
            }
            let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(r) = rank(&stem, &q).or_else(|| rank(&name, &q)) else { continue };
            let key = (std::cmp::Reverse(r), e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH));
            if best.as_ref().map_or(true, |(k, _)| key > *k) {
                best = Some((key, e.path()));
            }
        }
    }
    best.map(|b| b.1)
}

/// Turns a tool call into what to ask the user (lookups run here, on the worker thread).
fn act(name: &str, args: Option<JsonObject>) -> Reply {
    let arg = |k: &str| args.as_ref().and_then(|a| a.GetNamedString(&HSTRING::from(k)).ok()).map(|s| s.to_string().trim().to_string()).unwrap_or_default();
    match name {
        "start_timer" => {
            let min = args.as_ref().and_then(|a| a.GetNamedNumber(h!("minutes")).ok()).unwrap_or(0.0);
            let secs = (min * 60.0).round();
            if (1.0..=86_400.0).contains(&secs) {
                Reply::Ask(Action::Timer(secs as u32))
            } else {
                Reply::Text("Timers run from 1 second to 24 hours.".into())
            }
        }
        "open_app" => match unsafe { find_app(&arg("name")) } {
            Some((label, target)) => Reply::Ask(Action::Open { label, detail: "App".into(), target }),
            None => Reply::Text(format!("I couldn't find an app called \u{201c}{}\u{201d}.", arg("name"))),
        },
        "open_file" => match find_file(&arg("name")) {
            Some(p) => {
                let label = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let detail = p.parent().map(|d| format!("in {}", d.display())).unwrap_or_default();
                Reply::Ask(Action::Open { label, detail, target: p.display().to_string() })
            }
            None => Reply::Text(format!("I couldn't find a file called \u{201c}{}\u{201d} in your folders.", arg("name"))),
        },
        _ => Reply::Text("I can't do that from here.".into()),
    }
}

/// The system prompt with today's date and time, so questions about them need no tool.
fn system_prompt() -> String {
    let st = unsafe { GetLocalTime() };
    let date = calendar::date_text(st.wYear as i32, st.wMonth as u32, st.wDay as u32, "dddd d MMMM yyyy");
    format!("{SYSTEM} It is now {date}, {}.", clock().0)
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
fn complete(key: &str, chat: &[(bool, String)]) -> std::result::Result<Reply, String> {
    let ask = |model: &str| -> Result<std::result::Result<Reply, String>> {
        let msgs = to_json(chat)?;
        msgs.InsertAt(0, &message("system", &system_prompt())?)?;
        let body = JsonObject::new()?;
        body.SetNamedValue(h!("model"), &JsonValue::CreateStringValue(&HSTRING::from(model))?)?;
        body.SetNamedValue(h!("messages"), &msgs)?;
        body.SetNamedValue(h!("tools"), &JsonArray::Parse(&HSTRING::from(TOOLS))?)?;
        let req = HttpRequestMessage::Create(&HttpMethod::Post()?, &Uri::CreateUri(&HSTRING::from(URL))?)?;
        req.SetContent(&HttpStringContent::CreateFromStringWithEncodingAndMediaType(&body.Stringify()?, UnicodeEncoding::Utf8, h!("application/json"))?)?;
        Ok(match call(key, &req)? {
            Ok(j) => {
                let msg = j.GetNamedArray(h!("choices"))?.GetObjectAt(0)?.GetNamedObject(h!("message"))?;
                match msg.GetNamedArray(h!("tool_calls")).and_then(|c| c.GetObjectAt(0)) {
                    Ok(c) => {
                        let f = c.GetNamedObject(h!("function"))?;
                        Ok(act(&f.GetNamedString(h!("name"))?.to_string(), f.GetNamedString(h!("arguments")).and_then(|a| JsonObject::Parse(&a)).ok()))
                    }
                    Err(_) => Ok(Reply::Text(msg.GetNamedString(h!("content"))?.to_string().trim().to_string())),
                }
            }
            Err(e) => Err(e),
        })
    };
    let run = || -> Result<std::result::Result<Reply, String>> {
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
        self.ai.pending = None; // a new message instead of Allow means no
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

    pub fn on_ai(&mut self, path: PathBuf, answer: std::result::Result<Reply, String>) {
        let current = self.ai.file.as_ref() == Some(&path);
        if current {
            self.ai.waiting = false;
        }
        match answer {
            Ok(Reply::Ask(a)) if current => {
                self.ai.pending = Some(a);
                self.ai.scroll = 0.0;
            }
            Ok(Reply::Ask(_)) => {} // that chat was closed; nothing to allow any more
            Ok(Reply::Text(text)) if current => {
                self.ai.chat.push((false, text));
                self.ai.scroll = 0.0;
                save_chat(&path, &self.ai.chat);
            }
            Ok(Reply::Text(text)) => {
                // Answer for a chat that's no longer open: it still belongs in that chat's file.
                let mut chat = load_chat(&path);
                chat.push((false, text));
                save_chat(&path, &chat);
            }
            Err(e) if current => self.ai.error = Some(e),
            Err(_) => {}
        }
    }

    /// Runs (or drops) the action waiting in the card and says what happened in the chat.
    fn ai_decide(&mut self, allow: bool) {
        let Some(act) = self.ai.pending.take() else { return };
        let text = match act {
            _ if !allow => "Cancelled.".to_string(),
            Action::Timer(s) => {
                self.run_timer(s as f64);
                format!("Started a {} timer.", span(s))
            }
            Action::Open { label, target, .. } => {
                let ok = unsafe {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                    ShellExecuteW(None, w!("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL).0 as isize > 32
                };
                if ok { format!("Opened {label}.") } else { format!("Couldn't open {label}.") }
            }
        };
        self.ai.chat.push((false, text));
        self.ai.scroll = 0.0;
        if let Some(path) = &self.ai.file {
            save_chat(path, &self.ai.chat);
        }
    }

    /// The Allow / Cancel card: (card, allow button, cancel button).
    fn card(l: &Layout) -> (D2D_RECT_F, D2D_RECT_F, D2D_RECT_F) {
        let ib = Self::input_box(l);
        let card = rect(l.x + PAD, ib.top - 8.0 - CARD_H, l.w - 2.0 * PAD, CARD_H);
        let ok = rect(card.right - 10.0 - 60.0, card.top + 10.0, 60.0, 26.0);
        (card, ok, rect(ok.left - 6.0 - 64.0, ok.top, 64.0, 26.0))
    }

    /// Bottom of the messages, above the card while one waits.
    fn list_bottom(&self, l: &Layout) -> f32 {
        Self::input_box(l).top - 8.0 - if self.ai.pending.is_some() { CARD_H + 8.0 } else { 0.0 }
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
        let (_, ok, no) = Self::card(&l);
        if self.ai.pending.is_some() && (inside(&ok) || inside(&no)) {
            self.ai_decide(inside(&ok));
        } else if self.ai.key.is_some() && inside(&new) {
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
        let room = self.list_bottom(&l) - (l.y + LIST_TOP);
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
            let (top, bottom) = (l.y + LIST_TOP, self.list_bottom(l));
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
                text("Ask anything, or say \u{201c}open Spotify\u{201d}, \u{201c}timer 5 min\u{201d} or \u{201c}find my CV\u{201d}. Chats stay on this PC.", &g.f_wbody, rect(x0, top + 30.0, w, 50.0));
            }
            dc.PopAxisAlignedClip();
            if let Some(act) = &self.ai.pending {
                let (card, ok, no) = Self::card(l);
                br.SetColor(&fg(a * 0.08));
                dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: card, radiusX: 14.0, radiusY: 14.0 }, br);
                let tw = no.left - card.left - 20.0;
                br.SetColor(&fg(a));
                text(&act.question(), &g.f_title_s, rect(card.left + 12.0, card.top + 7.0, tw, 16.0));
                br.SetColor(&fg(a * 0.5));
                text(&act.detail(), &g.f_artist_s, rect(card.left + 12.0, card.top + 25.0, tw, 14.0));
                for (r, s, primary) in [(no, "Cancel", false), (ok, act.verb(), true)] {
                    br.SetColor(&fg(a * if primary { 1.0 } else { 0.12 }));
                    dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: 13.0, radiusY: 13.0 }, br);
                    br.SetColor(&if primary { bg(a) } else { fg(a * 0.85) });
                    text(s, &g.f_seg, rect(r.left, r.top + 5.0, r.right - r.left, 16.0));
                }
            }
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

    #[test]
    #[ignore] // reads this PC's Start menu and folders
    fn finds_apps_and_files_live() {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        for q in ["notepad", "settings", "spotify", "calc"] {
            println!("{q}: {:?}", unsafe { find_app(q) });
        }
        println!("cargo: {:?}", find_file("cargo"));
    }

    #[test]
    fn spans_and_ranks() {
        assert_eq!((span(45), span(300), span(330), span(3600), span(5400)), ("45 sec".into(), "5 min".into(), "5 min 30 sec".into(), "1 h".into(), "1 h 30 min".into()));
        assert_eq!((rank("spotify", "spotify"), rank("spotify music", "spotify"), rank("my spotify", "spotify"), rank("notepad", "spotify")), (Some(0), Some(1), Some(2), None));
    }
}
