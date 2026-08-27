// Metrics for remote (Qdrant) engine operations are collected by fetching
// the server-side `/telemetry` endpoint, not by client-side instrumentation.
// This module is intentionally empty; the telemetry fetcher lives in
// `graphdb-server/src/vector_metrics.rs`.
