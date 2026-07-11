use std::sync::Arc;

use super::runtime::{ExecutionRuntime, OperatorProfileKey};

#[derive(Debug)]
pub struct OperatorBase {
    pub plan_node_id: i64,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub opened: bool,
    /// Whether this operator produces global (merged) output.
    /// Local operators process one partition at a time.
    pub is_global: bool,
    /// Local partition that owns this operator. `None` denotes a global or
    /// non-partitioned operator.
    pub partition_id: Option<usize>,
}

impl OperatorBase {
    pub fn new(plan_node_id: i64) -> Self {
        Self {
            plan_node_id,
            runtime: None,
            opened: false,
            is_global: false,
            partition_id: None,
        }
    }

    pub fn with_runtime(mut self, rt: Option<Arc<ExecutionRuntime>>) -> Self {
        self.runtime = rt;
        self
    }

    pub fn with_global(mut self, is_global: bool) -> Self {
        self.is_global = is_global;
        self
    }

    pub fn with_partition(mut self, partition_id: usize) -> Self {
        self.partition_id = Some(partition_id);
        self
    }

    pub fn profile_key(&self) -> OperatorProfileKey {
        OperatorProfileKey::new(self.plan_node_id, self.partition_id)
    }

    pub fn ensure_not_cancelled(&self) -> Result<(), crate::core::error::QueryError> {
        if let Some(rt) = &self.runtime {
            rt.ensure_not_cancelled()
        } else {
            Ok(())
        }
    }

    pub fn record_profile_rows(&self, count: u64) {
        if let Some(rt) = &self.runtime {
            let mut profile = rt.profile().lock();
            if let Some(entry) = profile.operators.get_mut(&self.profile_key()) {
                entry.output_rows += count;
            }
        }
    }

    pub fn register_resource<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if let Some(rt) = &self.runtime {
            rt.on_cleanup(f);
        }
    }
}
