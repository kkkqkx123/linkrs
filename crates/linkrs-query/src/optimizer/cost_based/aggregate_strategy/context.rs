//! Contextual information for the selection of aggregation strategies.

/// Contextual information for the selection of aggregation strategies
#[derive(Debug, Clone)]
pub struct AggregateContext {
    /// Number of input lines
    pub input_rows: u64,
    /// List of grouping keys
    pub group_keys: Vec<String>,
    /// Number of aggregate functions
    pub agg_function_count: usize,
    /// Memory limit (in bytes; 0 indicates no limit)
    pub memory_limit: u64,
    /// Are the input data already sorted?
    pub input_is_sorted: bool,
    /// Does the sorting key match the grouping key?
    pub sort_keys_match_group_keys: bool,
    /// Are aggregate expressions deterministic?
    pub is_deterministic: bool,
    /// Aggregation expression complexity score
    pub complexity_score: u32,
    /// Table name (tag name) for statistics lookup
    pub table_name: Option<String>,
}

impl AggregateContext {
    /// Create a new aggregation context.
    pub fn new(input_rows: u64, group_keys: Vec<String>, agg_function_count: usize) -> Self {
        Self {
            input_rows,
            group_keys,
            agg_function_count,
            memory_limit: 0,
            input_is_sorted: false,
            sort_keys_match_group_keys: false,
            is_deterministic: true,
            complexity_score: 0,
            table_name: None,
        }
    }

    /// Setting memory limits
    pub fn with_memory_limit(mut self, memory_limit: u64) -> Self {
        self.memory_limit = memory_limit;
        self
    }

    /// Set the input to be sorted.
    pub fn with_sorted_input(mut self, sort_keys_match: bool) -> Self {
        self.input_is_sorted = true;
        self.sort_keys_match_group_keys = sort_keys_match;
        self
    }

    /// Setting expression properties
    pub fn with_expression_analysis(
        mut self,
        is_deterministic: bool,
        complexity_score: u32,
    ) -> Self {
        self.is_deterministic = is_deterministic;
        self.complexity_score = complexity_score;
        self
    }

    /// Set table name for statistics lookup
    pub fn with_table_name(mut self, table_name: Option<String>) -> Self {
        self.table_name = table_name;
        self
    }
}
