use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::domain::{
    AddressFamily, AppSettings, ClassificationUpdate, HistoryPoint, HistoryResponse, HistorySeries,
    PingSample, ProbeStatus, QualityIntervalRecord, QualityState, RangeSummary, StorageInfo,
    Target, unix_time_ms,
};

const SCHEMA_VERSION: i64 = 1;
/// Retention deletes are issued one time slice at a time so a single statement can never hold the
/// write lock for long.
const CLEANUP_SLICE_MS: i64 = 6 * 3_600_000;
const AUTO_VACUUM_INCREMENTAL: i64 = 2;
/// Rewriting the whole file is only worth its write lock once this much space can come back.
const COMPACT_THRESHOLD_BYTES: i64 = 32 * 1_024 * 1_024;
/// Pages returned per incremental-vacuum statement, so each one is a short transaction.
const VACUUM_SLICE_PAGES: u32 = 512;

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
    /// One connection stays open for as long as the database is in use. SQLite checkpoints and
    /// truncates the write-ahead log whenever the *last* connection closes, so without this every
    /// probe write — five a second — paid for a checkpoint of the whole database.
    _keeper: Arc<Mutex<Connection>>,
    /// Serialises read-modify-write cycles over the single settings row.
    settings_lock: Arc<Mutex<()>>,
}

