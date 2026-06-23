//! Implementation of sorting nodes
//!
//! The `SortNode` is used to perform sorting operations on the input data.

use crate::core::types::graph_schema::OrderDirection;
use crate::core::Expression;
use crate::define_plan_node_with_deps;

/// Sorting item definition
/// Includes expression and sorting direction.
/// Supports both simple column references and complex expressions (e.g., function calls).
#[derive(Debug, Clone, PartialEq)]
pub struct SortItem {
    /// Sort expression (can be a column reference or a complex expression)
    pub expression: Expression,
    /// Sorting direction
    pub direction: OrderDirection,
}

impl SortItem {
    /// Create a new sorting item with an expression.
    pub fn new(expression: Expression, direction: OrderDirection) -> Self {
        Self {
            expression,
            direction,
        }
    }

    /// Create items for ascending sorting with an expression.
    pub fn asc(expression: Expression) -> Self {
        Self::new(expression, OrderDirection::Asc)
    }

    /// Create descending order sorting items with an expression.
    pub fn desc(expression: Expression) -> Self {
        Self::new(expression, OrderDirection::Desc)
    }

    /// Create a sorting item from a column name (convenience method).
    pub fn column(column: String, direction: OrderDirection) -> Self {
        Self::new(Expression::Variable(column), direction)
    }

    /// Create an ascending sorting item from a column name.
    pub fn column_asc(column: String) -> Self {
        Self::column(column, OrderDirection::Asc)
    }

    /// Create a descending sorting item from a column name.
    pub fn column_desc(column: String) -> Self {
        Self::column(column, OrderDirection::Desc)
    }

    /// Get the column name if the expression is a simple variable reference.
    /// Returns None for complex expressions (function calls, property access, etc.)
    pub fn column_name(&self) -> Option<&str> {
        match &self.expression {
            Expression::Variable(name) => Some(name),
            _ => None,
        }
    }
}

define_plan_node_with_deps! {
    pub struct SortNode {
        sort_items: Vec<SortItem>,
        limit: Option<i64>,
    }
    enum: Sort
    input: SingleInputNode
}

impl SortNode {
    /// Create a new sorting node.
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        sort_items: Vec<SortItem>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            sort_items,
            limit: None,
            output_var: None,
            col_names,
        })
    }

    /// Obtain the sorted fields
    pub fn sort_items(&self) -> &[SortItem] {
        &self.sort_items
    }

    /// Obtain a limited quantity.
    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    /// Set a limit on the number of items.
    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }
}

define_plan_node_with_deps! {
    pub struct LimitNode {
        offset: i64,
        count: i64,
    }
    enum: Limit
    input: SingleInputNode
}

impl LimitNode {
    /// Create a new restriction node.
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        offset: i64,
        count: i64,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            offset,
            count,
            output_var: None,
            col_names,
        })
    }

    /// Obtain the offset value.
    pub fn offset(&self) -> i64 {
        self.offset
    }

    /// Obtain the count.
    pub fn count(&self) -> i64 {
        self.count
    }
}

define_plan_node_with_deps! {
    pub struct TopNNode {
        sort_items: Vec<SortItem>,
        limit: i64,
    }
    enum: TopN
    input: SingleInputNode
}

impl TopNNode {
    /// Create new TopN nodes.
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        sort_items: Vec<SortItem>,
        limit: i64,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            sort_items,
            limit,
            output_var: None,
            col_names,
        })
    }

    /// Get Sorted Fields
    pub fn sort_items(&self) -> &[SortItem] {
        &self.sort_items
    }

    /// Access to restricted quantities
    pub fn limit(&self) -> i64 {
        self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;

    #[test]
    fn test_sort_node_creation() {
        let start_node = PlanNodeEnum::Start(StartNode::new());

        let sort_items = vec![
            SortItem::column_asc("name".to_string()),
            SortItem::column_desc("age".to_string()),
        ];

        let sort_node =
            SortNode::new(start_node, sort_items).expect("SortNode creation should succeed");

        assert_eq!(sort_node.type_name(), "SortNode");
        assert_eq!(sort_node.dependencies().len(), 1);
        assert_eq!(sort_node.sort_items().len(), 2);
        assert_eq!(sort_node.sort_items()[0].direction, OrderDirection::Asc);
        assert_eq!(sort_node.sort_items()[1].direction, OrderDirection::Desc);
    }

    #[test]
    fn test_sort_node_with_expression() {
        let start_node = PlanNodeEnum::Start(StartNode::new());

        // Create a sort item with a function expression
        let func_expr = Expression::Function {
            name: "cosine_similarity".to_string(),
            args: vec![
                Expression::Property {
                    object: Box::new(Expression::Variable("p".to_string())),
                    property: "embedding".to_string(),
                },
                Expression::Literal(crate::core::Value::List(Box::new(
                    crate::core::value::list::List {
                        values: vec![
                            crate::core::Value::Double(0.1),
                            crate::core::Value::Double(0.2),
                        ],
                    },
                ))),
            ],
        };

        let sort_items = vec![SortItem::desc(func_expr)];

        let sort_node =
            SortNode::new(start_node, sort_items).expect("SortNode creation should succeed");

        assert_eq!(sort_node.type_name(), "SortNode");
        assert_eq!(sort_node.sort_items().len(), 1);
        assert_eq!(sort_node.sort_items()[0].direction, OrderDirection::Desc);
    }

    #[test]
    fn test_limit_node_creation() {
        let start_node = PlanNodeEnum::Start(StartNode::new());

        let limit_node =
            LimitNode::new(start_node, 10, 100).expect("Limit node should be created successfully");

        assert_eq!(limit_node.type_name(), "LimitNode");
        assert_eq!(limit_node.dependencies().len(), 1);
        assert_eq!(limit_node.offset(), 10);
        assert_eq!(limit_node.count(), 100);
    }

    #[test]
    fn test_topn_node_creation() {
        let start_node = PlanNodeEnum::Start(StartNode::new());

        let sort_items = vec![
            SortItem::column_asc("name".to_string()),
            SortItem::column_desc("age".to_string()),
        ];
        let topn_node = TopNNode::new(start_node, sort_items, 10)
            .expect("TopN node should be created successfully");

        assert_eq!(topn_node.type_name(), "TopNNode");
        assert_eq!(topn_node.dependencies().len(), 1);
        assert_eq!(topn_node.sort_items().len(), 2);
        assert_eq!(topn_node.limit(), 10);
        assert_eq!(topn_node.sort_items()[0].direction, OrderDirection::Asc);
        assert_eq!(topn_node.sort_items()[1].direction, OrderDirection::Desc);
    }
}
