use axum::{extract::Request, middleware::Next, response::Response};
use log::error;

pub async fn error_handling_middleware(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().to_string();

    let response = next.run(request).await;

    let status = response.status();
    if status.is_server_error() {
        error!("{} {} returned {}", method, path, status);
    }

    response
}
