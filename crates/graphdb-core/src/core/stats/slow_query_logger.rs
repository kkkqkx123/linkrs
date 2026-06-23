//! Slow Query Logger
//!
//! Independent slow query log file with async writing and log rotation.

use chrono::Local;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;

use super::profile::{QueryProfile, QueryStatus};
use super::utils::micros_to_millis;

/// Slow query log configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowQueryConfig {
    /// Whether to enable slow query logging
    pub enabled: bool,
    /// Slow query threshold in milliseconds
    pub threshold_ms: u64,
    /// Log file path
    pub log_file_path: String,
    /// Maximum file size in MB before rotation
    pub max_file_size_mb: u64,
    /// Maximum number of log files to keep
    pub max_files: u32,
    /// Whether to use verbose format
    pub verbose_format: bool,
    /// Async write buffer size
    pub buffer_size: usize,
    /// Whether to use JSON format
    pub json_format: bool,
}

impl Default for SlowQueryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_ms: 1000,
            log_file_path: "logs/slow_query.log".to_string(),
            max_file_size_mb: 100,
            max_files: 5,
            verbose_format: false,
            buffer_size: 100,
            json_format: false,
        }
    }
}

/// Slow query logger
#[derive(Debug)]
pub struct SlowQueryLogger {
    config: SlowQueryConfig,
    tx: Option<mpsc::Sender<String>>,
    writer_handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Drop for SlowQueryLogger {
    fn drop(&mut self) {
        // Drop the sender to signal the writer thread to exit
        // Take the sender first to ensure it's dropped before joining
        let _ = self.tx.take();

        // Wait for the writer thread to finish
        let mut handle_guard = self.writer_handle.lock();
        if let Some(handle) = handle_guard.take() {
            let _ = handle.join();
        }
    }
}

impl SlowQueryLogger {
    /// Create a new slow query logger
    pub fn new(config: SlowQueryConfig) -> Result<Self, std::io::Error> {
        // Create log directory
        if let Some(parent) = Path::new(&config.log_file_path).parent() {
            fs::create_dir_all(parent)?;
        }

        // Create channel
        let (tx, rx) = mpsc::channel::<String>();

        // Initialize log file
        let initial_path = PathBuf::from(&config.log_file_path);
        let file_size = if initial_path.exists() {
            fs::metadata(&initial_path)?.len()
        } else {
            0
        };

        // Spawn writer thread
        let writer_handle = Self::spawn_writer_thread(
            rx,
            config.clone(),
            AtomicU64::new(file_size),
            Mutex::new(initial_path.clone()),
        );

        Ok(Self {
            config,
            tx: Some(tx),
            writer_handle: Mutex::new(Some(writer_handle)),
        })
    }

    /// Log a slow query
    pub fn log(&self, profile: &QueryProfile) {
        // Write to log file asynchronously
        if let Some(ref tx) = self.tx {
            let log_entry = if self.config.json_format {
                self.format_json_log(profile)
            } else if self.config.verbose_format {
                self.format_verbose_log(profile)
            } else {
                self.format_standard_log(profile)
            };

            // Async send (non-blocking)
            let _ = tx.send(log_entry);
        }
    }

    /// Update configuration
    pub fn update_config(&mut self, config: SlowQueryConfig) {
        self.config = config;
    }

    /// Format standard log entry
    fn format_standard_log(&self, profile: &QueryProfile) -> String {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let status_str = match profile.status {
            QueryStatus::Success => "success",
            QueryStatus::Failed => "failed",
        };

        format!(
            "[{}] [SLOW_QUERY] [trace_id={}] [session_id={}] [duration={}ms] [status={}] {}\n",
            timestamp,
            profile.trace_id,
            profile.session_id,
            micros_to_millis(profile.total_duration_us),
            status_str,
            profile.query_text
        )
    }

