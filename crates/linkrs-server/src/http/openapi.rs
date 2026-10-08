//! OpenAPI document assembly and drift guards.
//!
//! This module collects every `#[utoipa::path]` annotation into a single
//! document. Referenced schemas are pulled in automatically by the derive,
//! so this module only lists paths and tags. Two tests pin the contract:
//! the serialized snapshot under `frontend/openapi.json` must match, and
//! every route registered in `router.rs` / `web.rs` / `web/handlers/*.rs`
//! must have a matching annotation.
//!
//! Document split: `CoreDoc` always applies and covers the unconditional
//! `/v1` handlers plus all `/api` web handlers. `VectorDoc` and
//! `FulltextDoc` are feature gated and merged on top when the corresponding
//! cargo feature is enabled.
//!
//! Standard commands (run from the workspace root):
//! ```text
//! GRAPHDB_REFRESH_OPENAPI=1 cargo test -p linkrs-server --features vector,fulltext openapi_snapshot_matches
//! cargo test -p linkrs-server --features vector,fulltext
//! ```

use std::sync::OnceLock;

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(title = "GraphDB API", version = "1.0.0"),
    paths(
        crate::http::handlers::health::check,
        crate::http::handlers::auth::login,
        crate::http::handlers::auth::logout,
        crate::http::handlers::auth::me,
        crate::http::handlers::users::list,
        crate::http::handlers::users::create,
        crate::http::handlers::users::drop_user,
        crate::http::handlers::users::reset_password,
        crate::http::handlers::users::enable,
        crate::http::handlers::users::disable,
        crate::http::handlers::users::grant,
        crate::http::handlers::users::revoke,
        crate::http::handlers::session::create,
        crate::http::handlers::session::list_sessions,
        crate::http::handlers::session::get_session,
        crate::http::handlers::session::delete_session,
        crate::http::handlers::query::execute,
        crate::http::handlers::query::validate,
        crate::http::handlers::query::explain,
        crate::http::handlers::query::execute_batch,
        crate::http::handlers::transaction::begin,
        crate::http::handlers::transaction::list_transactions,
        crate::http::handlers::transaction::metrics,
        crate::http::handlers::transaction::commit,
        crate::http::handlers::transaction::rollback,
        crate::http::handlers::transaction::kill_transaction,
        crate::http::handlers::transaction::retry_transaction_outbox,
        crate::http::handlers::transaction::create_savepoint,
        crate::http::handlers::transaction::get_savepoints,
        crate::http::handlers::transaction::rollback_to_savepoint,
        crate::http::handlers::transaction::release_savepoint,
        crate::http::handlers::batch::create,
        crate::http::handlers::batch::status,
        crate::http::handlers::batch::add_items,
        crate::http::handlers::batch::execute,
        crate::http::handlers::batch::cancel,
        crate::http::handlers::batch::delete,
        crate::http::handlers::import::import_file,
        crate::http::handlers::import::import_status,
        crate::http::handlers::export::export_data,
        crate::http::handlers::statistics::query::session,
        crate::http::handlers::statistics::query::queries,
        crate::http::handlers::statistics::query::query_profile_detail,
        crate::http::handlers::statistics::overview::overview,
        crate::http::handlers::statistics::database::database,
        crate::http::handlers::statistics::system::system,
        crate::http::handlers::statistics::query::search,
        crate::http::handlers::statistics::freeze::freeze_stats,
        crate::http::handlers::statistics::freeze::trigger_freeze,
        crate::http::handlers::statistics::migration::migration,
        crate::http::handlers::config::get,
        crate::http::handlers::config::update,
        crate::http::handlers::config::get_key,
        crate::http::handlers::config::update_key,
        crate::http::handlers::config::reset_key,
        crate::http::handlers::function::register,
        crate::http::handlers::function::list,
        crate::http::handlers::function::info,
        crate::http::handlers::function::unregister,
        crate::http::handlers::stream::execute_stream,
        crate::http::handlers::cursor::open_cursor,
        crate::http::handlers::cursor::fetch_cursor,
        crate::http::handlers::cursor::close_cursor,
        crate::http::handlers::sync::status,
        crate::http::handlers::sync::retry_outbox,
        crate::http::handlers::sync::diagnostics,
        crate::http::handlers::sync::dead_letters,
        crate::http::handlers::sync::requeue,
        crate::http::handlers::sync::degraded_ranges,
        crate::http::handlers::sync::degraded_clear,
        crate::http::handlers::sync::retention_run,
        crate::http::handlers::sync::retention_status,
        crate::http::handlers::schema::space::create_space,
        crate::http::handlers::schema::space::list_spaces,
        crate::http::handlers::schema::space::get_space,
        crate::http::handlers::schema::space::drop_space,
        crate::http::handlers::schema::tag::create_tag,
        crate::http::handlers::schema::tag::list_tags,
        crate::http::handlers::schema::edge_type::create_edge_type,
        crate::http::handlers::schema::edge_type::list_edge_types,
        crate::http::handlers::schema::version::get_version_history,
        crate::http::handlers::schema::version::get_schema_changes,
        crate::http::handlers::schema::version::detect_breaking_changes,
        crate::http::handlers::schema::migration::create_migration_plan,
        crate::http::handlers::schema::migration::execute_migration,
        crate::http::handlers::schema::migration::rollback_migration,
        crate::http::handlers::schema::migration::dry_run_migration,
        crate::http::handlers::schema::migration::migration_history,
        crate::http::handlers::schema::migration::migration_status,
        crate::http::handlers::migration_progress::migration_progress_stream,
        crate::web::handlers::metadata::add_history,
        crate::web::handlers::metadata::list_history,
        crate::web::handlers::metadata::delete_history,
        crate::web::handlers::metadata::clear_history,
        crate::web::handlers::metadata::add_favorite,
        crate::web::handlers::metadata::list_favorites,
        crate::web::handlers::metadata::get_favorite,
        crate::web::handlers::metadata::update_favorite,
        crate::web::handlers::metadata::delete_favorite,
        crate::web::handlers::metadata::clear_favorites,
        crate::web::handlers::schema_ext::space::list_spaces,
        crate::web::handlers::schema_ext::space::get_space_details,
        crate::web::handlers::schema_ext::space::get_space_statistics,
        crate::web::handlers::schema_ext::tag::list_tags,
        crate::web::handlers::schema_ext::tag::create_tag,
        crate::web::handlers::schema_ext::tag::get_tag,
        crate::web::handlers::schema_ext::tag::update_tag,
        crate::web::handlers::schema_ext::tag::delete_tag,
        crate::web::handlers::schema_ext::edge_type::list_edge_types,
        crate::web::handlers::schema_ext::edge_type::create_edge_type,
        crate::web::handlers::schema_ext::edge_type::get_edge_type,
        crate::web::handlers::schema_ext::edge_type::update_edge_type,
        crate::web::handlers::schema_ext::edge_type::delete_edge_type,
        crate::web::handlers::schema_ext::index::list_indexes,
        crate::web::handlers::schema_ext::index::create_index,
        crate::web::handlers::schema_ext::index::get_index,
        crate::web::handlers::schema_ext::index::delete_index,
        crate::web::handlers::schema_ext::index::rebuild_index,
        crate::web::handlers::data_browser::list_vertices_by_tag,
        crate::web::handlers::data_browser::list_edges_by_type,
        crate::web::handlers::graph_data::get_vertex,
        crate::web::handlers::graph_data::get_edge,
        crate::web::handlers::graph_data::get_neighbors,
    ),
    tags(
        (name = "WebHistory", description = "Web query history and favorites"),
        (name = "WebSchema", description = "Web extended schema management"),
        (name = "WebData", description = "Web data browsing"),
        (name = "WebGraph", description = "Web graph data queries"),
        (name = "Health", description = "Health checks"),
        (name = "Auth", description = "Authentication"),
        (name = "Session", description = "Session management"),
        (name = "Query", description = "Query execution"),
        (name = "Transaction", description = "Transaction management"),
        (name = "Batch", description = "Batch operations"),
        (name = "Import", description = "Data import"),
        (name = "Export", description = "Data export"),
        (name = "Statistics", description = "Statistics and monitoring"),
        (name = "Config", description = "Configuration management"),
        (name = "Function", description = "Custom functions"),
        (name = "Stream", description = "Streaming queries"),
        (name = "Sync", description = "Synchronization management"),
        (name = "Schema", description = "Schema management"),
        (name = "Migration", description = "Schema migration"),
    )
)]
struct CoreDoc;

