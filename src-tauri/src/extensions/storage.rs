use super::{
    package::{parse, valid_id},
    DeviceInfo, Installed, Mount, Result, WritePackage, MAX_PACKAGE,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub(super) const PRIMARY: &str = "/Reprise/Extensions/";
const LEGACY: &str = "/Revola/Extensions/";
/// The firmware image and the companion loader after 0.4.0, under this folder.
pub(super) const FIRMWARE_FOLDER: &str = "Reprise/Firmware";
pub(super) const FIRMWARE_FILES: [&str; 2] = ["reprise.bin", "loader.bin"];
/// The same two files of releases through 0.4.0, at the disk root.
pub(super) const ROOT_FIRMWARE_FILES: [&str; 2] = ["osos-cfw.bin", "cfw-loader.bin"];

fn link(m: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    m.file_type().is_symlink()
}

pub(super) fn regular(path: &Path) -> Result<File> {
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_file() || link(&m) {
        return Err(format!("Expected an ordinary file: {}", path.display()));
    }
    File::open(path).map_err(|e| e.to_string())
}

pub(super) fn directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !link(&m))
}

pub(super) fn file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !link(&m))
}

/// Moves the extension data folders a RepriseOS release through 0.4.0 kept
/// under /Revola/Data/Extensions to /Reprise/Data/Extensions, where later
/// firmware reads them. Only runs for such firmware; an older one would lose
/// its settings. A folder already present on the Reprise side is kept and its
/// Revola copy left behind. Failures only print; the device still lists.
pub(super) fn migrate_data(root: &Path, device: &DeviceInfo) {
    let old = root.join("Revola").join("Data");
    let old_extensions = old.join("Extensions");
    if !device.reprise_data()
        || !directory(&root.join("Revola"))
        || !directory(&old)
        || !directory(&old_extensions)
    {
        return;
    }
    let new = root.join("Reprise").join("Data");
    let new_extensions = new.join("Extensions");
    let moved = (|| -> io::Result<()> {
        for dir in [root.join("Reprise"), new.clone()] {
            match fs::symlink_metadata(&dir) {
                Ok(m) if m.is_dir() && !link(&m) => (),
                Ok(_) => return Err(io::ErrorKind::AlreadyExists.into()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&dir)?,
                Err(e) => return Err(e),
            }
        }
        match fs::symlink_metadata(&new_extensions) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::rename(&old_extensions, &new_extensions)?;
            }
            Ok(m) if m.is_dir() && !link(&m) => {
                for entry in fs::read_dir(&old_extensions)? {
                    let from = entry?.path();
                    let Some(name) = from.file_name() else {
                        continue;
                    };
                    let to = new_extensions.join(name);
                    if directory(&from) && fs::symlink_metadata(&to).is_err() {
                        fs::rename(&from, &to)?;
                    }
                }
                // Gone once every folder moved; otherwise the rest stays.
                let _ = fs::remove_dir(&old_extensions);
            }
            Ok(_) => return Err(io::ErrorKind::AlreadyExists.into()),
            Err(e) => return Err(e),
        }
        Ok(())
    })();
    if let Err(e) = moved {
        eprintln!(
            "Extension data: could not move {} to {}: {e}",
            old_extensions.display(),
            new_extensions.display()
        );
    }
    let _ = fs::remove_dir(&old);
}

