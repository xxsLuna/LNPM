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
    i18n::{active_language, message, text},
    storage::Database,
};

const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(7 * 60);
const DEFER_INTERVAL_MS: i64 = 24 * 60 * 60 * 1_000;
/// A download that stops making progress must not hold the update dialog — and with it the quit
/// path — open forever.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long a user-requested check waits out a scheduled one before giving up on it.
const CHECK_COLLISION_ATTEMPTS: u32 = 20;
const CHECK_COLLISION_WAIT: Duration = Duration::from_millis(500);

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

/// What a user-requested check has to say for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckReport {
    Checking,
    UpToDate,
    Failed,
    Busy,
}

impl CheckReport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::UpToDate => "upToDate",
            Self::Failed => "failed",
            Self::Busy => "busy",
        }
    }
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
    /// Guards one download+install run at a time.
    downloading: AtomicBool,
    /// Set only for the few hundred milliseconds around the point of no return, so that quitting
    /// and closing the window stay possible while bytes are still moving.
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
                downloading: AtomicBool::new(false),
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
                        // Keep checking while a version is deferred. Sleeping out the whole
                        // deferral kept the stale pending update and hid any newer release until
                        // the day was over.
                        if duration > CHECK_INTERVAL {
                            self.clear_pending();
                            delay = CHECK_INTERVAL;
                        } else {
                            delay = duration;
                        }
                    }
                    PromptDecision::Prompt => {
                        self.announce_update(&info);
                        // Waking only on a user decision meant that ignoring the dialog stopped
                        // update checking for the rest of the session.
                        let decided = tokio::select! {
                            _ = self.inner.wake.notified() => true,
                            _ = tokio::time::sleep(CHECK_INTERVAL) => false,
                        };
                        // Nobody has answered the dialog yet: look for a newer release, but never
                        // while bytes for the announced one are already being downloaded.
                        if !decided && !self.inner.downloading.load(Ordering::Acquire) {
                            self.check_once().await;
                        }
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
        self.run_check(false).await
    }

    /// `include_skipped` belongs to the caller: the scheduler must honour a skip, while a check the
    /// user asked for has to find the release again so it can be offered.
    async fn run_check(&self, include_skipped: bool) -> CheckOutcome {
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
                if !include_skipped
                    && settings.skipped_update_version.as_deref() == Some(update.version.as_str())
                {
                    return CheckOutcome::NoUpdate;
                }
                let mut pending = self.inner.pending.lock();
                // Take whatever the feed now offers, as long as it is a different release: an
                // identical answer must not churn the version the user is deciding about, but a
                // withdrawn one has to be replaced rather than kept pending forever.
                let supersedes = pending
                    .as_ref()
                    .is_none_or(|current| current.version != update.version);
                if !supersedes {
                    return CheckOutcome::NoUpdate;
                }
                *pending = Some(update);
                CheckOutcome::Found
            }
            Ok(_) => CheckOutcome::NoUpdate,
            Err(error) => {
                eprintln!("Update check failed; retrying later: {error}");
                CheckOutcome::Failed
            }
        }
    }

    /// Runs a check because the user asked for one from the tray, and reports the outcome either
    /// way — a check that says nothing is indistinguishable from a broken menu item.
    ///
    /// Unlike the scheduler, this ignores a deferral or a skip of the version it finds: the user is
    /// asking to see what is available right now, so the stored postponement is cleared and the
    /// update is offered again.
    pub async fn check_manually(&self) {
        // An update is already on its way: checking again would replace the release being installed
        // and reset the dialog out of its progress state. Put that dialog back in front instead.
        if self.inner.downloading.load(Ordering::Acquire)
            || self.inner.installing.load(Ordering::Acquire)
        {
            self.report_check(CheckReport::Busy);
            crate::tray::show_main_window(&self.inner.app);
            return;
        }
        self.report_check(CheckReport::Checking);
        let outcome = self.await_check().await;
        let pending = self.inner.pending.lock().as_ref().map(UpdateInfo::from);
        match (outcome, pending) {
            (_, Some(info)) => {
                let database = self.inner.database.clone();
                let version = info.version.clone();
                let cleared = tauri::async_runtime::spawn_blocking(move || {
                    database.update_settings(|settings| {
                        if settings.update_deferred_version.as_deref() == Some(version.as_str()) {
                            settings.update_deferred_version = None;
                            settings.update_deferred_until_ms = None;
                        }
                        if settings.skipped_update_version.as_deref() == Some(version.as_str()) {
                            settings.skipped_update_version = None;
                        }
                    })
                })
                .await;
                if let Ok(Ok(settings)) = cleared {
                    let _ = self.inner.app.emit("settings-updated", &settings);
                }
                // The window first, so the dialog carries the news instead of a notification
                // duplicating it, and then an announcement that ignores the "already announced"
                // guard — the request came from the user, so silence would look like a dead menu.
                crate::tray::show_main_window(&self.inner.app);
                self.announce_update_forced(&info);
                self.inner.wake.notify_waiters();
            }
            (CheckOutcome::AlreadyRunning, None) | (CheckOutcome::Failed, None) => {
                self.report_check(CheckReport::Failed)
            }
            (_, None) => self.report_check(CheckReport::UpToDate),
        }
    }

    /// Runs a user-requested check, waiting out a scheduled check that happens to hold the guard.
    /// Reporting "already running" to someone who just asked would be no answer at all.
    async fn await_check(&self) -> CheckOutcome {
        for _ in 0..CHECK_COLLISION_ATTEMPTS {
            match self.run_check(true).await {
                CheckOutcome::AlreadyRunning => {
                    tokio::time::sleep(CHECK_COLLISION_WAIT).await;
                }
                outcome => return outcome,
            }
        }
        CheckOutcome::AlreadyRunning
    }

    /// Tells the user how a manual check went, through the window if it is on screen and through a
    /// notification if it is not.
    fn report_check(&self, report: CheckReport) {
        let _ = self.inner.app.emit("update-check", report.as_str());
        if matches!(report, CheckReport::Checking) || self.main_window_is_visible() {
            return;
        }
        let settings = self.inner.database.load_settings().unwrap_or_default();
        let language = active_language(settings.language);
        let body = match report {
            CheckReport::UpToDate => text(language, "update.upToDate"),
            CheckReport::Failed => text(language, "error.updateCheck"),
            CheckReport::Busy => text(language, "error.updateBusy"),
            CheckReport::Checking => return,
        };
        let _ = self
            .inner
            .app
            .notification()
            .builder()
            .title("LNPM")
            .body(body)
            .show();
    }

    fn main_window_is_visible(&self) -> bool {
        self.inner
            .app
            .get_webview_window("main")
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false)
    }

    fn announce_update(&self, info: &UpdateInfo) {
        if !mark_announced(&mut self.inner.announced_version.lock(), &info.version) {
            return;
        }
        self.emit_announcement(info);
    }

    /// Announces a release the user explicitly asked to see, even if it was announced before.
    /// Clearing `announced_version` to force this would briefly disagree with the pending update,
    /// and `install` refuses to run while those two differ.
    fn announce_update_forced(&self, info: &UpdateInfo) {
        *self.inner.announced_version.lock() = Some(info.version.clone());
        self.emit_announcement(info);
    }

    fn emit_announcement(&self, info: &UpdateInfo) {
        let _ = self.inner.app.emit("update-available", info);
        if self.main_window_is_visible() {
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
        let settings = self.inner.database.update_settings(|settings| {
            settings.update_deferred_version = Some(version.to_string());
            settings.update_deferred_until_ms = Some(unix_time_ms() + DEFER_INTERVAL_MS);
        })?;
        *self.inner.announced_version.lock() = None;
        let _ = self.inner.app.emit("settings-updated", &settings);
        self.inner.wake.notify_waiters();
        Ok(settings)
    }

    fn skip(&self, version: &str) -> Result<AppSettings, CommandError> {
        self.require_pending_version(version)?;
        let settings = self.inner.database.update_settings(|settings| {
            settings.skipped_update_version = Some(version.to_string());
            if settings.update_deferred_version.as_deref() == Some(version) {
                settings.update_deferred_version = None;
                settings.update_deferred_until_ms = None;
            }
        })?;
        let _ = self.inner.app.emit("settings-updated", &settings);
        *self.inner.announced_version.lock() = None;
        self.inner.wake.notify_waiters();
        Ok(settings)
    }

    async fn install(&self) -> Result<(), CommandError> {
        let Some(_guard) = ExclusiveTaskGuard::acquire(&self.inner.downloading) else {
            return Err(CommandError::new(
                "updateBusy",
                "an update installation is already running",
            ));
        };
        let Some(update) = self.inner.pending.lock().clone() else {
            return Err(CommandError::new(
                "updateMissing",
                "there is no pending update",
            ));
        };
        // Install exactly the release the dialog offered. A check that lands in between can replace
        // the pending update, and installing that one instead would bypass the user's decision.
        if self.inner.announced_version.lock().as_deref() != Some(update.version.as_str()) {
            return Err(CommandError::new(
                "updateMissing",
                "the pending update changed while the dialog was open",
            ));
        }
        let version = update.version.clone();
        let app = self.inner.app.clone();
        let progress_version = version.clone();
        let verifying_app = app.clone();
        let verifying_version = version.clone();
        let mut downloaded_bytes = 0_u64;

        let download = update.download(
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
        );
        let bytes = match tokio::time::timeout(DOWNLOAD_TIMEOUT, download).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) => {
                let code = if is_signature_error(&error) {
                    "updateSignature"
                } else {
                    "updateDownload"
                };
                self.fail_install(&version, code, &error.to_string());
                return Err(CommandError::new(code, error));
            }
            Err(_) => {
                let detail = "the update download timed out";
                self.fail_install(&version, "updateDownload", detail);
                return Err(CommandError::new("updateDownload", detail));
            }
        };

        self.emit_progress(&version, UpdatePhase::Installing, None, None, None);
        // On Windows the plugin exits the process as soon as the installer is running, so nothing
        // after `install` executes. Close the open quality intervals here or the time until the
        // next launch is later reported as monitored uptime.
        if let Some(state) = self.inner.app.try_state::<crate::commands::AppState>()
            && let Err(error) = state.database.close_open_intervals(unix_time_ms())
        {
            eprintln!("Could not close quality intervals before installing: {error}");
        }
        self.inner.installing.store(true, Ordering::Release);
        if let Err(error) = update.install(&bytes) {
            self.fail_install(&version, "updateInstall", &error.to_string());
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

    fn fail_install(&self, version: &str, code: &str, detail: &str) {
        self.inner.installing.store(false, Ordering::Release);
        let _ = self.inner.app.emit(
            "update-error",
            UpdateFailure {
                version: version.to_string(),
                code: code.to_string(),
                detail: Some(detail.to_string()),
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

// Both write to the database, so they are dispatched off the event loop thread like every other
// command that touches it.
#[tauri::command]
pub async fn defer_update(
    manager: State<'_, UpdateManager>,
    version: String,
) -> Result<AppSettings, CommandError> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.defer(&version))
        .await
        .map_err(|error| CommandError::new("unknown", error))?
}

#[tauri::command]
pub async fn skip_update(
    manager: State<'_, UpdateManager>,
    version: String,
) -> Result<AppSettings, CommandError> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.skip(&version))
        .await
        .map_err(|error| CommandError::new("unknown", error))?
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
        // Clamped to the deferral length: a clock that jumped backwards would otherwise park the
        // updater for as long as the jump.
        let remaining_ms = until_ms.saturating_sub(now_ms).min(DEFER_INTERVAL_MS);
        return PromptDecision::Defer(Duration::from_millis(remaining_ms as u64));
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
    fn a_backwards_clock_jump_cannot_park_the_updater() {
        let settings = AppSettings {
            update_deferred_version: Some("0.3.0".into()),
            // As if the deferral had been recorded with the clock a year ahead.
            update_deferred_until_ms: Some(400 * 86_400_000),
            ..AppSettings::default()
        };
        assert_eq!(
            prompt_decision(&settings, "0.3.0", 0),
            PromptDecision::Defer(Duration::from_millis(DEFER_INTERVAL_MS as u64))
        );
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
