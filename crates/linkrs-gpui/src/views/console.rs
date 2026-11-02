use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState, InputState},
    notification::Notification,
    tab::{Tab, TabBar},
    table::{DataTable, TableState},
    v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
};
use gpui_kit::{
    actions, div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity,
    EventEmitter, Focusable, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window,
};
use linkrs_client::{ClientError, QueryErrorInfo};

use crate::components::{
    empty_state, form_row, new_table_state, on_copy_row, render_json_block, QueryTableData,
    QueryTableDelegate,
};
use crate::services::{connection_service, cursor_service, query_service};
use crate::settings::{self, HistoryPath, QueryHistoryItem};

actions!(linkrs, [RunQuery, CancelQuery]);

/// Page size for one cursor fetch in the console cursor pager.
const CURSOR_PAGE_SIZE: usize = 200;
/// Editor pane height as a fraction of the panel. Results take the rest.
const EDITOR_HEIGHT: f32 = 260.;

/// One executed statement with its result grid (or error) and metadata.
struct ResultTab {
    sql: String,
    status: TabStatus,
    data: Rc<RefCell<QueryTableData>>,
    table: Entity<TableState<QueryTableDelegate>>,
    error: Option<QueryErrorInfo>,
    elapsed_ms: u64,
    trace_id: Option<String>,
}

/// Title plus payload for the Validate / profile / running-queries inspector.
#[derive(Clone)]
struct InspectOutput {
    title: String,
    payload: serde_json::Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TabStatus {
    Pending,
    Running,
    Ok,
    Failed,
}

/// Query console panel: SQL editor, multi-statement execution, result tabs,
/// CSV/JSON export, and the local query history.
/// Parsed query parameters and session variables.
#[derive(Clone, Debug)]
struct ParsedBindings {
    parameters: std::collections::HashMap<String, serde_json::Value>,
    session_variables: std::collections::HashMap<String, serde_json::Value>,
}

pub struct ConsolePanel {
    editor: Entity<EditorState>,
    tabs: Vec<ResultTab>,
    active_tab: Option<usize>,
    running: Option<Task<()>>,
    history_open: bool,
    favorites_open: bool,
    fav_name: Entity<InputState>,
    fav_path: Option<HistoryPath>,
    inspect: Option<InspectOutput>,
    inspect_loading: bool,
    running_open: bool,
    running_payload: serde_json::Value,
    kill_session: Entity<InputState>,
    kill_query: Entity<InputState>,
    params_open: bool,
    params: Entity<InputState>,
    session_vars: Entity<InputState>,
    params_error: Option<SharedString>,
    /// Execution path captured at run start for history recording.
    run_stream: bool,
    /// When set, statements run through the SSE streaming endpoint.
    stream: bool,
    /// Forward-only cursor pager state (`None` when no cursor is open).
    cursor_id: Option<u64>,
    cursor_tab: Option<usize>,
    cursor_has_more: bool,
    cursor_loading: bool,
    cursor_error: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
}

impl ConsolePanel {
    pub(crate) const PANEL_NAME: &'static str = "Console";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| EditorState::new(window, cx).language("sql"));
        let fav_name = cx.new(|cx| InputState::new(window, cx).placeholder("Favorite name"));
        let kill_session = cx.new(|cx| InputState::new(window, cx).placeholder("Session id"));
        let kill_query = cx.new(|cx| InputState::new(window, cx).placeholder("Query id"));
        let params = cx.new(|cx| {
            InputState::new(window, cx).placeholder("{\"name\": value}")
        });
        let session_vars = cx.new(|cx| {
            InputState::new(window, cx).placeholder("{\"name\": value}")
        });
        Self {
            editor,
            tabs: Vec::new(),
            active_tab: None,
            running: None,
            history_open: false,
            favorites_open: false,
            fav_name,
            fav_path: None,
            inspect: None,
            inspect_loading: false,
            running_open: false,
            running_payload: serde_json::Value::Null,
            kill_session,
            kill_query,
            params_open: false,
            params,
            session_vars,
            params_error: None,
            run_stream: false,
            stream: false,
            cursor_id: None,
            cursor_tab: None,
            cursor_has_more: false,
            cursor_loading: false,
            cursor_error: None,
            focus_handle: cx.focus_handle(),
        }
    }

    // -- Actions -------------------------------------------------------------

