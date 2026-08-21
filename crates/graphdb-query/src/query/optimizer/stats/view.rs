//! Per-query statistics accessor scoped to a single space.
//!
//! The [`StatsView`] wraps the shared [`StatisticsManager`] together with the
//! space of the query being optimized. A `None` space yields no statistics
//! (returns `0` / `None`), which matches the semantics of space-less queries.

use super::manager::StatisticsManager;
use super::{EdgeTypeStatistics, TagStatistics};

/// A borrowed, space-scoped view over the statistics manager.
#[derive(Debug)]
pub struct StatsView<'a> {
    manager: &'a StatisticsManager,
    space: Option<&'a str>,
}

impl<'a> StatsView<'a> {
    /// Create a new statistics view for the given space.
    pub fn new(manager: &'a StatisticsManager, space: Option<&'a str>) -> Self {
        Self { manager, space }
    }

    /// The space this view is scoped to, if any.
    pub fn space(&self) -> Option<&'a str> {
        self.space
    }

    /// Number of vertices for a tag, or 0 when no space is set.
    pub fn vertex_count(&self, tag_name: &str) -> u64 {
        match self.space {
            Some(space) => self.manager.get_vertex_count(space, tag_name),
            None => 0,
        }
    }

    /// Number of edges for an edge type, or 0 when no space is set.
    pub fn edge_count(&self, edge_type: &str) -> u64 {
        match self.space {
            Some(space) => self.manager.get_edge_count(space, edge_type),
            None => 0,
        }
    }

    /// Tag statistics for the space, if any.
    pub fn tag_stats(&self, tag_name: &str) -> Option<TagStatistics> {
        match self.space {
            Some(space) => self.manager.get_tag_stats(space, tag_name),
            None => None,
        }
    }

    /// Edge type statistics for the space, if any.
    pub fn edge_stats(&self, edge_type: &str) -> Option<EdgeTypeStatistics> {
        match self.space {
            Some(space) => self.manager.get_edge_stats(space, edge_type),
            None => None,
        }
    }

    /// Property statistics for `property` under `tag_name` (or global), if any.
    pub fn property_stats(
        &self,
        tag_name: Option<&str>,
        property: &str,
    ) -> Option<crate::query::optimizer::stats::PropertyStatistics> {
        match self.space {
            Some(space) => self.manager.get_property_stats(space, tag_name, property),
            None => None,
        }
    }

    /// Distinct-value count (NDV) for a property column, if collected.
    pub fn property_ndv(&self, tag_name: Option<&str>, property: &str) -> Option<u64> {
        self.property_stats(tag_name, property)
            .map(|s| s.distinct_values)
            .filter(|&n| n > 0)
    }

    /// Combined cardinality for a set of grouping properties.
    pub fn combined_cardinality(
        &self,
        tag_name: Option<&str>,
        properties: &[String],
    ) -> Option<u64> {
        match self.space {
            Some(space) => self
                .manager
                .get_combined_cardinality(space, tag_name, properties),
            None => None,
        }
    }

    /// Reference to the underlying statistics manager.
    pub fn manager(&self) -> &StatisticsManager {
        self.manager
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::stats::TagStatistics;

    #[test]
    fn test_view_without_space_returns_no_stats() {
        let manager = StatisticsManager::new();
        let view = StatsView::new(&manager, None);
        assert_eq!(view.space(), None);
        assert_eq!(view.vertex_count("person"), 0);
        assert_eq!(view.edge_count("follows"), 0);
        assert!(view.tag_stats("person").is_none());
        assert!(view.edge_stats("follows").is_none());
    }

    #[test]
    fn test_view_with_space_reads_scoped_stats() {
        let manager = StatisticsManager::new();
        let mut tag = TagStatistics::new("person".to_string());
        tag.vertex_count = 1000;
        manager.update_tag_stats("basketball", tag);

        let view = StatsView::new(&manager, Some("basketball"));
        assert_eq!(view.vertex_count("person"), 1000);
        assert_eq!(view.edge_count("follows"), 0);
        assert_eq!(view.tag_stats("person").map(|s| s.vertex_count), Some(1000));

        let other = StatsView::new(&manager, Some("tennis"));
        assert_eq!(other.vertex_count("person"), 0);
    }
}
