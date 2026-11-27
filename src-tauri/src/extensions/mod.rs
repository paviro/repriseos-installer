pub(crate) mod eject;
mod firmware;
mod mounts;
mod package;
mod storage;

use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Mutex};

type Result<T> = std::result::Result<T, String>;
const MAX_PACKAGE: usize = 16 * 1024 * 1024;
static EDIT: Mutex<()> = Mutex::new(());

fn word(data: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(i..i + 4)?.try_into().ok()?))
}

/// The catalog's "major.minor" form of an RVX API word.
fn api_text(api: u32) -> String {
    format!("{}.{}", api >> 16, api & 0xffff)
}

/// rv_row and rv_menu_entry grew in API 1.86 and rv_pane in 1.89, so a core at
/// 1.89 or later only loads packages an SDK at 1.89 or later packed.
const LAYOUT_API: u32 = 0x10059;

/// The lowest header SDK a core at `api` loads, when it refuses older ones.
fn sdk_floor(api: u32) -> Option<u32> {
    (api >= LAYOUT_API && api >> 16 == LAYOUT_API >> 16).then_some(LAYOUT_API)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    model: String,
    eject_reboot: bool,
    repriseos: String,
    revision: Option<String>,
    api: Option<String>,
    firmware_id: Option<String>,
    /// The lowest SDK the firmware loads packages from, when it refuses older ones.
    sdk_floor: Option<String>,
}

impl DeviceInfo {
    /// Whether the firmware keeps extension data under /Reprise/Data. Releases
    /// through 0.4.0 read /Revola/Data.
    fn reprise_data(&self) -> bool {
        self.repriseos == "Development"
            || semver::Version::parse(&self.repriseos)
                .is_ok_and(|v| v.cmp_precedence(&semver::Version::new(0, 4, 0)).is_gt())
    }

    fn allows(&self, pkg: &package::Package) -> bool {
        let dev = self.repriseos == "Development";
        if !dev
            && !semver::Version::parse(&self.repriseos)
                .is_ok_and(|v| v.cmp_precedence(&semver::Version::new(0, 3, 2)).is_gt())
        {
            return false;
        }
        match (&self.api, &self.firmware_id) {
            (Some(api), Some(target)) => {
                let Some((major, minor)) = api.split_once('.') else {
                    return false;
                };
                let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>()) else {
                    return false;
                };
                major == pkg.minimum >> 16
                    && minor >= (pkg.minimum & 0xffff)
                    && sdk_floor(major << 16 | minor).is_none_or(|floor| pkg.sdk >= floor)
                    && u32::from_str_radix(target, 16) == Ok(pkg.target)
            }
            _ => dev,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct Installed {
    id: String,
    name: String,
    version: String,
    folder: String,
    requires: Vec<String>,
    #[serde(skip)]
    path: PathBuf,
    library: bool,
    /// The package's minimum API and target firmware, in catalog form.
    api: String,
    firmware_id: String,
    /// The SDK that packed it, in catalog form.
    sdk: String,
}

#[derive(Serialize)]
pub struct Snapshot {
    token: String,
    device: DeviceInfo,
    installed: Vec<Installed>,
}

struct Mount {
    root: PathBuf,
    token: String,
    device: DeviceInfo,
}

#[derive(Deserialize)]
pub struct WritePackage {
    id: String,
    version: String,
    sha256: String,
    data: Vec<u8>,
}

#[derive(Serialize)]
pub struct DeviceRead {
    mounts: String,
    snapshot: Option<Snapshot>,
}

#[tauri::command]
pub async fn read_extension_device(known_mounts: Option<String>) -> Result<Option<DeviceRead>> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = EDIT.lock().map_err(|e| e.to_string())?;
        mounts::read_mounts(mounts::roots()?, known_mounts.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn write_extension(token: String, package: WritePackage) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = EDIT.lock().map_err(|e| e.to_string())?;
        storage::write(&mounts::selected(&token)?, package)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn remove_extension(token: String, id: String, folder: String) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = EDIT.lock().map_err(|e| e.to_string())?;
        storage::remove(&mounts::selected(&token)?, &id, &folder)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests;
