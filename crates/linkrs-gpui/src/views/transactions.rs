use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, EventEmitter,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use linkrs_client::TransactionOptions;

use crate::components::{
    confirm, empty_state, form_row, panel_header, render_json_block, skeleton_rows,
};
use crate::i18n::{locale, t, tf};
use crate::services::{connection_service, monitoring_service, transaction_service};

/// Transaction management panel: explicit begin/commit/rollback, savepoint
/// management plus the live transaction table with kill and outbox-retry.
pub struct TransactionsPanel {
    overview: serde_json::Value,
    metrics: serde_json::Value,
    transactions: Vec<serde_json::Value>,
    savepoints: serde_json::Value,
    txn_id: Entity<InputState>,
    savepoint_name: Entity<InputState>,
    savepoint_id: Entity<InputState>,
    read_only: bool,
    loading: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
}

impl TransactionsPanel {
    pub(crate) const PANEL_NAME: &'static str = "Transactions";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = crate::state::AppState::global(cx);
        cx.observe(&state, |_, _, cx| cx.notify())
            .detach();
        let locale = locale(cx);
        let txn_id = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("transactions.txnIdPlaceholder", locale))
        });
        let savepoint_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("transactions.savepointNamePlaceholder", locale))
        });
        let savepoint_id = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("transactions.savepointIdPlaceholder", locale))
        });
        let mut panel = Self {
            overview: serde_json::Value::Null,
            metrics: serde_json::Value::Null,
            transactions: Vec::new(),
            savepoints: serde_json::Value::Null,
            txn_id,
            savepoint_name,
            savepoint_id,
            read_only: false,
            loading: false,
            message: None,
            error: None,
            focus_handle: cx.focus_handle(),
        };
        panel.refresh(window, cx);
        panel
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        let overview = monitoring_service::overview(cx);
        let metrics = transaction_service::metrics(cx);
        let list = transaction_service::list(cx);
        cx.spawn_in(window, async move |this, window| {
            let overview = overview.await;
            let metrics = metrics.await;
            let list = list.await;
            _ = this.update_in(window, |this, _, cx| {
                this.loading = false;
                match (overview, metrics, list) {
                    (Ok(overview), Ok(metrics), Ok(transactions)) => {
                        this.overview = overview;
                        this.metrics = metrics;
                        this.transactions = transactions;
                        this.error = None;
                    }
                    _ => {
                        this.error = Some(
                            t("transactions.loadFailed", crate::i18n::locale(cx))
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

    fn parse_txn_id(&self, cx: &App) -> Option<u64> {
        self.txn_id.read(cx).value().trim().parse::<u64>().ok()
    }

    fn parse_savepoint_id(&self, cx: &App) -> Option<u64> {
        self.savepoint_id
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .ok()
    }

    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut options = TransactionOptions::new();
        if self.read_only {
            options = options.read_only();
        }
        let task = transaction_service::begin(cx, options);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(response) => {
                        let locale = locale(cx);
                        this.message = Some(
                            crate::i18n::tf_args(
                                "transactions.began",
                                locale,
                                &[&response.transaction_id, &response.status],
                            )
                            .into(),
                        );
                        this.txn_id.update(cx, |field, cx| {
                            field.set_value(response.transaction_id.to_string(), window, cx);
                        });
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

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(txn_id) = self.parse_txn_id(cx) else {
            self.error = Some(t("transactions.idMustBeNumber", locale(cx)).into());
            cx.notify();
            return;
        };
        let task = transaction_service::commit(cx, txn_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(()) => {
                        this.message =
                            Some(tf("transactions.committed", locale(cx), txn_id).into());
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

    fn rollback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(txn_id) = self.parse_txn_id(cx) else {
            self.error = Some(t("transactions.idMustBeNumber", locale(cx)).into());
            cx.notify();
            return;
        };
        let task = transaction_service::rollback(cx, txn_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(()) => {
                        this.message =
                            Some(tf("transactions.rolledBack", locale(cx), txn_id).into());
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

    fn create_savepoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(txn_id) = self.parse_txn_id(cx) else {
            self.error = Some(t("transactions.idMustBeNumber", locale(cx)).into());
            cx.notify();
            return;
        };
        let name = self.savepoint_name.read(cx).value().trim().to_string();
        let name = (!name.is_empty()).then_some(name);
        let task = transaction_service::create_savepoint(cx, txn_id, name.clone());
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(response) => {
                        let locale = locale(cx);
                        this.message = Some(
                            crate::i18n::tf_args(
                                "transactions.savepointCreated",
                                locale,
                                &[&response.savepoint_id, &txn_id],
                            )
                            .into(),
                        );
                        this.savepoint_id.update(cx, |field, cx| {
                            field.set_value(response.savepoint_id.to_string(), window, cx);
                        });
                        this.list_savepoints_inner(txn_id, cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                        cx.notify();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn list_savepoints_inner(&mut self, txn_id: u64, cx: &mut Context<Self>) {
        let task = transaction_service::list_savepoints(cx, txn_id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(rows) => {
                        this.savepoints =
                            serde_json::to_value(&rows).unwrap_or(serde_json::Value::Null);
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

    fn list_savepoints(&mut self, _: &Window, cx: &mut Context<Self>) {
        let Some(txn_id) = self.parse_txn_id(cx) else {
            self.error = Some(t("transactions.idMustBeNumber", locale(cx)).into());
            cx.notify();
            return;
        };
        self.list_savepoints_inner(txn_id, cx);
    }

    fn rollback_to_savepoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(txn_id), Some(savepoint_id)) =
            (self.parse_txn_id(cx), self.parse_savepoint_id(cx))
        else {
            self.error = Some(t("transactions.bothIdsMustBeNumbers", locale(cx)).into());
            cx.notify();
            return;
        };
        let task = transaction_service::rollback_to_savepoint(cx, txn_id, savepoint_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(_) => {
                        let locale = locale(cx);
                        this.message = Some(
                            crate::i18n::tf_args(
                                "transactions.rolledBackTo",
                                locale,
                                &[&txn_id, &savepoint_id],
                            )
                            .into(),
                        );
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

    fn release_savepoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(txn_id), Some(savepoint_id)) =
            (self.parse_txn_id(cx), self.parse_savepoint_id(cx))
        else {
            self.error = Some(t("transactions.bothIdsMustBeNumbers", locale(cx)).into());
            cx.notify();
            return;
        };
        let task = transaction_service::release_savepoint(cx, txn_id, savepoint_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                match outcome {
                    Ok(_) => {
                        let locale = locale(cx);
                        this.message = Some(
                            crate::i18n::tf_args(
                                "transactions.savepointReleased",
                                locale,
                                &[&savepoint_id, &txn_id],
                            )
                            .into(),
                        );
                        this.list_savepoints_inner(txn_id, cx);
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

    fn kill(&mut self, txn_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let task = transaction_service::kill(cx, txn_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(_) => {
                        this.message =
                            Some(tf("transactions.terminated", locale(cx), txn_id).into());
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

    fn retry_outbox(&mut self, txn_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let task = transaction_service::retry_outbox(cx, txn_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                match outcome {
                    Ok(_) => {
                        this.message =
                            Some(tf("transactions.outboxRetried", locale(cx), txn_id).into());
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

    fn render_explicit(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let read_only = self.read_only;
        let locale = locale(cx);
        v_flex()
            .flex_shrink_0()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .child(SharedString::from(t("transactions.explicit", locale))),
            )
            .child(form_row(
                t("transactions.txnId", locale),
                self.txn_id.clone(),
                cx,
            ))
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(
                        Button::new("transactions-begin")
                            .label(t("transactions.begin", locale))
                            .icon(IconName::Plus)
                            .xsmall()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.begin(window, cx);
                            })),
                    )
                    .child(
                        Button::new("transactions-read-only")
                            .label(t("transactions.readOnly", locale))
                            .xsmall()
                            .ghost()
                            .selected(read_only)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.read_only = !this.read_only;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("transactions-commit")
                            .label(t("transactions.commit", locale))
                            .xsmall()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.commit(window, cx);
                            })),
                    )
                    .child(
                        Button::new("transactions-rollback")
                            .label(t("transactions.rollback", locale))
                            .xsmall()
                            .ghost()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.rollback(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .child(SharedString::from(t("transactions.savepoints", locale))),
            )
            .child(form_row(
                t("transactions.name", locale),
                self.savepoint_name.clone(),
                cx,
            ))
            .child(form_row(
                t("transactions.savepointId", locale),
                self.savepoint_id.clone(),
                cx,
            ))
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(
                        Button::new("transactions-savepoint-create")
                            .label(t("transactions.create", locale))
                            .xsmall()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_savepoint(window, cx);
                            })),
                    )
                    .child(
                        Button::new("transactions-savepoint-list")
                            .label(t("transactions.list", locale))
                            .xsmall()
                            .ghost()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.list_savepoints(window, cx);
                            })),
                    )
                    .child(
                        Button::new("transactions-savepoint-rollback")
                            .label(t("transactions.rollbackToButton", locale))
                            .xsmall()
                            .ghost()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.rollback_to_savepoint(window, cx);
                            })),
                    )
                    .child(
                        Button::new("transactions-savepoint-release")
                            .label(t("transactions.release", locale))
                            .xsmall()
                            .ghost()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.release_savepoint(window, cx);
                            })),
                    ),
            )
            .child(render_json_block(&self.savepoints, cx))
    }

    fn render_gauges(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = locale(cx);
        h_flex()
            .flex_shrink_0()
            .gap_2()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from(t("transactions.outcomes", locale))),
                    )
                    .child(render_json_block(&self.metrics, cx)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from(t("transactions.overview", locale))),
                    )
                    .child(render_json_block(&self.overview, cx)),
            )
    }
}

impl Focusable for TransactionsPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for TransactionsPanel {}

impl gpui_kit::base::dock::Panel for TransactionsPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for TransactionsPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for TransactionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        v_flex()
            .id("transactions-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.transactions", crate::i18n::locale(cx))),
                Some(SharedString::from(tf(
                    "transactions.activeSuffix",
                    locale(cx),
                    self.transactions.len(),
                ))),
                Some(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("transactions-refresh")
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
                        gpui_kit::component::alert::Alert::error("txn-error", message).small(),
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
                div()
                    .id("transactions-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(self.loading && self.transactions.is_empty(), |host| {
                        host.child(skeleton_rows(8, cx))
                    })
                    .child(self.render_explicit(cx))
                    .child(self.render_gauges(cx))
                    .child(
                        v_flex()
                            .flex_shrink_0()
                            .px_2()
                            .py_1()
                            .child(div().text_sm().text_color(theme.foreground).child(
                                SharedString::from(tf(
                                    "transactions.activeCount",
                                    locale(cx),
                                    self.transactions.len(),
                                )),
                            ))
                            .child(
                                v_flex()
                                    .children(
                                        self.transactions
                                            .iter()
                                            .map(|txn| render_transaction_row(txn, &theme, cx)),
                                    )
                                    .when(self.transactions.is_empty(), |list| {
                                        list.child(empty_state(
                                            IconName::Inbox,
                                            t("transactions.emptyTitle", locale(cx)),
                                            Some(
                                                t("transactions.emptyBody", locale(cx))
                                                    .to_string()
                                                    .into(),
                                            ),
                                            cx,
                                        ))
                                    }),
                            ),
                    ),
            )
    }
}