    /// Format verbose log entry
    fn format_verbose_log(&self, profile: &QueryProfile) -> String {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let mut log = String::new();

        log.push_str(&format!("[{}] [SLOW_QUERY]\n", timestamp));
        log.push_str(&format!("  trace_id: {}\n", profile.trace_id));
        log.push_str(&format!("  session_id: {}\n", profile.session_id));
        log.push_str(&format!("  query_text: {}\n", profile.query_text));
        log.push_str(&format!(
            "  duration: {}ms\n",
            micros_to_millis(profile.total_duration_us)
        ));

        let status_str = match profile.status {
            QueryStatus::Success => "success",
            QueryStatus::Failed => "failed",
        };
        log.push_str(&format!("  status: {}\n", status_str));

        if let Some(ref error_info) = profile.error_info {
            log.push_str(&format!("  error_type: {}\n", error_info.error_type));
            log.push_str(&format!("  error_phase: {}\n", error_info.error_phase));
            log.push_str(&format!("  error_message: {}\n", error_info.error_message));
        }

        // Stage statistics
        log.push_str("  stages:\n");
        log.push_str(&format!("    parse: {}ms\n", profile.stages.parse_ms()));
        log.push_str(&format!(
            "    validate: {}ms\n",
            profile.stages.validate_ms()
        ));
        log.push_str(&format!("    plan: {}ms\n", profile.stages.plan_ms()));
        log.push_str(&format!(
            "    optimize: {}ms\n",
            profile.stages.optimize_ms()
        ));
        log.push_str(&format!("    execute: {}ms\n", profile.stages.execute_ms()));

        log.push_str(&format!("  result_count: {}\n", profile.result_count));

        // Executor statistics
        if !profile.executor_stats.is_empty() {
            log.push_str("  executor_stats:\n");
            for stat in &profile.executor_stats {
                log.push_str(&format!(
                    "    - executor: {}, duration: {}ms, rows: {}\n",
                    stat.executor_type,
                    stat.duration_ms(),
                    stat.rows_processed()
                ));
            }
        }

        log.push('\n');
        log
    }

    /// Format JSON log entry
    fn format_json_log(&self, profile: &QueryProfile) -> String {
        #[derive(Serialize)]
        struct SlowQueryLogEntry {
            timestamp: String,
            trace_id: String,
            session_id: i64,
            query_text: String,
            duration_ms: f64,
            status: String,
            stages: StageStats,
            result_count: usize,
            executor_stats: Vec<ExecutorStatOutput>,
            #[serde(skip_serializing_if = "Option::is_none")]
            error_info: Option<ErrorInfoOutput>,
        }

        #[derive(Serialize)]
        struct StageStats {
            parse_ms: f64,
            validate_ms: f64,
            plan_ms: f64,
            optimize_ms: f64,
            execute_ms: f64,
        }

        #[derive(Serialize)]
        struct ExecutorStatOutput {
            executor_type: String,
            duration_ms: f64,
            rows_processed: usize,
        }

        #[derive(Serialize)]
        struct ErrorInfoOutput {
            error_type: String,
            error_phase: String,
            error_message: String,
        }

        let error_info = profile.error_info.as_ref().map(|e| ErrorInfoOutput {
            error_type: format!("{:?}", e.error_type),
            error_phase: format!("{:?}", e.error_phase),
            error_message: e.error_message.clone(),
        });

        let log_entry = SlowQueryLogEntry {
            timestamp: Local::now().to_rfc3339(),
            trace_id: profile.trace_id.clone(),
            session_id: profile.session_id,
            query_text: profile.query_text.clone(),
            duration_ms: micros_to_millis(profile.total_duration_us),
            status: match profile.status {
                QueryStatus::Success => "success",
                QueryStatus::Failed => "failed",
            }
            .to_string(),
            stages: StageStats {
                parse_ms: profile.stages.parse_ms(),
                validate_ms: profile.stages.validate_ms(),
                plan_ms: profile.stages.plan_ms(),
                optimize_ms: profile.stages.optimize_ms(),
                execute_ms: profile.stages.execute_ms(),
            },
            result_count: profile.result_count,
            executor_stats: profile
                .executor_stats
                .iter()
                .map(|stat| ExecutorStatOutput {
                    executor_type: stat.executor_type.clone(),
                    duration_ms: stat.duration_ms(),
                    rows_processed: stat.rows_processed(),
                })
                .collect(),
            error_info,
        };

        serde_json::to_string(&log_entry).unwrap_or_default() + "\n"
    }

