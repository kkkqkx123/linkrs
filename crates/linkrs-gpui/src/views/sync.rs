use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, EventEmitter,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{
    card, card_body, card_head, confirm, empty_state, form_row, panel_header, render_json_block,
    skeleton_rows,
};
use crate::i18n::{t, tf};
use crate::services::{connection_service, sync_service};

/// Sync/outbox management panel: status snapshot, dead-letter table with
/// requeue, degraded ranges with clearing, and retention controls.
pub struct SyncPanel {
    status: serde_json::Value,
    diagnostics: serde_json::Value,
    dead_letters: Vec<serde_json::Value>,
    degraded: Vec<serde_json::Value>,
    retention: serde_json::Value,
    target: Entity<InputState>,
    index_id: Entity<InputState>,
    generation: Entity<InputState>,
    limit: Entity<InputState>,
    message: Option<SharedString>,
    error: Option<SharedString>,
    loading: bool,
    focus_handle: gpui_kit::FocusHandle,
}

impl SyncPanel {
    pub(crate) const PANEL_NAME: &'static str = "Sync";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let locale = crate::i18n::locale(cx);
        let target = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("sync.targetPlaceholder", locale)));
        let index_id = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("sync.indexIdPlaceholder", locale))
        });
        let generation = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("sync.generationPlaceholder", locale))
        });
        let limit = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("sync.limitPlaceholder", locale)));
        let mut panel = Self {
            status: serde_json::Value::Null,
            diagnostics: serde_json::Value::Null,
            dead_letters: Vec::new(),
            degraded: Vec::new(),
            retention: serde_json::Value::Null,
            target,
            index_id,
            generation,
            limit,
            message: None,
            error: None,
            loading: false,
            focus_handle: cx.focus_handle(),
        };
        panel.refresh(window, cx);
        panel
    }

    fn filters(&self, cx: &App) -> (Option<String>, Option<u64>, Option<u64>, usize) {
        let text = |field: &Entity<InputState>| field.read(cx).value().trim().to_string();
        let target = text(&self.target);
        let index_id = text(&self.index_id).parse().ok();
        let generation = text(&self.generation).parse().ok();
        let limit = text(&self.limit).parse().unwrap_or(100);
        (
            if target.is_empty() {
                None
            } else {
                Some(target)
            },
            index_id,
            generation,
            limit,
        )
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        let (target, index_id, generation, limit) = self.filters(cx);
        let status = sync_service::status(cx);
        let diagnostics = sync_service::diagnostics(cx);
        let dead = sync_service::dead_letters(cx, target.clone(), index_id, generation, limit, 0);
        let degraded = sync_service::degraded_ranges(cx, target, index_id, generation);
        let retention = sync_service::retention_status(cx);
        cx.spawn_in(window, async move |this, window| {
            let status = status.await;
            let diagnostics = diagnostics.await;
            let dead = dead.await;
            let degraded = degraded.await;
            let retention = retention.await;
            _ = this.update_in(window, |this, _, cx| {
                this.loading = false;
                match (status, diagnostics, dead, degraded, retention) {
                    (Ok(status), Ok(diagnostics), Ok(dead), Ok(degraded), Ok(retention)) => {
                        this.status = status;
                        this.diagnostics = diagnostics;
                        this.dead_letters = rows_of(&dead, "dead_letters");
                        this.degraded = rows_of(&degraded, "degraded_ranges");
                        this.retention = retention;
                        this.error = None;
                    }
                    _ => {
                        this.error = Some(
                            t("sync.loadFailed", crate::i18n::locale(cx))
                                .to_string()
                                .into(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn requeue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (target, index_id, generation, limit) = self.filters(cx);
        let mut body = serde_json::Map::new();
        if let Some(target) = target {
            body.insert("target".into(), target.into());
        }
        if let Some(index_id) = index_id {
            body.insert("index_id".into(), index_id.into());
        }
        if let Some(generation) = generation {
            body.insert("generation".into(), generation.into());
        }
        body.insert("limit".into(), limit.into());
        let task = sync_service::requeue(cx, serde_json::Value::Object(body));
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(_) => {
                        this.message = Some(t("sync.requeued", crate::i18n::locale(cx)).into());
                        this.refresh(window, cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = sync_service::retry(cx);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(_) => {
                        this.message =
                            Some(t("sync.outboxRetried", crate::i18n::locale(cx)).into());
                        this.refresh(window, cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_degraded(
        &mut self,
        row: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = sync_service::degraded_clear(cx, row.clone());
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(_) => {
                        this.message =
                            Some(t("sync.degradedCleared", crate::i18n::locale(cx)).into());
                        this.refresh(window, cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn retention_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = sync_service::retention_run(cx, None, None);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(_) => {
                        this.message = Some(t("sync.retentionRan", crate::i18n::locale(cx)).into());
                        this.refresh(window, cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Focusable for SyncPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for SyncPanel {}

impl gpui_kit::base::dock::Panel for SyncPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for SyncPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for SyncPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = crate::i18n::locale(cx);

        v_flex()
            .id("sync-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.sync", crate::i18n::locale(cx))),
                None,
                Some(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("sync-requeue")
                                .label(t("sync.requeueFiltered", locale))
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.requeue(window, cx);
                                })),
                        )
                        .child(
                            Button::new("sync-retry")
                                .label(t("sync.retryOutbox", locale))
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.retry(window, cx);
                                })),
                        )
                        .child(
                            Button::new("sync-retention-run")
                                .label(t("sync.runRetention", locale))
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.retention_run(window, cx);
                                })),
                        )
                        .child(
                            Button::new("sync-refresh")
                                .icon(IconName::RefreshCw)
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.refresh(window, cx);
                                })),
                        )
                        .into_any_element(),
                ),
                cx,
            ))
            .when_some(self.error.clone(), |panel, message| {
                panel.child(
                    div().px(px(20.)).py(px(8.)).child(
                        gpui_kit::component::alert::Alert::error("sync-error", message).small(),
                    ),
                )
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
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .items_center()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(form_row(t("sync.target", locale), self.target.clone(), cx))
                    .child(form_row(
                        t("sync.indexId", locale),
                        self.index_id.clone(),
                        cx,
                    ))
                    .child(form_row(
                        t("sync.generation", locale),
                        self.generation.clone(),
                        cx,
                    ))
                    .child(form_row(t("sync.limit", locale), self.limit.clone(), cx)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("sync-status")
                            .flex_1()
                            .min_w_0()
                            .gap_3()
                            .p(px(16.))
                            .overflow_y_scroll()
                            .child(
                                card(cx)
                                    .child(card_head(t("sync.status", locale), None, cx))
                                    .child(
                                        card_body(crate::settings::global(cx).density.is_dense())
                                            .child(render_json_block(&self.status, cx)),
                                    ),
                            )
                            .child(
                                card(cx)
                                    .child(card_head(t("sync.diagnostics", locale), None, cx))
                                    .child(
                                        card_body(crate::settings::global(cx).density.is_dense())
                                            .child(render_json_block(&self.diagnostics, cx)),
                                    ),
                            )
                            .child(
                                card(cx)
                                    .child(card_head(t("sync.retention", locale), None, cx))
                                    .child(
                                        card_body(crate::settings::global(cx).density.is_dense())
                                            .child(render_json_block(&self.retention, cx)),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("sync-lists")
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .p_2()
                            .border_l_1()
                            .border_color(theme.border)
                            .overflow_y_scroll()
                            .child(div().text_sm().text_color(theme.foreground).child(
                                SharedString::from(tf(
                                    "sync.deadLetters",
                                    locale,
                                    self.dead_letters.len(),
                                )),
                            ))
                            .children(self.dead_letters.iter().enumerate().map(|(ix, row)| {
                                render_row(row, ix, &theme, cx, "sync-dead", None)
                            }))
                            .when(self.loading && self.dead_letters.is_empty(), |list| {
                                list.child(skeleton_rows(6, cx))
                            })
                            .when(self.dead_letters.is_empty(), |list| {
                                list.child(empty_state(
                                    IconName::Inbox,
                                    t("sync.emptyTitle", locale),
                                    Some(t("sync.emptyBody", locale).to_string().into()),
                                    cx,
                                ))
                            })
                            .child(div().text_sm().text_color(theme.foreground).child(
                                SharedString::from(tf(
                                    "sync.degradedRanges",
                                    locale,
                                    self.degraded.len(),
                                )),
                            ))
                            .children(self.degraded.iter().enumerate().map(|(ix, row)| {
                                let key = degraded_key(row);
                                let confirm = (key.clone(), row.clone());
                                render_row(row, ix, &theme, cx, "sync-degraded", Some(confirm))
                            })),
                    ),
            )
    }
}

/// Extract the row array from a wrapped or bare payload, like the frontend.
fn rows_of(payload: &serde_json::Value, key: &str) -> Vec<serde_json::Value> {
    if let Some(wrapped) = payload.get(key).and_then(|value| value.as_array()) {
        return wrapped.clone();
    }
    if let Some(rows) = payload.as_array() {
        return rows.clone();
    }
    Vec::new()
}

/// Stable key for a degraded range row, used for the clear confirmation.
fn degraded_key(row: &serde_json::Value) -> String {
    let field = |name: &str| {
        row.get(name)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".to_string())
    };
    format!(
        "{}:{}:{}",
        field("target"),
        field("index_id"),
        field("generation")
    )
}

/// One dead-letter or degraded-range row; degraded rows carry a clear action.
fn render_row(
    row: &serde_json::Value,
    ix: usize,
    theme: &gpui_kit::component::Theme,
    cx: &mut Context<SyncPanel>,
    prefix: &'static str,
    clear: Option<(String, serde_json::Value)>,
) -> impl IntoElement {
    let locale = crate::i18n::locale(cx);
    h_flex()
        .gap_2()
        .items_center()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(theme.border)
        .child(Icon::new(IconName::FileText).text_color(theme.muted_foreground))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(theme.muted_foreground)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(SharedString::from(
                    serde_json::to_string(row).unwrap_or_else(|_| "?".to_string()),
                )),
        )
        .when_some(clear, |line, (key, row)| {
            line.child(
                Button::new((prefix, ix))
                    .label(t("sync.clear", locale))
                    .small()
                    .ghost()
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let view = cx.entity();
                        let key = key.clone();
                        let row = row.clone();
                        confirm(
                            window,
                            cx,
                            t("sync.clearTitle", locale),
                            tf("sync.clearBody", locale, &key),
                            t("sync.clear", locale),
                            true,
                            move |window, cx| {
                                view.update(cx, |this, cx| {
                                    this.clear_degraded(&row, window, cx);
                                });
                            },
                        );
                    })),
            )
        })
}
