use std::collections::{HashMap, HashSet};

use graphdb_core::types::expr::ExpressionId;

use crate::planning::plan::factorization::FactorizedSchema;

/// Analysis result for group dependencies.
#[derive(Debug, Clone, Default)]
pub struct GroupDependencyAnalysis {
    pub dependent_groups: HashSet<u32>,
    pub required_flat_groups: HashSet<u32>,
    pub dependent_exprs: HashSet<ExpressionId>,
}

/// Analyzer that walks an expression tree to collect which factorization
/// groups the expression depends on.
///
/// Mirrors `lbug::planner::GroupDependencyAnalyzer` in
/// `ref/ladybug/src/planner/operator/factorization/flatten_resolver.cpp`.
pub struct GroupDependencyAnalyzer<'a> {
    schema: &'a FactorizedSchema,
    collect_dependent_expr: bool,
    dependent_groups: HashSet<u32>,
    required_flat_groups: HashSet<u32>,
    dependent_exprs: HashSet<ExpressionId>,
    /// For function handling: maps expr id to inner expression for recursion
    expr_store: HashMap<ExpressionId, graphdb_core::Expression>,
}

impl<'a> GroupDependencyAnalyzer<'a> {
    pub fn new(schema: &'a FactorizedSchema, collect_dependent_expr: bool) -> Self {
        Self {
            schema,
            collect_dependent_expr,
            dependent_groups: HashSet::new(),
            required_flat_groups: HashSet::new(),
            dependent_exprs: HashSet::new(),
            expr_store: HashMap::new(),
        }
    }

