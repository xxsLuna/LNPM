use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use parking_lot::{Mutex, RwLock};
use tauri::async_runtime::JoinHandle;
use thiserror::Error;
use tokio::time::MissedTickBehavior;

use crate::{
    domain::{
        DashboardSnapshot, LiveTargetStatus, PingSample, QualityMetrics, QualityState,
        QualityTransitionEvent, Target, TargetValidationError, unix_time_ms,
    },
    probe::PingProbe,
    quality::QualityClassifier,
    storage::{Database, StorageError},
};

#[derive(Debug, Error)]
pub enum MonitorError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Validation(#[from] TargetValidationError),
    #[error("target not found: {0}")]
    TargetNotFound(String),
}

impl MonitorError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Storage(error) => match error {
                StorageError::Database(_) => "storage",
                StorageError::Io(_) => "filesystem",
                StorageError::Json(_) => "serialization",
                StorageError::InvalidData(_) => "invalidData",
            },
            Self::Validation(error) => error.code(),
            Self::TargetNotFound(_) => "targetNotFound",
        }
    }
}

pub trait MonitorEventSink: Send + Sync {
    fn dashboard_updated(&self, snapshot: DashboardSnapshot);
    fn quality_transition(&self, event: QualityTransitionEvent);
    fn monitor_error(&self, target_id: Option<&str>, code: &str, detail: &str);
}

#[derive(Default)]
pub struct NoopEventSink;

impl MonitorEventSink for NoopEventSink {
    fn dashboard_updated(&self, _snapshot: DashboardSnapshot) {}
    fn quality_transition(&self, _event: QualityTransitionEvent) {}
    fn monitor_error(&self, _target_id: Option<&str>, _code: &str, _detail: &str) {}
}

struct TargetRuntime {
    status: LiveTargetStatus,
    classifier: QualityClassifier,
}

pub struct MonitorService {
    database: Database,
    probe: Arc<dyn PingProbe>,
    event_sink: Arc<dyn MonitorEventSink>,
    runtimes: RwLock<HashMap<String, TargetRuntime>>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    /// Serialises the multi-step target mutations. Commands run on worker threads, so two saves of
    /// the same target could otherwise interleave their archive/save/install steps.
    mutations: Mutex<()>,
    paused: AtomicBool,
    shutting_down: AtomicBool,
}

