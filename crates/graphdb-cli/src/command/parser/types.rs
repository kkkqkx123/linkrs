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
    Login {
        username: String,
        password: Option<String>,
    },
    WhoAmI,
    ShowSpaces,
    ShowTags {
        pattern: Option<String>,
    },
    ShowEdges {
        pattern: Option<String>,
    },
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
    Stream {
        query: String,
    },
    CursorOpen {
        query: String,
    },
    CursorFetch {
        cursor_id: Option<u64>,
        page_size: Option<usize>,
    },
    CursorClose {
        cursor_id: Option<u64>,
    },
    Statistics {
        target: Option<String>,
    },
    Status,
    Config {
        action: ConfigAction,
    },
    Batch {
        action: BatchAction,
    },
    Sync {
        action: SyncAction,
    },
}

#[derive(Debug, Clone)]
pub enum ConfigAction {
    Show {
        section: Option<String>,
    },
    Get {
        section: String,
        key: String,
    },
    Set {
        section: String,
        key: String,
        value: String,
    },
    Reset {
        section: String,
        key: String,
    },
}

#[derive(Debug, Clone)]
pub enum CopyDirection {
    From,
    To,
}

#[derive(Debug, Clone)]
pub enum BatchAction {
    Create {
        space_id: u64,
        batch_type: String,
        batch_size: usize,
    },
    Add {
        batch_id: String,
        file_path: String,
    },
    Execute {
        batch_id: String,
    },
    Status {
        batch_id: String,
    },
    Cancel {
        batch_id: String,
    },
    Delete {
        batch_id: String,
    },
}

#[derive(Debug, Clone)]
pub enum SyncAction {
    Status,
    Diagnostics,
    DeadLetters {
        target: Option<String>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
        offset: usize,
    },
    Requeue {
        target: Option<String>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
    },
    Retry,
    DegradedRanges {
        target: Option<String>,
        index_id: Option<u64>,
        generation: Option<u64>,
    },
    DegradedClear {
        target: String,
        index_id: u64,
        generation: u64,
        start_lsn: u64,
        end_lsn: u64,
    },
    RetentionStatus,
    RetentionRun {
        grace_lsn_distance: Option<u64>,
        max_age_ms: Option<u64>,
    },
}

#[derive(Debug, Clone)]
pub enum HistoryAction {
    Show { count: Option<usize> },
    Search { pattern: String },
    Clear,
    Exec { id: usize },
}
