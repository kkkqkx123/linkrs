use crate::analysis::explain::ExplainFormat;
use crate::io::{ExportFormat, ImportFormat, ImportTarget};
use crate::output::formatter::OutputFormat;

#[derive(Debug)]
pub enum Command {
    Query(String),
    MetaCommand(MetaCommand),
    Empty,
}

#[derive(Debug)]
pub enum MetaCommand {
    Quit,
    ForceQuit,
    Help {
        topic: Option<String>,
    },
    Connect {
        space: String,
    },
    Disconnect,
    ConnInfo,
    ShowSpaces,
    ShowTags {
        pattern: Option<String>,
    },
    ShowEdges {
        pattern: Option<String>,
    },
    ShowIndexes {
        pattern: Option<String>,
    },
    ShowUsers,
    ShowFunctions,
    Describe {
        object: String,
    },
    DescribeEdge {
        name: String,
    },
    Format {
        format: OutputFormat,
    },
    Pager {
        command: Option<String>,
    },
    Timing,
    Set {
        name: String,
        value: Option<String>,
    },
    Unset {
        name: String,
    },
    ShowVariables,
    ExecuteScript {
        path: String,
    },
    ExecuteScriptRaw {
        path: String,
    },
    OutputRedirect {
        path: Option<String>,
    },
    ShellCommand {
        command: String,
    },
    Version,
    Copyright,
    Begin,
    Commit,
    Rollback,
    Autocommit {
        value: Option<String>,
    },
    Isolation {
        level: Option<String>,
    },
    Savepoint {
        name: String,
    },
    RollbackTo {
        name: String,
    },
    ReleaseSavepoint {
        name: String,
    },
    TxStatus,
    Edit {
        file: Option<String>,
        line: Option<usize>,
    },
    PrintBuffer,
    ResetBuffer,
    WriteBuffer {
        file: String,
    },
    History {
        action: HistoryAction,
    },
    If {
        condition: String,
    },
    Elif {
        condition: String,
    },
    Else,
    EndIf,
    Explain {
        query: String,
        analyze: bool,
        format: ExplainFormat,
    },
    Profile {
        query: String,
    },
    Import {
        format: ImportFormat,
        file_path: String,
        target: ImportTarget,
        batch_size: Option<usize>,
    },
    Export {
        format: ExportFormat,
        file_path: String,
        query: String,
        streaming: bool,
        chunk_size: Option<usize>,
    },
    Copy {
        direction: CopyDirection,
        target: String,
        file_path: String,
        streaming: bool,
        chunk_size: Option<usize>,
    },
    Dump {
        database: String,
        output_path: String,
        format: String,
        compress: bool,
    },
    Restore {
        source_path: String,
        database: String,
        overwrite: bool,
        strict: bool,
    },
    ExportSpace {
        space_name: String,
        output_path: String,
        format: String,
        tags: Option<String>,
        edge_types: Option<String>,
    },
    ExportSchema {
        output_path: String,
        format: String,
    },
    ImportSchema {
        file_path: String,
    },
}

#[derive(Debug, Clone)]
pub enum CopyDirection {
    From,
    To,
}

#[derive(Debug, Clone)]
pub enum HistoryAction {
    Show { count: Option<usize> },
    Search { pattern: String },
    Clear,
    Exec { id: usize },
}
