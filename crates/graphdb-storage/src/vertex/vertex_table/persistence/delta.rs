use std::path::Path;

use graphdb_core::StorageResult;

use super::super::core::VertexTable;

impl VertexTable {
    /// Strict delta apply for manifest-pinned checkpoints.
    /// Every page must decode: a bad name, an unknown column, or a corrupt
    /// payload refuses the open instead of running with silently skipped data.
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
            let (col_name, page_id) = match path
                .file_stem()
                .and_then(|n| n.to_str())
                .and_then(|stem| stem.rsplit_once('_'))
            {
                Some((name, page)) => match page.parse::<usize>() {
                    Ok(id) => (name.to_string(), id),
                    Err(_) => {
                        return Err(graphdb_core::StorageError::deserialize_error(format!(
                            "bad delta page name {}",
                            path.display()
                        )));
                    }
                },
                None => {
                    return Err(graphdb_core::StorageError::deserialize_error(format!(
                        "bad delta page name {}",
                        path.display()
                    )));
                }
            };
            let Some(col) = self.columns.get_column(&col_name) else {
                return Err(graphdb_core::StorageError::deserialize_error(format!(
                    "delta page {} targets unknown column {}",
                    path.display(),
                    col_name
                )));
            };
            col.deserialize_page(&bytes).map_err(|e| {
                graphdb_core::StorageError::deserialize_error(format!(
                    "corrupt delta page {} for column {}: {}",
                    path.display(),
                    col_name,
                    e
                ))
            })?;
            applied.push((col_name, page_id));
        }
        self.columns.clear_pages(&applied);
        Ok(())
    }
}
