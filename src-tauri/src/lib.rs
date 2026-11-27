mod device;
mod extensions;
mod firmware;
mod install;
mod package;
mod theme;
mod updates;
mod usb_permissions;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    reprise_device::mac_dfu_guard::install_panic_hook();
    if let Err(error) = reprise_device::mac_dfu_guard::recover_stale() {
        eprintln!("macOS DFU guard recovery: {error}");
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::UpdateState::default())
        .manage(device::UsbState::default())
        .manage(package::PackageState::default())
        .manage(firmware::FirmwareState::default())
        .manage(install::InstallState::default())
        .on_window_event(|window, event| {
            use tauri::Manager;
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window
                    .state::<install::InstallState>()
                    .running
                    .load(std::sync::atomic::Ordering::Acquire)
                    || window
                        .state::<updates::UpdateState>()
                        .installing
                        .load(std::sync::atomic::Ordering::Acquire)
                {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            usb_permissions::usb_permissions_ready,
            usb_permissions::enable_usb_access,
            device::discover_devices,
            device::check_device,
            device::mac_dfu_guard_begin,
            device::mac_dfu_guard_end,
            package::choose_local_package,
            package::prepare_package,
            firmware::choose_firmware,
            firmware::cached_firmware,
            extensions::read_extension_device,
            extensions::eject::eject_and_reboot,
            extensions::write_extension,
            extensions::remove_extension,
            install::install,
            install::diagnostics::save_debug_zip,
            updates::check_installer_update,
            updates::install_installer_update,
            updates::open_installer_download,
            install::cancel_install
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            use tauri::Manager;
            // Tauri exits the process without dropping managed state.
            if let tauri::RunEvent::Exit = event {
                let _ = reprise_device::mac_dfu_guard::restore_now();
            }
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if app
                    .state::<install::InstallState>()
                    .running
                    .load(std::sync::atomic::Ordering::Acquire)
                    || app
                        .state::<updates::UpdateState>()
                        .installing
                        .load(std::sync::atomic::Ordering::Acquire)
                {
                    api.prevent_exit();
                }
            }
        });
}
