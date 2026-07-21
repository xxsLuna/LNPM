pub mod commands;
pub mod domain;
pub mod i18n;
pub mod monitor;
pub mod probe;
pub mod quality;
pub mod storage;
pub mod tray;
pub mod updater;

use std::{io, sync::Arc, time::Duration};

use commands::AppState;
use directories::ProjectDirs;
use monitor::MonitorService;
use probe::SurgePingProbe;
use storage::Database;
use tauri::{Manager, RunEvent};
use tauri_plugin_autostart::MacosLauncher;
use tray::{TauriEventSink, build_tray, handle_menu_event, handle_tray_event, handle_window_event};
use updater::UpdateManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let data_directory = ProjectDirs::from("io.github.xxsluna", "xxsLuna", "LNPM")
                .map(|directories| directories.data_local_dir().to_path_buf())
                .ok_or_else(|| {
                    io::Error::other("unable to determine application data directory")
                })?;
            let database = Database::new(data_directory)?;
            let settings = database.load_settings()?;
            let probe =
                tauri::async_runtime::block_on(SurgePingProbe::new()).map_err(io::Error::other)?;
            let event_sink = TauriEventSink::new(app.handle().clone(), database.clone());
            let monitor = MonitorService::new(database.clone(), probe, event_sink);
            monitor.start_all()?;
            app.manage(AppState {
                monitor: Arc::clone(&monitor),
                database: database.clone(),
            });
            let update_manager = UpdateManager::new(app.handle().clone(), database.clone());
            app.manage(update_manager.clone());
            update_manager.start();
            build_tray(app, &settings)?;

            if settings.first_run || monitor.snapshot().targets.is_empty() {
                tray::show_main_window(app.handle());
            }

            let maintenance_database = database;
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
                    let database = maintenance_database.clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        let settings = database.load_settings()?;
                        database.cleanup(settings.retention_days)
                    })
                    .await;
                }
            });
            Ok(())
        })
        .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()))
        .on_tray_icon_event(|app, event| handle_tray_event(app, &event))
        .on_window_event(handle_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::get_dashboard,
            commands::list_targets,
            commands::create_target,
            commands::save_target,
            commands::archive_target,
            commands::set_monitoring_paused,
            commands::test_target,
            commands::get_history,
            commands::get_settings,
            commands::save_settings,
            commands::get_storage_info,
            commands::run_retention_cleanup,
            commands::backup_database,
            commands::show_main,
            commands::hide_popup,
            commands::quit_app,
            updater::get_pending_update,
            updater::defer_update,
            updater::skip_update,
            updater::install_update,
        ]);

    let mut context = tauri::generate_context!();
    context.set_default_window_icon(Some(
        tauri::include_image!("./icons/128x128.png").to_owned(),
    ));
    let app = builder.build(context).expect("error while building LNPM");
    app.run(|app, event| {
        if matches!(event, RunEvent::Exit | RunEvent::ExitRequested { .. })
            && let Some(state) = app.try_state::<AppState>()
        {
            state.monitor.shutdown();
        }
    });
}
