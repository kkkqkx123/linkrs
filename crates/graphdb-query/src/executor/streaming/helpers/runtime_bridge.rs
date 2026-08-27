//! Bridge for awaiting async coordinator operations from operator threads.
//!
//! Operators execute on arbitrary worker threads that may or may not run
//! inside a tokio runtime. Remote engines require the ambient runtime's
//! reactor and timers, so blocking waits must go through the shared
//! runtime-aware helper instead of a bare executor block-on.

#[cfg(feature = "vector")]
use crate::core::error::QueryError;

/// Drive `future` to completion on the calling thread, mapping both the
/// bridging failure and the operation failure into a labeled query error.
#[cfg(feature = "vector")]
pub(crate) fn wait<F, T, E>(label: &str, future: F) -> Result<T, QueryError>
where
    F: std::future::Future<Output = Result<T, E>> + Send,
    T: Send,
    E: Send + std::fmt::Display,
{
    let label = format!("{label} failed");
    crate::sync::runtime::block_on_ambient(future)
        .map_err(|error| QueryError::execution(format!("{label}: {error}")))?
        .map_err(|error| QueryError::execution(format!("{label}: {error}")))
}