    /// Spawn writer thread
    fn spawn_writer_thread(
        rx: mpsc::Receiver<String>,
        config: SlowQueryConfig,
        file_size: AtomicU64,
        _file_path: Mutex<PathBuf>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut writer = BufWriter::new(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&config.log_file_path)
                    .expect("Failed to open slow query log file"),
            );

            let mut lines_written = 0;

            while let Ok(log_entry) = rx.recv() {
                let bytes = log_entry.as_bytes();
                let current_size = file_size.load(Ordering::Relaxed);

                // Check if rotation is needed
                if current_size + bytes.len() as u64 > config.max_file_size_mb * 1024 * 1024 {
                    // Perform log rotation
                    if let Err(e) = Self::rotate_logs(&config) {
                        eprintln!("Failed to rotate slow query log: {}", e);
                    }

                    // Reopen new file
                    writer = BufWriter::new(
                        OpenOptions::new()
                            .create(true)
                            .write(true)
                            .truncate(true)
                            .open(&config.log_file_path)
                            .expect("Failed to open new slow query log file"),
                    );
                    file_size.store(0, Ordering::Relaxed);
                }

                // Write log entry
                if let Err(e) = writer.write_all(bytes) {
                    eprintln!("Failed to write slow query log: {}", e);
                }

                lines_written += 1;

                // Periodic flush
                if lines_written % 10 == 0 {
                    let _ = writer.flush();
                }

                file_size.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            }

            // Ensure all data is written
            let _ = writer.flush();
        })
    }

    /// Rotate log files
    fn rotate_logs(config: &SlowQueryConfig) -> std::io::Result<()> {
        let base_path = Path::new(&config.log_file_path);

        // Delete oldest file
        let oldest_path = format!("{}.{}", base_path.display(), config.max_files);
        if Path::new(&oldest_path).exists() {
            fs::remove_file(&oldest_path)?;
        }

        // Rotate existing files
        for i in (1..config.max_files).rev() {
            let old_path = if i == 1 {
                base_path.to_path_buf()
            } else {
                PathBuf::from(format!("{}.{}", base_path.display(), i))
            };

            let new_path = PathBuf::from(format!("{}.{}", base_path.display(), i + 1));

            if old_path.exists() {
                fs::rename(&old_path, &new_path)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slow_query_logger_creation() {
        let config = SlowQueryConfig::default();
        let logger = SlowQueryLogger::new(config);
        assert!(logger.is_ok());
    }

    #[test]
    fn test_log_standard_format() {
        let config = SlowQueryConfig {
            verbose_format: false,
            json_format: false,
            ..Default::default()
        };

        let logger = SlowQueryLogger::new(config).unwrap();
        let mut profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        profile.total_duration_us = 2000000; // 2000ms

        logger.log(&profile);
        // Log should be written asynchronously
    }

    #[test]
    fn test_log_verbose_format() {
        let config = SlowQueryConfig {
            verbose_format: true,
            json_format: false,
            ..Default::default()
        };

        let logger = SlowQueryLogger::new(config).unwrap();
        let mut profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        profile.total_duration_us = 2000000;

        logger.log(&profile);
    }

    #[test]
    fn test_log_json_format() {
        let config = SlowQueryConfig {
            verbose_format: false,
            json_format: true,
            ..Default::default()
        };

        let logger = SlowQueryLogger::new(config).unwrap();
        let mut profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        profile.total_duration_us = 2000000;

        logger.log(&profile);
    }
}
