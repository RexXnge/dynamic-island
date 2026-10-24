// Downloads: watches the Downloads folder for browsers' in-progress files (.crdownload, .part, ...)
// and their rename to the final name. Browsers don't say the total size, so progress is the
// size so far with a spinning ring, not a fraction.
use super::*;
use std::path::{Path, PathBuf};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::*;

pub const WM_DL: u32 = 0x8008; // lparam = Box<DlEvent>
pub const WM_DRAG: u32 = 0x8009; // start dragging the finished download out (outside the APP borrow)
pub const T_DL: usize = 7;
const TEMP_EXT: [&str; 5] = ["crdownload", "part", "partial", "download", "opdownload"];

pub enum DlEvent {
    Start(PathBuf),
    Done(PathBuf),
    Cancel(PathBuf),
}

pub struct Download {
    pub path: PathBuf,
    pub done: bool,
}

fn is_temp(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| TEMP_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

/// What to call the file: its final name, without the browser's temp extension.
pub fn display_name(p: &Path) -> String {
    let name = if is_temp(p) { p.file_stem() } else { p.file_name() };
    let name = name.map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.starts_with("Unconfirmed ") { "File".into() } else { name }
}

pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    match bytes {
        0..=999_999 => format!("{:.0} KB", b / 1e3),
        1_000_000..=999_999_999 => format!("{:.1} MB", b / 1e6),
        _ => format!("{:.2} GB", b / 1e9),
    }
}

/// Watches the user's Downloads folder on its own thread; sleeps in ReadDirectoryChangesW.
pub fn watch(hwnd: HWND) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Downloads, KNOWN_FOLDER_FLAG(0), None) else { return };
        let dir = PathBuf::from(p.to_string().unwrap_or_default());
        CoTaskMemFree(Some(p.0 as _));
        let Ok(h) = CreateFileW(
            &HSTRING::from(dir.as_os_str()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        ) else {
            return;
        };
        let post = |ev: DlEvent| {
            let b = Box::into_raw(Box::new(ev));
            if PostMessageW(HWND(hwnd as _), WM_DL, WPARAM(0), LPARAM(b as isize)).is_err() {
                drop(Box::from_raw(b));
            }
        };
        let mut buf = vec![0u64; 8192]; // 64 KB, u64 for the DWORD alignment the API needs
        let mut renamed_from: Option<PathBuf> = None;
        loop {
            let mut n = 0;
            if ReadDirectoryChangesW(h, buf.as_mut_ptr() as _, (buf.len() * 8) as u32, false, FILE_NOTIFY_CHANGE_FILE_NAME, Some(&mut n), None, None).is_err() {
                break;
            }
            let mut off = 0usize;
            while n > 0 {
                let info = &*((buf.as_ptr() as *const u8).add(off) as *const FILE_NOTIFY_INFORMATION);
                let name = std::slice::from_raw_parts(info.FileName.as_ptr(), info.FileNameLength as usize / 2);
                let path = dir.join(String::from_utf16_lossy(name));
                match info.Action {
                    FILE_ACTION_ADDED if is_temp(&path) => post(DlEvent::Start(path)),
                    FILE_ACTION_REMOVED if is_temp(&path) => post(DlEvent::Cancel(path)),
                    FILE_ACTION_RENAMED_OLD_NAME => renamed_from = Some(path),
                    FILE_ACTION_RENAMED_NEW_NAME => {
                        if renamed_from.take().is_some_and(|old| is_temp(&old)) {
                            // temp -> temp (Chrome naming its "Unconfirmed" file) or temp -> final
                            post(if is_temp(&path) { DlEvent::Start(path) } else { DlEvent::Done(path) });
                        }
                    }
                    _ => {}
                }
                if info.NextEntryOffset == 0 {
                    break;
                }
                off += info.NextEntryOffset as usize;
            }
        }
        let _ = CloseHandle(h);
    });
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }
    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drags the file out like Explorer would (drop it on a folder, a chat, an email). Modal until drop;
/// true when it was dropped somewhere.
pub unsafe fn drag_out(path: &Path) -> Result<bool> {
    let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)?;
    let data: IDataObject = item.BindToHandler(None, &BHID_DataObject)?;
    let src: IDropSource = DropSource.into();
    let mut effect = DROPEFFECT(0);
    let hr = DoDragDrop(&data, &src, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut effect);
    hr.ok()?;
    Ok(hr == DRAGDROP_S_DROP)
}

pub unsafe fn open(path: &Path) {
    let _ = AllowSetForegroundWindow(ASFW_ANY);
    ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL);
}
