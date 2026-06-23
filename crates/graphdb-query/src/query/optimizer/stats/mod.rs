//! Statistics Information Module
//!
//! Provide the functionality for managing and collecting statistical information required by the query optimizer.
//!
//! ## Module Structure
//!
//! “Manager” refers to a statistical information manager that oversees and manages all statistical data in a centralized manner.
//! `tag` – Statistics on tag usage
//! “Edge” – Statistical information about the type of edges
//! “Property” – Statistical information about properties.
//! `histogram` – Statistical information in the form of a histogram.
//! “Feedback” – A module for collecting runtime statistics and feedback.

pub mod edge;
pub mod feedback;
pub mod histogram;
pub mod manager;
pub mod property;
pub mod tag;

// Re-export the main types from the feedback module.
pub use edge::{EdgeTypeStatistics, HotVertexInfo, SkewnessLevel};
pub use feedback::{
    generate_query_fingerprint, normalize_query, ExecutionFeedbackCollector,
    FeedbackDrivenSelectivity, OperatorFeedback, QueryExecutionFeedback, QueryFeedbackHistory,
    SelectivityFeedbackManager, SimpleExecutionFeedback, SimpleFeedbackCollector,
};
pub use histogram::{Histogram, HistogramBucket, RangeCondition};
pub use manager::StatisticsManager;
pub use property::{PropertyCombinationStats, PropertyStatistics};
pub use tag::TagStatistics;
