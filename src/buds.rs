// Bluetooth audio: when headphones or earbuds connect, post their name and battery level (when the
// device reports one to Windows).
use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use windows::Devices::Enumeration::*;
use windows::Foundation::Collections::IIterable;
use windows::Foundation::{IPropertyValue, TypedEventHandler};

pub const WM_BUDS: u32 = 0x800A; // lparam = Box<(String name, Option<u8> battery)>
const CLASSIC_BT: &str = "System.Devices.Aep.ProtocolId:=\"{e0cbf06c-cd8b-4647-bb8a-263b43f0f974}\"";
const CONNECTED: &str = "System.Devices.Aep.IsConnected";
const CONTAINER: &str = "System.Devices.Aep.ContainerId";
const MAJOR: &str = "System.Devices.Aep.Bluetooth.Cod.Major";
const AUDIO_VIDEO: u8 = 4; // Bluetooth major device class for headsets, earbuds, speakers
const BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2"; // DEVPKEY_Bluetooth_Battery

struct Dev {
    name: String,
    connected: bool,
    audio: bool,
    container: Option<GUID>,
}

fn prop<T>(props: &windows::Foundation::Collections::IMapView<HSTRING, IInspectable>, key: &str, get: impl Fn(&IPropertyValue) -> Result<T>) -> Option<T> {
    let v = props.Lookup(&HSTRING::from(key)).ok()?;
    get(&v.cast::<IPropertyValue>().ok()?).ok()
}

fn props(keys: &[&str]) -> Result<IIterable<HSTRING>> {
    keys.iter().map(|k| HSTRING::from(*k)).collect::<Vec<_>>().try_into()
}

/// Battery % from the device's PnP nodes (Hands-Free devices report it); None if not reported.
fn battery(container: GUID) -> Option<u8> {
    let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{container:?}}}\""));
    let found = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&aqs, &props(&[BATTERY]).ok()?, DeviceInformationKind::Device).ok()?.get().ok()?;
    found.into_iter().find_map(|d| prop(&d.Properties().ok()?, BATTERY, |v| v.GetUInt8()))
}

pub fn watch(hwnd: HWND) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let devs: Arc<Mutex<HashMap<String, Dev>>> = Default::default();
        let Ok(w) = props(&[CONNECTED, CONTAINER, MAJOR]).and_then(|p| DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(&HSTRING::from(CLASSIC_BT), &p, DeviceInformationKind::AssociationEndpoint)) else {
            return;
        };
        let d = devs.clone();
        let _ = w.Added(&TypedEventHandler::new(move |_, info: &Option<DeviceInformation>| {
            if let Some(info) = info {
                let p = info.Properties()?;
                d.lock().unwrap().insert(
                    info.Id()?.to_string(),
                    Dev {
                        name: info.Name()?.to_string(),
                        connected: prop(&p, CONNECTED, |v| v.GetBoolean()).unwrap_or(false),
                        audio: prop(&p, MAJOR, |v| v.GetUInt8()).map_or(true, |m| m == AUDIO_VIDEO), // unknown class: allow
                        container: prop(&p, CONTAINER, |v| v.GetGuid()),
                    },
                );
            }
            Ok(())
        }));
        let d = devs.clone();
        let _ = w.Updated(&TypedEventHandler::new(move |_, up: &Option<DeviceInformationUpdate>| {
            let Some(up) = up else { return Ok(()) };
            let Some(now) = prop(&up.Properties()?, CONNECTED, |v| v.GetBoolean()) else { return Ok(()) };
            let hit = {
                let mut map = d.lock().unwrap();
                let Some(dev) = map.get_mut(&up.Id()?.to_string()) else { return Ok(()) };
                let rising = now && !dev.connected;
                dev.connected = now;
                (rising && dev.audio).then(|| (dev.name.clone(), dev.container))
            };
            if let Some((name, container)) = hit {
                // Windows learns the battery level a moment after the profiles connect.
                std::thread::sleep(std::time::Duration::from_millis(1500));
                let b = Box::into_raw(Box::new((name, container.and_then(battery))));
                if PostMessageW(HWND(hwnd as _), WM_BUDS, WPARAM(0), LPARAM(b as isize)).is_err() {
                    drop(Box::from_raw(b));
                }
            }
            Ok(())
        }));
        if w.Start().is_ok() {
            loop {
                std::thread::park(); // keeps the watcher (and its handlers) alive
            }
        }
    });
}
