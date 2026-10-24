// USB: a pill when a USB device is plugged in or pulled out, named the way Windows' "Bluetooth &
// devices" page names it (the device container), so you can tell which one just dropped.
use super::*;
use crate::buds::{prop, props};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use windows::Devices::Enumeration::*;
use windows::Foundation::TypedEventHandler;

pub const WM_USB: u32 = 0x8011; // wparam 1 plugged in / 0 pulled out, lparam = Box<String> name
const USB_DEVICE: &str = "System.Devices.InterfaceClassGuid:=\"{A5DCBF10-6530-11D2-901F-00C04FB951ED}\" AND System.Devices.InterfaceEnabled:=System.StructuredQueryType.Boolean#True";
const CONTAINER: &str = "System.Devices.ContainerId";
// Devices built into the PC share this container; they don't get plugged in or out.
const THIS_PC: GUID = GUID::from_u128(0x00000000_0000_0000_ffff_ffffffffffff);

/// The product name Windows shows for the device, or None for the PC's own internals.
fn name(info: &DeviceInformation) -> Option<String> {
    let c = prop(&info.Properties().ok()?, CONTAINER, |v| v.GetGuid());
    if c == Some(THIS_PC) {
        return None;
    }
    let container = c.and_then(|c| {
        let id = HSTRING::from(format!("{{{c:?}}}"));
        DeviceInformation::CreateFromIdAsyncWithKindAndAdditionalProperties(&id, &props(&[]).ok()?, DeviceInformationKind::DeviceContainer).ok()?.get().ok()?.Name().ok()
    });
    let n = container.or_else(|| info.Name().ok())?.to_string();
    (!n.trim().is_empty()).then_some(n)
}

fn post(hwnd: isize, plugged: bool, name: String) {
    let b = Box::into_raw(Box::new(name));
    unsafe {
        if PostMessageW(HWND(hwnd as _), WM_USB, WPARAM(plugged as usize), LPARAM(b as isize)).is_err() {
            drop(Box::from_raw(b));
        }
    }
}

pub fn watch(hwnd: HWND) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        // Names by interface id, so a pulled-out device (already gone from Windows) still has one.
        let names: Arc<Mutex<HashMap<String, Option<String>>>> = Default::default();
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false)); // initial listing done
        let Ok(w) = props(&[CONTAINER]).and_then(|p| DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(&HSTRING::from(USB_DEVICE), &p, DeviceInformationKind::DeviceInterface)) else {
            return;
        };
        let (n, r) = (names.clone(), ready.clone());
        let _ = w.Added(&TypedEventHandler::new(move |_, info: &Option<DeviceInformation>| {
            if let Some(info) = info {
                let name = name(info);
                n.lock().unwrap().insert(info.Id()?.to_string(), name.clone());
                if let Some(name) = name.filter(|_| r.load(std::sync::atomic::Ordering::Relaxed)) {
                    post(hwnd, true, name);
                }
            }
            Ok(())
        }));
        let n = names.clone();
        let _ = w.Removed(&TypedEventHandler::new(move |_, up: &Option<DeviceInformationUpdate>| {
            if let Some(up) = up {
                if let Some(Some(name)) = n.lock().unwrap().remove(&up.Id()?.to_string()) {
                    post(hwnd, false, name);
                }
            }
            Ok(())
        }));
        let r = ready.clone();
        let _ = w.EnumerationCompleted(&TypedEventHandler::new(move |_, _| {
            r.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }));
        if w.Start().is_ok() {
            loop {
                std::thread::park(); // keeps the watcher (and its handlers) alive
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // lists this PC's devices
    fn names_live() {
        let all = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&HSTRING::from(USB_DEVICE), &props(&[CONTAINER]).unwrap(), DeviceInformationKind::DeviceInterface).unwrap().get().unwrap();
        for d in all {
            println!("{:?}  ({})", name(&d), d.Name().unwrap());
        }
    }
}
