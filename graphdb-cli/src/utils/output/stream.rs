//! Stream output for file and console output

use std::io::Write;
use std::path::Path;

use super::config::{OutputConfig, OutputMode};
use super::writer::{FileWriter, MultiWriter, StdoutWriter};
use super::{OutputError, Result};

/// Enum to represent different writer types without dynamic dispatch
enum WriterKind {
    Stdout(StdoutWriter),
    File(FileWriter),
    Both(MultiWriter),
}

impl Write for WriterKind {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            WriterKind::Stdout(w) => w.write(buf),
            WriterKind::File(w) => w.write(buf),
            WriterKind::Both(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            WriterKind::Stdout(w) => w.flush(),
            WriterKind::File(w) => w.flush(),
            WriterKind::Both(w) => w.flush(),
        }
    }
}

/// Stream output manager for handling file and console output
pub struct StreamOutput {
    writer: WriterKind,
}

impl StreamOutput {
    /// Create a new stream output from configuration
    pub fn from_config(config: &OutputConfig) -> Result<Self> {
        config.validate().map_err(OutputError::InvalidConfig)?;

        match config.mode {
            OutputMode::Console => {
                let writer = WriterKind::Stdout(StdoutWriter::new());
                Ok(Self { writer })
            }
            OutputMode::File => {
                let file_path = config.file_path.as_ref().expect("validated");
                let file = Self::open_file(file_path, config.append)?;
                let file_writer = FileWriter::new(file.try_clone()?);
                Ok(Self {
                    writer: WriterKind::File(file_writer),
                })
            }
            OutputMode::Both => {
                let file_path = config.file_path.as_ref().expect("validated");
                let file = Self::open_file(file_path, config.append)?;
                let file_writer = FileWriter::new(file.try_clone()?);
                let stdout_writer = StdoutWriter::new();

                let multi = MultiWriter::with_stdout_and_file(stdout_writer, file_writer);

                Ok(Self {
                    writer: WriterKind::Both(multi),
                })
            }
        }
    }

    /// Create a console-only stream output
    pub fn console() -> Self {
        Self {
            writer: WriterKind::Stdout(StdoutWriter::new()),
        }
    }

    /// Create a file-only stream output
    pub fn file(path: impl AsRef<Path>, append: bool) -> Result<Self> {
        let file = Self::open_file(path.as_ref(), append)?;
        let file_writer = FileWriter::new(file.try_clone()?);
        Ok(Self {
            writer: WriterKind::File(file_writer),
        })
    }

    /// Create a stream output to both console and file
    pub fn both(path: impl AsRef<Path>, append: bool) -> Result<Self> {
        let file = Self::open_file(path.as_ref(), append)?;
        let file_writer = FileWriter::new(file.try_clone()?);
        let stdout_writer = StdoutWriter::new();

        let multi = MultiWriter::with_stdout_and_file(stdout_writer, file_writer);

        Ok(Self {
            writer: WriterKind::Both(multi),
        })
    }

    /// Open a file for output, creating parent directories if needed
    fn open_file(path: &Path, append: bool) -> Result<std::fs::File> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let file = if append {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?
        } else {
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?
        };

        Ok(file)
    }

    /// Get a reference to the writer
    pub fn writer(&mut self) -> &mut dyn Write {
        match &mut self.writer {
            WriterKind::Stdout(w) => w,
            WriterKind::File(w) => w,
            WriterKind::Both(w) => w,
        }
    }

    /// Write data to the stream
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        Ok(())
    }

    /// Write a line to the stream
    pub fn writeln(&mut self, line: &str) -> Result<()> {
        writeln!(self.writer, "{}", line)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Flush the stream
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::tempdir;

    #[test]
    fn test_stream_output_console() {
        let stream = StreamOutput::console();
        // Console output is always available
        assert!(matches!(stream.writer, WriterKind::Stdout(_)));
    }

    #[test]
    fn test_stream_output_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_output.txt");

        {
            let mut stream = StreamOutput::file(&file_path, false).unwrap();
            stream.writeln("Hello, World!").unwrap();
        }

        let mut file = std::fs::File::open(&file_path).unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();

        assert!(contents.contains("Hello, World!"));
    }

    #[test]
    fn test_stream_output_from_config() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("config_test.txt");

        let config = OutputConfig::file(&file_path);
        {
            let mut stream = StreamOutput::from_config(&config).unwrap();
            stream.writeln("Config test").unwrap();
        }

        let contents = std::fs::read_to_string(&file_path).unwrap();
        assert!(contents.contains("Config test"));
    }
}
