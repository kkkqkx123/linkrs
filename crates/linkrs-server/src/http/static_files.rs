//! Static frontend hosting for single-host deployments.
//!
//! Disabled by default, so a pure-API deployment never serves files. When
//! `[http].static_dir` is configured, this module serves the frontend static
//! directory (the `frontend/build` output) on the same port as the API: one
//! executable plus one static directory, no reverse proxy required.
//!
//! Routing precedence is API first, static second, SPA fallback last. The
//! fallback only rewrites extension-less non-API paths to the index page;
//! missing fingerprinted assets stay a 404 so broken builds fail loudly
//! instead of masquerading as a working page.

use axum::{
    extract::Request,
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json, Router,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tower::{service_fn, ServiceExt};
use tower_http::services::{ServeDir, ServeFile};

/// Resolved static hosting state shared by the file service.
#[derive(Debug, Clone)]
struct StaticHosting {
    dir: PathBuf,
    index: Arc<PathBuf>,
    fallback: bool,
    immutable_max_age_secs: u64,
}

/// Build the static router when static hosting is enabled.
///
/// Returns `Ok(None)` when `static_dir` is absent so the caller keeps the
/// pure-API router. Returns `Err` when hosting is enabled but the directory
/// is unusable, so the server refuses to start instead of serving a
/// half-broken page.
pub fn static_router_from_config<S>(
    config: &linkrs_config::HttpServerConfig,
) -> Result<Option<Router<S>>, String>
where
    S: Clone + Send + Sync + 'static,
{
    if !config.is_static_enabled() {
        return Ok(None);
    }
    let raw = config.static_dir.clone().unwrap_or_default();
    let dir = PathBuf::from(raw.trim());
    if !dir.is_dir() {
        return Err(format!("static directory is not usable: {}", dir.display()));
    }
    if config.static_fallback && !dir.join(config.static_index.trim()).is_file() {
        return Err(format!(
            "fallback page '{}' is missing in static directory {}",
            config.static_index.trim(),
            dir.display()
        ));
    }
    log::info!("Static frontend hosting enabled: {}", dir.display());
    Ok(Some(static_router_with(StaticHosting {
        index: Arc::new(dir.join(config.static_index.trim())),
        dir,
        fallback: config.static_fallback,
        immutable_max_age_secs: config.static_immutable_max_age_secs,
    })))
}

/// Assemble the fallback-only router for any router state.
///
/// The file service never needs handler state, so the same builder serves
/// both the production router and state-free test routers.
fn static_router_with<S>(hosting: StaticHosting) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let max_age = hosting.immutable_max_age_secs;
    let dir = hosting.dir.clone();
    let missing = service_fn(move |req: Request| {
        let hosting = hosting.clone();
        async move { Ok::<_, std::convert::Infallible>(serve_missing(&hosting, req).await) }
    });
    let files = ServeDir::new(&dir)
        .append_index_html_on_directories(false)
        .call_fallback_on_method_not_allowed(true)
        .fallback(missing);
    Router::new()
        .fallback_service(files)
        .layer(middleware::from_fn(
            move |request: Request, next: Next| async move {
                // API-looking paths keep their own missing semantics below;
                // the guard only hides dot segments on the static surface.
                let path = request.uri().path();
                if !is_api_path(path) && has_dot_segment(path) {
                    return json_404(format!("Static asset not found: {}", request.uri().path()));
                }
                let immutable = request.uri().path().starts_with("/_app/immutable/");
                let mut response = next.run(request).await;
                if let Ok(nosniff) = HeaderValue::from_str("nosniff") {
                    response
                        .headers_mut()
                        .insert(header::X_CONTENT_TYPE_OPTIONS, nosniff);
                }
                if immutable && response.status().is_success() {
                    let value = format!("public, max-age={max_age}, immutable");
                    if let Ok(header_value) = HeaderValue::from_str(&value) {
                        response
                            .headers_mut()
                            .insert(header::CACHE_CONTROL, header_value);
                    }
                }
                response
            },
        ))
}

