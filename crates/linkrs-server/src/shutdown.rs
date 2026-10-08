//! Shutdown signal handling

use log::info;
use tokio::signal;

/// Waiting for the shutdown signal (asynchronous implementation)
///
/// This function waits for the shutdown signal in an async context.
pub async fn shutdown_signal() {
    info!("Waiting for shutdown signal (Ctrl+C or SIGTERM)...");

    async_shutdown_signal().await;

    info!("Received shutdown signal");
}

/// Asynchronous shutdown signal (public for sibling modules)
pub async fn async_shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("shutdown signal received, starting graceful shutdown");
}
