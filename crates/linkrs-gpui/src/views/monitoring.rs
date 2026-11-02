use std::collections::VecDeque;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::{
    div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, App, Context,
    EventEmitter, Focusable, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::render_json_block;
use crate::services::{connection_service, monitoring_service};

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
    focus_handle: gpui_kit::FocusHandle,
}

impl MonitoringPanel {
    pub(crate) const PANEL_NAME: &'static str = "Monitoring";

    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
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

        cx.spawn(async move |this, cx| {
            let overview = overview.await;
            let system = system.await;
            let queries = queries.await;
            let database = database.await;
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
