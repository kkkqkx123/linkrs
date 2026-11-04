use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState, Input, InputState},
    notification::Notification,
    tab::{Tab, TabBar},
    table::{DataTable, TableState},
    v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
};
use gpui_kit::Div;
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
use crate::i18n::{t, tf};
use crate::services::{
    completion_service::{LinkrsCompletionProvider, SchemaSnapshot, SharedSchemaSnapshot},
    connection_service, cursor_service, graph_service, query_service, schema_service,
};
use crate::settings::{self, ExecutionPreference, HistoryPath, QueryHistoryItem};
use crate::state::AppState;

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
    /// Structured rows kept for result-to-graph imports; the table above
    /// only holds the flattened display strings.
    raw_rows: Vec<std::collections::HashMap<String, serde_json::Value>>,
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

/// Query console panel: SQL editor, multi-statement execution with
/// materialized/stream/auto routing, result tabs, CSV/JSON export,
/// and the local query history.
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
    /// Console execution preference; `Auto` decides per script.
    execution: ExecutionPreference,
    /// Row threshold for automatic stream routing.
    auto_threshold: Entity<InputState>,
    /// Last automatic routing outcome shown in the toolbar.
    auto_decision: Option<String>,
    /// Forward-only cursor pager state (`None` when no cursor is open).
    cursor_id: Option<u64>,
    cursor_tab: Option<usize>,
    cursor_has_more: bool,
    cursor_loading: bool,
    cursor_error: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
    /// Schema names feeding statement completion, refreshed on space
    /// switches and after schema-changing runs.
    completion_schema: SharedSchemaSnapshot,
    /// Space the completion snapshot was taken from.
    completion_space: Option<String>,
}

