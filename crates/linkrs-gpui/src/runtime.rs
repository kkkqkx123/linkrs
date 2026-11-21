use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui_kit::{App, Global, Task};
use linkrs_client::ClientError;

/// A cancellation signal for a spawned request. Calling [`cancel`](Self::cancel)
/// drops the underlying tokio future, which aborts an in-flight HTTP request.
#[derive(Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    tx: Arc<std::sync::Mutex<Option<futures::channel::oneshot::Sender<()>>>>,
}

impl CancellationToken {
    fn new() -> (Self, futures::channel::oneshot::Receiver<()>) {
        let (tx, rx) = futures::channel::oneshot::channel();
        (
            Self {
                cancelled: Arc::new(AtomicBool::new(false)),
                tx: Arc::new(std::sync::Mutex::new(Some(tx))),
            },
            rx,
        )
    }

    /// Request cancellation. Safe to call more than once or after completion.
    /// The `swap` makes repeated calls a no-op, so no separate query accessor
    /// is needed.
    pub fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            let sender = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(sender) = sender {
                let _ = sender.send(());
            }
        }
    }
}

/// Tokio runtime driving `linkrs-client` requests. The client stack (reqwest)
/// expects a tokio reactor, while GPUI runs its own executor, so the client
/// futures are polled here and forwarded back through a plain oneshot channel.
pub struct RuntimeGlobal(Option<tokio::runtime::Runtime>);

impl Global for RuntimeGlobal {}

pub fn init(runtime: tokio::runtime::Runtime, cx: &mut App) {
    cx.set_global(RuntimeGlobal(Some(runtime)));
}

/// Drive `future` on the tokio runtime and expose the result as a gpui task.
///
/// Dropping the returned task cancels only the local await; the tokio side
/// still runs to completion and its result is discarded (abandon semantics).
pub fn spawn<R, F>(cx: &App, future: F) -> Task<Result<R, ClientError>>
where
    F: Future<Output = Result<R, ClientError>> + Send + 'static,
    R: Send + 'static,
{
    let runtime = cx
        .try_global::<RuntimeGlobal>()
        .and_then(|global| global.0.as_ref());
    let Some(runtime) = runtime else {
        return Task::ready(Err(ClientError::Other(
            "tokio runtime is not initialized".into(),
        )));
    };

    let (tx, rx) = futures::channel::oneshot::channel();
    runtime.spawn(async move {
        let result = future.await;
        _ = tx.send(result);
    });

    cx.background_executor().spawn(async move {
        rx.await
            .unwrap_or_else(|_| Err(ClientError::Other("background request dropped".into())))
    })
}

/// Like [`spawn`], but returns a token that aborts the underlying request when
/// cancelled. Dropping the gpui task alone does not stop the tokio future (see
/// the abandon semantics above); callers that retire work early — for example
/// when a view's generation changes — should call
/// [`cancel`](CancellationToken::cancel) to release the HTTP request promptly.
pub fn spawn_cancellable<R, F>(
    cx: &App,
    future: F,
) -> (Task<Result<R, ClientError>>, CancellationToken)
where
    F: Future<Output = Result<R, ClientError>> + Send + 'static,
    R: Send + 'static,
{
    let runtime = cx
        .try_global::<RuntimeGlobal>()
        .and_then(|global| global.0.as_ref());
    let (token, cancel_rx) = CancellationToken::new();
    let Some(runtime) = runtime else {
        return (
            Task::ready(Err(ClientError::Other(
                "tokio runtime is not initialized".into(),
            ))),
            token,
        );
    };

    let (tx, rx) = futures::channel::oneshot::channel();
    runtime.spawn(async move {
        tokio::select! {
            biased;
            _ = cancel_rx => {
                let _ = tx.send(Err(ClientError::Other("request cancelled".into())));
            }
            result = future => {
                let _ = tx.send(result);
            }
        }
    });

    let task = cx.background_executor().spawn(async move {
        rx.await
            .unwrap_or_else(|_| Err(ClientError::Other("background request dropped".into())))
    });
    (task, token)
}
