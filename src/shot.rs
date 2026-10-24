// Screenshot preview for the "Copied" notice: a thumbnail of a copied image, and opening it in
// the Snipping Tool to annotate.
use super::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::UI::Shell::*;

const THUMB_H: u32 = 96; // pixels; drawn ~30 DIPs tall, so sharp up to 300% scaling
// The handler's name is its exe path, e.g. ...\WindowsApps\Microsoft.ScreenSketch_11.x_x64__8wekyb3d8bbwe\SnippingTool\SnippingTool.exe
const SNIPPING_TOOL: &str = "\\Microsoft.ScreenSketch_";

/// The clipboard's image, copied out into a WIC bitmap (CF_BITMAP is synthesized from a DIB).
unsafe fn clipboard_image(hwnd: HWND, wic: &IWICImagingFactory) -> Result<IWICBitmap> {
    OpenClipboard(hwnd)?;
    let bmp = GetClipboardData(2).and_then(|h| wic.CreateBitmapFromHBITMAP(HBITMAP(h.0), HPALETTE::default(), WICBitmapIgnoreAlpha));
    let _ = CloseClipboard();
    bmp
}

fn wic() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

/// Small premultiplied copy of the clipboard image as a brush, plus its pixel size.
pub unsafe fn thumbnail(hwnd: HWND, dc: &ID2D1DeviceContext) -> Result<(ID2D1BitmapBrush1, f32, f32)> {
    let wic = wic()?;
    let src = clipboard_image(hwnd, &wic)?;
    let (mut w, mut h) = (0, 0);
    src.GetSize(&mut w, &mut h)?;
    let th = THUMB_H.min(h.max(1));
    let tw = ((w as u64 * th as u64 / h.max(1) as u64) as u32).clamp(1, th * 4);
    let scaler = wic.CreateBitmapScaler()?;
    scaler.Initialize(&src, tw, th, WICBitmapInterpolationModeFant)?;
    let conv = wic.CreateFormatConverter()?;
    conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
    let bmp = dc.CreateBitmapFromWicBitmap(&conv, None)?;
    Ok((dc.CreateBitmapBrush(&bmp, None, None)?, tw as f32, th as f32))
}

/// Saves the clipboard image as a PNG in %TEMP% and opens it in the Snipping Tool's editor.
/// Re-reads the clipboard here instead of keeping the full-size capture in memory.
pub unsafe fn edit_in_snipping_tool(hwnd: HWND) -> Result<()> {
    let wic = wic()?;
    let src = clipboard_image(hwnd, &wic)?;
    let dir = std::env::temp_dir().join("DynamicIsland");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("Screenshot {}.png", now_ft()));
    let wpath = HSTRING::from(path.as_os_str());

    let stream = wic.CreateStream()?;
    stream.InitializeFromFilename(&wpath, GENERIC_WRITE.0)?;
    let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
    enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
    let mut frame = None;
    enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
    let frame = frame.ok_or_else(|| Error::from(E_FAIL))?;
    frame.Initialize(None)?;
    let (mut w, mut h) = (0, 0);
    src.GetSize(&mut w, &mut h)?;
    frame.SetSize(w, h)?;
    let mut fmt = GUID_WICPixelFormat24bppBGR;
    frame.SetPixelFormat(&mut fmt)?; // the encoder may pick another format; convert to whatever it chose
    let conv = wic.CreateFormatConverter()?;
    conv.Initialize(&src, &fmt, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
    frame.WriteSource(&conv, std::ptr::null())?;
    frame.Commit()?;
    enc.Commit()?;
    drop(stream); // close the file before another app opens it

    let _ = AllowSetForegroundWindow(ASFW_ANY); // the click was ours; let the editor come to the front
    // Same path as Explorer's "Open with > Snipping Tool"; it's a full-trust app, so
    // ActivateForFile and its command-line alias both ignore the file.
    let opened = (|| -> Result<()> {
        let item: IShellItem = SHCreateItemFromParsingName(&wpath, None)?;
        let data: windows::Win32::System::Com::IDataObject = item.BindToHandler(None, &BHID_DataObject)?;
        let handlers = SHAssocEnumHandlers(w!(".png"), ASSOC_FILTER_NONE)?;
        loop {
            let mut h = [None];
            if handlers.Next(&mut h, None).is_err() {
                return Err(E_FAIL.into());
            }
            let Some(h) = h[0].take() else { return Err(E_FAIL.into()) };
            if h.GetName().is_ok_and(|n| n.to_string().is_ok_and(|n| n.contains(SNIPPING_TOOL))) {
                return h.Invoke(&data);
            }
        }
    })();
    if opened.is_err() {
        // No Snipping Tool (or it refused): fall back to the default image app.
        ShellExecuteW(None, w!("open"), &wpath, None, None, SW_SHOWNORMAL);
    }
    Ok(())
}
