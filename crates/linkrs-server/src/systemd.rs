//! systemd readiness notification helpers.
//!
//! These helpers implement the client side of the `sd_notify` protocol
//! without extra dependencies: notifications are sent as datagrams to the
//! socket named by `NOTIFY_SOCKET`. All helpers are best-effort and safe to
//! call outside systemd: when `NOTIFY_SOCKET` is unset or empty they are
//! no-ops, and any socket failure is only logged so startup and shutdown are
//! never blocked by the notification path.

#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::os::unix::net::UnixDatagram;

#[cfg(unix)]
use log::debug;

/// Notify systemd that the service has bound its listeners and is ready.
pub fn notify_ready() {
    send("READY=1");
}

/// Notify systemd that the service is stopping.
pub fn notify_stopping() {
    send("STOPPING=1");
}

fn send(payload: &str) {
    #[cfg(not(unix))]
    {
        let _ = payload;
        return;
    }

    #[cfg(unix)]
    {
        send_unix(payload);
    }
}

#[cfg(unix)]
fn send_unix(payload: &str) {
    let socket_path = match env::var("NOTIFY_SOCKET") {
        Ok(value) if !value.is_empty() => value,
        _ => return,
    };
    // Abstract namespace addresses are passed with a leading `@`, which the
    // standard datagram interface cannot express. Skip them with a diagnostic
    // instead of failing against a misleading filesystem path.
    if socket_path.starts_with('@') {
        debug!(
            "skipping systemd notification '{}': abstract socket addresses are not supported",
            payload
        );
        return;
    }

    let socket = match UnixDatagram::unbound() {
        Ok(socket) => socket,
        Err(err) => {
            debug!("failed to create systemd notification socket: {}", err);
            return;
        }
    };

    if let Err(err) = socket.send_to(payload.as_bytes(), &socket_path) {
        debug!("failed to send systemd notification '{}': {}", payload, err);
    } else {
        debug!("sent systemd notification: {}", payload);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn with_notify_socket<T>(value: Option<&str>, run: impl FnOnce() -> T) -> T {
        let saved = std::env::var("NOTIFY_SOCKET").ok();
        match value {
            Some(socket) => std::env::set_var("NOTIFY_SOCKET", socket),
            None => std::env::remove_var("NOTIFY_SOCKET"),
        }
        let result = run();
        match saved {
            Some(socket) => std::env::set_var("NOTIFY_SOCKET", socket),
            None => std::env::remove_var("NOTIFY_SOCKET"),
        }
        result
    }

    #[test]
    fn notify_without_socket_is_noop() {
        with_notify_socket(None, || {
            notify_ready();
            notify_stopping();
        });
    }

    #[test]
    fn notify_with_empty_socket_is_noop() {
        with_notify_socket(Some(""), || {
            notify_ready();
            notify_stopping();
        });
    }

    #[test]
    fn notify_with_unreachable_socket_never_fails() {
        with_notify_socket(Some("/nonexistent/linkrs-notify.sock"), || {
            notify_ready();
            notify_stopping();
        });
    }

    #[test]
    fn notify_with_abstract_socket_is_skipped() {
        with_notify_socket(Some("@/nonexistent-abstract"), || {
            notify_ready();
            notify_stopping();
        });
    }
}
