use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionStats {
    pub total_time_ms: u64,
    pub planning_time_ms: u64,
    pub execution_time_ms: u64,
    pub rows_scanned: u64,
    pub rows_returned: usize,
    pub memory_used_bytes: Option<u64>,
    pub index_hits: Option<u64>,
    pub cache_hits: Option<u64>,
    pub cache_misses: Option<u64>,
}

impl ExecutionStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_total_time(mut self, ms: u64) -> Self {
        self.total_time_ms = ms;
        self
    }

    pub fn with_planning_time(mut self, ms: u64) -> Self {
        self.planning_time_ms = ms;
        self
    }

    pub fn with_execution_time(mut self, ms: u64) -> Self {
        self.execution_time_ms = ms;
        self
    }

    pub fn with_rows_scanned(mut self, rows: u64) -> Self {
        self.rows_scanned = rows;
        self
    }

    pub fn with_rows_returned(mut self, rows: usize) -> Self {
        self.rows_returned = rows;
        self
    }

    pub fn format_summary(&self) -> String {
        let mut output = String::new();

        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str("Execution Statistics\n");
        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str(&format!(
            "Total Time:      {:.3} ms\n",
            self.total_time_ms as f64
        ));

        if self.planning_time_ms > 0 {
            output.push_str(&format!(
                "Planning Time:   {:.3} ms\n",
                self.planning_time_ms as f64
            ));
        }

        output.push_str(&format!(
            "Execution Time:  {:.3} ms\n",
            self.execution_time_ms as f64
        ));
        output.push_str(&format!("Rows Scanned:    {}\n", self.rows_scanned));
        output.push_str(&format!("Rows Returned:   {}\n", self.rows_returned));

        if let Some(mem) = self.memory_used_bytes {
            output.push_str(&format!(
                "Memory Used:     {:.2} MB\n",
                mem as f64 / 1024.0 / 1024.0
            ));
        }

        if let Some(hits) = self.index_hits {
            output.push_str(&format!("Index Hits:      {}\n", hits));
        }

        if let (Some(hits), Some(misses)) = (self.cache_hits, self.cache_misses) {
            let total = hits + misses;
            let hit_rate = if total > 0 {
                hits as f64 / total as f64 * 100.0
            } else {
                0.0
            };
            output.push_str(&format!("Cache Hit Rate:  {:.1}%\n", hit_rate));
        }

        output
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileResult {
    pub query: String,
    pub plan: crate::analysis::explain::QueryPlan,
    pub stats: ExecutionStats,
    pub warnings: Vec<String>,
}

impl ProfileResult {
    pub fn new(
        query: String,
        plan: crate::analysis::explain::QueryPlan,
        stats: ExecutionStats,
    ) -> Self {
        let mut result = Self {
            query,
            plan,
            stats,
            warnings: Vec::new(),
        };
        result.analyze_warnings();
        result
    }

    pub fn analyze_warnings(&mut self) {
        if self.stats.rows_scanned > 10000 && self.stats.rows_returned < 100 {
            self.warnings
                .push("High scan-to-result ratio. Consider adding an index.".to_string());
        }

        if self.stats.execution_time_ms > 1000 {
            self.warnings
                .push("Query execution time exceeds 1 second. Consider optimization.".to_string());
        }

        if let Some(misses) = self.stats.cache_misses {
            if misses > 1000 {
                self.warnings
                    .push("High cache miss count. Data may not be in memory.".to_string());
            }
        }

        if self.plan.max_depth() > 10 {
            self.warnings
                .push("Deep query plan. Consider simplifying the query.".to_string());
        }
    }

    pub fn format_report(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!("Query: {}\n\n", self.query));
        output.push_str("QUERY PLAN\n");
        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str(&self.plan.format_tree());
        output.push('\n');
        output.push_str(&self.stats.format_summary());

        if !self.warnings.is_empty() {
            output.push_str("\nWarnings:\n");
            for warning in &self.warnings {
                output.push_str(&format!("  ⚠ {}\n", warning));
            }
        }

        output
    }
}
