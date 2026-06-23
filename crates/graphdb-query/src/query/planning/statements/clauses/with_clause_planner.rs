//! The WITH clause planner
//!
//! It is responsible for planning the execution of the WITH clause and serves as a point of transformation in the data flow.
//!
//! The function of the WITH clause:
//! Projection: Select the output columns and, if necessary, rename them.
//! 2. Filtering: The results are filtered using the WHERE clause.
//! 3. Sorting: Sort the results using the ORDER BY clause.
//! 4. Pagination: The number of results can be limited using the SKIP/LIMIT parameters.
//! 5. Scope reset: Only the variables that are output are retained; all other variables become invisible.

use crate::core::types::expr::expression_utils::extract_group_info;
use crate::core::YieldColumn;
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::{FilterNode, LimitNode, PlanNodeEnum, ProjectNode};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;
use crate::query::planning::statements::statement_planner::ClausePlanner;
use crate::query::validator::structs::{
    AliasType, CypherClauseKind, OrderByClauseContext, PaginationContext, WithClauseContext,
};
use crate::query::QueryContext;
use std::collections::HashMap;
use std::sync::Arc;

/// WITH clause planner
#[derive(Debug)]
pub struct WithClausePlanner {}

impl WithClausePlanner {
    /// Create a new planner for the WITH clause.
    pub fn new() -> Self {
        Self {}
    }

    /// Planning with the WITH clause
    ///
    /// # Parameters
    /// `with_ctx`: The context of the WITH clause, which includes information such as the projected columns, WHERE conditions, sorting criteria, and pagination options.
    /// `input_plan`: Input plan
    ///
    /// # Return
    /// Success: The generated sub-plan has been successfully created.
    /// Failure: Incorrect planning.
    pub fn plan_with_clause(
        &self,
        with_ctx: &WithClauseContext,
        input_plan: &SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let mut current_plan = input_plan.clone();

        // 1. Construct the projection node (if there are specific output columns).
        if !with_ctx.yield_clause.yield_columns.is_empty() {
            let project_node =
                self.create_project_node(&current_plan, &with_ctx.yield_clause.yield_columns)?;
            current_plan = SubPlan::new(Some(project_node), current_plan.tail.clone());
        }

        // 2. Processing the WHERE clause for filtering data
        if let Some(ref where_ctx) = with_ctx.where_clause {
            if let Some(ref filter) = where_ctx.filter {
                let filter_node = self.create_filter_node(&current_plan, filter)?;
                current_plan = SubPlan::new(Some(filter_node), current_plan.tail.clone());
            }
        }

        // 3. Handling the ORDER BY sorting
        if let Some(ref order_by_ctx) = with_ctx.order_by {
            current_plan = self.apply_order_by(current_plan, order_by_ctx)?;
        }

        // 4. Handling pagination (SKIP/LIMIT)
        if let Some(ref pagination) = with_ctx.pagination {
            current_plan = self.apply_pagination(current_plan, pagination)?;
        }

        // 5. Handling DISTINCT (removing duplicates)
        if with_ctx.distinct {
            current_plan = self.apply_distinct(current_plan)?;
        }

        Ok(current_plan)
    }

    /// Create a projection node.
    fn create_project_node(
        &self,
        input_plan: &SubPlan,
        columns: &[YieldColumn],
    ) -> Result<PlanNodeEnum, PlannerError> {
        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        ProjectNode::new(input_node.clone(), columns.to_vec())
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!(
                    "Failed to create projection node: {}",
                    e
                ))
            })
            .map(PlanNodeEnum::Project)
    }

    /// Create a filter node.
    fn create_filter_node(
        &self,
        input_plan: &SubPlan,
        condition: &crate::core::types::expr::contextual::ContextualExpression,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        FilterNode::new(input_node.clone(), condition.clone())
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create filter node: {}", e))
            })
            .map(PlanNodeEnum::Filter)
    }

    /// Use the ORDER BY clause to sort the data.
    ///
    /// Convert the sorting factors in the OrderByClauseContext into the names of the sorting fields and the direction of sorting.
    fn apply_order_by(
        &self,
        input_plan: SubPlan,
        order_by_ctx: &OrderByClauseContext,
    ) -> Result<SubPlan, PlannerError> {
        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        // Obtain the column names of the input node.
        let col_names = input_node.col_names();

        // Convert the index sorting factor into sorting items (which include column names and the direction of sorting).
        // It is assumed that the indices correspond to the positions in the list of column names.
        // If the index is out of range, use a placeholder name.
        let sort_items: Vec<crate::query::planning::plan::core::nodes::SortItem> = order_by_ctx
            .indexed_order_factors
            .iter()
            .map(|(idx, dir)| {
                let column = col_names
                    .get(*idx)
                    .cloned()
                    .unwrap_or_else(|| format!("col_{}", idx));
                crate::query::planning::plan::core::nodes::SortItem::column(column, *dir)
            })
            .collect();

        if sort_items.is_empty() {
            // If there are no valid sorting criteria, simply return the input plan as is.
            return Ok(input_plan);
        }

        // Create a sorting node.
        let sort_node = crate::query::planning::plan::core::nodes::SortNode::new(
            input_node.clone(),
            sort_items,
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create sort node: {}", e))
        })?;

        Ok(SubPlan::new(
            Some(PlanNodeEnum::Sort(sort_node)),
            input_plan.tail.clone(),
        ))
    }

    /// Application pagination
    fn apply_pagination(
        &self,
        input_plan: SubPlan,
        pagination: &PaginationContext,
    ) -> Result<SubPlan, PlannerError> {
        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        let limit_node = LimitNode::new(input_node.clone(), pagination.skip, pagination.limit)
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create paging node: {}", e))
            })?;

        Ok(SubPlan::new(
            Some(PlanNodeEnum::Limit(limit_node)),
            input_plan.tail.clone(),
        ))
    }

    /// Using the DISTINCT keyword (to remove duplicates)
    fn apply_distinct(&self, input_plan: SubPlan) -> Result<SubPlan, PlannerError> {
        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        // Create a deduplication node (using a simplified version of AggregateNode)
        let dedup_node = crate::query::planning::plan::core::nodes::DedupNode::new(
            input_node.clone(),
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!(
                "Failed to create de-duplicated node: {}",
                e
            ))
        })?;

        Ok(SubPlan::new(
            Some(PlanNodeEnum::Dedup(dedup_node)),
            input_plan.tail.clone(),
        ))
    }
}

