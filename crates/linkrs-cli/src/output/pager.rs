use std::io::Write;
use std::process::{Command, Stdio};

pub struct Pager {
    command: Option<String>,
}

impl Default for Pager {
    fn default() -> Self {
        Self::new()
    }
}

impl Pager {
    pub fn new() -> Self {
        Self { command: None }
    }

    pub fn set_command(&mut self, command: Option<String>) {
        self.command = command;
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    pub fn display(&self, content: &str) -> std::io::Result<()> {
        if let Some(cmd) = &self.command {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if parts.is_empty() {
                println!("{}", content);
                return Ok(());
            }

            let program = parts[0];
            let args: &[&str] = &parts[1..];

            let mut child = Command::new(program)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(content.as_bytes())?;
            }

            child.wait()?;
        } else {
            println!("{}", content);
        }

        Ok(())
    }
}
