// Logging utility module
//
// Encapsulates flexi_logger initialization and shutdown operations, ensuring async logs are properly flushed

use crate::Config;
use flexi_logger::{
    Cleanup, Criterion, DeferredNow, Duplicate, FileSpec, FormatFunction, Logger, LoggerHandle,
    Naming, WriteMode, TS_DASHES_BLANK_COLONS_DOT_BLANK,
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
        record.args()
    )
}

/// One JSON object per line, for structured log pipelines.
fn json_log_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &log::Record,
) -> Result<(), std::io::Error> {
    writeln!(
        w,
        "{{\"ts\":\"{}\",\"level\":\"{}\",\"module\":\"{}\",\"message\":\"{}\"}}",
        now.format(TS_DASHES_BLANK_COLONS_DOT_BLANK),
        record.level(),
        json_escape(record.module_path().unwrap_or("unknown")),
        json_escape(&record.args().to_string()),
    )
}

/// Escape the characters that must not appear raw inside a JSON string.
fn json_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control < ' ' => {
                escaped.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// Selected line format for file, stdout and any duplicated writer.
fn configured_format(config: &Config) -> FormatFunction {
    if config.log.json_format {
        json_log_format
    } else {
        log_format
    }
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
/// use linkrs_config::Config;
/// use linkrs_config::logging;
///
/// let config = Config::default();
/// logging::init(&config).expect("Logging initialization failed");
/// ```
pub fn init(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let format = configured_format(config);
    let mut logger = Logger::try_with_str(&config.log.level)?
        .log_to_file(
            FileSpec::default()
                .basename(&config.log.basename)
                .directory(&config.log.dir),
        )
        .format_for_files(format)
        .rotate(
            Criterion::Size(config.log.max_file_size_mb * 1024 * 1024),
            Naming::Numbers,
            Cleanup::KeepLogFiles(config.log.max_files),
        )
        .write_mode(WriteMode::Async)
        .append();

    if config.log.stdout {
        logger = logger
            .duplicate_to_stdout(Duplicate::All)
            .format_for_stdout(format);
    }

    let handle = logger.start()?;

    // Save handle for subsequent flush operations
    *LOGGER_HANDLE.lock() = Some(handle);

    log::info!(
        "Logging system initialized: {}/{}.log (stdout={}, json={})",
        config.log.dir,
        config.log.basename,
        config.log.stdout,
        config.log.json_format
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
/// use linkrs_config::logging;
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
                    .basename(&config.log.basename)
                    .directory(&config.log.dir),
            )
            .format_for_files(log_format)
            .rotate(
                Criterion::Size(config.log.max_file_size_mb * 1024 * 1024),
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
    fn test_json_escape_covers_json_string_specials() {
        assert_eq!(json_escape("plain"), "plain");
        assert_eq!(json_escape("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(json_escape("line\nbreak"), "line\\nbreak");
        assert_eq!(json_escape("tab\there"), "tab\\there");
        assert_eq!(json_escape("\u{1}"), "\\u0001");
    }

    #[test]
    fn test_json_log_format_emits_single_line_object() {
        let mut buffer: Vec<u8> = Vec::new();
        let mut now = DeferredNow::new();
        let record = log::Record::builder()
            .args(format_args!("query \"failed\"\nsecond line"))
            .level(log::Level::Error)
            .target("test")
            .module_path(Some("linkrs::test"))
            .build();

        json_log_format(&mut buffer, &mut now, &record).expect("formatting should succeed");

        let line = String::from_utf8(buffer).expect("valid utf-8 output");
        assert!(line.starts_with("{\"ts\":\""), "unexpected line: {line}");
        assert!(line.ends_with("}\n"), "unexpected line: {line}");
        assert!(
            line.contains("\"level\":\"ERROR\""),
            "unexpected line: {line}"
        );
        assert!(
            line.contains("\"module\":\"linkrs::test\""),
            "unexpected line: {line}"
        );
        assert!(line.contains("\\\"failed\\\""), "unexpected line: {line}");
        assert!(line.contains("\\n"), "unexpected line: {line}");
        assert_eq!(
            line.matches('\n').count(),
            1,
            "message newline must be escaped"
        );
    }

    #[test]
    fn test_is_initialized_before_init() {
        // Ensure it returns false before initialization
        // Note: Since LOGGER_HANDLE is global, this test may be affected by other tests
        // In practice, it should be tested in an independent process
    }
}
