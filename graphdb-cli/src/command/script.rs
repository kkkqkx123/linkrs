pub mod condition;
pub mod conditional;
pub mod context;
pub mod parser;
pub mod tracker;

pub use condition::ConditionExpr;
pub use conditional::ConditionalStack;
pub use context::ScriptExecutionContext;
pub use parser::{ParsedStatement, ScriptParser, StatementKind};
pub use tracker::StatementBalanceTracker;

pub fn is_statement_complete(input: &str) -> bool {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return true;
    }

    if trimmed.starts_with('\\') {
        return true;
    }

    let mut tracker = StatementBalanceTracker::new();
    for ch in trimmed.chars() {
        tracker.feed(ch);
    }

    if !tracker.is_balanced() {
        return false;
    }

    if trimmed.ends_with(';') {
        return true;
    }

    let upper = trimmed.to_uppercase();
    let auto_complete = [
        "SHOW SPACES",
        "SHOW TAGS",
        "SHOW EDGES",
        "SHOW INDEXES",
        "SHOW USERS",
        "SHOW FUNCTIONS",
    ];
    auto_complete.iter().any(|cmd| upper == *cmd)
}
