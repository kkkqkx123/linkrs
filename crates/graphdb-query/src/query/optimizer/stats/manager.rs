//! Statistical Information Manager Module
//!
//! Centralized management of all statistical information, with thread-safe access.

use dashmap::DashMap;
use std::sync::Arc;

use crate::core::types::Index;

use super::{EdgeTypeStatistics, PropertyCombinationStats, PropertyStatistics, TagStatistics};

/// Statistical Information Manager
///
/// Centralized management of all statistical information, ensuring thread-safe access.
/// All statistics are scoped to a space; keys are `"{space}.{name}"` composite keys.
#[derive(Debug)]
pub struct StatisticsManager {
    /// Tag statistics information, keyed by `"{space}.{tag_name}"`.
    tag_stats: Arc<DashMap<String, TagStatistics>>,
    /// Mapping from Tag ID to Tag Name
    tag_id_to_name: Arc<DashMap<i32, String>>,
    /// Type statistics information for edges, keyed by `"{space}.{edge_type}"`.
    edge_stats: Arc<DashMap<String, EdgeTypeStatistics>>,
    /// Attribute statistics information, keyed by `"{space}.{tag}.{property}"`.
    property_stats: Arc<DashMap<String, PropertyStatistics>>,
    /// Property combination statistics for GROUP BY cardinality estimation,
    /// keyed by `"{space}.{tag}.{prop1}.{prop2}..."`.
    property_combo_stats: Arc<DashMap<String, PropertyCombinationStats>>,
    /// Composite cache key `(schema_version, data_epoch)` at which each
    /// space's statistics were last collected.  A DDL bumps the schema
    /// generation; a committed write bumps the data epoch held by the storage
    /// engine.  Both must match for a cache hit.
    space_versions: Arc<DashMap<String, (u64, u64)>>,
    /// Index catalog for cost-based index selection, keyed by
    /// `"{space}.{tag_name}"` → (tag_id, available indexes). Registered
    /// per query from the planning metadata context.
    index_catalog: Arc<DashMap<String, (i32, Vec<Index>)>>,
}

impl StatisticsManager {
    /// Create a new statistical information manager.
    pub fn new() -> Self {
        Self {
            tag_stats: Arc::new(DashMap::new()),
            tag_id_to_name: Arc::new(DashMap::new()),
            edge_stats: Arc::new(DashMap::new()),
            property_stats: Arc::new(DashMap::new()),
            property_combo_stats: Arc::new(DashMap::new()),
            space_versions: Arc::new(DashMap::new()),
            index_catalog: Arc::new(DashMap::new()),
        }
    }

    /// Build the composite storage key for space-qualified statistics.
    fn composite_key(space: &str, name: &str) -> String {
        format!("{}.{}", space, name)
    }

