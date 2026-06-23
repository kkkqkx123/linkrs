use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use crate::analysis::timing::QueryTimer;
use crate::command::parser::{Command, HistoryAction, MetaCommand};
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
}

impl CommandExecutor {
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
        }
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

    pub fn execute_meta_sync(
        &mut self,
        meta: MetaCommand,
        session_mgr: &mut SessionManager,
    ) -> Result<SyncMetaResult> {
        match &meta {
            MetaCommand::If { .. }
            | MetaCommand::Elif { .. }
            | MetaCommand::Else
            | MetaCommand::EndIf => {
                self.handle_conditional(&meta, session_mgr)?;
                Ok(SyncMetaResult::Continue)
            }
            MetaCommand::Edit { .. }
            | MetaCommand::PrintBuffer
            | MetaCommand::ResetBuffer
            | MetaCommand::WriteBuffer { .. } => {
                self.handle_buffer_command(&meta, session_mgr)?;
                Ok(SyncMetaResult::Continue)
            }
            MetaCommand::History { .. } => {
                self.handle_history_command(&meta, session_mgr)?;
                Ok(SyncMetaResult::Continue)
            }
            _ => Ok(SyncMetaResult::NeedsAsync(meta)),
        }
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
            MetaCommand::ShowSpaces => meta::schema::execute_show_spaces(self, session_mgr).await,
            MetaCommand::ShowTags { .. } => {
                meta::schema::execute_show_tags(self, session_mgr).await
            }
            MetaCommand::ShowEdges { .. } => {
                meta::schema::execute_show_edges(self, session_mgr).await
            }
            MetaCommand::ShowIndexes { .. } => {
                meta::schema::execute_show_indexes(self, session_mgr).await
            }
            MetaCommand::ShowUsers => meta::schema::execute_show_users(self),
            MetaCommand::ShowFunctions => meta::schema::execute_show_functions(self),
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
            MetaCommand::History { action } => {
                self.handle_history_action(action, session_mgr)?;
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
              MetaCommand::Dump {
                  database,
                  output_path,
                  format,
                  compress,
              } => {
meta::io::execute_dump(
                       self,
                       database,
                       output_path,
                       format,
                       compress,
                   )
                   .await
              }
              MetaCommand::Restore {
                  source_path,
                  database,
                  overwrite,
                  strict,
              } => {
meta::io::execute_restore(
                       self,
                       source_path,
                       database,
                       overwrite,
                       strict,
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
              } => {
                  meta::io::execute_export_schema(self, output_path, format, session_mgr).await
              }
              MetaCommand::ImportSchema { file_path } => {
                  meta::io::execute_import_schema(self, file_path, session_mgr).await
              }
          }
     }

    fn handle_conditional(
        &mut self,
        meta: &MetaCommand,
        session_mgr: &mut SessionManager,
    ) -> Result<()> {
        match meta {
            MetaCommand::If { condition } => self.handle_if(condition.clone(), session_mgr)?,
            MetaCommand::Elif { condition } => self.handle_elif(condition.clone(), session_mgr)?,
            MetaCommand::Else => self.conditional_stack.push_else(),
            MetaCommand::EndIf => {
                self.conditional_stack.pop();
            }
            _ => {}
        }
        Ok(())
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

    fn handle_buffer_command(
        &mut self,
        meta: &MetaCommand,
        session_mgr: &mut SessionManager,
    ) -> Result<()> {
        match meta {
            MetaCommand::Edit { file, line } => {
                meta::buffer::execute_edit(self, file.as_deref(), *line, session_mgr)?;
            }
            MetaCommand::PrintBuffer => {
                meta::buffer::execute_print_buffer(self)?;
            }
            MetaCommand::ResetBuffer => {
                meta::buffer::execute_reset_buffer(self)?;
            }
            MetaCommand::WriteBuffer { file } => {
                meta::buffer::execute_write_buffer(self, file)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_history_command(
        &mut self,
        meta: &MetaCommand,
        _session_mgr: &mut SessionManager,
    ) -> Result<()> {
        if let MetaCommand::History { action } = meta {
            self.handle_history_action(action.clone(), _session_mgr)?;
        }
        Ok(())
    }

    fn handle_history_action(
        &mut self,
        action: HistoryAction,
        _session_mgr: &mut SessionManager,
    ) -> Result<()> {
        match action {
            HistoryAction::Show { count } => {
                self.write_output("History display is handled by the REPL loop.")?;
                let _ = count;
            }
            HistoryAction::Search { pattern } => {
                self.write_output(&format!(
                    "History search for '{}' is handled by the REPL loop.",
                    pattern
                ))?;
            }
            HistoryAction::Clear => {
                self.write_output("History clear is handled by the REPL loop.")?;
            }
            HistoryAction::Exec { id } => {
                self.write_output(&format!(
                    "History exec #{} is handled by the REPL loop.",
                    id
                ))?;
                let _ = id;
            }
        }
        Ok(())
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

        for stmt in &statements {
            if !self.conditional_stack.is_active()
                && !matches!(
                    stmt.kind,
                    crate::command::script::StatementKind::MetaCommand
                )
            {
                continue;
            }

            let content = if !raw {
                let session = session_mgr.session();
                if let Some(s) = session {
                    s.substitute_variables(&stmt.content)?
                } else {
                    stmt.content.clone()
                }
            } else {
                stmt.content.clone()
            };

            let command = crate::command::parser::parse_command(&content);
            match self.execute(command, session_mgr).await {
                Ok(should_continue) => {
                    if !should_continue {
                        break;
                    }
                }
                Err(e) => {
                    let line_info = if stmt.start_line == stmt.end_line {
                        format!("line {}", stmt.start_line)
                    } else {
                        format!("lines {}-{}", stmt.start_line, stmt.end_line)
                    };
                    self.write_output(
                        &self
                            .formatter
                            .format_error(&format!("{}: {} (in {})", path, e, line_info)),
                    )?;

                    let on_error_stop = session_mgr
                        .session()
                        .map(|s| s.variable_store.get_bool("ON_ERROR_STOP"))
                        .unwrap_or(false);

                    if on_error_stop && !self.force_mode {
                        break;
                    }
                }
            }
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

pub enum SyncMetaResult {
    Continue,
    NeedsAsync(MetaCommand),
}
