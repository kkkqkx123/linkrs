use std::sync::Arc;

use crate::executor::streaming::instance::QueryBindings;
use crate::executor::streaming::plan::types::PhysicalPlan;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::types::expr::SubqueryBody;
use linkrs_core::Value;

use super::runner::SubqueryCache;
use super::*;

use crate::executor::base::{ExecutionContext, MemoryBudget};
use crate::executor::streaming::plan::arena_builder::PhysicalPlanBuilder;
use crate::executor::streaming::plan::context::PhysicalPlanBuildContext;
use crate::executor::streaming::plan::validator::PhysicalPlanValidator;
use crate::executor::streaming::runtime::QueryIdentity;
use crate::executor::streaming::transaction_scope::TransactionScope;
use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
use crate::planning::plan::core::nodes::control_flow::ArgumentNode;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode;
use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::planning::plan::PlanNodeEnum;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::expr::ExpressionMeta;
use linkrs_core::types::ContextualExpression;
use std::collections::HashMap;

fn build_plan(node: &PlanNodeEnum) -> Arc<PhysicalPlan> {
    let mut ctx = PhysicalPlanBuildContext::new();
    let exec_ctx = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
    let plan =
        Arc::new(PhysicalPlanBuilder::build(node, &mut ctx, &exec_ctx).expect("plan should build"));
    PhysicalPlanValidator::validate(&plan).expect("plan should validate");
    plan
}

fn contextual(expr: linkrs_core::Expression) -> ContextualExpression {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let id = ctx.register_expression(ExpressionMeta::new(expr));
    ContextualExpression::new(id, ctx)
}

/// A subquery plan producing one row per element of `values` in column 0.
fn values_subquery_plan(values: Vec<Value>) -> Arc<PhysicalPlan> {
    let start = PlanNodeEnum::Start(StartNode::new());
    let list = values
        .into_iter()
        .map(linkrs_core::Expression::Literal)
        .collect();
    let unwind = UnwindNode::new(start, "v", contextual(linkrs_core::Expression::list(list)))
        .expect("unwind should build");
    build_plan(&PlanNodeEnum::Unwind(unwind))
}

/// A correlated subquery plan that echoes the frame's `x` column.
fn correlated_echo_plan() -> Arc<PhysicalPlan> {
    let mut argument = ArgumentNode::new(-2, "_arg");
    argument.set_col_names(vec!["x".to_string()]);
    let project = ProjectNode::new(
        PlanNodeEnum::Argument(argument),
        vec![linkrs_core::YieldColumn::new(
            contextual(linkrs_core::Expression::variable("x")),
            "x".to_string(),
        )],
    )
    .expect("project should build");
    build_plan(&PlanNodeEnum::Project(project))
}

/// A Group-Join build side: `Unwind(v over values) -> Aggregate(group_by
/// v, sum(v))`. Output rows are `[v, sum]`, so the table maps each value
/// to its duplicated-sum (e.g. `[10, 10, 20]` → `{10: 20, 20: 20}`).
fn group_join_plan(values: Vec<i32>) -> Arc<PhysicalPlan> {
    use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
    use linkrs_core::types::operators::AggregateFunction;

    let start = PlanNodeEnum::Start(StartNode::new());
    let list = values
        .into_iter()
        .map(|v| linkrs_core::Expression::Literal(Value::Int(v)))
        .collect();
    let unwind = UnwindNode::new(start, "v", contextual(linkrs_core::Expression::list(list)))
        .expect("unwind should build");
    let mut aggregate = AggregateNode::new(
        PlanNodeEnum::Unwind(unwind),
        vec!["v".to_string()],
        vec![AggregateFunction::Sum],
    )
    .expect("aggregate should build");
    aggregate.set_aggregation_args(vec![vec![linkrs_core::Expression::variable("v")]]);
    build_plan(&PlanNodeEnum::Aggregate(aggregate))
}

