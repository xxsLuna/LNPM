use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AddressFamily {
    Auto,
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QualityThresholds {
    pub window_seconds: u64,
    pub minimum_samples: usize,
    pub packet_loss_percent: f64,
    pub jitter_ms: f64,
    pub p95_latency_ms: f64,
    pub unstable_for_seconds: u64,
    pub stable_for_seconds: u64,
    pub outage_failures: u32,
    pub recovery_successes: u32,
}

impl Default for QualityThresholds {
    fn default() -> Self {
        Self {
            window_seconds: 60,
            minimum_samples: 10,
            packet_loss_percent: 5.0,
            jitter_ms: 30.0,
            p95_latency_ms: 150.0,
            unstable_for_seconds: 10,
            stable_for_seconds: 30,
            outage_failures: 5,
            recovery_successes: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub id: String,
    pub name: String,
    pub host: String,
    pub enabled: bool,
    pub address_family: AddressFamily,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub thresholds: QualityThresholds,
    pub created_at_ms: i64,
    pub archived_at_ms: Option<i64>,
}

impl Target {
    pub fn new(name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            host: host.into(),
            enabled: true,
            address_family: AddressFamily::Auto,
            interval_ms: 1_000,
            timeout_ms: 1_000,
            thresholds: QualityThresholds::default(),
            created_at_ms: unix_time_ms(),
            archived_at_ms: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let host = self.host.trim();
        if host.is_empty() {
            return Err("Host is required".into());
        }
        if host.contains("://") || host.contains('/') || host.contains(' ') {
            return Err("Enter a hostname or IP address without a URL scheme or path".into());
        }
        if !(1_000..=60_000).contains(&self.interval_ms) {
            return Err("Interval must be between 1 and 60 seconds".into());
        }
        if !(250..=10_000).contains(&self.timeout_ms) {
            return Err("Timeout must be between 250 ms and 10 seconds".into());
        }
        if self.timeout_ms > self.interval_ms {
            return Err("Timeout cannot be greater than the sampling interval".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProbeStatus {
    Success,
    Timeout,
    Unreachable,
    DnsError,
    PermissionDenied,
    Error,
}

impl ProbeStatus {
    pub fn is_success(self) -> bool {
        self == Self::Success
    }

    pub fn counts_as_network_failure(self) -> bool {
        matches!(self, Self::Timeout | Self::Unreachable | Self::DnsError)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PingSample {
    pub target_id: String,
    pub timestamp_ms: i64,
    pub latency_ms: Option<f64>,
    pub status: ProbeStatus,
    pub resolved_address: Option<String>,
    pub error: Option<String>,
}

impl PingSample {
    pub fn success(target_id: impl Into<String>, timestamp_ms: i64, latency_ms: f64) -> Self {
        Self {
            target_id: target_id.into(),
            timestamp_ms,
            latency_ms: Some(latency_ms),
            status: ProbeStatus::Success,
            resolved_address: None,
            error: None,
        }
    }

    pub fn failure(target_id: impl Into<String>, timestamp_ms: i64, status: ProbeStatus) -> Self {
        Self {
            target_id: target_id.into(),
            timestamp_ms,
            latency_ms: None,
            status,
            resolved_address: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum QualityState {
    WarmingUp,
    Stable,
    Unstable,
    Disconnected,
    Paused,
    Unobserved,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum QualityReason {
    PacketLoss,
    Jitter,
    HighLatency,
    ConsecutiveFailures,
    Configuration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QualityMetrics {
    pub sample_count: usize,
    pub success_count: usize,
    pub packet_loss_percent: f64,
    pub average_latency_ms: Option<f64>,
    pub minimum_latency_ms: Option<f64>,
    pub maximum_latency_ms: Option<f64>,
    pub p95_latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
}

impl Default for QualityMetrics {
    fn default() -> Self {
        Self {
            sample_count: 0,
            success_count: 0,
            packet_loss_percent: 0.0,
            average_latency_ms: None,
            minimum_latency_ms: None,
            maximum_latency_ms: None,
            p95_latency_ms: None,
            jitter_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StateTransition {
    pub from: QualityState,
    pub to: QualityState,
    pub effective_at_ms: i64,
    pub reasons: Vec<QualityReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationUpdate {
    pub state: QualityState,
    pub state_since_ms: i64,
    pub metrics: QualityMetrics,
    pub reasons: Vec<QualityReason>,
    pub transition: Option<StateTransition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveTargetStatus {
    pub target: Target,
    pub state: QualityState,
    pub state_since_ms: i64,
    pub latest_sample: Option<PingSample>,
    pub metrics: QualityMetrics,
    pub reasons: Vec<QualityReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    pub now_ms: i64,
    pub paused: bool,
    pub targets: Vec<LiveTargetStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub retention_days: Option<u32>,
    pub notifications_enabled: bool,
    pub start_at_login: bool,
    pub language: String,
    pub first_run: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            retention_days: Some(30),
            notifications_enabled: true,
            start_at_login: false,
            language: "auto".into(),
            first_run: true,
        }
    }
}

pub fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_target_input() {
        let mut target = Target::new("Google", "google.com");
        assert!(target.validate().is_ok());

        target.host = "https://google.com/path".into();
        assert!(target.validate().is_err());

        target.host = "1.1.1.1".into();
        target.interval_ms = 1_000;
        target.timeout_ms = 2_000;
        assert!(target.validate().is_err());
    }
}
