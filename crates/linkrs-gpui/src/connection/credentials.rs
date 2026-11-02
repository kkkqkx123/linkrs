//! OS keychain access for remembered login passwords.
//!
//! Passwords are stored per connection endpoint and only with explicit user
//! consent (the login view checkbox). Reads that miss, and any platform
//! failure such as a missing Secret Service daemon or a locked store, resolve
//! to an empty result with a log record; callers fall back to an in-memory
//! login and never write secrets to plain files. Session ids stay
//! memory-only by design and never pass through here.
use keyring::Error;

/// Service name under which all entries are kept in the platform store.
const SERVICE_NAME: &str = "linkrs-gpui";

/// Build the account identifier for an endpoint. Pure and side-effect free so
/// key layout stays covered by unit tests without touching the OS store.
///
/// Host and user are trimmed; the user keeps its case to match server-side
/// authentication semantics, while the port keeps endpoints distinct.
pub fn account(host: &str, port: u16, user: &str) -> String {
    format!("{}@{}:{}", user.trim(), host.trim(), port)
}

/// Load the remembered password for an endpoint. Returns `None` when nothing
/// was stored or when the platform store is unavailable. The secret value is
/// never logged.
pub fn load(host: &str, port: u16, user: &str) -> Option<String> {
    let account = account(host, port, user);
    let entry = match keyring::Entry::new(SERVICE_NAME, &account) {
        Ok(entry) => entry,
        Err(error) => {
            log::warn!("credential store unavailable for {host}:{port} ({user}): {error}");
            return None;
        }
    };
    match entry.get_password() {
        Ok(password) => Some(password),
        Err(Error::NoEntry) => {
            log::debug!("no remembered password for {host}:{port} ({user})");
            None
        }
        Err(error) => {
            log::warn!("failed to read remembered password for {host}:{port} ({user}): {error}");
            None
        }
    }
}

/// Remember the password for an endpoint, replacing any previous entry.
/// Empty passwords are rejected. Returns whether the entry was stored.
pub fn save(host: &str, port: u16, user: &str, password: &str) -> bool {
    if password.is_empty() {
        log::warn!("refused to store an empty password for {host}:{port} ({user})");
        return false;
    }
    let account = account(host, port, user);
    let entry = match keyring::Entry::new(SERVICE_NAME, &account) {
        Ok(entry) => entry,
        Err(error) => {
            log::warn!("credential store unavailable for {host}:{port} ({user}): {error}");
            return false;
        }
    };
    match entry.set_password(password) {
        Ok(()) => true,
        Err(error) => {
            log::warn!("failed to store password for {host}:{port} ({user}): {error}");
            false
        }
    }
}

/// Forget the password stored for an endpoint. A missing entry already
/// satisfies the request and reports success.
pub fn delete(host: &str, port: u16, user: &str) -> bool {
    let account = account(host, port, user);
    let entry = match keyring::Entry::new(SERVICE_NAME, &account) {
        Ok(entry) => entry,
        Err(error) => {
            log::warn!("credential store unavailable for {host}:{port} ({user}): {error}");
            return false;
        }
    };
    match entry.delete_credential() {
        Ok(()) => true,
        Err(Error::NoEntry) => true,
        Err(error) => {
            log::warn!("failed to forget password for {host}:{port} ({user}): {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_trims_host_and_user() {
        assert_eq!(
            account("  127.0.0.1 ", 8080, " root "),
            "root@127.0.0.1:8080"
        );
    }

    #[test]
    fn account_keeps_user_case_and_distinguishes_ports() {
        assert_eq!(account("db", 8080, "Root"), "Root@db:8080");
        assert_ne!(account("db", 8080, "root"), account("db", 8080, "Root"));
        assert_ne!(account("db", 8080, "root"), account("db", 8081, "root"));
    }

    #[test]
    fn save_rejects_empty_password_without_touching_the_store() {
        assert!(!save("127.0.0.1", 8080, "root", ""));
    }
}
