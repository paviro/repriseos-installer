use super::{
    mounts::{discover, inspect, read_mounts},
    package::parse,
    storage::{folder, inventory, remove, write, PRIMARY},
    *,
};
use sha2::{Digest, Sha256};
use std::fs;

fn image(version: &str) -> Vec<u8> {
    core(version, 0x10051)
}

fn core(version: &str, api: u32) -> Vec<u8> {
    let mut bytes =
        format!("\0Build: abc12345-dirty\0RepriseOS {version}\0RepriseRVX1\0").into_bytes();
    bytes.extend(api.to_le_bytes());
    bytes.extend(0x660eb6ffu32.to_le_bytes());
    bytes
}

fn request(id: &str, version: &str, minor: u32, dependency: Option<&str>) -> WritePackage {
    packed(id, version, minor, 0x10060, dependency)
}

/// A package whose manifest asks for API 1.`minor`, packed by SDK `sdk`.
fn packed(id: &str, version: &str, minor: u32, sdk: u32, dependency: Option<&str>) -> WritePackage {
    let mut manifest = vec![0; 64];
    let mut strings = Vec::new();
    for (index, value) in [id, id, "extension", version, "author", "MIT", ""]
        .into_iter()
        .enumerate()
    {
        manifest[28 + index * 2..30 + index * 2]
            .copy_from_slice(&(strings.len() as u16).to_le_bytes());
        strings.extend(value.bytes().chain([0]));
    }
    if let Some(dep) = dependency {
        manifest[52] = 1;
        manifest[44..46].copy_from_slice(&(strings.len() as u16).to_le_bytes());
        strings.extend(dep.bytes().chain([0]));
    }
    manifest[46] = if id == "dsp" { 5 } else { 1 };
    for (at, value) in [
        (0, 0x464d5652u32),
        (4, 4),
        (8, 0x10000 + minor),
        (16, 1),
        (24, strings.len() as u32),
    ] {
        manifest[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    manifest.extend(strings);
    let len = manifest.len() as u32;
    manifest.resize(manifest.len().next_multiple_of(4) + 20, 0);
    let hash = manifest.iter().fold(2166136261u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(16777619)
    });
    let mut data: Vec<_> = [0x30585652u32, 1, sdk, 0x660eb6ff, len, 1, hash]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    data.extend(manifest);
    WritePackage {
        id: id.into(),
        version: version.into(),
        sha256: format!("{:x}", Sha256::digest(&data)),
        data,
    }
}

#[test]
fn mounted_install_update_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    fs::write(root.join("osos-cfw.bin"), image("Development")).unwrap();
    fs::write(root.join("cfw-loader.bin"), vec![0; 0x25220]).unwrap();
    let mount = inspect(root).unwrap().unwrap();
    assert_eq!(mount.device.api.as_deref(), Some("1.81"));
    let ext = || request("crossfeed", "1.0.0", 78, Some("dsp"));
    assert!(write(&mount, ext())
        .unwrap_err()
        .contains("required library"));
    write(&mount, request("dsp", "1.0.0", 74, None)).unwrap();
    write(&mount, ext()).unwrap();
    assert!(remove(&mount, "dsp", PRIMARY)
        .unwrap_err()
        .contains("Required by"));
    let dir = folder(root, PRIMARY, false).unwrap().unwrap();
    fs::rename(dir.join("crossfeed.rvx"), dir.join("custom-name.rvx")).unwrap();
    write(&mount, request("crossfeed", "1.1.0", 81, Some("dsp"))).unwrap();
    let installed = inventory(root).unwrap();
    let crossfeed = installed.iter().find(|i| i.id == "crossfeed").unwrap();
    assert_eq!(crossfeed.version, "1.1.0");
    assert_eq!(
        (
            crossfeed.api.as_str(),
            crossfeed.firmware_id.as_str(),
            crossfeed.sdk.as_str()
        ),
        ("1.81", "660eb6ff", "1.96")
    );
    assert!(write(&mount, request("crossfeed", "1.2.0", 82, Some("dsp"))).is_err());
    // A copy placed without the installer still reports the API it needs.
    fs::write(
        dir.join("newer.rvx"),
        request("newer", "1.0.0", 84, None).data,
    )
    .unwrap();
    let installed = inventory(root).unwrap();
    assert_eq!(
        installed.iter().find(|i| i.id == "newer").unwrap().api,
        "1.84"
    );
    fs::remove_file(dir.join("newer.rvx")).unwrap();
    remove(&mount, "crossfeed", PRIMARY).unwrap();
    remove(&mount, "dsp", PRIMARY).unwrap();
    assert!(inventory(root).unwrap().is_empty());

    let pkg = request("sample", "1.0.0", 74, None);
    fs::write(dir.join("sample.rvx"), b"unrelated data").unwrap();
    assert!(write(&mount, pkg).is_err());
    assert_eq!(fs::read(dir.join("sample.rvx")).unwrap(), b"unrelated data");
}

