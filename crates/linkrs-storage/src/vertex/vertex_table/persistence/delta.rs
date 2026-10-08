use std::path::Path;

use linkrs_core::StorageResult;

use super::super::core::VertexTable;

impl VertexTable {
    /// Strict delta apply for manifest-pinned checkpoints.
    /// A bad page name or an unknown column still refuses the apply: those
    /// signal a corrupt manifest or a schema divergence, not a single-column
    /// fault. A corrupt payload for a known column marks only that column
    /// unavailable and continues with the remaining pages, so one damaged
    /// column page never refuses the whole shard.
    pub fn apply_delta_pages(&mut self, shard_dir: &Path) -> StorageResult<()> {
        let delta_dir = shard_dir.join("columns_pages");
        if !delta_dir.exists() {
            return Ok(());
        }
        let mut applied: Vec<(String, usize)> = Vec::new();
        for entry in std::fs::read_dir(&delta_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("page") {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    let (col_name, _) = match path
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .and_then(|stem| stem.rsplit_once('_'))
                    {
                        Some((name, _)) => (name.to_string(), ()),
                        None => {
                            return Err(linkrs_core::StorageError::deserialize_error(format!(
                                "bad delta page name {}",
                                path.display()
                            )));
                        }
                    };
                    let reason = format!("delta page unreadable at {}: {}", path.display(), e);
                    log::warn!("column {} unavailable: {}", col_name, reason);
                    self.columns.mark_column_unavailable(&col_name, reason);
                    continue;
                }
            };
            let (col_name, page_id) = match path
                .file_stem()
                .and_then(|n| n.to_str())
                .and_then(|stem| stem.rsplit_once('_'))
            {
                Some((name, page)) => match page.parse::<usize>() {
                    Ok(id) => (name.to_string(), id),
                    Err(_) => {
                        return Err(linkrs_core::StorageError::deserialize_error(format!(
                            "bad delta page name {}",
                            path.display()
                        )));
                    }
                },
                None => {
                    return Err(linkrs_core::StorageError::deserialize_error(format!(
                        "bad delta page name {}",
                        path.display()
                    )));
                }
            };
            let Some(col) = self.columns.get_column(&col_name) else {
                return Err(linkrs_core::StorageError::deserialize_error(format!(
                    "delta page {} targets unknown column {}",
                    path.display(),
                    col_name
                )));
            };
            if let Err(e) = col.deserialize_page(&bytes) {
                let reason = format!(
                    "corrupt delta page {} for column {}: {}",
                    path.display(),
                    col_name,
                    e
                );
                log::warn!("column {} unavailable: {}", col_name, reason);
                self.columns.mark_column_unavailable(&col_name, reason);
                continue;
            }
            applied.push((col_name, page_id));
        }
        self.columns.clear_pages(&applied);
        Ok(())
    }
}
