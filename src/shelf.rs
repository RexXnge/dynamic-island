// File shelf: drag files onto the island to park them, drag them back out later (or click to open).
// Parked files stay where they are; the shelf only holds their paths, in memory.
use super::*;
use std::cell::Cell;
use std::path::PathBuf;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::*;

pub const WM_SHELF_HOVER: u32 = 0x800B; // wparam = files are being dragged over the island
pub const WM_SHELF_ADD: u32 = 0x800C; // lparam = Box<Vec<PathBuf>>
const MAX: usize = 12;
const COLS: usize = 4;
const TOP_Y: f32 = 36.0; // first tile row, below the icon row
const TILE_H: f32 = 66.0;
const ICON: f32 = 34.0;

pub struct Item {
    pub path: PathBuf,
    icon: Option<ID2D1Bitmap1>,
}

#[implement(IDropTarget)]
struct Target {
    hwnd: isize,
    files: Cell<bool>,
}

unsafe fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: 15, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 } // CF_HDROP
}

impl Target_Impl {
    fn post(&self, msg: u32, wp: usize, lp: isize) -> bool {
        unsafe { PostMessageW(HWND(self.hwnd as _), msg, WPARAM(wp), LPARAM(lp)).is_ok() }
    }
}

// Calls arrive on the UI thread, sometimes while the app is busy; everything goes through messages.
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Option<&IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let files = data.is_some_and(|d| unsafe { d.QueryGetData(&hdrop_format()) } == S_OK);
        self.files.set(files);
        unsafe { *effect = if files { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        if files {
            self.post(WM_SHELF_HOVER, 1, 0);
        }
        Ok(())
    }
    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *effect = if self.files.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        self.post(WM_SHELF_HOVER, 0, 0);
        Ok(())
    }
    fn Drop(&self, data: Option<&IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.post(WM_SHELF_HOVER, 0, 0);
        let Some(data) = data else { return Ok(()) };
        unsafe {
            let mut med = data.GetData(&hdrop_format())?;
            let hdrop = HDROP(med.u.hGlobal.0);
            let n = DragQueryFileW(hdrop, u32::MAX, None);
            let paths: Vec<PathBuf> = (0..n)
                .filter_map(|i| {
                    let mut buf = vec![0u16; DragQueryFileW(hdrop, i, None) as usize + 1];
                    let len = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
                    (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
                })
                .collect();
            ReleaseStgMedium(&mut med);
            *effect = DROPEFFECT_COPY; // we only keep the path; the source leaves its file alone
            let msg = Box::into_raw(Box::new(paths));
            if !self.post(WM_SHELF_ADD, 0, msg as isize) {
                drop(Box::from_raw(msg));
            }
        }
        Ok(())
    }
}

/// Makes the island a drop target for files. Needs OLE initialised on this thread.
pub unsafe fn register(hwnd: HWND) -> Result<()> {
    let t: IDropTarget = Target { hwnd: hwnd.0 as isize, files: Cell::new(false) }.into();
    RegisterDragDrop(hwnd, &t) // keeps its own reference
}

/// The file's Explorer thumbnail (pictures, videos) or icon, as a D2D bitmap.
unsafe fn icon(dc: &ID2D1DeviceContext, path: &std::path::Path) -> Result<ID2D1Bitmap1> {
    let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)?;
    let px = 96; // enough for the 34-DIP tile at 250% scaling
    let hbmp = item.GetImage(SIZE { cx: px, cy: px }, SIIGBF_RESIZETOFIT)?;
    let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let bmp = wic.CreateBitmapFromHBITMAP(hbmp, HPALETTE::default(), WICBitmapUsePremultipliedAlpha);
    let _ = DeleteObject(hbmp);
    let conv = wic.CreateFormatConverter()?;
    conv.Initialize(&bmp?, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
    dc.CreateBitmapFromWicBitmap(&conv, None)
}

impl App {
    pub fn shelf_size(&self) -> (f32, f32, f32) {
        let rows = self.shelf.len().div_ceil(COLS).max(1) as f32;
        (EXPANDED.0, TOP_Y + rows * TILE_H + 10.0, 36.0)
    }

    fn tile(l: &Layout, i: usize) -> D2D_RECT_F {
        let w = (l.w - 24.0) / COLS as f32;
        rect(l.x + 12.0 + (i % COLS) as f32 * w, l.y + TOP_Y + (i / COLS) as f32 * TILE_H, w, TILE_H - 4.0)
    }

    fn tile_at(&self, l: &Layout, x: f32, y: f32) -> Option<usize> {
        (0..self.shelf.len()).find(|&i| {
            let r = Self::tile(l, i);
            x >= r.left && x < r.right && y >= r.top && y < r.bottom
        })
    }

    /// The remove button in a tile's corner.
    fn tile_x(r: &D2D_RECT_F) -> (f32, f32) {
        (r.right - 12.0, r.top + 10.0)
    }

    pub fn on_shelf_hover(&mut self, over: bool) {
        self.shelf_drop.to = over as u8 as f32;
        if over {
            if self.view != View::Shelf || self.expand.to < 0.5 {
                self.set_view(View::Shelf);
            }
            self.expand.to = 1.0;
            unsafe { let _ = KillTimer(self.hwnd, T_LEAVE); }
        } else if !self.hover {
            unsafe { SetTimer(self.hwnd, T_LEAVE, 400, None) };
        }
    }

