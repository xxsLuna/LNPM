use std::{path::PathBuf, sync::Arc};

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

pub struct AppState {
    pub monitor: Arc<MonitorService>,
    pub database: Database,
}

#[tauri::command]
pub fn get_dashboard(state: State<'_, AppState>) -> DashboardSnapshot {
    state.monitor.snapshot()
}

#[tauri::command]
pub fn list_targets(state: State<'_, AppState>) -> Result<Vec<Target>, CommandError> {
    Ok(state.database.list_targets(false)?)
}

#[tauri::command]
pub fn create_target(
    state: State<'_, AppState>,
    name: String,
    host: String,
) -> Result<Target, CommandError> {
    Ok(state.monitor.create_target(name, host)?)
}

#[tauri::command]
pub fn save_target(state: State<'_, AppState>, target: Target) -> Result<Target, CommandError> {
    Ok(state.monitor.upsert_target(target)?)
}

#[tauri::command]
pub fn archive_target(state: State<'_, AppState>, target_id: String) -> Result<(), CommandError> {
    Ok(state.monitor.archive_target(&target_id)?)
}

#[tauri::command]
pub fn set_monitoring_paused(state: State<'_, AppState>, paused: bool) {
    state.monitor.set_paused(paused);
}

#[tauri::command]
pub async fn test_target(
    state: State<'_, AppState>,
    target: Target,
) -> Result<PingSample, CommandError> {
    target.validate()?;
    Ok(state.monitor.test_probe(&target).await)
}

#[tauri::command]
pub fn get_history(
    state: State<'_, AppState>,
    target_ids: Vec<String>,
    from_ms: i64,
    to_ms: i64,
    max_points: usize,
) -> Result<HistoryResponse, CommandError> {
    Ok(state
        .database
        .history(&target_ids, from_ms, to_ms, max_points)?)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, CommandError> {
    Ok(state.database.load_settings()?)
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, CommandError> {
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
    state.database.save_settings(&settings)?;
    refresh_tray(&app, &settings, &state.monitor.snapshot())
        .map_err(|error| CommandError::new("monitoring", error))?;
    let _ = app.emit("settings-updated", &settings);
    Ok(settings)
}

#[tauri::command]
pub fn get_storage_info(state: State<'_, AppState>) -> Result<StorageInfo, CommandError> {
    Ok(state.database.storage_info()?)
}

#[tauri::command]
pub fn run_retention_cleanup(state: State<'_, AppState>) -> Result<u64, CommandError> {
    let settings = state.database.load_settings()?;
    Ok(state.database.cleanup(settings.retention_days)?)
}

#[tauri::command]
pub fn backup_database(state: State<'_, AppState>) -> Result<String, CommandError> {
    let storage = state.database.storage_info()?;
    let destination = PathBuf::from(storage.data_directory)
        .join(format!("lnpm-backup-{}.sqlite3", unix_time_ms()));
    state.database.backup_to(&destination)?;
    Ok(destination.to_string_lossy().to_string())
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    show_main_window(&app);
}

#[tauri::command]
pub fn hide_popup(app: AppHandle) {
    if let Some(window) = app.get_webview_window("popup") {
        let _ = window.hide();
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
