use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState},
    list::ListItem,
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
    empty_state, new_table_state, on_copy_row, QueryTableData, QueryTableDelegate,
};
use crate::services::{connection_service, query_service};
use crate::settings;

actions!(linkrs, [RunQuery, CancelQuery]);

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
pub struct ConsolePanel {
    editor: Entity<EditorState>,
    tabs: Vec<ResultTab>,
    active_tab: Option<usize>,
    running: Option<Task<()>>,
    history_open: bool,
    focus_handle: gpui_kit::FocusHandle,
}

impl ConsolePanel {
    pub(crate) const PANEL_NAME: &'static str = "Console";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| EditorState::new(window, cx).language("sql"));
        Self {
            editor,
            tabs: Vec::new(),
            active_tab: None,
            running: None,
            history_open: false,
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
                }
            })
            .collect();
        self.active_tab = Some(0);

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
                    query_service::execute(cx, &sql, false)
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

        match result {
            Ok(result) => match result.error {
                None => {
                    *tab.data.borrow_mut() = QueryTableData::from_result(&result);
                    tab.status = TabStatus::Ok;
                    tab.table.update(cx, |_, cx| cx.notify());
                    true
                }
                Some(error) => {
                    tab.error = Some(error);
                    tab.status = TabStatus::Failed;
                    false
                }
            },
            Err(error) => {
                if connection_service::is_session_expired(&error) {
                    crate::state::AppState::global(cx).update(cx, |state, cx| {
                        state.set_disconnected(cx);
                    });
                }
                tab.error = Some(QueryErrorInfo {
                    code: "CLIENT_ERROR".into(),
                    message: error.to_string(),
                    details: None,
                });
                tab.status = TabStatus::Failed;
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
        let sql = self.editor.read(cx).text().to_string();
        settings::record_query(cx, &sql);
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
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let history = settings::global(cx).query_history;
        let view = cx.entity();

        v_flex()
            .w(px(280.))
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
                        "History ({} entries, newest first)",
                        history.len()
                    ))),
            )
            .child(
                v_flex()
                    .id("history-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .children(history.iter().enumerate().map(|(ix, sql)| {
                        let sql = sql.clone();
                        let view = view.clone();
                        ListItem::new(("history-entry", ix))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .min_w_0()
                                    .child(
                                        Icon::new(IconName::FileText)
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(ConsolePanel::tab_label(&sql)),
                                    ),
                            )
                            .on_click(move |_, window, cx| {
                                let sql = sql.clone();
                                view.update(cx, |this, cx| {
                                    this.editor.update(cx, |state, cx| {
                                        state.set_value(sql, window, cx);
                                    });
                                    this.history_open = false;
                                    cx.notify();
                                });
                            })
                    })),
            )
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
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.history_open, |row| row.child(self.render_history(cx)))
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
