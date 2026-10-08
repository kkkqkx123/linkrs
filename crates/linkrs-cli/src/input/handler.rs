use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;

use crate::completion::completer::GraphDBCompleter;
use crate::input::history::{HistoryDedupPolicy, HistoryManager, HistorySavePolicy};
use crate::utils::error::{CliError, Result};

pub struct InputHandler {
    editor: Editor<GraphDBCompleter, DefaultHistory>,
    history_mgr: HistoryManager,
}

impl InputHandler {
    pub fn new() -> Result<Self> {
        let history_mgr = HistoryManager::new(
            crate::input::history::get_default_history_path(),
            5000,
            HistoryDedupPolicy::Consecutive,
            HistorySavePolicy::Incremental,
        );

        Self::with_history_manager(history_mgr)
    }

    pub fn with_history_manager(mut history_mgr: HistoryManager) -> Result<Self> {
        let completer = GraphDBCompleter::new();
        let mut editor = Editor::new()
            .map_err(|e| CliError::Other(format!("Failed to create line editor: {}", e)))?;

        editor.set_helper(Some(completer));
        editor.set_auto_add_history(false);

        history_mgr.load()?;

        for entry in history_mgr.entries() {
            let _ = editor.add_history_entry(&entry.command);
        }

        Ok(Self {
            editor,
            history_mgr,
        })
    }

    pub fn read_line(&mut self, prompt: &str) -> Result<Option<String>> {
        match self.editor.readline(prompt) {
            Ok(line) => Ok(Some(line)),
            Err(ReadlineError::Interrupted) => Ok(None),
            Err(ReadlineError::Eof) => Ok(None),
            Err(e) => Err(CliError::Other(format!("Read error: {}", e))),
        }
    }

    pub fn add_history(&mut self, command: &str, space: Option<&str>) {
        if command.trim().is_empty() {
            return;
        }

        if self.history_mgr.should_skip(command) {
            return;
        }

        let _ = self.editor.add_history_entry(command);
        self.history_mgr.add_entry(command, space);
    }

    pub fn save_history(&mut self) {
        let _ = self.history_mgr.save_all();
    }

    pub fn history_manager(&self) -> &HistoryManager {
        &self.history_mgr
    }

    pub fn history_manager_mut(&mut self) -> &mut HistoryManager {
        &mut self.history_mgr
    }
}
