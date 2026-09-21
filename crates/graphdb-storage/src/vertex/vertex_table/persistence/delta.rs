use std::path::Path;

use graphdb_core::StorageResult;

use super::super::core::VertexTable;

impl VertexTable {
    /// Apply delta pages from an incremental checkpoint shard directory.
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
            let bytes = std::fs::read(&path)?;
            // File name encodes the target column: "<col>_<page>.page".
            // Only the named column may consume the page; a page that fails
            // to deserialize there is corrupt and must not be tried against
            // other columns (their payloads are independent).
            let (col_name, page_id) = match path
                .file_stem()
                .and_then(|n| n.to_str())
                .and_then(|stem| stem.rsplit_once('_'))
            {
                Some((name, page)) => match page.parse::<usize>() {
                    Ok(id) => (name.to_string(), id),
                    Err(_) => {
                        log::warn!("Skipping delta page with bad name {}", path.display());
                        continue;
                    }
                },
                None => {
                    log::warn!("Skipping delta page with bad name {}", path.display());
                    continue;
                }
            };
            let Some(col) = self.columns.get_column_mut(&col_name) else {
                log::warn!(
                    "Skipping delta page {} for unknown column {}",
                    path.display(),
                    col_name
                );
                continue;
            };
            if let Err(e) = col.deserialize_page(&bytes) {
                log::warn!(
                    "Skipping corrupted delta page {} for column {}: {}",
                    path.display(),
                    col_name,
                    e
                );
                continue;
            }
            applied.push((col_name, page_id));
        }
        // Mark only successfully applied pages clean; corrupt or unknown
        // pages stay dirty so the next flush retries them.
        self.columns.clear_pages(&applied);
        Ok(())
    }
}
