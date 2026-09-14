//! Sync manager construction, builder-style configuration and runtime lifecycle.
use super::*;
#[cfg(feature = "fulltext")]
use graphdb_fulltext::SyncConfig;
use graphdb_metrics::StatsManager;
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
            dead_letter_queue: None,
            sqlite_outbox: None,
            #[cfg(feature = "vector")]
            vector_receiver: None,
            outbox_consumer: Arc::new(OutboxConsumerConfig::default()),
            #[cfg(feature = "vector")]
            backend_policy: None,
            backpressure: Arc::new(OutboxBackpressureConfig::default()),
            auth_paused_until_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            stats_manager: None,
            handle: Mutex::new(None),
            rebuild_locks: DashMap::new(),
        }
    }

    #[cfg(feature = "fulltext")]
    pub fn new(sync_coordinator: Arc<SyncCoordinator>) -> Self {
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

    #[cfg(feature = "fulltext")]
    pub fn with_sync_config(
        sync_coordinator: Arc<SyncCoordinator>,
        _sync_config: SyncConfig,
    ) -> Self {
        Self::new(sync_coordinator)
    }

    pub fn with_dead_letter_queue(
        mut self,
        dead_letter_queue: Arc<crate::DeadLetterQueue>,
    ) -> Self {
        self.dead_letter_queue = Some(dead_letter_queue);
        self
    }

    pub fn with_stats_manager(mut self, stats_manager: Arc<StatsManager>) -> Self {
        self.stats_manager = Some(stats_manager);
        self
    }

    pub fn set_stats_manager(&mut self, stats_manager: Arc<StatsManager>) {
        self.stats_manager = Some(stats_manager);
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
                *handle = Some(tokio::spawn(async move {
                    let retry_interval = std::time::Duration::from_secs(5);
                    while manager.running.load(std::sync::atomic::Ordering::SeqCst) {
                        if let Err(error) = manager.retry_outbox_sync() {
                            tracing::warn!("Outbox delivery attempt failed: {}", error);
                        }
                        tokio::time::sleep(retry_interval).await;
                    }
                }));
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
