//! systemd readiness notification helpers.
//!
//! These helpers are safe to call in non-systemd environments: when
//! `NOTIFY_SOCKET` is unset they become no-ops.

use log::debug;
use std::env;

#[cfg(unix)]
use std::os::unix::net::UnixDatagram;

/// Notify systemd that the service is ready.
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
        let socket_path = match env::var("NOTIFY_SOCKET") {
            Ok(value) if !value.is_empty() => value,
            _ => return,
        };

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
        let _ = ();
    }
}
