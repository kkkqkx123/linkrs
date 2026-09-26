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
//! GRAPHDB_REFRESH_OPENAPI=1 cargo test -p graphdb-server --features vector,fulltext openapi_snapshot_matches
//! cargo test -p graphdb-server --features vector,fulltext
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
        crate::http::handlers::session::create,
        crate::http::handlers::session::get_session,
        crate::http::handlers::session::delete_session,
        crate::http::handlers::query::execute,
        crate::http::handlers::query::validate,
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
        crate::http::handlers::statistics::session,
        crate::http::handlers::statistics::queries,
        crate::http::handlers::statistics::database,
        crate::http::handlers::statistics::system,
        crate::http::handlers::statistics::search,
        crate::http::handlers::statistics::freeze_stats,
        crate::http::handlers::statistics::trigger_freeze,
        crate::http::handlers::statistics::migration,
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
        crate::http::handlers::sync::status,
        crate::http::handlers::sync::retry_outbox,
        crate::http::handlers::sync::diagnostics,
        crate::http::handlers::sync::dead_letters,
        crate::http::handlers::sync::requeue,
        crate::http::handlers::sync::degraded_ranges,
        crate::http::handlers::sync::degraded_clear,
        crate::http::handlers::sync::retention_run,
        crate::http::handlers::sync::retention_status,
        crate::http::handlers::schema::create_space,
        crate::http::handlers::schema::list_spaces,
        crate::http::handlers::schema::get_space,
        crate::http::handlers::schema::drop_space,
        crate::http::handlers::schema::create_tag,
        crate::http::handlers::schema::list_tags,
        crate::http::handlers::schema::create_edge_type,
        crate::http::handlers::schema::list_edge_types,
        crate::http::handlers::schema::get_version_history,
        crate::http::handlers::schema::get_schema_changes,
        crate::http::handlers::schema::detect_breaking_changes,
        crate::http::handlers::schema::create_migration_plan,
        crate::http::handlers::schema::execute_migration,
        crate::http::handlers::schema::rollback_migration,
        crate::http::handlers::schema::dry_run_migration,
        crate::http::handlers::schema::migration_history,
        crate::http::handlers::schema::migration_status,
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
        crate::web::handlers::schema_ext::list_spaces,
        crate::web::handlers::schema_ext::get_space_details,
        crate::web::handlers::schema_ext::get_space_statistics,
        crate::web::handlers::schema_ext::list_tags,
        crate::web::handlers::schema_ext::create_tag,
        crate::web::handlers::schema_ext::get_tag,
        crate::web::handlers::schema_ext::update_tag,
        crate::web::handlers::schema_ext::delete_tag,
        crate::web::handlers::schema_ext::list_edge_types,
        crate::web::handlers::schema_ext::create_edge_type,
        crate::web::handlers::schema_ext::get_edge_type,
        crate::web::handlers::schema_ext::update_edge_type,
        crate::web::handlers::schema_ext::delete_edge_type,
        crate::web::handlers::schema_ext::list_indexes,
        crate::web::handlers::schema_ext::create_index,
        crate::web::handlers::schema_ext::get_index,
        crate::web::handlers::schema_ext::delete_index,
        crate::web::handlers::schema_ext::rebuild_index,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const SNAPSHOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../frontend/openapi.json");

    const ROUTER_SRC: &str = include_str!("router.rs");
    const WEB_SRC: &str = include_str!("../web.rs");
    const WEB_METADATA_SRC: &str = include_str!("../web/handlers/metadata.rs");
    const WEB_SCHEMA_EXT_SRC: &str = include_str!("../web/handlers/schema_ext.rs");
    const WEB_DATA_BROWSER_SRC: &str = include_str!("../web/handlers/data_browser.rs");
    const WEB_GRAPH_DATA_SRC: &str = include_str!("../web/handlers/graph_data.rs");

    #[test]
    fn openapi_snapshot_matches() {
        let doc = openapi_json();
        if std::env::var("GRAPHDB_REFRESH_OPENAPI").as_deref() == Ok("1") {
            std::fs::write(SNAPSHOT, &doc).expect("snapshot must be writable");
            return;
        }
        // The canonical snapshot is generated with all features enabled.
        // Standard command: cargo test -p graphdb-server --features vector,fulltext
        if cfg!(feature = "vector") && cfg!(feature = "fulltext") {
            let expected = std::fs::read_to_string(SNAPSHOT).expect("openapi snapshot must exist");
            assert_eq!(doc.trim_end(), expected.trim_end());
        } else {
            let value: serde_json::Value = serde_json::from_str(&doc).expect("document must parse");
            let paths = value
                .get("paths")
                .and_then(|p| p.as_object())
                .expect("document must contain paths");
            assert!(!paths.is_empty(), "document must contain paths");
        }
    }

    #[test]
    fn routes_match_openapi_paths() {
        let mut expected: BTreeSet<(String, String)> = BTreeSet::new();

        // Unconditional /v1 routes: whole router minus the feature-gated
        // helper bodies.
        let core_src = remove_fn_bodies(
            &strip_line_comments(ROUTER_SRC),
            &["add_vector_routes", "add_fulltext_routes"],
        );
        for (path, methods) in extract_route_calls(&core_src) {
            // Debug-only document endpoint is not part of the contract.
            if path.contains("api-docs") {
                continue;
            }
            for method in methods {
                expected.insert((method, format!("/v1{path}")));
            }
        }

        // Feature-gated /v1 routes only apply when the feature is enabled.
        // The disabled stubs hold no routes, so every body found counts.
        if cfg!(feature = "vector") {
            for body in fn_bodies(&strip_line_comments(ROUTER_SRC), "add_vector_routes") {
                for (path, methods) in extract_route_calls(&body) {
                    for method in methods {
                        expected.insert((method, format!("/v1{path}")));
                    }
                }
            }
        }
        if cfg!(feature = "fulltext") {
            for body in fn_bodies(&strip_line_comments(ROUTER_SRC), "add_fulltext_routes") {
                for (path, methods) in extract_route_calls(&body) {
                    for method in methods {
                        expected.insert((method, format!("/v1{path}")));
                    }
                }
            }
        }

        // Web routes: nest prefixes come from web.rs, handlers from
        // web/handlers/*.rs, everything mounted under /api.
        for (module, prefix) in parse_web_nests(&strip_line_comments(WEB_SRC)) {
            let src = match module.as_str() {
                "metadata" => WEB_METADATA_SRC,
                "schema_ext" => WEB_SCHEMA_EXT_SRC,
                "data_browser" => WEB_DATA_BROWSER_SRC,
                "graph_data" => WEB_GRAPH_DATA_SRC,
                other => panic!("unknown web handler module: {other}"),
            };
            for (path, methods) in extract_route_calls(&strip_line_comments(src)) {
                for method in methods {
                    expected.insert((method, format!("/api{prefix}{path}")));
                }
            }
        }

        let doc: serde_json::Value =
            serde_json::from_str(&openapi_json()).expect("document must parse");
        let mut documented: BTreeSet<(String, String)> = BTreeSet::new();
        if let Some(paths) = doc.get("paths").and_then(|p| p.as_object()) {
            for (path, item) in paths {
                if let Some(ops) = item.as_object() {
                    for method in ops.keys() {
                        documented.insert((method.to_uppercase(), path.clone()));
                    }
                }
            }
        }

        let routed_only: Vec<_> = expected.difference(&documented).collect();
        let documented_only: Vec<_> = documented.difference(&expected).collect();
        assert!(
            routed_only.is_empty() && documented_only.is_empty(),
            "router/document drift: routed-but-undocumented={routed_only:?} documented-but-unrouted={documented_only:?}"
        );
    }

    /// Drop `//` line comments; none of the scanned sources hold `//`
    /// inside string literals.
    fn strip_line_comments(src: &str) -> String {
        src.lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Parse `.route("path", method(...))` registrations into path plus
    /// HTTP methods. Handles multi-line calls and chained methods.
    /// Byte based; all scanned constructs are ASCII.
    fn extract_route_calls(src: &str) -> Vec<(String, Vec<String>)> {
        const METHODS: [&str; 8] = [
            "get", "post", "put", "delete", "patch", "head", "options", "trace",
        ];
        let b = src.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i..].starts_with(b".route(") {
                let mut j = i + ".route(".len();
                while j < b.len() && (b[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < b.len() && b[j] == b'"' {
                    j += 1;
                    let mut path = Vec::new();
                    while j < b.len() && b[j] != b'"' {
                        if b[j] == b'\\' && j + 1 < b.len() {
                            path.push(b[j + 1]);
                            j += 2;
                        } else {
                            path.push(b[j]);
                            j += 1;
                        }
                    }
                    j += 1; // Consume closing quote.
                    let path = String::from_utf8(path).expect("route path must be UTF-8");
                    // Scan call arguments up to the matching paren,
                    // collecting method wrapper idents.
                    let mut depth = 1usize;
                    let mut methods = Vec::new();
                    while j < b.len() && depth > 0 {
                        let c = b[j];
                        if c == b'"' {
                            j = skip_string(b, j).expect("string must terminate");
                            continue;
                        }
                        if c == b'(' {
                            depth += 1;
                            j += 1;
                            continue;
                        }
                        if c == b')' {
                            depth -= 1;
                            j += 1;
                            continue;
                        }
                        if c.is_ascii_alphabetic() || c == b'_' {
                            let start = j;
                            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                                j += 1;
                            }
                            let ident = &src[start..j];
                            let mut k = j;
                            while k < b.len() && (b[k] as char).is_whitespace() {
                                k += 1;
                            }
                            if k < b.len() && b[k] == b'(' && METHODS.contains(&ident) {
                                methods.push(ident.to_uppercase());
                            }
                            continue;
                        }
                        j += 1;
                    }
                    out.push((path, methods));
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// Index after the closing quote of the string starting at `open`.
    fn skip_string(b: &[u8], open: usize) -> Option<usize> {
        let mut j = open + 1;
        while j < b.len() && b[j] != b'"' {
            if b[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        if j < b.len() {
            Some(j + 1)
        } else {
            None
        }
    }

    /// Best-effort skip of a char literal or lifetime quote.
    fn skip_quote(b: &[u8], at: usize) -> usize {
        if at + 2 < b.len() && b[at + 2] == b'\'' {
            at + 3
        } else if at + 3 < b.len() && b[at + 1] == b'\\' && b[at + 3] == b'\'' {
            at + 4
        } else {
            at + 1
        }
    }

    /// Locate the `{ ... }` block of the first function body at or after
    /// `from`. Returns byte indices (open, end-exclusive).
    fn block_span(b: &[u8], from: usize) -> Option<(usize, usize)> {
        let mut i = from;
        // Find the opening brace of the body.
        loop {
            if i >= b.len() {
                return None;
            }
            match b[i] {
                b'"' => i = skip_string(b, i)?,
                b'\'' => i = skip_quote(b, i),
                b'{' => break,
                _ => i += 1,
            }
        }
        let open = i;
        let mut depth = 0usize;
        while i < b.len() {
            match b[i] {
                b'"' => {
                    i = skip_string(b, i)?;
                    continue;
                }
                b'\'' => {
                    i = skip_quote(b, i);
                    continue;
                }
                b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                    i += 2;
                    while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                        i += 1;
                    }
                    i += 2;
                    continue;
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((open, i + 1));
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Byte spans of every `fn name ... { ... }` definition.
    fn fn_spans(src: &str, name: &str) -> Vec<(usize, usize)> {
        let b = src.as_bytes();
        let needle = format!("fn {name}");
        let nb = needle.as_bytes();
        let mut spans = Vec::new();
        let mut s = 0;
        while s + nb.len() <= b.len() {
            if &b[s..s + nb.len()] == nb {
                let before_ok = s == 0 || !(b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_');
                let after = s + nb.len();
                let after_ok =
                    after >= b.len() || !(b[after].is_ascii_alphanumeric() || b[after] == b'_');
                if before_ok && after_ok {
                    if let Some((_, end)) = block_span(b, s) {
                        spans.push((s, end));
                        s = end;
                        continue;
                    }
                    break;
                }
            }
            s += 1;
        }
        spans
    }

    /// Bodies (including braces) of every `fn name` definition.
    fn fn_bodies(src: &str, name: &str) -> Vec<String> {
        fn_spans(src, name)
            .into_iter()
            .filter_map(|(start, end)| {
                block_span(src.as_bytes(), start).map(|(open, close)| src[open..close].to_string())
            })
            .collect()
    }

    /// Source with every `fn name ... { ... }` definition removed.
    fn remove_fn_bodies(src: &str, names: &[&str]) -> String {
        let mut spans = Vec::new();
        for name in names {
            spans.extend(fn_spans(src, name));
        }
        spans.sort();
        let mut out = String::with_capacity(src.len());
        let mut cursor = 0;
        for (start, end) in spans {
            out.push_str(&src[cursor..start]);
            cursor = end;
        }
        out.push_str(&src[cursor..]);
        out
    }

    /// Parse `.nest("prefix", var)` plus `let var = handlers::mod::...`
    /// bindings into (module, prefix) pairs.
    fn parse_web_nests(web_src: &str) -> Vec<(String, String)> {
        use std::collections::HashMap;
        let mut var_prefix: HashMap<String, String> = HashMap::new();
        let mut rest = web_src;
        while let Some(pos) = rest.find(".nest(\"") {
            let start = pos + ".nest(\"".len();
            if let Some(end) = rest[start..].find('"') {
                let prefix = rest[start..start + end].to_string();
                let after = &rest[start + end..];
                if let Some(comma) = after.find(',') {
                    let var: String = after[comma + 1..]
                        .chars()
                        .skip_while(|c| c.is_whitespace())
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !var.is_empty() {
                        var_prefix.insert(var, prefix);
                    }
                }
                rest = &rest[start + end..];
            } else {
                break;
            }
        }
        let mut out = Vec::new();
        for line in web_src.lines() {
            if line.contains("handlers::") && line.contains("create_routes") {
                if let Some(mod_pos) = line.find("handlers::") {
                    let tail = &line[mod_pos + "handlers::".len()..];
                    let module: String = tail
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if let Some(let_pos) = line.find("let ") {
                        let var: String = line[let_pos + 4..]
                            .chars()
                            .skip_while(|c| c.is_whitespace())
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if let Some(prefix) = var_prefix.get(&var) {
                            out.push((module, prefix.clone()));
                        }
                    }
                }
            }
        }
        out
    }
}
