//! Runtime-aware bridging from synchronous code to asynchronous operations.
//!
//! Operators and managers run on arbitrary threads (per-query worker threads,
//! session threads, maintenance threads). A bare executor-agnostic
//! `block_on` cannot drive futures that depend on the tokio reactor or
//! timers (network clients, timeouts), so callers should route through this
//! helper, which reuses the ambient runtime when one is present.

/// Error raised when no runtime can be established to drive a future.
#[derive(Debug)]
pub struct RuntimeBridgeError(String);

impl std::fmt::Display for RuntimeBridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "runtime bridge failed: {}", self.0)
    }
}

impl std::error::Error for RuntimeBridgeError {}

/// Block the calling thread until `future` completes on an injected owner.
///
/// The injected handle is authoritative when present: it avoids creating a
/// transient runtime on the commit hot path. Only when no handle was
/// injected does the caller fall back to `block_on_ambient`.
///
/// Behavior with an injected handle:
/// - outside any runtime: drive directly on the injected handle;
/// - inside a runtime: `block_in_place` when the flavor allows it,
///   otherwise drive on a helper thread that blocks on the injected handle.
///   The future itself always runs on the injected owner; no new runtime is
///   created here.
pub fn block_on_injected<F, T>(
    handle: &tokio::runtime::Handle,
    future: F,
) -> Result<T, RuntimeBridgeError>
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                Ok(tokio::task::block_in_place(|| handle.block_on(future)))
            }
            _ => std::thread::scope(|scope| {
                scope
                    .spawn(|| Ok(handle.block_on(future)))
                    .join()
                    .map_err(|_| {
                        RuntimeBridgeError("injected runtime thread panicked".to_string())
                    })?
            }),
        };
    }
    Ok(handle.block_on(future))
}

/// Block the calling thread until `future` completes.
///
/// Behavior by ambient context:
/// - inside a multi-thread runtime: `block_in_place` on the current worker;
/// - inside a current-thread runtime: drive the future on a dedicated
///   transient runtime thread (the worker itself must stay free);
/// - outside any runtime: drive the future on a transient current-thread
///   runtime created here.
pub fn block_on_ambient<F, T>(future: F) -> Result<T, RuntimeBridgeError>
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                Ok(tokio::task::block_in_place(|| handle.block_on(future)))
            }
            tokio::runtime::RuntimeFlavor::CurrentThread => std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|error| RuntimeBridgeError(error.to_string()))?;
                        Ok(runtime.block_on(future))
                    })
                    .join()
                    .map_err(|_| {
                        RuntimeBridgeError("transient runtime thread panicked".to_string())
                    })?
            }),
            _ => Ok(handle.block_on(future)),
        };
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| RuntimeBridgeError(error.to_string()))?;
    Ok(runtime.block_on(future))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_future_outside_any_runtime() {
        let value = block_on_ambient(async { 41 + 1 }).expect("bridge must succeed");
        assert_eq!(value, 42);
    }

    #[test]
    fn completes_future_inside_multi_thread_runtime() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .expect("test runtime must build");
        let value = runtime.block_on(async move {
            let inner = tokio::task::spawn_blocking(|| {
                block_on_ambient(async { std::string::String::from("from-blocking") })
                    .expect("bridge must succeed")
            })
            .await
            .expect("spawn_blocking must succeed");
            assert_eq!(inner, "from-blocking");
            block_on_ambient(async { true }).expect("bridge must succeed")
        });
        assert!(value);
    }
}