    fn on_run_query(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx);
    }

    fn on_cancel_query(&mut self, _: &CancelQuery, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel(cx);
    }

    /// Run the editor content; used by the command palette.
    pub(crate) fn run_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx);
    }

    /// Flip the streaming execution toggle; used by the command palette.
    pub(crate) fn toggle_stream(&mut self, cx: &mut Context<Self>) {
        self.stream = !self.stream;
        cx.notify();
    }

    fn on_copy_row(
        &mut self,
        _: &crate::components::CopyRow,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.active_tab.and_then(|ix| self.tabs.get(ix)) {
            on_copy_row(&tab.data, cx);
        }
    }

    // -- Execution -----------------------------------------------------------

    /// Split the editor script into statements and run them one by one,
    /// stopping at the first failure.
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let script = self.editor.read(cx).text().to_string();
        let statements = query_service::split_statements(&script);
        if statements.is_empty() {
            window.push_notification(
                Notification::info("Nothing to run: the editor is empty."),
                cx,
            );
            return;
        }

        self.tabs = statements
            .into_iter()
            .map(|sql| {
                let data = Rc::new(RefCell::new(QueryTableData::default()));
                let table = new_table_state(&data, window, cx);
                ResultTab {
                    sql,
                    status: TabStatus::Pending,
                    data,
                    table,
                    error: None,
                    elapsed_ms: 0,
                    trace_id: None,
                }
            })
            .collect();
        self.active_tab = Some(0);
        // A fresh run owns the result area: drop any open cursor pager so
        // the two result pipelines never interleave rows.
        self.cursor_id = None;
        self.cursor_tab = None;
        self.cursor_has_more = false;
        self.cursor_error = None;
        self.run_stream = self.stream;
        let stream = self.stream;
        let bindings = match self.parse_bindings(cx) {
            Ok(bindings) => bindings,
            Err(message) => {
                self.params_error = Some(message.into());
                cx.notify();
                return;
            }
        };
        self.params_error = None;
        let bound = !bindings.parameters.is_empty() || !bindings.session_variables.is_empty();

        let task = cx.spawn_in(window, async move |this, cx| {
            let count = this
                .update_in(cx, |this, _, _| this.tabs.len())
                .unwrap_or_default();
            let mut failed = false;
            for ix in 0..count {
                let started = std::time::Instant::now();
                let task = this.update_in(cx, |this, _, cx| {
                    this.tabs[ix].status = TabStatus::Running;
                    this.active_tab = Some(ix);
                    cx.notify();
                    let sql = this.tabs[ix].sql.clone();
                    if stream {
                        query_service::execute_stream(cx, &sql)
                    } else if bound {
                        let bindings = bindings.clone();
                        query_service::execute_with_options(
                            cx,
                            &sql,
                            bindings.parameters,
                            bindings.session_variables,
                        )
                    } else {
                        query_service::execute(cx, &sql, false)
                    }
                });
                let Ok(task) = task else { break };
                let result = task.await;
                let elapsed = started.elapsed().as_millis() as u64;

                let proceed = this.update_in(cx, |this, window, cx| {
                    this.finish_statement(ix, result, elapsed, window, cx)
                });
                match proceed {
                    Ok(true) => continue,
                    Ok(false) => {
                        failed = true;
                        break;
                    }
                    Err(_) => break,
                }
            }

            this.update_in(cx, |this, window, cx| {
                this.running = None;
                this.finish_run(failed, window, cx);
            })
            .ok();
        });
        self.running = Some(task);
        cx.notify();
    }

    /// Store one statement outcome. Returns whether the next statement should
    /// still run.
    fn finish_statement(
        &mut self,
        ix: usize,
        result: Result<linkrs_client::QueryResult, ClientError>,
        elapsed_ms: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return false;
        };
        tab.elapsed_ms = elapsed_ms;
        let path = if self.run_stream {
            HistoryPath::Stream
        } else {
            HistoryPath::Materialized
        };

        match result {
            Ok(result) => match result.error {
                None => {
                    let row_count = result.row_count;
                    let trace_id = result.trace_id.clone();
                    *tab.data.borrow_mut() = QueryTableData::from_result(&result);
                    tab.status = TabStatus::Ok;
                    tab.trace_id = trace_id.clone();
                    tab.table.update(cx, |_, cx| cx.notify());
                    settings::record_query(
                        cx,
                        QueryHistoryItem {
                            sql: tab.sql.clone(),
                            elapsed_ms,
                            row_count,
                            success: true,
                            path,
                            error_code: None,
                            trace_id,
                            timestamp_ms: settings::now_ms(),
                        },
                    );
                    true
                }
                Some(error) => {
                    let code = error.code.clone();
                    tab.error = Some(error);
                    tab.status = TabStatus::Failed;
                    settings::record_query(
                        cx,
                        QueryHistoryItem {
                            sql: tab.sql.clone(),
                            elapsed_ms,
                            row_count: 0,
                            success: false,
                            path,
                            error_code: Some(code),
                            trace_id: None,
                            timestamp_ms: settings::now_ms(),
                        },
                    );
                    false
                }
            },
            Err(error) => {
                if connection_service::is_session_expired(&error) {
                    connection_service::invalidate_session(cx);
                }
                tab.error = Some(QueryErrorInfo {
                    code: "CLIENT_ERROR".into(),
                    message: error.to_string(),
                    details: None,
                });
                tab.status = TabStatus::Failed;
                settings::record_query(
                    cx,
                    QueryHistoryItem {
                        sql: tab.sql.clone(),
                        elapsed_ms,
                        row_count: 0,
                        success: false,
                        path,
                        error_code: Some("CLIENT_ERROR".to_string()),
                        trace_id: None,
                        timestamp_ms: settings::now_ms(),
                    },
                );
                window.push_notification(
                    Notification::error(user_message(&error)).title("Query failed"),
                    cx,
                );
                false
            }
        }
    }

    /// Called once after the run loop ends (completed or aborted on error).
    fn finish_run(&mut self, failed: bool, window: &mut Window, cx: &mut Context<Self>) {
        if failed {
            cx.notify();
            return;
        }
        let (rows, elapsed) = self.tabs.iter().fold((0usize, 0u64), |(rows, time), tab| {
            (rows + tab.data.borrow().row_count(), time + tab.elapsed_ms)
        });
        window.push_notification(
            Notification::success(format!(
                "{} statement(s) finished, {} row(s) in {elapsed} ms",
                self.tabs.len(),
                rows
            ))
            .title("Run complete"),
            cx,
        );
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.running.take().is_some() {
            for tab in &mut self.tabs {
                if tab.status == TabStatus::Running || tab.status == TabStatus::Pending {
                    tab.status = TabStatus::Pending;
                }
            }
            cx.notify();
        }
    }

    // -- Validate / explain / running queries / profile ---------------------

    /// First editor statement; the single-statement helpers all target it.
    /// Parse the parameter bindings: empty input means no bindings,
    /// otherwise a JSON object of name to value.
    fn parse_bindings(
        &self,
        cx: &App,
    ) -> Result<ParsedBindings, String> {
        let parse = |field: &Entity<InputState>, what: &str| {
            let raw = field.read(cx).value().trim().to_string();
            if raw.is_empty() {
                return Ok(std::collections::HashMap::new());
            }
            let value: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|_| format!("{what} must be a JSON object."))?;
            value
                .as_object()
                .map(|object| object.iter().map(|(key, item)| (key.clone(), item.clone())).collect())
                .ok_or_else(|| format!("{what} must be a JSON object."))
        };
        Ok(ParsedBindings {
            parameters: parse(&self.params, "Parameters")?,
            session_variables: parse(&self.session_vars, "Session variables")?,
        })
    }

    fn first_statement(&self, cx: &App) -> Option<String> {
        query_service::split_statements(&self.editor.read(cx).text().to_string())
            .into_iter()
            .next()
    }

    fn inspect_result(
        &mut self,
        title: String,
        outcome: Result<serde_json::Value, ClientError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.inspect_loading = false;
        match outcome {
            Ok(payload) => {
                self.inspect = Some(InspectOutput { title, payload });
            }
            Err(error) => {
                if connection_service::is_session_expired(&error) {
                    connection_service::invalidate_session(cx);
                }
                window.push_notification(
                    Notification::error(user_message(&error)).title("Inspection failed"),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Validate the first statement without executing it.
    fn validate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sql) = self.first_statement(cx) else {
            window.push_notification(
                Notification::info("The editor holds no statement to validate."),
                cx,
            );
            return;
        };
        self.inspect_loading = true;
        cx.notify();
        let task = query_service::validate(cx, &sql);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                this.inspect_result("Validate".to_string(), outcome, window, cx);
            });
        })
        .detach();
    }

    /// Explain the first statement and open the plan as a new result tab.
    fn explain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let Some(sql) = self.first_statement(cx) else {
            window.push_notification(
                Notification::info("The editor holds no statement to explain."),
                cx,
            );
            return;
        };
        self.inspect_loading = true;
        cx.notify();
        let task = query_service::explain(cx, &sql);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                this.inspect_loading = false;
                match outcome {
                    Ok(result) => {
                        let label = format!("EXPLAIN {sql}");
                        let data = Rc::new(RefCell::new(QueryTableData::from_result(&result)));
                        let table = new_table_state(&data, window, cx);
                        let trace_id = result.trace_id.clone();
                        this.tabs.push(ResultTab {
                            sql: label,
                            status: TabStatus::Ok,
                            data,
                            table,
                            error: result.error,
                            elapsed_ms: result.execution_time_ms,
                            trace_id,
                        });
                        this.active_tab = Some(this.tabs.len() - 1);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        window.push_notification(
                            Notification::error(user_message(&error)).title("Explain failed"),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Load the server-side running-query list into the inspector.
    fn load_running(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.running_open = true;
        self.inspect_loading = true;
        cx.notify();
        let task = query_service::list_running(cx);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                this.inspect_loading = false;
                match outcome {
                    Ok(payload) => this.running_payload = payload,
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        window.push_notification(
                            Notification::error(user_message(&error))
                                .title("Running queries failed"),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Terminate one running query by (session id, query id).
    fn kill_running_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session_id: Option<i64> = self.kill_session.read(cx).value().trim().parse().ok();
        let query_id: Option<i64> = self.kill_query.read(cx).value().trim().parse().ok();
        let (Some(session_id), Some(query_id)) = (session_id, query_id) else {
            window.push_notification(
                Notification::info("Enter numeric session and query ids first."),
                cx,
            );
            return;
        };
        let task = query_service::kill_running(cx, session_id, query_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(payload) => {
                        this.running_payload = payload;
                        window.push_notification(
                            Notification::success(format!(
                                "Terminated query {query_id} on session {session_id}."
                            )),
                            cx,
                        );
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        window.push_notification(
                            Notification::error(user_message(&error)).title("Terminate failed"),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetch the server-side portrait for the active tab's trace id.
    fn profile_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let trace_id = self
            .active_tab
            .and_then(|ix| self.tabs.get(ix))
            .and_then(|tab| tab.trace_id.clone());
        let Some(trace_id) = trace_id else {
            window.push_notification(Notification::info("The active result has no trace id."), cx);
            return;
        };
        self.inspect_loading = true;
        cx.notify();
        let task = query_service::profile(cx, &trace_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task
                .await
                .map(|detail| serde_json::to_value(&detail).unwrap_or(serde_json::Value::Null));
            _ = this.update_in(window, |this, window, cx| {
                this.inspect_result(format!("Profile {trace_id}"), outcome, window, cx);
            });
        })
        .detach();
    }

    // -- Favorites -----------------------------------------------------------

    /// Save the editor content under the entered favorite name.
    fn save_favorite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.fav_name.read(cx).value().to_string();
        let sql = self.editor.read(cx).text().to_string();
        match settings::add_favorite(cx, &name, &sql, self.fav_path) {
            Ok(()) => {
                self.fav_name.update(cx, |state, cx| {
                    state.set_value(String::new(), window, cx);
                });
                cx.notify();
            }
            Err(message) => {
                window.push_notification(Notification::error(message).title("Save favorite"), cx)
            }
        }
    }

    /// Load a history entry or favorite into the editor, optionally running it.
    fn load_sql(
        &mut self,
        sql: &str,
        run_after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |state, cx| {
            state.set_value(sql.to_string(), window, cx);
        });
        self.history_open = false;
        self.favorites_open = false;
        cx.notify();
        if run_after {
            self.run(window, cx);
        }
    }

    // -- Cursor pager --------------------------------------------------------

    /// Open a forward-only cursor over the first statement of the editor.
    /// The first page becomes its own result tab; further pages append.
    fn open_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cursor_id.is_some() || self.cursor_loading {
            return;
        }
        let script = self.editor.read(cx).text().to_string();
        let mut statements = query_service::split_statements(&script);
        if statements.is_empty() {
            self.cursor_error = Some("The editor holds no statement to page.".into());
            cx.notify();
            return;
        }
        let first = statements.remove(0);
        self.cursor_loading = true;
        self.cursor_error = None;
        cx.notify();

        let task = cursor_service::open(cx, &first);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                this.cursor_loading = false;
                match outcome {
                    Ok((id, columns)) => {
                        this.cursor_id = Some(id);
                        this.start_cursor_tab(&columns, window, cx);
                        this.fetch_cursor_page(window, cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.cursor_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Create the empty cursor result tab with the cursor column header.
    fn start_cursor_tab(
        &mut self,
        columns: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let data = Rc::new(RefCell::new(QueryTableData::from_columns_rows(
            columns.to_vec(),
            Vec::new(),
        )));
        let table = new_table_state(&data, window, cx);
        let ix = self.tabs.len();
        self.tabs.push(ResultTab {
            sql: "-- cursor".to_string(),
            status: TabStatus::Running,
            data,
            table,
            error: None,
            elapsed_ms: 0,
            trace_id: None,
        });
        self.cursor_tab = Some(ix);
        self.active_tab = Some(ix);
        cx.notify();
    }

    /// Fetch the next cursor page and append it to the cursor result tab.
    fn fetch_cursor_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.cursor_id else {
            return;
        };
        if self.cursor_loading {
            return;
        }
        self.cursor_loading = true;
        cx.notify();
        let task = cursor_service::fetch(cx, id, CURSOR_PAGE_SIZE);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                this.cursor_loading = false;
                match outcome {
                    Ok((result, has_more)) => {
                        this.cursor_has_more = has_more;
                        if let Some(ix) = this.cursor_tab {
                            if let Some(tab) = this.tabs.get_mut(ix) {
                                tab.data.borrow_mut().extend_from_result(&result);
                                tab.status = TabStatus::Ok;
                                tab.table.update(cx, |_, cx| cx.notify());
                                this.active_tab = Some(ix);
                            }
                        }
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.cursor_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Release the server-side cursor. Rows already fetched stay visible.
    fn close_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.cursor_id.take() else {
            return;
        };
        self.cursor_tab = None;
        self.cursor_has_more = false;
        self.cursor_loading = false;
        cx.notify();
        let task = cursor_service::close(cx, id);
        cx.spawn(async move |_this, _cx| {
            let _ = task.await;
        })
        .detach();
    }

    // -- Export --------------------------------------------------------------

    fn export(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active_tab else {
            return;
        };
        let Some(tab) = self.tabs.get(active) else {
            return;
        };
        if tab.status != TabStatus::Ok {
            window.push_notification(
                Notification::info("Run the statement successfully before exporting."),
                cx,
            );
            return;
        }
        let (columns, rows) = {
            let data = tab.data.borrow();
            (data.columns().to_vec(), data.rows().to_vec())
        };
        if rows.is_empty() {
            window.push_notification(Notification::info("The result set is empty."), cx);
            return;
        }

        let suggested = match format {
            ExportFormat::Csv => "result.csv",
            ExportFormat::Json => "result.json",
        };
        let directory = export_directory();
        let receiver = cx.prompt_for_new_path(&directory, Some(suggested));

        cx.spawn_in(window, async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(path))) => path,
                _ => return,
            };
            let content = match format {
                ExportFormat::Csv => to_csv(&columns, &rows),
                ExportFormat::Json => to_json(&columns, &rows),
            };
            let written = std::fs::write(&path, content);
            let _ = this.update_in(cx, |_, window, cx| match written {
                Ok(()) => window.push_notification(
                    Notification::success(format!("Exported to {}", path.display())),
                    cx,
                ),
                Err(err) => window.push_notification(
                    Notification::error(format!("Export failed: {err}")).title("Export"),
                    cx,
                ),
            });
        })
        .detach();
    }

    // -- Rendering helpers ---------------------------------------------------

    fn tab_label(sql: &str) -> String {
        let first_line = sql.lines().next().unwrap_or(sql);
        let mut label: String = first_line.chars().take(32).collect();
        if first_line.chars().count() > 32 {
            label.push('…');
        }
        label
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running.is_some();
        let history_open = self.history_open;
        let favorites_open = self.favorites_open;
        let running_open = self.running_open;
        let inspect_busy = self.inspect_loading;
        let stream = self.stream;
        let cursor_open = self.cursor_id.is_some();
        let cursor_busy = self.cursor_loading;
        let cursor_more = self.cursor_has_more;
        let theme = cx.theme().clone();

        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .when(!running, |bar| {
                bar.child(
                    Button::new("run")
                        .label("Run")
                        .icon(IconName::Play)
                        .primary()
                        .xsmall()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run(window, cx);
                        })),
                )
            })
            .when(running, |bar| {
                bar.child(
                    Button::new("cancel")
                        .label("Cancel")
                        .icon(IconName::CircleX)
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel(cx);
                        })),
                )
            })
            .child(
                Button::new("export-csv")
                    .label("CSV")
                    .icon(IconName::Download)
                    .xsmall()
                    .disabled(running)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.export(ExportFormat::Csv, window, cx);
                    })),
            )
            .child(
                Button::new("export-json")
                    .label("JSON")
                    .icon(IconName::Download)
                    .xsmall()
                    .disabled(running)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.export(ExportFormat::Json, window, cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("toggle-stream")
                    .label("Stream")
                    .icon(IconName::Zap)
                    .xsmall()
                    .ghost()
                    .selected(stream)
                    .disabled(running)
                    .accessibility_label("Run through the SSE streaming endpoint")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stream = !this.stream;
                        cx.notify();
                    })),
            )
            .when(!cursor_open, |bar| {
                bar.child(
                    Button::new("cursor-open")
                        .label("Cursor")
                        .icon(IconName::Database)
                        .xsmall()
                        .ghost()
                        .disabled(running || cursor_busy)
                        .accessibility_label("Open a server cursor for large results")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_cursor(window, cx);
                        })),
                )
            })
            .when(cursor_open, |bar| {
                bar.child(
                    Button::new("cursor-next")
                        .label(if cursor_more { "Next page" } else { "Done" })
                        .icon(IconName::ChevronRight)
                        .xsmall()
                        .disabled(cursor_busy || !cursor_more)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.fetch_cursor_page(window, cx);
                        })),
                )
                .child(
                    Button::new("cursor-close")
                        .label("Close cursor")
                        .icon(IconName::X)
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_cursor(cx);
                        })),
                )
            })
            .child(
                Button::new("validate")
                    .label("Validate")
                    .icon(IconName::Check)
                    .xsmall()
                    .ghost()
                    .disabled(running || inspect_busy)
                    .accessibility_label("Validate the first statement without executing")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.validate(window, cx);
                    })),
            )
            .child(
                Button::new("explain")
                    .label("Explain")
                    .icon(IconName::Info)
                    .xsmall()
                    .ghost()
                    .disabled(running || inspect_busy)
                    .accessibility_label("Explain the first statement into a new tab")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.explain(window, cx);
                    })),
            )
            .child(
                Button::new("profile")
                    .label("Profile")
                    .icon(IconName::Activity)
                    .xsmall()
                    .ghost()
                    .disabled(running || inspect_busy)
                    .accessibility_label("Fetch the server portrait for the active result")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.profile_active(window, cx);
                    })),
            )
            .child(
                Button::new("toggle-running")
                    .label("Running")
                    .icon(IconName::List)
                    .xsmall()
                    .ghost()
                    .selected(running_open)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.running_open {
                            this.running_open = false;
                            cx.notify();
                        } else {
                            this.load_running(window, cx);
                        }
                    })),
            )
            .child(
                Button::new("toggle-history")
                    .label("History")
                    .icon(IconName::Clock)
                    .xsmall()
                    .ghost()
                    .selected(history_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.history_open = !this.history_open;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("toggle-favorites")
                    .label("Favorites")
                    .icon(IconName::Star)
                    .xsmall()
                    .ghost()
                    .selected(favorites_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.favorites_open = !this.favorites_open;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("toggle-params")
                    .label("Params")
                    .icon(IconName::Braces)
                    .xsmall()
                    .ghost()
                    .selected(self.params_open)
                    .disabled(running)
                    .accessibility_label("Query parameters and session variables")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.params_open = !this.params_open;
                        cx.notify();
                    })),
            )
    }

    fn render_params(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let params_error = self.params_error.clone();
        v_flex()
            .flex_shrink_0()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(form_row("Parameters", self.params.clone(), cx))
            .child(form_row("Session vars", self.session_vars.clone(), cx))
            .when_some(params_error, |panel, message| {
                panel.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(message),
                )
            })
    }

    /// One history row: status icon, statement text (click loads), outcome
    /// summary and a rerun button. The text area and the button are siblings
    /// so load and rerun never both fire for one click.
    fn history_row(
        item: QueryHistoryItem,
        ix: usize,
        view: &Entity<Self>,
        theme: &gpui_kit::component::Theme,
    ) -> impl IntoElement {
        let load_sql = item.sql.clone();
        let rerun_sql = item.sql.clone();
        let path = item.path;
        let summary = if item.success {
            format!("{} rows · {} ms", item.row_count, item.elapsed_ms)
        } else {
            item.error_code
                .clone()
                .unwrap_or_else(|| "failed".to_string())
        };
        let view_load = view.clone();
        let view_rerun = view.clone();
        let status_icon = if item.success {
            Icon::new(IconName::Check).text_color(theme.success)
        } else {
            Icon::new(IconName::CircleX).text_color(theme.danger)
        };
        h_flex()
            .gap_1()
            .items_center()
            .px_1()
            .py_1()
            .child(
                div()
                    .id(("history-load", ix))
                    .flex_1()
                    .min_w_0()
                    .on_click(move |_, window, cx| {
                        let sql = load_sql.clone();
                        view_load.update(cx, |this, cx| {
                            this.load_sql(&sql, false, window, cx);
                        });
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .min_w_0()
                            .child(status_icon)
                            .child(
                                div()
                                    .text_sm()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(ConsolePanel::tab_label(&item.sql)),
                            ),
                    )
                    .child(div().text_xs().child(SharedString::from(summary))),
            )
            .child(
                Button::new(("history-rerun", ix))
                    .icon(IconName::Play)
                    .xsmall()
                    .ghost()
                    .accessibility_label("Load and rerun this statement")
                    .on_click(move |_, window, cx| {
                        let sql = rerun_sql.clone();
                        view_rerun.update(cx, |this, cx| {
                            if path == HistoryPath::Stream {
                                this.stream = true;
                            }
                            this.load_sql(&sql, true, window, cx);
                        });
                    }),
            )
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let history = settings::global(cx).query_history;
        let view = cx.entity();

        v_flex()
            .w(px(300.))
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!(
                                "History ({} entries, newest first)",
                                history.len()
                            ))),
                    )
                    .child(
                        Button::new("history-clear")
                            .label("Clear")
                            .xsmall()
                            .ghost()
                            .disabled(history.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                settings::clear_history(cx);
                                this.history_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("history-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .children(
                        history
                            .iter()
                            .enumerate()
                            .map(|(ix, item)| Self::history_row(item.clone(), ix, &view, &theme)),
                    )
                    .when(history.is_empty(), |list| {
                        list.child(empty_state(
                            IconName::Clock,
                            "No history yet",
                            Some("Executed statements land here with timings.".into()),
                            cx,
                        ))
                    }),
            )
    }

    fn render_favorites(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let favorites = settings::global(cx).favorites;
        let view = cx.entity();
        let fav_path = self.fav_path;

        let path_button = |id: &'static str, label: &'static str, value: Option<HistoryPath>| {
            Button::new(id)
                .label(label)
                .xsmall()
                .ghost()
                .selected(fav_path == value)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.fav_path = value;
                    cx.notify();
                }))
                .into_any_element()
        };

        v_flex()
            .w(px(300.))
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!(
                        "Favorites ({} saved)",
                        favorites.len()
                    ))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .px_2()
                    .pb_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(form_row("Name", self.fav_name.clone(), cx))
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from("Rerun path")),
                            )
                            .child(path_button("fav-path-auto", "Auto", None))
                            .child(path_button(
                                "fav-path-materialized",
                                "Results",
                                Some(HistoryPath::Materialized),
                            ))
                            .child(path_button(
                                "fav-path-stream",
                                "Stream",
                                Some(HistoryPath::Stream),
                            )),
                    )
                    .child(
                        Button::new("fav-save")
                            .label("Save current editor")
                            .icon(IconName::Star)
                            .xsmall()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_favorite(window, cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("favorites-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .children(favorites.iter().enumerate().map(|(ix, fav)| {
                        let load_sql = fav.sql.clone();
                        let rerun_sql = fav.sql.clone();
                        let preferred = fav.preferred_path;
                        let name = fav.name.clone();
                        let delete_name = fav.name.clone();
                        let view_load = view.clone();
                        let view_rerun = view.clone();
                        let view_delete = view.clone();
                        h_flex()
                            .gap_1()
                            .items_center()
                            .px_1()
                            .py_1()
                            .child(
                                div()
                                    .id(("fav-load", ix))
                                    .flex_1()
                                    .min_w_0()
                                    .on_click(move |_, window, cx| {
                                        let sql = load_sql.clone();
                                        view_load.update(cx, |this, cx| {
                                            this.load_sql(&sql, false, window, cx);
                                        });
                                    })
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .min_w_0()
                                            .child(
                                                Icon::new(IconName::Star)
                                                    .text_color(theme.muted_foreground),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(SharedString::from(name)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(ConsolePanel::tab_label(&fav.sql)),
                                    ),
                            )
                            .child(
                                Button::new(("fav-rerun", ix))
                                    .icon(IconName::Play)
                                    .xsmall()
                                    .ghost()
                                    .accessibility_label("Load and rerun this favorite")
                                    .on_click(move |_, window, cx| {
                                        let sql = rerun_sql.clone();
                                        view_rerun.update(cx, |this, cx| {
                                            match preferred {
                                                Some(HistoryPath::Stream) => {
                                                    this.stream = true;
                                                }
                                                Some(HistoryPath::Materialized) => {
                                                    this.stream = false;
                                                }
                                                None => {}
                                            }
                                            this.load_sql(&sql, true, window, cx);
                                        });
                                    }),
                            )
                            .child(
                                Button::new(("fav-delete", ix))
                                    .icon(IconName::Trash)
                                    .xsmall()
                                    .ghost()
                                    .accessibility_label("Delete this favorite")
                                    .on_click(move |_, _, cx| {
                                        let name = delete_name.clone();
                                        view_delete.update(cx, |_, cx| {
                                            settings::remove_favorite(cx, &name);
                                            cx.notify();
                                        });
                                    }),
                            )
                    }))
                    .when(favorites.is_empty(), |list| {
                        list.child(empty_state(
                            IconName::Star,
                            "No favorites yet",
                            Some("Name the current editor above to save it.".into()),
                            cx,
                        ))
                    }),
            )
    }

    fn render_inspect(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.inspect_loading && self.inspect.is_none() {
            return div()
                .flex_shrink_0()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from("Loading…"))
                .into_any_element();
        }
        let Some(inspect) = self.inspect.clone() else {
            return div().into_any_element();
        };
        v_flex()
            .flex_shrink_0()
            .h(px(220.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .child(SharedString::from(inspect.title)),
                    )
                    .child(
                        Button::new("inspect-close")
                            .icon(IconName::X)
                            .xsmall()
                            .ghost()
                            .accessibility_label("Close inspector")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspect = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .child(render_json_block(&inspect.payload, cx)),
            )
            .into_any_element()
    }

    fn render_running(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.running_open {
            return div().into_any_element();
        }
        let theme = cx.theme().clone();
        v_flex()
            .flex_shrink_0()
            .h(px(220.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .child(SharedString::from("Running queries")),
                    )
                    .child(
                        Button::new("running-refresh")
                            .icon(IconName::RefreshCw)
                            .xsmall()
                            .ghost()
                            .accessibility_label("Refresh running queries")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_running(window, cx);
                            })),
                    )
                    .child(
                        Button::new("running-close")
                            .icon(IconName::X)
                            .xsmall()
                            .ghost()
                            .accessibility_label("Close running queries")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.running_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .px_2()
                    .child(form_row("Session id", self.kill_session.clone(), cx))
                    .child(form_row("Query id", self.kill_query.clone(), cx))
                    .child(
                        Button::new("running-kill")
                            .label("Terminate")
                            .icon(IconName::Ban)
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.kill_running_query(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .child(render_json_block(&self.running_payload, cx)),
            )
            .into_any_element()
    }

    fn render_results(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        if self.tabs.is_empty() {
            return div().flex_1().min_h_0().child(empty_state(
                IconName::Terminal,
                "No results yet",
                Some("Write a statement and press Run (Ctrl+Enter).".into()),
                cx,
            ));
        }

        let active = self.active_tab.unwrap_or(0).min(self.tabs.len() - 1);
        let tabs = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let label = ConsolePanel::tab_label(&tab.sql);
            let status = tab.status;
            Tab::new()
                .label(label)
                .selected(ix == active)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_tab = Some(ix);
                    cx.notify();
                }))
                .when(status == TabStatus::Running, |tab| {
                    tab.suffix(Icon::new(IconName::Loader).text_color(theme.warning))
                })
                .when(status == TabStatus::Failed, |tab| {
                    tab.suffix(Icon::new(IconName::CircleX).text_color(theme.danger))
                })
        });

        let active_sql_empty = self.tabs[active].data.borrow().columns().is_empty();
        let active_status = self.tabs[active].status;
        let active_table = self.tabs[active].table.clone();
        let summary = match active_status {
            TabStatus::Pending => "pending".to_string(),
            TabStatus::Running => "running…".to_string(),
            TabStatus::Ok => format!(
                "{} rows · {} ms",
                self.tabs[active].data.borrow().row_count(),
                self.tabs[active].elapsed_ms
            ),
            TabStatus::Failed => "failed".to_string(),
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                TabBar::new("result-tabs")
                    .border_b_1()
                    .border_color(theme.border)
                    .children(tabs)
                    .selected_index(active),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .justify_end()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(summary)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(active_status == TabStatus::Failed, |host| {
                        host.child(self.render_error(active, cx))
                    })
                    .when(
                        active_status != TabStatus::Failed && !active_sql_empty,
                        |host| {
                            host.child(
                                div()
                                    .size_full()
                                    .p_1()
                                    .child(DataTable::new(&active_table).stripe(true)),
                            )
                        },
                    )
                    .when(
                        active_status != TabStatus::Failed && active_sql_empty,
                        |host| {
                            host.child(empty_state(
                                IconName::Inbox,
                                "No rows",
                                Some("The statement produced no result set.".into()),
                                cx,
                            ))
                        },
                    ),
            )
    }

    fn render_error(&mut self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let error = self.tabs[ix].error.clone();
        v_flex()
            .flex_1()
            .gap_2()
            .p_4()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::CircleX).text_color(theme.danger))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(format!("Statement {} failed", ix + 1))),
                    ),
            )
            .when_some(error, |panel, error| {
                panel
                    .child(
                        h_flex().gap_2().items_center().child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded(theme.radius)
                                .bg(theme.danger.opacity(0.12))
                                .text_xs()
                                .text_color(theme.danger)
                                .child(error.code.clone()),
                        ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(SharedString::from(error.message.clone())),
                    )
                    .when_some(error.details.clone(), |panel, details| {
                        panel.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(SharedString::from(details)),
                        )
                    })
            })
    }
}

