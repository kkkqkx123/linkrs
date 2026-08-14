//! Cost-based index selection rewriter.
//!
//! Rewrites `Filter -> ScanVertices` subtrees into `Filter -> IndexScan`
//! when the cost-based `IndexSelector` prefers a property index over a full
//! scan. The residual filter is kept above the index scan so semantics are
//! unchanged; the index only narrows the scanned key range.
//!
//! The index catalog is supplied per query by the pipeline (registered into
//! the optimizer's `StatisticsManager` from the planning metadata context).

use std::sync::Arc;

use crate::core::types::expr::Expression;
use crate::core::types::operators::BinaryOperator;
use crate::core::types::Index;
use crate::core::value::Value;
use crate::query::optimizer::cost_based::traversal::rewrite_children;
use crate::query::optimizer::cost_based::traversal_logical::rewrite_children_logical;
use crate::query::optimizer::cost_based::{
    IndexSelection, IndexSelector, PredicateOperator, PropertyPredicate,
};
use crate::query::optimizer::stats::StatisticsManager;
use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
use crate::query::planning::plan::core::nodes::access::index_scan::{
    IndexLimit, IndexScanNode, ScanType,
};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::logical::logical_node_traits::LogicalSingleInputNode;
use crate::query::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
use crate::query::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
use crate::query::planning::plan::logical::LogicalNodeEnum;
use crate::query::planning::plan::PlanNodeEnum;

/// Rewrite eligible `ScanVertices` scans into `IndexScan` nodes.
///
/// Walks the plan post-order; at each `Filter` whose input is a
/// `ScanVertices` with a tag, property predicates are extracted from the
/// filter condition and the `IndexSelector` decides between the available
/// indexes and a full scan. Decisions are appended to `notes`.
///
/// `space_hint` is the query-level space name, used when the scan node does
/// not carry one; the scan's own `space_name` wins when present.
pub fn rewrite_index_scans(
    node: &PlanNodeEnum,
    selector: &IndexSelector,
    stats_manager: &Arc<StatisticsManager>,
    space_hint: Option<&str>,
    notes: &mut Vec<String>,
) -> PlanNodeEnum {
    use PlanNodeEnum::*;

    // Try index selection at this level first.
    if let Filter(filter) = node {
        let input = filter.input();
        if let ScanVertices(scan) = input {
            if let Some((new_input, note)) =
                try_rewrite_scan(scan, filter, selector, stats_manager, space_hint)
            {
                notes.push(note);
                let mut new_filter = filter.clone();
                new_filter.set_input(new_input);
                return Filter(new_filter);
            }
        }
    }

    // Recursively rewrite children.
    let mut closure = |child: &PlanNodeEnum| {
        rewrite_index_scans(child, selector, stats_manager, space_hint, notes)
    };
    rewrite_children(node, &mut closure)
}

/// Try to rewrite a single `Filter -> ScanVertices` pair into
/// `Filter -> IndexScan`. Returns `(new_input, note)` when an index scan
/// is chosen; `None` when the rewrite does not apply.
fn try_rewrite_scan(
    scan: &ScanVerticesNode,
    filter: &crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode,
    selector: &IndexSelector,
    stats_manager: &Arc<StatisticsManager>,
    space_hint: Option<&str>,
) -> Option<(PlanNodeEnum, String)> {
    let tag = scan.tag().cloned()?;
    let space: String = if scan.space_name().is_empty() {
        space_hint?.to_string()
    } else {
        scan.space_name().to_string()
    };

    // The predicates come from the filter condition above the scan (the
    // scan node's own vertex filter is redundant with it in practice).
    let predicates = filter
        .condition()
        .expression()
        .map(|meta| extract_property_predicates(meta.inner()))
        .unwrap_or_default();
    if predicates.is_empty() {
        return None;
    }

    let (tag_id, available_indexes) = stats_manager.get_tag_indexes(&space, &tag)?;
    if available_indexes.is_empty() {
        return None;
    }

    let selection = selector.select_index(&space, &tag, &predicates, &available_indexes);
    let (index_name, selectivity, estimated_cost) = match selection {
        IndexSelection::PropertyIndex {
            index_name,
            selectivity,
            estimated_cost,
            ..
        } => (index_name, selectivity, estimated_cost),
        IndexSelection::FullScan { .. } | IndexSelection::TagIndex { .. } => return None,
    };

    let index = available_indexes
        .iter()
        .find(|candidate| candidate.name == index_name)?;
    let scan_limits = build_scan_limits(&predicates, &index.properties);
    if scan_limits.is_empty() {
        return None;
    }

    let scan_type = if scan_limits.len() == 1 && scan_limits[0].scan_type == ScanType::Unique {
        ScanType::Unique
    } else {
        ScanType::Range
    };

    let full_scan = full_scan_cost(selector, &space, &tag, &available_indexes, &predicates);
    let note = format!(
        "index: tag '{}' -> index_scan('{}') (sel={:.3}, cost {:.2} vs full_scan {:.2})",
        tag,
        index_name,
        selectivity,
        estimated_cost,
        full_scan.unwrap_or(estimated_cost)
    );

    let mut index_scan = IndexScanNode::new(
        scan.space_id(),
        tag_id,
        index.id,
        index_name,
        tag,
        scan_type,
    );
    index_scan.set_scan_limits(scan_limits);
    index_scan.set_col_names(scan.col_names().to_vec());
    if let Some(output_var) = scan.output_var() {
        index_scan.set_output_var(output_var.to_string());
    }

    Some((PlanNodeEnum::IndexScan(index_scan), note))
}

