use std::time::{Duration, Instant};

use crate::session::manager::SessionManager;
use crate::utils::error::{CliError, Result};

use super::{IsolationLevel, Savepoint, TransactionInfo, TransactionState};

pub struct TransactionManager {
    state: TransactionState,
    autocommit: bool,
    isolation_level: IsolationLevel,
    savepoints: Vec<Savepoint>,
    started_at: Option<Instant>,
    timeout: Option<Duration>,
    query_count: usize,
}

impl TransactionManager {
    pub fn new() -> Self {
        Self {
            state: TransactionState::Idle,
            autocommit: true,
            isolation_level: IsolationLevel::default(),
            savepoints: Vec::new(),
            started_at: None,
            timeout: None,
            query_count: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }

    pub fn is_failed(&self) -> bool {
        self.state.is_failed()
    }

    pub fn is_idle(&self) -> bool {
        self.state.is_idle()
    }

    pub fn autocommit(&self) -> bool {
        self.autocommit
    }

    pub fn set_autocommit(&mut self, value: bool) {
        self.autocommit = value;
    }

    pub fn isolation_level(&self) -> IsolationLevel {
        self.isolation_level
    }

    pub fn set_isolation_level(&mut self, level: IsolationLevel) {
        self.isolation_level = level;
    }

    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub async fn begin(&mut self, session: &mut SessionManager) -> Result<()> {
        if self.is_active() {
            return Err(CliError::TransactionAlreadyActive);
        }

        let query = format!(
            "BEGIN TRANSACTION ISOLATION LEVEL {}",
            self.isolation_level.as_str()
        );

        session.execute_query(&query).await?;

        self.state = TransactionState::Active {
            id: format!("tx_{}", uuid::Uuid::new_v4()),
            space: session.current_space().unwrap_or_default().to_string(),
        };
        self.started_at = Some(Instant::now());
        self.query_count = 0;
        self.savepoints.clear();

        Ok(())
    }

    pub async fn commit(&mut self, session: &mut SessionManager) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        session.execute_query("COMMIT").await?;

        self.state = TransactionState::Idle;
        self.started_at = None;
        self.savepoints.clear();

        Ok(())
    }

    pub async fn rollback(&mut self, session: &mut SessionManager) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        session.execute_query("ROLLBACK").await?;

        self.state = TransactionState::Idle;
        self.started_at = None;
        self.savepoints.clear();

        Ok(())
    }

    pub async fn create_savepoint(
        &mut self,
        name: &str,
        session: &mut SessionManager,
    ) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        session
            .execute_query(&format!("SAVEPOINT {}", name))
            .await?;

        self.savepoints
            .push(Savepoint::new(name.to_string(), self.query_count));

        Ok(())
    }

    pub async fn rollback_to_savepoint(
        &mut self,
        name: &str,
        session: &mut SessionManager,
    ) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        let pos = self
            .savepoints
            .iter()
            .position(|s| s.name == name)
            .ok_or_else(|| CliError::SavepointNotFound(name.to_string()))?;

        session
            .execute_query(&format!("ROLLBACK TO SAVEPOINT {}", name))
            .await?;

        let savepoint = &self.savepoints[pos];
        self.query_count = savepoint.query_count;
        self.savepoints.truncate(pos + 1);

        Ok(())
    }

    pub async fn release_savepoint(
        &mut self,
        name: &str,
        session: &mut SessionManager,
    ) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        let pos = self
            .savepoints
            .iter()
            .position(|s| s.name == name)
            .ok_or_else(|| CliError::SavepointNotFound(name.to_string()))?;

        session
            .execute_query(&format!("RELEASE SAVEPOINT {}", name))
            .await?;

        self.savepoints.remove(pos);

        Ok(())
    }

    pub fn record_query(&mut self) {
        self.query_count += 1;
    }

    pub fn mark_failed(&mut self, error: String) {
        if let TransactionState::Active { id, .. } = &self.state {
            self.state = TransactionState::Failed {
                id: id.clone(),
                error,
            };
        }
    }

    pub fn check_timeout(&self) -> Result<()> {
        if let (Some(started), Some(timeout)) = (self.started_at, self.timeout) {
            if started.elapsed() > timeout {
                return Err(CliError::TransactionTimeout);
            }
        }
        Ok(())
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.started_at.map(|t| t.elapsed().as_millis() as u64)
    }

    pub fn query_count(&self) -> usize {
        self.query_count
    }

    pub fn info(&self) -> TransactionInfo {
        TransactionInfo {
            state: self.state.clone(),
            autocommit: self.autocommit,
            isolation_level: self.isolation_level,
            duration_ms: self.duration_ms(),
            query_count: self.query_count,
            savepoints: self.savepoints.iter().map(|s| s.name.clone()).collect(),
        }
    }

    pub fn state(&self) -> &TransactionState {
        &self.state
    }

    pub fn has_savepoint(&self, name: &str) -> bool {
        self.savepoints.iter().any(|s| s.name == name)
    }

    pub fn savepoints(&self) -> &[Savepoint] {
        &self.savepoints
    }
}

impl Default for TransactionManager {
    fn default() -> Self {
        Self::new()
    }
}
