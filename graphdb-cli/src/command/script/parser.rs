#[derive(Debug, Clone)]
pub struct ParsedStatement {
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub kind: StatementKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatementKind {
    Query,
    MetaCommand,
}

pub struct ScriptParser;

impl ScriptParser {
    pub fn parse(content: &str) -> Vec<ParsedStatement> {
        let mut statements = Vec::new();
        let mut current = String::new();
        let mut line_number = 1;
        let mut start_line = 1;
        let mut parser = crate::command::script::StatementBalanceTracker::new();

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.is_empty() {
                if !current.is_empty() {
                    current.push('\n');
                }
                line_number += 1;
                continue;
            }

            if trimmed.starts_with("--") || trimmed.starts_with("//") {
                line_number += 1;
                continue;
            }

            if trimmed.starts_with('\\') {
                if !current.trim().is_empty() {
                    statements.push(ParsedStatement {
                        content: current.trim().to_string(),
                        start_line,
                        end_line: line_number - 1,
                        kind: StatementKind::Query,
                    });
                    current.clear();
                    parser = crate::command::script::StatementBalanceTracker::new();
                }

                statements.push(ParsedStatement {
                    content: trimmed.to_string(),
                    start_line: line_number,
                    end_line: line_number,
                    kind: StatementKind::MetaCommand,
                });

                line_number += 1;
                start_line = line_number;
                continue;
            }

            if current.is_empty() {
                start_line = line_number;
            } else {
                current.push('\n');
            }
            current.push_str(line);

            for ch in line.chars() {
                parser.feed(ch);
            }

            if parser.is_balanced() && trimmed.ends_with(';') {
                statements.push(ParsedStatement {
                    content: current.trim().to_string(),
                    start_line,
                    end_line: line_number,
                    kind: StatementKind::Query,
                });
                current.clear();
                parser = crate::command::script::StatementBalanceTracker::new();
                start_line = line_number + 1;
            }

            line_number += 1;
        }

        if !current.trim().is_empty() {
            statements.push(ParsedStatement {
                content: current.trim().to_string(),
                start_line,
                end_line: line_number - 1,
                kind: StatementKind::Query,
            });
        }

        statements
    }
}
