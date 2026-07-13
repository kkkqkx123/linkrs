use serde::{Deserialize, Serialize};

/// Node description key-value pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pair {
    pub key: String,
    pub value: String,
}

impl Pair {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Branch information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanNodeBranchInfo {
    pub is_do_branch: bool,
    pub condition_node_id: i64,
}

impl PlanNodeBranchInfo {
    pub fn new(is_do_branch: bool, condition_node_id: i64) -> Self {
        Self {
            is_do_branch,
            condition_node_id,
        }
    }
}

/// Performance statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilingStats {
    pub rows: i64,
    pub exec_duration_in_us: i64,
    pub total_duration_in_us: i64,
    pub other_stats: std::collections::HashMap<String, String>,
}

impl ProfilingStats {
    pub fn new() -> Self {
        Self {
            rows: 0,
            exec_duration_in_us: 0,
            total_duration_in_us: 0,
            other_stats: std::collections::HashMap::new(),
        }
    }
}

impl Default for ProfilingStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Plan Node Description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanNodeDescription {
    pub name: String,
    pub id: i64,
    pub output_var: String,
    pub description: Option<Vec<Pair>>,
    pub profiles: Option<Vec<ProfilingStats>>,
    pub branch_info: Option<PlanNodeBranchInfo>,
    pub dependencies: Option<Vec<i64>>,
}

impl PlanNodeDescription {
    pub fn new(name: impl Into<String>, id: i64) -> Self {
        Self {
            name: name.into(),
            id,
            output_var: String::new(),
            description: None,
            profiles: None,
            branch_info: None,
            dependencies: None,
        }
    }

    pub fn with_output_var(mut self, output_var: impl Into<String>) -> Self {
        self.output_var = output_var.into();
        self
    }

    pub fn add_description(&mut self, key: impl Into<String>, value: impl Into<String>) {
        if self.description.is_none() {
            self.description = Some(Vec::new());
        }
        self.description
            .as_mut()
            .expect("description should be Some after initialization")
            .push(Pair::new(key, value));
    }

    pub fn with_description(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.add_description(key, value);
        self
    }

    pub fn set_dependencies(&mut self, deps: Vec<i64>) {
        self.dependencies = Some(deps);
    }

    pub fn with_dependencies(mut self, deps: Vec<i64>) -> Self {
        self.dependencies = Some(deps);
        self
    }

    pub fn set_branch_info(&mut self, branch_info: PlanNodeBranchInfo) {
        self.branch_info = Some(branch_info);
    }

    pub fn with_branch_info(mut self, branch_info: PlanNodeBranchInfo) -> Self {
        self.branch_info = Some(branch_info);
        self
    }

    pub fn add_profile(&mut self, profile: ProfilingStats) {
        if self.profiles.is_none() {
            self.profiles = Some(Vec::new());
        }
        self.profiles
            .as_mut()
            .expect("profiles should be Some after initialization")
            .push(profile);
    }
}

/// Plan Description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDescription {
    pub plan_node_descs: Vec<PlanNodeDescription>,
    pub node_index_map: std::collections::HashMap<i64, usize>,
    pub format: String,
    pub optimize_time_in_us: i64,
    pub partition_spec_description: Option<String>,

    // ── P8 parallel execution profile (populated at runtime) ──
    pub requested_workers: usize,
    pub actual_workers: usize,
    pub parallel_wall_time_us: u64,
    pub parallel_work_time_us: u64,
    pub parallel_buffered_chunks_peak: usize,
    pub parallel_buffered_bytes_peak: usize,
    pub parallel_fallback_reason: String,
}

impl PlanDescription {
    pub fn new() -> Self {
        Self {
            plan_node_descs: Vec::new(),
            node_index_map: std::collections::HashMap::new(),
            format: String::new(),
            optimize_time_in_us: 0,
            partition_spec_description: None,
            requested_workers: 1,
            actual_workers: 1,
            parallel_wall_time_us: 0,
            parallel_work_time_us: 0,
            parallel_buffered_chunks_peak: 0,
            parallel_buffered_bytes_peak: 0,
            parallel_fallback_reason: String::new(),
        }
    }

    pub fn add_node_desc(&mut self, desc: PlanNodeDescription) -> usize {
        let index = self.plan_node_descs.len();
        let node_id = desc.id;
        self.plan_node_descs.push(desc);
        self.node_index_map.insert(node_id, index);
        index
    }

    pub fn get_node_desc(&self, node_id: i64) -> Option<&PlanNodeDescription> {
        self.node_index_map
            .get(&node_id)
            .and_then(|&index| self.plan_node_descs.get(index))
    }

    pub fn get_node_desc_mut(&mut self, node_id: i64) -> Option<&mut PlanNodeDescription> {
        if let Some(&index) = self.node_index_map.get(&node_id) {
            self.plan_node_descs.get_mut(index)
        } else {
            None
        }
    }
}

impl Default for PlanDescription {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_description_p8_fields_defaults() {
        let desc = PlanDescription::new();
        assert_eq!(desc.requested_workers, 1);
        assert_eq!(desc.actual_workers, 1);
        assert_eq!(desc.parallel_wall_time_us, 0);
        assert_eq!(desc.parallel_work_time_us, 0);
        assert_eq!(desc.parallel_buffered_chunks_peak, 0);
        assert_eq!(desc.parallel_buffered_bytes_peak, 0);
        assert!(desc.parallel_fallback_reason.is_empty());
    }

    #[test]
    fn plan_description_p8_fields_round_trip_serialize() {
        let mut desc = PlanDescription::new();
        desc.requested_workers = 4;
        desc.actual_workers = 2;
        desc.parallel_wall_time_us = 1000;
        desc.parallel_work_time_us = 2000;
        desc.parallel_buffered_chunks_peak = 3;
        desc.parallel_buffered_bytes_peak = 8192;
        desc.parallel_fallback_reason = "test fallback".to_string();

        let json = serde_json::to_string(&desc).expect("serialize");
        let restored: PlanDescription = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.requested_workers, 4);
        assert_eq!(restored.actual_workers, 2);
        assert_eq!(restored.parallel_wall_time_us, 1000);
        assert_eq!(restored.parallel_work_time_us, 2000);
        assert_eq!(restored.parallel_buffered_chunks_peak, 3);
        assert_eq!(restored.parallel_buffered_bytes_peak, 8192);
        assert_eq!(restored.parallel_fallback_reason, "test fallback");
    }
}