#[test]
fn extension_data_moves_to_reprise_on_firmware_that_reads_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    fs::write(root.join("cfw-loader.bin"), vec![0; 0x25220]).unwrap();
    let old = root.join("Revola/Data/Extensions");
    let new = root.join("Reprise/Data/Extensions");
    let probe = || read_mounts(vec![root.into()], None).unwrap().unwrap();

    // The 0.4.0 firmware reads /Revola/Data: the folder stays.
    fs::create_dir_all(old.join("night-shift/.settings")).unwrap();
    fs::write(old.join("night-shift/.settings/0001"), b"on").unwrap();
    fs::write(root.join("osos-cfw.bin"), image("0.4.0")).unwrap();
    assert!(probe().snapshot.is_some());
    assert!(old.join("night-shift/.settings/0001").is_file());
    assert!(!root.join("Reprise/Data").exists());

    // Later firmware reads /Reprise/Data: the whole folder moves once.
    fs::write(root.join("osos-cfw.bin"), image("Development")).unwrap();
    assert!(probe().snapshot.is_some());
    assert_eq!(
        fs::read(new.join("night-shift/.settings/0001")).unwrap(),
        b"on"
    );
    assert!(!root.join("Revola").join("Data").exists());
    assert!(probe().snapshot.is_some());
    assert_eq!(
        fs::read(new.join("night-shift/.settings/0001")).unwrap(),
        b"on"
    );

    // A folder already on the Reprise side is kept; the others still move.
    fs::create_dir_all(old.join("night-shift/.settings")).unwrap();
    fs::write(old.join("night-shift/.settings/0001"), b"stale").unwrap();
    fs::create_dir_all(old.join("balance")).unwrap();
    fs::write(old.join("balance/left"), b"3").unwrap();
    assert!(probe().snapshot.is_some());
    assert_eq!(
        fs::read(new.join("night-shift/.settings/0001")).unwrap(),
        b"on"
    );
    assert_eq!(fs::read(new.join("balance/left")).unwrap(), b"3");
    assert_eq!(
        fs::read(old.join("night-shift/.settings/0001")).unwrap(),
        b"stale"
    );
    assert!(!old.join("balance").exists());
}

