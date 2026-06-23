//! MATCH Route Planner
//!
//! Responsible for planning the path patterns in MATCH queries and generating the traversal plans.

use crate::core::types::graph_schema::EdgeDirection;
use crate::core::{StorageError, Value};
use crate::query::planning::statements::seeks::seek_strategy_base::{
    NodePattern, SeekStrategyContext, SeekStrategySelector, SeekStrategyType,
};

pub type PlannerError = StorageError;

#[derive(Debug, Clone)]
pub struct VariableLengthPathPattern<'a> {
    pub start: &'a NodePattern,
    pub edge: &'a EdgePattern,
    pub end: &'a NodePattern,
    pub lower: Option<usize>,
    pub upper: Option<usize>,
}

#[derive(Debug)]
pub struct MatchPathPlanner;

impl Default for MatchPathPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchPathPlanner {
    pub fn new() -> Self {
        Self
    }

    pub fn plan_path_pattern(
        &self,
        pattern: &PathPattern,
        space_id: u64,
    ) -> Result<PathPlan, PlannerError> {
        match &pattern.kind {
            PathPatternKind::Simple { start, edge, end } => {
                self.plan_simple_pattern(start, edge, end, space_id)
            }
            PathPatternKind::VariableLength {
                start,
                edge,
                end,
                lower,
                upper,
            } => {
                let pattern = VariableLengthPathPattern {
                    start,
                    edge,
                    end,
                    lower: *lower,
                    upper: *upper,
                };
                self.plan_variable_length_pattern(&pattern, space_id)
            }
            PathPatternKind::Named { name, inner } => {
                let inner_plan = self.plan_path_pattern(inner, space_id)?;
                Ok(PathPlan::Named {
                    name: name.clone(),
                    inner: Box::new(inner_plan),
                })
            }
        }
    }

    fn plan_simple_pattern(
        &self,
        start: &NodePattern,
        edge: &EdgePattern,
        end: &NodePattern,
        space_id: u64,
    ) -> Result<PathPlan, PlannerError> {
        let start_finder = self.plan_start_finder(start, space_id)?;
        let edge_traversal = self.plan_edge_traversal(edge)?;
        let end_finder = self.plan_end_finder(end)?;

        Ok(PathPlan::Simple {
            start: Box::new(start_finder),
            edge: edge_traversal,
            end: end_finder,
        })
    }

    fn plan_variable_length_pattern(
        &self,
        pattern: &VariableLengthPathPattern,
        space_id: u64,
    ) -> Result<PathPlan, PlannerError> {
        let start_finder = self.plan_start_finder(pattern.start, space_id)?;
        let edge_types = self.extract_edge_types(pattern.edge)?;
        let direction = self.extract_direction(pattern.edge)?;
        let end_finder = self.plan_end_finder(pattern.end)?;

        Ok(PathPlan::VariableLength {
            start: Box::new(start_finder),
            edge_types,
            direction,
            end: end_finder,
            lower: pattern.lower,
            upper: pattern.upper,
        })
    }

    fn plan_start_finder(
        &self,
        pattern: &NodePattern,
        space_id: u64,
    ) -> Result<StartVidFinder, PlannerError> {
        let context = SeekStrategyContext::new(space_id, pattern.clone(), vec![]);
        let selector = SeekStrategySelector::new();
        let strategy_type = selector.select_strategy(&context);

        let finder = match strategy_type {
            SeekStrategyType::VertexSeek => StartVidFinder::VertexSeek {
                pattern: pattern.clone(),
            },
            SeekStrategyType::IndexSeek => StartVidFinder::IndexScan {
                pattern: pattern.clone(),
            },
            SeekStrategyType::PropIndexSeek => StartVidFinder::PropIndexScan {
                pattern: pattern.clone(),
            },
            SeekStrategyType::VariablePropIndexSeek => StartVidFinder::VariablePropIndexScan {
                pattern: pattern.clone(),
            },
            SeekStrategyType::EdgeSeek => StartVidFinder::EdgeScan {
                pattern: pattern.clone(),
            },
            SeekStrategyType::ScanSeek => StartVidFinder::FullScan {
                pattern: pattern.clone(),
            },
        };

        Ok(finder)
    }

    fn plan_end_finder(&self, pattern: &NodePattern) -> Result<EndCondition, PlannerError> {
        Ok(EndCondition {
            pattern: pattern.clone(),
        })
    }

