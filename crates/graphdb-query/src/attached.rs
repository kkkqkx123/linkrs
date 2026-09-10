//! Attached-database catalog.
//!
//! Tracks external data sources registered through `ATTACH DATABASE` for the
//! lifetime of the process. Federation (cross-source query routing) is not
//! implemented yet; the catalog records the attachment so clients and
//! `SHOW ATTACHED DATABASES` can observe it.

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::RwLock;

/// A single attached data source.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachedDatabase {
    /// Alias used to reference the source.
    pub alias: String,
    /// Source path as written in the `ATTACH` statement.
    pub path: String,
    /// Declared source type (`None` when omitted).
    pub db_type: Option<String>,
}

impl AttachedDatabase {
    /// Create an attachment record, trimming surrounding whitespace.
    pub fn new(alias: String, path: String, db_type: Option<String>) -> Self {
        Self {
            alias,
            path,
            db_type,
        }
    }
}

fn registry() -> &'static RwLock<HashMap<String, AttachedDatabase>> {
    static REGISTRY: OnceLock<RwLock<HashMap<String, AttachedDatabase>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register a data source under `info.alias`.
///
/// Fails when the alias is already attached.
pub fn attach_database(info: AttachedDatabase) -> Result<(), String> {
    let mut guard = registry().write();
    if guard.contains_key(&info.alias) {
        return Err(format!("Database '{}' is already attached", info.alias));
    }
    guard.insert(info.alias.clone(), info);
    Ok(())
}

/// Remove the data source registered under `alias`, returning it.
///
/// Fails when no database is attached under `alias`.
pub fn detach_database(alias: &str) -> Result<AttachedDatabase, String> {
    registry()
        .write()
        .remove(alias)
        .ok_or_else(|| format!("No database attached as '{}'", alias))
}

/// List all attached data sources ordered by alias.
pub fn list_attached_databases() -> Vec<AttachedDatabase> {
    let mut entries: Vec<AttachedDatabase> = registry().read().values().cloned().collect();
    entries.sort_by(|a, b| a.alias.cmp(&b.alias));
    entries
}

/// Remove all attachments. Intended for tests.
pub fn clear_attached_databases() {
    registry().write().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_lists_and_detaches_in_alias_order() {
        clear_attached_databases();
        attach_database(AttachedDatabase::new(
            "b".to_string(),
            "/tmp/b".to_string(),
            None,
        ))
        .expect("first attach succeeds");
        attach_database(AttachedDatabase::new(
            "a".to_string(),
            "/tmp/a".to_string(),
            Some("KUZU".to_string()),
        ))
        .expect("second attach succeeds");
        assert!(attach_database(AttachedDatabase::new(
            "a".to_string(),
            "/tmp/other".to_string(),
            None,
        ))
        .is_err());

        let listed = list_attached_databases();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].alias, "a");
        assert_eq!(listed[0].db_type.as_deref(), Some("KUZU"));

        let removed = detach_database("a").expect("detach succeeds");
        assert_eq!(removed.path, "/tmp/a");
        assert!(detach_database("a").is_err());
        clear_attached_databases();
        assert!(list_attached_databases().is_empty());
    }
}