#[cfg(feature = "vector")]
#[derive(OpenApi)]
#[openapi(
    info(title = "GraphDB API", version = "1.0.0"),
    paths(
        crate::http::handlers::vector::create_index,
        crate::http::handlers::vector::list_indexes,
        crate::http::handlers::vector::get_index_info,
        crate::http::handlers::vector::drop_index,
        crate::http::handlers::vector::search,
        crate::http::handlers::vector::get_vector,
        crate::http::handlers::vector::count,
        crate::http::handlers::vector::set_payload,
        crate::http::handlers::vector::set_payload_fields,
        crate::http::handlers::vector::delete_payload,
        crate::http::handlers::vector::scroll,
        crate::http::handlers::rebuild::rebuild_vector,
        crate::http::handlers::rebuild::vector_rebuild_status,
        crate::http::handlers::rebuild::clear_vector,
    ),
    components(schemas(
        crate::core::vector::VectorFilter,
        crate::core::vector::MinShouldCondition,
        crate::core::vector::FilterCondition,
        crate::core::vector::ConditionType,
        crate::core::vector::RangeCondition,
        crate::core::vector::GeoRadius,
        crate::core::vector::GeoBoundingBox,
        crate::core::vector::ValuesCountCondition,
    )),
    tags((name = "Vector", description = "Vector search and index management"))
)]
struct VectorDoc;

