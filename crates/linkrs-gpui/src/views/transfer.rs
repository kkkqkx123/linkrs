use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::{v_flex, ActiveTheme as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, App, AppContext as _, ClickEvent, Context, Entity,
    EventEmitter, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{form_row, render_json_block};
use crate::services::{connection_service, transfer_service};

/// Import/export panel: server-side multipart import with status polling and
/// server-side export (`GET /v1/export`) saved through the file dialog.
pub struct TransferPanel {
    space: Entity<InputState>,
    format: Entity<InputState>,
    target_type: Entity<InputState>,
    target_name: Entity<InputState>,
    file_path: Entity<InputState>,
    export_query: Entity<InputState>,
    export_format_csv: bool,
    save_path: Entity<InputState>,
    busy: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    import_result: serde_json::Value,
    export_result: serde_json::Value,
    focus_handle: gpui_kit::FocusHandle,
}

impl TransferPanel {
    pub(crate) const PANEL_NAME: &'static str = "Transfer";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let space = cx.new(|cx| InputState::new(window, cx).placeholder("Space"));
        let format = cx.new(|cx| InputState::new(window, cx).placeholder("csv, json or jsonl"));
        format.update(cx, |field, cx| {
            field.set_value("csv", window, cx);
        });
        let target_type =
            cx.new(|cx| InputState::new(window, cx).placeholder("tag/vertex or edge"));
        target_type.update(cx, |field, cx| {
            field.set_value("vertex", window, cx);
        });
        let target_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("Target tag/edge name"));
        let file_path = cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/data.csv"));
        let export_query =
            cx.new(|cx| InputState::new(window, cx).placeholder("MATCH (v) RETURN v LIMIT 100"));
        let save_path = cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/export.csv"));
        Self {
            space,
            format,
            target_type,
            target_name,
            file_path,
            export_query,
            export_format_csv: true,
            save_path,
            busy: false,
            message: None,
            error: None,
            import_result: serde_json::Value::Null,
            export_result: serde_json::Value::Null,
            focus_handle: cx.focus_handle(),
        }
    }

    fn text(field: &Entity<InputState>, cx: &App) -> String {
        field.read(cx).value().trim().to_string()
    }

    fn import(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let space = Self::text(&self.space, cx);
        let format = Self::text(&self.format, cx);
        let target_type = Self::text(&self.target_type, cx);
        let target_name = Self::text(&self.target_name, cx);
        let file_path = Self::text(&self.file_path, cx);
        if space.is_empty() || target_name.is_empty() || file_path.is_empty() {
            self.error = Some("Space, target name and file path are required.".into());
            cx.notify();
            return;
        }
        let bytes = match std::fs::read(&file_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(format!("Cannot read {file_path}: {error}").into());
                cx.notify();
                return;
            }
        };
        let file_name = std::path::Path::new(&file_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("upload.dat")
            .to_string();

        self.busy = true;
        self.error = None;
        cx.notify();
        let task = transfer_service::import_file(
            cx,
            &space,
            &format,
            &target_type,
            &target_name,
            None,
            &file_name,
            bytes,
        );
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(value) => {
                        this.import_result = value.clone();
                        // Follow up with a status poll when the server hands
                        // back a job id so the user sees the final state.
                        let job_id = value
                            .get("job_id")
                            .or_else(|| value.get("jobId"))
                            .and_then(|id| id.as_str())
                            .map(str::to_string);
                        this.message =
                            Some(format!("Uploaded {file_name}; import accepted.").into());
                        drop(value);
                        if let Some(job_id) = job_id {
                            this.poll_import(&job_id, cx);
                        }
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn poll_import(&mut self, job_id: &str, cx: &mut Context<Self>) {
        let task = transfer_service::import_status(cx, job_id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(value) => {
                        this.import_result = value;
                    }
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn export(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let query = Self::text(&self.export_query, cx);
        let save_path = Self::text(&self.save_path, cx);
        if query.is_empty() || save_path.is_empty() {
            self.error = Some("Query and save path are required for export.".into());
            cx.notify();
            return;
        }
        let format = if self.export_format_csv {
            "csv"
        } else {
            "jsonl"
        };
        // Confirm the destination is writable before hitting the server.
        let receiver = cx.prompt_for_new_path(
            &std::path::PathBuf::from(&save_path)
                .parent()
                .map(|parent| parent.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
            Some(
                std::path::Path::new(&save_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("export.csv"),
            ),
        );

        self.busy = true;
        self.error = None;
        cx.notify();
        let task = transfer_service::export_bytes(cx, &query, format, None);
        cx.spawn_in(window, async move |this, cx| {
            let bytes = task.await;
            let path = receiver.await;
            _ = this.update_in(cx, |this, _, cx| {
                this.busy = false;
                match (bytes, path) {
                    (Ok(bytes), Ok(Ok(Some(path)))) => match std::fs::write(&path, &bytes) {
                        Ok(()) => {
                            this.export_result = serde_json::json!({
                                "path": path.display().to_string(),
                                "bytes": bytes.len(),
                            });
                            this.message = Some(
                                format!("Exported {} byte(s) to {}.", bytes.len(), path.display())
                                    .into(),
                            );
                        }
                        Err(error) => {
                            this.error = Some(format!("Cannot write export: {error}").into());
                        }
                    },
                    (Err(error), _) => {
                        this.error = Some(error.to_string().into());
                    }
                    _ => {
                        this.message = Some("Export cancelled.".into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_import(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .p_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(SharedString::from("Server-side import")),
            )
            .child(form_row("Space", self.space.clone(), cx))
            .child(form_row("Format", self.format.clone(), cx))
            .child(form_row("Target type", self.target_type.clone(), cx))
            .child(form_row("Target name", self.target_name.clone(), cx))
            .child(form_row("Local file", self.file_path.clone(), cx))
            .child(
                Button::new("transfer-import")
                    .label("Upload and import")
                    .icon(IconName::Upload)
                    .primary()
                    .xsmall()
                    .disabled(self.busy)
                    .on_click(cx.listener(Self::import)),
            )
            .when(!self.import_result.is_null(), |panel| {
                panel.child(render_json_block(&self.import_result, cx))
            })
    }

    fn render_export(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let csv = self.export_format_csv;
        v_flex()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(SharedString::from("Server-side export")),
            )
            .child(form_row("Query", self.export_query.clone(), cx))
            .child(form_row("Save as", self.save_path.clone(), cx))
            .child(gpui_kit::component::h_flex().gap_1().children(
                ["csv", "jsonl"].into_iter().map(|candidate| {
                    let active = (candidate == "csv") == csv;
                    Button::new(format!("transfer-format-{candidate}"))
                        .label(candidate)
                        .xsmall()
                        .selected(active)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.export_format_csv = candidate == "csv";
                            cx.notify();
                        }))
                }),
            ))
            .child(
                Button::new("transfer-export")
                    .label("Export to file")
                    .icon(IconName::Download)
                    .xsmall()
                    .disabled(self.busy)
                    .on_click(cx.listener(Self::export)),
            )
            .when(!self.export_result.is_null(), |panel| {
                panel.child(render_json_block(&self.export_result, cx))
            })
    }
}

impl Focusable for TransferPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for TransferPanel {}

impl gpui_kit::base::dock::Panel for TransferPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for TransferPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for TransferPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .id("transfer-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                gpui_kit::component::h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from("Import / Export")),
                    )
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(theme.muted_foreground).child(
                        SharedString::from(if self.busy { "Working…" } else { "Idle" }),
                    )),
            )
            .when_some(self.error.clone(), |panel, message| {
                panel.child(
                    gpui_kit::component::h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(theme.danger.opacity(0.08))
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(message),
                        ),
                )
            })
            .when_some(self.message.clone(), |panel, message| {
                panel.child(
                    gpui_kit::component::h_flex()
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(message),
                        ),
                )
            })
            .child(
                div()
                    .id("transfer-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.render_import(cx))
                    .child(self.render_export(cx)),
            )
    }
}
