use axum::{extract::Request, middleware::Next, response::Response};
use log::info;

pub async fn logging_middleware(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();

    let response = next.run(request).await;

    info!("{} {} -> {}", method, uri, response.status());

    response
}
