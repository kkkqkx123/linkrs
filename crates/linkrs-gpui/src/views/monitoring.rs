use std::collections::VecDeque;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::input::InputState;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::{
    div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, App,
    AppContext as _, Context, Entity, EventEmitter, Focusable, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window,
};

use crate::components::{form_row, render_json_block};
use crate::services::{
    config_service, connection_service, fulltext_service, monitoring_service, query_service,
    stats_service, vector_service,
};

/// Seconds between monitoring polls when auto-refresh is on.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Points kept per chart series.
const MAX_POINTS: usize = 60;

#[derive(Clone)]
struct MetricPoint {
    index: usize,
    value: f64,
}

/// Monitoring panel: overview/system/query/database snapshots with polled
/// QPS and latency charts plus the raw JSON payload.
pub struct MonitoringPanel {
    auto_refresh: bool,
    generation: u64,
    loading: bool,
    message: Option<SharedString>,
    qps: VecDeque<MetricPoint>,
    latency: VecDeque<MetricPoint>,
    overview: serde_json::Value,
    system: serde_json::Value,
    queries: serde_json::Value,
    database: serde_json::Value,
    search: serde_json::Value,
    freeze: serde_json::Value,
    session: serde_json::Value,
    migration: serde_json::Value,
    retrieval: serde_json::Value,
    confirm_freeze: bool,
    cpu_threshold: Entity<InputState>,
    mem_threshold: Entity<InputState>,
    slow_threshold: Entity<InputState>,
    portrait_trace: Option<String>,
    portrait: serde_json::Value,
    portrait_loading: bool,
    focus_handle: gpui_kit::FocusHandle,
}

