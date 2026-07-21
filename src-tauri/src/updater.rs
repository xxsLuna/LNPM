use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use parking_lot::Mutex;
use semver::Version;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Error as TauriUpdaterError, Update, UpdaterExt};
use tokio::sync::Notify;

use crate::{
    commands::CommandError,
    domain::{AppSettings, unix_time_ms},
    i18n::{active_language, message},
    storage::Database,
};

const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(7 * 60);
const DEFER_INTERVAL_MS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

impl From<&Update> for UpdateInfo {
    fn from(update: &Update) -> Self {
        Self {
            version: update.version.clone(),
            notes: update.body.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UpdatePhase {
    Downloading,
    Verifying,
    Installing,
    Restarting,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub version: String,
    pub status: UpdatePhase,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateFailure {
    version: String,
    code: String,
    detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptDecision {
    Prompt,
    Defer(Duration),
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckOutcome {
    Found,
    NoUpdate,
    Failed,
    AlreadyRunning,
}

#[derive(Clone)]
pub struct UpdateManager {
    inner: Arc<UpdateManagerInner>,
}

struct UpdateManagerInner {
    app: AppHandle,
    database: Database,
    pending: Mutex<Option<Update>>,
    announced_version: Mutex<Option<String>>,
    checking: AtomicBool,
    installing: AtomicBool,
    wake: Notify,
}

impl UpdateManager {
    pub fn new(app: AppHandle, database: Database) -> Self {
        Self {
            inner: Arc::new(UpdateManagerInner {
                app,
                database,
                pending: Mutex::new(None),
                announced_version: Mutex::new(None),
                checking: AtomicBool::new(false),
                installing: AtomicBool::new(false),
                wake: Notify::new(),
            }),
        }
    }

    pub fn start(&self) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.run_scheduler().await;
        });
    }

    pub fn is_installing(&self) -> bool {
        self.inner.installing.load(Ordering::Acquire)
    }

    fn pending_info(&self) -> Option<UpdateInfo> {
        let info = self.inner.pending.lock().as_ref().map(UpdateInfo::from)?;
        let settings = self.inner.database.load_settings().ok()?;
        matches!(
            prompt_decision(&settings, &info.version, unix_time_ms()),
            PromptDecision::Prompt
        )
        .then_some(info)
    }

    async fn run_scheduler(&self) {
        let mut delay = Duration::ZERO;
        loop {
            if !delay.is_zero() {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = self.inner.wake.notified() => {}
                }
            }

            let pending_info = self.inner.pending.lock().as_ref().map(UpdateInfo::from);
            if let Some(info) = pending_info {
                let settings = self.inner.database.load_settings().unwrap_or_default();
                match prompt_decision(&settings, &info.version, unix_time_ms()) {
                    PromptDecision::Skip => {
                        self.clear_pending();
                        delay = CHECK_INTERVAL;
                    }
                    PromptDecision::Defer(duration) => {
                        delay = duration;
                    }
                    PromptDecision::Prompt => {
                        self.announce_update(&info);
                        self.inner.wake.notified().await;
                        delay = Duration::ZERO;
                    }
                }
                continue;
            }

            delay = match self.check_once().await {
                CheckOutcome::Found => Duration::ZERO,
                CheckOutcome::AlreadyRunning => RETRY_INTERVAL,
                CheckOutcome::NoUpdate => CHECK_INTERVAL,
                CheckOutcome::Failed => RETRY_INTERVAL,
            };
        }
    }

    async fn check_once(&self) -> CheckOutcome {
        let Some(_guard) = ExclusiveTaskGuard::acquire(&self.inner.checking) else {
            return CheckOutcome::AlreadyRunning;
        };
        let result = match self.inner.app.updater() {
            Ok(updater) => updater.check().await,
            Err(error) => {
                eprintln!("Updater initialization failed: {error}");
                return CheckOutcome::Failed;
            }
        };
        match result {
            Ok(Some(update)) if is_newer_version(&update.version, &update.current_version) => {
                let settings = self.inner.database.load_settings().unwrap_or_default();
                if settings.skipped_update_version.as_deref() == Some(update.version.as_str()) {
                    return CheckOutcome::NoUpdate;
                }
                *self.inner.pending.lock() = Some(update);
                CheckOutcome::Found
            }
            Ok(_) => CheckOutcome::NoUpdate,
            Err(error) => {
                eprintln!("Update check failed; retrying later: {error}");
                CheckOutcome::Failed
            }
        }
    }

    fn announce_update(&self, info: &UpdateInfo) {
        if !mark_announced(&mut self.inner.announced_version.lock(), &info.version) {
            return;
        }
        let _ = self.inner.app.emit("update-available", info);
        let main_is_visible = self
            .inner
            .app
            .get_webview_window("main")
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        if main_is_visible {
            return;
        }
        let settings = self.inner.database.load_settings().unwrap_or_default();
        let language = active_language(settings.language);
        let body = message(
            language,
            "notification.updateAvailable",
            &[("version", &info.version)],
        );
        let _ = self
            .inner
            .app
            .notification()
            .builder()
            .title("LNPM")
            .body(body)
            .show();
    }

    fn defer(&self, version: &str) -> Result<AppSettings, CommandError> {
        self.require_pending_version(version)?;
        let mut settings = self.inner.database.load_settings()?;
        settings.update_deferred_version = Some(version.to_string());
        settings.update_deferred_until_ms = Some(unix_time_ms() + DEFER_INTERVAL_MS);
        self.inner.database.save_settings(&settings)?;
        *self.inner.announced_version.lock() = None;
        let _ = self.inner.app.emit("settings-updated", &settings);
        self.inner.wake.notify_waiters();
        Ok(settings)
    }

    fn skip(&self, version: &str) -> Result<AppSettings, CommandError> {
        self.require_pending_version(version)?;
        let mut settings = self.inner.database.load_settings()?;
        settings.skipped_update_version = Some(version.to_string());
        if settings.update_deferred_version.as_deref() == Some(version) {
            settings.update_deferred_version = None;
            settings.update_deferred_until_ms = None;
        }
        self.inner.database.save_settings(&settings)?;
        let _ = self.inner.app.emit("settings-updated", &settings);
        *self.inner.announced_version.lock() = None;
        self.inner.wake.notify_waiters();
        Ok(settings)
    }

    async fn install(&self) -> Result<(), CommandError> {
        if self
            .inner
            .installing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(CommandError::new(
                "updateBusy",
                "an update installation is already running",
            ));
        }
        let Some(update) = self.inner.pending.lock().clone() else {
            self.inner.installing.store(false, Ordering::Release);
            return Err(CommandError::new(
                "updateMissing",
                "there is no pending update",
            ));
        };
        let version = update.version.clone();
        let app = self.inner.app.clone();
        let progress_version = version.clone();
        let verifying_app = app.clone();
        let verifying_version = version.clone();
        let mut downloaded_bytes = 0_u64;

        let bytes = update
            .download(
                move |chunk_length, content_length| {
                    downloaded_bytes = downloaded_bytes.saturating_add(chunk_length as u64);
                    let percent = content_length
                        .filter(|total| *total > 0)
                        .map(|total| downloaded_bytes as f64 / total as f64 * 100.0);
                    let _ = app.emit(
                        "update-progress",
                        UpdateProgress {
                            version: progress_version.clone(),
                            status: UpdatePhase::Downloading,
                            downloaded_bytes: Some(downloaded_bytes),
                            total_bytes: content_length,
                            percent,
                        },
                    );
                },
                move || {
                    let _ = verifying_app.emit(
                        "update-progress",
                        UpdateProgress {
                            version: verifying_version,
                            status: UpdatePhase::Verifying,
                            downloaded_bytes: None,
                            total_bytes: None,
                            percent: None,
                        },
                    );
                },
            )
            .await;
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                let code = if is_signature_error(&error) {
                    "updateSignature"
                } else {
                    "updateDownload"
                };
                self.fail_install(&version, code, &error);
                return Err(CommandError::new(code, error));
            }
        };

