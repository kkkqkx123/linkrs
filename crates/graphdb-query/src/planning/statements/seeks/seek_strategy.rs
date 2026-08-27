//! Search Strategy Module
//!
//! Define vertex search strategies and selectors to determine the method for finding the starting vertex in MATCH queries.

use crate::core::StorageError;
use crate::storage::StorageReader;

use super::edge_seek::{EdgePattern, EdgeSeek};
use super::index_seek::IndexSeek;
use super::prop_index_seek::PropIndexSeek;
use super::scan_seek::ScanSeek;
use super::seek_strategy_base::{
    SeekResult, SeekStrategyContext, SeekStrategySelector, SeekStrategyType,
};
use super::variable_prop_index_seek::VariablePropIndexSeek;
use super::vertex_seek::VertexSeek;

pub trait SeekStrategy: Send + Sync {
    fn execute<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError>;

    fn supports(&self, context: &SeekStrategyContext) -> bool;
}

pub enum AnySeekStrategy {
    VertexSeek(VertexSeek),
    IndexSeek(IndexSeek),
    PropIndexSeek(PropIndexSeek),
    VariablePropIndexSeek(VariablePropIndexSeek),
    EdgeSeek(EdgeSeek),
    ScanSeek(ScanSeek),
}

impl Clone for AnySeekStrategy {
    fn clone(&self) -> Self {
        match self {
            AnySeekStrategy::VertexSeek(v) => AnySeekStrategy::VertexSeek(v.clone()),
            AnySeekStrategy::IndexSeek(i) => AnySeekStrategy::IndexSeek(i.clone()),
            AnySeekStrategy::PropIndexSeek(p) => AnySeekStrategy::PropIndexSeek(p.clone()),
            AnySeekStrategy::VariablePropIndexSeek(v) => {
                AnySeekStrategy::VariablePropIndexSeek(v.clone())
            }
            AnySeekStrategy::EdgeSeek(e) => AnySeekStrategy::EdgeSeek(EdgeSeek::new(EdgePattern {
                edge_types: e.edge_pattern.edge_types.clone(),
                direction: e.edge_pattern.direction,
                src_vid: e.edge_pattern.src_vid.clone(),
                dst_vid: e.edge_pattern.dst_vid.clone(),
                properties: e.edge_pattern.properties.clone(),
            })),
            AnySeekStrategy::ScanSeek(s) => AnySeekStrategy::ScanSeek(s.clone()),
        }
    }
}

impl SeekStrategy for AnySeekStrategy {
    fn execute<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        match self {
            AnySeekStrategy::VertexSeek(s) => s.execute(storage, context),
            AnySeekStrategy::IndexSeek(s) => s.execute(storage, context),
            AnySeekStrategy::PropIndexSeek(s) => s.execute(storage, context),
            AnySeekStrategy::VariablePropIndexSeek(s) => s.execute(storage, context),
            AnySeekStrategy::EdgeSeek(s) => s.execute(storage, context),
            AnySeekStrategy::ScanSeek(s) => s.execute(storage, context),
        }
    }

    fn supports(&self, context: &SeekStrategyContext) -> bool {
        match self {
            AnySeekStrategy::VertexSeek(s) => s.supports(context),
            AnySeekStrategy::IndexSeek(s) => s.supports(context),
            AnySeekStrategy::PropIndexSeek(s) => s.supports(context),
            AnySeekStrategy::VariablePropIndexSeek(s) => s.supports(context),
            AnySeekStrategy::EdgeSeek(s) => s.supports(context),
            AnySeekStrategy::ScanSeek(s) => s.supports(context),
        }
    }
}

impl SeekStrategySelector {
    /// Create a PropIndexSeek strategy with parameters
    pub fn create_prop_index_strategy(
        &self,
        predicates: Vec<super::prop_index_seek::PropertyPredicate>,
    ) -> AnySeekStrategy {
        AnySeekStrategy::PropIndexSeek(PropIndexSeek::new(predicates))
    }

    /// Create a VariablePropIndexSeek strategy with parameters
    pub fn create_variable_prop_index_strategy(
        &self,
        predicates: Vec<super::variable_prop_index_seek::VariablePropertyPredicate>,
    ) -> AnySeekStrategy {
        AnySeekStrategy::VariablePropIndexSeek(VariablePropIndexSeek::new(predicates))
    }

    /// Creating an EdgeSeek policy with parameters
    pub fn create_edge_strategy(&self, edge_pattern: EdgePattern) -> AnySeekStrategy {
        AnySeekStrategy::EdgeSeek(EdgeSeek::new(edge_pattern))
    }

    pub fn find<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        let strategy_type = self.select_strategy(context);
        let strategy = match strategy_type {
            SeekStrategyType::VertexSeek => AnySeekStrategy::VertexSeek(VertexSeek::new()),
            SeekStrategyType::IndexSeek => AnySeekStrategy::IndexSeek(IndexSeek::new()),
            SeekStrategyType::PropIndexSeek => {
                let predicates = PropIndexSeek::extract_predicates(&context.predicates);
                self.create_prop_index_strategy(predicates)
            }
            SeekStrategyType::VariablePropIndexSeek => {
                let predicates = VariablePropIndexSeek::extract_predicates(&context.predicates);
                self.create_variable_prop_index_strategy(predicates)
            }
            SeekStrategyType::EdgeSeek => {
                let edge_pattern = EdgePattern {
                    edge_types: context.node_pattern.labels.clone(),
                    direction: super::edge_seek::EdgeDirection::Both,
                    src_vid: context.node_pattern.vid.clone(),
                    dst_vid: None,
                    properties: context.node_pattern.properties.clone(),
                };
                self.create_edge_strategy(edge_pattern)
            }
            SeekStrategyType::ScanSeek => {
                let any_label = context.node_pattern.labels.is_empty();
                AnySeekStrategy::ScanSeek(ScanSeek::new().with_any_label(any_label))
            }
        };
        strategy.execute(storage, context)
    }
}