/// Extract property predicates from a conjunctive filter expression.
///
/// Supports `prop op literal` (either operand order) for `=`, `!=`, `<`,
/// `<=`, `>`, `>=`, `LIKE`, and `IN`. Nested `AND` conditions are flattened.
fn extract_property_predicates(expr: &Expression) -> Vec<PropertyPredicate> {
    let mut predicates = Vec::new();
    extract_property_predicates_recursive(expr, &mut predicates);
    predicates
}

fn extract_property_predicates_recursive(
    expr: &Expression,
    predicates: &mut Vec<PropertyPredicate>,
) {
    match expr {
        Expression::Binary {
            op: BinaryOperator::And,
            left,
            right,
        } => {
            extract_property_predicates_recursive(left, predicates);
            extract_property_predicates_recursive(right, predicates);
        }
        Expression::Binary { left, op, right } => {
            if let Some((property_name, value, operator)) =
                extract_binary_predicate(left, right, op)
            {
                predicates.push(PropertyPredicate {
                    property_name,
                    operator,
                    value,
                });
            }
        }
        _ => {}
    }
}

fn extract_binary_predicate(
    left: &Expression,
    right: &Expression,
    op: &BinaryOperator,
) -> Option<(String, Expression, PredicateOperator)> {
    let (property_side, value_side, operator) =
        match extract_property(left).zip(extract_literal(right)) {
            Some((prop, value)) => (prop, value, map_predicate_operator(op, false)),
            None => match extract_property(right).zip(extract_literal(left)) {
                Some((prop, value)) => (prop, value, map_predicate_operator(op, true)),
                None => return None,
            },
        };
    operator.map(|op| (property_side, Expression::Literal(value_side), op))
}

