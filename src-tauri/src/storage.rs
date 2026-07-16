use std::{fs, path::PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::domain::{
    AddressFamily, AppSettings, ClassificationUpdate, HistoryPoint, HistoryResponse, HistorySeries,
    PingSample, ProbeStatus, QualityIntervalRecord, QualityState, RangeSummary, StorageInfo,
    Target, unix_time_ms,
};

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("stored data is invalid: {0}")]
    InvalidData(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug, Clone)]
pub struct Database {
    data_directory: PathBuf,
    database_path: PathBuf,
}

impl Database {
    pub fn new(data_directory: PathBuf) -> StorageResult<Self> {
        fs::create_dir_all(&data_directory)?;
        let database_path = data_directory.join("lnpm.sqlite3");
        let database = Self {
            data_directory,
            database_path,
        };
        database.initialize()?;
        Ok(database)
    }

    pub fn initialize(&self) -> StorageResult<()> {
        let connection = self.open()?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_info (
                version INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS targets (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                host TEXT NOT NULL,
                enabled INTEGER NOT NULL,
                address_family INTEGER NOT NULL,
                interval_ms INTEGER NOT NULL,
                timeout_ms INTEGER NOT NULL,
                thresholds_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                archived_at_ms INTEGER
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS ping_samples (
                target_id TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                latency_ms REAL,
                status INTEGER NOT NULL,
                resolved_address TEXT,
                error TEXT,
                PRIMARY KEY (target_id, timestamp_ms)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS quality_intervals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_id TEXT NOT NULL,
                start_ms INTEGER NOT NULL,
                end_ms INTEGER,
                state INTEGER NOT NULL,
                reasons_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_quality_intervals_range
                ON quality_intervals(target_id, start_ms, end_ms);

            CREATE TABLE IF NOT EXISTS minute_rollups (
                target_id TEXT NOT NULL,
                bucket_ms INTEGER NOT NULL,
                sample_count INTEGER NOT NULL,
                success_count INTEGER NOT NULL,
                failure_count INTEGER NOT NULL,
                latency_sum REAL NOT NULL,
                minimum_latency_ms REAL,
                maximum_latency_ms REAL,
                stable_ms INTEGER NOT NULL,
                unstable_ms INTEGER NOT NULL,
                disconnected_ms INTEGER NOT NULL,
                PRIMARY KEY (target_id, bucket_ms)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS app_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                settings_json TEXT NOT NULL
            );
            ",
        )?;

        let current_version: Option<i64> = connection
            .query_row("SELECT version FROM schema_info LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        match current_version {
            None => {
                connection.execute(
                    "INSERT INTO schema_info(version) VALUES (?1)",
                    [SCHEMA_VERSION],
                )?;
            }
            Some(version) if version > SCHEMA_VERSION => {
                return Err(StorageError::InvalidData(format!(
                    "database schema {version} is newer than supported schema {SCHEMA_VERSION}"
                )));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn list_targets(&self, include_archived: bool) -> StorageResult<Vec<Target>> {
        let connection = self.open()?;
        let sql = if include_archived {
            "SELECT id, name, host, enabled, address_family, interval_ms, timeout_ms,
                    thresholds_json, created_at_ms, archived_at_ms
             FROM targets ORDER BY archived_at_ms IS NOT NULL, created_at_ms"
        } else {
            "SELECT id, name, host, enabled, address_family, interval_ms, timeout_ms,
                    thresholds_json, created_at_ms, archived_at_ms
             FROM targets WHERE archived_at_ms IS NULL ORDER BY created_at_ms"
        };
        let mut statement = connection.prepare(sql)?;
        let rows = statement.query_map([], target_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_target(&self, id: &str) -> StorageResult<Option<Target>> {
        let connection = self.open()?;
        connection
            .query_row(
                "SELECT id, name, host, enabled, address_family, interval_ms, timeout_ms,
                        thresholds_json, created_at_ms, archived_at_ms
                 FROM targets WHERE id = ?1",
                [id],
                target_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn save_target(&self, target: &Target) -> StorageResult<()> {
        target.validate().map_err(StorageError::InvalidData)?;
        let connection = self.open()?;
        connection.execute(
            "INSERT INTO targets (
                id, name, host, enabled, address_family, interval_ms, timeout_ms,
                thresholds_json, created_at_ms, archived_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                host = excluded.host,
                enabled = excluded.enabled,
                address_family = excluded.address_family,
                interval_ms = excluded.interval_ms,
                timeout_ms = excluded.timeout_ms,
                thresholds_json = excluded.thresholds_json,
                archived_at_ms = excluded.archived_at_ms",
            params![
                target.id,
                target.name,
                target.host,
                target.enabled as i64,
                address_family_to_i64(target.address_family),
                target.interval_ms as i64,
                target.timeout_ms as i64,
                serde_json::to_string(&target.thresholds)?,
                target.created_at_ms,
                target.archived_at_ms,
            ],
        )?;
        Ok(())
    }

    pub fn archive_target(&self, id: &str, timestamp_ms: i64) -> StorageResult<()> {
        let connection = self.open()?;
        connection.execute(
            "UPDATE targets SET enabled = 0, archived_at_ms = ?2 WHERE id = ?1",
            params![id, timestamp_ms],
        )?;
        connection.execute(
            "UPDATE quality_intervals SET end_ms = ?2
             WHERE target_id = ?1 AND end_ms IS NULL",
            params![id, timestamp_ms],
        )?;
        Ok(())
    }

    pub fn close_open_intervals(&self, timestamp_ms: i64) -> StorageResult<u64> {
        let connection = self.open()?;
        let changed = connection.execute(
            "UPDATE quality_intervals SET end_ms = ?1 WHERE end_ms IS NULL",
            [timestamp_ms],
        )?;
        Ok(changed as u64)
    }

    pub fn load_settings(&self) -> StorageResult<AppSettings> {
        let connection = self.open()?;
        let json = connection
            .query_row(
                "SELECT settings_json FROM app_settings WHERE id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match json {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(AppSettings::default()),
        }
    }

    pub fn save_settings(&self, settings: &AppSettings) -> StorageResult<()> {
        if settings
            .retention_days
            .is_some_and(|days| days == 0 || days > 365)
        {
            return Err(StorageError::InvalidData(
                "retention must be between 1 and 365 days, or unlimited".into(),
            ));
        }
        let connection = self.open()?;
        connection.execute(
            "INSERT INTO app_settings(id, settings_json) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET settings_json = excluded.settings_json",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }

    pub fn write_sample(
        &self,
        sample: &PingSample,
        update: &ClassificationUpdate,
        interval_ms: u64,
    ) -> StorageResult<()> {
        let mut connection = self.open()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR REPLACE INTO ping_samples(
                target_id, timestamp_ms, latency_ms, status, resolved_address, error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                sample.target_id,
                sample.timestamp_ms,
                sample.latency_ms,
                probe_status_to_i64(sample.status),
                sample.resolved_address,
                sample.error,
            ],
        )?;

        if let Some(transition) = &update.transition {
            transaction.execute(
                "UPDATE quality_intervals SET end_ms = ?2
                 WHERE target_id = ?1 AND end_ms IS NULL",
                params![sample.target_id, transition.effective_at_ms],
            )?;
            transaction.execute(
                "INSERT INTO quality_intervals(
                    target_id, start_ms, end_ms, state, reasons_json
                 ) VALUES (?1, ?2, NULL, ?3, ?4)",
                params![
                    sample.target_id,
                    transition.effective_at_ms,
                    quality_state_to_i64(transition.to),
                    serde_json::to_string(&transition.reasons)?,
                ],
            )?;
        }

        let bucket_ms = sample.timestamp_ms.div_euclid(60_000) * 60_000;
        let success = sample.status.is_success() as i64;
        let failure = (!sample.status.is_success()) as i64;
        let (stable_ms, unstable_ms, disconnected_ms) = match update.state {
            QualityState::Stable | QualityState::WarmingUp => (interval_ms as i64, 0, 0),
            QualityState::Unstable => (0, interval_ms as i64, 0),
            QualityState::Disconnected => (0, 0, interval_ms as i64),
            _ => (0, 0, 0),
        };
        transaction.execute(
            "INSERT INTO minute_rollups(
                target_id, bucket_ms, sample_count, success_count, failure_count,
                latency_sum, minimum_latency_ms, maximum_latency_ms,
                stable_ms, unstable_ms, disconnected_ms
             ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9)
             ON CONFLICT(target_id, bucket_ms) DO UPDATE SET
                sample_count = sample_count + 1,
                success_count = success_count + excluded.success_count,
                failure_count = failure_count + excluded.failure_count,
                latency_sum = latency_sum + excluded.latency_sum,
                minimum_latency_ms = CASE
                    WHEN excluded.minimum_latency_ms IS NULL THEN minimum_latency_ms
                    WHEN minimum_latency_ms IS NULL THEN excluded.minimum_latency_ms
                    ELSE MIN(minimum_latency_ms, excluded.minimum_latency_ms)
                END,
                maximum_latency_ms = CASE
                    WHEN excluded.maximum_latency_ms IS NULL THEN maximum_latency_ms
                    WHEN maximum_latency_ms IS NULL THEN excluded.maximum_latency_ms
                    ELSE MAX(maximum_latency_ms, excluded.maximum_latency_ms)
                END,
                stable_ms = stable_ms + excluded.stable_ms,
                unstable_ms = unstable_ms + excluded.unstable_ms,
                disconnected_ms = disconnected_ms + excluded.disconnected_ms",
            params![
                sample.target_id,
                bucket_ms,
                success,
                failure,
                sample.latency_ms.unwrap_or(0.0),
                sample.latency_ms,
                stable_ms,
                unstable_ms,
                disconnected_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn history(
        &self,
        target_ids: &[String],
        from_ms: i64,
        to_ms: i64,
        max_points: usize,
    ) -> StorageResult<HistoryResponse> {
        if to_ms <= from_ms {
            return Err(StorageError::InvalidData(
                "history end must be after start".into(),
            ));
        }
        if !(100..=10_000).contains(&max_points) {
            return Err(StorageError::InvalidData(
                "max points must be between 100 and 10000".into(),
            ));
        }
        let raw_bucket_ms = ((to_ms - from_ms) / max_points as i64).max(1_000);
        let bucket_ms = nice_bucket_size(raw_bucket_ms);
        let mut series = Vec::new();
        for target_id in target_ids {
            let Some(target) = self.get_target(target_id)? else {
                continue;
            };
            let points = if bucket_ms >= 60_000 {
                self.query_rollup_points(target_id, from_ms, to_ms, bucket_ms)?
            } else {
                self.query_raw_points(target_id, from_ms, to_ms, bucket_ms)?
            };
            let intervals = self.query_intervals(target_id, from_ms, to_ms)?;
            let summary = self.query_summary(target_id, from_ms, to_ms, &intervals)?;
            series.push(HistorySeries {
                target,
                points,
                intervals,
                summary,
            });
        }
        Ok(HistoryResponse {
            from_ms,
            to_ms,
            bucket_ms,
            series,
        })
    }

    pub fn cleanup(&self, retention_days: Option<u32>) -> StorageResult<u64> {
        let Some(retention_days) = retention_days else {
            return Ok(0);
        };
        let cutoff_ms = unix_time_ms() - retention_days as i64 * 86_400_000;
        let connection = self.open()?;
        let deleted = connection.execute(
            "DELETE FROM ping_samples WHERE timestamp_ms < ?1",
            [cutoff_ms],
        )?;
        connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE); PRAGMA incremental_vacuum;")?;
        Ok(deleted as u64)
    }

    pub fn storage_info(&self) -> StorageResult<StorageInfo> {
        let database_size_bytes = self.database_path.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(StorageInfo {
            data_directory: self.data_directory.to_string_lossy().to_string(),
            database_path: self.database_path.to_string_lossy().to_string(),
            database_size_bytes,
        })
    }

    pub fn backup_to(&self, destination: &PathBuf) -> StorageResult<()> {
        let source = self.open()?;
        let mut destination_connection = Connection::open(destination)?;
        let backup = rusqlite::backup::Backup::new(&source, &mut destination_connection)?;
        backup.run_to_completion(128, std::time::Duration::from_millis(10), None)?;
        Ok(())
    }

    fn open(&self) -> StorageResult<Connection> {
        let connection = Connection::open(&self.database_path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA auto_vacuum = INCREMENTAL;",
        )?;
        Ok(connection)
    }

    fn query_raw_points(
        &self,
        target_id: &str,
        from_ms: i64,
        to_ms: i64,
        bucket_ms: i64,
    ) -> StorageResult<Vec<HistoryPoint>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "SELECT (timestamp_ms / ?4) * ?4 AS bucket,
                    AVG(latency_ms), MIN(latency_ms), MAX(latency_ms),
                    COUNT(*), SUM(CASE WHEN status = 0 THEN 0 ELSE 1 END)
             FROM ping_samples
             WHERE target_id = ?1 AND timestamp_ms >= ?2 AND timestamp_ms < ?3
             GROUP BY bucket ORDER BY bucket",
        )?;
        let rows = statement.query_map(
            params![target_id, from_ms, to_ms, bucket_ms],
            history_point_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn query_rollup_points(
        &self,
        target_id: &str,
        from_ms: i64,
        to_ms: i64,
        bucket_ms: i64,
    ) -> StorageResult<Vec<HistoryPoint>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "SELECT (bucket_ms / ?4) * ?4 AS bucket,
                    CASE WHEN SUM(success_count) > 0
                         THEN SUM(latency_sum) / SUM(success_count) ELSE NULL END,
                    MIN(minimum_latency_ms), MAX(maximum_latency_ms),
                    SUM(sample_count), SUM(failure_count)
             FROM minute_rollups
             WHERE target_id = ?1 AND bucket_ms >= ?2 AND bucket_ms < ?3
             GROUP BY 1 ORDER BY 1",
        )?;
        let rows = statement.query_map(
            params![target_id, from_ms, to_ms, bucket_ms],
            history_point_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn query_intervals(
        &self,
        target_id: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> StorageResult<Vec<QualityIntervalRecord>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "SELECT start_ms, end_ms, state, reasons_json
             FROM quality_intervals
             WHERE target_id = ?1 AND start_ms < ?3 AND COALESCE(end_ms, ?3) > ?2
             ORDER BY start_ms",
        )?;
        let rows = statement.query_map(params![target_id, from_ms, to_ms], |row| {
            let state_value: i64 = row.get(2)?;
            let reasons_json: String = row.get(3)?;
            Ok(QualityIntervalRecord {
                start_ms: row.get(0)?,
                end_ms: row.get(1)?,
                state: quality_state_from_i64(state_value).map_err(to_sql_error)?,
                reasons: serde_json::from_str(&reasons_json).map_err(to_sql_error)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn query_summary(
        &self,
        target_id: &str,
        from_ms: i64,
        to_ms: i64,
        intervals: &[QualityIntervalRecord],
    ) -> StorageResult<RangeSummary> {
        let connection = self.open()?;
        let mut summary = connection.query_row(
            "SELECT COALESCE(SUM(sample_count), 0),
                    COALESCE(SUM(success_count), 0),
                    COALESCE(SUM(failure_count), 0),
                    SUM(latency_sum), MIN(minimum_latency_ms), MAX(maximum_latency_ms)
             FROM minute_rollups
             WHERE target_id = ?1 AND bucket_ms >= (?2 / 60000) * 60000
               AND bucket_ms < ?3",
            params![target_id, from_ms, to_ms],
            |row| {
                let sample_count: u64 = row.get(0)?;
                let success_count: u64 = row.get(1)?;
                let failure_count: u64 = row.get(2)?;
                let latency_sum: Option<f64> = row.get(3)?;
                Ok(RangeSummary {
                    sample_count,
                    success_count,
                    failure_count,
                    packet_loss_percent: if sample_count == 0 {
                        0.0
                    } else {
                        failure_count as f64 / sample_count as f64 * 100.0
                    },
                    average_latency_ms: latency_sum
                        .filter(|_| success_count > 0)
                        .map(|sum| sum / success_count as f64),
                    minimum_latency_ms: row.get(4)?,
                    maximum_latency_ms: row.get(5)?,
                    p95_latency_ms: None,
                    stable_ms: 0,
                    unstable_ms: 0,
                    disconnected_ms: 0,
                    stable_percent: 0.0,
                    unstable_percent: 0.0,
                    disconnected_percent: 0.0,
                })
            },
        )?;

        for interval in intervals {
            let start = interval.start_ms.max(from_ms);
            let end = interval.end_ms.unwrap_or(to_ms).min(to_ms);
            let duration = end.saturating_sub(start);
            match interval.state {
                QualityState::Stable | QualityState::WarmingUp => summary.stable_ms += duration,
                QualityState::Unstable => summary.unstable_ms += duration,
                QualityState::Disconnected => summary.disconnected_ms += duration,
                _ => {}
            }
        }
        let monitored_ms = summary.stable_ms + summary.unstable_ms + summary.disconnected_ms;
        if monitored_ms > 0 {
            summary.stable_percent = summary.stable_ms as f64 / monitored_ms as f64 * 100.0;
            summary.unstable_percent = summary.unstable_ms as f64 / monitored_ms as f64 * 100.0;
            summary.disconnected_percent =
                summary.disconnected_ms as f64 / monitored_ms as f64 * 100.0;
        }
        summary.p95_latency_ms = approximate_p95(
            self.query_raw_points(target_id, from_ms, to_ms, 60_000)?
                .iter()
                .filter_map(|point| point.maximum_latency_ms)
                .collect(),
        );
        Ok(summary)
    }
}

fn target_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Target> {
    let address_family: i64 = row.get(4)?;
    let thresholds_json: String = row.get(7)?;
    Ok(Target {
        id: row.get(0)?,
        name: row.get(1)?,
        host: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        address_family: address_family_from_i64(address_family).map_err(to_sql_error)?,
        interval_ms: row.get::<_, i64>(5)? as u64,
        timeout_ms: row.get::<_, i64>(6)? as u64,
        thresholds: serde_json::from_str(&thresholds_json).map_err(to_sql_error)?,
        created_at_ms: row.get(8)?,
        archived_at_ms: row.get(9)?,
    })
}

fn history_point_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryPoint> {
    Ok(HistoryPoint {
        timestamp_ms: row.get(0)?,
        average_latency_ms: row.get(1)?,
        minimum_latency_ms: row.get(2)?,
        maximum_latency_ms: row.get(3)?,
        sample_count: row.get::<_, i64>(4)? as u64,
        failure_count: row.get::<_, i64>(5)? as u64,
    })
}

fn nice_bucket_size(raw_ms: i64) -> i64 {
    const BUCKETS: [i64; 16] = [
        1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000,
        1_800_000, 3_600_000, 10_800_000, 21_600_000, 86_400_000,
    ];
    BUCKETS
        .into_iter()
        .find(|bucket| *bucket >= raw_ms)
        .unwrap_or(86_400_000)
}

fn approximate_p95(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let index = (((values.len() as f64) * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    values.get(index).copied()
}

fn address_family_to_i64(value: AddressFamily) -> i64 {
    match value {
        AddressFamily::Auto => 0,
        AddressFamily::Ipv4 => 4,
        AddressFamily::Ipv6 => 6,
    }
}

fn address_family_from_i64(value: i64) -> Result<AddressFamily, StorageError> {
    match value {
        0 => Ok(AddressFamily::Auto),
        4 => Ok(AddressFamily::Ipv4),
        6 => Ok(AddressFamily::Ipv6),
        _ => Err(StorageError::InvalidData(format!(
            "unknown address family {value}"
        ))),
    }
}

fn probe_status_to_i64(value: ProbeStatus) -> i64 {
    match value {
        ProbeStatus::Success => 0,
        ProbeStatus::Timeout => 1,
        ProbeStatus::Unreachable => 2,
        ProbeStatus::DnsError => 3,
        ProbeStatus::PermissionDenied => 4,
        ProbeStatus::Error => 5,
    }
}

fn quality_state_to_i64(value: QualityState) -> i64 {
    match value {
        QualityState::WarmingUp => 0,
        QualityState::Stable => 1,
        QualityState::Unstable => 2,
        QualityState::Disconnected => 3,
        QualityState::Paused => 4,
        QualityState::Unobserved => 5,
        QualityState::Error => 6,
    }
}

fn quality_state_from_i64(value: i64) -> Result<QualityState, StorageError> {
    match value {
        0 => Ok(QualityState::WarmingUp),
        1 => Ok(QualityState::Stable),
        2 => Ok(QualityState::Unstable),
        3 => Ok(QualityState::Disconnected),
        4 => Ok(QualityState::Paused),
        5 => Ok(QualityState::Unobserved),
        6 => Ok(QualityState::Error),
        _ => Err(StorageError::InvalidData(format!(
            "unknown quality state {value}"
        ))),
    }
}

fn to_sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{
        domain::{
            PingSample, ProbeStatus, QualityMetrics, QualityReason, QualityThresholds,
            StateTransition,
        },
        quality::QualityClassifier,
    };

    fn database() -> (tempfile::TempDir, Database) {
        let directory = tempdir().unwrap();
        let database = Database::new(directory.path().to_path_buf()).unwrap();
        (directory, database)
    }

    #[test]
    fn persists_targets_and_settings() {
        let (_directory, database) = database();
        let target = Target::new("Cloudflare", "1.1.1.1");
        database.save_target(&target).unwrap();
        assert_eq!(database.list_targets(false).unwrap(), vec![target]);

        let settings = AppSettings {
            retention_days: Some(90),
            notifications_enabled: false,
            start_at_login: true,
            language: "ko".into(),
            first_run: false,
        };
        database.save_settings(&settings).unwrap();
        assert_eq!(database.load_settings().unwrap(), settings);
    }

    #[test]
    fn stores_samples_rollups_intervals_and_history() {
        let (_directory, database) = database();
        let mut target = Target::new("Cloudflare", "1.1.1.1");
        target.created_at_ms = 0;
        database.save_target(&target).unwrap();
        let mut classifier = QualityClassifier::new(QualityThresholds::default(), 0);

        for second in 0..12 {
            let sample =
                PingSample::success(target.id.clone(), second * 1_000, 20.0 + second as f64);
            let update = classifier.observe(sample.clone());
            database.write_sample(&sample, &update, 1_000).unwrap();
        }

        let history = database
            .history(std::slice::from_ref(&target.id), 0, 60_000, 100)
            .unwrap();
        assert_eq!(history.series.len(), 1);
        assert_eq!(history.series[0].summary.sample_count, 12);
        assert_eq!(history.series[0].summary.failure_count, 0);
        assert!(!history.series[0].points.is_empty());
        assert!(!history.series[0].intervals.is_empty());
    }

    #[test]
    fn records_disconnected_interval() {
        let (_directory, database) = database();
        let mut target = Target::new("Offline", "192.0.2.1");
        target.created_at_ms = 0;
        database.save_target(&target).unwrap();
        let sample = PingSample::failure(target.id.clone(), 5_000, ProbeStatus::Timeout);
        let update = ClassificationUpdate {
            state: QualityState::Disconnected,
            state_since_ms: 1_000,
            metrics: QualityMetrics::default(),
            reasons: vec![QualityReason::ConsecutiveFailures],
            transition: Some(StateTransition {
                from: QualityState::Stable,
                to: QualityState::Disconnected,
                effective_at_ms: 1_000,
                reasons: vec![QualityReason::ConsecutiveFailures],
            }),
        };
        database.write_sample(&sample, &update, 1_000).unwrap();

        let history = database
            .history(std::slice::from_ref(&target.id), 0, 10_000, 100)
            .unwrap();
        assert_eq!(
            history.series[0].intervals[0].state,
            QualityState::Disconnected
        );
    }
}