/// Removes the theme packs a RepriseOS release through 0.4.0 read from
/// /themes/default/theme.bin and /themes/dark/theme.bin, and the /themes
/// folder once it is empty. Later firmware does not read them; only runs for
/// such firmware. Other content under /themes stays, links are not followed
/// and a failure only prints; the device still lists.
pub(super) fn remove_themes(root: &Path, device: &DeviceInfo) {
    let themes = root.join("themes");
    if !device.reprise_data() || !directory(&themes) {
        return;
    }
    let removed = (|| -> io::Result<()> {
        for name in ["default", "dark"] {
            let pack = themes.join(name);
            if !directory(&pack) {
                continue;
            }
            let file = pack.join("theme.bin");
            match fs::symlink_metadata(&file) {
                Ok(m) if m.is_file() && !link(&m) => fs::remove_file(&file)?,
                Ok(_) => continue,
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
            // Stays if it holds anything else.
            let _ = fs::remove_dir(&pack);
        }
        Ok(())
    })();
    if let Err(e) = removed {
        eprintln!("Theme folder: could not remove {}: {e}", themes.display());
    }
    let _ = fs::remove_dir(&themes);
}

/// Removes the firmware files a RepriseOS release through 0.4.0 booted from
/// the disk root, /osos-cfw.bin and /cfw-loader.bin, once /Reprise/Firmware
/// holds both files of a later firmware; the bootloader of that firmware
/// reads only the folder. Links are not followed and a failure only prints;
/// the device still lists.
pub(super) fn remove_root_firmware(root: &Path, device: &DeviceInfo) {
    let folder = root.join(FIRMWARE_FOLDER);
    if !device.reprise_firmware() || !FIRMWARE_FILES.iter().all(|name| file(&folder.join(name))) {
        return;
    }
    for name in ROOT_FIRMWARE_FILES {
        let path = root.join(name);
        if file(&path) {
            if let Err(e) = fs::remove_file(&path) {
                eprintln!("Root firmware: could not remove {}: {e}", path.display());
            }
        }
    }
}

pub(super) fn write_file(path: &Path, bytes: &[u8], replace: bool) -> io::Result<()> {
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    if replace {
        return tmp.persist(path).map(drop).map_err(|e| e.error);
    }
    match tmp.persist_noclobber(path) {
        Ok(_) => Ok(()),
        // Darwin's FAT driver returns ENOTSUP for an exclusive rename.
        // Reserve the name exclusively, then rename the complete file over it.
        #[cfg(target_os = "macos")]
        Err(e)
            if e.error.raw_os_error() == Some(45)
                || e.error.kind() == io::ErrorKind::Unsupported =>
        {
            let reservation = File::create_new(path)?;
            let result = e.file.persist(path);
            drop(reservation);
            if let Err(e) = result {
                fs::remove_file(path)?;
                return Err(e.error);
            }
            Ok(())
        }
        Err(e) => Err(e.error),
    }
}

pub(super) fn folder(root: &Path, which: &str, create: bool) -> Result<Option<PathBuf>> {
    if ![PRIMARY, LEGACY].contains(&which) {
        return Err("Unknown extension folder".into());
    }
    let mut dir = root.to_path_buf();
    for part in which.trim_matches('/').split('/') {
        dir.push(part);
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.is_dir() && !link(&m) => (),
            Ok(_) => return Err("Unsafe extension folder".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                fs::create_dir(&dir).map_err(|e| e.to_string())?;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(Some(dir))
}

pub(super) fn inventory(root: &Path) -> Result<Vec<Installed>> {
    let mut files = Vec::new();
    for which in [PRIMARY, LEGACY] {
        let Some(dir) = folder(root, which, false)? else {
            continue;
        };
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if !path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("rvx"))
                || path
                    .file_name()
                    .is_some_and(|s| s.to_string_lossy().starts_with("._"))
            {
                continue;
            }
            let m = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if !m.is_file() || link(&m) || m.len() > MAX_PACKAGE as u64 {
                continue;
            }
            let mut bytes = Vec::new();
            regular(&path)?
                .take(MAX_PACKAGE as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if let Ok(pkg) = parse(&bytes) {
                files.push(Installed {
                    id: pkg.id,
                    name: pkg.name,
                    version: pkg.version,
                    requires: pkg.requires,
                    library: pkg.library,
                    api: super::api_text(pkg.minimum),
                    firmware_id: format!("{:08x}", pkg.target),
                    sdk: super::api_text(pkg.sdk),
                    folder: which.into(),
                    path,
                });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub(super) fn write(mount: &Mount, request: WritePackage) -> Result<()> {
    if request.data.len() > MAX_PACKAGE
        || !valid_id(&request.id)
        || format!("{:x}", Sha256::digest(&request.data)) != request.sha256
    {
        return Err("Invalid extension download".into());
    }
    let pkg = parse(&request.data)?;
    if pkg.id != request.id || pkg.version != request.version || !mount.device.allows(&pkg) {
        return Err("Package identity or RVX compatibility does not match the device".into());
    }
    let installed = inventory(&mount.root)?;
    for dep in &pkg.requires {
        if !installed.iter().any(|e| e.id == *dep && e.library) {
            return Err(format!("Missing required library {dep}"));
        }
    }
    let mut copies = installed
        .iter()
        .filter(|e| e.folder == PRIMARY && e.id == pkg.id);
    let dir = folder(&mount.root, PRIMARY, true)?.unwrap();
    let existing = copies.next();
    if copies.next().is_some() {
        return Err(format!("Multiple copies of {} in {PRIMARY}", pkg.id));
    }
    let path = existing
        .map(|e| e.path.clone())
        .unwrap_or_else(|| dir.join(format!("{}.rvx", pkg.id)));
    write_file(&path, &request.data, existing.is_some()).map_err(|e| e.to_string())
}

pub(super) fn remove(mount: &Mount, id: &str, which: &str) -> Result<()> {
    if !valid_id(id) || ![PRIMARY, LEGACY].contains(&which) {
        return Err("Invalid extension".into());
    }
    let installed = inventory(&mount.root)?;
    if !installed.iter().any(|e| e.id == id && e.folder != which) {
        if let Some(user) = installed
            .iter()
            .find(|e| e.requires.iter().any(|dep| dep == id))
        {
            return Err(format!("Required by {}", user.name));
        }
    }
    for entry in installed.iter().filter(|e| e.folder == which && e.id == id) {
        fs::remove_file(&entry.path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
