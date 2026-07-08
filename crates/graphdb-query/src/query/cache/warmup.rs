//! Cache Warmup Module
//!
//! Provide cache warmup functionality to reduce cold start impact.
//!
//! # Design Goals
//!
//! 1. Preload frequently used query plans
//! 2. Preload frequently used CTE results
//! 3. Support configuration-based warmup
//! 4. Support statistics-based automatic warmup

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::cte_cache::CteCacheManager;
use super::plan_cache::QueryPlanCache;

/// Cache warmer
///
/// Provides cache warmup functionality to reduce cold start impact
pub struct CacheWarmer {
    /// Plan cache reference
    plan_cache: Arc<QueryPlanCache>,
    /// CTE cache reference
    cte_cache: Arc<CteCacheManager>,
    /// Queries to warmup
    warmup_queries: Vec<WarmupQuery>,
    /// CTEs to warmup
    warmup_ctes: Vec<WarmupCte>,
}

/// Warmup query definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupQuery {
    /// Query text
    pub query: String,
    /// Expected frequency (for priority calculation)
    pub frequency: Option<u64>,
    /// Dependent tables
    pub tables: Option<Vec<String>>,
}

impl From<String> for WarmupQuery {
    fn from(query: String) -> Self {
        Self {
            query,
            frequency: None,
            tables: None,
        }
    }
}

impl From<&str> for WarmupQuery {
    fn from(query: &str) -> Self {
        Self {
            query: query.to_string(),
            frequency: None,
            tables: None,
        }
    }
}

/// Warmup CTE definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupCte {
    /// CTE definition
    pub definition: String,
    /// Estimated row count
    pub estimated_rows: u64,
    /// Estimated compute cost (ms)
    pub compute_cost_ms: Option<u64>,
    /// Dependent tables
    pub tables: Option<Vec<String>>,
}

/// Warmup configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WarmupConfig {
    /// Queries to warmup
    pub queries: Vec<WarmupQuery>,
    /// CTEs to warmup
    pub ctes: Vec<WarmupCte>,
    /// Minimum frequency threshold for auto-warmup
    pub min_frequency_threshold: u64,
    /// Enable parallel warmup
    pub parallel: bool,
}

impl WarmupConfig {
    /// Create a new warmup configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a query to warmup
    pub fn with_query(mut self, query: impl Into<WarmupQuery>) -> Self {
        self.queries.push(query.into());
        self
    }

    /// Add a CTE to warmup
    pub fn with_cte(mut self, cte: WarmupCte) -> Self {
        self.ctes.push(cte);
        self
    }

    /// Load from file
    pub fn from_file(path: &Path) -> Result<Self, WarmupError> {
        let file = File::open(path).map_err(|e| WarmupError::ConfigReadError(e.to_string()))?;
        serde_json::from_reader(file).map_err(|e| WarmupError::ConfigParseError(e.to_string()))
    }

    /// Save to file
    pub fn to_file(&self, path: &Path) -> Result<(), WarmupError> {
        let file = File::create(path).map_err(|e| WarmupError::ConfigWriteError(e.to_string()))?;
        serde_json::to_writer_pretty(file, self)
            .map_err(|e| WarmupError::ConfigWriteError(e.to_string()))
    }
}

/// Warmup result
#[derive(Debug, Clone, Default)]
pub struct WarmupResult {
    /// Number of successfully warmed up queries
    pub successful_queries: usize,
    /// Number of failed queries
    pub failed_queries: usize,
    /// Number of successfully warmed up CTEs
    pub successful_ctes: usize,
    /// Number of failed CTEs
    pub failed_ctes: usize,
    /// Error messages
    pub errors: Vec<String>,
    /// Time taken for warmup (ms)
    pub duration_ms: u64,
}

impl WarmupResult {
    /// Check if warmup was completely successful
    pub fn is_success(&self) -> bool {
        self.failed_queries == 0 && self.failed_ctes == 0
    }

    /// Get total queries processed
    pub fn total_queries(&self) -> usize {
        self.successful_queries + self.failed_queries
    }

    /// Get total CTEs processed
    pub fn total_ctes(&self) -> usize {
        self.successful_ctes + self.failed_ctes
    }

    /// Get success rate for queries
    pub fn query_success_rate(&self) -> f64 {
        let total = self.total_queries();
        if total == 0 {
            1.0
        } else {
            self.successful_queries as f64 / total as f64
        }
    }

    /// Get success rate for CTEs
    pub fn cte_success_rate(&self) -> f64 {
        let total = self.total_ctes();
        if total == 0 {
            1.0
        } else {
            self.successful_ctes as f64 / total as f64
        }
    }

