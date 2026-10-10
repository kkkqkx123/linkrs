//! Prometheus metrics endpoint.
//!
//! Renders the live `StatsManager` state in the Prometheus text exposition
//! format so generic monitoring stacks can scrape this instance next to the
//! JSON statistics API.

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_metrics::{MetricType, StatsManager};

const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

#[utoipa::path(
    get,
    path = "/v1/metrics",
    operation_id = "get_v1_metrics",
    tag = "Statistics",
    responses(
        (status = 200, description = "Prometheus metrics in the text exposition format"),
        (status = 500, description = "Internal error")
    )
)]
/// Serve all collected metrics in the Prometheus text format.
pub async fn metrics<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
) -> Result<Response, HttpError> {
    let stats = state.server.get_stats_manager().clone();
    let body = render_prometheus(&stats);
    Ok(([(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)], body).into_response())
}

/// Render the Prometheus exposition body for one `StatsManager`.
///
/// Series are emitted in a stable order so scrapers and tests see the same
/// output for identical counter values.
pub(crate) fn render_prometheus(stats: &StatsManager) -> String {
    let mut out = String::with_capacity(4096);

    let mut global: Vec<(MetricType, u64)> = stats.get_all_metrics().into_iter().collect();
    global.sort_by_key(|(metric, _)| metric.prometheus_name());
    for (metric_type, value) in global {
        push_sample(&mut out, metric_type.prometheus_name(), &[], value);
    }

    let mut spaces: Vec<(String, Vec<(MetricType, u64)>)> = stats
        .all_space_metrics()
        .into_iter()
        .map(|(space, metrics)| {
            let mut entries: Vec<(MetricType, u64)> = metrics.into_iter().collect();
            entries.sort_by_key(|(metric, _)| metric.prometheus_name());
            (space, entries)
        })
        .collect();
    spaces.sort_by(|left, right| left.0.cmp(&right.0));
    for (space, metrics) in spaces {
        for (metric_type, value) in metrics {
            push_sample(
                &mut out,
                metric_type.prometheus_name(),
                &[("space", &space)],
                value,
            );
        }
    }

    let latency = stats.query_latency_snapshot();
    for (quantile, micros) in [
        ("0.5", latency.p50_us),
        ("0.95", latency.p95_us),
        ("0.99", latency.p99_us),
    ] {
        push_sample(
            &mut out,
            "linkrs_query_latency_seconds",
            &[("quantile", quantile)],
            seconds(micros),
        );
    }
    push_sample(
        &mut out,
        "linkrs_query_latency_seconds_count",
        &[],
        latency.count as u64,
    );

    let errors = stats.error_snapshot();
    push_sample(
        &mut out,
        "linkrs_query_errors_total",
        &[],
        errors.total_errors,
    );
    let mut by_type: Vec<(String, u64)> = errors.errors_by_type.into_iter().collect();
    by_type.sort_by(|left, right| left.0.cmp(&right.0));
    for (error_type, count) in by_type {
        push_sample(
            &mut out,
            "linkrs_query_errors_by_type_total",
            &[("type", &error_type)],
            count,
        );
    }

    push_sample(
        &mut out,
        "linkrs_aggregated_queries_total",
        &[],
        stats.get_total_aggregated_queries(),
    );
    push_sample(
        &mut out,
        "linkrs_aggregated_slow_queries_total",
        &[],
        stats.get_total_aggregated_slow_queries(),
    );
    push_sample(
        &mut out,
        "linkrs_aggregated_qps",
        &[],
        stats.get_current_qps(),
    );

    out
}

/// Append one `name{labels} value` line.
fn push_sample(
    out: &mut String,
    name: &str,
    labels: &[(&str, &str)],
    value: impl std::fmt::Display,
) {
    out.push_str(name);
    if !labels.is_empty() {
        out.push('{');
        for (index, (key, label_value)) in labels.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(key);
            out.push_str("=\"");
            out.push_str(&escape_label(label_value));
            out.push('"');
        }
        out.push('}');
    }
    out.push(' ');
    out.push_str(&value.to_string());
    out.push('\n');
}

/// Convert a microsecond duration to Prometheus seconds.
fn seconds(micros: u64) -> f64 {
    micros as f64 / 1_000_000.0
}

/// Escape a label value for the Prometheus text format.
fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use linkrs_metrics::{ErrorType, QueryMetrics, QueryPhase, QueryProfile};

    #[test]
    fn render_prometheus_emits_counters_and_latency_quantiles() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumMatchQueries, 3);
        stats.add_space_metric("default", MetricType::NumQueries);
        stats.record_error(ErrorType::ParseError, QueryPhase::Parse);

        for i in 1..=10u64 {
            let mut profile = QueryProfile::new(i as i64, format!("MATCH (n) RETURN {i}"));
            profile.total_duration_us = i * 1000;
            stats.record_query_profile(profile);

            // Only the numeric query metrics feed the latency histogram.
            let mut metrics = QueryMetrics::new();
            metrics.total_time_us = i * 1000;
            stats.record_query_metrics(&metrics);
        }

        let body = render_prometheus(&stats);

        assert!(body.contains("linkrs_queries_total 10\n"), "{body}");
        assert!(
            body.contains("linkrs_queries_by_type_match_total 3\n"),
            "{body}"
        );
        assert!(body.contains("linkrs_query_errors_total 1\n"), "{body}");
        assert!(
            body.contains("linkrs_query_errors_by_type_total{type="),
            "{body}"
        );
        assert!(
            body.contains("linkrs_aggregated_queries_total 10\n"),
            "{body}"
        );
        assert!(
            body.contains("linkrs_query_latency_seconds{quantile=\"0.5\"}"),
            "{body}"
        );
        assert!(
            body.contains("linkrs_query_latency_seconds_count 10\n"),
            "{body}"
        );
        assert!(body.contains("space=\"default\"} 1"), "{body}");
    }

    #[test]
    fn render_prometheus_is_deterministic() {
        let build = || {
            let stats = StatsManager::new();
            stats.add_value_with_amount(MetricType::NumQueries, 1);
            stats.add_space_metric("b_space", MetricType::NumQueries);
            stats.add_space_metric("a_space", MetricType::NumQueries);
            render_prometheus(&stats)
        };
        assert_eq!(build(), build(), "rendering must be stable");
    }

    #[test]
    fn escape_label_handles_quotes_and_backslashes() {
        assert_eq!(escape_label("plain"), "plain");
        assert_eq!(escape_label("a\"b"), "a\\\"b");
        assert_eq!(escape_label("a\\b"), "a\\\\b");
        assert_eq!(escape_label("a\nb"), "a\\nb");
    }
}
