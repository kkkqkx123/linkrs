use std::path::PathBuf;

use crate::utils::error::{CliError, Result};

#[derive(Debug, Clone)]
pub struct QueryBuffer {
    lines: Vec<String>,
}

impl Default for QueryBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryBuffer {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    pub fn content(&self) -> String {
        self.lines.join("\n")
    }

    pub fn add_line(&mut self, line: &str) {
        self.lines.push(line.to_string());
    }

    pub fn reset(&mut self) {
        self.lines.clear();
    }

    pub fn set_content(&mut self, content: &str) {
        self.lines = content.lines().map(String::from).collect();
        if content.ends_with('\n') && !content.is_empty() {
            self.lines.push(String::new());
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

pub fn get_editor_command(session_editor: Option<&str>) -> String {
    if let Some(editor) = session_editor {
        if !editor.is_empty() {
            return editor.to_string();
        }
    }

    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            if cfg!(target_os = "windows") {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        })
}

pub fn edit_in_external_editor(
    buffer: &mut QueryBuffer,
    file: Option<&str>,
    line: Option<usize>,
    session_editor: Option<&str>,
) -> Result<bool> {
    let editor = get_editor_command(session_editor);

    let temp_dir = std::env::temp_dir();
    let temp_file = if let Some(f) = file {
        PathBuf::from(f)
    } else {
        temp_dir.join("graphdb_query.gql")
    };

    if file.is_none() || !temp_file.exists() {
        if let Some(parent) = temp_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&temp_file, buffer.content())
            .map_err(|e| CliError::Other(format!("Failed to write temp file: {}", e)))?;
    }

    let mut cmd = std::process::Command::new(&editor);
    if let Some(l) = line {
        cmd.arg(format!("+{}", l));
    }
    cmd.arg(&temp_file);

    let status = cmd
        .status()
        .map_err(|e| CliError::Other(format!("Failed to launch editor '{}': {}", editor, e)))?;

    if !status.success() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(&temp_file)
        .map_err(|e| CliError::Other(format!("Failed to read edited file: {}", e)))?;

    if file.is_none() {
        let _ = std::fs::remove_file(&temp_file);
    }

    buffer.set_content(&content);
    Ok(true)
}

pub fn write_buffer_to_file(buffer: &QueryBuffer, path: &str) -> Result<()> {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, buffer.content())
        .map_err(|e| CliError::Other(format!("Failed to write file '{}': {}", path.display(), e)))
}
