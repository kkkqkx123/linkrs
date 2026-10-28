//! Search Strategy Module
//!
//! Define vertex search strategies and selectors to determine the method for finding the starting vertex in MATCH queries.
//!
//! ## Index and cost model scope
//!
//! - `IndexInfo` (here) is the canonical planning-side index descriptor,
//!   reused by the LOOKUP composite analyzer. It is a snapshot built from
//!   metadata, not the metadata record itself.
//! - Single-label MATCH scans use `metadata::IndexMetadata` directly through
//!   `super::index_scan_planner` (first-hit heuristic, no cost comparison)
//!   by design; that path does not go through `IndexInfo`.
//! - All no-statistics index costs flow through
//!   `optimizer::cost::CostModelConfig::index_seek_cost`; the
//!   statistics-aware calculator entry stays in `optimizer::cost::calculator`.
//! - The integer `join_order::CostModel` prices MATCH graph DP splits only
//!   and is intentionally separate from the floating-point optimizer costs.
//!
//! ## Modules
//!
//! - `seek_strategy_base`: Core strategy types and cost-based selector
//! - `multi_label_index_selector`: Multi-label index selection for `(:Label1:Label2)` patterns
//! - `vertex_seek`: Direct vertex ID lookup
//! - `index_seek`: Tag/Label index scan
//! - `prop_index_seek`: Property index scan
//! - `scan_seek`: Full table scan

pub mod edge_seek;
pub mod index_seek;
pub mod multi_label_index_selector;
pub mod prop_index_seek;
pub mod scan_seek;
pub mod seek_strategy;
pub mod seek_strategy_base;
pub mod variable_prop_index_seek;
pub mod vertex_seek;

pub use edge_seek::EdgeSeek;
pub use index_seek::IndexSeek;
pub use multi_label_index_selector::{
    IndexRegistry, LabelStats, MultiLabelIndexSelector, MultiLabelStrategy, SelectorError,
};
pub use prop_index_seek::{PredicateOp, PropIndexSeek, PropertyPredicate};
pub use scan_seek::ScanSeek;
pub use seek_strategy::{AnySeekStrategy, SeekStrategy};
pub use seek_strategy_base::{
    IndexInfo, NodePattern, SeekResult, SeekStrategyContext, SeekStrategySelector, SeekStrategyType,
};
pub use variable_prop_index_seek::VariablePropIndexSeek;
pub use vertex_seek::VertexSeek;