fn test_executor(specs: Vec<SubqueryRunnerSpec>) -> SubqueryExecutor {
    let runtime = Arc::new(ExecutionRuntime::new(
        QueryIdentity::default(),
        MemoryBudget::new(1024 * 1024),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    let bindings = Arc::new(QueryBindings {
        parameters: Arc::new(HashMap::new()),
        session_variables: Arc::new(HashMap::new()),
        parameter_frame: None,
        space_name: None,
        storage: None,
        bound_snapshot: None,
        memory_budget: MemoryBudget::new(1024 * 1024),
        max_workers: 1,
        chunk_size: 2048,
        max_buffered_chunks: 4,
        query_id: 1,
        cancel_token: None,
        session_id: None,
        user_name: None,
        query_text: None,
        transaction: TransactionScope::None,
        shared_scheduler: None,
        partition_count: 0,
        arena: None,
        feedback_history: None,
        columnar_policy: None,
        macro_manager: None,
        type_alias_manager: None,
        sequence_manager: None,
        search: crate::executor::base::SearchContext::default(),
    });
    SubqueryExecutor::from_specs(runtime, bindings, &specs)
}

fn body(id: u64) -> SubqueryBody {
    SubqueryBody {
        id,
        patterns: Vec::new(),
        where_clause: None,
        return_expr: None,
    }
}

fn empty_layout() -> Arc<SlotLayout> {
    Arc::new(SlotLayout::new(vec![]))
}

#[test]
fn non_correlated_exists_caches_and_reuses_executor() {
    let plan = values_subquery_plan(vec![Value::Int(1), Value::Int(2), Value::Int(1)]);
    let executor = test_executor(vec![SubqueryRunnerSpec {
        id: 7,
        plan,
        correlated: false,
        group_join: None,
    }]);
    let b = body(7);

    let first = executor
        .execute_exists(&b, empty_layout(), Vec::new())
        .expect("exists should run");
    assert!(first, "non-empty subquery exists == true");

    // The runner is materialized exactly once and the result cached.
    let runner = executor.runners.get(&7).expect("runner present");
    assert!(runner.executor.lock().is_some(), "materialized once");
    assert!(
        matches!(&*runner.cache.lock(), Some(SubqueryCache::Exists(true))),
        "EXISTS result cached"
    );

    // Second call hits the cache without re-running.
    let second = executor
        .execute_exists(&b, empty_layout(), Vec::new())
        .expect("cached exists");
    assert!(second);
}

#[test]
fn non_correlated_in_uses_hash_set_cache() {
    let plan = values_subquery_plan(vec![Value::Int(1), Value::Int(2), Value::Int(1)]);
    let executor = test_executor(vec![SubqueryRunnerSpec {
        id: 8,
        plan,
        correlated: false,
        group_join: None,
    }]);
    let b = body(8);

    assert!(executor
        .execute_contains(&b, empty_layout(), Vec::new(), &Value::Int(1))
        .expect("contains 1"));
    assert!(!executor
        .execute_contains(&b, empty_layout(), Vec::new(), &Value::Int(3))
        .expect("not contains 3"));

    let runner = executor.runners.get(&8).expect("runner present");
    let cache = runner.cache.lock();
    let SubqueryCache::Contains(set) = cache.as_ref().expect("IN result cached") else {
        panic!("IN result must be cached as a HashSet");
    };
    assert_eq!(set.len(), 2, "duplicates and NULLs removed at collection");
}

#[test]
fn null_never_matches_and_skips_the_cache() {
    let plan = values_subquery_plan(vec![
        Value::Int(1),
        Value::Null(linkrs_core::value::NullType::Null),
    ]);
    let executor = test_executor(vec![SubqueryRunnerSpec {
        id: 9,
        plan,
        correlated: false,
        group_join: None,
    }]);
    let b = body(9);

    // NULL left operand never matches and must not execute the
    // subquery (the cache stays empty).
    assert!(!executor
        .execute_contains(
            &b,
            empty_layout(),
            Vec::new(),
            &Value::Null(linkrs_core::value::NullType::Null),
        )
        .expect("null never matches"));
    assert!(executor.runners.get(&9).unwrap().cache.lock().is_none());

    // A non-NULL value matching the non-NULL result element still hits.
    assert!(executor
        .execute_contains(&b, empty_layout(), Vec::new(), &Value::Int(1))
        .expect("value found despite NULL in the result set"));
}

#[test]
fn correlated_runner_reuses_executor_with_per_row_frames() {
    let plan = correlated_echo_plan();
    let executor = test_executor(vec![SubqueryRunnerSpec {
        id: 10,
        plan,
        correlated: true,
        group_join: None,
    }]);
    let b = body(10);
    let layout = Arc::new(SlotLayout::from_names(&["x".to_string()]));

    // First frame: x = 7.
    assert!(executor
        .execute_contains(&b, layout.clone(), vec![Value::Int(7)], &Value::Int(7),)
        .expect("x=7 contains 7"));
    // Same runner, new frame: x = 8 — the executor was reset, not
    // rebuilt, and the old frame is gone.
    assert!(!executor
        .execute_contains(&b, layout.clone(), vec![Value::Int(8)], &Value::Int(7))
        .expect("x=8 does not contain 7"));
    assert!(executor
        .execute_contains(&b, layout, vec![Value::Int(8)], &Value::Int(8))
        .expect("x=8 contains 8"));

    let runner = executor.runners.get(&10).expect("runner present");
    assert!(
        runner.executor.lock().is_some(),
        "correlated runner materializes once"
    );
    assert!(
        runner.cache.lock().is_none(),
        "correlated results are never cached"
    );
}

#[test]
fn missing_runner_reports_precise_error() {
    let executor = test_executor(Vec::new());
    let err = executor
        .execute_exists(&body(99), empty_layout(), Vec::new())
        .expect_err("no runner for id 99");
    assert!(
        err.message.contains("not supported"),
        "expected the last-resort error, got: {}",
        err.message
    );
}

fn group_join_spec(id: u64, values: Vec<i32>) -> SubqueryRunnerSpec {
    use linkrs_core::types::operators::AggregateFunction;
    SubqueryRunnerSpec {
        id,
        plan: group_join_plan(values),
        correlated: false,
        group_join: Some(GroupJoinSpec {
            hash_keys: vec![linkrs_core::Expression::variable("x")],
            key_columns: 1,
            function: AggregateFunction::Sum,
            distinct: false,
        }),
    }
}

fn x_layout() -> Arc<SlotLayout> {
    Arc::new(SlotLayout::from_names(&["x".to_string()]))
}

#[test]
fn group_join_exists_and_contains_backfill_from_table() {
    // Rows [10, 10, 20] → table {10: 20, 20: 20}.
    let executor = test_executor(vec![group_join_spec(20, vec![10, 10, 20])]);
    let b = body(20);
    let layout = x_layout();

    // EXISTS: the key is present iff at least one right row matched.
    assert!(executor
        .execute_exists(&b, layout.clone(), vec![Value::Int(10)])
        .expect("key 10 present"));
    assert!(executor
        .execute_exists(&b, layout.clone(), vec![Value::Int(20)])
        .expect("key 20 present"));
    assert!(!executor
        .execute_exists(&b, layout.clone(), vec![Value::Int(30)])
        .expect("key 30 absent"));

    // IN: compared against the aggregated value of the row's key.
    assert!(executor
        .execute_contains(&b, layout.clone(), vec![Value::Int(10)], &Value::Int(20))
        .expect("sum(10) == 20"));
    assert!(!executor
        .execute_contains(&b, layout.clone(), vec![Value::Int(10)], &Value::Int(10))
        .expect("sum(10) != 10"));
    assert!(!executor
        .execute_contains(&b, layout.clone(), vec![Value::Int(30)], &Value::Int(30))
        .expect("missing key never matches"));

    // The table was materialized exactly once.
    let runner = executor.runners.get(&20).expect("runner present");
    assert!(
        runner.executor.lock().is_none(),
        "Group-Join build side is drained locally, not stored"
    );
}

#[test]
fn group_join_null_key_never_matches_and_missing_key_yields_false() {
    // Empty right side → empty table; every lookup misses.
    let executor = test_executor(vec![group_join_spec(21, Vec::new())]);
    let b = body(21);
    let layout = x_layout();

    assert!(!executor
        .execute_exists(&b, layout.clone(), vec![Value::Int(1)])
        .expect("empty table misses"));
    // A NULL outer key component never matches (keys_match convention).
    assert!(!executor
        .execute_exists(
            &b,
            layout.clone(),
            vec![Value::Null(linkrs_core::NullType::Null)]
        )
        .expect("null key never matches"));
    assert!(!executor
        .execute_contains(&b, layout, vec![Value::Int(1)], &Value::Int(2))
        .expect("contains on empty table"));
}
