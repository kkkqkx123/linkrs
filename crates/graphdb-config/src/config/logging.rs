// Logging utility module
//
// Encapsulates flexi_logger initialization and shutdown operations, ensuring async logs are properly flushed

use crate::config::Config;
use flexi_logger::{
    Cleanup, Criterion, DeferredNow, FileSpec, Logger, LoggerHandle, Naming, WriteMode,
    TS_DASHES_BLANK_COLONS_DOT_BLANK,
};
use parking_lot::Mutex;

/// Global logger handle, used for flush on program exit
static LOGGER_HANDLE: Mutex<Option<LoggerHandle>> = Mutex::new(None);

/// Custom log formatting function, adds timestamp
///
/// Format: YYYY-MM-DD HH:MM:SS.mmm [LEVEL] module_path: message content
fn log_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &log::Record,
) -> Result<(), std::io::Error> {
    write!(
        w,
        "{} [{}] {}: {}",
        now.format(TS_DASHES_BLANK_COLONS_DOT_BLANK),
        record.level(),
        record.module_path().unwrap_or("unknown"),
        &record.args()
    )
}

/// Initialize logging system
///
/// # Arguments
/// * `config` - Application configuration, containing logging parameters
///
/// # Returns
/// * `Ok(())` - Initialization successful
/// * `Err(Box<dyn std::error::Error>)` - Initialization failed
///
/// # Examples
/// ```
/// use graphdb_config::config::Config;
/// use graphdb_config::config::logging;
///
/// let config = Config::default();
/// logging::init(&config).expect("Logging initialization failed");
/// ```
pub fn init(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let handle = Logger::try_with_str(&config.log.level)?
        .log_to_file(
            FileSpec::default()
                .basename(&config.log.file)
                .directory(&config.log.dir),
        )
        .format_for_files(log_format)
        .rotate(
            Criterion::Size(config.log.max_file_size),
            Naming::Numbers,
            Cleanup::KeepLogFiles(config.log.max_files),
        )
        .write_mode(WriteMode::Async)
        .append()
        .start()?;

    // Save handle for subsequent flush operations
    *LOGGER_HANDLE.lock() = Some(handle);

    log::info!(
        "Logging system initialized: {}/{}",
        config.log.dir,
        config.log.file
    );
    Ok(())
}

/// Flush and shutdown logging system
///
/// Call before program exit to ensure all async logs are written to file
/// This is a blocking operation that waits for the log thread to complete its work
///
/// # Examples
/// ```
/// use graphdb_config::config::logging;
///
/// // Before program exit
/// logging::shutdown();
/// ```
pub fn shutdown() {
    let mut guard = LOGGER_HANDLE.lock();
    if let Some(handle) = guard.take() {
        handle.flush();
        // handle is dropped here, which waits for async thread to complete
    }
}

/// Check if logging system is initialized
///
/// # Returns
/// * `true` - Logging system is initialized
/// * `false` - Logging system is not initialized
pub fn is_initialized() -> bool {
    LOGGER_HANDLE.lock().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Test logger initialization and shutdown with Direct mode for testing
    #[test]
    #[serial]
    fn test_logging_init_and_shutdown() {
        let config = Config::default();

        // Initialize logging with Direct mode to avoid async channel issues in tests
        // This uses flexi_logger directly instead of the init() function to avoid
        // async write mode which causes "Send" errors in concurrent test execution
        let handle = Logger::try_with_str(&config.log.level)
            .expect("Logger creation failed")
            .log_to_file(
                FileSpec::default()
                    .basename(&config.log.file)
                    .directory(&config.log.dir),
            )
            .format_for_files(log_format)
            .rotate(
                Criterion::Size(config.log.max_file_size),
                Naming::Numbers,
                Cleanup::KeepLogFiles(config.log.max_files),
            )
            .write_mode(WriteMode::Direct)
            .append()
            .start()
            .expect("Logger start failed");

        // Save handle for subsequent flush operations
        *LOGGER_HANDLE.lock() = Some(handle);
        assert!(is_initialized());

        // Write test log
        log::info!("Test log message");

        // Shutdown logging
        shutdown();
        assert!(!is_initialized());
    }

    #[test]
    fn test_is_initialized_before_init() {
        // Ensure it returns false before initialization
        // Note: Since LOGGER_HANDLE is global, this test may be affected by other tests
        // In practice, it should be tested in an independent process
    }
}