/// One table-like transaction row with kill / outbox-retry actions.
fn render_transaction_row(
    txn: &serde_json::Value,
    theme: &gpui_kit::component::Theme,
    cx: &mut Context<TransactionsPanel>,
) -> impl IntoElement {
    let locale = locale(cx);
    let txn_id = txn
        .get("transaction_id")
        .and_then(serde_json::Value::as_u64);
    let state = txn
        .get("state")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    let owner = txn
        .get("owner")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let elapsed = txn
        .get("elapsed_ms")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let staged = txn
        .get("staged_bytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let undo = txn
        .get("undo_bytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let blocking = txn
        .get("blocking_reason")
        .map(|value| value.to_string())
        .filter(|reason| reason != "null" && !reason.is_empty());
    let blocked_suffix = blocking
        .as_ref()
        .map(|reason| tf("transactions.blocked", locale, reason))
        .unwrap_or_default();

    h_flex()
        .gap_2()
        .items_center()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(theme.border)
        .child(Icon::new(IconName::Repeat).text_color(theme.muted_foreground))
        .child(
            div().w(px(80.)).text_sm().child(SharedString::from(
                txn_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "?".into()),
            )),
        )
        .child(
            div()
                .w(px(110.))
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(state)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(theme.muted_foreground)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(SharedString::from(format!(
                    "{owner}  {}{blocked_suffix}",
                    crate::i18n::tf_args(
                        "transactions.rowStats",
                        locale,
                        &[&elapsed, &staged, &undo]
                    )
                ))),
        )
        .when_some(txn_id, |row, id| {
            row.child(
                Button::new(("transactions-kill", id))
                    .label(t("transactions.kill", locale))
                    .small()
                    .ghost()
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let view = cx.entity();
                        let loc = crate::i18n::locale(cx);
                        confirm(
                            window,
                            cx,
                            t("transactions.killTitle", loc),
                            tf("transactions.killBody", loc, id),
                            t("transactions.terminate", loc),
                            true,
                            move |window, cx| {
                                view.update(cx, |this, cx| this.kill(id, window, cx));
                            },
                        );
                    })),
            )
            .when(blocking.is_some(), |row| {
                row.child(
                    Button::new(("transactions-retry", id))
                        .label(t("transactions.retryOutbox", locale))
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.retry_outbox(id, window, cx);
                        })),
                )
            })
        })
}
