pub mod commands;
pub mod domain;
pub mod i18n;
pub mod monitor;
pub mod probe;
pub mod quality;
pub mod storage;
pub mod tray;
pub mod updater;

use std::{fs, io, path::PathBuf, sync::Arc, time::Duration};

use commands::{AppState, SharedSettings};
use directories::ProjectDirs;
use monitor::MonitorService;
use parking_lot::Mutex;
use probe::SurgePingProbe;
use storage::Database;
use tauri::{AppHandle, Manager, RunEvent, WebviewWindowBuilder};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_notification::NotificationExt;
use tray::{TauriEventSink, build_tray, handle_menu_event, handle_tray_event, handle_window_event};
use updater::UpdateManager;

const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a failed startup keeps the process alive so its notification can be delivered.
const STARTUP_FAILURE_LINGER: Duration = Duration::from_secs(3);

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
            let Startup {
                monitor,
                database,
                settings,
            } = match initialize(app.handle()) {
                Ok(startup) => startup,
                Err(error) => {
                    report_startup_failure(app.handle(), error.as_ref());
                    return Ok(());
                }
            };
            let current_settings = settings.lock().clone();
            app.manage(AppState {
                monitor: Arc::clone(&monitor),
                database: database.clone(),
                settings: Arc::clone(&settings),
            });
            let update_manager = UpdateManager::new(app.handle().clone(), database.clone());
            app.manage(update_manager.clone());
            update_manager.start();
            if let Err(error) = build_tray(app, &current_settings) {
                eprintln!("LNPM could not create the tray icon: {error}");
            }
            // The windows are declared with `"create": false` so that no webview can invoke a
            // command before the state above is managed; create them now that it is.
            let mut main_window = Err(io::Error::other("no main window is configured"));
            for window_config in app.config().app.windows.clone() {
                let built = WebviewWindowBuilder::from_config(app.handle(), &window_config)
                    .and_then(|builder| builder.build());
                match (window_config.label.as_str(), built) {
                    ("main", Ok(_)) => main_window = Ok(()),
                    ("main", Err(error)) => {
                        main_window = Err(io::Error::other(format!(
                            "the main window could not be created: {error}"
                        )))
                    }
                    (_, Ok(_)) => {}
                    (label, Err(error)) => {
                        eprintln!("LNPM could not create the {label} window: {error}")
                    }
                }
            }
            if let Err(error) = main_window {
                // Without the main window there is no way to reach the app at all, and the process
                // would keep running invisibly while blocking every later launch.
                report_startup_failure(app.handle(), &error);
                return Ok(());
            }

            if current_settings.first_run || monitor.snapshot().targets.is_empty() {
                tray::show_main_window(app.handle());
            }

            spawn_maintenance(database);
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

struct Startup {
    monitor: Arc<MonitorService>,
    database: Database,
    settings: SharedSettings,
}

fn initialize(app: &AppHandle) -> Result<Startup, Box<dyn std::error::Error>> {
    let database = Database::new(data_directory()?)?;
    let settings: SharedSettings = Arc::new(Mutex::new(database.load_settings()?));
    let probe = tauri::async_runtime::block_on(SurgePingProbe::new()).map_err(io::Error::other)?;
    let event_sink = TauriEventSink::new(app.clone(), Arc::clone(&settings));
    let monitor = MonitorService::new(database.clone(), probe, event_sink);
    monitor.start_all()?;
    Ok(Startup {
        monitor,
        database,
        settings,
    })
}

fn data_directory() -> io::Result<PathBuf> {
    ProjectDirs::from("io.github.xxsluna", "xxsLuna", "LNPM")
        .map(|directories| directories.data_local_dir().to_path_buf())
        .ok_or_else(|| io::Error::other("unable to determine application data directory"))
}

/// A failed startup must not turn into a silent panic, nor into an invisible process that the
/// single-instance guard would turn into "every later launch does nothing". Report through a
/// notification and a log file, then end the process.
fn report_startup_failure(app: &AppHandle, error: &dyn std::error::Error) {
    let message = error.to_string();
    eprintln!("LNPM startup failed: {message}");
    if let Ok(directory) = data_directory() {
        let _ = fs::create_dir_all(&directory);
        let _ = fs::write(directory.join("startup-error.log"), format!("{message}\n"));
    }
    let _ = app
        .notification()
        .builder()
        .title("LNPM")
        .body(message)
        .show();
    // The notification plugin only spawns the toast, so the process has to outlive that task long
    // enough for the request to reach the OS. Run the normal teardown first, then exit with a status
    // that says the launch failed — `AppHandle::exit` would report success whatever code it is given.
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_FAILURE_LINGER).await;
        handle.cleanup_before_exit();
        std::process::exit(1);
    });
}

/// Retention has to run once at startup as well: a session shorter than [`MAINTENANCE_INTERVAL`]
/// would otherwise never prune anything and the database would grow without bound.
fn spawn_maintenance(database: Database) {
    tauri::async_runtime::spawn(async move {
        loop {
            let maintenance = database.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                let settings = maintenance.load_settings()?;
                maintenance.cleanup(settings.retention_days)
            })
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => eprintln!("LNPM retention cleanup failed: {error}"),
                Err(error) => eprintln!("LNPM retention cleanup could not run: {error}"),
            }
            tokio::time::sleep(MAINTENANCE_INTERVAL).await;
        }
    });
}