    fn plan_edge_traversal(&self, edge: &EdgePattern) -> Result<EdgeTraversal, PlannerError> {
        let direction = self.extract_direction(edge)?;
        let edge_types = self.extract_edge_types(edge)?;
        let properties = edge.properties.clone();

        Ok(EdgeTraversal {
            direction,
            edge_types,
            properties,
        })
    }

    fn extract_direction(&self, edge: &EdgePattern) -> Result<EdgeDirection, PlannerError> {
        Ok(match edge.direction {
            Some(ref dir) => *dir,
            None => EdgeDirection::Both,
        })
    }

    fn extract_edge_types(&self, edge: &EdgePattern) -> Result<Vec<String>, PlannerError> {
        Ok(edge.types.clone().unwrap_or_default())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathPattern {
    pub kind: PathPatternKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PathPatternKind {
    Simple {
        start: NodePattern,
        edge: EdgePattern,
        end: NodePattern,
    },
    VariableLength {
        start: NodePattern,
        edge: EdgePattern,
        end: NodePattern,
        lower: Option<usize>,
        upper: Option<usize>,
    },
    Named {
        name: String,
        inner: Box<PathPattern>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgePattern {
    pub types: Option<Vec<String>>,
    pub direction: Option<EdgeDirection>,
    pub properties: Vec<(String, Value)>,
}

#[derive(Debug)]
pub enum StartVidFinder {
    VertexSeek { pattern: NodePattern },
    IndexScan { pattern: NodePattern },
    PropIndexScan { pattern: NodePattern },
    VariablePropIndexScan { pattern: NodePattern },
    EdgeScan { pattern: NodePattern },
    FullScan { pattern: NodePattern },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndCondition {
    pub pattern: NodePattern,
}

#[derive(Debug)]
pub enum PathPlan {
    Simple {
        start: Box<StartVidFinder>,
        edge: EdgeTraversal,
        end: EndCondition,
    },
    VariableLength {
        start: Box<StartVidFinder>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        end: EndCondition,
        lower: Option<usize>,
        upper: Option<usize>,
    },
    Named {
        name: String,
        inner: Box<PathPlan>,
    },
}

#[derive(Debug)]
pub struct EdgeTraversal {
    pub direction: EdgeDirection,
    pub edge_types: Vec<String>,
    pub properties: Vec<(String, Value)>,
}

impl PathPattern {
    pub fn simple(start: NodePattern, edge: EdgePattern, end: NodePattern) -> Self {
        Self {
            kind: PathPatternKind::Simple { start, edge, end },
        }
    }

    pub fn variable_length(
        start: NodePattern,
        edge: EdgePattern,
        end: NodePattern,
        lower: Option<usize>,
        upper: Option<usize>,
    ) -> Self {
        Self {
            kind: PathPatternKind::VariableLength {
                start,
                edge,
                end,
                lower,
                upper,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_path_planner_new() {
        let _planner = MatchPathPlanner::new();
        // If the test is successful and you have reached this point, it means that everything has gone as planned.
    }

    #[test]
    fn test_path_pattern_simple() {
        let _pattern = PathPattern::simple(
            NodePattern {
                vid: Some(Value::String("start".to_string())),
                labels: vec![],
                properties: vec![],
            },
            EdgePattern {
                types: Some(vec!["follows".to_string()]),
                direction: Some(EdgeDirection::Out),
                properties: vec![],
            },
            NodePattern {
                vid: Some(Value::String("end".to_string())),
                labels: vec![],
                properties: vec![],
            },
        );
        // If the test is successful and you have reached this point, it means that everything has gone as planned.
    }

    #[test]
    fn test_path_pattern_variable_length() {
        let _pattern = PathPattern::variable_length(
            NodePattern {
                vid: None,
                labels: vec!["person".to_string()],
                properties: vec![],
            },
            EdgePattern {
                types: Some(vec!["follows".to_string()]),
                direction: Some(EdgeDirection::Out),
                properties: vec![],
            },
            NodePattern {
                vid: None,
                labels: vec!["person".to_string()],
                properties: vec![],
            },
            Some(1),
            Some(5),
        );
        // If the test is successful and you have reached this point, it means that everything has gone well.
    }
}