/// Whether the path belongs to the API surface and must never fall back to
/// the index page.
fn is_api_path(path: &str) -> bool {
    path == "/v1"
        || path.starts_with("/v1/")
        || path == "/api"
        || path.starts_with("/api/")
        || path == "/metrics"
        || path == "/health"
        || path.starts_with("/health/")
        || path == "/healthz"
        || path == "/livez"
        || path == "/readyz"
        || path == "/api-docs"
        || path.starts_with("/api-docs/")
}

/// Whether any path segment is hidden or a parent reference.
///
/// Hidden files are never served; the guard runs before the file service
/// so their existence stays invisible. Segments are checked after
/// percent-decoding, mirroring the file service itself: an encoded dot
/// (`/%2esecret`) must not bypass the guard. Undecodable paths are
/// treated as hidden, matching the file service rejecting them.
fn has_dot_segment(path: &str) -> bool {
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8();
    let Ok(decoded) = decoded else {
        return true;
    };
    decoded
        .split('/')
        .any(|seg| seg == ".." || seg.starts_with('.'))
}

/// What an unmatched path resolves to once the file service misses.
#[derive(Debug, PartialEq, Eq)]
enum MissingKind {
    /// Unknown API path: stays a JSON 404, never the index page.
    UnknownApi,
    /// Missing fingerprinted asset: stays a 404 with the request path.
    MissingAsset,
    /// Extension-less page path: rewrites to the index page.
    IndexPage,
    /// SPA fallback disabled: stays a 404 with the request path.
    NoFallback,
}

/// Classify an unmatched path without touching the filesystem.
fn classify_missing(path: &str, fallback: bool) -> MissingKind {
    if is_api_path(path) {
        return MissingKind::UnknownApi;
    }
    let leaf = path.rsplit('/').next().unwrap_or(path);
    if let Some((stem, ext)) = leaf.rsplit_once('.') {
        if !stem.is_empty() && !ext.is_empty() {
            return MissingKind::MissingAsset;
        }
    }
    if fallback {
        MissingKind::IndexPage
    } else {
        MissingKind::NoFallback
    }
}

/// Whether the request only reads the target.
fn is_read(method: &Method) -> bool {
    *method == Method::GET || *method == Method::HEAD
}

/// Answer a request the file service could not satisfy.
///
/// Unknown API paths stay a JSON 404 for every method; only static page
/// and asset paths enforce the read-only constraint.
async fn serve_missing(hosting: &StaticHosting, req: Request) -> Response {
    let kind = classify_missing(req.uri().path(), hosting.fallback);
    if kind != MissingKind::UnknownApi && !is_read(req.method()) {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            Json(json!({
                "error": format!(
                    "Method {} is not served for static path {}",
                    req.method(),
                    req.uri().path()
                ),
                "status": StatusCode::METHOD_NOT_ALLOWED.as_u16(),
            })),
        )
            .into_response();
    }
    match kind {
        MissingKind::UnknownApi => {
            super::HttpError::not_found(format!("Unknown API path: {}", req.uri().path()))
                .into_response()
        }
        MissingKind::MissingAsset => {
            super::HttpError::not_found(format!("Static asset not found: {}", req.uri().path()))
                .into_response()
        }
        MissingKind::NoFallback => {
            super::HttpError::not_found(format!("No static route for path: {}", req.uri().path()))
                .into_response()
        }
        // Serve the fallback page through the file service so
        // conditional requests, ranges and HEAD keep file semantics;
        // only the cache policy is forced to no-store.
        MissingKind::IndexPage => match ServeFile::new(hosting.index.as_ref()).oneshot(req).await {
            Ok(mut response) => {
                if response.status() == StatusCode::NOT_FOUND {
                    return super::HttpError::internal(format!(
                        "Fallback page is missing: {}",
                        hosting.index.display()
                    ))
                    .into_response();
                }
                response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                );
                response.map(axum::body::Body::new)
            }
            Err(error) => super::HttpError::internal(format!(
                "Failed to read fallback page {}: {error}",
                hosting.index.display()
            ))
            .into_response(),
        },
    }
}