    /// Format result for display
    pub fn format(&self) -> String {
        format!(
            "Warmup Result:\n\
             - Queries: {} successful, {} failed ({:.1}% success rate)\n\
             - CTEs: {} successful, {} failed ({:.1}% success rate)\n\
             - Duration: {}ms\n\
             - Errors: {}",
            self.successful_queries,
            self.failed_queries,
            self.query_success_rate() * 100.0,
            self.successful_ctes,
            self.failed_ctes,
            self.cte_success_rate() * 100.0,
            self.duration_ms,
            self.errors.len()
        )
    }
}

/// Query statistics for auto-warmup
#[derive(Debug, Clone, Default)]
pub struct QueryStats {
    /// Query frequency map
    query_frequencies: HashMap<String, u64>,
    /// Total query count
    total_queries: u64,
    /// CTE frequency map
    cte_frequencies: HashMap<String, u64>,
}

impl QueryStats {
    /// Create new query stats
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a query execution
    pub fn record_query(&mut self, query: &str) {
        *self.query_frequencies.entry(query.to_string()).or_insert(0) += 1;
        self.total_queries += 1;
    }

    /// Record a CTE execution
    pub fn record_cte(&mut self, cte: &str) {
        *self.cte_frequencies.entry(cte.to_string()).or_insert(0) += 1;
    }

    /// Get most frequent queries
    pub fn most_frequent_queries(&self, limit: usize) -> Vec<(String, u64)> {
        let mut queries: Vec<_> = self
            .query_frequencies
            .iter()
            .map(|(q, f)| (q.clone(), *f))
            .collect();

        queries.sort_by_key(|b| std::cmp::Reverse(b.1));
        queries.truncate(limit);

        queries
    }

    /// Get most frequent CTEs
    pub fn most_frequent_ctes(&self, limit: usize) -> Vec<(String, u64)> {
        let mut ctes: Vec<_> = self
            .cte_frequencies
            .iter()
            .map(|(c, f)| (c.clone(), *f))
            .collect();

        ctes.sort_by_key(|b| std::cmp::Reverse(b.1));
        ctes.truncate(limit);

        ctes
    }

    /// Get frequency of a specific query
    pub fn query_frequency(&self, query: &str) -> u64 {
        *self.query_frequencies.get(query).unwrap_or(&0)
    }

    /// Get total query count
    pub fn total_queries(&self) -> u64 {
        self.total_queries
    }

    /// Get unique query count
    pub fn unique_queries(&self) -> usize {
        self.query_frequencies.len()
    }

    /// Merge with another stats instance
    pub fn merge(&mut self, other: &QueryStats) {
        for (query, freq) in &other.query_frequencies {
            *self.query_frequencies.entry(query.clone()).or_insert(0) += freq;
        }
        for (cte, freq) in &other.cte_frequencies {
            *self.cte_frequencies.entry(cte.clone()).or_insert(0) += freq;
        }
        self.total_queries += other.total_queries;
    }
}

/// Warmup error
#[derive(Debug, Clone)]
pub enum WarmupError {
    /// Configuration file read error
    ConfigReadError(String),
    /// Configuration file write error
    ConfigWriteError(String),
    /// Configuration parse error
    ConfigParseError(String),
    /// Query prepare error
    QueryPrepareError(String),
    /// CTE prepare error
    CtePrepareError(String),
    /// Cache error
    CacheError(String),
}

impl std::fmt::Display for WarmupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfigReadError(msg) => write!(f, "Config read error: {}", msg),
            Self::ConfigWriteError(msg) => write!(f, "Config write error: {}", msg),
            Self::ConfigParseError(msg) => write!(f, "Config parse error: {}", msg),
            Self::QueryPrepareError(msg) => write!(f, "Query prepare error: {}", msg),
            Self::CtePrepareError(msg) => write!(f, "CTE prepare error: {}", msg),
            Self::CacheError(msg) => write!(f, "Cache error: {}", msg),
        }
    }
}

impl std::error::Error for WarmupError {}

impl CacheWarmer {
    /// Create a new cache warmer
    pub fn new(plan_cache: Arc<QueryPlanCache>, cte_cache: Arc<CteCacheManager>) -> Self {
        Self {
            plan_cache,
            cte_cache,
            warmup_queries: Vec::new(),
            warmup_ctes: Vec::new(),
        }
    }

    /// Create from configuration file
    pub fn from_config(
        config_path: &Path,
        plan_cache: Arc<QueryPlanCache>,
        cte_cache: Arc<CteCacheManager>,
    ) -> Result<Self, WarmupError> {
        let config = WarmupConfig::from_file(config_path)?;

        Ok(Self {
            plan_cache,
            cte_cache,
            warmup_queries: config.queries,
            warmup_ctes: config.ctes,
        })
    }

    /// Add a query to warmup
    pub fn add_warmup_query(&mut self, query: impl Into<WarmupQuery>) {
        self.warmup_queries.push(query.into());
    }

