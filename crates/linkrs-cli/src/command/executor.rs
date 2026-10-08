use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use crate::analysis::timing::QueryTimer;
use crate::command::parser::{Command, MetaCommand};
use crate::command::script::{
    ConditionExpr, ConditionalStack, ScriptExecutionContext, ScriptParser,
};
use crate::input::buffer::QueryBuffer;
use crate::output::formatter::OutputFormatter;
use crate::session::manager::SessionManager;
use crate::transaction::TransactionManager;
use crate::utils::error::{CliError, Result};

pub mod meta;

pub struct CommandExecutor {
    formatter: OutputFormatter,
    output_file: Option<std::fs::File>,
    query_buffer: QueryBuffer,
    conditional_stack: ConditionalStack,
    script_ctx: ScriptExecutionContext,
    force_mode: bool,
    single_transaction: bool,
    transaction_active: bool,
    tx_manager: TransactionManager,
    active_cursor: Option<u64>,
}

impl CommandExecutor {
    /// Upper bound on statements sent in a single batch request, bounding the
    /// server-side write-gate hold time and the HTTP request body size.
    const MAX_BATCH_STATEMENTS: usize = 10_000;

    /// Whether a statement is a pure `INSERT` (vertex/edge) DML statement that
    /// can be batched into a shared auto-commit window. The first whitespace
    /// token must be `INSERT`.
    fn is_insert_statement(content: &str) -> bool {
        content
            .split_whitespace()
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("INSERT"))
    }
    pub fn new(formatter: OutputFormatter) -> Self {
        Self {
            formatter,
            output_file: None,
            query_buffer: QueryBuffer::new(),
            conditional_stack: ConditionalStack::new(),
            script_ctx: ScriptExecutionContext::new(),
            force_mode: false,
            single_transaction: false,
            transaction_active: false,
            tx_manager: TransactionManager::new(),
            active_cursor: None,
        }
    }

    pub fn with_options(formatter: OutputFormatter, force: bool, single_transaction: bool) -> Self {
        Self {
            formatter,
            output_file: None,
            query_buffer: QueryBuffer::new(),
            conditional_stack: ConditionalStack::new(),
            script_ctx: ScriptExecutionContext::new(),
            force_mode: force,
            single_transaction,
            transaction_active: false,
            tx_manager: TransactionManager::new(),
            active_cursor: None,
        }
    }

    pub fn active_cursor(&self) -> Option<u64> {
        self.active_cursor
    }

    pub fn set_active_cursor(&mut self, cursor_id: Option<u64>) {
        self.active_cursor = cursor_id;
    }

    pub fn formatter(&self) -> &OutputFormatter {
        &self.formatter
    }

    pub fn formatter_mut(&mut self) -> &mut OutputFormatter {
        &mut self.formatter
    }

    pub fn query_buffer(&self) -> &QueryBuffer {
        &self.query_buffer
    }

    pub fn query_buffer_mut(&mut self) -> &mut QueryBuffer {
        &mut self.query_buffer
    }

    pub fn conditional_stack(&self) -> &ConditionalStack {
        &self.conditional_stack
    }

    pub fn tx_manager(&self) -> &TransactionManager {
        &self.tx_manager
    }

    pub fn tx_manager_mut(&mut self) -> &mut TransactionManager {
        &mut self.tx_manager
    }

    pub fn set_force_mode(&mut self, force: bool) {
        self.force_mode = force;
    }

    pub fn set_single_transaction(&mut self, single: bool) {
        self.single_transaction = single;
    }

    pub fn execute<'a>(
        &'a mut self,
        command: Command,
        session_mgr: &'a mut SessionManager,
    ) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + 'a>> {
        Box::pin(async move {
            match command {
                Command::Empty => Ok(true),
                Command::Query(query) => self.execute_query(&query, session_mgr).await,
                Command::MetaCommand(meta) => self.execute_meta(meta, session_mgr).await,
            }
        })
    }

    async fn execute_query(
        &mut self,
        query: &str,
        session_mgr: &mut SessionManager,
    ) -> Result<bool> {
        let query = query.trim();

        if query.is_empty() {
            return Ok(true);
        }

        if !self.conditional_stack.is_active() {
            return Ok(true);
        }

        if self.tx_manager.is_failed() {
            let error = self
                .tx_manager
                .state()
                .error_message()
                .unwrap_or("Transaction is in failed state")
                .to_string();
            return Err(CliError::TransactionFailed(error));
        }

        let use_match = query.trim_start().to_uppercase();
        if use_match.starts_with("USE ") {
            let space = query.trim_start()[4..].trim().trim_end_matches(';').trim();
            return self.execute_use_space(space, session_mgr).await;
        }

        let mut timer = QueryTimer::new();

        let result = session_mgr.execute_query(query).await?;
        timer.record_phase("execution");

        self.tx_manager.record_query();

        let output = self.formatter.format_result(&result);
        self.write_output(&output)?;

        if self.formatter.timing_enabled() {
            self.write_output(&timer.format_time())?;
        }

        Ok(true)
    }

    async fn execute_use_space(
        &mut self,
        space: &str,
        session_mgr: &mut SessionManager,
    ) -> Result<bool> {
        session_mgr.switch_space(space).await?;
        self.write_output(&format!("Space changed to '{}'", space))?;
        Ok(true)
    }

    async fn execute_meta(
        &mut self,
        meta: MetaCommand,
        session_mgr: &mut SessionManager,
    ) -> Result<bool> {
        match meta {
            MetaCommand::Quit => meta::control::execute_quit(self, session_mgr).await,
            MetaCommand::ForceQuit => meta::control::execute_force_quit(self, session_mgr).await,
            MetaCommand::Help { topic } => meta::control::execute_help(self, topic.as_deref()),
            MetaCommand::Connect { space } => {
                meta::connection::execute_connect(self, &space, session_mgr).await
            }
            MetaCommand::Disconnect => {
                meta::connection::execute_disconnect(self, session_mgr).await
            }
            MetaCommand::ConnInfo => meta::connection::execute_conninfo(self, session_mgr),
            MetaCommand::WhoAmI => meta::connection::execute_whoami(self, session_mgr),
            MetaCommand::Login { username, password } => {
                meta::connection::execute_login(self, &username, password, session_mgr).await
            }
            MetaCommand::Passwd => meta::connection::execute_passwd(self, session_mgr).await,
            MetaCommand::ShowSpaces => meta::schema::execute_show_spaces(self, session_mgr).await,
            MetaCommand::ShowTags { .. } => {
                meta::schema::execute_show_tags(self, session_mgr).await
            }
            MetaCommand::ShowEdges { .. } => {
                meta::schema::execute_show_edges(self, session_mgr).await
            }
            MetaCommand::ShowFunctions => {
                meta::schema::execute_show_functions(self, session_mgr).await
            }
            MetaCommand::Describe { object } => {
                meta::schema::execute_describe(self, &object, session_mgr).await
            }
            MetaCommand::DescribeEdge { name } => {
                meta::schema::execute_describe_edge(self, &name, session_mgr).await
            }
            MetaCommand::Format { format } => meta::control::execute_format(self, format),
            MetaCommand::Pager { command } => meta::control::execute_pager(self, command),
            MetaCommand::Timing => meta::control::execute_timing(self),
            MetaCommand::Set { name, value } => {
                meta::variables::execute_set(self, name, value, session_mgr).await
            }
            MetaCommand::Unset { name } => {
                meta::variables::execute_unset(self, name, session_mgr).await
            }
            MetaCommand::ShowVariables => {
                meta::variables::execute_show_variables(self, session_mgr)
            }
            MetaCommand::ExecuteScript { path } => {
                self.execute_script(&path, session_mgr, false).await
            }
            MetaCommand::ExecuteScriptRaw { path } => {
                self.execute_script(&path, session_mgr, true).await
            }
            MetaCommand::OutputRedirect { path } => meta::io::execute_output_redirect(self, path),
            MetaCommand::ShellCommand { command } => {
                meta::control::execute_shell_command(self, &command)
            }
            MetaCommand::Version => meta::control::execute_version(self),
            MetaCommand::Copyright => meta::control::execute_copyright(self),
            MetaCommand::Begin => meta::transaction::execute_begin(self, session_mgr).await,
            MetaCommand::Commit => meta::transaction::execute_commit(self, session_mgr).await,
            MetaCommand::Rollback => meta::transaction::execute_rollback(self, session_mgr).await,
            MetaCommand::Autocommit { value } => {
                meta::transaction::execute_autocommit(self, value).await
            }
            MetaCommand::Isolation { level } => {
                meta::transaction::execute_isolation(self, level).await
            }
            MetaCommand::Savepoint { name } => {
                meta::transaction::execute_savepoint(self, &name, session_mgr).await
            }
            MetaCommand::RollbackTo { name } => {
                meta::transaction::execute_rollback_to(self, &name, session_mgr).await
            }
            MetaCommand::ReleaseSavepoint { name } => {
                meta::transaction::execute_release_savepoint(self, &name, session_mgr).await
            }
            MetaCommand::TxStatus => meta::transaction::execute_txstatus(self),
            MetaCommand::Edit { file, line } => {
                meta::buffer::execute_edit(self, file.as_deref(), line, session_mgr)
            }
            MetaCommand::PrintBuffer => meta::buffer::execute_print_buffer(self),
            MetaCommand::ResetBuffer => meta::buffer::execute_reset_buffer(self),
            MetaCommand::WriteBuffer { file } => meta::buffer::execute_write_buffer(self, &file),
            MetaCommand::History { .. } => {
                // Command history lives in the line editor (`InputHandler`);
                // the executor never sees it, so `\history` only documents
                // the in-REPL navigation.
                self.write_output(
                    "Command history is kept by the REPL line editor; use UP/DOWN arrows to navigate it.",
                )?;
                Ok(true)
            }
            MetaCommand::If { condition } => {
                self.handle_if(condition, session_mgr)?;
                Ok(true)
            }
            MetaCommand::Elif { condition } => {
                self.handle_elif(condition, session_mgr)?;
                Ok(true)
            }
            MetaCommand::Else => {
                self.conditional_stack.push_else();
                Ok(true)
            }
            MetaCommand::EndIf => {
                self.conditional_stack.pop();
                Ok(true)
            }
            MetaCommand::Explain {
                query,
                analyze,
                format: _,
            } => meta::analyze::execute_explain(self, &query, analyze, session_mgr).await,
            MetaCommand::Profile { query } => {
                meta::analyze::execute_profile(self, &query, session_mgr).await
            }
            MetaCommand::Import {
                format,
                file_path,
                target,
                batch_size,
            } => {
                meta::io::execute_import(self, format, file_path, target, batch_size, session_mgr)
                    .await
            }
            MetaCommand::Export {
                format,
                file_path,
                query,
                streaming,
                chunk_size,
            } => {
                meta::io::execute_export(
                    self,
                    format,
                    file_path,
                    &query,
                    streaming,
                    chunk_size,
                    session_mgr,
                )
                .await
            }
            MetaCommand::Copy {
                direction,
                target,
                file_path,
                streaming,
                chunk_size,
            } => {
                meta::io::execute_copy(
                    self,
                    direction,
                    target,
                    file_path,
                    streaming,
                    chunk_size,
                    session_mgr,
                )
                .await
            }
            MetaCommand::ExportSpace {
                space_name,
                output_path,
                format,
                tags,
                edge_types,
            } => {
                meta::io::execute_export_space(
                    self,
                    space_name,
                    output_path,
                    format,
                    tags,
                    edge_types,
                    session_mgr,
                )
                .await
            }
            MetaCommand::ExportSchema {
                output_path,
                format,
            } => meta::io::execute_export_schema(self, output_path, format, session_mgr).await,
            MetaCommand::ImportSchema { file_path } => {
                meta::io::execute_import_schema(self, file_path, session_mgr).await
            }
            MetaCommand::Stream { query } => {
                meta::cursor::execute_stream(self, &query, session_mgr).await
            }
            MetaCommand::CursorOpen { query } => {
                meta::cursor::execute_cursor_open(self, &query, session_mgr).await
            }
            MetaCommand::CursorFetch {
                cursor_id,
                page_size,
            } => meta::cursor::execute_cursor_fetch(self, cursor_id, page_size, session_mgr).await,
            MetaCommand::CursorClose { cursor_id } => {
                meta::cursor::execute_cursor_close(self, cursor_id, session_mgr).await
            }
            MetaCommand::Statistics { target } => {
                meta::stats::execute_statistics(self, target.as_deref(), session_mgr).await
            }
            MetaCommand::Status => meta::stats::execute_status(self, session_mgr).await,
            MetaCommand::Config { action } => {
                meta::config::execute_config(self, &action, session_mgr).await
            }
            MetaCommand::Batch { action } => {
                meta::batch::execute_batch(self, &action, session_mgr).await
            }
            MetaCommand::Sync { action } => {
                meta::sync::execute_sync(self, &action, session_mgr).await
            }
        }
    }

    fn handle_if(&mut self, condition: String, session_mgr: &mut SessionManager) -> Result<()> {
        let vars = self.get_all_variables(session_mgr);
        let expr = ConditionExpr::parse(&condition)?;
        let result = expr.evaluate(&vars);
        self.conditional_stack.push_if(result);
        Ok(())
    }

    fn handle_elif(&mut self, condition: String, session_mgr: &mut SessionManager) -> Result<()> {
        let vars = self.get_all_variables(session_mgr);
        let expr = ConditionExpr::parse(&condition)?;
        let result = expr.evaluate(&vars);
        self.conditional_stack.push_elif(result);
        Ok(())
    }

    fn get_all_variables(&self, session_mgr: &SessionManager) -> HashMap<String, String> {
        let mut vars = HashMap::new();

        for (key, val) in std::env::vars() {
            vars.insert(format!("ENV_{}", key), val);
        }

        if let Some(session) = session_mgr.session() {
            for (key, val) in session.variable_store.all_variables() {
                vars.insert(key.clone(), val.clone());
            }
        }

        vars
    }

    async fn execute_script(
        &mut self,
        path: &str,
        session_mgr: &mut SessionManager,
        raw: bool,
    ) -> Result<bool> {
        self.script_ctx.enter_script(path)?;

        let content =
            fs::read_to_string(path).map_err(|_| CliError::ScriptNotFound(path.to_string()))?;

        let statements = ScriptParser::parse(&content);

        if self.single_transaction && !self.transaction_active {
            session_mgr.execute_query("BEGIN TRANSACTION").await?;
            self.transaction_active = true;
        }

        let mut index = 0;
        while index < statements.len() {
            let stmt = &statements[index];
            if !self.conditional_stack.is_active()
                && !matches!(
                    stmt.kind,
                    crate::command::script::StatementKind::MetaCommand
                )
            {
                index += 1;
                continue;
            }

            let content = Self::substitute_content(session_mgr, &stmt.content, raw)?;

            // Batch consecutive pure INSERT statements (auto-commit DML load):
            // they run inside a single server-side auto-commit batch window.
            // Any interleaved meta command, non-INSERT statement, or the batch
            // size cap breaks the run; the server falls back to per-statement
            // execution inside an explicit transaction.
            if Self::is_insert_statement(&content) {
                let (end, stop) = self
                    .execute_script_batch(path, &statements, index, content, raw, session_mgr)
                    .await?;
                index = end;
                if stop {
                    break;
                }
                continue;
            }

            let command = crate::command::parser::parse_command(&content);
            match self.execute(command, session_mgr).await {
                Ok(should_continue) => {
                    if !should_continue {
                        break;
                    }
                }
                Err(e) => {
                    if self.handle_script_error(
                        path,
                        stmt.start_line,
                        stmt.end_line,
                        &e.to_string(),
                        session_mgr,
                    )? {
                        break;
                    }
                }
            }
            index += 1;
        }

        if self.single_transaction && self.transaction_active {
            match session_mgr.execute_query("COMMIT").await {
                Ok(_) => {
                    self.transaction_active = false;
                    self.write_output("Transaction committed.")?;
                }
                Err(e) => {
                    let _ = session_mgr.execute_query("ROLLBACK").await;
                    self.transaction_active = false;
                    self.write_output(
                        &self
                            .formatter
                            .format_error(&format!("Transaction failed, rolled back: {}", e)),
                    )?;
                }
            }
        }

        self.script_ctx.exit_script();
        Ok(true)
    }

    /// Substitute `\set` variables in one script statement, unless raw mode.
    fn substitute_content(
        session_mgr: &SessionManager,
        content: &str,
        raw: bool,
    ) -> Result<String> {
        if raw {
            return Ok(content.to_string());
        }
        match session_mgr.session() {
            Some(s) => s.substitute_variables(content),
            None => Ok(content.to_string()),
        }
    }

    /// Run a consecutive INSERT run inside one server-side auto-commit batch
    /// window. Returns the first unconsumed statement index plus whether the
    /// script must stop (`ON_ERROR_STOP` after a failure).
    async fn execute_script_batch(
        &mut self,
        path: &str,
        statements: &[crate::command::script::ParsedStatement],
        start: usize,
        first: String,
        raw: bool,
        session_mgr: &mut SessionManager,
    ) -> Result<(usize, bool)> {
        let mut batch = vec![first];
        let mut end = start + 1;
        while end < statements.len() && batch.len() < Self::MAX_BATCH_STATEMENTS {
            let next = &statements[end];
            if !self.conditional_stack.is_active()
                && !matches!(
                    next.kind,
                    crate::command::script::StatementKind::MetaCommand
                )
            {
                break;
            }
            if !matches!(next.kind, crate::command::script::StatementKind::Query) {
                break;
            }
            let next_content = Self::substitute_content(session_mgr, &next.content, raw)?;
            if !Self::is_insert_statement(&next_content) {
                break;
            }
            batch.push(next_content);
            end += 1;
        }

        if self.tx_manager.is_failed() {
            let error = self
                .tx_manager
                .state()
                .error_message()
                .unwrap_or("Transaction is in failed state")
                .to_string();
            return Err(CliError::TransactionFailed(error));
        }

        let outcomes = session_mgr.execute_batch(&batch).await?;
        let mut stop = false;
        for (offset, result) in outcomes.iter().enumerate() {
            let batch_stmt = &statements[start + offset];
            self.tx_manager.record_query();
            if result.error.is_none() {
                let output = self.formatter.format_result(result);
                self.write_output(&output)?;
            } else {
                let error = result
                    .error
                    .as_ref()
                    .map(|e| format!("{}: {}", e.code, e.message))
                    .unwrap_or_else(|| "Unknown error".to_string());
                if self.handle_script_error(
                    path,
                    batch_stmt.start_line,
                    batch_stmt.end_line,
                    &error,
                    session_mgr,
                )? {
                    stop = true;
                    break;
                }
            }
        }
        Ok((end, stop))
    }

    /// Report one failed script statement. Returns `true` when execution
    /// must stop (`ON_ERROR_STOP` set without force mode).
    fn handle_script_error(
        &mut self,
        path: &str,
        start_line: usize,
        end_line: usize,
        error: &str,
        session_mgr: &SessionManager,
    ) -> Result<bool> {
        let line_info = if start_line == end_line {
            format!("line {}", start_line)
        } else {
            format!("lines {}-{}", start_line, end_line)
        };
        self.write_output(
            &self
                .formatter
                .format_error(&format!("{}: {} (in {})", path, error, line_info)),
        )?;

        let on_error_stop = session_mgr
            .session()
            .map(|s| s.variable_store.get_bool("ON_ERROR_STOP"))
            .unwrap_or(false);

        Ok(on_error_stop && !self.force_mode)
    }

    pub fn write_output(&mut self, content: &str) -> Result<()> {
        if let Some(ref mut file) = self.output_file {
            file.write_all(content.as_bytes())
                .map_err(CliError::IoError)?;
            file.write_all(b"\n").map_err(CliError::IoError)?;
        } else {
            println!("{}", content);
        }
        Ok(())
    }
}
