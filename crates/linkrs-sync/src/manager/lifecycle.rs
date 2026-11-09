//! Sync manager construction, builder-style configuration and runtime lifecycle.
use super::*;
use linkrs_metrics::StatsManager;
#[cfg_attr(
    not(any(feature = "fulltext", feature = "vector")),
    allow(unused_variables)
)]
impl super::SyncManager {
    fn new_common() -> Self {
        Self {
            #[cfg(feature = "fulltext")]
            sync_coordinator: None,
            #[cfg(feature = "vector")]
            vector_coordinator: None,
            pending_intents: DashMap::new(),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sqlite_outbox: None,
            #[cfg(feature = "vector")]
            vector_receiver: None,
            outbox_consumer: Arc::new(OutboxConsumerConfig::default()),
            backpressure: Arc::new(OutboxBackpressureConfig::default()),
            auth_paused_until_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            stats_manager: None,
            runtime_handle: None,
            handle: Mutex::new(None),
            rebuild_locks: DashMap::new(),
        }
    }

    #[cfg(feature = "fulltext")]
    pub fn new(sync_coordinator: Arc<FulltextSyncCoordinator>) -> Self {
        Self {
            sync_coordinator: Some(sync_coordinator),
            ..Self::new_common()
        }
    }

    pub fn new_without_fulltext() -> Self {
        Self::new_common()
    }

    #[cfg(feature = "vector")]
    pub fn with_vector_coordinator(
        mut self,
        vector_coordinator: Arc<VectorSyncCoordinator>,
    ) -> Self {
        self.vector_coordinator = Some(vector_coordinator);
        self
    }

    pub fn with_stats_manager(mut self, stats_manager: Arc<StatsManager>) -> Self {
        self.stats_manager = Some(stats_manager);
        self
    }

    pub fn set_stats_manager(&mut self, stats_manager: Arc<StatsManager>) {
        self.stats_manager = Some(stats_manager);
    }

    /// Inject the runtime that owns the background delivery task.
    /// Assembly provides it; the manager never creates one itself.
    pub fn with_runtime_handle(mut self, handle: tokio::runtime::Handle) -> Self {
        self.runtime_handle = Some(handle);
        self
    }

    /// Inject the runtime that owns the background delivery task.
    /// Assembly provides it; the manager never creates one itself.
    pub fn set_runtime_handle(&mut self, handle: tokio::runtime::Handle) {
        self.runtime_handle = Some(handle);
    }

    pub async fn start(&self) -> Result<(), SyncError> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        if self.sqlite_outbox.is_some() {
            let mut handle = self.handle.lock().await;
            if handle.is_none() {
                let manager = self.clone();
                let task = async move {
                    let retry_interval = std::time::Duration::from_secs(5);
                    while manager.running.load(std::sync::atomic::Ordering::SeqCst) {
                        if let Err(error) = manager.retry_outbox_sync() {
                            tracing::warn!("Outbox delivery attempt failed: {}", error);
                        }
                        tokio::time::sleep(retry_interval).await;
                    }
                };
                if let Some(runtime) = self.runtime_handle.clone() {
                    *handle = Some(runtime.spawn(task));
                } else if tokio::runtime::Handle::try_current().is_ok() {
                    *handle = Some(tokio::spawn(task));
                } else {
                    self.running
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                    return Err(SyncError::Internal(
                        "cannot start outbox delivery without a tokio runtime; inject one with set_runtime_handle"
                            .to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    pub async fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);

        if let Some(handle) = self.handle.lock().await.take() {
            let _ = handle.await;
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }
}
