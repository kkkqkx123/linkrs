//! Session, query, database and system statistics handlers.

use std::collections::HashMap;

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::convert::profile_start_ms;
use super::error::parse_session_id;
use super::proto::*;
use super::service::GraphDBService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > GraphDBService<S>
{
    pub(crate) async fn handle_get_session_statistics(
        &self,
        request: Request<GetSessionStatisticsRequest>,
    ) -> Result<Response<GetSessionStatisticsResponse>, Status> {
        let req = request.into_inner();
        let session_manager = self.app_state.server.get_session_manager();
        if let Some(raw) = req.session_id.filter(|s| !s.is_empty()) {
            let session_id = parse_session_id(&raw)?;
            if session_manager.find_session(session_id).is_none() {
                return Err(Status::not_found(format!("session '{raw}' not found")));
            }
        }
        let sessions = session_manager.list_sessions().await;
        let mut by_user: HashMap<String, i64> = HashMap::new();
        for session in &sessions {
            *by_user.entry(session.user_name.clone()).or_insert(0) += 1;
        }
        let failed = self
            .app_state
            .server
            .get_stats_manager()
            .get_value(graphdb_metrics::MetricType::NumAuthFailedSessions)
            .unwrap_or(0) as i64;
        let total = session_manager.total_sessions_created() as i64;
        Ok(Response::new(GetSessionStatisticsResponse {
            active_sessions: sessions.len() as i64,
            total_sessions: total,
            failed_sessions: failed,
            session_by_user: by_user,
        }))
    }

    pub(crate) async fn handle_get_query_statistics(
        &self,
        request: Request<GetQueryStatisticsRequest>,
    ) -> Result<Response<GetQueryStatisticsResponse>, Status> {
        let req = request.into_inner();
        let from_ms = req.from_timestamp.unwrap_or(0);
        let to_ms = req.to_timestamp.unwrap_or(i64::MAX);
        if from_ms < 0 || to_ms < 0 || from_ms > to_ms {
            return Err(Status::invalid_argument(
                "from_timestamp/to_timestamp must be non-negative epoch millis with from <= to",
            ));
        }
        let in_window = |q: &graphdb_metrics::QueryProfile| {
            let ts = profile_start_ms(q);
            ts >= from_ms && ts <= to_ms
        };
        let stats_manager = self.app_state.server.get_stats_manager();
        let total = stats_manager
            .get_value(graphdb_metrics::MetricType::NumQueries)
            .unwrap_or(0) as i64;
        let slow: Vec<graphdb_metrics::QueryProfile> = stats_manager
            .get_slow_queries(10)
            .into_iter()
            .filter(in_window)
            .collect();
        let recent: Vec<graphdb_metrics::QueryProfile> = stats_manager
            .get_recent_queries(200)
            .into_iter()
            .filter(in_window)
            .collect();
        let failed = recent
            .iter()
            .filter(|q| q.status == graphdb_metrics::QueryStatus::Failed)
            .count() as i64;
        let (avg_ms, max_ms) = if recent.is_empty() {
            (0, 0)
        } else {
            let sum_ms: u64 = recent.iter().map(|q| q.total_duration_us / 1000).sum();
            let max_ms: u64 = recent
                .iter()
                .map(|q| q.total_duration_us / 1000)
                .max()
                .unwrap_or(0);
            ((sum_ms / recent.len() as u64) as i64, max_ms as i64)
        };
        Ok(Response::new(GetQueryStatisticsResponse {
            total_queries: total,
            slow_queries: slow.len() as i64,
            failed_queries: failed,
            avg_execution_time_ms: avg_ms,
            max_execution_time_ms: max_ms,
            slow_query_list: slow
                .into_iter()
                .map(|q| {
                    let timestamp = profile_start_ms(&q);
                    SlowQuery {
                        query: q.query_text,
                        execution_time_ms: (q.total_duration_us / 1000) as i64,
                        timestamp,
                        session_id: Some(q.session_id.to_string()),
                    }
                })
                .collect(),
        }))
    }

    pub(crate) async fn handle_get_database_statistics(
        &self,
        _request: Request<GetDatabaseStatisticsRequest>,
    ) -> Result<Response<GetDatabaseStatisticsResponse>, Status> {
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let stats = storage_guard.get_storage_stats();
        Ok(Response::new(GetDatabaseStatisticsResponse {
            total_spaces: stats.total_spaces as i32,
            total_vertices: stats.total_vertices as i64,
            total_edges: stats.total_edges as i64,
            storage_size_bytes: stats.total_size_bytes as i64,
        }))
    }

    pub(crate) async fn handle_get_system_statistics(
        &self,
        _request: Request<GetSystemStatisticsRequest>,
    ) -> Result<Response<GetSystemStatisticsResponse>, Status> {
        let (memory_used, memory_total) = {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            (sys.used_memory() * 1024, sys.total_memory() * 1024)
        };
        let cpu_usage = {
            let mut sys = sysinfo::System::new();
            sys.refresh_cpu_usage();
            let cpus = sys.cpus();
            if cpus.is_empty() {
                0.0
            } else {
                let avg: f32 =
                    cpus.iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / cpus.len() as f32;
                avg as f64
            }
        };
        let active = self
            .app_state
            .server
            .get_session_manager()
            .active_session_count()
            .await as i32;
        let (disk_used_bytes, disk_total_bytes) = {
            let disks = sysinfo::Disks::new_with_refreshed_list();
            disks
                .list()
                .iter()
                .fold((0u64, 0u64), |(used, total), disk| {
                    (
                        used + disk.total_space().saturating_sub(disk.available_space()),
                        total + disk.total_space(),
                    )
                })
        };
        let disk_usage_percent = if disk_total_bytes > 0 {
            disk_used_bytes as f64 / disk_total_bytes as f64 * 100.0
        } else {
            0.0
        };
        let (network_rx_bytes, network_tx_bytes) = {
            let networks = sysinfo::Networks::new_with_refreshed_list();
            networks
                .list()
                .values()
                .fold((0u64, 0u64), |(rx, tx), data| {
                    (rx + data.received(), tx + data.transmitted())
                })
        };
        Ok(Response::new(GetSystemStatisticsResponse {
            cpu_usage_percent: cpu_usage,
            memory_used_bytes: memory_used as i64,
            memory_total_bytes: memory_total as i64,
            disk_usage_percent,
            active_connections: active,
            network_rx_bytes: network_rx_bytes as i64,
            network_tx_bytes: network_tx_bytes as i64,
        }))
    }
}