        self.emit_progress(&version, UpdatePhase::Installing, None, None, None);
        if let Err(error) = update.install(&bytes) {
            self.fail_install(&version, "updateInstall", &error);
            return Err(CommandError::new("updateInstall", error));
        }

        self.clear_pending();
        #[cfg(not(target_os = "windows"))]
        {
            self.emit_progress(&version, UpdatePhase::Restarting, None, None, None);
            self.inner.app.restart();
        }
        #[cfg(target_os = "windows")]
        Ok(())
    }

    fn fail_install(&self, version: &str, code: &str, error: &TauriUpdaterError) {
        self.inner.installing.store(false, Ordering::Release);
        let _ = self.inner.app.emit(
            "update-error",
            UpdateFailure {
                version: version.to_string(),
                code: code.to_string(),
                detail: Some(error.to_string()),
            },
        );
    }

    fn emit_progress(
        &self,
        version: &str,
        status: UpdatePhase,
        downloaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
        percent: Option<f64>,
    ) {
        let _ = self.inner.app.emit(
            "update-progress",
            UpdateProgress {
                version: version.to_string(),
                status,
                downloaded_bytes,
                total_bytes,
                percent,
            },
        );
    }

    fn require_pending_version(&self, version: &str) -> Result<(), CommandError> {
        let matches = self
            .inner
            .pending
            .lock()
            .as_ref()
            .is_some_and(|update| update.version == version);
        if matches {
            Ok(())
        } else {
            Err(CommandError::new(
                "updateMissing",
                "the requested update is no longer pending",
            ))
        }
    }

    fn clear_pending(&self) {
        *self.inner.pending.lock() = None;
        *self.inner.announced_version.lock() = None;
    }
}

#[tauri::command]
pub fn get_pending_update(manager: State<'_, UpdateManager>) -> Option<UpdateInfo> {
    manager.pending_info()
}

#[tauri::command]
pub fn defer_update(
    manager: State<'_, UpdateManager>,
    version: String,
) -> Result<AppSettings, CommandError> {
    manager.defer(&version)
}