impl MonitorService {
    pub fn new(
        database: Database,
        probe: Arc<dyn PingProbe>,
        event_sink: Arc<dyn MonitorEventSink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            probe,
            event_sink,
            runtimes: RwLock::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            mutations: Mutex::new(()),
            paused: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
        })
    }

    pub fn start_all(self: &Arc<Self>) -> Result<(), MonitorError> {
        let now_ms = unix_time_ms();
        self.database.close_open_intervals(now_ms)?;
        let targets = self.database.list_targets(false)?;
        for target in targets {
            self.install_target(target);
        }
        self.event_sink.dashboard_updated(self.snapshot());
        Ok(())
    }

    pub fn snapshot(&self) -> DashboardSnapshot {
        let mut targets = self
            .runtimes
            .read()
            .values()
            .map(|runtime| runtime.status.clone())
            .collect::<Vec<_>>();
        targets.sort_by_key(|status| status.target.created_at_ms);
        DashboardSnapshot {
            now_ms: unix_time_ms(),
            paused: self.paused.load(Ordering::Relaxed),
            targets,
        }
    }

    pub fn upsert_target(self: &Arc<Self>, mut target: Target) -> Result<Target, MonitorError> {
        target.validate()?;
        let _mutation = self.mutations.lock();
        if let Some(existing) = self.database.get_target(&target.id)? {
            if existing.host != target.host || existing.address_family != target.address_family {
                let now_ms = unix_time_ms();
                self.archive_target_locked(&existing.id)?;
                let mut replacement = Target::new(target.name.clone(), target.host.clone());
                replacement.enabled = target.enabled;
                replacement.address_family = target.address_family;
                replacement.interval_ms = target.interval_ms;
                replacement.timeout_ms = target.timeout_ms;
                replacement.thresholds = target.thresholds.clone();
                replacement.created_at_ms = now_ms;
                target = replacement;
            }
        }

        self.database.save_target(&target)?;
        self.stop_target(&target.id);
        self.install_target(target.clone());
        self.event_sink.dashboard_updated(self.snapshot());
        Ok(target)
    }

    pub fn create_target(
        self: &Arc<Self>,
        name: impl Into<String>,
        host: impl Into<String>,
    ) -> Result<Target, MonitorError> {
        self.upsert_target(Target::new(name, host))
    }

    pub fn archive_target(&self, target_id: &str) -> Result<(), MonitorError> {
        let _mutation = self.mutations.lock();
        self.archive_target_locked(target_id)
    }

    /// Body of [`Self::archive_target`], for callers that already hold the mutation lock.
    fn archive_target_locked(&self, target_id: &str) -> Result<(), MonitorError> {
        if self.database.get_target(target_id)?.is_none() {
            return Err(MonitorError::TargetNotFound(target_id.into()));
        }
        self.stop_target(target_id);
        self.database.archive_target(target_id, unix_time_ms())?;
        self.runtimes.write().remove(target_id);
        self.event_sink.dashboard_updated(self.snapshot());
        Ok(())
    }

    /// Flips the global pause in one step. Reading the current value in the caller and writing it
    /// here would drop a second click that arrives before the first one has been applied.
    pub fn toggle_paused(&self) {
        self.apply_paused(!self.paused.load(Ordering::SeqCst));
    }

    pub fn set_paused(&self, paused: bool) {
        self.apply_paused(paused);
    }

    fn apply_paused(&self, paused: bool) {
        if self.paused.swap(paused, Ordering::SeqCst) == paused {
            return;
        }
        let now_ms = unix_time_ms();
        if paused {
            // Nothing will be observed until monitoring resumes, so the intervals have to be closed
            // here; otherwise the paused time is later reported as uptime.
            if let Err(error) = self.database.close_open_intervals(now_ms) {
                self.event_sink.monitor_error(
                    None,
                    "storage",
                    &format!("failed to close intervals: {error}"),
                );
            }
        }
        let mut transitions = Vec::new();
        {
            let mut runtimes = self.runtimes.write();
            for runtime in runtimes.values_mut() {
                // A disabled target is not observed even while monitoring is running, so resuming
                // must leave it paused instead of warming up forever.
                let paused = paused || !runtime.status.target.enabled;
                if let Some(transition) = runtime.classifier.set_paused(paused, now_ms) {
                    runtime.status.state = transition.to;
                    runtime.status.state_since_ms = transition.effective_at_ms;
                    runtime.status.metrics = QualityMetrics::default();
                    runtime.status.reasons.clear();
                    transitions.push(QualityTransitionEvent {
                        target: runtime.status.target.clone(),
                        transition,
                        metrics: QualityMetrics::default(),
                    });
                }
            }
        }
        for event in transitions {
            self.event_sink.quality_transition(event);
        }
        self.event_sink.dashboard_updated(self.snapshot());
    }

    pub async fn test_probe(&self, target: &Target) -> PingSample {
        self.probe.probe(target).await
    }

    pub fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }
        for (_, task) in self.tasks.lock().drain() {
            task.abort();
        }
        if let Err(error) = self.database.close_open_intervals(unix_time_ms()) {
            self.event_sink.monitor_error(
                None,
                "storage",
                &format!("failed to close intervals: {error}"),
            );
        }
    }

    fn install_target(self: &Arc<Self>, target: Target) {
        let state = if self.paused.load(Ordering::Relaxed) {
            QualityState::Paused
        } else if target.enabled {
            QualityState::WarmingUp
        } else {
            QualityState::Paused
        };
        if !target.enabled {
            // Stop attributing time to the last observed state while the target is switched off.
            if let Err(error) = self
                .database
                .close_open_intervals_for(&target.id, unix_time_ms())
            {
                self.event_sink.monitor_error(
                    Some(&target.id),
                    "storage",
                    &format!("failed to close intervals: {error}"),
                );
            }
        }
        self.runtimes.write().insert(
            target.id.clone(),
            TargetRuntime {
                classifier: QualityClassifier::new(
                    target.thresholds.clone(),
                    target.interval_ms,
                    unix_time_ms(),
                ),
                status: LiveTargetStatus {
                    target: target.clone(),
                    state,
                    state_since_ms: unix_time_ms(),
                    latest_sample: None,
                    metrics: QualityMetrics::default(),
                    reasons: Vec::new(),
                },
            },
        );
        if target.enabled {
            self.spawn_target(target.id);
        }
    }

    fn spawn_target(self: &Arc<Self>, target_id: String) {
        let weak = Arc::downgrade(self);
        let task_target_id = target_id.clone();
        let task = tauri::async_runtime::spawn(async move {
            run_target_loop(weak, task_target_id).await;
        });
        // Dropping a JoinHandle detaches the task instead of cancelling it, so a displaced loop has
        // to be aborted explicitly or the target would be probed twice.
        if let Some(previous) = self.tasks.lock().insert(target_id, task) {
            previous.abort();
        }
    }

    fn stop_target(&self, target_id: &str) {
        if let Some(task) = self.tasks.lock().remove(target_id) {
            task.abort();
        }
    }

    async fn perform_probe(&self, target_id: &str) -> Result<(), MonitorError> {
        let target = self
            .runtimes
            .read()
            .get(target_id)
            .map(|runtime| runtime.status.target.clone())
            .ok_or_else(|| MonitorError::TargetNotFound(target_id.into()))?;
        let sample = self.probe.probe(&target).await;

        let (update, snapshot) = {
            let mut runtimes = self.runtimes.write();
            let runtime = runtimes
                .get_mut(target_id)
                .ok_or_else(|| MonitorError::TargetNotFound(target_id.into()))?;
            let update = runtime.classifier.observe(sample.clone());
            runtime.status.state = update.state;
            runtime.status.state_since_ms = update.state_since_ms;
            runtime.status.latest_sample = Some(sample.clone());
            runtime.status.metrics = update.metrics.clone();
            runtime.status.reasons = update.reasons.clone();
            (update, self.snapshot_unlocked(&runtimes))
        };

        let database = self.database.clone();
        let persisted_sample = sample.clone();
        let persisted_update = update.clone();
        let interval_ms = target.interval_ms;
        let write_result = tauri::async_runtime::spawn_blocking(move || {
            database.write_sample(&persisted_sample, &persisted_update, interval_ms)
        })
        .await;

        // The live dashboard and a state change the user needs to see must not be lost just because
        // this one sample could not be persisted; the write error is still reported below.
        self.event_sink.dashboard_updated(snapshot);
        if let Some(transition) = update.transition {
            self.event_sink.quality_transition(QualityTransitionEvent {
                target,
                transition,
                metrics: update.metrics,
            });
        }

        match write_result {
            Ok(result) => result?,
            Err(error) => return Err(StorageError::InvalidData(error.to_string()).into()),
        }
        Ok(())
    }

    fn snapshot_unlocked(&self, runtimes: &HashMap<String, TargetRuntime>) -> DashboardSnapshot {
        let mut targets = runtimes
            .values()
            .map(|runtime| runtime.status.clone())
            .collect::<Vec<_>>();
        targets.sort_by_key(|status| status.target.created_at_ms);
        DashboardSnapshot {
            now_ms: unix_time_ms(),
            paused: self.paused.load(Ordering::Relaxed),
            targets,
        }
    }
}

