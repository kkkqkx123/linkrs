use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use graphdb_core::{StorageError, StorageResult};

use super::format::{parse_header, serving_error, write_serving_file};
use super::MappedFrozen;
use crate::edge::ImmutableCsr;

impl MappedFrozen {
    /// Open and validate a serving file. Any structural problem is an error;
    /// callers fall back to the authoritative checkpoint.
    pub fn open(path: &Path) -> StorageResult<Self> {
        Self::open_with_intent(
            path,
            crate::edge::edge_table::config::MemoryIntent::HeapDefault,
        )
    }

    /// Open and validate a serving file under a declared memory intent.
    ///
    /// `ReadServing` explicitly requests transparent huge pages on Linux for
    /// the read-only scan-heavy mapping; `BulkLoad` and `HeapDefault` skip
    /// the hint so ingest and default heap paths stay on base pages. Every
    /// hint is best-effort: rejection falls back to base pages without
    /// failing the open.
    pub fn open_with_intent(
        path: &Path,
        intent: crate::edge::edge_table::config::MemoryIntent,
    ) -> StorageResult<Self> {
        use crate::edge::edge_table::config::MemoryIntent;
        let file = File::open(path)
            .map_err(|e| StorageError::io_error(format!("serving file open failed: {e}")))?;
        let map = unsafe { memmap2::Mmap::map(&file) }
            .map_err(|e| StorageError::io_error(format!("serving file map failed: {e}")))?;
        // Transparent huge pages only on the declared read-serving path.
        // The hint is best-effort: rejection falls back to base pages
        // without failing the open.
        #[cfg(target_os = "linux")]
        if intent == MemoryIntent::ReadServing {
            let _ = map.advise(memmap2::Advice::HugePage);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = intent;
        Self::from_map(Arc::new(map))
    }

    /// Open the serving file, rebuilding it from `frozen` when it is missing
    /// or fails validation.
    pub fn open_or_rebuild(path: &Path, frozen: &ImmutableCsr) -> StorageResult<Self> {
        match Self::open(path) {
            Ok(mapped) => Ok(mapped),
            Err(_) => {
                write_serving_file(frozen, path)?;
                Self::open(path)
            }
        }
    }

    fn from_map(map: Arc<memmap2::Mmap>) -> StorageResult<Self> {
        let (rows, entries, edge_count, columns) = parse_header(&map)?;
        let degree_bytes = map
            .get(columns.degrees.start..columns.degrees.end())
            .ok_or_else(|| {
                serving_error(format!(
                    "serving degree column out of range at byte {}",
                    columns.degrees.start
                ))
            })?;
        if degree_bytes.len() != rows.saturating_mul(4) {
            return Err(serving_error(format!(
                "serving degree column length mismatch: holds {} bytes, layout needs {}",
                degree_bytes.len(),
                rows.saturating_mul(4)
            )));
        }
        let mut offsets = Vec::with_capacity(rows);
        let mut base: u64 = 0;
        for chunk in degree_bytes.as_chunks::<4>().0 {
            let degree = u32::from_le_bytes(*chunk) as u64;
            if base > u32::MAX as u64 {
                return Err(serving_error(format!(
                    "serving row offset overflow at entry {base}"
                )));
            }
            offsets.push(base as u32);
            base = base.saturating_add(degree);
        }
        if base as usize != entries {
            return Err(serving_error(format!(
                "serving row window mismatch: degrees cover {base} entries, payload holds {entries}"
            )));
        }
        Ok(Self {
            map,
            columns,
            offsets: Arc::new(offsets),
            rows,
            entries,
            edge_count,
        })
    }

    /// Row count of the mapped table.
    pub fn vertex_capacity(&self) -> usize {
        self.rows
    }

    /// Live edge count stored at write time.
    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }
}
