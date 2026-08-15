//! Executors for the `\snapshot` meta command.

use crate::command::executor::CommandExecutor;
use crate::command::parser::types::SnapshotAction;
use crate::session::SessionManager;
use crate::utils::error::Result;

pub async fn execute_snapshot(
    executor: &mut CommandExecutor,
    action: SnapshotAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    match action {
        SnapshotAction::List => execute_list(executor, session_mgr).await,
        SnapshotAction::Info { label } => execute_info(executor, session_mgr, label).await,
        SnapshotAction::Load { path } => execute_load(executor, session_mgr, &path).await,
        SnapshotAction::Remove { label } => execute_remove(executor, session_mgr, label).await,
        SnapshotAction::Export { label, path } => {
            execute_export(executor, session_mgr, label, &path).await
        }
        SnapshotAction::Merge { labels } => execute_merge(executor, session_mgr, &labels).await,
    }
}

fn format_info(info: &crate::client::ColdSnapshotInfo) -> String {
    format!(
        "label={} ({}) | ts={} | edges={} | file={} | size={}B | crc32={:#010x}",
        info.label,
        if info.label_name.is_empty() {
            "unknown"
        } else {
            info.label_name.as_str()
        },
        info.snapshot_ts,
        info.edge_count,
        if info.file_path.is_empty() {
            "in-memory"
        } else {
            info.file_path.as_str()
        },
        info.file_size,
        info.checksum
    )
}

async fn execute_list(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    let snapshots = session_mgr.list_cold_snapshots().await?;
    if snapshots.is_empty() {
        executor.write_output("No cold snapshots registered.")?;
        return Ok(true);
    }
    let lines: Vec<String> = snapshots.iter().map(format_info).collect();
    executor.write_output(&lines.join("\n"))?;
    Ok(true)
}

async fn execute_info(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
    label: u32,
) -> Result<bool> {
    let snapshots = session_mgr.list_cold_snapshots().await?;
    let matches: Vec<&crate::client::ColdSnapshotInfo> =
        snapshots.iter().filter(|s| s.label == label).collect();
    if matches.is_empty() {
        executor.write_output(&format!("No cold snapshot for label {}.", label))?;
        return Ok(true);
    }
    let lines: Vec<String> = matches.iter().map(|s| format_info(s)).collect();
    executor.write_output(&lines.join("\n"))?;
    Ok(true)
}

async fn execute_load(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
    path: &str,
) -> Result<bool> {
    let info = session_mgr.load_cold_snapshot(path).await?;
    executor.write_output(&format!("Loaded: {}", format_info(&info)))?;
    Ok(true)
}

async fn execute_remove(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
    label: u32,
) -> Result<bool> {
    session_mgr.remove_cold_snapshot(label).await?;
    executor.write_output(&format!("Removed cold snapshots of label {}.", label))?;
    Ok(true)
}

async fn execute_export(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
    label: u32,
    path: &str,
) -> Result<bool> {
    let info = session_mgr.export_cold_snapshot(label, path).await?;
    executor.write_output(&format!("Exported: {}", format_info(&info)))?;
    Ok(true)
}

async fn execute_merge(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
    labels: &[u32],
) -> Result<bool> {
    let merged = session_mgr.merge_cold_snapshots(labels).await?;
    if merged.is_empty() {
        executor.write_output("No snapshots to merge.")?;
        return Ok(true);
    }
    let lines: Vec<String> = merged.iter().map(format_info).collect();
    executor.write_output(&format!("Merged:\n{}", lines.join("\n")))?;
    Ok(true)
}