impl MonitoringPanel {
    pub(crate) const PANEL_NAME: &'static str = "Monitoring";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            auto_refresh: true,
            generation: 0,
            loading: false,
            message: None,
            qps: VecDeque::with_capacity(MAX_POINTS),
            latency: VecDeque::with_capacity(MAX_POINTS),
            overview: serde_json::Value::Null,
            system: serde_json::Value::Null,
            queries: serde_json::Value::Null,
            database: serde_json::Value::Null,
            search: serde_json::Value::Null,
            freeze: serde_json::Value::Null,
            session: serde_json::Value::Null,
            migration: serde_json::Value::Null,
            retrieval: serde_json::Value::Null,
            confirm_freeze: false,
            cpu_threshold: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("80")
            }),
            mem_threshold: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("80")
            }),
            slow_threshold: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("100")
            }),
            portrait_trace: None,
            portrait: serde_json::Value::Null,
            portrait_loading: false,
            focus_handle: cx.focus_handle(),
        };
        panel.refresh(cx);
        panel.spawn_loop(cx);
        panel
    }

    /// Background poll loop: each generation bump restarts the cadence so a
    /// dropped panel stops polling through its dead weak handle.
    fn spawn_loop(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let view = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let alive = view
                .update(cx, |this, cx| {
                    if this.generation != generation || !this.auto_refresh {
                        return false;
                    }
                    this.refresh(cx);
                    true
                })
                .unwrap_or(false);
            if !alive {
                break;
            }
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();

        let overview = monitoring_service::overview(cx);
        let system = monitoring_service::system(cx);
        let queries = monitoring_service::query_stats(cx);
        let database = monitoring_service::database(cx);
        let search = stats_service::search(cx);
        let freeze = stats_service::freeze(cx);
        let session = stats_service::session(cx);
        let migration = stats_service::migration(cx);
        let config = config_service::get(cx);
        let vector_indexes = vector_service::list_indexes(cx);
        let fulltext_inconsistent = fulltext_service::list_inconsistent(cx);

        cx.spawn(async move |this, cx| {
            let overview = overview.await;
            let system = system.await;
            let queries = queries.await;
            let database = database.await;
            let search = search.await;
            let freeze = freeze.await;
            let session = session
                .await
                .map(|stats| serde_json::to_value(&stats).unwrap_or(serde_json::Value::Null));
            let migration = migration.await;
            let config = config.await.ok();
            let vector_indexes = vector_indexes.await.ok();
            let fulltext_inconsistent = fulltext_inconsistent.await.ok();
            this.update(cx, |this, cx| {
                this.loading = false;
                let mut failed: Option<String> = None;
                match overview {
                    Ok(value) => this.overview = value,
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        failed = Some(error.to_string());
                    }
                }
                match system {
                    Ok(value) => this.system = value,
                    Err(error) => failed = failed.or_else(|| Some(error.to_string())),
                }
                match queries {
                    Ok(value) => {
                        push_point(&mut this.qps, pick(&value, &["qps", "queries_per_second"]));
                        push_point(
                            &mut this.latency,
                            pick(&value, &["avg_latency_ms", "p99_latency_ms", "latency_ms"]),
                        );
                        this.queries = value;
                    }
                    Err(error) => failed = failed.or_else(|| Some(error.to_string())),
                }
                match database {
                    Ok(value) => this.database = value,
                    Err(error) => failed = failed.or_else(|| Some(error.to_string())),
                }
                // Best-effort extras: a failing section keeps its previous
                // payload instead of failing the whole refresh.
                if let Ok(value) = search {
                    this.search = value;
                }
                if let Ok(value) = freeze {
                    this.freeze = value;
                }
                if let Ok(value) = session {
                    this.session = value;
                }
                if let Ok(value) = migration {
                    this.migration = value;
                }
                this.retrieval = retrieval_snapshot(
                    config.as_ref(),
                    vector_indexes.as_ref(),
                    fulltext_inconsistent.as_ref(),
                );
                if let Some(error) = failed.as_deref() {
                    this.message = Some(error.to_string().into());
                } else {
                    this.message = None;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn trigger_freeze(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_freeze = false;
        let task = stats_service::trigger_freeze(cx);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _window, cx| {
                match outcome {
                    Ok(value) => {
                        this.freeze = value;
                        this.message = Some("Freeze triggered.".into());
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.message = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn threshold(&self, field: &Entity<InputState>, cx: &App, default: f64) -> f64 {
        field
            .read(cx)
            .value()
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| *value > 0.0)
            .unwrap_or(default)
    }

    /// Slow queries at or above the configured millisecond threshold.
    fn slow_rows(&self, cx: &App) -> Vec<(String, i64, String, f64, String)> {
        let min_ms = self.threshold(&self.slow_threshold, cx, 100.0);
        self.queries
            .get("slow_queries")
            .and_then(|rows| rows.as_array())
            .map(|rows| {
                rows
                    .iter()
                    .filter_map(|row| {
                        let duration = row.get("duration_ms")?.as_f64()?;
                        if duration < min_ms {
                            return None;
                        }
                        Some((
                            row.get("trace_id")?.as_str()?.to_string(),
                            row.get("session_id")?.as_i64().unwrap_or(0),
                            row.get("query")?.as_str().unwrap_or("").to_string(),
                            duration,
                            row.get("status")?.as_str().unwrap_or("").to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Fetch the server portrait for one slow-query trace.
    fn view_portrait(&mut self, window: &mut Window, cx: &mut Context<Self>, trace_id: String) {
        self.portrait_trace = Some(trace_id.clone());
        self.portrait_loading = true;
        cx.notify();
        let task = query_service::profile(cx, &trace_id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, _window, cx| {
                this.portrait_loading = false;
                match outcome {
                    Ok(detail) => {
                        this.portrait = serde_json::to_value(&detail)
                            .unwrap_or(serde_json::Value::Null);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.message = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_chart(
        &self,
        title: &str,
        unit: &str,
        points: &VecDeque<MetricPoint>,
        color: Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let data: Vec<MetricPoint> = points.iter().cloned().collect();
        let current = data.last().map(|point| point.value).unwrap_or(0.0);
        v_flex()
            .min_h(px(160.))
            .flex_1()
            .gap_2()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .child(
                h_flex()
                    .justify_between()
                    .py_1()
                    .px_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(SharedString::from(title.to_string())),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(color)
                            .child(SharedString::from(format!("{current:.2} {unit}"))),
                    ),
            )
            .child(
                AreaChart::new(data)
                    .x(|point| point.index.to_string())
                    .y(|point| point.value)
                    .stroke(color)
                    .fill(linear_gradient(
                        0.,
                        linear_color_stop(color.opacity(0.4), 1.),
                        linear_color_stop(cx.theme().background.opacity(0.1), 0.),
                    ))
                    .tick_margin(15),
            )
    }

    fn render_slow_queries(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self.slow_rows(cx);
        let count = rows.len();
        let mut section = v_flex().gap_1().child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .child(SharedString::from(format!("Slow queries ({count})"))),
        );
        if rows.is_empty() {
            section = section.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("No slow queries above the threshold."),
            );
        } else {
            section = section.child(v_flex().gap_1().children(rows.into_iter().enumerate().map(
                |(ix, (trace_id, session_id, query, duration, status))| {
                    let label = trace_id.clone();
                    let selected = self.portrait_trace.as_deref() == Some(trace_id.as_str());
                    let summary = if query.chars().count() > 80 {
                        format!("{}…", query.chars().take(80).collect::<String>())
                    } else {
                        query
                    };
                    h_flex()
                        .id(("slow-row", ix))
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(if selected { theme.info } else { theme.border })
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground)
                                .child(SharedString::from(format!(
                                    "{duration:.1}ms · session {session_id} · {status}"
                                ))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(SharedString::from(summary)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.view_portrait(window, cx, label.clone());
                        }))
                },
            )));
        }
        if self.portrait_trace.is_some() {
            let title = self
                .portrait_trace
                .clone()
                .map(|trace| format!("Portrait: {trace}"))
                .unwrap_or_default();
            section = section.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from(title)),
                    )
                    .child(if self.portrait_loading {
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Loading portrait…")
                            .into_any_element()
                    } else {
                        render_json_block(&self.portrait, cx).into_any_element()
                    }),
            );
        }
        section
    }

    fn render_thresholds(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let cpu = pick(&self.system, &["cpu_usage_percent"]);
        let mem_ratio = self
            .system
            .get("memory_usage")
            .and_then(|usage| {
                let used = usage.get("used_bytes")?.as_f64()?;
                let total = usage.get("total_bytes")?.as_f64()?;
                (total > 0.0).then(|| used / total * 100.0)
            })
            .or_else(|| pick(&self.system, &["memory_usage_percent"]));
        let cpu_limit = self.threshold(&self.cpu_threshold, cx, 80.0);
        let mem_limit = self.threshold(&self.mem_threshold, cx, 80.0);
        let mut exceeded = Vec::new();
        if cpu.map(|value| value > cpu_limit).unwrap_or(false) {
            exceeded.push(format!("CPU {:.1}% > {cpu_limit:.0}%", cpu.unwrap_or(0.0)));
        }
        if mem_ratio.map(|value| value > mem_limit).unwrap_or(false) {
            exceeded.push(format!("Memory {:.1}% > {mem_limit:.0}%", mem_ratio.unwrap_or(0.0)));
        }
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .flex_wrap()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .child("Thresholds"),
            )
            .child(form_row("CPU % >", self.cpu_threshold.clone(), cx))
            .child(form_row("Memory % >", self.mem_threshold.clone(), cx))
            .child(form_row("Slow ms >", self.slow_threshold.clone(), cx))
            .when(!exceeded.is_empty(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(SharedString::from(exceeded.join(" · "))),
                )
            })
    }

    fn render_stat_cards(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let cards: Vec<(String, String)> = vec![
            (
                "Spaces".to_string(),
                number(&self.database, &["space_count"])
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".to_string()),
            ),
            (
                "Vertices".to_string(),
                number(&self.database, &["total_vertices"])
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".to_string()),
            ),
            (
                "Edges".to_string(),
                number(&self.database, &["total_edges"])
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".to_string()),
            ),
            (
                "Active queries".to_string(),
                number(&self.database, &["active_queries"])
                    .or_else(|| number(&self.queries, &["active_queries"]))
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".to_string()),
            ),
        ];
        h_flex()
            .flex_shrink_0()
            .flex_wrap()
            .gap_2()
            .children(cards.into_iter().map(|(label, value)| {
                v_flex()
                    .w(px(150.))
                    .gap_1()
                    .p_3()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(label)),
                    )
                    .child(
                        div()
                            .text_lg()
                            .text_color(theme.foreground)
                            .child(SharedString::from(value)),
                    )
            }))
    }
}

impl Focusable for MonitoringPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for MonitoringPanel {}

impl gpui_kit::base::dock::Panel for MonitoringPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for MonitoringPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for MonitoringPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        v_flex()
            .id("monitoring-panel")
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
                        Button::new("monitoring-refresh")
                            .icon(IconName::RefreshCw)
                            .xsmall()
                            .ghost()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("monitoring-auto")
                            .label("Auto refresh (5s)")
                            .xsmall()
                            .ghost()
                            .selected(self.auto_refresh)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.auto_refresh = !this.auto_refresh;
                                if this.auto_refresh {
                                    this.spawn_loop(cx);
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(theme.muted_foreground).child(
                        SharedString::from(if self.loading {
                            "Refreshing…"
                        } else {
                            "Idle"
                        }),
                    )),
            )
            .when_some(self.message.clone(), |panel, message| {
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
            .child(
                div()
                    .id("monitoring-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(
                        v_flex()
                            .gap_3()
                            .child(self.render_stat_cards(cx))
                            .child(self.render_thresholds(cx))
                            .child(self.render_slow_queries(cx))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .child(self.render_chart(
                                        "Queries per second",
                                        "qps",
                                        &self.qps,
                                        theme.blue,
                                        cx,
                                    ))
                                    .child(self.render_chart(
                                        "Average latency",
                                        "ms",
                                        &self.latency,
                                        theme.red,
                                        cx,
                                    )),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(SharedString::from("Overview payload")),
                                    )
                                    .child(render_json_block(&self.overview, cx)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(SharedString::from("Search statistics")),
                                    )
                                    .child(render_json_block(&self.search, cx)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_sm()
                                                    .text_color(theme.foreground)
                                                    .child(SharedString::from("Freeze statistics")),
                                            )
                                            .child(
                                                Button::new("monitoring-freeze")
                                                    .label("Trigger freeze")
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.loading)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.confirm_freeze = true;
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                    .when(self.confirm_freeze, |area| {
                                        area.child(
                                            h_flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .text_xs()
                                                        .text_color(theme.warning)
                                                        .child(SharedString::from(
                                                            "Trigger a freeze now?",
                                                        )),
                                                )
                                                .child(
                                                    Button::new("monitoring-freeze-confirm")
                                                        .label("Confirm")
                                                        .xsmall()
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.trigger_freeze(window, cx);
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    Button::new("monitoring-freeze-cancel")
                                                        .label("Cancel")
                                                        .xsmall()
                                                        .ghost()
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.confirm_freeze = false;
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                    })
                                    .child(render_json_block(&self.freeze, cx)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(SharedString::from("Current session")),
                                    )
                                    .child(render_json_block(&self.session, cx)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(SharedString::from("Migration statistics")),
                                    )
                                    .child(render_json_block(&self.migration, cx)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(SharedString::from("Retrieval snapshot")),
                                    )
                                    .child(render_json_block(&self.retrieval, cx)),
                            ),
                    ),
            )
    }
}

/// Push one sample, keeping the ring buffer bounded.
fn push_point(series: &mut VecDeque<MetricPoint>, value: Option<f64>) {
    let index = series.back().map(|point| point.index + 1).unwrap_or(0);
    series.push_back(MetricPoint {
        index,
        value: value.unwrap_or(0.0),
    });
    while series.len() > MAX_POINTS {
        series.pop_front();
    }
}

/// First numeric hit across candidate keys (top level and one level deep).
fn pick(payload: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(value) = monitoring_service::number_at(payload, key) {
            return Some(value);
        }
        if let Some(object) = payload.as_object() {
            for (_, child) in object {
                if let Some(value) = monitoring_service::number_at(child, key) {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn number(payload: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    pick(payload, keys).map(|value| value as i64)
}

/// Best-effort retrieval health snapshot, mirroring the frontend retrieval
/// service: config sections decide enabled/disabled, the index listings
/// double as reachability probes. Unknown backends stay `null`.
fn retrieval_snapshot(
    config: Option<&serde_json::Value>,
    vector: Option<&serde_json::Value>,
    fulltext: Option<&serde_json::Value>,
) -> serde_json::Value {
    let section = |name: &str| config.and_then(|config| config.get(name));
    let enabled = |name: &str| {
        section(name)
            .and_then(|section| section.get("enabled"))
            .and_then(serde_json::Value::as_bool)
    };
    let count = |payload: Option<&serde_json::Value>| {
        let payload = payload?;
        if let Some(count) = payload.get("count").and_then(serde_json::Value::as_u64) {
            return Some(count);
        }
        if let Some(indexes) = payload.get("indexes").and_then(|value| value.as_array()) {
            return Some(indexes.len() as u64);
        }
        payload.as_array().map(|rows| rows.len() as u64)
    };
    let probe = |is_enabled: Option<bool>, listed: bool| match (is_enabled, listed) {
        (Some(false), _) => "disabled",
        (_, true) => "reachable",
        (Some(true), false) => "unreachable",
        (None, false) => "unknown",
    };
    let vector_enabled = enabled("vector");
    let fulltext_enabled = enabled("fulltext");
    serde_json::json!({
        "vector": {
            "enabled": vector_enabled,
            "engine": section("vector").and_then(|s| s.get("engine")),
            "index_count": count(vector),
            "probe": probe(vector_enabled, vector.is_some()),
        },
        "fulltext": {
            "enabled": fulltext_enabled,
            "inconsistent_count": count(fulltext),
            "probe": probe(fulltext_enabled, fulltext.is_some()),
        },
    })
}
