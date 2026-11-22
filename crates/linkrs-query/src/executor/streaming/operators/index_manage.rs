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

/// The `SHOW <family> INDEX [LIKE pattern] [FROM schema]` predicate.
///
/// Three independent dimensions, each carried explicitly:
///
/// - `space_id` scopes the listing to the space the statement runs in. Index
///   names are only unique inside a space, so a listing that ignores the
///   space mixes catalogs that never see each other.
/// - `schema` is the `FROM <schema>` filter and matches the **tag** name —
///   the space name is not a schema and must not be substituted for it.
/// - `pattern` matches as a substring of the statement-level index name; an
///   index with no statement-level name never matches a pattern.
pub(crate) struct ShowFilter<'a> {
    pattern: Option<&'a str>,
    schema: Option<&'a str>,
    space_id: u64,
}

impl<'a> ShowFilter<'a> {
    pub(crate) fn new(pattern: Option<&'a str>, schema: Option<&'a str>, space_id: u64) -> Self {
        Self {
            pattern,
            schema,
            space_id,
        }
    }

    /// Whether one catalog entry belongs in the listing.
    pub(crate) fn matches(&self, index_name: Option<&str>, tag_name: &str, space_id: u64) -> bool {
        if space_id != self.space_id {
            return false;
        }
        let pattern_ok = match self.pattern {
            Some(pattern) => index_name.is_some_and(|name| name.contains(pattern)),
            None => true,
        };
        let schema_ok = self.schema.is_none_or(|schema| tag_name == schema);
        pattern_ok && schema_ok
    }
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
        let filter = ShowFilter::new(Some("idx"), None, 1);
        assert!(filter.matches(Some("idx_a"), "t", 1));
        assert!(!filter.matches(None, "t", 1));
        assert!(ShowFilter::new(None, None, 1).matches(None, "t", 1));
    }

    #[test]
    fn show_filter_matches_schema_exactly() {
        let filter = ShowFilter::new(None, Some("t"), 1);
        assert!(filter.matches(None, "t", 1));
        assert!(!filter.matches(None, "t2", 1));
    }

    #[test]
    fn show_filter_is_scoped_to_one_space() {
        let filter = ShowFilter::new(None, None, 7);
        assert!(filter.matches(None, "t", 7));
        assert!(
            !filter.matches(None, "t", 8),
            "an index from another space must not be listed"
        );
        assert!(
            !ShowFilter::new(None, Some("t"), 7).matches(None, "t", 8),
            "the space filter must win over a matching tag name"
        );
    }

    #[test]
    fn unresolved_drop_is_a_noop_only_with_if_exists() {
        assert!(unresolved_drop(IndexKind::Vector, "i", true).is_none());
        let error = unresolved_drop(IndexKind::Vector, "i", false)
            .expect("without IF EXISTS the drop must fail");
        assert!(error.to_string().contains("vector index 'i'"));
    }
}
