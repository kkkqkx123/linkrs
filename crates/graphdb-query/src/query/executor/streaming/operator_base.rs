use std::sync::Arc;

use super::runtime::ExecutionRuntime;

#[derive(Debug)]
pub struct OperatorBase {
    pub plan_node_id: i64,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub opened: bool,
    /// Whether this operator produces global (merged) output.
    /// Local operators process one partition at a time.
    pub is_global: bool,
}

impl OperatorBase {
    pub fn new(plan_node_id: i64) -> Self {
        Self {
            plan_node_id,
            runtime: None,
            opened: false,
            is_global: false,
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

    pub fn ensure_not_cancelled(&self) -> Result<(), crate::core::error::QueryError> {
        if let Some(rt) = &self.runtime {
            rt.ensure_not_cancelled()
        } else {
            Ok(())
        }
    }

    pub fn record_profile_timing(&self, phase: &str, elapsed_us: u64) {
        if let Some(rt) = &self.runtime {
            use crate::query::executor::streaming::runtime::OperatorProfile;
            let name = "unknown";
            let mut profile = rt.profile().lock();
            let entry = profile
                .operators
                .entry(self.plan_node_id)
                .or_insert_with(|| OperatorProfile {
                    node_id: self.plan_node_id,
                    name: name.to_string(),
                    ..OperatorProfile::default()
                });
            match phase {
                "open" => entry.open_time_us += elapsed_us,
                "next" => entry.next_time_us += elapsed_us,
                "close" => entry.close_time_us += elapsed_us,
                _ => {}
            }
        }
    }

    pub fn record_profile_rows(&self, count: u64) {
        if let Some(rt) = &self.runtime {
            let mut profile = rt.profile().lock();
            if let Some(entry) = profile.operators.get_mut(&self.plan_node_id) {
                entry.output_rows += count;
            }
            profile.add_rows(count);
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
