use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, Context, EventEmitter, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{empty_state, render_json_block};
use crate::services::{connection_service, monitoring_service, transaction_service};

/// Transaction management panel: outcome/resource gauges plus the live
/// transaction table with kill and outbox-retry actions.
pub struct TransactionsPanel {
    overview: serde_json::Value,
    metrics: serde_json::Value,
    transactions: Vec<serde_json::Value>,
    loading: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    confirm_kill: Option<u64>,
    focus_handle: gpui_kit::FocusHandle,
}

impl TransactionsPanel {
    pub(crate) const PANEL_NAME: &'static str = "Transactions";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            overview: serde_json::Value::Null,
            metrics: serde_json::Value::Null,
            transactions: Vec::new(),
            loading: false,
            message: None,
            error: None,
            confirm_kill: None,
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
        self.confirm_kill = None;
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
                            "Failed to load transaction data; retry or check the connection."
                                .into(),
                        );
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
                this.confirm_kill = None;
                match outcome {
                    Ok(_) => {
                        this.message = Some(format!("Terminated transaction {txn_id}.").into());
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
                            Some(format!("Retried outbox for transaction {txn_id}.").into());
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

    fn render_gauges(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
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
                            .child(SharedString::from("Outcomes & resources")),
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
                            .child(SharedString::from("Overview")),
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
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from("Transactions")),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("transactions-refresh")
                            .icon(IconName::RefreshCw)
                            .xsmall()
                            .ghost()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(window, cx);
                            })),
                    ),
            )
            .when_some(self.error.clone(), |panel, message| {
                panel.child(
                    h_flex()
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
                    h_flex().flex_shrink_0().px_2().py_1().child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(message),
                    ),
                )
            })
            .when_some(self.confirm_kill, |panel, txn_id| {
                panel.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(theme.warning.opacity(0.1))
                        .child(div().flex_1().text_xs().text_color(theme.warning).child(
                            SharedString::from(format!("Terminate transaction {txn_id}?")),
                        ))
                        .child(
                            Button::new("transactions-kill-confirm")
                                .label("Confirm")
                                .xsmall()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.kill(txn_id, window, cx);
                                })),
                        )
                        .child(
                            Button::new("transactions-kill-cancel")
                                .label("Cancel")
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_kill = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(self.render_gauges(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from(format!(
                                "Active transactions ({})",
                                self.transactions.len()
                            ))),
                    )
                    .child(
                        v_flex()
                            .id("transactions-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .children(self.transactions.iter().map(|txn| {
                                render_transaction_row(txn, &theme, self.confirm_kill, cx)
                            }))
                            .when(self.transactions.is_empty(), |list| {
                                list.child(empty_state(
                                    IconName::Inbox,
                                    "No active transactions",
                                    Some("The server reports no running transactions.".into()),
                                    cx,
                                ))
                            }),
                    ),
            )
    }
}

/// One table-like transaction row with kill / outbox-retry actions.
fn render_transaction_row(
    txn: &serde_json::Value,
    theme: &gpui_kit::component::Theme,
    confirm_kill: Option<u64>,
    cx: &mut Context<TransactionsPanel>,
) -> impl IntoElement {
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
        .map(|reason| format!(" · blocked: {reason}"))
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
                    "{owner} · {elapsed} ms · staged {staged} · undo {undo}{blocked_suffix}"
                ))),
        )
        .when_some(txn_id, |row, id| {
            row.child(
                Button::new(("transactions-kill", id))
                    .label("Kill")
                    .xsmall()
                    .ghost()
                    .disabled(confirm_kill == Some(id))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm_kill = Some(id);
                        cx.notify();
                    })),
            )
            .when(blocking.is_some(), |row| {
                row.child(
                    Button::new(("transactions-retry", id))
                        .label("Retry outbox")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.retry_outbox(id, window, cx);
                        })),
                )
            })
        })
}