async fn run_target_loop(service: Weak<MonitorService>, target_id: String) {
    let Some(initial_service) = service.upgrade() else {
        return;
    };
    let interval_ms = initial_service
        .runtimes
        .read()
        .get(&target_id)
        .map(|runtime| runtime.status.target.interval_ms)
        .unwrap_or(1_000);
    drop(initial_service);

    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let Some(service) = service.upgrade() else {
            return;
        };
        if service.shutting_down.load(Ordering::Relaxed) {
            return;
        }
        if service.paused.load(Ordering::Relaxed) {
            continue;
        }
        if let Err(error) = service.perform_probe(&target_id).await {
            service
                .event_sink
                .monitor_error(Some(&target_id), error.code(), &error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use tempfile::tempdir;

    use super::*;
    use crate::domain::{PingSample, Target};

    struct FakeProbe;

    #[async_trait]
    impl PingProbe for FakeProbe {
        async fn probe(&self, target: &Target) -> PingSample {
            PingSample::success(target.id.clone(), unix_time_ms(), 12.0)
        }
    }

    #[tokio::test]
    async fn manages_targets_and_global_pause() {
        let directory = tempdir().unwrap();
        let database = Database::new(directory.path().to_path_buf()).unwrap();
        let monitor = MonitorService::new(database, Arc::new(FakeProbe), Arc::new(NoopEventSink));
        monitor.start_all().unwrap();
        let target = monitor.create_target("Loopback", "127.0.0.1").unwrap();
        assert_eq!(monitor.snapshot().targets.len(), 1);

        monitor.set_paused(true);
        assert!(monitor.snapshot().paused);
        assert_eq!(monitor.snapshot().targets[0].state, QualityState::Paused);

        monitor.archive_target(&target.id).unwrap();
        assert!(monitor.snapshot().targets.is_empty());
        monitor.shutdown();
    }
}