impl ConsolePanel {
    pub(crate) const PANEL_NAME: &'static str = "Console";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&AppState::global(cx), |this, _, cx| {
            let space = AppState::global(cx)
                .read(cx)
                .space
                .clone()
                .map(|name| name.to_string());
            if space != this.completion_space {
                this.refresh_completion_snapshot(cx);
            }
            cx.notify();
        })
        .detach();
        let completion_schema = SharedSchemaSnapshot::new();
        let provider_schema = completion_schema.clone();
        let editor = cx.new(|cx| EditorState::new(window, cx).language("linkrs"));
        editor.update(cx, |state, _| {
            // Mount through the editor's public LSP hook: the trait is
            // transport-agnostic (text plus offset in, items out), so a local
            // source needs no language server behind it.
            state.lsp_mut().completion_provider =
                Some(Rc::new(LinkrsCompletionProvider::new(provider_schema)));
        });
        let fav_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("console.favoriteName", crate::i18n::locale(cx)))
        });
        let kill_session = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("console.sessionId", crate::i18n::locale(cx)))
        });
        let kill_query = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("console.queryId", crate::i18n::locale(cx)))
        });
        let params = cx.new(|cx| InputState::new(window, cx).placeholder("{\"name\": value}"));
        let session_vars =
            cx.new(|cx| InputState::new(window, cx).placeholder("{\"name\": value}"));
        let mut panel = Self {
            editor,
            tabs: Vec::new(),
            active_tab: None,
            running: None,
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
            execution: settings::global(cx).execution_preference,
            auto_threshold: cx.new(|cx| {
                let saved = settings::global(cx).auto_stream_threshold;
                let mut input = InputState::new(window, cx).placeholder("1000");
                input.set_value(&saved.to_string(), window, cx);
                input
            }),
            auto_decision: None,
            cursor_id: None,
            cursor_tab: None,
            cursor_has_more: false,
            cursor_loading: false,
            cursor_error: None,
            focus_handle: cx.focus_handle(),
            completion_schema,
            completion_space: None,
        };
        panel.refresh_completion_snapshot(cx);
        panel
    }

    /// Reload tag and edge type names for completion. Skipped without a
    /// selected space or when the snapshot already matches it; failed
    /// fetches clear the marker so the next trigger retries.
    fn refresh_completion_snapshot(&mut self, cx: &mut Context<Self>) {
        let space = AppState::global(cx)
            .read(cx)
            .space
            .clone()
            .map(|name| name.to_string());
        let Some(space) = space else {
            return;
        };
        if self.completion_space.as_deref() == Some(space.as_str()) {
            return;
        }
        self.completion_space = Some(space.clone());
        let tags = schema_service::tags(cx);
        let edges = schema_service::edge_types(cx);
        let handle = self.completion_schema.clone();
        cx.spawn(async move |this, cx| {
            let tags = tags.await;
            let edges = edges.await;
            this.update(cx, |this, _| match (tags, edges) {
                (Ok(tags), Ok(edges)) => {
                    handle.store(SchemaSnapshot::from_wire(&tags, &edges));
                }
                _ => {
                    this.completion_space = None;
                }
            })
            .ok();
        })
        .detach();
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

    /// Flip the streaming execution preference; used by the command palette.
    /// `Auto` resolves to an explicit choice so the toggle stays binary.
    pub(crate) fn toggle_stream(&mut self, cx: &mut Context<Self>) {
        self.execution = match self.execution {
            ExecutionPreference::Stream => ExecutionPreference::Materialized,
            _ => ExecutionPreference::Stream,
        };
        settings::update(cx, |settings| {
            settings.execution_preference = self.execution;
        });
        cx.notify();
    }

    /// Persist the console execution preference.
    fn set_execution(&mut self, preference: ExecutionPreference, cx: &mut Context<Self>) {
        self.execution = preference;
        settings::update(cx, |settings| {
            settings.execution_preference = preference;
        });
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

    /// Run the whole editor script, one result tab per statement.
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let script = self.editor.read(cx).text().to_string();
        let statements = query_service::split_statements(&script);
        if statements.is_empty() {
            let locale = AppState::global(cx).read(cx).locale;
            window.push_notification(Notification::info(t("console.emptyEditor", locale)), cx);
            return;
        }
        self.start_run(statements, window, cx);
    }

    /// Run the selected statement, the statement under the cursor, or the
    /// full script when the cursor sits in a gap between statements.
    fn run_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let Some(target) = self.target_statement(cx) else {
            let locale = AppState::global(cx).read(cx).locale;
            window.push_notification(Notification::info(t("console.emptyEditor", locale)), cx);
            return;
        };
        let statements = query_service::split_statements(&target);
        if statements.is_empty() {
            let locale = AppState::global(cx).read(cx).locale;
            window.push_notification(Notification::info(t("console.emptySelection", locale)), cx);
            return;
        }
        self.start_run(statements, window, cx);
    }

    /// Run pre-split statements one by one, stopping at the first failure.
    fn start_run(&mut self, statements: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
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
                    raw_rows: Vec::new(),
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
        let preference = self.execution;
        let threshold = Self::auto_threshold_value(&self.auto_threshold, cx);
        settings::update(cx, |settings| {
            settings.execution_preference = preference;
            settings.auto_stream_threshold = threshold;
        });
        self.auto_decision = None;
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
            // Decide the execution path. Explicit preferences apply to the
            // whole script; `Auto` streams multi-statement scripts, runs
            // bound single statements materially (the stream channel carries
            // no bindings) and validates lone statements for a planner
            // estimate first.
            let mut stream = preference == ExecutionPreference::Stream;
            let mut decision = None::<String>;
            if preference == ExecutionPreference::Auto {
                if count > 1 {
                    stream = true;
                    decision = Some(format!("auto \u{2192} stream ({count} statements)"));
                } else if bound {
                    stream = false;
                    decision = Some("auto \u{2192} materialized (parameters bound)".to_string());
                } else if count == 1 {
                    let validate_task = this.update_in(cx, |this, _, cx| {
                        query_service::validate(cx, &this.tabs[0].sql)
                    });
                    let outcome = match validate_task {
                        Ok(task) => task.await,
                        Err(_) => Err(ClientError::NotConnected),
                    };
                    let (choice, note) = auto_decision(&outcome, threshold);
                    stream = choice;
                    decision = Some(note);
                }
            }
            this.update_in(cx, |this, _, _| {
                this.run_stream = stream;
                this.auto_decision = decision;
            })
            .ok();
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
                    tab.raw_rows = result.rows.clone();
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
                    Notification::error(user_message(&error, crate::i18n::locale(cx)))
                        .title(t("console.queryFailed", crate::i18n::locale(cx))),
                    cx,
                );
                false
            }
        }
    }

    /// Called once after the run loop ends (completed or aborted on error).
    fn finish_run(&mut self, failed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let locale = AppState::global(cx).read(cx).locale;
        if failed {
            cx.notify();
            return;
        }
        // Schema-changing runs invalidate the completion names.
        if self
            .tabs
            .iter()
            .any(|tab| script_may_change_schema(&tab.sql))
        {
            self.completion_space = None;
            self.refresh_completion_snapshot(cx);
        }
        let (rows, elapsed) = self.tabs.iter().fold((0usize, 0u64), |(rows, time), tab| {
            (rows + tab.data.borrow().row_count(), time + tab.elapsed_ms)
        });
        window.push_notification(
            Notification::success(crate::i18n::tf_args(
                "console.multiFinished",
                locale,
                &[&self.tabs.len(), &rows, &elapsed],
            ))
            .title(t("console.runComplete", locale)),
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
    fn parse_bindings(&self, cx: &App) -> Result<ParsedBindings, String> {
        let parse = |field: &Entity<InputState>, what: &str| {
            let raw = field.read(cx).value().trim().to_string();
            if raw.is_empty() {
                return Ok(std::collections::HashMap::new());
            }
            let value: serde_json::Value =
                serde_json::from_str(&raw).map_err(|_| format!("{what} must be a JSON object."))?;
            value
                .as_object()
                .map(|object| {
                    object
                        .iter()
                        .map(|(key, item)| (key.clone(), item.clone()))
                        .collect()
                })
                .ok_or_else(|| format!("{what} must be a JSON object."))
        };
        Ok(ParsedBindings {
            parameters: parse(
                &self.params,
                t("console.parameters", crate::i18n::locale(cx)),
            )?,
            session_variables: parse(
                &self.session_vars,
                t("console.sessionVars", crate::i18n::locale(cx)),
            )?,
        })
    }

    /// Statement targeted by single-statement actions: the non-empty
    /// selection, else the statement under the cursor, else the full script.
    /// Returns `None` only when the editor holds no statement at all.
    fn target_statement(&self, cx: &App) -> Option<String> {
        let editor = self.editor.read(cx);
        let range = editor.selected_range();
        if range.start != range.end {
            let selected = editor.selected_text().to_string();
            if !selected.trim().is_empty() {
                return Some(selected.trim().to_string());
            }
        }
        let script = editor.text().to_string();
        if script.trim().is_empty() {
            return None;
        }
        let offset = editor.cursor();
        Some(query_service::statement_at_offset(&script, offset).unwrap_or(script))
    }

    /// Format the selected text, or the whole script when nothing is
    /// selected. Only layout changes; string literals and comments stay
    /// untouched.
    fn format_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (range, input, script) = {
            let editor = self.editor.read(cx);
            let script = editor.text().to_string();
            let range = editor.selected_range();
            if range.start != range.end {
                match script.get(range.clone()) {
                    Some(selected) if !selected.trim().is_empty() => {
                        (range, selected.to_string(), script)
                    }
                    _ => (0..script.len(), script.clone(), script),
                }
            } else {
                (0..script.len(), script.clone(), script)
            }
        };
        if input.trim().is_empty() {
            let locale = AppState::global(cx).read(cx).locale;
            window.push_notification(Notification::info(t("console.emptyFormat", locale)), cx);
            return;
        }
        let formatted = query_service::format_script(&input);
        if formatted == input.trim() {
            let locale = AppState::global(cx).read(cx).locale;
            window.push_notification(
                Notification::info(t("console.alreadyFormatted", locale)),
                cx,
            );
            return;
        }
        let head = script
            .get(..range.start)
            .expect("selection range comes from the same editor text");
        let tail = script
            .get(range.end..)
            .expect("selection range comes from the same editor text");
        let merged = format!("{head}{formatted}{tail}");
        self.editor.update(cx, |state, cx| {
            state.set_value(merged, window, cx);
        });
        let locale = AppState::global(cx).read(cx).locale;
        window.push_notification(Notification::info(t("console.formatted", locale)), cx);
        cx.notify();
    }

    /// Parse the auto-route row threshold, clamped to the frontend range.
    fn auto_threshold_value(field: &Entity<InputState>, cx: &App) -> u64 {
        field
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .map(|value| value.min(10_000_000))
            .unwrap_or_else(settings::default_auto_stream_threshold)
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
                    Notification::error(user_message(&error, crate::i18n::locale(cx)))
                        .title(t("console.inspectFailed", crate::i18n::locale(cx))),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Validate the targeted statement without executing it.
    fn validate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sql) = self.target_statement(cx) else {
            let locale = AppState::global(cx).read(cx).locale;
            window.push_notification(Notification::info(t("console.emptyValidate", locale)), cx);
            return;
        };
        self.inspect_loading = true;
        cx.notify();
        let task = query_service::validate(cx, &sql);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                this.inspect_result(
                    crate::i18n::t("console.validate", crate::i18n::locale(cx)).to_string(),
                    outcome,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    /// Explain the targeted statement and open the plan as a new result tab.
    fn explain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let Some(sql) = self.target_statement(cx) else {
            let locale = AppState::global(cx).read(cx).locale;
            window.push_notification(Notification::info(t("console.emptyExplain", locale)), cx);
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
                            raw_rows: Vec::new(),
                        });
                        this.active_tab = Some(this.tabs.len() - 1);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        window.push_notification(
                            Notification::error(user_message(&error, crate::i18n::locale(cx)))
                                .title(t("console.explainFailed", crate::i18n::locale(cx))),
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
                            Notification::error(user_message(&error, crate::i18n::locale(cx)))
                                .title(t("console.runningFailed", crate::i18n::locale(cx))),
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
                Notification::info(t("console.numericIds", crate::i18n::locale(cx))),
                cx,
            );
            return;
        };
        let task = query_service::kill_running(cx, session_id, query_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                let locale = crate::i18n::locale(cx);
                match outcome {
                    Ok(payload) => {
                        this.running_payload = payload;
                        window.push_notification(
                            Notification::success(
                                crate::i18n::tf_args(
                                    "console.terminatedQuery",
                                    locale,
                                    &[&query_id, &session_id],
                                ),
                            ),
                            cx,
                        );
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        window.push_notification(
                            Notification::error(user_message(&error, crate::i18n::locale(cx)))
                                .title(t("console.terminateFailed", crate::i18n::locale(cx))),
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
            window.push_notification(
                Notification::info(t("console.noTraceId", crate::i18n::locale(cx))),
                cx,
            );
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
                this.inspect_result(
                    format!(
                        "{} {trace_id}",
                        t("console.profile", crate::i18n::locale(cx))
                    ),
                    outcome,
                    window,
                    cx,
                );
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
            Err(message) => window.push_notification(
                Notification::error(message)
                    .title(t("console.saveFavoriteFailed", crate::i18n::locale(cx))),
                cx,
            ),
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
        cx.notify();
        if run_after {
            self.run(window, cx);
        }
    }

    // -- Cursor pager --------------------------------------------------------

    /// Open a forward-only cursor over the targeted statement of the editor.
    /// The first page becomes its own result tab; further pages append.
    fn open_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cursor_id.is_some() || self.cursor_loading {
            return;
        }
        let locale = AppState::global(cx).read(cx).locale;
        let Some(target) = self.target_statement(cx) else {
            self.cursor_error = Some(t("console.emptyPage", locale).into());
            cx.notify();
            return;
        };
        let mut statements = query_service::split_statements(&target);
        if statements.is_empty() {
            self.cursor_error = Some(t("console.emptyPage", locale).into());
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
            raw_rows: Vec::new(),
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
        let locale = AppState::global(cx).read(cx).locale;
        let Some(active) = self.active_tab else {
            return;
        };
        let Some(tab) = self.tabs.get(active) else {
            return;
        };
        if tab.status != TabStatus::Ok {
            window.push_notification(
                Notification::info(t("console.exportNotRun", crate::i18n::locale(cx))),
                cx,
            );
            return;
        }
        let (columns, rows) = {
            let data = tab.data.borrow();
            (data.columns().to_vec(), data.rows().to_vec())
        };
        if rows.is_empty() {
            window.push_notification(Notification::info(t("console.exportEmpty", locale)), cx);
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
            let _ = this.update_in(cx, |_, window, cx| {
                let locale = AppState::global(cx).read(cx).locale;
                match written {
                    Ok(()) => window.push_notification(
                        Notification::success(tf("console.exportedTo", locale, path.display())),
                        cx,
                    ),
                    Err(err) => window.push_notification(
                        Notification::error(tf("console.exportFailed", locale, err))
                            .title(t("console.export", locale)),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    // -- Rendering helpers ---------------------------------------------------

    /// Push the active result tab into the graph panel through the global
    /// drop box; the shell switches to the graph panel on arrival.
    fn send_to_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = AppState::global(cx).read(cx).locale;
        let tab = self.active_tab.and_then(|active| self.tabs.get(active));
        let Some(tab) = tab else {
            return;
        };
        if tab.status != TabStatus::Ok || tab.raw_rows.is_empty() {
            window.push_notification(Notification::info(t("console.sendGraphNoRows", locale)), cx);
            return;
        }
        match graph_service::neighborhood_from_rows(&tab.raw_rows) {
            Some((neighborhood, truncated)) => {
                let truncated_suffix = if truncated {
                    t("console.truncated", locale)
                } else {
                    ""
                };
                let summary = crate::i18n::tf_args(
                    "console.sentGraph",
                    locale,
                    &[
                        &neighborhood.nodes.len(),
                        &neighborhood.edges.len(),
                        &truncated_suffix,
                    ],
                );
                AppState::global(cx).update(cx, |state, cx| {
                    state.push_graph_drop(neighborhood, cx);
                });
                window.push_notification(Notification::success(summary), cx);
            }
            None => {
                window.push_notification(
                    Notification::info(t("console.sendGraphNoGraphData", locale)),
                    cx,
                );
            }
        }
    }

    fn tab_label(sql: &str) -> String {
        let first_line = sql.lines().next().unwrap_or(sql);
        let mut label: String = first_line.chars().take(32).collect();
        if first_line.chars().count() > 32 {
            label.push('…');
        }
        label
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = AppState::global(cx).read(cx).locale;
        let running = self.running.is_some();
        let inspect_busy = self.inspect_loading;
        let running_open = self.running_open;
        let preference = self.execution;
        let decision = self.auto_decision.clone();
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
                        .label(t("console.run", locale))
                        .icon(IconName::Play)
                        .primary()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run(window, cx);
                        })),
                )
            })
            .when(running, |bar| {
                bar.child(
                    Button::new("cancel")
                        .label(t("common.cancel", locale))
                        .icon(IconName::CircleX)
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel(cx);
                        })),
                )
            })
            .when(!running, |bar| {
                bar.child(
                    Button::new("run-stmt")
                        .label(t("console.runStmt", locale))
                        .icon(IconName::StepForward)
                        .small()
                        .accessibility_label(t("console.runStmtHint", locale))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run_target(window, cx);
                        })),
                )
            })
            .child(
                Button::new("format")
                    .label(t("console.format", locale))
                    .icon(IconName::Wand)
                    .xsmall()
                    .ghost()
                    .disabled(running)
                    .accessibility_label(t("console.formatHint", locale))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.format_editor(window, cx);
                    })),
            )
            .child(
                Button::new("export-csv")
                    .label(crate::i18n::t("console.exportCsv", crate::i18n::locale(cx)))
                    .icon(IconName::Download)
                    .xsmall()
                    .disabled(running)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.export(ExportFormat::Csv, window, cx);
                    })),
            )
            .child(
                Button::new("export-json")
                    .label(crate::i18n::t(
                        "console.exportJson",
                        crate::i18n::locale(cx),
                    ))
                    .icon(IconName::Download)
                    .xsmall()
                    .disabled(running)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.export(ExportFormat::Json, window, cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("pref-materialized")
                    .label(t("console.materialized", locale))
                    .xsmall()
                    .ghost()
                    .selected(preference == ExecutionPreference::Materialized)
                    .disabled(running)
                    .accessibility_label(t("console.materializedHint", locale))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_execution(ExecutionPreference::Materialized, cx);
                    })),
            )
            .child(
                Button::new("pref-stream")
                    .label(t("console.stream", locale))
                    .xsmall()
                    .ghost()
                    .selected(preference == ExecutionPreference::Stream)
                    .disabled(running)
                    .accessibility_label(t("console.streamHint", locale))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_execution(ExecutionPreference::Stream, cx);
                    })),
            )
            .child(
                Button::new("pref-auto")
                    .label(t("console.auto", locale))
                    .xsmall()
                    .ghost()
                    .selected(preference == ExecutionPreference::Auto)
                    .disabled(running)
                    .accessibility_label(t("console.autoHint", locale))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_execution(ExecutionPreference::Auto, cx);
                    })),
            )
            .child(div().w(px(72.)).child(Input::new(&self.auto_threshold)))
            .when_some(decision, |bar, text| {
                bar.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(text)),
                )
            })
            .when(!cursor_open, |bar| {
                bar.child(
                    Button::new("cursor-open")
                        .label(t("console.cursor", locale))
                        .icon(IconName::Database)
                        .xsmall()
                        .ghost()
                        .disabled(running || cursor_busy)
                        .accessibility_label(t("console.openCursorHint", locale))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_cursor(window, cx);
                        })),
                )
            })
            .when(cursor_open, |bar| {
                bar.child(
                    Button::new("cursor-next")
                        .label(if cursor_more {
                            t("console.nextPage", locale)
                        } else {
                            t("console.done", locale)
                        })
                        .icon(IconName::ChevronRight)
                        .xsmall()
                        .disabled(cursor_busy || !cursor_more)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.fetch_cursor_page(window, cx);
                        })),
                )
                .child(
                    Button::new("cursor-close")
                        .label(t("console.closeCursor", locale))
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
                    .label(t("console.validate", locale))
                    .icon(IconName::Check)
                    .xsmall()
                    .ghost()
                    .disabled(running || inspect_busy)
                    .accessibility_label(t("console.validateHint", locale))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.validate(window, cx);
                    })),
            )
            .child(
                Button::new("explain")
                    .label(t("console.explain", locale))
                    .icon(IconName::Info)
                    .xsmall()
                    .ghost()
                    .disabled(running || inspect_busy)
                    .accessibility_label(t("console.explainHint", locale))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.explain(window, cx);
                    })),
            )
            .child(
                Button::new("profile")
                    .label(t("console.profile", locale))
                    .icon(IconName::Activity)
                    .xsmall()
                    .ghost()
                    .disabled(running || inspect_busy)
                    .accessibility_label(t("console.profileHint", locale))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.profile_active(window, cx);
                    })),
            )
            .child(
                Button::new("toggle-running")
                    .label(t("console.running", locale))
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
                    .label(t("console.history", locale))
                    .icon(IconName::Clock)
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_history_drawer(window, cx);
                    })),
            )
            .child(
                Button::new("toggle-favorites")
                    .label(t("console.favorites", locale))
                    .icon(IconName::Star)
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_favorites_drawer(window, cx);
                    })),
            )
            .child(
                Button::new("toggle-params")
                    .label(t("console.params", locale))
                    .icon(IconName::Braces)
                    .xsmall()
                    .ghost()
                    .selected(self.params_open)
                    .disabled(running)
                    .accessibility_label(t("console.paramsHint", locale))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.params_open = !this.params_open;
                        cx.notify();
                    })),
            )
    }

    fn render_params(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let params_error = self.params_error.clone();
        let locale = crate::i18n::locale(cx);
        v_flex()
            .flex_shrink_0()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(form_row(
                t("console.parameters", locale),
                self.params.clone(),
                cx,
            ))
            .child(form_row(
                t("console.sessionVars", locale),
                self.session_vars.clone(),
                cx,
            ))
            .when_some(params_error, |panel, message| {
                panel.child(div().text_xs().text_color(cx.theme().danger).child(message))
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
        locale: crate::settings::Locale,
    ) -> impl IntoElement {
        let load_sql = item.sql.clone();
        let rerun_sql = item.sql.clone();
        let path = item.path;
        let summary = if item.success {
            crate::i18n::tf_args(
                "console.rowsMs",
                locale,
                &[&item.row_count, &item.elapsed_ms],
            )
        } else {
            item.error_code
                .clone()
                .unwrap_or_else(|| t("console.failed", locale).to_string())
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
                    .accessibility_label(t("console.rerunHint", locale))
                    .on_click(move |_, window, cx| {
                        let sql = rerun_sql.clone();
                        view_rerun.update(cx, |this, cx| {
                            if path == HistoryPath::Stream {
                                this.execution = ExecutionPreference::Stream;
                            }
                            this.load_sql(&sql, true, window, cx);
                        });
                    }),
            )
    }

    /// Open the query history in a right-side drawer (the web studio's
    /// history drawer).
    fn open_history_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let title = t("console.history", crate::i18n::locale(cx)).to_string();
        crate::components::drawer(window, cx, title, move |sheet, _, cx| {
            sheet.child(Self::history_body(&view, cx))
        });
    }

    /// Open favorites (saved queries) in a right-side drawer.
    fn open_favorites_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let title = t("console.favorites", crate::i18n::locale(cx)).to_string();
        crate::components::drawer(window, cx, title, move |sheet, _, cx| {
            sheet.child(Self::favorites_body(&view, cx))
        });
    }

    /// Drawer body for history: summary, clear action, list. Built from the
    /// app context so it can live inside a `Sheet` builder closure; the
    /// captured entity keeps the handlers live.
    fn history_body(view: &Entity<Self>, cx: &App) -> Div {
        let theme = cx.theme().clone();
        let locale = crate::i18n::locale(cx);
        let history = settings::global(cx).query_history;

        v_flex()
            .size_full()
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
                            .child(SharedString::from(tf(
                                "console.historyCount",
                                locale,
                                history.len(),
                            ))),
                    )
                    .child(
                        Button::new("history-clear")
                            .label(t("console.clear", locale))
                            .xsmall()
                            .ghost()
                            .disabled(history.is_empty())
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    let view = view.clone();
                                    crate::components::confirm(
                                        window,
                                        cx,
                                        t("console.clearHistoryTitle", locale),
                                        t("console.clearHistoryDesc", locale),
                                        t("console.clear", locale),
                                        true,
                                        move |_, cx| {
                                            settings::clear_history(cx);
                                            view.update(cx, |_, cx| cx.notify());
                                        },
                                    );
                                }
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id("history-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .children(history.iter().enumerate().map(|(ix, item)| {
                        Self::history_row(item.clone(), ix, view, &theme, locale)
                    }))
                    .when(history.is_empty(), |list| {
                        list.child(empty_state(
                            IconName::Clock,
                            t("console.noHistory", locale),
                            Some(t("console.noHistoryHint", locale).into()),
                            cx,
                        ))
                    }),
            )
    }

    /// Drawer body for favorites: save form plus the saved list.
    fn favorites_body(view: &Entity<Self>, cx: &App) -> Div {
        let theme = cx.theme().clone();
        let locale = crate::i18n::locale(cx);
        let favorites = settings::global(cx).favorites;
        let fav_path = view.read(cx).fav_path;
        let fav_name = view.read(cx).fav_name.clone();

        let path_button = |id: &'static str, key: &'static str, value: Option<HistoryPath>| {
            Button::new(id)
                .label(t(key, locale))
                .xsmall()
                .ghost()
                .selected(fav_path == value)
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| {
                            this.fav_path = value;
                            cx.notify();
                        });
                    }
                })
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(tf(
                        "console.favoritesCount",
                        locale,
                        favorites.len(),
                    ))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .px_2()
                    .pb_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(t("console.name", locale)),
                            )
                            .child(Input::new(&fav_name).small()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(t("console.rerunPath", locale)),
                            )
                            .child(path_button("fav-path-auto", "console.pathAuto", None))
                            .child(path_button(
                                "fav-path-materialized",
                                "console.pathResults",
                                Some(HistoryPath::Materialized),
                            ))
                            .child(path_button(
                                "fav-path-stream",
                                "console.pathStream",
                                Some(HistoryPath::Stream),
                            )),
                    )
                    .child(
                        Button::new("fav-save")
                            .label(t("console.saveFavorite", locale))
                            .icon(IconName::Star)
                            .xsmall()
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |this, cx| {
                                        this.save_favorite(window, cx);
                                    });
                                }
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id("favorites-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .children(
                        favorites
                            .iter()
                            .enumerate()
                            .map(|(ix, fav)| Self::favorite_row(fav, ix, view, &theme, locale)),
                    )
                    .when(favorites.is_empty(), |list| {
                        list.child(empty_state(
                            IconName::Star,
                            t("console.noFavorites", locale),
                            Some(t("console.noFavoritesHint", locale).into()),
                            cx,
                        ))
                    }),
            )
    }

    /// One favorite row: name (click loads), statement preview, rerun and
    /// delete actions. Handler-only variant usable inside drawer bodies.
    fn favorite_row(
        fav: &crate::settings::QueryFavoriteItem,
        ix: usize,
        view: &Entity<Self>,
        theme: &gpui_kit::component::Theme,
        locale: crate::settings::Locale,
    ) -> impl IntoElement {
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
                            .child(Icon::new(IconName::Star).text_color(theme.muted_foreground))
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
                    .accessibility_label(t("console.rerunFavHint", locale))
                    .on_click(move |_, window, cx| {
                        let sql = rerun_sql.clone();
                        view_rerun.update(cx, |this, cx| {
                            match preferred {
                                Some(HistoryPath::Stream) => {
                                    this.execution = ExecutionPreference::Stream;
                                }
                                Some(HistoryPath::Materialized) => {
                                    this.execution = ExecutionPreference::Materialized;
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
                    .accessibility_label(t("console.deleteFavHint", locale))
                    .on_click(move |_, _, cx| {
                        let name = delete_name.clone();
                        view_delete.update(cx, |_, cx| {
                            settings::remove_favorite(cx, &name);
                            cx.notify();
                        });
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
                .child(t("console.loading", crate::i18n::locale(cx)))
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
                            .accessibility_label(t(
                                "console.closeInspector",
                                crate::i18n::locale(cx),
                            ))
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
                            .child(SharedString::from(crate::i18n::t(
                                "console.runningQueries",
                                crate::i18n::locale(cx),
                            ))),
                    )
                    .child(
                        Button::new("running-refresh")
                            .icon(IconName::RefreshCw)
                            .xsmall()
                            .ghost()
                            .accessibility_label(crate::i18n::t(
                                "console.refreshRunning",
                                crate::i18n::locale(cx),
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_running(window, cx);
                            })),
                    )
                    .child(
                        Button::new("running-close")
                            .icon(IconName::X)
                            .xsmall()
                            .ghost()
                            .accessibility_label(t("console.closeRunning", crate::i18n::locale(cx)))
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
                    .child(form_row(
                        t("console.sessionId", crate::i18n::locale(cx)),
                        self.kill_session.clone(),
                        cx,
                    ))
                    .child(form_row(
                        t("console.queryId", crate::i18n::locale(cx)),
                        self.kill_query.clone(),
                        cx,
                    ))
                    .child(
                        Button::new("running-kill")
                            .label(crate::i18n::t("console.terminate", crate::i18n::locale(cx)))
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
                t("console.noResults", crate::i18n::locale(cx)),
                Some(t("console.noResultsHint", crate::i18n::locale(cx)).into()),
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
        let can_send = active_status == TabStatus::Ok && !self.tabs[active].raw_rows.is_empty();

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
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        Button::new("send-to-graph")
                            .label(crate::i18n::t("console.graph", crate::i18n::locale(cx)))
                            .icon(IconName::Network)
                            .xsmall()
                            .ghost()
                            .disabled(!can_send)
                            .accessibility_label(t(
                                "console.sendGraphHint",
                                crate::i18n::locale(cx),
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.send_to_graph(window, cx);
                            })),
                    )
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
                                div().size_full().p_1().child(
                                    DataTable::new(&active_table)
                                        .stripe(true)
                                        .with_size(crate::components::table_size(cx)),
                                ),
                            )
                        },
                    )
                    .when(
                        active_status != TabStatus::Failed && active_sql_empty,
                        |host| {
                            host.child(empty_state(
                                IconName::Inbox,
                                t("console.noRows", crate::i18n::locale(cx)),
                                Some(t("console.noRowsHint", crate::i18n::locale(cx)).into()),
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
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(
                        SharedString::from(tf(
                            "console.stmtFailed",
                            crate::i18n::locale(cx),
                            ix + 1,
                        )),
                    )),
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
            .when(self.params_open, |panel| {
                panel.child(self.render_params(cx))
            })
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
                h_flex().flex_1().min_h_0().child(
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
/// Whether a script may change schema names (tags, edge types, spaces),
/// in which case statement completion must reload its snapshot.
fn script_may_change_schema(script: &str) -> bool {
    let upper = script.to_uppercase();
    let verb = ["CREATE", "DROP", "ALTER"]
        .iter()
        .any(|v| upper.contains(v));
    let object = ["TAG", "EDGE", "SPACE"].iter().any(|o| upper.contains(o));
    verb && object
}

/// Decide the automatic execution path from a validate outcome: invalid
/// statements run materially, valid ones compare the planner estimate
/// against the threshold (a missing estimate streams, matching the
/// frontend routing).
fn auto_decision(
    outcome: &Result<serde_json::Value, ClientError>,
    threshold: u64,
) -> (bool, String) {
    match outcome {
        Err(_) => (
            false,
            "auto \u{2192} materialized (validate failed)".to_string(),
        ),
        Ok(payload) => {
            if !payload
                .get("valid")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                return (false, "auto \u{2192} materialized (invalid)".to_string());
            }
            match payload
                .get("estimated_rows")
                .and_then(|value| value.as_u64())
            {
                Some(estimated) if estimated <= threshold => (
                    false,
                    format!("auto \u{2192} materialized (est. {estimated} \u{2264} {threshold})"),
                ),
                Some(estimated) => (
                    true,
                    format!("auto \u{2192} stream (est. {estimated} > {threshold})"),
                ),
                None => (true, "auto \u{2192} stream (no estimate)".to_string()),
            }
        }
    }
}

fn user_message(error: &ClientError, locale: crate::settings::Locale) -> String {
    match error {
        ClientError::NotConnected => t("console.notConnected", locale).to_string(),
        ClientError::NoSpaceSelected => t("console.selectSpaceFirst", locale).to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(estimated: Option<u64>) -> Result<serde_json::Value, ClientError> {
        Ok(serde_json::json!({"valid": true, "estimated_rows": estimated}))
    }

    #[test]
    fn auto_routes_by_estimate() {
        let (stream, note) = auto_decision(&valid(Some(5)), 1000);
        assert!(!stream);
        assert!(note.contains("materialized"));
        let (stream, note) = auto_decision(&valid(Some(5000)), 1000);
        assert!(stream);
        assert!(note.contains("stream"));
        let (stream, _) = auto_decision(&valid(None), 1000);
        assert!(stream);
    }

    #[test]
    fn auto_falls_back_to_materialized() {
        let invalid = Ok(serde_json::json!({"valid": false, "estimated_rows": 99_999}));
        assert!(!auto_decision(&invalid, 1000).0);
        let failed: Result<serde_json::Value, ClientError> = Err(ClientError::NotConnected);
        assert!(!auto_decision(&failed, 1000).0);
    }
}