#[test]
fn theme_packs_leave_an_ipod_whose_firmware_no_longer_reads_them() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    fs::write(root.join("cfw-loader.bin"), vec![0; 0x25220]).unwrap();
    let themes = root.join("themes");
    let packs = || {
        for name in ["default", "dark"] {
            fs::create_dir_all(themes.join(name)).unwrap();
            fs::write(themes.join(name).join("theme.bin"), name).unwrap();
        }
    };
    let probe = || read_mounts(vec![root.into()], None).unwrap().unwrap();

    // The 0.4.0 firmware reads the packs: everything stays.
    packs();
    fs::write(root.join("osos-cfw.bin"), image("0.4.0")).unwrap();
    assert!(probe().snapshot.is_some());
    assert_eq!(
        fs::read(themes.join("default/theme.bin")).unwrap(),
        b"default"
    );
    assert_eq!(fs::read(themes.join("dark/theme.bin")).unwrap(), b"dark");

    // Later firmware does not: the packs and the folder go, once.
    fs::write(root.join("osos-cfw.bin"), image("Development")).unwrap();
    assert!(probe().snapshot.is_some());
    assert!(!themes.exists());
    assert!(probe().snapshot.is_some());
    assert!(!themes.exists());

    // Other content under /themes stays, with the folder.
    packs();
    fs::write(themes.join("notes.txt"), b"mine").unwrap();
    fs::create_dir_all(themes.join("mine")).unwrap();
    fs::write(themes.join("mine/theme.bin"), b"custom").unwrap();
    fs::write(themes.join("dark/extra"), b"kept").unwrap();
    assert!(probe().snapshot.is_some());
    assert!(!themes.join("default").exists());
    assert!(!themes.join("dark/theme.bin").exists());
    assert_eq!(fs::read(themes.join("dark/extra")).unwrap(), b"kept");
    assert_eq!(fs::read(themes.join("notes.txt")).unwrap(), b"mine");
    assert_eq!(fs::read(themes.join("mine/theme.bin")).unwrap(), b"custom");

    // A link is not followed: its target keeps the pack.
    #[cfg(unix)]
    {
        fs::remove_dir_all(&themes).unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("theme.bin"), b"elsewhere").unwrap();
        fs::create_dir_all(themes.join("default")).unwrap();
        fs::write(themes.join("default/theme.bin"), b"default").unwrap();
        std::os::unix::fs::symlink(outside.path(), themes.join("dark")).unwrap();
        assert!(probe().snapshot.is_some());
        assert!(!themes.join("default").exists());
        assert!(fs::symlink_metadata(themes.join("dark"))
            .unwrap()
            .is_symlink());
        assert_eq!(
            fs::read(outside.path().join("theme.bin")).unwrap(),
            b"elsewhere"
        );
    }
}

#[test]
fn unknown_development_can_install_and_old_release_cannot() {
    let dev = firmware::identify(b"RepriseOS Development\0", "iPod".into()).unwrap();
    let ext = parse(&request("sample", "1.0.0", 81, None).data).unwrap();
    assert!(dev.allows(&ext));
    for version in ["0.3.1", "0.3.2"] {
        assert!(!firmware::identify(&image(version), "iPod".into())
            .unwrap()
            .allows(&ext));
    }
    assert!(firmware::identify(&image("0.3.3"), "iPod".into())
        .unwrap()
        .allows(&ext));
}

