use std::time::Instant;

use super::IsolationLevel;

#[derive(Debug, Clone, Default, PartialEq)]
pub enum TransactionState {
    #[default]
    Idle,
    Active {
        id: String,
        space: String,
    },
    Failed {
        id: String,
        error: String,
    },
}

impl TransactionState {
    pub fn is_active(&self) -> bool {
        matches!(self, TransactionState::Active { .. })
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, TransactionState::Failed { .. })
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, TransactionState::Idle)
    }

    pub fn id(&self) -> Option<&str> {
        match self {
            TransactionState::Active { id, .. } | TransactionState::Failed { id, .. } => Some(id),
            _ => None,
        }
    }

    pub fn error_message(&self) -> Option<&str> {
        match self {
            TransactionState::Failed { error, .. } => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Savepoint {
    pub name: String,
    pub created_at: Instant,
    pub query_count: usize,
}

impl Savepoint {
    pub fn new(name: String, query_count: usize) -> Self {
        Self {
            name,
            created_at: Instant::now(),
            query_count,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TransactionInfo {
    pub state: TransactionState,
    pub autocommit: bool,
    pub isolation_level: IsolationLevel,
    pub duration_ms: Option<u64>,
    pub query_count: usize,
    pub savepoints: Vec<String>,
}

impl TransactionInfo {
    pub fn format_status(&self) -> String {
        let mut output = String::new();

        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str("Transaction Status\n");
        output.push_str("─────────────────────────────────────────────────────────────\n");

        match &self.state {
            TransactionState::Idle => {
                output.push_str("State:           Idle\n");
            }
            TransactionState::Active { id, space } => {
                output.push_str("State:           Active\n");
                output.push_str(&format!("Transaction ID:  {}\n", id));
                output.push_str(&format!("Space:           {}\n", space));
            }
            TransactionState::Failed { id, error } => {
                output.push_str("State:           Failed\n");
                output.push_str(&format!("Transaction ID:  {}\n", id));
                output.push_str(&format!("Error:           {}\n", error));
            }
        }

        output.push_str(&format!(
            "Autocommit:      {}\n",
            if self.autocommit { "on" } else { "off" }
        ));
        output.push_str(&format!(
            "Isolation:       {}\n",
            self.isolation_level.as_str()
        ));

        if let Some(duration) = self.duration_ms {
            output.push_str(&format!(
                "Duration:        {:.3} s\n",
                duration as f64 / 1000.0
            ));
        }

        output.push_str(&format!("Queries:         {}\n", self.query_count));

        if !self.savepoints.is_empty() {
            output.push_str(&format!(
                "Savepoints:      {}\n",
                self.savepoints.join(", ")
            ));
        }

        output
    }
}
