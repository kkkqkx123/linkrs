use dashmap::DashMap;
use log::{info, warn};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time;

use super::{SessionError, SessionResult};
use crate::api::server::client::{ClientSession, Session};

pub const DEFAULT_MAX_ALLOWED_CONNECTIONS: usize = 100; // Default maximum number of connections (in a single-node scenario)
pub const DEFAULT_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(600); // 10 minutes

/// Global session ID counter, used to generate unique session IDs
static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Session information, used to display the list of sessions
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: i64,
    pub user_name: String,
    pub space_name: Option<String>,
    pub graph_addr: Option<String>,
    pub create_time: SystemTime,
    pub last_access_time: SystemTime,
    pub active_queries: usize,
    pub timezone: Option<i32>,
}

impl SessionInfo {
    pub fn from_client_session(session: &ClientSession, create_time: SystemTime) -> Self {
        Self {
            session_id: session.id(),
            user_name: session.user(),
            space_name: session.space_name(),
            graph_addr: session.graph_addr(),
            create_time,
            last_access_time: SystemTime::now() - Duration::from_millis(session.idle_seconds()),
            active_queries: session.active_queries_count(),
            timezone: session.timezone(),
        }
    }

    pub fn from_params(
        session_id_str: &str,
        user_name: &str,
        space_name: Option<String>,
        client_ip: &str,
        client_port: u16,
    ) -> Result<Self, String> {
        let session_id = session_id_str
            .parse::<i64>()
            .map_err(|_| format!("Invalid session ID: {}", session_id_str))?;

        let graph_addr = if client_ip.is_empty() {
            None
        } else {
            Some(format!("{}:{}", client_ip, client_port))
        };

        let now = SystemTime::now();
        Ok(Self {
            session_id,
            user_name: user_name.to_string(),
            space_name,
            graph_addr,
            create_time: now,
            last_access_time: now,
            active_queries: 0,
            timezone: None,
        })
    }

    pub fn touch(&mut self) {
        self.last_access_time = SystemTime::now();
    }
}

#[derive(Debug)]
pub struct GraphSessionManager {
    // Use DashMap to achieve true concurrent access without the need for explicit locking.
    sessions: Arc<DashMap<i64, Arc<ClientSession>>>,
    active_sessions: Arc<DashMap<i64, Instant>>, // session_id -> last_activity_time
    // Read more, write less – and use RwLock.
    session_create_times: Arc<RwLock<HashMap<i64, SystemTime>>>, // session_id -> create_time
    host_addr: String,
    max_connections: usize,
    session_idle_timeout: Duration,
    /// Is the background cleanup task currently running?
    cleanup_task_running: Arc<AtomicBool>,
}

