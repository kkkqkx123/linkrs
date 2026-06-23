//! Statistical Information Manager Module
//!
//! Centralized management of all statistical information, with thread-safe access.

use dashmap::DashMap;
use std::sync::Arc;

use super::{EdgeTypeStatistics, PropertyCombinationStats, PropertyStatistics, TagStatistics};

/// Statistical Information Manager
///
/// Centralized management of all statistical information, ensuring thread-safe access.
#[derive(Debug)]
pub struct StatisticsManager {
    /// Tag statistics information (with tag names as keys)
    tag_stats: Arc<DashMap<String, TagStatistics>>,
    /// Mapping from Tag ID to Tag Name
    tag_id_to_name: Arc<DashMap<i32, String>>,
    /// Type statistics information for edges
    edge_stats: Arc<DashMap<String, EdgeTypeStatistics>>,
    /// Attribute statistics information
    property_stats: Arc<DashMap<String, PropertyStatistics>>,
    /// Property combination statistics for GROUP BY cardinality estimation
    property_combo_stats: Arc<DashMap<String, PropertyCombinationStats>>,
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
        }
    }

    /// Mapping of registered tag IDs to their corresponding names
    pub fn register_tag_id(&self, tag_id: i32, tag_name: String) {
        self.tag_id_to_name.insert(tag_id, tag_name);
    }

    /// Retrieve the tag name based on the tag ID.
    pub fn get_tag_name_by_id(&self, tag_id: i32) -> Option<String> {
        self.tag_id_to_name.get(&tag_id).map(|v| v.clone())
    }

    /// Retrieve tag statistics based on the tag ID.
    pub fn get_tag_stats_by_id(&self, tag_id: i32) -> Option<TagStatistics> {
        let tag_name = self.get_tag_name_by_id(tag_id)?;
        self.get_tag_stats(&tag_name)
    }

    /// Get the number of vertices based on the tag ID.
    pub fn get_vertex_count_by_id(&self, tag_id: i32) -> u64 {
        self.get_tag_stats_by_id(tag_id)
            .map(|s| s.vertex_count)
            .unwrap_or(0)
    }

    /// Obtain tag statistics information
    pub fn get_tag_stats(&self, tag_name: &str) -> Option<TagStatistics> {
        self.tag_stats.get(tag_name).map(|v| v.clone())
    }

    /// Update the tag statistics information.
    pub fn update_tag_stats(&self, stats: TagStatistics) {
        self.tag_stats.insert(stats.tag_name.clone(), stats);
    }

    /// Obtain the number of vertices
    pub fn get_vertex_count(&self, tag_name: &str) -> u64 {
        self.get_tag_stats(tag_name)
            .map(|s| s.vertex_count)
            .unwrap_or(0)
    }

    /// Obtain statistical information about the types of edges.
    pub fn get_edge_stats(&self, edge_type: &str) -> Option<EdgeTypeStatistics> {
        self.edge_stats.get(edge_type).map(|v| v.clone())
    }

    /// Update the statistics information on edge types.
    pub fn update_edge_stats(&self, stats: EdgeTypeStatistics) {
        self.edge_stats.insert(stats.edge_type.clone(), stats);
    }

    /// Obtain the number of edges
    pub fn get_edge_count(&self, edge_type: &str) -> u64 {
        self.get_edge_stats(edge_type)
            .map(|s| s.edge_count)
            .unwrap_or(0)
    }

    /// Obtain attribute statistics information
    pub fn get_property_stats(
        &self,
        tag_name: Option<&str>,
        property_name: &str,
    ) -> Option<PropertyStatistics> {
        let key = match tag_name {
            Some(tag) => format!("{}.{}", tag, property_name),
            None => property_name.to_string(),
        };
        self.property_stats.get(&key).map(|v| v.clone())
    }

    /// Update attribute statistics information
    pub fn update_property_stats(&self, stats: PropertyStatistics) {
        let key = match &stats.tag_name {
            Some(tag) => format!("{}.{}", tag, stats.property_name),
            None => stats.property_name.clone(),
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
    }

    /// Get property combination statistics for GROUP BY cardinality estimation.
    pub fn get_property_combo_stats(
        &self,
        tag_name: &str,
        properties: &[String],
    ) -> Option<PropertyCombinationStats> {
        let key = format!("{}.{}", tag_name, properties.join("."));
        self.property_combo_stats.get(&key).map(|v| v.clone())
    }

    /// Update property combination statistics.
    pub fn update_property_combo_stats(&self, stats: PropertyCombinationStats) {
        self.property_combo_stats.insert(stats.key.clone(), stats);
    }

    /// Get combined cardinality for a set of properties.
    /// Returns None if no statistics are available.
    pub fn get_combined_cardinality(
        &self,
        tag_name: Option<&str>,
        properties: &[String],
    ) -> Option<u64> {
        let tag = tag_name?;
        self.get_property_combo_stats(tag, properties)
            .map(|s| s.estimated_cardinality())
    }

    /// Retrieve all tag names
    pub fn get_all_tags(&self) -> Vec<String> {
        self.tag_stats.iter().map(|k| k.key().clone()).collect()
    }

    /// Obtain the names of all edge types.
    pub fn get_all_edge_types(&self) -> Vec<String> {
        self.edge_stats.iter().map(|k| k.key().clone()).collect()
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::stats::edge::EdgeTypeStatistics;
    use crate::query::optimizer::stats::property::PropertyStatistics;

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

        assert_eq!(
            manager.get_tag_name_by_id(1),
            Some("person".to_string())
        );
        assert_eq!(manager.get_tag_name_by_id(2), None);
    }

    #[test]
    fn test_add_tag_statistics() {
        let manager = StatisticsManager::new();
        let mut stats = TagStatistics::new("person".to_string());
        stats.vertex_count = 1000;
        stats.avg_out_degree = 5.5;
        stats.avg_in_degree = 3.2;

        manager.update_tag_stats(stats.clone());

        let retrieved = manager.get_tag_stats("person").expect("Tag stats not found");
        assert_eq!(retrieved.vertex_count, 1000);
        assert_eq!(retrieved.avg_out_degree, 5.5);
        assert_eq!(retrieved.avg_in_degree, 3.2);
    }

    #[test]
    fn test_get_vertex_count() {
        let manager = StatisticsManager::new();
        let mut stats = TagStatistics::new("company".to_string());
        stats.vertex_count = 500;
        manager.update_tag_stats(stats);

        assert_eq!(manager.get_vertex_count("company"), 500);
        assert_eq!(manager.get_vertex_count("nonexistent"), 0);
    }

    #[test]
    fn test_get_tag_stats_by_id() {
        let manager = StatisticsManager::new();
        manager.register_tag_id(10, "product".to_string());

        let mut stats = TagStatistics::new("product".to_string());
        stats.vertex_count = 2000;
        manager.update_tag_stats(stats);

        let retrieved = manager.get_tag_stats_by_id(10).expect("Tag stats not found");
        assert_eq!(retrieved.vertex_count, 2000);
    }

    #[test]
    fn test_get_vertex_count_by_id() {
        let manager = StatisticsManager::new();
        manager.register_tag_id(5, "category".to_string());

        let mut stats = TagStatistics::new("category".to_string());
        stats.vertex_count = 100;
        manager.update_tag_stats(stats);

        assert_eq!(manager.get_vertex_count_by_id(5), 100);
        assert_eq!(manager.get_vertex_count_by_id(999), 0);
    }

    #[test]
    fn test_add_edge_statistics() {
        let manager = StatisticsManager::new();
        let mut edge_stats = EdgeTypeStatistics::new("follows".to_string());
        edge_stats.edge_count = 5000;

        manager.update_edge_stats(edge_stats.clone());

        let retrieved = manager.get_edge_stats("follows").expect("Edge stats not found");
        assert_eq!(retrieved.edge_count, 5000);
    }

    #[test]
    fn test_get_edge_count() {
        let manager = StatisticsManager::new();
        let mut edge_stats = EdgeTypeStatistics::new("works_at".to_string());
        edge_stats.edge_count = 3000;
        manager.update_edge_stats(edge_stats);

        assert_eq!(manager.get_edge_count("works_at"), 3000);
        assert_eq!(manager.get_edge_count("nonexistent"), 0);
    }

    #[test]
    fn test_add_property_statistics() {
        let manager = StatisticsManager::new();
        let mut prop_stats = PropertyStatistics::new(
            "age".to_string(),
            Some("person".to_string()),
        );
        prop_stats.distinct_values = 100;

        manager.update_property_stats(prop_stats);

        let retrieved = manager
            .get_property_stats(Some("person"), "age")
            .expect("Property stats not found");
        assert_eq!(retrieved.distinct_values, 100);
    }

    #[test]
    fn test_multiple_tags_statistics() {
        let manager = StatisticsManager::new();

        let mut person_stats = TagStatistics::new("person".to_string());
        person_stats.vertex_count = 1000;
        manager.update_tag_stats(person_stats);

        let mut company_stats = TagStatistics::new("company".to_string());
        company_stats.vertex_count = 500;
        manager.update_tag_stats(company_stats);

        assert_eq!(manager.get_vertex_count("person"), 1000);
        assert_eq!(manager.get_vertex_count("company"), 500);

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
        manager.update_tag_stats(stats1);

        let mut stats2 = TagStatistics::new("person".to_string());
        stats2.vertex_count = 2000;
        manager.update_tag_stats(stats2);

        assert_eq!(manager.get_vertex_count("person"), 2000);
    }

    #[test]
    fn test_clear_all_statistics() {
        let manager = StatisticsManager::new();

        let mut person_stats = TagStatistics::new("person".to_string());
        person_stats.vertex_count = 1000;
        manager.update_tag_stats(person_stats);

        let mut edge_stats = EdgeTypeStatistics::new("follows".to_string());
        edge_stats.edge_count = 5000;
        manager.update_edge_stats(edge_stats);

        assert_eq!(manager.get_all_tags().len(), 1);
        assert_eq!(manager.get_all_edge_types().len(), 1);

        manager.clear_all();

        assert_eq!(manager.get_all_tags().len(), 0);
        assert_eq!(manager.get_all_edge_types().len(), 0);
        assert_eq!(manager.get_vertex_count("person"), 0);
        assert_eq!(manager.get_edge_count("follows"), 0);
    }

    #[test]
    fn test_statistics_manager_clone() {
        let manager = StatisticsManager::new();

        let mut stats = TagStatistics::new("person".to_string());
        stats.vertex_count = 1000;
        manager.update_tag_stats(stats);

        let cloned = manager.clone();
        assert_eq!(cloned.get_vertex_count("person"), 1000);

        let mut new_stats = TagStatistics::new("company".to_string());
        new_stats.vertex_count = 500;
        cloned.update_tag_stats(new_stats);

        assert_eq!(manager.get_vertex_count("company"), 500);
    }

    #[test]
    fn test_property_combination_statistics() {
        let manager = StatisticsManager::new();
        let props = vec!["city".to_string(), "age".to_string()];
        let key = format!("person.{}", props.join("."));
        let mut combo_stats = PropertyCombinationStats::new(
            key.clone(),
            Some("person".to_string()),
            props.clone(),
        );
        combo_stats.combined_distinct_values = 50;

        manager.update_property_combo_stats(combo_stats);

        let retrieved = manager
            .get_property_combo_stats("person", &props)
            .expect("Combo stats not found");
        assert_eq!(retrieved.combined_distinct_values, 50);
    }

    #[test]
    fn test_get_combined_cardinality() {
        let manager = StatisticsManager::new();
        let props = vec!["city".to_string(), "age".to_string()];
        let key = format!("person.{}", props.join("."));
        let mut combo_stats = PropertyCombinationStats::new(
            key.clone(),
            Some("person".to_string()),
            props.clone(),
        );
        combo_stats.combined_distinct_values = 75;

        manager.update_property_combo_stats(combo_stats);

        let cardinality = manager
            .get_combined_cardinality(Some("person"), &props)
            .expect("Combined cardinality not found");
        assert_eq!(cardinality, 75);
    }

    #[test]
    fn test_property_stats_without_tag() {
        let manager = StatisticsManager::new();
        let mut prop_stats = PropertyStatistics::new("global_prop".to_string(), None);
        prop_stats.distinct_values = 200;

        manager.update_property_stats(prop_stats);

        let retrieved = manager
            .get_property_stats(None, "global_prop")
            .expect("Property stats not found");
        assert_eq!(retrieved.distinct_values, 200);
    }

    #[test]
    fn test_multiple_edge_types() {
        let manager = StatisticsManager::new();

        let mut follows = EdgeTypeStatistics::new("follows".to_string());
        follows.edge_count = 5000;
        manager.update_edge_stats(follows);

        let mut works_at = EdgeTypeStatistics::new("works_at".to_string());
        works_at.edge_count = 3000;
        manager.update_edge_stats(works_at);

        assert_eq!(manager.get_edge_count("follows"), 5000);
        assert_eq!(manager.get_edge_count("works_at"), 3000);

        let all_edge_types = manager.get_all_edge_types();
        assert_eq!(all_edge_types.len(), 2);
        assert!(all_edge_types.contains(&"follows".to_string()));
        assert!(all_edge_types.contains(&"works_at".to_string()));
    }
}