impl Database {
    pub fn new(data_directory: PathBuf) -> StorageResult<Self> {
        fs::create_dir_all(&data_directory)?;
        let database_path = data_directory.join("lnpm.sqlite3");
        let database = Self {
            _keeper: Arc::new(Mutex::new(open_connection(&database_path)?)),
            settings_lock: Arc::new(Mutex::new(())),
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
        target
            .validate()
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
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

    /// Closes intervals a previous session left open, clamped to the last sample that session
    /// actually recorded. Stamping `end_ms = now` instead would attribute every minute the process
    /// was not running to whatever state the target was in when it stopped.
    pub fn close_open_intervals(&self, timestamp_ms: i64) -> StorageResult<u64> {
        let connection = self.open()?;
        let changed = connection.execute(
            "UPDATE quality_intervals
             SET end_ms = MAX(start_ms, MIN(?1, COALESCE(
                 (SELECT MAX(timestamp_ms) FROM ping_samples
                  WHERE target_id = quality_intervals.target_id), ?1)))
             WHERE end_ms IS NULL",
            [timestamp_ms],
        )?;
        Ok(changed as u64)
    }

    /// Closes the open interval of a single target, used when it stops being observed (disabled or
    /// paused) while the process keeps running.
    pub fn close_open_intervals_for(
        &self,
        target_id: &str,
        timestamp_ms: i64,
    ) -> StorageResult<()> {
        let connection = self.open()?;
        connection.execute(
            "UPDATE quality_intervals SET end_ms = MAX(start_ms, ?2)
             WHERE target_id = ?1 AND end_ms IS NULL",
            params![target_id, timestamp_ms],
        )?;
        Ok(())
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

    /// Reads, changes and writes the settings as one step. Several places mutate different parts of
    /// the same record — the settings dialog, the updater's defer and skip — and each of them used to
    /// load and save independently, so whichever wrote last silently discarded the other's change.
    pub fn update_settings(
        &self,
        mutate: impl FnOnce(&mut AppSettings),
    ) -> StorageResult<AppSettings> {
        let _guard = self.settings_lock.lock();
        let mut settings = self.load_settings()?;
        mutate(&mut settings);
        self.save_settings(&settings)?;
        Ok(settings)
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
            let summary = self.query_summary(target_id, from_ms, to_ms, bucket_ms, &intervals)?;
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
        // Read the ids from `targets` (a handful of rows, archived ones included). Asking
        // `ping_samples` for its distinct ids would scan the entire samples table on every pass.
        let target_ids = {
            let mut statement = connection.prepare("SELECT id FROM targets")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let mut deleted = 0_u64;
        for target_id in target_ids {
            let oldest_ms: Option<i64> = connection.query_row(
                "SELECT MIN(timestamp_ms) FROM ping_samples WHERE target_id = ?1",
                [&target_id],
                |row| row.get(0),
            )?;
            let Some(oldest_ms) = oldest_ms else {
                continue;
            };
            // One `DELETE` for millions of rows holds the write lock for seconds and makes every
            // concurrent probe wait; deleting per target in time slices keeps each statement short
            // and lets it use the `(target_id, timestamp_ms)` primary key instead of scanning.
            let mut slice_start_ms = oldest_ms;
            while slice_start_ms < cutoff_ms {
                let slice_end_ms = slice_start_ms
                    .saturating_add(CLEANUP_SLICE_MS)
                    .min(cutoff_ms);
                deleted += connection.execute(
                    "DELETE FROM ping_samples
                     WHERE target_id = ?1 AND timestamp_ms >= ?2 AND timestamp_ms < ?3",
                    params![target_id, slice_start_ms, slice_end_ms],
                )? as u64;
                slice_start_ms = slice_end_ms;
            }
        }

        // Rollups and closed intervals used to be kept forever, so the database still grew without
        // bound after retention had pruned the raw samples they summarise. The bound is aligned to
        // the bucket grid so the minute straddling the cutoff — whose newer samples are retained —
        // keeps its rollup.
        connection.execute(
            "DELETE FROM minute_rollups WHERE bucket_ms < (?1 / 60000) * 60000",
            [cutoff_ms],
        )?;
        connection.execute(
            "DELETE FROM quality_intervals WHERE end_ms IS NOT NULL AND end_ms < ?1",
            [cutoff_ms],
        )?;

        if auto_vacuum_mode(&connection)? == AUTO_VACUUM_INCREMENTAL {
            // The pragma reports one row per freed page, so a statement stepped once frees exactly
            // one page — it has to be drained. An unbounded drain is one long write transaction
            // though, so it is asked for a slice at a time, like the deletes above.
            if let Err(error) = reclaim_free_pages(&connection) {
                eprintln!("LNPM could not return free pages to the file system: {error}");
            }
        }
        // After the vacuum, so the pages it freed are written back to the file rather than left in
        // the log. Failures here do not undo the deletes that have already been committed.
        if let Err(error) = connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE);") {
            eprintln!("LNPM could not checkpoint the write-ahead log: {error}");
        }
        Ok(deleted)
    }

    /// Rewrites the database to give the freed pages back to the file system, and switches a
    /// database created before `auto_vacuum` was stamped correctly over to incremental vacuuming so
    /// that later retention passes can reclaim space on their own. Returns whether it ran.
    ///
    /// `VACUUM` holds the write lock for as long as it takes to rewrite the file, so it only runs
    /// when there is a worthwhile amount of free space to reclaim.
    pub fn compact(&self) -> StorageResult<bool> {
        let connection = self.open()?;
        let incremental = auto_vacuum_mode(&connection)? == AUTO_VACUUM_INCREMENTAL;
        let free_pages: i64 =
            connection.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
        let page_bytes: i64 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        // Rewriting the file locks out every writer for as long as it takes, so it has to be worth
        // it — whatever the vacuum mode. A database still stamped NONE is converted by the same
        // rewrite, which is the only way it can ever reclaim space by itself.
        if free_pages * page_bytes < COMPACT_THRESHOLD_BYTES {
            return Ok(false);
        }
        if !incremental {
            connection.execute_batch("PRAGMA auto_vacuum = INCREMENTAL;")?;
        }
        connection.execute_batch("VACUUM;")?;
        // VACUUM writes the whole database through the log, so without this the freed space would
        // reappear as an equally large log file. A checkpoint reports a lost race in its result row
        // rather than as an error.
        let busy: i64 =
            connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        if busy != 0 {
            eprintln!("the write-ahead log could not be truncated right after compaction");
        }
        Ok(true)
    }

    pub fn storage_info(&self) -> StorageResult<StorageInfo> {
        let sidecar_bytes = |suffix: &str| {
            PathBuf::from(format!("{}{suffix}", self.database_path.display()))
                .metadata()
                .map(|meta| meta.len())
                .unwrap_or(0)
        };
        // The write-ahead log holds data that belongs to the database but not to its main file. A
        // main file that cannot be measured is an error, not a database of zero bytes.
        let database_size_bytes = self.database_path.metadata()?.len() + sidecar_bytes("-wal");
        Ok(StorageInfo {
            data_directory: self.data_directory.to_string_lossy().to_string(),
            database_path: self.database_path.to_string_lossy().to_string(),
            database_size_bytes,
        })
    }

    /// `VACUUM INTO` copies a consistent snapshot in a single pass. The incremental
    /// `rusqlite::backup` API restarts from the first page whenever another connection writes the
    /// source database, so with a probe writing every second it never reached completion.
    pub fn backup_to(&self, destination: &Path) -> StorageResult<()> {
        let connection = self.open()?;
        connection.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])?;
        Ok(())
    }