impl GraphSessionManager {
    /// Create a new session manager.
    ///
    /// This constructor does not automatically initiate background cleanup tasks.
    /// Need to explicitly call `start_cleanup_task()` to start
    pub fn new(
        host_addr: String,
        max_connections: usize,
        session_idle_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            sessions: Arc::new(DashMap::new()),
            active_sessions: Arc::new(DashMap::new()),
            session_create_times: Arc::new(RwLock::new(HashMap::new())),
            host_addr,
            max_connections,
            session_idle_timeout,
            cleanup_task_running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Start the background session cleanup task.
    ///
    /// If the task is already running, this method will not start it again.
    pub async fn start_cleanup_task(self: &Arc<Self>) {
        if self.cleanup_task_running.swap(true, Ordering::SeqCst) {
            info!("Session cleanup task is already running");
            return;
        }

        info!("Starting session cleanup task");
        let manager_clone = Arc::clone(self);
        tokio::spawn(async move {
            manager_clone.background_reclamation_task().await;
        });
    }

    /// Stop the background session cleaning task.
    ///
    /// Set a stop flag; the background task will exit during the next iteration.
    pub fn stop_cleanup_task(&self) {
        info!("Stopping session cleanup task");
        self.cleanup_task_running.store(false, Ordering::SeqCst);
    }

    /// Check whether the background cleanup task is currently running.
    pub fn is_cleanup_task_running(&self) -> bool {
        self.cleanup_task_running.load(Ordering::SeqCst)
    }

    /// Creates a new session
    pub async fn create_session(
        &self,
        user_name: String,
        _client_ip: String,
    ) -> Result<Arc<ClientSession>, String> {
        info!("Creating new session for user: {}", user_name);

        // Check if we're out of connections
        if self.is_out_of_connections().await {
            warn!(
                "Failed to create session for user {}: maximum connections exceeded",
                user_name
            );
            return Err("Exceeded maximum allowed connections".to_string());
        }

        // Generate a new session ID
        let session_id = self.generate_session_id();
        info!(
            "Generated session ID: {} for user: {}",
            session_id, user_name
        );

        let session = Session {
            session_id,
            user_name: user_name.clone(),
            space_name: None,
            graph_addr: Some(self.host_addr.clone()),
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        // Add to sessions and active sessions
        let create_time = SystemTime::now();

        // DashMap does not require explicit locking; concurrent insertions are possible without any issues.
        self.sessions
            .insert(session_id, Arc::clone(&client_session));
        self.active_sessions.insert(session_id, Instant::now());

        // Write lock protection creation time
        {
            let mut create_times = self.session_create_times.write();
            create_times.insert(session_id, create_time);
        }

        info!(
            "Successfully created session ID: {} for user: {}",
            session_id, user_name
        );
        Ok(client_session)
    }

    /// Finds an existing session
    pub fn find_session(&self, session_id: i64) -> Option<Arc<ClientSession>> {
        // DashMap supports true concurrent reading without the need for locking.
        self.sessions.get(&session_id).map(|entry| entry.clone())
    }

    /// Finds an existing session only from local cache
    pub fn find_session_from_cache(&self, session_id: i64) -> Option<Arc<ClientSession>> {
        self.find_session(session_id)
    }

    /// Removes a session from local cache
    pub async fn remove_session(&self, session_id: i64) {
        info!("Removing session ID: {}", session_id);

        // DashMap does not require explicit locking.
        self.sessions.remove(&session_id);
        self.active_sessions.remove(&session_id);

        // Write lock protection creation time
        {
            let mut create_times = self.session_create_times.write();
            create_times.remove(&session_id);
        }

        info!("Successfully removed session ID: {}", session_id);
    }

    /// Gets all sessions from the local cache
    pub fn get_sessions_from_local_cache(&self) -> Vec<Session> {
        // DashMap supports iterators, and no locking is required.
        self.sessions
            .iter()
            .map(|entry| entry.value().get_session())
            .collect()
    }

    /// Obtain information about the session list, which is used for the SHOW SESSIONS command.
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        // Read lock acquisition time
        let create_times = self.session_create_times.read();

        // DashMap’s iteration process does not require locking.
        self.sessions
            .iter()
            .filter_map(|entry| {
                let session_id = entry.key();
                let client_session = entry.value();
                create_times.get(session_id).map(|&create_time| {
                    SessionInfo::from_client_session(client_session, create_time)
                })
            })
            .collect()
    }

    /// Obtain detailed information about the specified session.
    pub async fn get_session_info(&self, session_id: i64) -> Option<SessionInfo> {
        // Reading DashMap does not require any locking mechanisms.
        let client_session = self.sessions.get(&session_id)?;

        // Read lock acquisition time
        let create_times = self.session_create_times.read();
        create_times
            .get(&session_id)
            .map(|&create_time| SessionInfo::from_client_session(&client_session, create_time))
    }

    /// Terminate the specified session (KILL SESSION)
    ///
    /// # Parameters
    /// `session_id` – The ID of the session that needs to be terminated.
    /// `current_user` – The username of the user who is performing the termination operation.
    /// `is_admin` – Determines whether the current user has the Admin role.
    ///
    /// # Return
    /// * `Ok(())` - Successfully terminate the session
    /// * `Err(SessionError)` - Specific reasons for termination failure
    pub async fn kill_session(
        &self,
        session_id: i64,
        current_user: &str,
        is_admin: bool,
    ) -> SessionResult<()> {
        info!(
            "Attempting to kill session ID: {} by user: {} (is_admin: {})",
            session_id, current_user, is_admin
        );

        // Find the target conversation.
        let target_session = self
            .find_session(session_id)
            .ok_or(SessionError::session_not_found(session_id))?;

        let target_user = target_session.user();

        // Permission check: You can only terminate your own session, or you must have Admin privileges.
        if !is_admin && target_user != current_user {
            warn!(
                "User {} attempted to kill session {} without permission (target user: {})",
                current_user, session_id, target_user
            );
            return Err(SessionError::insufficient_permission());
        }

        info!(
            "Killing session {} (user: {}, active queries: {})",
            session_id,
            target_user,
            target_session.active_queries_count()
        );

        // Terminate all queries in the session.
        target_session.mark_all_queries_killed();

        // Remove the session from the manager.
        self.remove_session(session_id).await;

        info!(
            "Successfully killed session ID: {} by user: {}",
            session_id, current_user
        );
        Ok(())
    }

    /// Batch termination of multiple sessions
    pub async fn kill_multiple_sessions(
        &self,
        session_ids: &[i64],
        current_user: &str,
        is_admin: bool,
    ) -> Vec<SessionResult<()>> {
        let mut results = Vec::with_capacity(session_ids.len());
        for &session_id in session_ids {
            results.push(self.kill_session(session_id, current_user, is_admin).await);
        }
        results
    }

    /// Whether exceeds the max allowed connections
    pub async fn is_out_of_connections(&self) -> bool {
        // DashMap's len() is an O(1) operation, no lock required
        self.active_sessions.len() >= self.max_connections
    }

    /// Obtain the number of active sessions
    pub async fn active_session_count(&self) -> usize {
        self.active_sessions.len()
    }

    /// Obtaining the maximum limit for the number of connections
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// Generate a new unique session ID
    ///
    /// Generate a unique session ID using a combination of strategies:
    /// High 48 bits: Current timestamp (in milliseconds)
    /// Lower 16 bits: An auto-incrementing counter
    /// Make sure that the IDs generated within the same millisecond are also unique.
    fn generate_session_id(&self) -> i64 {
        let timestamp_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("System time is before Unix epoch")
            .as_millis() as u64;

        let counter = SESSION_ID_COUNTER.fetch_add(1, Ordering::SeqCst) & 0xFFFF;

        // Combining timestamps and counters
        let session_id = ((timestamp_millis & 0xFFFFFFFFFFFF0000) | counter) as i64;

        // Ensure that the generated ID is a positive number and not equal to 0.
        if session_id <= 0 {
            // If the generated ID is invalid, use the hash value of the timestamp.
            ((timestamp_millis.wrapping_mul(0x9E3779B97F4A7C15)) & 0x7FFFFFFFFFFFFFFF) as i64
        } else {
            session_id
        }
    }

    /// Background task: Regularly clean up expired sessions.
    ///
    /// Check every 30 seconds and remove sessions that have exceeded the idle timeout period.
    /// Can be stopped via the `stop_cleanup_task()` method
    async fn background_reclamation_task(self: Arc<Self>) {
        let mut interval = time::interval(Duration::from_secs(30));

        loop {
            interval.tick().await;

            // Check whether it should be stopped.
            if !self.cleanup_task_running.load(Ordering::SeqCst) {
                info!("Session cleanup task is stopping");
                break;
            }

            self.reclaim_expired_sessions().await;
        }

        info!("Session cleanup task has stopped");
    }

    /// Reclaims expired sessions
    async fn reclaim_expired_sessions(&self) {
        // DashMap supports iteration without the need for locking.
        let expired_sessions: Vec<i64> = self
            .active_sessions
            .iter()
            .filter(|entry| entry.value().elapsed() > self.session_idle_timeout)
            .map(|entry| *entry.key())
            .collect();

        if !expired_sessions.is_empty() {
            info!(
                "Found {} expired sessions to reclaim",
                expired_sessions.len()
            );
        }

        // Remove expired sessions
        for session_id in expired_sessions {
            info!("Reclaiming expired session ID: {}", session_id);
            self.remove_session(session_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_session_manager() -> Arc<GraphSessionManager> {
        GraphSessionManager::new(
            "127.0.0.1:9669".to_string(),
            DEFAULT_MAX_ALLOWED_CONNECTIONS,
            DEFAULT_SESSION_IDLE_TIMEOUT,
        )
    }

    #[tokio::test]
    async fn test_session_manager_creation() {
        let session_manager = create_test_session_manager();

        assert_eq!(session_manager.host_addr, "127.0.0.1:9669");
        assert_eq!(session_manager.get_sessions_from_local_cache().len(), 0);
        assert!(!session_manager.is_cleanup_task_running());
    }

    #[tokio::test]
    async fn test_create_and_find_session() {
        let session_manager = create_test_session_manager();

        let session = session_manager
            .create_session("testuser".to_string(), "127.0.0.1".to_string())
            .await
            .expect("Failed to create session");

        assert_eq!(session.user(), "testuser");
        assert!(!session_manager.is_out_of_connections().await);

        let found_session = session_manager
            .find_session(session.id())
            .expect("Failed to find session");
        assert_eq!(found_session.user(), "testuser");

        // Test find non-existent session
        assert!(session_manager.find_session(999999).is_none());
    }

    #[tokio::test]
    async fn test_remove_session() {
        let session_manager = create_test_session_manager();

        let session = session_manager
            .create_session("testuser".to_string(), "127.0.0.1".to_string())
            .await
            .expect("Failed to create session");

        assert!(session_manager.find_session(session.id()).is_some());

        session_manager.remove_session(session.id()).await;
        assert!(session_manager.find_session(session.id()).is_none());
    }

    #[tokio::test]
    async fn test_max_connections() {
        let session_manager = GraphSessionManager::new(
            "127.0.0.1:9669".to_string(),
            5,
            DEFAULT_SESSION_IDLE_TIMEOUT,
        );

        assert!(!session_manager.is_out_of_connections().await);

        for i in 0..5 {
            let _ = session_manager
                .create_session(format!("user{}", i), "127.0.0.1".to_string())
                .await;
        }

        assert!(session_manager.is_out_of_connections().await);

        // Attempts to create a 6th session should fail
        let result = session_manager
            .create_session("user6".to_string(), "127.0.0.1".to_string())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_kill_session() {
        let session_manager = create_test_session_manager();

        let session = session_manager
            .create_session("testuser".to_string(), "127.0.0.1".to_string())
            .await
            .expect("Failed to create session");

        let session_id = session.id();

        // Normal users try to terminate their own session - should succeed
        let result = session_manager
            .kill_session(session_id, "testuser", false)
            .await;
        assert!(result.is_ok());
        assert!(session_manager.find_session(session_id).is_none());

        // Create a new session to test permission checking
        let session2 = session_manager
            .create_session("user2".to_string(), "127.0.0.1".to_string())
            .await
            .expect("Failed to create session");

        // Normal user attempts to terminate another user's session - should fail
        let result = session_manager
            .kill_session(session2.id(), "otheruser", false)
            .await;
        assert!(result.is_err());

        // Admin Terminate another user's session - should succeed!
        let result = session_manager
            .kill_session(session2.id(), "admin", true)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let session_manager = create_test_session_manager();

        // Creating Multiple Sessions
        for i in 0..3 {
            let _ = session_manager
                .create_session(format!("user{}", i), "127.0.0.1".to_string())
                .await;
        }

        let sessions = session_manager.list_sessions().await;
        assert_eq!(sessions.len(), 3);
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        use tokio::task;

        let session_manager = create_test_session_manager();
        let mut handles = vec![];

        // Concurrent Session Creation
        for i in 0..10 {
            let manager = Arc::clone(&session_manager);
            let handle = task::spawn(async move {
                manager
                    .create_session(format!("user{}", i), "127.0.0.1".to_string())
                    .await
            });
            handles.push(handle);
        }

        // Waiting for all tasks to be completed
        for handle in handles {
            let _ = handle.await.expect("Task should complete");
        }

        // Verify that all sessions were created successfully
        assert_eq!(session_manager.get_sessions_from_local_cache().len(), 10);
    }
}