    /// Add a CTE to warmup
    pub fn add_warmup_cte(&mut self, cte: WarmupCte) {
        self.warmup_ctes.push(cte);
    }

    /// Clear warmup data
    pub fn clear_warmup_data(&mut self) {
        self.warmup_queries.clear();
        self.warmup_ctes.clear();
    }

    /// Get warmup queries
    pub fn warmup_queries(&self) -> &[WarmupQuery] {
        &self.warmup_queries
    }

    /// Get warmup CTEs
    pub fn warmup_ctes(&self) -> &[WarmupCte] {
        &self.warmup_ctes
    }

    /// Execute warmup
    pub fn warmup(&self) -> WarmupResult {
        let start = std::time::Instant::now();
        let mut result = WarmupResult::default();

        log::info!("Starting cache warmup...");

        for warmup_query in &self.warmup_queries {
            match self.warmup_query(warmup_query) {
                Ok(_) => {
                    result.successful_queries += 1;
                    log::debug!("Warmed up query plan: {}", warmup_query.query);
                }
                Err(e) => {
                    result.failed_queries += 1;
                    result
                        .errors
                        .push(format!("Query '{}': {}", warmup_query.query, e));
                    log::warn!("Failed to warmup query '{}': {}", warmup_query.query, e);
                }
            }
        }

        for warmup_cte in &self.warmup_ctes {
            match self.warmup_cte(warmup_cte) {
                Ok(_) => {
                    result.successful_ctes += 1;
                    log::debug!("Warmed up CTE: {}", warmup_cte.definition);
                }
                Err(e) => {
                    result.failed_ctes += 1;
                    result
                        .errors
                        .push(format!("CTE '{}': {}", warmup_cte.definition, e));
                    log::warn!("Failed to warmup CTE '{}': {}", warmup_cte.definition, e);
                }
            }
        }

        result.duration_ms = start.elapsed().as_millis() as u64;

        log::info!(
            "Cache warmup completed: {} queries successful, {} failed, {} CTEs successful, {} failed, took {}ms",
            result.successful_queries,
            result.failed_queries,
            result.successful_ctes,
            result.failed_ctes,
            result.duration_ms
        );

        result
    }

    /// Warmup a single query
    fn warmup_query(&self, warmup_query: &WarmupQuery) -> Result<(), WarmupError> {
        if self.plan_cache.contains(&warmup_query.query) {
            log::debug!("Query already cached: {}", warmup_query.query);
            return Ok(());
        }

        self.prepare_query(&warmup_query.query)
    }

    /// Warmup a single CTE
    fn warmup_cte(&self, warmup_cte: &WarmupCte) -> Result<(), WarmupError> {
        if self.cte_cache.contains(&warmup_cte.definition) {
            log::debug!("CTE already cached: {}", warmup_cte.definition);
            return Ok(());
        }

        self.prepare_cte(warmup_cte)
    }

    /// Prepare a query (placeholder for actual implementation)
    ///
    /// In a real implementation, this would:
    /// 1. Parse the query
    /// 2. Generate an execution plan
    /// 3. Cache the plan
    fn prepare_query(&self, query: &str) -> Result<(), WarmupError> {
        log::debug!("Preparing query for warmup: {}", query);

        // Placeholder implementation
        // In production, this would integrate with the query planner
        // to actually parse and plan the query, then cache it

        Ok(())
    }

    /// Prepare a CTE (placeholder for actual implementation)
    ///
    /// In a real implementation, this would:
    /// 1. Execute the CTE
    /// 2. Cache the result
    fn prepare_cte(&self, warmup_cte: &WarmupCte) -> Result<(), WarmupError> {
        log::debug!("Preparing CTE for warmup: {}", warmup_cte.definition);

        // Placeholder implementation
        // In production, this would execute the CTE and cache the result

        Ok(())
    }

