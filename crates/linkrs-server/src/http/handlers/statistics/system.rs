//! System resource handlers and host probes.
//!
//! Collects process and host level resource usage for monitoring.

use axum::{extract::State, response::Json as JsonResponse};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_wire::meta::{ConnectionStats, MemoryUsage, SystemResourceResponse};

#[utoipa::path(
    get,
    path = "/v1/statistics/system",
    tag = "Statistics",
    responses(
        (status = 200, body = SystemResourceResponse, description = "System resource usage"),
        (status = 500, description = "Internal error")
    )
)]
/// Obtaining information about the use of system resources
pub async fn system<
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
) -> Result<JsonResponse<SystemResourceResponse>, HttpError> {
    Ok(JsonResponse(collect_system_resource(&state).await))
}

pub(crate) async fn collect_system_resource<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: &AppState<S>,
) -> SystemResourceResponse {
    let session_manager = state.server.get_session_manager();
    let stats_manager = state.server.get_stats_manager();

    let active_connections = session_manager.active_session_count().await;
    let max_connections = session_manager.max_connections();

    let (memory_used, memory_total) = get_memory_info();
    let cpu_usage = get_cpu_usage_two_sample().await;
    let process_memory = get_process_memory_bytes();
    let uptime_secs = get_process_uptime_secs().or(Some(sysinfo::System::uptime()));
    let max_fds = max_file_descriptors();

    let storage_path = state.server.get_config().storage_path().to_string();
    let data_path = std::path::PathBuf::from(&storage_path);
    let data_dir_size = dir_size(&data_path);
    let wal_dir_size = dir_size(&data_path.join("wal"));

    stats_manager.record_resource_sample(memory_used, memory_total, cpu_usage);

    SystemResourceResponse {
        cpu_usage_percent: cpu_usage,
        memory_usage: MemoryUsage {
            used_bytes: memory_used,
            total_bytes: memory_total,
        },
        connections: ConnectionStats {
            active: active_connections,
            total: active_connections,
            max: max_connections,
        },
        process_memory_bytes: process_memory,
        uptime_secs,
        data_dir_size_bytes: data_dir_size,
        wal_dir_size_bytes: wal_dir_size,
        max_file_descriptors: max_fds,
    }
}

/// Obtaining memory information (number of bytes used and total number of bytes)
/// Implementing cross-platform support using the sysinfo crate
pub(crate) fn get_memory_info() -> (u64, u64) {
    use sysinfo::System;

    // Create an instance of system information and refresh the memory information.
    let mut sys = System::new();
    sys.refresh_memory();

    // Get the total system memory and the used memory (both converted to bytes).
    let total_memory = sys.total_memory() * 1024;
    let used_memory = sys.used_memory() * 1024;

    (used_memory, total_memory)
}

/// Obtain the percentage of CPU usage with two samples so the delta is real.
pub(crate) async fn get_cpu_usage_two_sample() -> f64 {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_cpu_usage();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    sys.refresh_cpu_usage();

    let cpus = sys.cpus();
    if cpus.is_empty() {
        0.0
    } else {
        let avg_usage: f32 =
            cpus.iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / cpus.len() as f32;
        avg_usage as f64
    }
}

pub(crate) fn get_process_memory_bytes() -> Option<u64> {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let pid = sysinfo::get_current_pid().ok()?;
    sys.process(pid).map(|p| p.memory() * 1024)
}

pub(crate) fn get_process_uptime_secs() -> Option<u64> {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let pid = sysinfo::get_current_pid().ok()?;
    sys.process(pid).map(|p| p.run_time())
}

pub(crate) fn dir_size(path: &std::path::Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.is_file() {
        return Some(metadata.len());
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            visited += 1;
            if visited > 200_000 {
                return Some(total);
            }
            if let Ok(md) = entry.metadata() {
                if md.is_dir() {
                    stack.push(entry.path());
                } else {
                    total = total.saturating_add(md.len());
                }
            }
        }
    }
    Some(total)
}

pub(crate) fn max_file_descriptors() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/self/limits").ok()?;
    for line in content.lines() {
        if line.trim_start().starts_with("Max open files") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                if let Ok(v) = parts[3].parse::<u64>() {
                    return Some(v);
                }
            }
        }
    }
    None
}