impl ClausePlanner for WithClausePlanner {
    fn clause_kind(&self) -> CypherClauseKind {
        CypherClauseKind::With
    }

    fn transform_clause(
        &self,
        _qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let with_ctx = Self::extract_with_context(stmt)?;
        self.plan_with_clause(&with_ctx, &input_plan)
    }
}

impl WithClausePlanner {
    /// Extract the context of the WITH clause from the sentence.
    ///
    /// The improved implementation includes:
    /// - Extract the complete information of the WITH clause from Stmt::With.
    /// - Constructing the YieldClauseContext
    /// - Handling ORDER BY and pagination
    /// - Collecting information about aliases
    /// - Handling aggregate expressions and grouping keys
    fn extract_with_context(stmt: &Stmt) -> Result<WithClauseContext, PlannerError> {
        use crate::core::YieldColumn;
        use crate::query::parser::ast::Stmt;
        use crate::query::validator::structs::{
            OrderByClauseContext, PaginationContext, YieldClauseContext,
        };

        let with_stmt = match stmt {
            Stmt::With(w) => w,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "Expecting a WITH statement, but getting other types of statements".to_string(),
                ));
            }
        };

        // Convert `ReturnItem` to `YieldColumn`
        let mut yield_columns = Vec::new();
        let mut has_agg = false;
        let mut aliases_generated = HashMap::new();

        for item in &with_stmt.items {
            match item {
                crate::query::parser::ast::stmt::ReturnItem::Expression { expression, alias } => {
                    let col_alias = alias
                        .clone()
                        .unwrap_or_else(|| Self::generate_default_alias(expression));

                    yield_columns.push(YieldColumn {
                        expression: expression.clone(),
                        alias: col_alias.clone(),
                        is_matched: false,
                    });

                    // Collect the generated aliases.
                    if !col_alias.is_empty() && col_alias != "*" {
                        let alias_type = Self::deduce_alias_type(expression);
                        aliases_generated.insert(col_alias, alias_type);
                    }

                    if expression.contains_aggregate() {
                        has_agg = true;
                    }
                }
            }
        }

        // Extract the group keys and the aggregation items.
        let (group_keys, group_items) = if has_agg {
            extract_group_info(&yield_columns)
        } else {
            (vec![], vec![])
        };

        // Constructing the context for the ORDER BY clause
        let order_by = with_stmt
            .order_by
            .as_ref()
            .map(|order| OrderByClauseContext {
                indexed_order_factors: order
                    .items
                    .iter()
                    .enumerate()
                    .map(|(idx, item)| {
                        let direction = match item.direction {
                            crate::core::types::OrderDirection::Asc => {
                                crate::core::types::graph_schema::OrderDirection::Asc
                            }
                            crate::core::types::OrderDirection::Desc => {
                                crate::core::types::graph_schema::OrderDirection::Desc
                            }
                        };
                        (idx, direction)
                    })
                    .collect(),
            });

        // Building the pagination context
        let pagination = if with_stmt.skip.is_some() || with_stmt.limit.is_some() {
            Some(PaginationContext {
                skip: with_stmt.skip.unwrap_or(0) as i64,
                limit: with_stmt.limit.unwrap_or(0) as i64,
            })
        } else {
            None
        };

        // Constructing a YieldClauseContext
        let yield_clause = YieldClauseContext {
            yield_columns: yield_columns.clone(),
            aliases_available: HashMap::new(), // The aliases obtained from the input plan are filled in during the planning phase.
            aliases_generated: aliases_generated.clone(),
            distinct: with_stmt.distinct,
            has_agg,
            group_keys: group_keys.clone(),
            group_items: group_items.clone(),
            need_gen_project: has_agg,
            agg_output_column_names: vec![],
            proj_output_column_names: vec![],
            paths: vec![],
            query_parts: vec![],
            errors: vec![],
            filter_condition: with_stmt.where_clause.clone(),
            skip: with_stmt.skip,
            limit: with_stmt.limit,
        };

        Ok(WithClauseContext {
            yield_clause,
            aliases_available: HashMap::new(), // The aliases obtained from the input plan are filled in during the planning phase.
            aliases_generated,
            where_clause: with_stmt.where_clause.clone().map(|condition| {
                crate::query::validator::structs::WhereClauseContext {
                    filter: Some(condition),
                    aliases_available: HashMap::new(),
                    aliases_generated: HashMap::new(),
                    paths: vec![],
                    query_parts: vec![],
                    errors: vec![],
                }
            }),
            pagination,
            order_by,
            distinct: with_stmt.distinct,
            query_parts: vec![],
            errors: vec![],
        })
    }

    /// Determine the type of alias
    ///
    /// Determine the alias type based on the expression.
    /// Refer to the implementation of DeduceAliasTypeVisitor in NebulaGraph.
    fn deduce_alias_type(
        expression: &crate::core::types::expr::contextual::ContextualExpression,
    ) -> AliasType {
        Self::deduce_alias_type_from_contextual(expression)
    }

    /// Infer the alias type from the ContextualExpression (auxiliary method)
    fn deduce_alias_type_from_contextual(
        expression: &crate::core::types::expr::contextual::ContextualExpression,
    ) -> AliasType {
        // For most expressions, it is not possible to determine their type; therefore, the default value returned is “Runtime”.
        if expression.is_literal()
            || expression.is_unary()
            || expression.is_type_cast()
            || expression.is_label()
            || expression.is_binary()
            || expression.is_aggregate()
            || expression.is_list()
            || expression.is_map()
            || expression.is_case()
            || expression.is_reduce()
            || expression.is_parameter()
            || expression.is_list_comprehension()
        {
            return AliasType::Runtime;
        }

        // Variable reference: By default, the Variable is returned; the actual type must be obtained from aliases_available.
        if expression.is_variable() {
            return AliasType::Variable;
        }

        // Property access – Attempting to infer the type of an object from its properties
        if expression.is_property() {
            // The type of attribute access cannot be determined directly; it is necessary to obtain this information from the `aliases_available` source.
            return AliasType::Runtime;
        }

        // Path construction expressions
        if expression.is_path_build() || expression.is_path() {
            return AliasType::Path;
        }

        // Function call – Inferring the type based on the function name
        if let Some(name) = expression.as_function_name() {
            let name_lower = name.to_lowercase();
            match name_lower.as_str() {
                "nodes" => return AliasType::NodeList,
                "relationships" => return AliasType::EdgeList,
                "reversepath" => return AliasType::Path,
                "startnode" | "endnode" => return AliasType::Node,
                _ => return AliasType::Runtime,
            }
        }

        // Subscript access – Recursive inference of set types
        if expression.is_subscript() {
            // The type of access via the subscript cannot be determined directly; it is necessary to obtain this information from the `aliases_available` list.
            return AliasType::Runtime;
        }

        // Range expressions – Recursive inference of set types
        if expression.is_range() {
            // The type of the range expression cannot be determined directly; it is necessary to obtain the information from `aliases_available`.
            return AliasType::Runtime;
        }

        AliasType::Runtime
    }

    /// Generate default aliases
    fn generate_default_alias(
        expression: &crate::core::types::expr::contextual::ContextualExpression,
    ) -> String {
        use crate::core::Expression;

        if let Some(e) = expression.get_expression() {
            match e {
                Expression::Variable(name) => name.clone(),
                Expression::Property { object, property } => {
                    if let Expression::Variable(name) = object.as_ref() {
                        format!("{}.{}", name, property)
                    } else {
                        "expr".to_string()
                    }
                }
                Expression::Function { name, .. } => name.clone(),
                Expression::Aggregate { func, .. } => format!("{:?}", func).to_lowercase(),
                _ => "expr".to_string(),
            }
        } else {
            "expr".to_string()
        }
    }
}

impl Default for WithClausePlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_clause_planner_creation() {
        let planner = WithClausePlanner::new();
        assert_eq!(planner.clause_kind(), CypherClauseKind::With);
    }
}