    fn open(&self) -> StorageResult<Connection> {
        open_connection(&self.database_path)
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
        bucket_ms: i64,
        intervals: &[QualityIntervalRecord],
    ) -> StorageResult<RangeSummary> {
        let connection = self.open()?;
        let from_raw_samples = bucket_ms < 60_000;
        // Minute rollups can only answer for whole minutes, so a short range summarised from them
        // silently counted up to 59 s of samples from before the range and disagreed with the
        // chart. Whenever the chart itself is drawn from raw samples, summarise the same rows.
        let statement = if from_raw_samples {
            "SELECT COUNT(*),
                    COALESCE(SUM(status = 0), 0),
                    COALESCE(SUM(status <> 0), 0),
                    SUM(latency_ms), MIN(latency_ms), MAX(latency_ms)
             FROM ping_samples
             WHERE target_id = ?1 AND timestamp_ms >= ?2 AND timestamp_ms < ?3"
        } else {
            "SELECT COALESCE(SUM(sample_count), 0),
                    COALESCE(SUM(success_count), 0),
                    COALESCE(SUM(failure_count), 0),
                    SUM(latency_sum), MIN(minimum_latency_ms), MAX(maximum_latency_ms)
             FROM minute_rollups
             WHERE target_id = ?1 AND bucket_ms >= ?2 AND bucket_ms < ?3"
        };
        let mut summary =
            connection.query_row(statement, params![target_id, from_ms, to_ms], |row| {
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
            })?;

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
        summary.p95_latency_ms =
            self.query_p95(&connection, target_id, from_ms, to_ms, from_raw_samples)?;
        Ok(summary)
    }

