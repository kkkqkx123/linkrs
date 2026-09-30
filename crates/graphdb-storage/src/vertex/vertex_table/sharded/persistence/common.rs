//! Shared checkpoint primitives: manifest version plus wall-clock helper.

/// Persistent layout version of both manifests. Stays at 1 through the
/// development phase: any manifest whose version differs from this constant
/// is rejected with a rebuild directive; there is no automatic migration
/// and old on-disk data is never made compatible.
pub(crate) const MANIFEST_FORMAT_VERSION: u8 = 1;

pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
