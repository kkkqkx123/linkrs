//! Shared execution helpers for secondary-index management statements.
//!
//! `CREATE|DROP|SHOW|DESCRIBE <family> INDEX` differ only in which catalog
//! they read and in their `SHOW`/`DESCRIBE` column sets. Everything else —
//! the status row shape, the `IF EXISTS` outcome, and the `SHOW` filter
//! predicate — is identical, and lives here so the two families cannot drift.

use std::sync::Arc;

use linkrs_core::error::QueryError;
use linkrs_core::Value;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::slot::SlotLayout;

/// Which secondary-index family a management statement addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexKind {
    Fulltext,
    Vector,
}

impl IndexKind {
    /// Lowercase family name used in statement labels.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            IndexKind::Fulltext => "fulltext",
            IndexKind::Vector => "vector",
        }
    }
}

/// Columns of a `(action, name, status)` row emitted by state-changing
/// statements (`CREATE` / `DROP` / `ALTER`).
pub(crate) const STATUS_COLUMNS: &[&str] = &["action", "name", "status"];

/// Columns of the catalog listing emitted by `SHOW <family> INDEX`.
///
/// The two families expose different card fields, so the listing is per
/// family; the operators build their chunks from these same names, which
/// keeps the plan layout and the runtime chunk in agreement.
pub(crate) const fn show_columns(kind: IndexKind) -> &'static [&'static str] {
    match kind {
        IndexKind::Fulltext => &["index_id", "space_id", "tag_name", "field_name", "status"],
        IndexKind::Vector => &[
            "index_id",
            "collection_name",
            "space_id",
            "tag_name",
            "field_name",
        ],
    }
}

/// Columns of the single index card emitted by `DESCRIBE <family> INDEX`.
pub(crate) const fn describe_columns(kind: IndexKind) -> &'static [&'static str] {
    match kind {
        IndexKind::Fulltext => &["index_id", "tag_name", "field_name", "status"],
        IndexKind::Vector => &["index_id", "tag_name", "field_name", "vector_count"],
    }
}

/// Which shape a management statement produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManageShape {
    /// `(action, name, status)` — `CREATE` / `DROP` / `ALTER`.
    Status,
    /// Catalog listing — `SHOW`.
    Show,
    /// Single index card — `DESCRIBE`.
    Describe,
}

impl ManageShape {
    pub(crate) const fn columns(self, kind: IndexKind) -> &'static [&'static str] {
        match self {
            ManageShape::Status => STATUS_COLUMNS,
            ManageShape::Show => show_columns(kind),
            ManageShape::Describe => describe_columns(kind),
        }
    }
}

/// Slot layout of a management statement's output.
///
/// The plan layout and the chunk an operator emits must agree column for
/// column; deriving both from [`ManageShape::columns`] is what keeps `SHOW`
/// and `DESCRIBE` from advertising the `(action, name, status)` shape.
pub(crate) fn layout_of(kind: IndexKind, shape: ManageShape) -> SlotLayout {
    let names: Vec<String> = shape
        .columns(kind)
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    SlotLayout::from_names(&names)
}

/// One-row `(action, name, status)` result emitted by management statements.
pub(crate) fn manage_result(
    output_layout: Arc<SlotLayout>,
    action: &str,
    name: Option<&str>,
    status: &str,
) -> DataChunk {
    let name_val = name
        .map(Value::string)
        .unwrap_or(Value::Null(linkrs_core::NullType::Null));
    DataChunk::new_with_layout(
        vec![vec![Value::string(action), name_val, Value::string(status)]],
        output_layout,
    )
}

/// Whether an index passes the `SHOW <family> INDEX [LIKE pattern] [FROM schema]`
/// filter.
///
/// `pattern` matches as a substring of the statement-level index name; an
/// index with no statement-level name never matches a pattern.
pub(crate) fn matches_show_filter(
    pattern: Option<&str>,
    from_schema: Option<&str>,
    index_name: Option<&str>,
    tag_name: &str,
) -> bool {
    let pattern_ok = match pattern {
        Some(pattern) => index_name.is_some_and(|name| name.contains(pattern)),
        None => true,
    };
    let schema_ok = from_schema.is_none_or(|schema| tag_name == schema);
    pattern_ok && schema_ok
}

/// Apply `IF EXISTS` semantics to a drop target whose location planning could
/// not resolve.
///
/// `None` means the statement degrades to a `not_exists` status row; `Some`
/// carries the error to return.
pub(crate) fn unresolved_drop(
    kind: IndexKind,
    index_name: &str,
    if_exists: bool,
) -> Option<QueryError> {
    if if_exists {
        return None;
    }
    Some(QueryError::execution(format!(
        "{} index '{index_name}' cannot be dropped: index location (tag/field) is not resolved",
        kind.label()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_filter_requires_a_named_index_when_pattern_given() {
        assert!(matches_show_filter(Some("idx"), None, Some("idx_a"), "t"));
        assert!(!matches_show_filter(Some("idx"), None, None, "t"));
        assert!(matches_show_filter(None, None, None, "t"));
    }

    #[test]
    fn show_filter_matches_schema_exactly() {
        assert!(matches_show_filter(None, Some("t"), None, "t"));
        assert!(!matches_show_filter(None, Some("t"), None, "t2"));
    }

    #[test]
    fn unresolved_drop_is_a_noop_only_with_if_exists() {
        assert!(unresolved_drop(IndexKind::Vector, "i", true).is_none());
        let error = unresolved_drop(IndexKind::Vector, "i", false)
            .expect("without IF EXISTS the drop must fail");
        assert!(error.to_string().contains("vector index 'i'"));
    }
}