    /// The percentile is taken over individual samples whenever the range is short enough to be
    /// served from them. It used to be taken over per-minute *maxima* — which reports roughly the
    /// worst sample of every minute, many times the real 95th percentile — and it re-scanned every
    /// raw sample even for ranges that were otherwise answered from the rollups.
    fn query_p95(
        &self,
        connection: &Connection,
        target_id: &str,
        from_ms: i64,
        to_ms: i64,
        from_raw_samples: bool,
    ) -> StorageResult<Option<f64>> {
        let (count_statement, value_statement) = if from_raw_samples {
            (
                "SELECT COUNT(*) FROM ping_samples
                 WHERE target_id = ?1 AND timestamp_ms >= ?2 AND timestamp_ms < ?3
                   AND latency_ms IS NOT NULL",
                "SELECT latency_ms FROM ping_samples
                 WHERE target_id = ?1 AND timestamp_ms >= ?2 AND timestamp_ms < ?3
                   AND latency_ms IS NOT NULL
                 ORDER BY latency_ms LIMIT 1 OFFSET ?4",
            )
        } else {
            (
                "SELECT COUNT(*) FROM minute_rollups
                 WHERE target_id = ?1 AND bucket_ms >= ?2 AND bucket_ms < ?3
                   AND success_count > 0",
                "SELECT latency_sum / success_count FROM minute_rollups
                 WHERE target_id = ?1 AND bucket_ms >= ?2 AND bucket_ms < ?3
                   AND success_count > 0
                 ORDER BY latency_sum / success_count LIMIT 1 OFFSET ?4",
            )
        };
        let count: i64 =
            connection.query_row(count_statement, params![target_id, from_ms, to_ms], |row| {
                row.get(0)
            })?;
        if count == 0 {
            return Ok(None);
        }
        let offset = (((count as f64) * 0.95).ceil() as i64 - 1).clamp(0, count - 1);
        let value = connection.query_row(
            value_statement,
            params![target_id, from_ms, to_ms, offset],
            |row| row.get::<_, Option<f64>>(0),
        )?;
        Ok(value)
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

fn open_connection(database_path: &Path) -> StorageResult<Connection> {
    let connection = Connection::open(database_path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        // `auto_vacuum` has to come first: it can only be stamped into the header of a database that
        // has no pages yet, and setting the journal mode already writes page 1. On an existing file
        // it is a silent no-op, which is why `compact()` exists.
        "PRAGMA auto_vacuum = INCREMENTAL;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA journal_size_limit = 33554432;",
    )?;
    Ok(connection)
}

/// Hands the freelist back to the file system a slice at a time. Each statement is its own short
/// write transaction, so a large reclaim never blocks the probes for more than a moment.
fn reclaim_free_pages(connection: &Connection) -> StorageResult<()> {
    loop {
        let mut statement =
            connection.prepare(&format!("PRAGMA incremental_vacuum({VACUUM_SLICE_PAGES})"))?;
        let mut rows = statement.query([])?;
        let mut freed = 0_u32;
        while rows.next()?.is_some() {
            freed += 1;
        }
        if freed < VACUUM_SLICE_PAGES {
            return Ok(());
        }
    }
}

fn auto_vacuum_mode(connection: &Connection) -> StorageResult<i64> {
    Ok(connection.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?)
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
            language: crate::domain::LanguagePreference::Ko,
            first_run: false,
            ..AppSettings::default()
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
        let mut classifier = QualityClassifier::new(QualityThresholds::default(), 1_000, 0);

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

    fn count(database: &Database, sql: &str) -> i64 {
        database
            .open()
            .unwrap()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn a_new_database_can_reclaim_space_incrementally() {
        let (_directory, database) = database();
        // `auto_vacuum` has to be stamped before anything else touches the file, otherwise the
        // retention pass can never hand pages back and the file only ever grows.
        assert_eq!(
            count(&database, "PRAGMA auto_vacuum"),
            AUTO_VACUUM_INCREMENTAL
        );
    }

    #[test]
    fn compact_leaves_a_database_without_reclaimable_space_alone() {
        let (_directory, database) = database();
        database
            .save_target(&Target::new("Cloudflare", "1.1.1.1"))
            .unwrap();
        assert!(!database.compact().unwrap());
    }

    #[test]
    fn p95_is_taken_over_samples_rather_than_per_minute_maxima() {
        let (_directory, database) = database();
        let mut target = Target::new("Cloudflare", "1.1.1.1");
        target.created_at_ms = 0;
        database.save_target(&target).unwrap();
        let mut classifier = QualityClassifier::new(QualityThresholds::default(), 1_000, 0);

        for second in 0..120_i64 {
            let sample = PingSample::success(
                target.id.clone(),
                second * 1_000,
                (second + 1) as f64, // 1 ms .. 120 ms
            );
            let update = classifier.observe(sample.clone());
            database.write_sample(&sample, &update, 1_000).unwrap();
        }

        let summary = database
            .history(std::slice::from_ref(&target.id), 0, 120_000, 100)
            .unwrap()
            .series[0]
            .summary
            .clone();
        assert_eq!(summary.sample_count, 120);
        assert_eq!(summary.p95_latency_ms, Some(114.0));
        assert_eq!(summary.maximum_latency_ms, Some(120.0));
    }

    #[test]
    fn cleanup_prunes_samples_rollups_and_intervals_beyond_retention() {
        let (_directory, database) = database();
        let mut target = Target::new("Cloudflare", "1.1.1.1");
        target.created_at_ms = 0;
        database.save_target(&target).unwrap();
        let now_ms = unix_time_ms();
        let stale_ms = now_ms - 40 * 86_400_000;

        for (timestamp_ms, transition_state) in [
            (stale_ms, QualityState::Stable),
            (stale_ms + 1_000, QualityState::Unstable),
            (now_ms - 3_600_000, QualityState::Stable),
        ] {
            let sample = PingSample::success(target.id.clone(), timestamp_ms, 20.0);
            let update = ClassificationUpdate {
                state: transition_state,
                state_since_ms: timestamp_ms,
                metrics: QualityMetrics::default(),
                reasons: Vec::new(),
                transition: Some(StateTransition {
                    from: QualityState::WarmingUp,
                    to: transition_state,
                    effective_at_ms: timestamp_ms,
                    reasons: Vec::new(),
                }),
            };
            database.write_sample(&sample, &update, 1_000).unwrap();
        }

        let deleted = database.cleanup(Some(30)).unwrap();

        assert_eq!(deleted, 2);
        assert_eq!(count(&database, "SELECT COUNT(*) FROM ping_samples"), 1);
        assert_eq!(count(&database, "SELECT COUNT(*) FROM minute_rollups"), 1);
        // Two intervals survive: the one that still reaches into the retained range and the one
        // that is still open. Only the interval that ended before the cutoff is pruned.
        assert_eq!(
            count(&database, "SELECT COUNT(*) FROM quality_intervals"),
            2
        );
        assert_eq!(
            count(
                &database,
                &format!(
                    "SELECT COUNT(*) FROM quality_intervals WHERE end_ms < {}",
                    now_ms - 30 * 86_400_000
                )
            ),
            0
        );
        assert_eq!(
            count(
                &database,
                "SELECT COUNT(*) FROM quality_intervals WHERE end_ms IS NULL"
            ),
            1
        );
    }

    #[test]
    fn cleanup_keeps_everything_when_retention_is_unlimited() {
        let (_directory, database) = database();
        let target = Target::new("Cloudflare", "1.1.1.1");
        database.save_target(&target).unwrap();
        let sample = PingSample::success(target.id.clone(), unix_time_ms() - 10 * 86_400_000, 20.0);
        let mut classifier = QualityClassifier::new(QualityThresholds::default(), 1_000, 0);
        let update = classifier.observe(sample.clone());
        database.write_sample(&sample, &update, 1_000).unwrap();

        assert_eq!(database.cleanup(None).unwrap(), 0);
        assert_eq!(count(&database, "SELECT COUNT(*) FROM ping_samples"), 1);
    }

    #[test]
    fn backup_writes_a_readable_copy_and_compact_keeps_the_data() {
        let (directory, database) = database();
        let target = Target::new("Cloudflare", "1.1.1.1");
        database.save_target(&target).unwrap();
        let mut classifier = QualityClassifier::new(QualityThresholds::default(), 1_000, 0);
        for second in 0..5_i64 {
            let sample = PingSample::success(target.id.clone(), second * 1_000, 20.0);
            let update = classifier.observe(sample.clone());
            database.write_sample(&sample, &update, 1_000).unwrap();
        }

        let destination = directory.path().join("backup.sqlite3");
        database.backup_to(&destination).unwrap();
        let copy = Connection::open(&destination).unwrap();
        let samples: i64 = copy
            .query_row("SELECT COUNT(*) FROM ping_samples", [], |row| row.get(0))
            .unwrap();
        assert_eq!(samples, 5);

        assert!(!database.compact().unwrap());
        assert_eq!(count(&database, "SELECT COUNT(*) FROM ping_samples"), 5);
    }

    #[test]
    fn closing_intervals_after_a_crash_clamps_to_the_last_sample() {
        let (_directory, database) = database();
        let target = Target::new("Offline", "192.0.2.1");
        database.save_target(&target).unwrap();
        let sample = PingSample::success(target.id.clone(), 5_000, 20.0);
        let update = ClassificationUpdate {
            state: QualityState::Stable,
            state_since_ms: 1_000,
            metrics: QualityMetrics::default(),
            reasons: Vec::new(),
            transition: Some(StateTransition {
                from: QualityState::WarmingUp,
                to: QualityState::Stable,
                effective_at_ms: 1_000,
                reasons: Vec::new(),
            }),
        };
        database.write_sample(&sample, &update, 1_000).unwrap();

        // A day later the app starts again: the gap must not be reported as monitored time.
        assert_eq!(database.close_open_intervals(86_400_000).unwrap(), 1);
        assert_eq!(
            count(&database, "SELECT end_ms FROM quality_intervals"),
            5_000
        );
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
