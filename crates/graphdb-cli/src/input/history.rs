use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::utils::error::{CliError, Result};

#[derive(Debug, Clone)]
pub enum HistoryDedupPolicy {
    None,
    Consecutive,
    Global,
}

#[derive(Debug, Clone)]
pub enum HistorySavePolicy {
    OnExit,
    Incremental,
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub id: usize,
    pub timestamp: u64,
    pub space: Option<String>,
    pub command: String,
}

pub struct HistoryManager {
    history_path: PathBuf,
    max_size: usize,
    dedup: HistoryDedupPolicy,
    save_policy: HistorySavePolicy,
    entries: Vec<HistoryEntry>,
    next_id: usize,
}

impl Default for HistoryManager {
    fn default() -> Self {
        Self::new(
            get_default_history_path(),
            5000,
            HistoryDedupPolicy::Consecutive,
            HistorySavePolicy::Incremental,
        )
    }
}

impl HistoryManager {
    pub fn new(
        history_path: PathBuf,
        max_size: usize,
        dedup: HistoryDedupPolicy,
        save_policy: HistorySavePolicy,
    ) -> Self {
        Self {
            history_path,
            max_size,
            dedup,
            save_policy,
            entries: Vec::new(),
            next_id: 1,
        }
    }

    pub fn load(&mut self) -> Result<()> {
        if !self.history_path.exists() {
            return Ok(());
        }

        let file = File::open(&self.history_path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(entry) = self.parse_line(line) {
                self.entries.push(entry);
            }
        }

        if !self.entries.is_empty() {
            self.next_id = self.entries.last().map(|e| e.id + 1).unwrap_or(1);
        }

        self.enforce_max_size();
        Ok(())
    }

    fn parse_line(&mut self, line: &str) -> Option<HistoryEntry> {
        if let Some(rest) = line.strip_prefix('#') {
            let parts: Vec<&str> = rest.splitn(2, '|').collect();
            if parts.len() == 2 {
                let timestamp = parts[0].parse().ok()?;
                let command_part = parts[1];

                let (space, command) = if let Some(cmd_rest) = command_part.strip_prefix('[') {
                    if let Some(bracket_end) = cmd_rest.find(']') {
                        let space = cmd_rest[..bracket_end].to_string();
                        let cmd = cmd_rest[bracket_end + 1..]
                            .trim_start_matches('|')
                            .to_string();
                        (Some(space), cmd)
                    } else {
                        (None, command_part.to_string())
                    }
                } else {
                    (None, command_part.to_string())
                };

                let id = self.next_id;
                self.next_id += 1;

                return Some(HistoryEntry {
                    id,
                    timestamp,
                    space,
                    command,
                });
            }
        }

        let id = self.next_id;
        self.next_id += 1;

        Some(HistoryEntry {
            id,
            timestamp: 0,
            space: None,
            command: line.to_string(),
        })
    }

    pub fn add_entry(&mut self, command: &str, space: Option<&str>) {
        let command = command.trim().to_string();
        if command.is_empty() {
            return;
        }

        if self.should_skip(&command) {
            return;
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let entry = HistoryEntry {
            id: self.next_id,
            timestamp,
            space: space.map(String::from),
            command,
        };
        self.next_id += 1;

        self.entries.push(entry);
        self.enforce_max_size();

        if matches!(self.save_policy, HistorySavePolicy::Incremental) {
            let _ = self.append_last();
        }
    }

    pub fn should_skip(&self, command: &str) -> bool {
        let command = command.trim();
        if command.is_empty() {
            return true;
        }

        match self.dedup {
            HistoryDedupPolicy::None => false,
            HistoryDedupPolicy::Consecutive => self
                .entries
                .last()
                .map(|e| e.command == command)
                .unwrap_or(false),
            HistoryDedupPolicy::Global => self.entries.iter().any(|e| e.command == command),
        }
    }

    fn enforce_max_size(&mut self) {
        while self.entries.len() > self.max_size {
            self.entries.remove(0);
        }
    }

    pub fn append_last(&self) -> Result<()> {
        if let Some(entry) = self.entries.last() {
            if let Some(parent) = self.history_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.history_path)
                .map_err(CliError::IoError)?;

            let line = format_entry(entry);
            writeln!(file, "{}", line).map_err(CliError::IoError)?;
        }
        Ok(())
    }

    pub fn save_all(&self) -> Result<()> {
        if let Some(parent) = self.history_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.history_path)
            .map_err(CliError::IoError)?;

        for entry in &self.entries {
            let line = format_entry(entry);
            writeln!(file, "{}", line).map_err(CliError::IoError)?;
        }

        Ok(())
    }

    pub fn clear(&mut self) -> Result<()> {
        self.entries.clear();
        self.next_id = 1;
        if self.history_path.exists() {
            std::fs::remove_file(&self.history_path)?;
        }
        Ok(())
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn search(&self, pattern: &str) -> Vec<&HistoryEntry> {
        let pattern_lower = pattern.to_lowercase();
        self.entries
            .iter()
            .filter(|e| e.command.to_lowercase().contains(&pattern_lower))
            .collect()
    }

    pub fn recent(&self, count: usize) -> Vec<&HistoryEntry> {
        let start = self.entries.len().saturating_sub(count);
        self.entries[start..].iter().collect()
    }

    pub fn get_by_id(&self, id: usize) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn save_policy(&self) -> &HistorySavePolicy {
        &self.save_policy
    }

    pub fn commands(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.command.clone()).collect()
    }
}

fn format_entry(entry: &HistoryEntry) -> String {
    match &entry.space {
        Some(space) => format!("#{}|[{}]|{}", entry.timestamp, space, entry.command),
        None => format!("#{}|{}", entry.timestamp, entry.command),
    }
}

pub fn get_default_history_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".graphdb").join("cli_history")
}