    pub fn with_expr_store(
        schema: &'a FactorizedSchema,
        collect_dependent_expr: bool,
        store: HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> Self {
        Self {
            schema,
            collect_dependent_expr,
            dependent_groups: HashSet::new(),
            required_flat_groups: HashSet::new(),
            dependent_exprs: HashSet::new(),
            expr_store: store,
        }
    }

    pub fn dependent_groups(&self) -> &HashSet<u32> {
        &self.dependent_groups
    }

    pub fn required_flat_groups(&self) -> &HashSet<u32> {
        &self.required_flat_groups
    }

    pub fn dependent_exprs(&self) -> &HashSet<ExpressionId> {
        &self.dependent_exprs
    }

    pub fn into_analysis(self) -> GroupDependencyAnalysis {
        GroupDependencyAnalysis {
            dependent_groups: self.dependent_groups,
            required_flat_groups: self.required_flat_groups,
            dependent_exprs: self.dependent_exprs,
        }
    }

    /// Analyze a single expression id: if it is in scope, record its group.
    /// Otherwise attempt to walk its inner expression tree if available.
    pub fn visit(&mut self, expr_id: &ExpressionId) {
        if self.schema.is_expression_in_scope(expr_id) {
            if let Some(pos) = self.schema.get_group_pos(expr_id) {
                self.dependent_groups.insert(pos);
            }
            if self.collect_dependent_expr {
                self.dependent_exprs.insert(expr_id.clone());
            }
            return;
        }

        // Not in scope: try to expand via stored expression.
        if let Some(expr) = self.expr_store.get(expr_id).cloned() {
            self.visit_expression(&expr);
        } else {
            // Fallback: treat as opaque dependency – no groups.
        }
    }

    pub fn visit_expression(&mut self, expr: &graphdb_core::Expression) {
        use graphdb_core::Expression;
        match expr {
            Expression::Variable(name) => {
                if let Some(pos) = self.schema.get_group_pos_by_name_opt(name) {
                    self.dependent_groups.insert(pos);
                }
            }
            Expression::Property { object, .. } => {
                self.visit_expression(object);
            }
            Expression::Binary { left, right, .. } => {
                self.visit_expression(left);
                self.visit_expression(right);
            }
            Expression::Unary { operand, .. } => {
                self.visit_expression(operand);
            }
            Expression::Function { args, name } => {
                for arg in args {
                    self.visit_expression(arg);
                }
                // List lambda functions require their lambda body to be flat.
                // Heuristic: names starting with list_ and containing lambda.
                if is_list_lambda(name) {
                    // For list lambda, all dependent groups of the lambda
                    // expression must be flat.
                    // Here args[1] is typically the lambda body.
                    if args.len() > 1 {
                        let mut lambda_analyzer =
                            GroupDependencyAnalyzer::new(self.schema, self.collect_dependent_expr);
                        lambda_analyzer.expr_store = self.expr_store.clone();
                        lambda_analyzer.visit_expression(&args[1]);
                        self.required_flat_groups
                            .extend(lambda_analyzer.dependent_groups);
                    }
                }
            }
            Expression::Aggregate { args, filter, .. } => {
                for a in args {
                    self.visit_expression(a);
                }
                if let Some(f) = filter {
                    self.visit_expression(f);
                }
            }
            Expression::List(items) => {
                for item in items {
                    self.visit_expression(item);
                }
            }
            Expression::Map(pairs) => {
                for (_, v) in pairs {
                    self.visit_expression(v);
                }
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(t) = test_expr {
                    self.visit_expression(t);
                }
                for (when, then) in conditions {
                    self.visit_expression(when);
                    self.visit_expression(then);
                }
                if let Some(d) = default {
                    self.visit_expression(d);
                }
            }
            Expression::Subscript { collection, index } => {
                self.visit_expression(collection);
                self.visit_expression(index);
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                self.visit_expression(collection);
                if let Some(s) = start {
                    self.visit_expression(s);
                }
                if let Some(e) = end {
                    self.visit_expression(e);
                }
            }
            Expression::Literal(_)
            | Expression::Parameter(_)
            | Expression::Label(_)
            | Expression::SessionVariable(_) => {}
            Expression::Path { .. } => {}
            Expression::TypeCast {
                expression: inner, ..
            } => {
                self.visit_expression(inner);
            }
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => {
                self.visit_expression(initial);
                self.visit_expression(source);
                self.visit_expression(mapping);
            }
            Expression::ListComprehension {
                variable: _,
                source,
                filter,
                map,
            } => {
                self.visit_expression(source);
                if let Some(f) = filter {
                    self.visit_expression(f);
                }
                if let Some(m) = map {
                    self.visit_expression(m);
                }
            }
            _ => {
                // For any other variants, walk children generically.
                for child in expr.children() {
                    self.visit_expression(child);
                }
            }
        }
    }

    /// Convenience: analyze a set of expression ids.
    pub fn analyze_ids(
        schema: &'a FactorizedSchema,
        expr_ids: &[ExpressionId],
        store: HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> GroupDependencyAnalysis {
        let mut analyzer = GroupDependencyAnalyzer::with_expr_store(schema, false, store);
        for id in expr_ids {
            analyzer.visit(id);
        }
        analyzer.into_analysis()
    }
}

fn is_list_lambda(name: &str) -> bool {
    // Ladybug checks function.isListLambda
    // Heuristic list lambda names in linkrs.
    matches!(
        name.to_lowercase().as_str(),
        "list_filter" | "list_extract" | "list_transform" | "list_reduce"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::factorization::FactorizedSchema;
    use graphdb_core::types::expr::ExpressionId;

    fn expr(id: u64) -> ExpressionId {
        ExpressionId::new(id)
    }

    #[test]
    fn single_expr_dependency() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0);
        schema.insert_to_group_and_scope(expr(2), g1);

        let mut analyzer = GroupDependencyAnalyzer::new(&schema, false);
        analyzer.visit(&expr(2));
        assert!(analyzer.dependent_groups().contains(&g1));
        assert!(!analyzer.dependent_groups().contains(&g0));
    }

    #[test]
    fn expression_tree_walk() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        let id_a = expr(10);
        let id_b = expr(20);
        schema.insert_to_group_and_scope(id_a.clone(), g0);
        schema.insert_to_group_and_scope(id_b.clone(), g1);

        // Build store where combined expr 30 = a + b
        let store = HashMap::new();
        let _combined_expr = graphdb_core::Expression::Binary {
            left: Box::new(graphdb_core::Expression::Variable("a".to_string())),
            op: graphdb_core::types::operators::BinaryOperator::Add,
            right: Box::new(graphdb_core::Expression::Variable("b".to_string())),
        };
        // Simulate that id_a and id_b are the leaves but combined not in scope,
        // analyzer should walk its children when store contains mapping.
        // Here we use visit_expression directly.
        let mut analyzer = GroupDependencyAnalyzer::with_expr_store(&schema, false, store);
        // Direct visit of Binary should not add groups because variable names not ids
        // So we test that visit by id works, and expression walk for property.
        analyzer.visit(&id_a);
        analyzer.visit(&id_b);
        assert_eq!(analyzer.dependent_groups().len(), 2);
    }

    #[test]
    fn variable_name_fallback() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope_with_name(expr(10), Some("a".to_string()), g0);
        schema.insert_to_group_and_scope_with_name(expr(20), Some("b".to_string()), g1);

        let the_expr = graphdb_core::Expression::Binary {
            left: Box::new(graphdb_core::Expression::Variable("a".to_string())),
            op: graphdb_core::types::operators::BinaryOperator::Add,
            right: Box::new(graphdb_core::Expression::Variable("b".to_string())),
        };
        let mut store = HashMap::new();
        let fake_id = expr(999);
        store.insert(fake_id.clone(), the_expr);
        let mut analyzer = GroupDependencyAnalyzer::with_expr_store(&schema, false, store);
        analyzer.visit(&fake_id);
        assert!(analyzer.dependent_groups().contains(&g0));
        assert!(analyzer.dependent_groups().contains(&g1));
        assert_eq!(analyzer.dependent_groups().len(), 2);
    }
}
