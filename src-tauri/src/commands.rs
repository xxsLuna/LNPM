use std::{path::PathBuf, sync::Arc};

use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::{
    domain::{
        AppSettings, DashboardSnapshot, HistoryResponse, PingSample, StorageInfo, Target,
        unix_time_ms,
    },
    monitor::MonitorService,
    storage::Database,
    tray::show_main_window,
};

pub struct AppState {
    pub monitor: Arc<MonitorService>,
    pub database: Database,
}

#[tauri::command]
pub fn get_dashboard(state: State<'_, AppState>) -> DashboardSnapshot {
    state.monitor.snapshot()
}

#[tauri::command]
pub fn list_targets(state: State<'_, AppState>) -> Result<Vec<Target>, String> {
    state
        .database
        .list_targets(false)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn create_target(
    state: State<'_, AppState>,
    name: String,
    host: String,
) -> Result<Target, String> {
    state
        .monitor
        .create_target(name, host)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_target(state: State<'_, AppState>, target: Target) -> Result<Target, String> {
    state
        .monitor
        .upsert_target(target)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn archive_target(state: State<'_, AppState>, target_id: String) -> Result<(), String> {
    state
        .monitor
        .archive_target(&target_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_monitoring_paused(state: State<'_, AppState>, paused: bool) {
    state.monitor.set_paused(paused);
}

#[tauri::command]
pub async fn test_target(state: State<'_, AppState>, target: Target) -> Result<PingSample, String> {
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
) -> Result<HistoryResponse, String> {
    state
        .database
        .history(&target_ids, from_ms, to_ms, max_points)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    state
        .database
        .load_settings()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    let autolaunch = app.autolaunch();
    let currently_enabled = autolaunch.is_enabled().map_err(|error| error.to_string())?;
    if settings.start_at_login != currently_enabled {
        if settings.start_at_login {
            autolaunch.enable().map_err(|error| error.to_string())?;
        } else {
            autolaunch.disable().map_err(|error| error.to_string())?;
        }
    }
    state
        .database
        .save_settings(&settings)
        .map_err(|error| error.to_string())?;
    Ok(settings)
}

#[tauri::command]
pub fn get_storage_info(state: State<'_, AppState>) -> Result<StorageInfo, String> {
    state
        .database
        .storage_info()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn run_retention_cleanup(state: State<'_, AppState>) -> Result<u64, String> {
    let settings = state
        .database
        .load_settings()
        .map_err(|error| error.to_string())?;
    state
        .database
        .cleanup(settings.retention_days)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn backup_database(state: State<'_, AppState>) -> Result<String, String> {
    let storage = state
        .database
        .storage_info()
        .map_err(|error| error.to_string())?;
    let destination = PathBuf::from(storage.data_directory)
        .join(format!("lnpm-backup-{}.sqlite3", unix_time_ms()));
    state
        .database
        .backup_to(&destination)
        .map_err(|error| error.to_string())?;
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
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
