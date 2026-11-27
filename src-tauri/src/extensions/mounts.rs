use super::{
    firmware,
    storage::{directory, inventory, regular},
    DeviceRead, Mount, Result, Snapshot,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[cfg(target_os = "macos")]
pub(super) fn roots() -> Result<Vec<PathBuf>> {
    Ok(std::fs::read_dir("/Volumes")
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect())
}

#[cfg(target_os = "linux")]
pub(super) fn roots() -> Result<Vec<PathBuf>> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let mounts = std::fs::read("/proc/self/mountinfo").map_err(|e| e.to_string())?;
    Ok(mounts
        .split(|b| *b == b'\n')
        .filter_map(|line| {
            let field = line.split(|b| *b == b' ').nth(4)?;
            let mut path = Vec::new();
            let mut i = 0;
            while i < field.len() {
                if field[i] == b'\\' {
                    let digits = field.get(i + 1..i + 4)?;
                    if !digits.iter().all(|c| (b'0'..=b'7').contains(c)) {
                        return None;
                    }
                    path.push(
                        ((digits[0] - b'0') << 6) | ((digits[1] - b'0') << 3) | (digits[2] - b'0'),
                    );
                    i += 4;
                } else {
                    path.push(field[i]);
                    i += 1;
                }
            }
            Some(PathBuf::from(OsString::from_vec(path)))
        })
        .collect())
}

#[cfg(windows)]
pub(super) fn roots() -> Result<Vec<PathBuf>> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    let drives = unsafe { GetLogicalDrives() };
    if drives == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok((0..26)
        .filter(|i| drives & (1 << i) != 0)
        .filter_map(|i| {
            let path = format!("{}:\\", char::from(b'A' + i));
            let wide: Vec<_> = path.encode_utf16().chain([0]).collect();
            matches!(unsafe { GetDriveTypeW(wide.as_ptr()) }, 2 | 3).then(|| PathBuf::from(path))
        })
        .collect())
}

#[cfg(windows)]
pub(super) fn volume_name(root: &std::path::Path) -> Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumeNameForVolumeMountPointW(root: *const u16, name: *mut u16, size: u32) -> i32;
    }
    let drive = root.ancestors().last().ok_or("Missing drive root")?;
    let path: Vec<_> = drive.as_os_str().encode_wide().chain([0]).collect();
    let mut name = vec![0; 50];
    if unsafe {
        GetVolumeNameForVolumeMountPointW(path.as_ptr(), name.as_mut_ptr(), name.len() as u32)
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(name)
}

fn candidate(root: &Path) -> bool {
    directory(root)
        && directory(&root.join("iPod_Control"))
        && directory(&root.join("iPod_Control/Device"))
        && root.join("osos-cfw.bin").is_file()
        && root.join("cfw-loader.bin").is_file()
}

pub(super) fn inspect(root: &Path) -> Result<Option<Mount>> {
    if !candidate(root) {
        return Ok(None);
    }
    let mut image = regular(&root.join("osos-cfw.bin"))?;
    let len = image.metadata().map_err(|e| e.to_string())?.len();
    // The CFW resources and the relocatable RVX runtime follow the Apple image.
    let start = len.saturating_sub(2 * 1024 * 1024);
    image
        .seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut payload = Vec::new();
    image
        .take(2 * 1024 * 1024)
        .read_to_end(&mut payload)
        .map_err(|e| e.to_string())?;
    let mut loader = regular(&root.join("cfw-loader.bin"))?;
    let mut sysinfo = [0; 0x120];
    loader
        .seek(SeekFrom::Start(0x25100))
        .map_err(|e| e.to_string())?;
    loader.read_exact(&mut sysinfo).map_err(|e| e.to_string())?;
    let model = if sysinfo.starts_with(b"IsyS") {
        let raw = std::str::from_utf8(&sysinfo[0x98..0xa8])
            .unwrap_or("")
            .trim_end_matches('\0');
        match raw.get(1..) {
            Some("C293" | "C297") => "iPod classic 7G",
            Some("B562" | "B565") => "iPod classic 6.5G",
            Some("B029" | "B147" | "B145" | "B150") => "iPod classic 6G",
            _ => "iPod classic",
        }
    } else {
        "iPod classic"
    };
    let Some(device) = firmware::identify(&payload, model.into()) else {
        return Ok(None);
    };
    let mut hash = Sha256::new();
    hash.update(root.as_os_str().as_encoded_bytes());
    hash.update(sysinfo);
    hash.update(len.to_le_bytes());
    hash.update(device.repriseos.as_bytes());
    hash.update(device.revision.as_deref().unwrap_or("").as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(
            loader
                .metadata()
                .map_err(|e| e.to_string())?
                .dev()
                .to_le_bytes(),
        );
    }
    Ok(Some(Mount {
        root: root.into(),
        token: format!("{:x}", hash.finalize()),
        device,
    }))
}

pub(super) fn discover(roots: &[PathBuf]) -> Result<Option<Mount>> {
    let mut found = None;
    for path in roots {
        if let Some(mount) = inspect(path)? {
            if found.is_some() {
                return Err(
                    "More than one RepriseOS iPod is mounted. Connect one at a time.".into(),
                );
            }
            found = Some(mount);
        }
    }
    Ok(found)
}

pub(super) fn selected(token: &str) -> Result<Mount> {
    discover(&roots()?)?
        .filter(|m| m.token == token)
        .ok_or_else(|| "The iPod changed or was disconnected. Refresh the device.".into())
}

// Compare mount and file metadata before reading the firmware and RVX packages.
pub(super) fn read_mounts(
    mut roots: Vec<PathBuf>,
    known_mounts: Option<&str>,
) -> Result<Option<DeviceRead>> {
    roots.retain(|root| candidate(root));
    roots.sort_unstable();
    let mut key = DefaultHasher::new();
    for root in &roots {
        root.hash(&mut key);
        #[cfg(windows)]
        volume_name(root)?.hash(&mut key);
        for file in ["osos-cfw.bin", "cfw-loader.bin"] {
            let meta = fs::symlink_metadata(root.join(file)).map_err(|e| e.to_string())?;
            meta.len().hash(&mut key);
            meta.modified().ok().hash(&mut key);
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino()).hash(&mut key);
            }
        }
    }
    let mounts = format!("{:x}", key.finish());
    if known_mounts == Some(&mounts) {
        return Ok(None);
    }
    let snapshot = match discover(&roots)? {
        Some(m) => {
            super::storage::migrate_data(&m.root, &m.device);
            Some(Snapshot {
                installed: inventory(&m.root)?,
                token: m.token,
                device: m.device,
            })
        }
        None => None,
    };
    Ok(Some(DeviceRead { mounts, snapshot }))
}
