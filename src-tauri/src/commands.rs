use std::{path::PathBuf, sync::Arc};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::{
    domain::{
        AppSettings, DashboardSnapshot, HistoryResponse, PingSample, StorageInfo, Target,
        TargetValidationError, unix_time_ms,
    },
    monitor::{MonitorError, MonitorService},
    storage::{Database, StorageError},
    tray::{refresh_tray, show_main_window},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    code: String,
    detail: Option<String>,
}

impl CommandError {
    pub(crate) fn new(code: impl Into<String>, detail: impl ToString) -> Self {
        Self {
            code: code.into(),
            detail: Some(detail.to_string()),
        }
    }
}

impl From<TargetValidationError> for CommandError {
    fn from(error: TargetValidationError) -> Self {
        Self::new(error.code(), error)
    }
}

impl From<StorageError> for CommandError {
    fn from(error: StorageError) -> Self {
        let code = match &error {
            StorageError::Database(_) => "storage",
            StorageError::Io(_) => "filesystem",
            StorageError::Json(_) => "serialization",
            StorageError::InvalidData(_) => "invalidData",
        };
        Self::new(code, error)
    }
}

impl From<MonitorError> for CommandError {
    fn from(error: MonitorError) -> Self {
        let code = error.code();
        Self::new(code, error)
    }
}

/// Read cache of the persisted settings. The tray and the notification path need the language and
/// the notification switch on every probe, and reopening the database five times a second for that
/// is pure overhead.
pub type SharedSettings = Arc<Mutex<AppSettings>>;

pub struct AppState {
    pub monitor: Arc<MonitorService>,
    pub database: Database,
    pub settings: SharedSettings,
}

#[tauri::command]
pub fn get_dashboard(state: State<'_, AppState>) -> DashboardSnapshot {
    state.monitor.snapshot()
}

// Tauri runs the body of a synchronous command on the event loop thread, so every command that
// touches the database is async and hands the work to a blocking thread; otherwise a query that has
// to wait for the write lock freezes both windows and the tray.
#[tauri::command]
pub async fn list_targets(state: State<'_, AppState>) -> Result<Vec<Target>, CommandError> {
    let database = state.database.clone();
    off_thread(move || database.list_targets(false)).await
}

#[tauri::command]
pub async fn create_target(
    state: State<'_, AppState>,
    name: String,
    host: String,
) -> Result<Target, CommandError> {
    let monitor = Arc::clone(&state.monitor);
    off_thread(move || monitor.create_target(name, host)).await
}

#[tauri::command]
pub async fn save_target(
    state: State<'_, AppState>,
    target: Target,
) -> Result<Target, CommandError> {
    let monitor = Arc::clone(&state.monitor);
    off_thread(move || monitor.upsert_target(target)).await
}

#[tauri::command]
pub async fn archive_target(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<(), CommandError> {
    let monitor = Arc::clone(&state.monitor);
    off_thread(move || monitor.archive_target(&target_id)).await
}

#[tauri::command]
pub async fn set_monitoring_paused(
    state: State<'_, AppState>,
    paused: bool,
) -> Result<(), CommandError> {
    let monitor = Arc::clone(&state.monitor);
    // Pausing closes the open quality intervals, which is a database write.
    off_thread(move || {
        monitor.set_paused(paused);
        Ok::<(), StorageError>(())
    })
    .await
}

#[tauri::command]
pub async fn test_target(
    state: State<'_, AppState>,
    target: Target,
) -> Result<PingSample, CommandError> {
    target.validate_probe()?;
    Ok(state.monitor.test_probe(&target).await)
}

#[tauri::command]
pub async fn get_history(
    state: State<'_, AppState>,
    target_ids: Vec<String>,
    from_ms: i64,
    to_ms: i64,
    max_points: usize,
) -> Result<HistoryResponse, CommandError> {
    let database = state.database.clone();
    off_thread(move || database.history(&target_ids, from_ms, to_ms, max_points)).await
}

/// Runs blocking database work on a worker thread and maps both failure modes to a UI error.
async fn off_thread<T, E>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, CommandError>
where
    T: Send + 'static,
    E: Into<CommandError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        // A join failure means the worker itself gave up (it panicked), which is not a storage
        // problem and should not be reported as one.
        .map_err(|error| CommandError::new("unknown", error))?
        .map_err(Into::into)
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, CommandError> {
    let database = state.database.clone();
    off_thread(move || database.load_settings()).await
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, CommandError> {
    // Deferral and skip state belong to the updater. Accepting the frontend's copy let a stale
    // settings object resurrect an update the user had already skipped, so the stored values are
    // read and written back in one hop — nothing else can slip in between.
    let database = state.database.clone();
    let settings = off_thread(move || {
        database.update_settings(|stored| {
            *stored = AppSettings {
                update_deferred_version: stored.update_deferred_version.take(),
                update_deferred_until_ms: stored.update_deferred_until_ms,
                skipped_update_version: stored.skipped_update_version.take(),
                ..settings
            };
        })
    })
    .await?;
    let autolaunch = app.autolaunch();
    let currently_enabled = autolaunch
        .is_enabled()
        .map_err(|error| CommandError::new("autostart", error))?;
    if settings.start_at_login != currently_enabled {
        if settings.start_at_login {
            autolaunch
                .enable()
                .map_err(|error| CommandError::new("autostart", error))?;
        } else {
            autolaunch
                .disable()
                .map_err(|error| CommandError::new("autostart", error))?;
        }
    }
    *state.settings.lock() = settings.clone();
    refresh_tray(&app, &settings, &state.monitor.snapshot())
        .map_err(|error| CommandError::new("monitoring", error))?;
    let _ = app.emit("settings-updated", &settings);
    Ok(settings)
}

#[tauri::command]
pub async fn get_storage_info(state: State<'_, AppState>) -> Result<StorageInfo, CommandError> {
    let database = state.database.clone();
    off_thread(move || database.storage_info()).await
}

#[tauri::command]
pub async fn run_retention_cleanup(state: State<'_, AppState>) -> Result<u64, CommandError> {
    let database = state.database.clone();
    off_thread(move || {
        let settings = database.load_settings()?;
        let deleted = database.cleanup(settings.retention_days)?;
        // A manual clean-up is a request for disk space back, so hand the freed pages to the file
        // system as well — but only when there is enough of them to be worth the write lock, and
        // never at the cost of reporting a successful pass as a failure.
        if let Err(error) = database.compact() {
            eprintln!("LNPM could not compact the database: {error}");
        }
        Ok::<u64, StorageError>(deleted)
    })
    .await
}

#[tauri::command]
pub async fn backup_database(state: State<'_, AppState>) -> Result<String, CommandError> {
    let database = state.database.clone();
    off_thread(move || {
        let storage = database.storage_info()?;
        let destination = PathBuf::from(storage.data_directory)
            .join(format!("lnpm-backup-{}.sqlite3", unix_time_ms()));
        database.backup_to(&destination)?;
        Ok::<String, StorageError>(destination.to_string_lossy().to_string())
    })
    .await
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    show_main_window(&app);
}

#[tauri::command]
pub fn hide_popup(app: AppHandle) {
    if let Some(window) = app.get_webview_window("popup") {
        let _ = window.hide();
        crate::tray::announce_visibility(&window, false);
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) -> Result<(), CommandError> {
    if app
        .try_state::<crate::updater::UpdateManager>()
        .is_some_and(|manager| manager.is_installing())
    {
        return Err(CommandError::new(
            "updateBusy",
            "an update is currently being installed",
        ));
    }
    app.exit(0);
    Ok(())
}
