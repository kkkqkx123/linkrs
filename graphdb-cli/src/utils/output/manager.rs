//! Output manager for centralized output control

use std::io::Write;
use std::sync::Mutex;

use super::writer::{StderrWriter, StdoutWriter};
use super::Result;

/// Output format types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// Plain text output
    #[default]
    Plain,
    /// JSON formatted output
    Json,
    /// Table formatted output
    Table,
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Format::Plain => write!(f, "plain"),
            Format::Json => write!(f, "json"),
            Format::Table => write!(f, "table"),
        }
    }
}

/// Manages output configuration and operations
pub struct OutputManager {
    stdout: Mutex<Box<dyn Write + Send>>,
    stderr: Mutex<Box<dyn Write + Send>>,
    format: Format,
}

impl OutputManager {
    /// Create a new output manager with default stdout/stderr
    pub fn new() -> Self {
        Self {
            stdout: Mutex::new(Box::new(StdoutWriter::new())),
            stderr: Mutex::new(Box::new(StderrWriter::new())),
            format: Format::default(),
        }
    }

    /// Set the stdout writer
    pub fn with_stdout<W: Write + Send + 'static>(mut self, writer: W) -> Self {
        self.stdout = Mutex::new(Box::new(writer));
        self
    }

    /// Set the stderr writer
    pub fn with_stderr<W: Write + Send + 'static>(mut self, writer: W) -> Self {
        self.stderr = Mutex::new(Box::new(writer));
        self
    }

    /// Set the output format
    pub fn with_format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    /// Get the current format
    pub fn format(&self) -> Format {
        self.format
    }

    /// Check if current format is JSON
    pub fn is_json_format(&self) -> bool {
        self.format == Format::Json
    }

    /// Check if current format is Table
    pub fn is_table_format(&self) -> bool {
        self.format == Format::Table
    }

    /// Print a line to stdout
    pub fn println(&self, msg: &str) -> Result<()> {
        let mut stdout = self.stdout.lock().expect("lock poisoned");
        writeln!(stdout, "{}", msg)?;
        stdout.flush()?;
        Ok(())
    }

    /// Print to stdout without newline
    pub fn print(&self, msg: &str) -> Result<()> {
        let mut stdout = self.stdout.lock().expect("lock poisoned");
        write!(stdout, "{}", msg)?;
        stdout.flush()?;
        Ok(())
    }

    /// Print an error message to stderr
    pub fn print_error(&self, msg: &str) -> Result<()> {
        let mut stderr = self.stderr.lock().expect("lock poisoned");
        writeln!(stderr, "{}", msg)?;
        stderr.flush()?;
        Ok(())
    }

    /// Print a success message
    pub fn print_success(&self, msg: &str) -> Result<()> {
        self.println(&format!("[OK] {}", msg))
    }

    /// Print a warning message
    pub fn print_warning(&self, msg: &str) -> Result<()> {
        self.println(&format!("[WARN] {}", msg))
    }

    /// Print an info message
    pub fn print_info(&self, msg: &str) -> Result<()> {
        self.println(&format!("[INFO] {}", msg))
    }

    /// Print a separator line
    pub fn print_separator(&self, char: char, length: usize) -> Result<()> {
        let line: String = std::iter::repeat_n(char, length).collect();
        self.println(&line)
    }

    /// Print an empty line
    pub fn print_empty_line(&self) -> Result<()> {
        self.println("")
    }

    /// Get a reference to the stdout writer
    pub fn stdout(&self) -> std::sync::MutexGuard<'_, Box<dyn Write + Send>> {
        self.stdout.lock().expect("lock poisoned")
    }

    /// Get a reference to the stderr writer
    pub fn stderr(&self) -> std::sync::MutexGuard<'_, Box<dyn Write + Send>> {
        self.stderr.lock().expect("lock poisoned")
    }
}

impl Default for OutputManager {
    fn default() -> Self {
        Self::new()
    }
}

// Global default manager
static DEFAULT_MANAGER: Mutex<Option<OutputManager>> = Mutex::new(None);

/// Get the global default output manager, initializing if necessary
pub fn get_default_manager() -> OutputManager {
    let mut guard = DEFAULT_MANAGER.lock().expect("lock poisoned");
    if guard.is_none() {
        *guard = Some(OutputManager::new());
    }
    guard.as_ref().expect("just initialized").clone()
}

impl Clone for OutputManager {
    fn clone(&self) -> Self {
        // Note: This creates a new manager with fresh stdout/stderr handles
        // The cloned manager will have independent locks
        Self {
            stdout: Mutex::new(Box::new(StdoutWriter::new())),
            stderr: Mutex::new(Box::new(StderrWriter::new())),
            format: self.format,
        }
    }
}

/// Global convenience function: print a line
pub fn println(msg: &str) -> Result<()> {
    get_default_manager().println(msg)
}

/// Global convenience function: print without newline
pub fn print(msg: &str) -> Result<()> {
    get_default_manager().print(msg)
}

/// Global convenience function: print error
pub fn print_error(msg: &str) -> Result<()> {
    get_default_manager().print_error(msg)
}

/// Global convenience function: print success
pub fn print_success(msg: &str) -> Result<()> {
    get_default_manager().print_success(msg)
}

/// Global convenience function: print warning
pub fn print_warning(msg: &str) -> Result<()> {
    get_default_manager().print_warning(msg)
}

/// Global convenience function: print info
pub fn print_info(msg: &str) -> Result<()> {
    get_default_manager().print_info(msg)
}

/// Set the global output format
pub fn set_global_format(format: Format) {
    let mut guard = DEFAULT_MANAGER.lock().expect("lock poisoned");
    if guard.is_none() {
        *guard = Some(OutputManager::new());
    }
    if let Some(ref mut manager) = *guard {
        *manager = manager.clone().with_format(format);
    }
}

/// Get the global output format
pub fn get_global_format() -> Format {
    get_default_manager().format()
}