impl Focusable for ConsolePanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for ConsolePanel {}

impl gpui_kit::base::dock::Panel for ConsolePanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for ConsolePanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for ConsolePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        v_flex()
            .id("console-panel")
            .key_context("Console")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(Self::on_run_query))
            .on_action(cx.listener(Self::on_cancel_query))
            .on_action(cx.listener(Self::on_copy_row))
            .child(self.render_toolbar(cx))
            .when(self.params_open, |panel| panel.child(self.render_params(cx)))
            .when_some(self.cursor_error.clone(), |panel, message| {
                panel.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(theme.danger.opacity(0.08))
                        .child(Icon::new(IconName::TriangleAlert).text_color(theme.danger))
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(message),
                        ),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.history_open, |row| row.child(self.render_history(cx)))
                    .when(self.favorites_open, |row| {
                        row.child(self.render_favorites(cx))
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .h(px(EDITOR_HEIGHT))
                                    .flex_shrink_0()
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .child(Editor::new(&self.editor)),
                            )
                            .child(self.render_inspect(cx))
                            .child(self.render_running(cx))
                            .child(self.render_results(cx)),
                    ),
            )
    }
}

/// Destination format for [`ConsolePanel::export`].
#[derive(Clone, Copy)]
enum ExportFormat {
    Csv,
    Json,
}