    /// Split a composite `"{space}.{name}"` key back into the space, if it is one.
    fn split_composite_key<'a>(key: &'a str, name: &str) -> Option<&'a str> {
        key.strip_suffix(&format!(".{}", name))
    }

    /// Mark the statistics for `space` as stale (forces re-collection).
    pub fn mark_space_dirty(&self, space: &str) {
        self.space_versions.remove(space);
    }

    /// Invalidate statistics for a space (`None` = all spaces).
    pub fn invalidate_space(&self, space: Option<&str>) {
        match space {
            Some(space) => self.mark_space_dirty(space),
            None => self.space_versions.clear(),
        }
    }

    /// The composite `(schema_version, data_epoch)` stamp for `space`, if
    /// statistics have been collected for it.
    pub fn space_stamp(&self, space: &str) -> Option<(u64, u64)> {
        self.space_versions.get(space).map(|v| *v)
    }

    /// The schema version recorded for `space` at its last collection, if any.
    pub fn space_version(&self, space: &str) -> Option<u64> {
        self.space_stamp(space).map(|stamp| stamp.0)
    }

    /// Record the composite stamp at which `space` was last collected.
    pub fn set_space_stamp(&self, space: &str, schema_version: u64, data_epoch: u64) {
        self.space_versions
            .insert(space.to_string(), (schema_version, data_epoch));
    }

    /// Register the available indexes for `tag_name` in `space`.
    ///
    /// Called by the query pipeline when it builds the planning metadata
    /// context; registration overwrites any previously registered catalog
    /// for the tag, so schema changes are picked up on the next query.
    pub fn register_tag_indexes(
        &self,
        space: &str,
        tag_name: &str,
        tag_id: i32,
        indexes: Vec<Index>,
    ) {
        self.index_catalog
            .insert(Self::composite_key(space, tag_name), (tag_id, indexes));
    }

    /// The available indexes for `tag_name` in `space`, with the tag id.
    pub fn get_tag_indexes(&self, space: &str, tag_name: &str) -> Option<(i32, Vec<Index>)> {
        self.index_catalog
            .get(&Self::composite_key(space, tag_name))
            .map(|entry| entry.clone())
    }

    /// Mapping of registered tag IDs to their corresponding names
    pub fn register_tag_id(&self, tag_id: i32, tag_name: String) {
        self.tag_id_to_name.insert(tag_id, tag_name);
    }

    /// Retrieve the tag name based on the tag ID.
    pub fn get_tag_name_by_id(&self, tag_id: i32) -> Option<String> {
        self.tag_id_to_name.get(&tag_id).map(|v| v.clone())
    }

    /// Retrieve tag statistics based on the tag ID in a specific space.
    pub fn get_tag_stats_by_id(&self, space: &str, tag_id: i32) -> Option<TagStatistics> {
        let tag_name = self.get_tag_name_by_id(tag_id)?;
        self.get_tag_stats(space, &tag_name)
    }

    /// Get the number of vertices based on the tag ID in a specific space.
    pub fn get_vertex_count_by_id(&self, space: &str, tag_id: i32) -> u64 {
        self.get_tag_stats_by_id(space, tag_id)
            .map(|s| s.vertex_count)
            .unwrap_or(0)
    }

    /// Obtain tag statistics information for a specific space.
    pub fn get_tag_stats(&self, space: &str, tag_name: &str) -> Option<TagStatistics> {
        self.tag_stats
            .get(&Self::composite_key(space, tag_name))
            .map(|v| v.clone())
    }

    /// Update the tag statistics information for a specific space.
    pub fn update_tag_stats(&self, space: &str, stats: TagStatistics) {
        self.tag_stats
            .insert(Self::composite_key(space, &stats.tag_name), stats);
    }

    /// Obtain the number of vertices for a specific space.
    pub fn get_vertex_count(&self, space: &str, tag_name: &str) -> u64 {
        self.get_tag_stats(space, tag_name)
            .map(|s| s.vertex_count)
            .unwrap_or(0)
    }

    /// Obtain statistical information about the types of edges for a specific space.
    pub fn get_edge_stats(&self, space: &str, edge_type: &str) -> Option<EdgeTypeStatistics> {
        self.edge_stats
            .get(&Self::composite_key(space, edge_type))
            .map(|v| v.clone())
    }

    /// Update the statistics information on edge types for a specific space.
    pub fn update_edge_stats(&self, space: &str, stats: EdgeTypeStatistics) {
        self.edge_stats
            .insert(Self::composite_key(space, &stats.edge_type), stats);
    }

    /// Obtain the number of edges for a specific space.
    pub fn get_edge_count(&self, space: &str, edge_type: &str) -> u64 {
        self.get_edge_stats(space, edge_type)
            .map(|s| s.edge_count)
            .unwrap_or(0)
    }

    /// Obtain attribute statistics information for a specific space.
    pub fn get_property_stats(
        &self,
        space: &str,
        tag_name: Option<&str>,
        property_name: &str,
    ) -> Option<PropertyStatistics> {
        let key = match tag_name {
            Some(tag) => format!("{}.{}.{}", space, tag, property_name),
            None => format!("{}.{}", space, property_name),
        };
        self.property_stats.get(&key).map(|v| v.clone())
    }

    /// Update attribute statistics information for a specific space.
    pub fn update_property_stats(&self, space: &str, stats: PropertyStatistics) {
        let key = match &stats.tag_name {
            Some(tag) => format!("{}.{}.{}", space, tag, stats.property_name),
            None => format!("{}.{}", space, stats.property_name),
        };
        self.property_stats.insert(key, stats);
    }

    /// Clear all statistical information.
    pub fn clear_all(&self) {
        self.tag_stats.clear();
        self.tag_id_to_name.clear();
        self.edge_stats.clear();
        self.property_stats.clear();
        self.property_combo_stats.clear();
        self.space_versions.clear();
        self.index_catalog.clear();
    }

    /// Get property combination statistics for GROUP BY cardinality estimation.
    pub fn get_property_combo_stats(
        &self,
        space: &str,
        tag_name: &str,
        properties: &[String],
    ) -> Option<PropertyCombinationStats> {
        let key = format!("{}.{}.{}", space, tag_name, properties.join("."));
        self.property_combo_stats.get(&key).map(|v| v.clone())
    }

    /// Update property combination statistics for a specific space.
    pub fn update_property_combo_stats(&self, space: &str, stats: PropertyCombinationStats) {
        let tag = stats.tag_name.as_deref().unwrap_or("");
        let key = format!("{}.{}.{}", space, tag, stats.properties.join("."));
        self.property_combo_stats.insert(key, stats);
    }

    /// Get combined cardinality for a set of properties.
    /// Returns None if no statistics are available.
    pub fn get_combined_cardinality(
        &self,
        space: &str,
        tag_name: Option<&str>,
        properties: &[String],
    ) -> Option<u64> {
        let tag = tag_name?;
        self.get_property_combo_stats(space, tag, properties)
            .map(|s| s.estimated_cardinality())
    }

    /// Retrieve all tag names.
    ///
    /// Returns bare tag names; space-qualified entries are de-duplicated.
    pub fn get_all_tags(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for entry in self.tag_stats.iter() {
            let name = Self::split_composite_key(entry.key(), &entry.tag_name)
                .map(|_| entry.tag_name.clone())
                .unwrap_or_else(|| entry.key().clone());
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// Obtain the names of all edge types (space-qualified entries de-duplicated).
    pub fn get_all_edge_types(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for entry in self.edge_stats.iter() {
            let name = Self::split_composite_key(entry.key(), &entry.edge_type)
                .map(|_| entry.edge_type.clone())
                .unwrap_or_else(|| entry.key().clone());
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }
}

impl Default for StatisticsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for StatisticsManager {
    fn clone(&self) -> Self {
        Self {
            tag_stats: Arc::clone(&self.tag_stats),
            tag_id_to_name: Arc::clone(&self.tag_id_to_name),
            edge_stats: Arc::clone(&self.edge_stats),
            property_stats: Arc::clone(&self.property_stats),
            property_combo_stats: Arc::clone(&self.property_combo_stats),
            space_versions: Arc::clone(&self.space_versions),
            index_catalog: Arc::clone(&self.index_catalog),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::stats::edge::EdgeTypeStatistics;
    use crate::query::optimizer::stats::property::PropertyStatistics;

    const TEST_SPACE: &str = "test";

    #[test]
    fn test_statistics_manager_creation() {
        let manager = StatisticsManager::new();
        assert_eq!(manager.get_all_tags().len(), 0);
        assert_eq!(manager.get_all_edge_types().len(), 0);
    }

    #[test]
    fn test_statistics_manager_default() {
        let manager = StatisticsManager::default();
        assert_eq!(manager.get_all_tags().len(), 0);
    }

    #[test]
    fn test_register_and_get_tag_id() {
        let manager = StatisticsManager::new();
        manager.register_tag_id(1, "person".to_string());

        assert_eq!(manager.get_tag_name_by_id(1), Some("person".to_string()));
        assert_eq!(manager.get_tag_name_by_id(2), None);
    }

    #[test]
    fn test_add_tag_statistics() {
        let manager = StatisticsManager::new();
        let mut stats = TagStatistics::new("person".to_string());
        stats.vertex_count = 1000;
        stats.avg_out_degree = 5.5;
        stats.avg_in_degree = 3.2;

        manager.update_tag_stats(TEST_SPACE, stats.clone());

        let retrieved = manager
            .get_tag_stats(TEST_SPACE, "person")
            .expect("Tag stats not found");
        assert_eq!(retrieved.vertex_count, 1000);
        assert_eq!(retrieved.avg_out_degree, 5.5);
        assert_eq!(retrieved.avg_in_degree, 3.2);
    }

    #[test]
    fn test_get_vertex_count() {
        let manager = StatisticsManager::new();
        let mut stats = TagStatistics::new("company".to_string());
        stats.vertex_count = 500;
        manager.update_tag_stats(TEST_SPACE, stats);

        assert_eq!(manager.get_vertex_count(TEST_SPACE, "company"), 500);
        assert_eq!(manager.get_vertex_count(TEST_SPACE, "nonexistent"), 0);
    }

    #[test]
    fn test_get_tag_stats_by_id() {
        let manager = StatisticsManager::new();
        manager.register_tag_id(10, "product".to_string());

        let mut stats = TagStatistics::new("product".to_string());
        stats.vertex_count = 2000;
        manager.update_tag_stats(TEST_SPACE, stats);

        let retrieved = manager
            .get_tag_stats_by_id(TEST_SPACE, 10)
            .expect("Tag stats not found");
        assert_eq!(retrieved.vertex_count, 2000);
    }

    #[test]
    fn test_get_vertex_count_by_id() {
        let manager = StatisticsManager::new();
        manager.register_tag_id(5, "category".to_string());

        let mut stats = TagStatistics::new("category".to_string());
        stats.vertex_count = 100;
        manager.update_tag_stats(TEST_SPACE, stats);

        assert_eq!(manager.get_vertex_count_by_id(TEST_SPACE, 5), 100);
        assert_eq!(manager.get_vertex_count_by_id(TEST_SPACE, 999), 0);
    }

    #[test]
    fn test_add_edge_statistics() {
        let manager = StatisticsManager::new();
        let mut edge_stats = EdgeTypeStatistics::new("follows".to_string());
        edge_stats.edge_count = 5000;

        manager.update_edge_stats(TEST_SPACE, edge_stats.clone());

        let retrieved = manager
            .get_edge_stats(TEST_SPACE, "follows")
            .expect("Edge stats not found");
        assert_eq!(retrieved.edge_count, 5000);
    }

    #[test]
    fn test_get_edge_count() {
        let manager = StatisticsManager::new();
        let mut edge_stats = EdgeTypeStatistics::new("works_at".to_string());
        edge_stats.edge_count = 3000;
        manager.update_edge_stats(TEST_SPACE, edge_stats);

        assert_eq!(manager.get_edge_count(TEST_SPACE, "works_at"), 3000);
        assert_eq!(manager.get_edge_count(TEST_SPACE, "nonexistent"), 0);
    }

    #[test]
    fn test_add_property_statistics() {
        let manager = StatisticsManager::new();
        let mut prop_stats = PropertyStatistics::new("age".to_string(), Some("person".to_string()));
        prop_stats.distinct_values = 100;

        manager.update_property_stats(TEST_SPACE, prop_stats);

        let retrieved = manager
            .get_property_stats(TEST_SPACE, Some("person"), "age")
            .expect("Property stats not found");
        assert_eq!(retrieved.distinct_values, 100);
    }

    #[test]
    fn test_multiple_tags_statistics() {
        let manager = StatisticsManager::new();

        let mut person_stats = TagStatistics::new("person".to_string());
        person_stats.vertex_count = 1000;
        manager.update_tag_stats(TEST_SPACE, person_stats);

        let mut company_stats = TagStatistics::new("company".to_string());
        company_stats.vertex_count = 500;
        manager.update_tag_stats(TEST_SPACE, company_stats);

        assert_eq!(manager.get_vertex_count(TEST_SPACE, "person"), 1000);
        assert_eq!(manager.get_vertex_count(TEST_SPACE, "company"), 500);

        let all_tags = manager.get_all_tags();
        assert_eq!(all_tags.len(), 2);
        assert!(all_tags.contains(&"person".to_string()));
        assert!(all_tags.contains(&"company".to_string()));
    }

    #[test]
    fn test_statistics_update_overwrite() {
        let manager = StatisticsManager::new();

        let mut stats1 = TagStatistics::new("person".to_string());
        stats1.vertex_count = 1000;
        manager.update_tag_stats(TEST_SPACE, stats1);

        let mut stats2 = TagStatistics::new("person".to_string());
        stats2.vertex_count = 2000;
        manager.update_tag_stats(TEST_SPACE, stats2);

        assert_eq!(manager.get_vertex_count(TEST_SPACE, "person"), 2000);
    }

    #[test]
    fn test_clear_all_statistics() {
        let manager = StatisticsManager::new();

        let mut person_stats = TagStatistics::new("person".to_string());
        person_stats.vertex_count = 1000;
        manager.update_tag_stats(TEST_SPACE, person_stats);

        let mut edge_stats = EdgeTypeStatistics::new("follows".to_string());
        edge_stats.edge_count = 5000;
        manager.update_edge_stats(TEST_SPACE, edge_stats);

        assert_eq!(manager.get_all_tags().len(), 1);
        assert_eq!(manager.get_all_edge_types().len(), 1);

        manager.clear_all();

        assert_eq!(manager.get_all_tags().len(), 0);
        assert_eq!(manager.get_all_edge_types().len(), 0);
        assert_eq!(manager.get_vertex_count(TEST_SPACE, "person"), 0);
        assert_eq!(manager.get_edge_count(TEST_SPACE, "follows"), 0);
    }

    #[test]
    fn test_statistics_manager_clone() {
        let manager = StatisticsManager::new();

        let mut stats = TagStatistics::new("person".to_string());
        stats.vertex_count = 1000;
        manager.update_tag_stats(TEST_SPACE, stats);

        let cloned = manager.clone();
        assert_eq!(cloned.get_vertex_count(TEST_SPACE, "person"), 1000);

        let mut new_stats = TagStatistics::new("company".to_string());
        new_stats.vertex_count = 500;
        cloned.update_tag_stats(TEST_SPACE, new_stats);

        assert_eq!(manager.get_vertex_count(TEST_SPACE, "company"), 500);
    }

    #[test]
    fn test_property_combination_statistics() {
        let manager = StatisticsManager::new();
        let props = vec!["city".to_string(), "age".to_string()];
        let mut combo_stats =
            PropertyCombinationStats::new(String::new(), Some("person".to_string()), props.clone());
        combo_stats.combined_distinct_values = 50;

        manager.update_property_combo_stats(TEST_SPACE, combo_stats);

        let retrieved = manager
            .get_property_combo_stats(TEST_SPACE, "person", &props)
            .expect("Combo stats not found");
        assert_eq!(retrieved.combined_distinct_values, 50);
    }

    #[test]
    fn test_get_combined_cardinality() {
        let manager = StatisticsManager::new();
        let props = vec!["city".to_string(), "age".to_string()];
        let mut combo_stats =
            PropertyCombinationStats::new(String::new(), Some("person".to_string()), props.clone());
        combo_stats.combined_distinct_values = 75;

        manager.update_property_combo_stats(TEST_SPACE, combo_stats);

        let cardinality = manager
            .get_combined_cardinality(TEST_SPACE, Some("person"), &props)
            .expect("Combined cardinality not found");
        assert_eq!(cardinality, 75);
    }

    #[test]
    fn test_property_stats_without_tag() {
        let manager = StatisticsManager::new();
        let mut prop_stats = PropertyStatistics::new("global_prop".to_string(), None);
        prop_stats.distinct_values = 200;

        manager.update_property_stats(TEST_SPACE, prop_stats);

        let retrieved = manager
            .get_property_stats(TEST_SPACE, None, "global_prop")
            .expect("Property stats not found");
        assert_eq!(retrieved.distinct_values, 200);
    }

    #[test]
    fn test_multiple_edge_types() {
        let manager = StatisticsManager::new();

        let mut follows = EdgeTypeStatistics::new("follows".to_string());
        follows.edge_count = 5000;
        manager.update_edge_stats(TEST_SPACE, follows);

        let mut works_at = EdgeTypeStatistics::new("works_at".to_string());
        works_at.edge_count = 3000;
        manager.update_edge_stats(TEST_SPACE, works_at);

        assert_eq!(manager.get_edge_count(TEST_SPACE, "follows"), 5000);
        assert_eq!(manager.get_edge_count(TEST_SPACE, "works_at"), 3000);

        let all_edge_types = manager.get_all_edge_types();
        assert_eq!(all_edge_types.len(), 2);
        assert!(all_edge_types.contains(&"follows".to_string()));
        assert!(all_edge_types.contains(&"works_at".to_string()));
    }

    #[test]
    fn test_space_isolation() {
        let manager = StatisticsManager::new();

        let mut stats = TagStatistics::new("person".to_string());
        stats.vertex_count = 42;
        manager.update_tag_stats(
            "basketball",
            stats.with_version("basketball".to_string(), 7),
        );

        let mut edge_stats = EdgeTypeStatistics::new("follow".to_string());
        edge_stats.edge_count = 99;
        manager.update_edge_stats(
            "basketball",
            edge_stats.with_version("basketball".to_string(), 7),
        );

        assert_eq!(manager.get_vertex_count("basketball", "person"), 42);
        assert_eq!(manager.get_edge_count("basketball", "follow"), 99);
        let tag_stats = manager.get_tag_stats("basketball", "person").expect("tag");
        assert_eq!(tag_stats.schema_version, Some(7));
        assert_eq!(tag_stats.space.as_deref(), Some("basketball"));

        // Different spaces are isolated.
        assert_eq!(manager.get_vertex_count("tennis", "person"), 0);
        assert_eq!(manager.get_edge_count("tennis", "follow"), 0);
    }

    #[test]
    fn test_space_version_gate() {
        let manager = StatisticsManager::new();
        assert_eq!(manager.space_stamp("basketball"), None);

        manager.set_space_stamp("basketball", 3, 100);
        assert_eq!(manager.space_stamp("basketball"), Some((3, 100)));

        manager.mark_space_dirty("basketball");
        assert_eq!(manager.space_stamp("basketball"), None);

        manager.set_space_stamp("a", 1, 10);
        manager.set_space_stamp("b", 2, 20);
        manager.invalidate_space(None);
        assert_eq!(manager.space_stamp("a"), None);
        assert_eq!(manager.space_stamp("b"), None);
    }
}
