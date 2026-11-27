use super::{
    job::{InstallOutcome, Job},
    Result,
};
use reprise_bundle::{
    assembly::{self, Artifacts},
    VerifiedBundle,
};
use reprise_device::{Session, UploadFile, UploadHelper, UploadOptions};
use std::io::Cursor;

/// Each upload costs about three seconds of fixed work plus a second per MB,
/// so a file weighs its bytes plus this many.
const FILE_OVERHEAD_BYTES: u64 = 3 << 20;
const BYTES_PER_SECOND: u64 = 1 << 20;

/// Share of the upload phase spent hashing the files before the helper launches.
const HASH_SHARE: f64 = 0.1;

/// Where each transfer sub-stage lies within a file's share, as (start, end).
fn band(stage: &str) -> Option<(f64, f64)> {
    Some(match stage {
        "upload" => (0.0, 0.55),
        "write" => (0.55, 0.9),
        "verify" => (0.9, 1.0),
        "complete" => (1.0, 1.0),
        _ => return None,
    })
}

pub(super) fn inspect_storage(
    session: Session,
    helper: &UploadHelper,
    bundle: &VerifiedBundle,
    job: &mut Job<'_>,
) -> Result<Session> {
    job.stage("storage", "Checking storage")?;
    let required = assembly::disk_bytes(bundle)?;
    let directory = job.directory.clone();
    let (returned, storage) =
        session.inspect_storage_with_diagnostics(helper, required, Some(&directory), |detail| {
            job.log(detail)
        })?;

    job.save_json("storage.json", &storage)?;
    job.emit(
        format!(
            "{} bytes free; companion present: {}; OSOS present: {}",
            storage.free_bytes, storage.companion_exists, storage.osos_exists
        ),
        None,
    );

    Ok(returned)
}

/// One install file: its job stage, the name shown in progress and where it goes.
struct Upload<'a> {
    stage: &'static str,
    name: &'static str,
    destination: &'static str,
    bytes: &'a [u8],
}

pub(super) fn install(
    mut session: Session,
    helper: &UploadHelper,
    artifacts: &Artifacts,
    action: assembly::BootloaderAction,
    job: &mut Job<'_>,
) -> Result<InstallOutcome> {
    let uploads = [
        Upload {
            stage: "uploadLoader",
            name: "companion loader",
            destination: "/Reprise/Firmware/loader.bin",
            bytes: &artifacts.companion,
        },
        Upload {
            stage: "uploadOsos",
            name: "RepriseOS",
            destination: "/Reprise/Firmware/reprise.bin",
            bytes: &artifacts.osos,
        },
    ];
    let weights: Vec<u64> = uploads
        .iter()
        .map(|u| FILE_OVERHEAD_BYTES + u.bytes.len() as u64)
        .collect();
    job.set_weight(
        "upload",
        (weights.iter().sum::<u64>() / BYTES_PER_SECOND) as u32,
    );
    if helper.supports_batch_upload() {
        session = upload_batch(session, helper, &uploads, &weights, 0, job)?;
    } else {
        // Older packages' helpers take one file per launch.
        for (first, file) in uploads.iter().enumerate() {
            session = upload_batch(
                session,
                helper,
                std::slice::from_ref(file),
                &weights,
                first,
                job,
            )?;
        }
    }
    if action == assembly::BootloaderAction::Keep {
        job.begin_restart()?;
        session.restart()?;
        job.emit(
            "UPDATE: disk files verified; restart requested; NOR unchanged",
            None,
        );
        return Ok(InstallOutcome::FilesUpdated);
    }
    job.begin_bootloader()?;
    session.install_bootloader(&artifacts.nor_installer, |done, total| {
        job.emit(
            "Sending bootloader installer",
            Some((done as u64, total as u64)),
        )
    })?;
    job.emit("Bootloader installer sent", None);
    Ok(InstallOutcome::InstallerSent)
}

/// Sends `uploads` through one helper session and saves a report per file of every
/// batch whose last file was verified, also when a later batch fails. `weights`
/// covers every file of the install and `first` is the index of `uploads[0]` in it.
fn upload_batch(
    session: Session,
    helper: &UploadHelper,
    uploads: &[Upload<'_>],
    weights: &[u64],
    first: usize,
    job: &mut Job<'_>,
) -> Result<Session> {
    let total_weight: u64 = weights.iter().sum();
    let mut cursors: Vec<Cursor<&[u8]>> = uploads.iter().map(|u| Cursor::new(u.bytes)).collect();
    let mut files: Vec<UploadFile<'_>> = cursors
        .iter_mut()
        .zip(uploads)
        .map(|(input, upload)| UploadFile {
            input,
            options: UploadOptions {
                destination: upload.destination.into(),
                overwrite: true,
            },
        })
        .collect();
    job.stage(uploads[0].stage, &format!("Preparing {}", uploads[0].name))?;
    let log_job = std::cell::RefCell::new(&mut *job);
    // Hashing runs over every file before the launch and keeps the current stage.
    let mut current = 0;
    let uploaded = session.upload_files(
        helper,
        &mut files,
        |p| {
            let Upload { stage, name, .. } = &uploads[p.file];
            let mut job = log_job.borrow_mut();
            if p.stage != "hash" && current != p.file {
                current = p.file;
                job.stage(stage, &format!("Preparing {name}"))
                    .map_err(|e| reprise_device::Error::Invalid(e.to_string()))?;
            }
            if p.total > 0 {
                let file = first + p.file;
                let before: u64 = weights[..file].iter().sum();
                let through = |within: f64| {
                    (before as f64 + weights[file] as f64 * within) / total_weight as f64
                };
                let count = p.completed as f64 / p.total as f64;
                if p.stage == "hash" {
                    job.set_fraction(HASH_SHARE * through(count));
                } else if let Some((start, end)) = band(p.stage) {
                    let within = start + (end - start) * count;
                    job.set_fraction(HASH_SHARE + (1.0 - HASH_SHARE) * through(within));
                }
            }
            let detail = match p.stage {
                "hash" => format!("Checking {name}"),
                "launch" => "Starting transfer helper".into(),
                "upload" => format!("Uploading {name}"),
                "write" => format!("Writing {name}"),
                "verify" => format!("Verifying {name}"),
                "dfu-return" => "Reconnecting to iPod".into(),
                "complete" => format!("Verified {name}"),
                _ => format!("Preparing {name}"),
            };
            let progress = match p.stage {
                "hash" | "upload" | "write" | "verify" | "complete" => Some((p.completed, p.total)),
                _ => None,
            };
            job.emit(detail, progress);
            Ok(())
        },
        |detail| log_job.borrow_mut().log(detail),
    );
    let (session, reports) = match uploaded {
        Ok(uploaded) => (uploaded.session, uploaded.reports),
        Err(failure) => {
            for (upload, report) in uploads.iter().zip(&failure.reports) {
                if let Err(e) = job.save_json(&format!("{}.json", upload.stage), report) {
                    job.log(format!("Could not save {}.json: {e}", upload.stage));
                }
            }
            return Err(failure.into());
        }
    };
    for (upload, report) in uploads.iter().zip(&reports) {
        job.save_json(&format!("{}.json", upload.stage), report)?;
    }
    Ok(session)
}