#[tauri::command]
pub fn skip_update(
    manager: State<'_, UpdateManager>,
    version: String,
) -> Result<AppSettings, CommandError> {
    manager.skip(&version)
}

#[tauri::command]
pub async fn install_update(manager: State<'_, UpdateManager>) -> Result<(), CommandError> {
    manager.install().await
}

fn prompt_decision(settings: &AppSettings, version: &str, now_ms: i64) -> PromptDecision {
    if settings.skipped_update_version.as_deref() == Some(version) {
        return PromptDecision::Skip;
    }
    if settings.update_deferred_version.as_deref() == Some(version)
        && let Some(until_ms) = settings.update_deferred_until_ms
        && until_ms > now_ms
    {
        return PromptDecision::Defer(
            Duration::from_millis(until_ms.saturating_sub(now_ms) as u64),
        );
    }
    PromptDecision::Prompt
}

fn is_newer_version(candidate: &str, current: &str) -> bool {
    let candidate = Version::parse(candidate.trim_start_matches('v'));
    let current = Version::parse(current.trim_start_matches('v'));
    matches!((candidate, current), (Ok(candidate), Ok(current)) if candidate > current)
}

fn mark_announced(announced_version: &mut Option<String>, version: &str) -> bool {
    if announced_version.as_deref() == Some(version) {
        return false;
    }
    *announced_version = Some(version.to_string());
    true
}

fn is_signature_error(error: &TauriUpdaterError) -> bool {
    matches!(
        error,
        TauriUpdaterError::Minisign(_)
            | TauriUpdaterError::Base64(_)
            | TauriUpdaterError::SignatureUtf8(_)
            | TauriUpdaterError::AuthenticationFailed
    )
}

struct ExclusiveTaskGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> ExclusiveTaskGuard<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { flag })
    }
}

impl Drop for ExclusiveTaskGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_thirty_minute_checks_and_seven_minute_retries() {
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(1_800));
        assert_eq!(RETRY_INTERVAL, Duration::from_secs(420));
    }

    #[test]
    fn prevents_duplicate_checks() {
        let running = AtomicBool::new(false);
        let first = ExclusiveTaskGuard::acquire(&running).unwrap();
        assert!(ExclusiveTaskGuard::acquire(&running).is_none());
        drop(first);
        assert!(ExclusiveTaskGuard::acquire(&running).is_some());
    }

    #[test]
    fn compares_new_equal_and_older_semantic_versions() {
        assert!(is_newer_version("v0.3.0", "0.2.0"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(!is_newer_version("0.1.9", "0.2.0"));
    }

    #[test]
    fn honors_later_and_skipped_versions() {
        let mut settings = AppSettings {
            update_deferred_version: Some("0.3.0".into()),
            update_deferred_until_ms: Some(1_000_000),
            ..AppSettings::default()
        };
        assert_eq!(
            prompt_decision(&settings, "0.3.0", 500_000),
            PromptDecision::Defer(Duration::from_millis(500_000))
        );
        assert_eq!(
            prompt_decision(&settings, "0.3.0", 1_000_000),
            PromptDecision::Prompt
        );
        settings.skipped_update_version = Some("0.3.0".into());
        assert_eq!(
            prompt_decision(&settings, "0.3.0", 1_000_000),
            PromptDecision::Skip
        );
        assert_eq!(
            prompt_decision(&settings, "0.4.0", 1_000_000),
            PromptDecision::Prompt
        );
    }

    #[test]
    fn announces_each_discovered_version_once() {
        let mut announced = None;
        assert!(mark_announced(&mut announced, "0.3.0"));
        assert!(!mark_announced(&mut announced, "0.3.0"));
        assert!(mark_announced(&mut announced, "0.4.0"));
    }

    #[test]
    fn serializes_update_events_and_progress_states() {
        let info = UpdateInfo {
            version: "0.3.0".into(),
            notes: Some("Signed update".into()),
        };
        assert_eq!(
            serde_json::to_value(info).unwrap(),
            serde_json::json!({"version": "0.3.0", "notes": "Signed update"})
        );
        let progress = UpdateProgress {
            version: "0.3.0".into(),
            status: UpdatePhase::Downloading,
            downloaded_bytes: Some(50),
            total_bytes: Some(100),
            percent: Some(50.0),
        };
        assert_eq!(
            serde_json::to_value(progress).unwrap()["status"],
            "downloading"
        );
    }

    #[test]
    fn release_configuration_requires_signed_updater_assets() {
        let config = include_str!("../tauri.conf.json");
        assert!(config.contains("\"createUpdaterArtifacts\": true"));
        assert!(config.contains("releases/latest/download/latest.json"));
        assert!(!config.contains("\"pubkey\": \"\""));

        let workflow = include_str!("../../.github/workflows/release.yml");
        for required in [
            "TAURI_SIGNING_PRIVATE_KEY",
            "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
            "uploadUpdaterJson: true",
            "uploadUpdaterSignatures: true",
            "updaterJsonPreferNsis: true",
            "Verify installers and updater metadata",
        ] {
            assert!(workflow.contains(required), "missing {required}");
        }
    }
}
