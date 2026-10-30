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
    Passwd,
    WhoAmI,
    ShowUsers,
    Users {
        action: UsersAction,
    },
    Sessions {
        action: SessionsAction,
    },
    ShowSpaces,
    ShowTags {
        pattern: Option<String>,
    },
    ShowEdges {
        pattern: Option<String>,
    },
    SchemaVersions {
        space: String,
        label: String,
        is_edge: bool,
    },
    SchemaChanges {
        space: String,
        label: String,
        from: u64,
        to: u64,
        is_edge: bool,
    },
    SchemaBreaking {
        space: String,
        label: String,
        from: u64,
        to: u64,
        is_edge: bool,
    },
    ShowFunctions,
    ShowFunction {
        name: String,
    },
    RegisterFunction {
        name: String,
        implementation: String,
    },
    UnregisterFunction {
        name: String,
    },
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
    },
    Profile {
        query: String,
    },
    Validate {
        query: String,
    },
    Fulltext {
        action: FulltextAction,
    },
    Vector {
        action: VectorAction,
    },
    Import {
        format: ImportFormat,
        file_path: String,
        target: ImportTarget,
        batch_size: Option<usize>,
        server: bool,
    },
    Export {
        format: ExportFormat,
        file_path: String,
        query: String,
        streaming: bool,
        chunk_size: Option<usize>,
        server: bool,
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
    Transactions {
        action: TransactionsAction,
    },
    Migration {
        action: MigrationAction,
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

/// User-management subcommands (`\users ...`).
#[derive(Debug, Clone)]
pub enum UsersAction {
    List,
    Create {
        username: String,
        password: String,
    },
    Drop {
        username: String,
    },
    ResetPassword {
        username: String,
    },
    Enable {
        username: String,
    },
    Disable {
        username: String,
    },
    Grant {
        username: String,
        role: String,
        space: String,
    },
    Revoke {
        username: String,
        space: String,
    },
}

/// Server-side transaction observability and intervention
/// (`\transactions ...`).
///
/// Unlike `\txstatus` (local session state), these commands query the
/// server truth across all sessions: list, metrics, kill, per-transaction
/// outbox retry.
#[derive(Debug, Clone)]
pub enum TransactionsAction {
    List,
    Metrics,
    Kill { id: u64 },
    RetryOutbox { id: u64 },
    SavepointCreate { id: u64, name: String },
    SavepointList { id: u64 },
    SavepointRollback { id: u64, savepoint_id: u64 },
    SavepointRelease { id: u64, savepoint_id: u64 },
}

/// Server-session management subcommands (`\sessions ...`).
#[derive(Debug, Clone)]
pub enum SessionsAction {
    List,
    Show { session_id: i64 },
    Kill { session_id: i64 },
}

/// Fulltext index subcommands (`\fulltext ...`).
#[derive(Debug, Clone)]
pub enum FulltextAction {
    Search {
        space_id: u64,
        tag_name: String,
        field_name: String,
        query: String,
        limit: usize,
    },
    ListIndexes,
    Inconsistent,
    Rebuild {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },
    RebuildStatus {
        rebuild_id: String,
    },
    Clear {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },
}

/// Vector index subcommands (`\vector ...`).
#[derive(Debug, Clone)]
pub enum VectorAction {
    Create {
        space_id: u64,
        tag_name: String,
        field_name: String,
        vector_size: usize,
    },
    ListIndexes,
    Info {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },
    Drop {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },
    Search {
        space_id: u64,
        tag_name: String,
        field_name: String,
        vector: Vec<f32>,
        limit: usize,
    },
    Get {
        space_id: u64,
        tag_name: String,
        field_name: String,
        point_id: String,
    },
    Scroll {
        space_id: u64,
        tag_name: String,
        field_name: String,
        limit: usize,
        offset: Option<String>,
    },
    SetPayload {
        space_id: u64,
        tag_name: String,
        field_name: String,
        point_ids: Vec<String>,
        payload: serde_json::Map<String, serde_json::Value>,
    },
    MergePayloadFields {
        space_id: u64,
        tag_name: String,
        field_name: String,
        point_ids: Vec<String>,
        payload: serde_json::Map<String, serde_json::Value>,
    },
    DeletePayload {
        space_id: u64,
        tag_name: String,
        field_name: String,
        point_ids: Vec<String>,
        keys: Vec<String>,
    },
    Count {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },
    Rebuild {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },
    RebuildStatus {
        rebuild_id: String,
    },
    Clear {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },
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
pub enum MigrationAction {
    Plan {
        space: String,
        label: String,
        from: u64,
        to: u64,
        is_edge: bool,
        expand_contract: bool,
        output: Option<String>,
    },
    DryRun {
        file: String,
    },
    Execute {
        file: String,
        yes: bool,
    },
    Rollback {
        file: String,
        yes: bool,
    },
    Status {
        space: String,
        label: String,
        is_edge: bool,
    },
    History {
        space: String,
        label: String,
        is_edge: bool,
    },
    Watch {
        space: String,
        label: String,
        is_edge: bool,
        follow: bool,
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
        event_ids: Option<Vec<i64>>,
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