/// Short, user-facing sentence for transport-level failures.
fn user_message(error: &ClientError) -> String {
    match error {
        ClientError::NotConnected => "The session is no longer connected.".into(),
        ClientError::NoSpaceSelected => "Select a space before running this statement.".into(),
        _ => error.to_string(),
    }
}

/// Best-effort default directory for the save dialog.
fn export_directory() -> PathBuf {
    directories::UserDirs::new()
        .map(|dirs| {
            dirs.document_dir()
                .or(dirs.download_dir())
                .unwrap_or(dirs.home_dir())
                .to_path_buf()
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Serialize one result set as RFC4180 CSV (headers included).
fn to_csv(columns: &[String], rows: &[Vec<String>]) -> String {
    let mut buffer = Vec::new();
    {
        let mut writer = csv::Writer::from_writer(&mut buffer);
        let _ = writer.write_record(columns);
        for row in rows {
            let _ = writer.write_record(row);
        }
        let _ = writer.flush();
    }
    String::from_utf8(buffer).unwrap_or_default()
}

/// Serialize one result set as a JSON array of objects. Cells that look like
/// JSON keep their shape; `NULL` cells become JSON null.
fn to_json(columns: &[String], rows: &[Vec<String>]) -> String {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut object = serde_json::Map::new();
            for (ix, name) in columns.iter().enumerate() {
                let cell = row.get(ix).map(String::as_str).unwrap_or("NULL");
                let value = if cell == "NULL" {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str(cell)
                        .unwrap_or_else(|_| serde_json::Value::String(cell.into()))
                };
                object.insert(name.clone(), value);
            }
            serde_json::Value::Object(object)
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}