#[test]
fn layout_floor_refuses_packages_from_older_sdks() {
    let old = parse(&packed("sample", "1.0.0", 82, 0x10058, None).data).unwrap();
    let rebuilt = parse(&packed("sample", "1.0.1", 82, 0x10059, None).data).unwrap();
    let before = firmware::identify(&core("0.4.0", 0x10052), "iPod".into()).unwrap();
    assert_eq!(before.sdk_floor, None);
    assert!(before.allows(&old) && before.allows(&rebuilt));
    for (version, api) in [
        ("0.5.0", 0x10059),
        ("0.5.0", 0x1005a),
        ("Development", 0x10059),
    ] {
        let device = firmware::identify(&core(version, api), "iPod".into()).unwrap();
        assert_eq!(device.sdk_floor.as_deref(), Some("1.89"));
        assert!(!device.allows(&old));
        assert!(device.allows(&rebuilt));
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    fs::write(root.join("osos-cfw.bin"), core("0.5.0", 0x10059)).unwrap();
    fs::write(root.join("cfw-loader.bin"), vec![0; 0x25220]).unwrap();
    let mount = inspect(root).unwrap().unwrap();
    assert!(write(&mount, packed("sample", "1.0.0", 82, 0x10058, None)).is_err());
    write(&mount, packed("sample", "1.0.1", 82, 0x10059, None)).unwrap();
    assert_eq!(inventory(root).unwrap()[0].sdk, "1.89");
}

#[test]
#[ignore = "requires a mounted RepriseOS iPod; reads only"]
fn read_connected_ipod() {
    let m = discover(&mounts::roots().unwrap())
        .unwrap()
        .expect("mount a RepriseOS iPod");
    println!("{:?}", m.device);
    for item in inventory(&m.root).unwrap() {
        println!("{} {} {}", item.folder, item.id, item.version);
    }
}

#[test]
#[ignore = "requires a local CFW image and built RVX packages"]
fn local_artifacts() {
    let firmware = fs::read(std::env::var_os("TEST_CFW_IMAGE").unwrap()).unwrap();
    let info = firmware::identify(&firmware, "test".into()).unwrap();
    let pkgs = PathBuf::from(std::env::var_os("TEST_RVX_PACKAGES").unwrap());
    let root = tempfile::tempdir().unwrap();
    let m = Mount {
        root: root.path().into(),
        device: info,
        token: "test".into(),
    };
    for id in ["dsp", "crossfeed", "night-shift", "balance", "playback-log"] {
        let data = fs::read(pkgs.join(format!("{id}.rvx"))).unwrap();
        let version = parse(&data).unwrap().version;
        write(
            &m,
            WritePackage {
                id: id.into(),
                version,
                sha256: format!("{:x}", Sha256::digest(&data)),
                data,
            },
        )
        .unwrap();
    }
    assert_eq!(inventory(root.path()).unwrap().len(), 5);
    assert!(remove(&m, "dsp", PRIMARY).is_err());
    remove(&m, "crossfeed", PRIMARY).unwrap();
    remove(&m, "dsp", PRIMARY).unwrap();
}

#[test]
fn eject_request_requires_support_and_survives_only_a_successful_handoff() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dir = root.join("iPod_Control/Device");
    fs::create_dir_all(&dir).unwrap();
    let cfw = root.join("osos-cfw.bin");
    let mut bytes = image("Development");
    fs::write(&cfw, &bytes).unwrap();
    fs::write(root.join("cfw-loader.bin"), vec![0; 0x25220]).unwrap();
    let mount = || inspect(root).unwrap().unwrap();
    let request = dir.join("RepriseReboot1.req");
    assert!(eject::request_reboot(&mount(), |_| panic!("unsupported")).is_err());
    assert!(!request.exists());
    bytes.extend_from_slice(eject::FIRMWARE_PATH);
    fs::write(cfw, bytes).unwrap();
    assert!(eject::request_reboot(&mount(), |_| {
        assert_eq!(fs::read(&request).unwrap(), b"RBR1");
        Err("busy".into())
    })
    .is_err());
    assert!(!request.exists());
    fs::write(&request, b"unrelated").unwrap();
    assert!(eject::request_reboot(&mount(), |_| panic!("name collision")).is_err());
    assert_eq!(fs::read(&request).unwrap(), b"unrelated");
    fs::remove_file(&request).unwrap();
    eject::request_reboot(&mount(), |path| {
        assert_eq!(path, root);
        Ok(())
    })
    .unwrap();
    assert_eq!(fs::read(&request).unwrap(), b"RBR1");
}

#[test]
fn mount_probe_skips_unchanged_inventory_and_detects_removal_and_return() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let probe = |key: Option<&str>| read_mounts(vec![root.into()], key).unwrap();
    let empty = probe(None).unwrap();
    assert!(empty.snapshot.is_none());
    fs::create_dir_all(root.join("iPod_Control/Device")).unwrap();
    fs::write(root.join("osos-cfw.bin"), image("Development")).unwrap();
    fs::write(root.join("cfw-loader.bin"), vec![0; 0x25220]).unwrap();
    let mounted = probe(Some(&empty.mounts)).unwrap();
    assert!(mounted.snapshot.is_some());
    assert!(probe(Some(&mounted.mounts)).is_none());
    fs::rename(root.join("cfw-loader.bin"), root.join("offline.bin")).unwrap();
    assert!(probe(Some(&mounted.mounts)).unwrap().snapshot.is_none());
    fs::rename(root.join("offline.bin"), root.join("cfw-loader.bin")).unwrap();
    assert!(probe(Some(&empty.mounts)).unwrap().snapshot.is_some());
}
