//! File import for embedded sessions.
//!
//! Server-side `/import` accepts an uploaded CSV, JSON, or JSONL file and
//! streams the resulting statements through group-commit windows. This is
//! the embedded equivalent: the same shared parser produces the statements,
//! and the session loads them in-process through the same group-commit
//! path, returning imported/failed row counts.

use super::Session;
use crate::api_core::import::{
    parse_import, ImportFormat, ImportStats, ImportTarget, ParsedImport,
};
use crate::api_core::{CoreError, CoreResult};
use linkrs_storage::StorageClient;
use std::path::Path;

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Read a CSV/JSON/JSONL file and import its rows into `target_name`.
    ///
    /// The session's current graph space receives the rows. Rows that fail
    /// to parse or execute are counted in [`ImportStats::rows_failed`] and
    /// do not abort the import.
    pub fn import_file(
        &self,
        path: impl AsRef<Path>,
        format: ImportFormat,
        target: ImportTarget,
        target_name: &str,
        batch_size: usize,
    ) -> CoreResult<ImportStats> {
        let data = std::fs::read(path.as_ref())
            .map_err(|e| CoreError::Internal(format!("failed to read import file: {e}")))?;
        self.import_bytes(format, target, target_name, &data, batch_size)
    }

    /// Import rows from file bytes.
    ///
    /// See [`Self::import_file`]; this variant skips the filesystem read for
    /// callers that already hold the payload in memory.
    pub fn import_bytes(
        &self,
        format: ImportFormat,
        target: ImportTarget,
        target_name: &str,
        data: &[u8],
        batch_size: usize,
    ) -> CoreResult<ImportStats> {
        self.ensure_not_interrupted()?;
        self.ensure_writable_op()?;
        let parsed = parse_import(format, target, target_name, data)?;
        self.load_import_rows(parsed, batch_size)
    }

    fn load_import_rows(&self, parsed: ParsedImport, batch_size: usize) -> CoreResult<ImportStats> {
        let mut stats = ImportStats {
            rows_imported: 0,
            rows_failed: parsed.failures.len(),
        };
        if parsed.rows.is_empty() {
            return Ok(stats);
        }

        // Inside a text-begun transaction (or when auto-commit is off) the
        // rows run through the session execution path so they join the
        // caller's transaction instead of opening their own windows.
        if self.current_transaction.read().is_some() || !self.auto_commit {
            for row in &parsed.rows {
                match self.execute(&row.statement) {
                    Ok(_) => stats.rows_imported += 1,
                    Err(_) => stats.rows_failed += 1,
                }
            }
            return Ok(stats);
        }

        let group_size = batch_size.max(1);
        let storage = self.db.storage.clone();
        let mut query_api = self.db.query_api.write();
        for chunk in parsed.rows.chunks(group_size) {
            let window = storage
                .read()
                .begin_auto_commit_group()
                .map_err(|error| CoreError::StorageError(error.to_string()))?;
            for row in chunk {
                let statement_storage = match storage.read().bind_auto_commit_statement(&window) {
                    Ok(statement_storage) => statement_storage,
                    Err(_) => {
                        stats.rows_failed += 1;
                        continue;
                    }
                };
                let ctx = self.session_query_request().build();
                match query_api.execute_with_operation_storage(
                    &row.statement,
                    ctx,
                    statement_storage,
                ) {
                    Ok(_) => stats.rows_imported += 1,
                    Err(_) => stats.rows_failed += 1,
                }
            }
            storage
                .read()
                .finalize_auto_commit_group(&window)
                .map_err(|error| CoreError::StorageError(error.to_string()))?;
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_core::SpaceConfig;
    use crate::embedded::database::GraphDatabase;
    use linkrs_storage::GraphStorage;

    fn writable_session() -> (GraphDatabase<GraphStorage>, Session<GraphStorage>) {
        let db = GraphDatabase::open_in_memory().expect("test database opens");
        let mut session = db.session().expect("session creates");
        db.create_space("import_space", SpaceConfig::default())
            .expect("space creates");
        session.use_space("import_space").expect("space switches");
        (db, session)
    }

    #[test]
    fn import_bytes_reports_parse_failures_and_imports_valid_rows() {
        let (_db, session) = writable_session();
        session
            .execute("CREATE TAG Person(name string, city string)")
            .expect("tag creates");

        // The first tag property is its primary key and must mirror the VID.
        let csv = "vid,name,city\n1,1,Shanghai\n2,2,Beijing\n";
        let stats = session
            .import_bytes(
                ImportFormat::Csv,
                ImportTarget::Vertex,
                "Person",
                csv.as_bytes(),
                2,
            )
            .expect("import succeeds");
        assert_eq!(stats.rows_imported, 2);
        assert_eq!(stats.rows_failed, 0);
        assert_eq!(stats.total_rows(), 2);
    }

    #[test]
    fn import_file_reads_from_disk() {
        let (_db, session) = writable_session();
        session
            .execute("CREATE TAG Person(name string)")
            .expect("tag creates");

        let path = std::env::temp_dir().join("linkrs_embedded_import_test.csv");
        std::fs::write(&path, "vid,name\n100,100\n").expect("sample file writes");
        let stats = session
            .import_file(
                &path,
                ImportFormat::Csv,
                ImportTarget::Vertex,
                "Person",
                100,
            )
            .expect("import succeeds");
        assert_eq!(stats.rows_imported, 1);
        assert_eq!(stats.rows_failed, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_jsonl_rows_are_counted_not_fatal() {
        let (_db, session) = writable_session();
        session
            .execute("CREATE TAG Person(name string)")
            .expect("tag creates");

        let jsonl = "{\"_id\":\"v1\",\"name\":\"v1\"}\nnot json\n";
        let stats = session
            .import_bytes(
                ImportFormat::Jsonl,
                ImportTarget::Vertex,
                "Person",
                jsonl.as_bytes(),
                10,
            )
            .expect("import succeeds");
        assert_eq!(stats.rows_imported, 1);
        assert_eq!(stats.rows_failed, 1);
    }
}