#[cfg(feature = "fulltext")]
#[derive(OpenApi)]
#[openapi(
    info(title = "GraphDB API", version = "1.0.0"),
    paths(
        crate::http::handlers::rebuild::rebuild_fulltext,
        crate::http::handlers::rebuild::fulltext_rebuild_status,
        crate::http::handlers::rebuild::clear_fulltext,
        crate::http::handlers::rebuild::inconsistent_fulltext,
    ),
    tags((name = "Fulltext", description = "Fulltext index management"))
)]
struct FulltextDoc;

/// Serialize the OpenAPI document, cached after the first call.
pub fn openapi_json() -> String {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            #[allow(unused_mut)]
            let mut doc = CoreDoc::openapi();
            #[cfg(feature = "vector")]
            {
                doc = doc.merge_from(VectorDoc::openapi());
            }
            #[cfg(feature = "fulltext")]
            {
                doc = doc.merge_from(FulltextDoc::openapi());
            }
            serde_json::to_string_pretty(&doc).expect("OpenAPI document must serialize")
        })
        .clone()
}

#[cfg(all(feature = "vector", feature = "fulltext"))]
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    // ---- contract configuration (repo-specific) ----

    const SNAPSHOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../frontend/openapi.json");
    const REFRESH_ENV: &str = "GRAPHDB_REFRESH_OPENAPI";

    fn source_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// Mount prefix for `.route()` registrations in the file at `rel`
    /// (relative to the crate `src` root); `None` skips the file for route
    /// collection. Axum routes are relative: `/v1` handlers live in
    /// `http/router.rs`, web handlers in `web/handlers/*.rs` nest under
    /// `/api` with one prefix per module (see `web.rs`).
    fn route_prefix(rel: &str) -> Option<String> {
        match rel {
            "http/router.rs" => Some("/v1".to_string()),
            "web/handlers/metadata.rs" => Some("/api/v1/queries".to_string()),
            "web/handlers/schema_ext/routes.rs" => Some("/api/v1/schema".to_string()),
            "web/handlers/data_browser.rs" => Some("/api/v1/data".to_string()),
            "web/handlers/graph_data.rs" => Some("/api/v1/graph".to_string()),
            _ => None,
        }
    }

    /// Debug-only documentation endpoint, not part of the contract.
    fn is_ignored_path(path: &str) -> bool {
        path.contains("/api-docs")
    }

    fn feature_enabled(name: &str) -> bool {
        match name {
            "vector" => cfg!(feature = "vector"),
            "fulltext" => cfg!(feature = "fulltext"),
            _ => panic!("unexpected feature gate in scanned route sources: {name}"),
        }
    }

    #[test]
    fn openapi_snapshot_matches() {
        let doc = openapi_json();
        if std::env::var(REFRESH_ENV).as_deref() == Ok("1") {
            std::fs::write(SNAPSHOT, format!("{doc}\n")).expect("snapshot must be writable");
            return;
        }
        // The canonical snapshot is generated with all features enabled.
        // Standard command: cargo test -p linkrs-server --features vector,fulltext
        let expected = std::fs::read_to_string(SNAPSHOT).expect("openapi snapshot must exist");
        assert_eq!(
            doc.trim_end(),
            expected.trim_end(),
            "snapshot drifted; refresh with {REFRESH_ENV}=1"
        );
    }

    #[test]
    fn routes_match_openapi_paths() {
        let routed = collect_routed();
        let annotated = collect_annotated();
        let documented = collect_documented();

        assert_sets_equal(
            &routed,
            &annotated,
            "router registrations vs #[utoipa::path] annotations",
        );
        assert_sets_equal(
            &annotated,
            &documented,
            "#[utoipa::path] annotations vs ApiDoc registration",
        );
        assert!(!routed.is_empty(), "expected a non-empty route set");
    }

    /// Each operation needs a document-unique `operationId`: `openapi-typescript`
    /// keys its `operations` map by it, so collisions silently overwrite an
    /// operation and mis-type call sites that use `openapi-fetch`.
    #[test]
    fn operation_ids_are_unique() {
        use std::collections::HashSet;
        let doc: serde_json::Value =
            serde_json::from_str(&openapi_json()).expect("document must parse");
        let mut seen = HashSet::new();
        for (_path, item) in doc["paths"]
            .as_object()
            .expect("document must contain paths")
        {
            if let Some(ops) = item.as_object() {
                for (_method, op) in ops {
                    if let Some(id) = op.get("operationId").and_then(|id| id.as_str()) {
                        assert!(seen.insert(id.to_string()), "duplicate operationId: {id}");
                    }
                }
            }
        }
    }

    // ---- unified guard engine (shared across repos; keep verbatim) ----

    const HTTP_METHODS: [&str; 8] = [
        "get", "put", "post", "patch", "delete", "head", "options", "trace",
    ];

    fn assert_sets_equal(
        left: &BTreeSet<(String, String)>,
        right: &BTreeSet<(String, String)>,
        label: &str,
    ) {
        let only_left: Vec<_> = left.difference(right).collect();
        let only_right: Vec<_> = right.difference(left).collect();
        assert!(
            only_left.is_empty() && only_right.is_empty(),
            "{label} drift\nonly on the left: {only_left:?}\nonly on the right: {only_right:?}"
        );
    }

    fn walk_rs(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).expect("read source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                out.extend(walk_rs(&path));
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
        out
    }

    fn source_files() -> Vec<(String, String)> {
        let root = source_root();
        walk_rs(&root)
            .into_iter()
            .map(|path| {
                let rel = path
                    .strip_prefix(&root)
                    .expect("source under src root")
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = std::fs::read_to_string(&path).expect("read source file");
                (rel, text)
            })
            .collect()
    }

    fn collect_routed() -> BTreeSet<(String, String)> {
        let mut out = BTreeSet::new();
        for (rel, text) in source_files() {
            let Some(prefix) = route_prefix(&rel) else {
                continue;
            };
            for (method, path) in parse_route_registrations(&text) {
                let full = format!("{prefix}{path}");
                if !is_ignored_path(&full) {
                    out.insert((method, full));
                }
            }
        }
        out
    }

    fn collect_annotated() -> BTreeSet<(String, String)> {
        let mut out = BTreeSet::new();
        for (_rel, text) in source_files() {
            for (method, path) in parse_utoipa_annotations(&text) {
                out.insert((method, path));
            }
        }
        out
    }

    fn collect_documented() -> BTreeSet<(String, String)> {
        let doc: serde_json::Value =
            serde_json::from_str(&openapi_json()).expect("document must parse");
        let mut out = BTreeSet::new();
        if let Some(paths) = doc.get("paths").and_then(|p| p.as_object()) {
            for (path, item) in paths {
                if let Some(ops) = item.as_object() {
                    for method in ops.keys() {
                        out.insert((method.to_uppercase(), path.clone()));
                    }
                }
            }
        }
        out
    }

    /// Parse `#[utoipa::path(...)]` attributes into (method, full path).
    fn parse_utoipa_annotations(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut lines = text.lines().peekable();
        let mut buf: Vec<String> = Vec::new();
        while let Some(line) = lines.next() {
            let trimmed = line.trim();
            if let Some(tail) = trimmed.strip_prefix("#[utoipa::path(") {
                buf.clear();
                if tail != "(" {
                    buf.push(tail.to_string());
                }
                for line in lines.by_ref().take(60) {
                    let trimmed = line.trim();
                    if let Some(closer) = trimmed.strip_prefix(")]") {
                        buf.push(trimmed.to_string());
                        let _ = closer;
                        break;
                    }
                    buf.push(trimmed.to_string());
                }
                if let Some((method, path)) = annotation_head(&buf) {
                    out.push((method, path));
                }
            }
        }
        out
    }

    fn annotation_head(buf: &[String]) -> Option<(String, String)> {
        let method = buf.iter().find_map(|line| {
            HTTP_METHODS
                .iter()
                .find(|m| line.starts_with(&format!("{m},")))
                .map(|m| m.to_uppercase())
        })?;
        let path = buf
            .iter()
            .find_map(|line| {
                line.find("path = \"")
                    .map(|i| &line[i + "path = \"".len()..])
            })
            .and_then(|rest| rest.split('"').next())
            .map(str::to_string)?;
        Some((method, path))
    }

    /// Parse axum `.route("<path>", get(..).post(..))` registrations into
    /// (method, path) pairs. Line-based: rustfmt keeps these tables stable.
    /// Top-level `#[cfg(...)]` attributes gate whole functions so
    /// feature-disabled route bodies are ignored.
    fn parse_route_registrations(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut pending_attr: Option<String> = None;
        let mut fn_active = true;
        let mut entry: Option<(Option<String>, Vec<String>)> = None;

        let flush = |out: &mut Vec<(String, String)>,
                     entry: &mut Option<(Option<String>, Vec<String>)>| {
            if let Some((Some(path), methods)) = entry.take() {
                for method in methods {
                    out.push((method, path.clone()));
                }
            } else {
                entry.take();
            }
        };

        for line in text.lines() {
            let trimmed = line.trim();
            let top_level = !trimmed.is_empty() && !line.starts_with(char::is_whitespace);
            if top_level {
                if let Some(expr) = trimmed
                    .strip_prefix("#[cfg(")
                    .and_then(|rest| rest.strip_suffix(")]"))
                {
                    pending_attr = Some(expr.to_string());
                    continue;
                }
                if trimmed.starts_with("#[") || trimmed.starts_with("//") {
                    continue;
                }
                let is_fn = trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("pub(crate) fn ")
                    || trimmed.starts_with("async fn ")
                    || trimmed.starts_with("pub async fn ")
                    || trimmed.starts_with("pub(crate) async fn ");
                if is_fn {
                    fn_active = pending_attr.take().map_or(true, |expr| eval_cfg(&expr));
                    entry = None;
                    continue;
                }
                pending_attr = None;
            }
            if !fn_active {
                continue;
            }

            if let Some(pos) = line.find(".route(") {
                flush(&mut out, &mut entry);
                let tail = &line[pos + ".route(".len()..];
                entry = Some((first_string_literal(tail), method_tokens(tail)));
            } else if let Some((path, methods)) = entry.as_mut() {
                if path.is_none() {
                    *path = first_string_literal(trimmed);
                }
                methods.extend(method_tokens(trimmed));
            }

            let ends_entry = trimmed.starts_with(".nest(")
                || trimmed.starts_with(".layer(")
                || trimmed.starts_with(".route_layer(")
                || trimmed.starts_with(".merge(")
                || trimmed.starts_with(".fallback(")
                || trimmed.starts_with(".with_state(")
                || trimmed == ")"
                || trimmed == "),"
                || trimmed.ends_with(");");
            if ends_entry {
                flush(&mut out, &mut entry);
            }
        }
        flush(&mut out, &mut entry);
        out
    }

    fn first_string_literal(s: &str) -> Option<String> {
        let start = s.find('"')?;
        let rest = &s[start + 1..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    }

    fn method_tokens(line: &str) -> Vec<String> {
        let cleaned = strip_string_literals(line);
        let b = cleaned.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if !(b[i].is_ascii_alphabetic() || b[i] == b'_') {
                i += 1;
                continue;
            }
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &cleaned[start..i];
            let mut j = i;
            while j < b.len() && (b[j] as char).is_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' && HTTP_METHODS.contains(&word) {
                out.push(word.to_uppercase());
            }
        }
        out
    }

    fn strip_string_literals(line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut in_string = false;
        for c in line.chars() {
            if c == '"' {
                in_string = !in_string;
                out.push(' ');
            } else if in_string {
                out.push(' ');
            } else {
                out.push(c);
            }
        }
        out
    }

    fn eval_cfg(expr: &str) -> bool {
        let expr = expr.trim();
        if let Some(inner) = expr.strip_prefix("not(").and_then(|s| s.strip_suffix(')')) {
            return !eval_cfg(inner);
        }
        if let Some(inner) = expr.strip_prefix("all(").and_then(|s| s.strip_suffix(')')) {
            return split_cfg_items(inner).iter().all(|item| eval_cfg(item));
        }
        if let Some(inner) = expr.strip_prefix("any(").and_then(|s| s.strip_suffix(')')) {
            return split_cfg_items(inner).iter().any(|item| eval_cfg(item));
        }
        match expr {
            "debug_assertions" => cfg!(debug_assertions),
            "test" => false,
            other => {
                if let Some(name) = other
                    .strip_prefix("feature")
                    .and_then(|s| s.trim().strip_prefix('='))
                    .and_then(|s| s.trim().split('"').nth(1))
                {
                    feature_enabled(name)
                } else {
                    panic!("unsupported cfg predicate: {expr}")
                }
            }
        }
    }

    fn split_cfg_items(expr: &str) -> Vec<&str> {
        let mut depth = 0usize;
        let mut items = Vec::new();
        let mut start = 0;
        for (i, c) in expr.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    items.push(expr[start..i].trim());
                    start = i + 1;
                }
                _ => {}
            }
        }
        let last = expr[start..].trim();
        if !last.is_empty() {
            items.push(last);
        }
        items
    }
}
