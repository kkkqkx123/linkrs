use crate::command::script::ConditionalStack;
use crate::utils::error::{CliError, Result};

pub struct ScriptExecutionContext {
    pub depth: usize,
    pub call_stack: Vec<String>,
    pub conditional_stack: ConditionalStack,
    pub current_file: Option<String>,
}

impl Default for ScriptExecutionContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptExecutionContext {
    pub fn new() -> Self {
        Self {
            depth: 0,
            call_stack: Vec::new(),
            conditional_stack: ConditionalStack::new(),
            current_file: None,
        }
    }

    pub fn enter_script(&mut self, path: &str) -> Result<()> {
        const MAX_SCRIPT_DEPTH: usize = 16;
        if self.depth >= MAX_SCRIPT_DEPTH {
            return Err(CliError::Other(format!(
                "Script nesting too deep (max {}): {}",
                MAX_SCRIPT_DEPTH, path
            )));
        }

        let canonical = std::path::Path::new(path)
            .canonicalize()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string());

        if self.call_stack.contains(&canonical) {
            return Err(CliError::Other(format!(
                "Circular script reference detected: {}",
                path
            )));
        }

        self.depth += 1;
        self.call_stack.push(canonical);
        self.current_file = Some(path.to_string());
        Ok(())
    }

    pub fn exit_script(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
        }
        self.call_stack.pop();
        self.current_file = self.call_stack.last().cloned();
    }
}
