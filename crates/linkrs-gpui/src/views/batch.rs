use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, ClickEvent, Context, Entity,
    EventEmitter, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use linkrs_wire::batch::{BatchItem, BatchStatus, BatchStatusResponse, BatchType};

use crate::components::{form_row, panel_header, render_json_block};
use crate::services::{batch_service, connection_service};

/// Batch job panel: create a bulk-load task, append JSON items, execute it
/// and poll its status with a progress bar. Known task ids are kept in the
/// panel for the session; status refreshes automatically while running.
pub struct BatchPanel {
    space_id: Entity<InputState>,
    batch_size: Entity<InputState>,
    batch_type: BatchType,
    items_text: Entity<InputState>,
    batch_id: Option<String>,
    status: Option<BatchStatusResponse>,
    auto_poll: bool,
    busy: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
}

impl BatchPanel {
    pub(crate) const PANEL_NAME: &'static str = "Batch";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let locale = crate::i18n::locale(cx);
        let space_id = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::t("batch.spaceIdPlaceholder", locale))
        });
        let batch_size = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::t("batch.batchSizePlaceholder", locale))
        });
        batch_size.update(cx, |field, cx| {
            field.set_value("500", window, cx);
        });
        let items_text = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::t("batch.itemsPlaceholder", locale))
        });
        Self {
            space_id,
            batch_size,
            batch_type: BatchType::Mixed,
            items_text,
            batch_id: None,
            status: None,
            auto_poll: true,
            busy: false,
            message: None,
            error: None,
            focus_handle: cx.focus_handle(),
        }
    }

    fn parse_space(&self, cx: &App) -> Option<u64> {
        self.space_id.read(cx).value().trim().parse::<u64>().ok()
    }

    fn parse_batch_size(&self, cx: &App) -> usize {
        self.batch_size
            .read(cx)
            .value()
            .trim()
            .parse::<usize>()
            .unwrap_or(500)
            .max(1)
    }

    fn parse_items(&self, cx: &App) -> Result<Vec<BatchItem>, String> {
        let text = self.items_text.read(cx).value().to_string();
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str::<Vec<BatchItem>>(&text).map_err(|error| {
            crate::i18n::tf("batch.itemsArrayError", crate::i18n::locale(cx), error)
        })
    }

    fn create(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(space_id) = self.parse_space(cx) else {
            self.error =
                Some(crate::i18n::t("batch.spaceIdRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        };
        let batch_size = self.parse_batch_size(cx);
        let batch_type = self.batch_type;

        self.busy = true;
        self.error = None;
        cx.notify();
        let task = batch_service::create(cx, space_id, batch_type, batch_size);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(batch_id) => {
                        this.batch_id = Some(batch_id.clone());
                        this.status = None;
                        this.message = Some(
                            crate::i18n::tf("batch.created", crate::i18n::locale(cx), &batch_id)
                                .into(),
                        );
                        this.poll(cx);
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

    /// Load the items JSON array from a file chosen in the system dialog.
    fn load_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::i18n::t("batch.loadPrompt", crate::i18n::locale(cx)).into()),
        });
        cx.spawn_in(window, async move |this, window| {
            let paths = receiver.await;
            _ = this.update_in(window, move |this, window, cx| {
                match paths {
                    Ok(Ok(Some(mut list))) if !list.is_empty() => {
                        let path = list.remove(0);
                        match std::fs::read_to_string(&path) {
                            Ok(text) => {
                                this.items_text.update(cx, |field, cx| {
                                    field.set_value(text, window, cx);
                                });
                                this.error = None;
                                this.message = Some(
                                    crate::i18n::tf(
                                        "batch.loaded",
                                        crate::i18n::locale(cx),
                                        path.display(),
                                    )
                                    .into(),
                                );
                            }
                            Err(error) => {
                                this.error = Some(
                                    crate::i18n::tf(
                                        "batch.cannotRead",
                                        crate::i18n::locale(cx),
                                        path.display(),
                                    )
                                    .replacen("{}", &error.to_string(), 1)
                                    .into(),
                                );
                            }
                        }
                    }
                    _ => {
                        this.message = Some(
                            crate::i18n::t("batch.fileCancelled", crate::i18n::locale(cx)).into(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_items(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch_id) = self.batch_id.clone() else {
            self.error = Some(crate::i18n::t("batch.createFirst", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        };
        let items = match self.parse_items(cx) {
            Ok(items) => items,
            Err(message) => {
                self.error = Some(message.into());
                cx.notify();
                return;
            }
        };
        if items.is_empty() {
            self.error = Some(crate::i18n::t("batch.pasteItems", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        self.busy = true;
        cx.notify();
        let task = batch_service::add_items(cx, &batch_id, items);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(accepted) => {
                        this.message = Some(
                            crate::i18n::tf("batch.added", crate::i18n::locale(cx), accepted)
                                .into(),
                        );
                        this.poll(cx);
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

    fn execute(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch_id) = self.batch_id.clone() else {
            self.error = Some(crate::i18n::t("batch.executeFirst", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = batch_service::execute(cx, &batch_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            this.update_in(window, |this, window, cx| {
                this.busy = false;
                match outcome {
                    Ok(_) => {
                        this.message = Some(
                            crate::i18n::t("batch.executionStarted", crate::i18n::locale(cx))
                                .into(),
                        );
                        window.push_notification(
                            Notification::success(crate::i18n::t(
                                "batch.executionStartedNotify",
                                crate::i18n::locale(cx),
                            )),
                            cx,
                        );
                        this.poll(cx);
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

    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(batch_id) = self.batch_id.clone() else {
            return;
        };
        let task = batch_service::status(cx, &batch_id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(status) => {
                        let running = status.status == BatchStatus::Running
                            || status.status == BatchStatus::Created;
                        this.status = Some(status);
                        this.error = None;
                        // Keep polling while the task is live and auto-poll is on.
                        if running && this.auto_poll {
                            let view = cx.entity().downgrade();
                            cx.spawn(async move |_this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_secs(2))
                                    .await;
                                _ = view.update(cx, |this, cx| this.poll(cx));
                            })
                            .detach();
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

    fn cancel(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch_id) = self.batch_id.clone() else {
            return;
        };
        let task = batch_service::cancel(cx, &batch_id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => {
                        this.message = Some(
                            crate::i18n::t("batch.cancelRequested", crate::i18n::locale(cx)).into(),
                        );
                        this.poll(cx);
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

    fn delete(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch_id) = self.batch_id.clone() else {
            return;
        };
        let task = batch_service::delete(cx, &batch_id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => {
                        this.message = Some(
                            crate::i18n::tf("batch.deleted", crate::i18n::locale(cx), &batch_id)
                                .into(),
                        );
                        this.batch_id = None;
                        this.status = None;
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

    fn progress_value(&self) -> Option<f32> {
        let status = self.status.as_ref()?;
        let value = serde_json::to_value(status).ok()?;
        value
            .get("progress")
            .and_then(|progress| progress.as_f64())
            .map(|progress| progress.clamp(0.0, 1.0) as f32)
            .or_else(|| {
                let total = value.get("total").and_then(|v| v.as_u64())?;
                let done = value.get("processed").and_then(|v| v.as_u64()).unwrap_or(0);
                if total == 0 {
                    None
                } else {
                    Some((done as f32 / total as f32).clamp(0.0, 1.0))
                }
            })
    }

    fn render_form(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let batch_type = self.batch_type;
        v_flex()
            .gap_2()
            .p_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(SharedString::from(crate::i18n::t(
                        "batch.newTask",
                        crate::i18n::locale(cx),
                    ))),
            )
            .child(form_row(
                crate::i18n::t("batch.spaceId", crate::i18n::locale(cx)),
                self.space_id.clone(),
                cx,
            ))
            .child(form_row(
                crate::i18n::t("batch.batchSize", crate::i18n::locale(cx)),
                self.batch_size.clone(),
                cx,
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(crate::i18n::t(
                                "batch.type",
                                crate::i18n::locale(cx),
                            ))),
                    )
                    .child(
                        h_flex().gap_1().items_center().children(
                            [BatchType::Vertex, BatchType::Edge, BatchType::Mixed]
                                .into_iter()
                                .map(|candidate| {
                                    Button::new(format!("batch-type-{candidate}"))
                                        .label(candidate.to_string())
                                        .xsmall()
                                        .selected(batch_type == candidate)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.batch_type = candidate;
                                            cx.notify();
                                        }))
                                }),
                        ),
                    ),
            )
            .child(
                Button::new("batch-create")
                    .label(crate::i18n::t("batch.create", crate::i18n::locale(cx)))
                    .icon(IconName::Plus)
                    .primary()
                    .xsmall()
                    .disabled(self.busy)
                    .on_click(cx.listener(Self::create)),
            )
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let batch_id = self.batch_id.clone().unwrap_or_else(|| "—".to_string());
        let status_label = self
            .status
            .as_ref()
            .map(|status| status.status.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        v_flex()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from(crate::i18n::tf(
                                "batch.title",
                                crate::i18n::locale(cx),
                                batch_id,
                            ))),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded(theme.radius)
                            .bg(theme.muted.opacity(0.4))
                            .text_xs()
                            .child(SharedString::from(status_label)),
                    ),
            )
            .when_some(self.progress_value(), |panel, value| {
                panel.child(Progress::new("batch-progress").value(value))
            })
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("batch-poll")
                            .label(crate::i18n::t("batch.refresh", crate::i18n::locale(cx)))
                            .icon(IconName::RefreshCw)
                            .xsmall()
                            .ghost()
                            .disabled(self.batch_id.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.poll(cx))),
                    )
                    .child(
                        Button::new("batch-cancel")
                            .label(crate::i18n::t("batch.cancel", crate::i18n::locale(cx)))
                            .icon(IconName::Ban)
                            .xsmall()
                            .ghost()
                            .disabled(self.batch_id.is_none())
                            .on_click(cx.listener(Self::cancel)),
                    )
                    .child(
                        Button::new("batch-delete")
                            .label(crate::i18n::t("batch.delete", crate::i18n::locale(cx)))
                            .icon(IconName::Trash)
                            .xsmall()
                            .ghost()
                            .disabled(self.batch_id.is_none())
                            .on_click(cx.listener(Self::delete)),
                    )
                    .child(
                        Button::new("batch-auto")
                            .label(crate::i18n::t("batch.autoPoll", crate::i18n::locale(cx)))
                            .xsmall()
                            .ghost()
                            .selected(self.auto_poll)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.auto_poll = !this.auto_poll;
                                cx.notify();
                            })),
                    ),
            )
            .when_some(self.status.clone(), |panel, status| {
                let value = serde_json::to_value(&status).unwrap_or(serde_json::Value::Null);
                panel.child(render_json_block(&value, cx))
            })
    }
}

impl Focusable for BatchPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for BatchPanel {}

impl gpui_kit::base::dock::Panel for BatchPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for BatchPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for BatchPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .id("batch-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.batch", crate::i18n::locale(cx))),
                Some(SharedString::from(if self.busy {
                    crate::i18n::t("common.working", crate::i18n::locale(cx))
                } else {
                    crate::i18n::t("common.idle", crate::i18n::locale(cx))
                })),
                None,
                cx,
            ))
            .when_some(self.error.clone(), |panel, message| {
                panel.child(div().px(px(20.)).py(px(8.)).child(
                    gpui_kit::component::alert::Alert::error("batch-error", message).small(),
                ))
            })
            .when_some(self.message.clone(), |panel, message| {
                panel.child(
                    h_flex().flex_shrink_0().px_2().py_1().child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(message),
                    ),
                )
            })
            .child(
                div()
                    .id("batch-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.render_form(cx))
                    .child(
                        v_flex()
                            .gap_1()
                            .px_3()
                            .child(div().text_sm().text_color(theme.foreground).child(
                                SharedString::from(crate::i18n::t(
                                    "batch.items",
                                    crate::i18n::locale(cx),
                                )),
                            ))
                            .child(
                                div()
                                    .min_h(px(96.))
                                    .rounded(theme.radius)
                                    .border_1()
                                    .border_color(theme.border)
                                    .p_1()
                                    .child(Input::new(&self.items_text)),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("batch-load")
                                            .label(crate::i18n::t(
                                                "batch.loadFile",
                                                crate::i18n::locale(cx),
                                            ))
                                            .icon(IconName::FolderOpen)
                                            .xsmall()
                                            .disabled(self.busy)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.load_file(window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("batch-add")
                                            .label(crate::i18n::t(
                                                "batch.addItems",
                                                crate::i18n::locale(cx),
                                            ))
                                            .icon(IconName::Plus)
                                            .xsmall()
                                            .disabled(self.busy)
                                            .on_click(cx.listener(Self::add_items)),
                                    )
                                    .child(
                                        Button::new("batch-execute")
                                            .label(crate::i18n::t(
                                                "batch.execute",
                                                crate::i18n::locale(cx),
                                            ))
                                            .icon(IconName::Play)
                                            .primary()
                                            .xsmall()
                                            .disabled(self.busy)
                                            .on_click(cx.listener(Self::execute)),
                                    ),
                            ),
                    )
                    .child(self.render_status(cx)),
            )
    }
}