    pub fn on_shelf_add(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            self.shelf.retain(|it| it.path != p);
            let icon = unsafe { icon(&self.g.dc, &p).ok() };
            self.shelf.push(Item { path: p, icon });
        }
        if self.shelf.len() > MAX {
            self.shelf.drain(..self.shelf.len() - MAX); // oldest go first
        }
        self.bump.v = 20.0;
        self.set_view(View::Shelf); // resize for the new row count
        self.expand.to = 1.0;
        unsafe { SetTimer(self.hwnd, T_LEAVE, 1500, None) }; // no pointer over us after a drop
    }

    /// Press on a tile: remove via its ×, else wait to see if it's a click (open) or a drag (out).
    pub fn shelf_click(&mut self, x: f32, y: f32) -> bool {
        if self.view != View::Shelf {
            return false;
        }
        let l = self.layout();
        if let Some(i) = self.tile_at(&l, x, y) {
            let (bx, by) = Self::tile_x(&Self::tile(&l, i));
            if (x - bx).hypot(y - by) < 9.0 {
                self.shelf.remove(i);
                self.set_view(View::Shelf);
            } else {
                self.shelf_press = Some((x, y, i));
                unsafe { SetCapture(self.hwnd) };
            }
        }
        true
    }

    /// Released without dragging: open the file.
    pub fn shelf_release(&mut self) -> bool {
        let Some((_, _, i)) = self.shelf_press.take() else { return false };
        if let Some(it) = self.shelf.get(i) {
            unsafe { downloads::open(&it.path) };
            self.expand.to = 0.0;
        }
        unsafe { let _ = ReleaseCapture(); }
        true
    }

    pub unsafe fn draw_shelf(&self, l: &Layout, a: f32) {
        let (dc, br, g) = (&self.g.dc, &self.g.brush, &self.g);
        let (opts, mm) = (D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        let dy = (1.0 - a) * 6.0;
        let d = clamp01(self.shelf_drop.x);
        let (ar, ag, ab) = accent();
        if self.shelf.is_empty() || d > 0.0 {
            // Drop zone outline, lit in the accent colour while files hover over the island.
            let z = rrect(l.x + 12.0, l.y + TOP_Y + dy, l.w - 24.0, l.h - TOP_Y - 10.0, 14.0);
            br.SetColor(&rgba(ar, ag, ab, a * 0.14 * d));
            dc.FillRoundedRectangle(&z, br);
            br.SetColor(&rgba(lerp(fg_rgb().0, ar, d), lerp(fg_rgb().1, ag, d), lerp(fg_rgb().2, ab, d), a * lerp(0.25, 0.9, d)));
            dc.DrawRoundedRectangle(&z, br, 1.5, None);
        }
        if self.shelf.is_empty() {
            br.SetColor(&fg(a * lerp(0.45, 0.9, d)));
            let msg = if d > 0.5 { "Drop to park here" } else { "Drag files here to park them" };
            dc.DrawText(&msg.encode_utf16().collect::<Vec<_>>(), &g.f_seg, &rect(l.x, l.y + TOP_Y + dy, l.w, l.h - TOP_Y - 10.0), br, opts, mm);
            return;
        }
        let hot = self.hover.then(|| self.tile_at(l, self.mouse.0, self.mouse.1)).flatten();
        for (i, it) in self.shelf.iter().enumerate() {
            let r = Self::tile(l, i);
            let r = rect(r.left, r.top + dy, r.right - r.left, r.bottom - r.top);
            let cx = (r.left + r.right) / 2.0;
            let pressed = self.shelf_press.is_some_and(|p| p.2 == i);
            if hot == Some(i) || pressed {
                br.SetColor(&fg(a * if pressed { 0.16 } else { 0.1 }));
                dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: 10.0, radiusY: 10.0 }, br);
            }
            match &it.icon {
                Some(b) => {
                    // Fit the thumbnail in the icon square, keeping its shape.
                    let s = b.GetSize();
                    let k = ICON / s.width.max(s.height).max(1.0);
                    let (w, h) = (s.width * k, s.height * k);
                    let (x, y) = (cx - w / 2.0, r.top + 6.0 + (ICON - h) / 2.0);
                    dc.DrawBitmap(b, Some(&rect(x, y, w, h)), a, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
                }
                None => {
                    br.SetColor(&surface(a));
                    dc.FillRoundedRectangle(&rrect(cx - 13.0, r.top + 6.0, 26.0, ICON, 5.0), br);
                }
            }
            let name = it.path.file_name().map_or_else(|| it.path.to_string_lossy(), |n| n.to_string_lossy());
            br.SetColor(&fg(a * 0.85));
            dc.DrawText(&name.encode_utf16().collect::<Vec<_>>(), &g.f_seg, &rect(r.left + 4.0, r.top + 8.0 + ICON, r.right - r.left - 8.0, 16.0), br, opts, mm);
            if hot == Some(i) {
                let (bx, by) = Self::tile_x(&r);
                br.SetColor(&surface(a));
                dc.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: bx, y: by }, radiusX: 8.0, radiusY: 8.0 }, br);
                br.SetColor(&fg(a * 0.8));
                let e = 3.0;
                dc.DrawLine(D2D_POINT_2F { x: bx - e, y: by - e }, D2D_POINT_2F { x: bx + e, y: by + e }, br, 1.5, &g.caps);
                dc.DrawLine(D2D_POINT_2F { x: bx + e, y: by - e }, D2D_POINT_2F { x: bx - e, y: by + e }, br, 1.5, &g.caps);
            }
        }
    }
}
