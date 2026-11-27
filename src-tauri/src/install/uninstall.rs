use super::{
    firmware,
    job::{self, InstallEvent, InstallFailure, InstallOutcome, InstallResult, Job},
    selected_bundle, take_checked, InstallState, Result,
};
use reprise_bundle::assembly;
use reprise_device::UploadHelper;
use tauri::{ipc::Channel, AppHandle, Manager};

/// Remove the RepriseOS files, then restore Apple's bootloader. Files go first
/// because the RepriseOS bootloader stops at a missing-file screen without them,
/// while the restored Apple bootloader ignores them. Removal is best effort:
/// whatever refuses to go is reported and the bootloader is restored anyway, so
/// the iPod never ends up with the RepriseOS bootloader and no firmware file.
/// With `keep_files` the files stay for a later install and only the bootloader
/// changes. Both steps can run again.
#[tauri::command]
pub async fn uninstall(
    app: AppHandle,
    package_digest: String,
    keep_files: bool,
    on_event: Channel<InstallEvent>,
) -> std::result::Result<InstallResult, InstallFailure> {
    super::start(app, move |app, state| {
        run(app, state, package_digest, keep_files, on_event)
    })
    .await
}

fn run(
    app: &AppHandle,
    state: &InstallState,
    package_digest: String,
    keep_files: bool,
    channel: Channel<InstallEvent>,
) -> Result<InstallResult> {
    let bundle = selected_bundle(app, &package_digest)?;
    let helper = UploadHelper::from_bytes(
        bundle.file("usb_helper", "image")?,
        bundle.file("usb_helper", "descriptor")?,
    )?;
    if !keep_files && !helper.supports_file_removal() {
        return Err("Choose a newer package with file removal support.".into());
    }
    let uninstaller = assembly::nor_uninstaller(&bundle)?;
    let checked = take_checked(app, &bundle)?;
    let identity = checked
        .report
        .identity
        .as_ref()
        .ok_or("Missing checked device identity.")?;
    let data = app.path().app_data_dir()?;
    let directory = job::backup_directory(&data, &identity.hardware_id)?;
    job::save_json(&directory.join("device.json"), &checked.report)?;
    job::save_json(
        &directory.join("inputs.json"),
        &serde_json::json!({"bundle": bundle.digest(), "keep_files": keep_files}),
    )?;
    let mut job = Job::new(state, channel, directory, "uninstall")?;
    if keep_files {
        job.set_weight("removeFiles", 0);
    }
    job.log(format!(
        "Installer {}; uninstall with package {} ({}); keep files: {keep_files}",
        env!("CARGO_PKG_VERSION"),
        bundle.manifest().version,
        bundle.digest(),
    ));
    job.log(format!(
        "Device: {} serial={} hardware_id={:02x?}; logs={}",
        identity.model,
        identity.serial,
        identity.hardware_id,
        job.directory.display()
    ));
    let outcome = (|| {
        let mut session = checked.session;
        job.stage("checkBootloader", "Checking installed bootloader")?;
        let bootloader = session.inspect_bootloader(|p| {
            job.progress_in(
                "Verifying installed bootloader",
                (0.0, 1.0),
                p.completed,
                p.total,
            )
        })?;
        job.save_json("bootloader.json", &bootloader)?;
        job.log(format!(
            "Detected bootloader: apple={}; code_sha256={}",
            bootloader.apple, bootloader.fingerprint.sha256
        ));
        if bootloader.apple && keep_files {
            // Nothing to do; leave DFU so the iPod does not sit there.
            job.begin_restart()?;
            session.restart()?;
            return Err("Apple's bootloader is already in place and the files were kept, so there was nothing to remove. Your iPod restarts into Apple's software.".into());
        }
        job.plan_uninstall(bootloader.apple, keep_files)?;
        if !bootloader.apple {
            firmware::backup(
                &mut session,
                identity,
                assembly::BootloaderAction::Upgrade,
                &mut job,
            )?;
        }

        if !keep_files {
            job.begin_file_removal()?;
            let directory = job.directory.clone();
            let (returned, removed) =
                session
                    .remove_files_with_log(&helper, Some(&directory), |detail| job.log(detail))?;
            session = returned;
            job.save_json("removal.json", &removed)?;
            job.emit(
                format!(
                    "Removed: companion loader {}; RepriseOS {}; extensions {}; unfinished uploads {}",
                    removed.companion, removed.firmware, removed.extensions, removed.staged_uploads
                ),
                None,
            );
            job.leftovers = removed.leftovers();
            if !job.leftovers.is_empty() {
                job.emit(
                    if bootloader.apple {
                        format!("Could not remove {}", job.leftovers.join(", "))
                    } else {
                        format!(
                            "Could not remove {}; continuing so the bootloader is restored",
                            job.leftovers.join(", ")
                        )
                    },
                    None,
                );
            }
        }

        if bootloader.apple {
            job.begin_restart()?;
            session.restart()?;
            return Ok(InstallOutcome::FilesRemoved);
        }
        job.begin_bootloader_removal()?;
        session.install_bootloader(&uninstaller, |done, total| {
            job.emit(
                "Sending bootloader uninstaller",
                Some((done as u64, total as u64)),
            )
        })?;
        job.emit("Bootloader uninstaller sent", None);
        Ok(InstallOutcome::UninstallerSent)
    })();
    job.finish(outcome)
}
