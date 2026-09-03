//! Immutable configuration for blocking operators.

use crate::executor::streaming::executor::SortDirection;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::operators::AggregateFunction;

/// Immutable config for blocking operators.
#[derive(Debug, Clone)]
pub enum BlockingSpec {
    Sort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
    },
    Aggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<(AggregateFunction, Vec<Expression>)>,
        output_col_names: Vec<String>,
    },
    GroupBy {
        group_by_expressions: Vec<Expression>,
    },
    WindowFunction {
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
    },
    Window {
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
    },
    TopN {
        n: u32,
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
    },
    Distinct,
    Materialize,
    DataCollect,
    RollUpApply {
        rollup_expressions: Vec<Expression>,
    },
    PartialAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<(AggregateFunction, Vec<Expression>)>,
        output_col_names: Vec<String>,
    },
    FinalAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<(AggregateFunction, Vec<Expression>)>,
        output_col_names: Vec<String>,
    },
}