fn extract_property(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Property { object, property } => match object.as_ref() {
            Expression::Variable(_) => Some(property.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn extract_literal(expr: &Expression) -> Option<Value> {
    match expr {
        Expression::Literal(value) => Some(value.clone()),
        _ => None,
    }
}

fn map_predicate_operator(op: &BinaryOperator, reversed: bool) -> Option<PredicateOperator> {
    let mapped = match op {
        BinaryOperator::Equal => PredicateOperator::Equal,
        BinaryOperator::NotEqual => PredicateOperator::NotEqual,
        BinaryOperator::LessThan => PredicateOperator::LessThan,
        BinaryOperator::LessThanOrEqual => PredicateOperator::LessThanOrEqual,
        BinaryOperator::GreaterThan => PredicateOperator::GreaterThan,
        BinaryOperator::GreaterThanOrEqual => PredicateOperator::GreaterThanOrEqual,
        BinaryOperator::Like => PredicateOperator::Like,
        BinaryOperator::In => PredicateOperator::In,
        _ => return None,
    };
    // When the literal is on the left (`5 < n.age`), mirror the operator.
    if reversed {
        Some(match mapped {
            PredicateOperator::LessThan => PredicateOperator::GreaterThan,
            PredicateOperator::LessThanOrEqual => PredicateOperator::GreaterThanOrEqual,
            PredicateOperator::GreaterThan => PredicateOperator::LessThan,
            PredicateOperator::GreaterThanOrEqual => PredicateOperator::LessThanOrEqual,
            other => other,
        })
    } else {
        Some(mapped)
    }
}

/// Build the index limits pushed into the native index cursor.
///
/// Only predicates on columns of `index_properties` map to limits; the
/// remaining conditions stay in the residual filter above.
fn build_scan_limits(
    predicates: &[PropertyPredicate],
    index_properties: &[String],
) -> Vec<IndexLimit> {
    predicates
        .iter()
        .filter(|predicate| {
            index_properties
                .iter()
                .any(|prop| prop == &predicate.property_name)
        })
        .filter_map(|predicate| {
            let value = match &predicate.value {
                Expression::Literal(value) => value.clone(),
                _ => return None,
            };
            match predicate.operator {
                PredicateOperator::Equal => {
                    Some(IndexLimit::equal(predicate.property_name.clone(), value))
                }
                PredicateOperator::LessThan => Some(IndexLimit::range(
                    predicate.property_name.clone(),
                    None,
                    Some(value),
                    false,
                    false,
                )),
                PredicateOperator::LessThanOrEqual => Some(IndexLimit::range(
                    predicate.property_name.clone(),
                    None,
                    Some(value),
                    false,
                    true,
                )),
                PredicateOperator::GreaterThan => Some(IndexLimit::range(
                    predicate.property_name.clone(),
                    Some(value),
                    None,
                    false,
                    false,
                )),
                PredicateOperator::GreaterThanOrEqual => Some(IndexLimit::range(
                    predicate.property_name.clone(),
                    Some(value),
                    None,
                    true,
                    false,
                )),
                _ => None,
            }
        })
        .collect()
}

/// Look up the full-scan cost the selector would assign (helper for notes).
fn full_scan_cost(
    selector: &IndexSelector,
    space: &str,
    tag: &str,
    available_indexes: &[Index],
    predicates: &[PropertyPredicate],
) -> Option<f64> {
    let selection = selector.select_index(space, tag, predicates, available_indexes);
    match selection {
        IndexSelection::FullScan { estimated_cost, .. }
        | IndexSelection::TagIndex { estimated_cost, .. } => Some(estimated_cost),
        IndexSelection::PropertyIndex { .. } => None,
    }
}

// =====================================================================
// Logical-plan variant (PlanNodeEnum logic/physical separation).
//
// The index selection decision is taken on the pure logical tree
// (`LogicalNodeEnum`) and emits the same `index:` note as the physical
// walker. The logical tree stays pure — the IndexScan rewrite is applied
// by the physical walker on the executable root.
// =====================================================================

/// Walk a logical plan tree and record the cost-based index selection
/// decision for every `Filter -> ScanVertices` pair as a CBO note.
///
/// The tree itself is returned unchanged (IndexScan is a physical operator
/// and cannot appear in the logical tree); the physical rewriter consumes
/// the same statistics to apply the structural rewrite.
pub fn rewrite_index_scans_logical(
    node: &LogicalNodeEnum,
    selector: &IndexSelector,
    stats_manager: &Arc<StatisticsManager>,
    space_hint: Option<&str>,
    notes: &mut Vec<String>,
) -> LogicalNodeEnum {
    use LogicalNodeEnum::*;

    // Try index selection at this level first.
    if let Filter(filter) = node {
        let input = filter.input();
        if let ScanVertices(scan) = input {
            if let Some(note) =
                try_decide_index_scan_logical(scan, filter, selector, stats_manager, space_hint)
            {
                notes.push(note);
            }
        }
    }

    // Recursively walk children (decision-only, tree unchanged).
    let mut closure = |child: &LogicalNodeEnum| {
        rewrite_index_scans_logical(child, selector, stats_manager, space_hint, notes)
    };
    rewrite_children_logical(node, &mut closure)
}

/// Decide whether a logical `Filter -> ScanVertices` pair would be rewritten
/// to an index scan, returning the CBO note when an index wins.
fn try_decide_index_scan_logical(
    scan: &LogicalScanVerticesNode,
    filter: &LogicalFilterNode,
    selector: &IndexSelector,
    stats_manager: &Arc<StatisticsManager>,
    space_hint: Option<&str>,
) -> Option<String> {
    let tag = scan.tag.clone()?;
    let space: String = if scan.space_name.is_empty() {
        space_hint?.to_string()
    } else {
        scan.space_name.clone()
    };

    let predicates = filter
        .condition
        .expression()
        .map(|meta| extract_property_predicates(meta.inner()))
        .unwrap_or_default();
    if predicates.is_empty() {
        return None;
    }

    let (_tag_id, available_indexes) = stats_manager.get_tag_indexes(&space, &tag)?;
    if available_indexes.is_empty() {
        return None;
    }

    let selection = selector.select_index(&space, &tag, &predicates, &available_indexes);
    let (index_name, selectivity, estimated_cost) = match selection {
        IndexSelection::PropertyIndex {
            index_name,
            selectivity,
            estimated_cost,
            ..
        } => (index_name, selectivity, estimated_cost),
        IndexSelection::FullScan { .. } | IndexSelection::TagIndex { .. } => return None,
    };

    let index = available_indexes
        .iter()
        .find(|candidate| candidate.name == index_name)?;
    let scan_limits = build_scan_limits(&predicates, &index.properties);
    if scan_limits.is_empty() {
        return None;
    }

    let full_scan = full_scan_cost(selector, &space, &tag, &available_indexes, &predicates);
    Some(format!(
        "index: tag '{}' -> index_scan('{}') (sel={:.3}, cost {:.2} vs full_scan {:.2})",
        tag,
        index_name,
        selectivity,
        estimated_cost,
        full_scan.unwrap_or(estimated_cost)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::query::optimizer::cost::CostCalculator;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use std::sync::Arc;

    fn test_selector() -> (IndexSelector, Arc<StatisticsManager>) {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager.clone()));
        let selectivity = Arc::new(crate::query::optimizer::cost::SelectivityEstimator::new(
            stats_manager.clone(),
        ));
        (
            IndexSelector::new(cost_calculator, selectivity),
            stats_manager,
        )
    }

    fn register_index(manager: &Arc<StatisticsManager>, tag: &str, name: &str, property: &str) {
        let index = Index {
            id: 1,
            name: name.to_string(),
            space_id: 1,
            schema_name: tag.to_string(),
            fields: Vec::new(),
            properties: vec![property.to_string()],
            index_type: crate::core::types::IndexType::TagIndex,
            status: crate::core::types::IndexStatus::Active,
            is_unique: false,
            comment: None,
            covering: false,
            partial_condition: None,
        };
        manager.register_tag_indexes("test", tag, 1, vec![index]);
    }

    fn build_scan_filter(tag: &str, property: &str, value: Value) -> PlanNodeEnum {
        use crate::core::types::expr::{ContextualExpression, ExpressionMeta};

        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag(tag);
        scan.set_col_names(vec!["n".to_string()]);
        scan.set_output_var("n".to_string());
        let context = Arc::new(ExpressionAnalysisContext::new());
        let expression = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: property.to_string(),
            }),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(value)),
        };
        let id = context.register_expression(ExpressionMeta::new(expression));
        let contextual = ContextualExpression::new(id, context);
        let filter = FilterNode::new(PlanNodeEnum::ScanVertices(scan), contextual)
            .expect("filter should build");
        PlanNodeEnum::Filter(filter)
    }

    #[test]
    fn rewrites_scan_with_index_to_index_scan() {
        let (selector, manager) = test_selector();
        register_index(&manager, "person", "idx_name", "name");
        let plan = build_scan_filter("person", "name", Value::String("alice".into()));
        let mut notes = Vec::new();
        let rewritten = rewrite_index_scans(&plan, &selector, &manager, Some("test"), &mut notes);
        assert!(matches!(&rewritten, PlanNodeEnum::Filter(_)));
        let filter = match &rewritten {
            PlanNodeEnum::Filter(f) => f,
            _ => panic!("expected filter"),
        };
        assert!(matches!(filter.input(), PlanNodeEnum::IndexScan(_)));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("index_scan('idx_name')"));
    }

    #[test]
    fn keeps_scan_when_no_index_registered() {
        let (selector, manager) = test_selector();
        let plan = build_scan_filter("person", "name", Value::String("alice".into()));
        let mut notes = Vec::new();
        let rewritten = rewrite_index_scans(&plan, &selector, &manager, Some("test"), &mut notes);
        assert!(matches!(rewritten, PlanNodeEnum::Filter(_)));
        let filter = match &rewritten {
            PlanNodeEnum::Filter(f) => f,
            _ => panic!("expected filter"),
        };
        assert!(matches!(filter.input(), PlanNodeEnum::ScanVertices(_)));
        assert!(notes.is_empty());
    }

    #[test]
    fn extract_handles_and_and_operator_sides() {
        let expression = Expression::Binary {
            op: BinaryOperator::And,
            left: Box::new(Expression::Binary {
                op: BinaryOperator::Equal,
                left: Box::new(Expression::Property {
                    object: Box::new(Expression::Variable("n".to_string())),
                    property: "name".to_string(),
                }),
                right: Box::new(Expression::Literal(Value::String("alice".into()))),
            }),
            right: Box::new(Expression::Binary {
                op: BinaryOperator::LessThan,
                left: Box::new(Expression::Literal(Value::Int(30))),
                right: Box::new(Expression::Property {
                    object: Box::new(Expression::Variable("n".to_string())),
                    property: "age".to_string(),
                }),
            }),
        };
        let predicates = extract_property_predicates(&expression);
        assert_eq!(predicates.len(), 2);
        assert_eq!(predicates[0].property_name, "name");
        assert_eq!(predicates[0].operator, PredicateOperator::Equal);
        assert_eq!(predicates[1].property_name, "age");
        assert_eq!(predicates[1].operator, PredicateOperator::GreaterThan);
    }

    #[test]
    fn build_limits_maps_equality_to_unique() {
        let predicates = vec![PropertyPredicate {
            property_name: "name".to_string(),
            operator: PredicateOperator::Equal,
            value: Expression::Literal(Value::String("alice".into())),
        }];
        let limits = build_scan_limits(&predicates, &["name".to_string()]);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].scan_type, ScanType::Unique);
    }

    #[test]
    fn scan_type_is_range_for_multiple_limits() {
        let predicates = vec![
            PropertyPredicate {
                property_name: "age".to_string(),
                operator: PredicateOperator::GreaterThan,
                value: Expression::Literal(Value::Int(30)),
            },
            PropertyPredicate {
                property_name: "age".to_string(),
                operator: PredicateOperator::LessThan,
                value: Expression::Literal(Value::Int(60)),
            },
        ];
        let limits = build_scan_limits(&predicates, &["age".to_string()]);
        assert_eq!(limits.len(), 2);
        assert!(limits
            .iter()
            .all(|limit| limit.scan_type == ScanType::Range));
    }

    #[test]
    fn full_scan_cost_returns_scan_cost_for_notes() {
        let (selector, manager) = test_selector();
        register_index(&manager, "person", "idx_name", "other_prop");
        let predicates = vec![PropertyPredicate {
            property_name: "name".to_string(),
            operator: PredicateOperator::Equal,
            value: Expression::Literal(Value::String("alice".into())),
        }];
        let (_, indexes) = manager.get_tag_indexes("test", "person").expect("indexes");
        let cost = full_scan_cost(&selector, "test", "person", &indexes, &predicates);
        assert!(cost.is_some());
    }

    // ===================================================================
    // Logical-plan walker tests
    // ===================================================================

    use crate::query::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::query::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
    use crate::query::planning::plan::logical::LogicalNodeEnum;

    fn build_logical_scan_filter(tag: &str, property: &str, value: Value) -> LogicalNodeEnum {
        use crate::core::types::expr::{ContextualExpression, ExpressionMeta};

        let scan = LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: 1,
            space_id: 1,
            space_name: "test".to_string(),
            tag: Some(tag.to_string()),
            expression: None,
            limit: None,
            projected_properties: vec![],
            output_var: Some("n".to_string()),
            col_names: vec!["n".to_string()],
            column_types: vec![],
        });
        let context = Arc::new(ExpressionAnalysisContext::new());
        let expression = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: property.to_string(),
            }),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(value)),
        };
        let id = context.register_expression(ExpressionMeta::new(expression));
        let contextual = ContextualExpression::new(id, context);
        LogicalNodeEnum::Filter(LogicalFilterNode {
            id: 2,
            input: Some(Box::new(scan.clone())),
            deps: vec![scan],
            condition: contextual,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }

    #[test]
    fn logical_walk_emits_index_note_when_index_registered() {
        let (selector, manager) = test_selector();
        register_index(&manager, "person", "idx_name", "name");
        let plan = build_logical_scan_filter("person", "name", Value::String("alice".into()));
        let mut notes = Vec::new();
        let rewritten =
            rewrite_index_scans_logical(&plan, &selector, &manager, Some("test"), &mut notes);

        // The logical tree stays pure — the ScanVertices input is preserved.
        assert!(matches!(&rewritten, LogicalNodeEnum::Filter(_)));
        let filter = match &rewritten {
            LogicalNodeEnum::Filter(f) => f,
            _ => panic!("expected logical filter"),
        };
        assert!(matches!(filter.input(), LogicalNodeEnum::ScanVertices(_)));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("index_scan('idx_name')"));
    }

    #[test]
    fn logical_walk_keeps_silent_without_index() {
        let (selector, manager) = test_selector();
        let plan = build_logical_scan_filter("person", "name", Value::String("alice".into()));
        let mut notes = Vec::new();
        let rewritten =
            rewrite_index_scans_logical(&plan, &selector, &manager, Some("test"), &mut notes);
        assert!(matches!(rewritten, LogicalNodeEnum::Filter(_)));
        assert!(notes.is_empty());
    }
}
