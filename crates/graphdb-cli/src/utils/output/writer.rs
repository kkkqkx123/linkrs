//! Writer implementations for output module

use std::io::{self, BufWriter, Write};

/// A writer that outputs to stdout
pub struct StdoutWriter {
    stdout: io::Stdout,
}

impl StdoutWriter {
    /// Create a new stdout writer
    pub fn new() -> Self {
        Self {
            stdout: io::stdout(),
        }
    }
}

impl Default for StdoutWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for StdoutWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdout.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()
    }
}

/// A writer that outputs to stderr
pub struct StderrWriter {
    stderr: io::Stderr,
}

impl StderrWriter {
    /// Create a new stderr writer
    pub fn new() -> Self {
        Self {
            stderr: io::stderr(),
        }
    }
}

impl Default for StderrWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for StderrWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stderr.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stderr.flush()
    }
}

/// A writer that outputs to a file with buffering
pub struct FileWriter {
    writer: BufWriter<std::fs::File>,
}

impl FileWriter {
    /// Create a new file writer
    pub fn new(file: std::fs::File) -> Self {
        Self {
            writer: BufWriter::new(file),
        }
    }

    /// Create a new file writer from path
    pub fn from_path(path: &std::path::Path, append: bool) -> io::Result<Self> {
        use std::fs::OpenOptions;

        let file = if append {
            OpenOptions::new().create(true).append(true).open(path)?
        } else {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?
        };

        Ok(Self::new(file))
    }
}

impl Write for FileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// A writer that outputs to multiple writers
/// Uses enum to avoid dynamic dispatch for known writer types
pub enum MultiWriter {
    StdoutFile(StdoutWriter, FileWriter),
}

impl MultiWriter {
    /// Create a new multi-writer with stdout and file writers
    pub fn with_stdout_and_file(stdout: StdoutWriter, file: FileWriter) -> Self {
        Self::StdoutFile(stdout, file)
    }

    /// Get the number of writers
    pub fn len(&self) -> usize {
        match self {
            MultiWriter::StdoutFile(_, _) => 2,
        }
    }

    /// Check if the multi-writer is empty
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl Write for MultiWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            MultiWriter::StdoutFile(stdout, file) => {
                // Write to both writers
                stdout.write_all(buf)?;
                file.write_all(buf)?;
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            MultiWriter::StdoutFile(stdout, file) => {
                stdout.flush()?;
                file.flush()?;
                Ok(())
            }
        }
    }
}
