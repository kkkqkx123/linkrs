//! Shutdown signal handling
//!
//! Shutdown is triggered by OS signals (Ctrl+C everywhere, SIGTERM on Unix)
//! or programmatically through `request_shutdown`, which the Windows service
//! control handler uses. Every waiter observes the same trigger, so signal
//! and service-manager stops share one graceful shutdown path.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use log::info;
use tokio::signal;
use tokio::sync::watch;

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_SENDER: OnceLock<watch::Sender<bool>> = OnceLock::new();

fn shutdown_sender() -> &'static watch::Sender<bool> {
    SHUTDOWN_SENDER.get_or_init(|| watch::channel(false).0)
}

/// Request shutdown programmatically (for example from a service control
/// handler). Safe to call from any thread, including non-async contexts.
/// Only consumed by the Windows service module, hence the scoped lint.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    let _ = shutdown_sender().send(true);
}

/// Future resolving once shutdown has been requested by any trigger.
///
/// The flag is checked before waiting so a request issued before any waiter
/// subscribes is never lost.
async fn shutdown_notified() {
    let mut receiver = shutdown_sender().subscribe();
    if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        return;
    }
    let _ = receiver.wait_for(|requested| *requested).await;
}

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

    let programmatic = async {
        shutdown_notified().await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
        _ = programmatic => {},
    }

    info!("shutdown signal received, starting graceful shutdown");
}