/// JSON 404 in the shared API error envelope.
fn json_404(message: String) -> Response {
    super::HttpError::not_found(message).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    fn test_hosting(dir: &std::path::Path, fallback: bool) -> StaticHosting {
        StaticHosting {
            dir: dir.to_path_buf(),
            index: Arc::new(dir.join("index.html")),
            fallback,
            immutable_max_age_secs: 60,
        }
    }

    fn write_fixture(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("_app/immutable")).expect("fixture dirs");
        std::fs::write(dir.join("index.html"), "<html>app</html>").expect("index fixture");
        std::fs::write(dir.join("_app/immutable/app.js"), "console.log(1)").expect("asset fixture");
    }

    fn get(uri: &str) -> Request {
        Request::builder()
            .uri(uri)
            .body(Body::from(""))
            .expect("test request")
    }

    #[test]
    fn missing_path_classification_prefers_api_and_assets() {
        assert_eq!(classify_missing("/v1/query", true), MissingKind::UnknownApi);
        assert_eq!(classify_missing("/api/data", true), MissingKind::UnknownApi);
        assert_eq!(classify_missing("/metrics", true), MissingKind::UnknownApi);
        assert_eq!(classify_missing("/health", true), MissingKind::UnknownApi);
        assert_eq!(classify_missing("/healthz", true), MissingKind::UnknownApi);
        assert_eq!(
            classify_missing("/favicon.png", true),
            MissingKind::MissingAsset
        );
        assert_eq!(
            classify_missing("/_app/immutable/app.js", true),
            MissingKind::MissingAsset
        );
        assert_eq!(classify_missing("/dashboard", true), MissingKind::IndexPage);
        assert_eq!(classify_missing("/", true), MissingKind::IndexPage);
        assert_eq!(
            classify_missing("/dashboard.", true),
            MissingKind::IndexPage
        );
        assert_eq!(
            classify_missing("/dashboard", false),
            MissingKind::NoFallback
        );
        assert_eq!(
            classify_missing("/api/data", false),
            MissingKind::UnknownApi
        );
    }

    #[tokio::test]
    async fn serves_files_and_falls_back_to_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(dir.path());
        let router = static_router_with::<()>(test_hosting(dir.path(), true));

        let response = router
            .clone()
            .oneshot(get("/_app/immutable/app.js"))
            .await
            .expect("static asset");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CACHE_CONTROL)
            .is_some_and(|v| v.to_str().unwrap_or_default().contains("immutable")));

        let response = router
            .clone()
            .oneshot(get("/dashboard"))
            .await
            .expect("spa path");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.to_str().unwrap_or_default().to_string())
                .as_deref(),
            Some("text/html; charset=utf-8"),
        );

        let response = router
            .oneshot(get("/missing.js"))
            .await
            .expect("missing asset");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rejects_non_read_methods_and_unknown_api() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(dir.path());
        let router = static_router_with::<()>(test_hosting(dir.path(), true));

        let post = Request::builder()
            .method(Method::POST)
            .uri("/dashboard")
            .body(Body::from(""))
            .expect("test request");
        let response = router.clone().oneshot(post).await.expect("post");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        let response = router
            .clone()
            .oneshot(get("/v1/no-such-route"))
            .await
            .expect("api");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let post_api = Request::builder()
            .method(Method::POST)
            .uri("/api/no-such-route")
            .body(Body::from(""))
            .expect("test request");
        let response = router.oneshot(post_api).await.expect("post api");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn disabled_fallback_keeps_page_paths_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(dir.path());
        let router = static_router_with::<()>(test_hosting(dir.path(), false));

        let response = router.oneshot(get("/dashboard")).await.expect("page");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn hidden_segments_stay_invisible() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(dir.path());
        std::fs::write(dir.path().join(".secret"), "hidden").expect("dotfile fixture");
        let router = static_router_with::<()>(test_hosting(dir.path(), true));

        let response = router
            .clone()
            .oneshot(get("/.secret"))
            .await
            .expect("dotfile");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = router
            .oneshot(get("/_app/../.secret"))
            .await
            .expect("traversal");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn encoded_dots_do_not_bypass_the_guard() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(dir.path());
        std::fs::write(dir.path().join(".secret"), "hidden").expect("dotfile fixture");
        let router = static_router_with::<()>(test_hosting(dir.path(), true));

        for uri in ["/%2esecret", "/%2Esecret", "/%2e%2e/.secret"] {
            let response = router.clone().oneshot(get(uri)).await.expect("encoded");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        }

        let response = router
            .oneshot(get("/%64ashboard"))
            .await
            .expect("encoded page");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