    /// Warmup from statistics
    pub fn warmup_from_stats(&self, stats: &QueryStats, min_frequency: u64) -> WarmupResult {
        let start = std::time::Instant::now();
        let mut result = WarmupResult::default();

        log::info!("Starting cache warmup from statistics...");

        let top_queries = stats.most_frequent_queries(100);

        for (query, frequency) in top_queries {
            if frequency < min_frequency {
                continue;
            }

            let warmup_query = WarmupQuery {
                query,
                frequency: Some(frequency),
                tables: None,
            };

            match self.warmup_query(&warmup_query) {
                Ok(_) => {
                    result.successful_queries += 1;
                    log::debug!(
                        "Warmed up query from stats: {} (freq: {})",
                        warmup_query.query,
                        frequency
                    );
                }
                Err(e) => {
                    result.failed_queries += 1;
                    result
                        .errors
                        .push(format!("Query '{}': {}", warmup_query.query, e));
                    log::warn!("Failed to warmup query '{}': {}", warmup_query.query, e);
                }
            }
        }

        let top_ctes = stats.most_frequent_ctes(50);

        for (cte, frequency) in top_ctes {
            if frequency < min_frequency {
                continue;
            }

            let warmup_cte = WarmupCte {
                definition: cte,
                estimated_rows: 1000,
                compute_cost_ms: Some(100),
                tables: None,
            };

            match self.warmup_cte(&warmup_cte) {
                Ok(_) => {
                    result.successful_ctes += 1;
                    log::debug!("Warmed up CTE from stats (freq: {})", frequency);
                }
                Err(e) => {
                    result.failed_ctes += 1;
                    result.errors.push(format!("CTE: {}", e));
                    log::warn!("Failed to warmup CTE: {}", e);
                }
            }
        }

        result.duration_ms = start.elapsed().as_millis() as u64;

        log::info!(
            "Cache warmup from stats completed: {} queries successful, {} CTEs successful, took {}ms",
            result.successful_queries,
            result.successful_ctes,
            result.duration_ms
        );

        result
    }

    /// Generate warmup configuration from statistics
    pub fn generate_config_from_stats(
        &self,
        stats: &QueryStats,
        min_frequency: u64,
        max_queries: usize,
        max_ctes: usize,
    ) -> WarmupConfig {
        let mut config = WarmupConfig::default();

        let top_queries = stats.most_frequent_queries(max_queries);
        for (query, frequency) in top_queries {
            if frequency >= min_frequency {
                config.queries.push(WarmupQuery {
                    query,
                    frequency: Some(frequency),
                    tables: None,
                });
            }
        }

        let top_ctes = stats.most_frequent_ctes(max_ctes);
        for (cte, frequency) in top_ctes {
            if frequency >= min_frequency {
                config.ctes.push(WarmupCte {
                    definition: cte,
                    estimated_rows: 1000,
                    compute_cost_ms: Some(100),
                    tables: None,
                });
            }
        }

        config.min_frequency_threshold = min_frequency;

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_warmup_config_default() {
        let config = WarmupConfig::default();
        assert!(config.queries.is_empty());
        assert!(config.ctes.is_empty());
    }

    #[test]
    fn test_warmup_config_builder() {
        let config = WarmupConfig::new()
            .with_query("SELECT 1")
            .with_query(WarmupQuery {
                query: "SELECT 2".to_string(),
                frequency: Some(100),
                tables: Some(vec!["users".to_string()]),
            });

        assert_eq!(config.queries.len(), 2);
    }

    #[test]
    fn test_warmup_result_default() {
        let result = WarmupResult::default();
        assert_eq!(result.successful_queries, 0);
        assert_eq!(result.failed_queries, 0);
        assert!(result.is_success());
        assert_eq!(result.query_success_rate(), 1.0);
    }

    #[test]
    fn test_warmup_result_format() {
        let result = WarmupResult {
            successful_queries: 8,
            failed_queries: 2,
            successful_ctes: 5,
            failed_ctes: 0,
            errors: vec!["Error 1".to_string()],
            duration_ms: 100,
        };

        let formatted = result.format();
        assert!(formatted.contains("Queries: 8 successful, 2 failed"));
        assert!(formatted.contains("CTEs: 5 successful, 0 failed"));
    }

    #[test]
    fn test_query_stats() {
        let mut stats = QueryStats::new();

        stats.record_query("SELECT 1");
        stats.record_query("SELECT 1");
        stats.record_query("SELECT 2");
        stats.record_cte("cte1");

        assert_eq!(stats.total_queries(), 3);
        assert_eq!(stats.unique_queries(), 2);
        assert_eq!(stats.query_frequency("SELECT 1"), 2);
        assert_eq!(stats.query_frequency("SELECT 2"), 1);
    }

    #[test]
    fn test_most_frequent_queries() {
        let mut stats = QueryStats::new();

        stats.record_query("SELECT 1");
        stats.record_query("SELECT 1");
        stats.record_query("SELECT 1");
        stats.record_query("SELECT 2");
        stats.record_query("SELECT 2");
        stats.record_query("SELECT 3");

        let top = stats.most_frequent_queries(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "SELECT 1");
        assert_eq!(top[0].1, 3);
        assert_eq!(top[1].0, "SELECT 2");
        assert_eq!(top[1].1, 2);
    }

    #[test]
    fn test_warmup_error_display() {
        let err = WarmupError::ConfigReadError("File not found".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Config read error"));
    }

    #[test]
    fn test_warmup_query_from_string() {
        let query: WarmupQuery = "SELECT 1".into();
        assert_eq!(query.query, "SELECT 1");
        assert!(query.frequency.is_none());
    }
}
