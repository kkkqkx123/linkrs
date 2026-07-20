//! PhysicalPlanMaterializer: converts an arena [`PhysicalPlan`] into a
//! [`StreamingExecutor`] tree.
//!
//! This is the single instantiation path for all production queries:
//!
//! ```text
//! Arc<PhysicalPlan> + QueryBindings
//!   -> validate
//!   -> allocate runtime + state arenas
//!   -> build operator tree per fragment
//!   -> return root StreamingExecutor
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use super::super::executor::StreamingExecutor;
use super::super::operators::apply_operator::ApplyOperator;
use super::super::operators::base::OperatorBase;
use super::super::operators::blocking::BlockingOperator;
use super::super::operators::ddl_operator::DdlOperator;
use super::super::operators::exchange_operator::ExchangeOperator;
use super::super::operators::fulltext_operator::FulltextOperator;
use super::super::operators::graph_operator::GraphOperator;
use super::super::operators::join_operator::JoinOperator;
use super::super::operators::recursive_fragment_operator::RecursiveFragmentOperator;
use super::super::operators::set_operator::SetOperator;
use super::super::operators::sink_operator::SinkOperator;
use super::super::operators::source_operator::SourceOperator;
use super::super::operators::txn_operator::TxnOperator;
use super::super::operators::unary_operator::UnaryOperator;
use super::super::operators::vector_operator::VectorOperator;
use super::super::runtime::ExecutionRuntime;
use super::types::{FragmentGraph, FragmentId, FragmentSpec, OperatorKindSpec, PhysicalPlan};
use crate::core::error::QueryError;

use super::super::instance::QueryBindings;

/// Converts an arena [`PhysicalPlan`] into a [`StreamingExecutor`] tree.
///
/// Processing order:
/// 1. Validate bindings and capabilities.
/// 2. Create [`ExecutionRuntime`] from bindings.
/// 3. Walk fragment graph in topological order (producers before consumers).
/// 4. For each fragment, build the operator pipeline from its arena specs.
/// 5. Connect fragment inputs by moving them from the producer map.
/// 6. Return root executor.
pub struct PhysicalPlanMaterializer;

impl PhysicalPlanMaterializer {
    /// Materialize a physical plan into an executable operator tree.
    ///
    /// M1.4: validates bindings and builds the [`ParameterFrame`] for
    /// slot-based parameter access during execution.
    pub fn materialize(
        plan: &PhysicalPlan,
        bindings: &QueryBindings,
    ) -> Result<(StreamingExecutor, Arc<ExecutionRuntime>), QueryError> {
        Self::validate_bindings(plan, bindings)?;

        // M1.4: build the parameter frame — this requires a mutable bindings
        // clone to set the frame.  We clone because the caller's bindings
        // are immutable past this point.
        let mut mutable_bindings = bindings.clone();
        if !plan.parameter_schema.is_empty() {
            mutable_bindings.build_parameter_frame(&plan.parameter_schema);
        }

        let runtime = Self::create_runtime(&mutable_bindings);

        let topo_order = Self::topological_order(&plan.fragments)?;

        // fragment id → root executor (owned, moved out when consumed).
        let mut fragment_roots: HashMap<FragmentId, StreamingExecutor> = HashMap::new();

        for &fid in &topo_order {
            let fragment = plan
                .fragments
                .get(fid)
                .ok_or_else(|| QueryError::execution(format!("Fragment {:?} not found", fid)))?;

            let executor = Self::build_fragment_pipeline(
                fragment,
                plan,
                &mut fragment_roots,
                &runtime,
                &mutable_bindings,
            )?;
            fragment_roots.insert(fid, executor);
        }

        let root_executor = fragment_roots.remove(&plan.root_fragment).ok_or_else(|| {
            QueryError::execution(format!(
                "Root fragment {:?} has no materialized executor",
                plan.root_fragment
            ))
        })?;

        Ok((root_executor, runtime))
    }

    // ── Fragment pipeline construction ──

    /// Build the operator tree for a single fragment.
    ///
    /// Within a fragment, operators form a linear pipeline.  The `operators`
    /// list is processed with a stack:
    ///
    /// - Source: push
    /// - Unary/Blocking/Graph/Sink/Ddl/Fulltext/Vector/Txn: pop 1, wrap, push
    /// - Join/Set/Apply: pop 2, wrap, push
    /// - Exchange: takes all remaining stack items
    ///
    /// Fragment inputs (producer fragment roots) are placed on the stack
    /// before any operators are processed, serving as initial leaf values.
    fn build_fragment_pipeline(
        fragment: &FragmentSpec,
        plan: &PhysicalPlan,
        fragment_roots: &mut HashMap<FragmentId, StreamingExecutor>,
        runtime: &Arc<ExecutionRuntime>,
        bindings: &QueryBindings,
    ) -> Result<StreamingExecutor, QueryError> {
        // ── Initial stack: input fragment roots ──
        // Producers are removed from the map (ownership transfer).
        let mut stack: Vec<StreamingExecutor> = {
            let mut v = Vec::new();
            for input_id in &fragment.inputs {
                if let Some(root) = fragment_roots.remove(input_id) {
                    v.push(root);
                }
            }
            v
        };

        // ── Process operators leaf → root ──
        for &op_id in &fragment.operators {
            let op_spec = plan.operator(op_id).ok_or_else(|| {
                QueryError::execution(format!("Operator {:?} not found in plan arena", op_id))
            })?;

            let plan_node_id = op_spec
                .logical_node_id
                .map(|logical_id| logical_id.0)
                .unwrap_or(op_spec.operator_id.0 as i64);
            let base = OperatorBase::new(plan_node_id)
                .with_physical_operator_id(op_spec.operator_id)
                .with_output_layout(Arc::new(op_spec.output_layout.clone()));

            let exec = match &op_spec.spec {
                OperatorKindSpec::Source(src_spec) => {
                    let storage = bindings.storage.clone();
                    let op = SourceOperator::from_spec(src_spec, storage);
                    StreamingExecutor::Source(base, op)
                }
                OperatorKindSpec::Unary(unary_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let op = UnaryOperator::from_spec(unary_spec);
                    StreamingExecutor::Unary(base, Box::new(child), op)
                }
                OperatorKindSpec::Blocking(blocking_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let op = BlockingOperator::from_spec(blocking_spec, &bindings.memory_budget);
                    StreamingExecutor::Blocking(base, Box::new(child), op)
                }
                OperatorKindSpec::Join(join_spec) => {
                    let right = pop_stack_or_err(&mut stack)?;
                    let left = pop_stack_or_err(&mut stack)?;
                    let op = JoinOperator::from_spec(join_spec, &bindings.memory_budget);
                    StreamingExecutor::Join(base, Box::new(left), Box::new(right), op)
                }
                OperatorKindSpec::Graph(graph_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let storage = runtime.storage.clone();
                    let space_name = runtime.query_id().space_name.clone().unwrap_or_default();
                    let op = GraphOperator::from_spec(graph_spec, storage, space_name);
                    StreamingExecutor::Graph(base, Box::new(child), op)
                }
                OperatorKindSpec::RecursiveFragment(rf_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let storage = runtime.storage.clone();
                    let space_name = runtime.query_id().space_name.clone().unwrap_or_default();
                    let op = RecursiveFragmentOperator::from_spec(rf_spec, storage, space_name);
                    StreamingExecutor::RecursiveFragment(base, Box::new(child), op)
                }
                OperatorKindSpec::Sink(sink_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let storage = runtime.storage.clone();
                    let op = SinkOperator::from_spec(sink_spec, storage);
                    StreamingExecutor::Sink(base, Box::new(child), op)
                }
                OperatorKindSpec::Set(set_spec) => {
                    let right = pop_stack_or_err(&mut stack)?;
                    let left = pop_stack_or_err(&mut stack)?;
                    let op = SetOperator::from_spec(set_spec, &bindings.memory_budget);
                    StreamingExecutor::Set(base, Box::new(left), Box::new(right), op)
                }
                OperatorKindSpec::Apply(apply_spec) => {
                    let right = pop_stack_or_err(&mut stack)?;
                    let left = pop_stack_or_err(&mut stack)?;
                    let op = ApplyOperator::from_spec(apply_spec, &bindings.memory_budget);
                    StreamingExecutor::Apply(base, Box::new(left), Box::new(right), op)
                }
                OperatorKindSpec::Exchange(exchange_spec) => {
                    let children = std::mem::take(&mut stack);
                    let op = ExchangeOperator::from_spec(exchange_spec);
                    StreamingExecutor::Exchange(base, children, op)
                }
                OperatorKindSpec::Ddl(ddl_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let storage = runtime.storage.clone();
                    let op = DdlOperator::from_spec(ddl_spec, storage);
                    StreamingExecutor::Ddl(base, Box::new(child), op)
                }
                OperatorKindSpec::Fulltext(ft_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let storage = runtime.storage.clone();
                    #[cfg(feature = "fulltext-search")]
                    let ft_mgr = runtime.fulltext_manager.clone();
                    let op = FulltextOperator::from_spec(
                        ft_spec,
                        storage,
                        #[cfg(feature = "fulltext-search")]
                        ft_mgr,
                    );
                    StreamingExecutor::Fulltext(base, Box::new(child), op)
                }
                OperatorKindSpec::Vector(vector_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let storage = runtime.storage.clone();
                    #[cfg(feature = "qdrant")]
                    let coord = runtime.vector_coordinator.clone();
                    let op = VectorOperator::from_spec(
                        vector_spec,
                        storage,
                        #[cfg(feature = "qdrant")]
                        coord,
                    );
                    StreamingExecutor::Vector(base, Box::new(child), op)
                }
                OperatorKindSpec::Txn(txn_spec) => {
                    let child = pop_stack_or_err(&mut stack)?;
                    let op = TxnOperator::from_spec(txn_spec);
                    StreamingExecutor::Txn(base, Box::new(child), op)
                }
            };

            stack.push(exec);
        }

        if stack.is_empty() {
            return Err(QueryError::execution(format!(
                "Fragment {:?} produced no operators",
                fragment.id
            )));
        }

        // Top of stack is the fragment root.
        let mut root_executor = stack.remove(stack.len() - 1);

        // Flatten remaining stack: if the pipeline produced multiple roots
        // (e.g. from multi-operator chains), chain them linearly.
        while let Some(remaining) = stack.pop() {
            match &mut root_executor {
                StreamingExecutor::Unary(_, child, _)
                | StreamingExecutor::Blocking(_, child, _)
                | StreamingExecutor::Graph(_, child, _)
                | StreamingExecutor::RecursiveFragment(_, child, _)
                | StreamingExecutor::Sink(_, child, _)
                | StreamingExecutor::Ddl(_, child, _)
                | StreamingExecutor::Fulltext(_, child, _)
                | StreamingExecutor::Vector(_, child, _)
                | StreamingExecutor::Txn(_, child, _) => {
                    // root_executor is unary → wrap remaining as its child chain.
                    let mut inner = remaining;
                    match &mut inner {
                        StreamingExecutor::Unary(_, c, _)
                        | StreamingExecutor::Blocking(_, c, _)
                        | StreamingExecutor::Graph(_, c, _)
                        | StreamingExecutor::RecursiveFragment(_, c, _)
                        | StreamingExecutor::Sink(_, c, _)
                        | StreamingExecutor::Ddl(_, c, _)
                        | StreamingExecutor::Fulltext(_, c, _)
                        | StreamingExecutor::Vector(_, c, _)
                        | StreamingExecutor::Txn(_, c, _) => {
                            std::mem::swap(child, c);
                        }
                        _ => {
                            **child = inner;
                        }
                    }
                }
                _ => {
                    // Binary or source root — cannot absorb extra items.
                    // Prepend remaining before root.
                    let mut prepend = remaining;
                    match &mut prepend {
                        StreamingExecutor::Unary(_, c, _)
                        | StreamingExecutor::Blocking(_, c, _) => {
                            **c = root_executor;
                            root_executor = prepend;
                        }
                        _ => {
                            return Err(QueryError::execution(format!(
                                "Cannot flatten pipeline in fragment {:?}",
                                fragment.id
                            )));
                        }
                    }
                }
            }
        }

        root_executor.set_chunk_size(bindings.chunk_size);
        root_executor.set_runtime(Some(runtime.clone()));

        Ok(root_executor)
    }

    // ── Fragment graph traversal ──

    fn topological_order(fragments: &FragmentGraph) -> Result<Vec<FragmentId>, QueryError> {
        let root = fragments.root();
        let mut order = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![root];

        while let Some(fid) = stack.pop() {
            if !visited.insert(fid) {
                continue;
            }
            if let Some(frag) = fragments.get(fid) {
                for &input in &frag.inputs {
                    stack.push(input);
                }
                order.push(fid);
            }
        }

        order.reverse();

        if order.is_empty() {
            return Err(QueryError::execution("Fragment graph is empty".to_string()));
        }

        Ok(order)
    }

    // ── Binding validation ──

    /// Validate bindings against the plan's parameter schema.
    ///
    /// M1.3: checks for missing required params, unknown params, and type
    /// compatibility.  Returns an error description listing all violations.
    fn validate_bindings(plan: &PhysicalPlan, bindings: &QueryBindings) -> Result<(), QueryError> {
        let schema = &plan.parameter_schema;
        if schema.is_empty() {
            return Ok(());
        }

        let mut errors: Vec<String> = Vec::new();

        // Check missing required params.
        for param in &schema.params {
            if !bindings.parameters.contains_key(&param.name) {
                errors.push(format!("Missing required parameter: {}", param.name));
            }
        }

        // Check unknown params (present in bindings but not in schema).
        for (name, _value) in bindings.parameters.iter() {
            if schema.slot(name).is_none() {
                errors.push(format!("Unknown parameter: {}", name));
            }
        }

        // Check type compatibility for params present in both.
        for param in &schema.params {
            if let Some(value) = bindings.parameters.get(&param.name) {
                if let Some(ref expected_type) = param.value_type {
                    if !Self::type_compatible(value, expected_type) {
                        errors.push(format!(
                            "Parameter '{}': expected type {:?}, got value {:?}",
                            param.name, expected_type, value
                        ));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(QueryError::execution(errors.join("; ")))
        }
    }

    /// Rough type compatibility check for parameter values.
    /// M1.3: ensures the runtime value is semantically assignable to the
    /// declared parameter type.
    fn type_compatible(value: &crate::core::Value, expected_type: &crate::core::DataType) -> bool {
        use crate::core::DataType;
        use crate::core::Value;
        matches!(
            (value, expected_type),
            (Value::Null(_), _)
                | (Value::Bool(_), DataType::Bool)
                | (Value::Int(_), DataType::Int | DataType::BigInt)
                | (Value::BigInt(_), DataType::BigInt | DataType::Int)
                | (Value::Float(_), DataType::Float | DataType::Double)
                | (Value::Double(_), DataType::Double | DataType::Float)
                | (Value::String(_), DataType::String)
                | (Value::Date(_), DataType::Date)
                | (Value::Time(_), DataType::Time)
                | (Value::DateTime(_), DataType::DateTime)
        )
    }

    // ── Runtime creation ──

    /// Create an [`ExecutionRuntime`] from bindings.
    ///
    /// M2: injects transaction scope and session controller into the runtime
    /// so that operators can check write permissions and transaction commands
    /// can drive real state transitions.
    fn create_runtime(bindings: &QueryBindings) -> Arc<ExecutionRuntime> {
        let mut runtime = ExecutionRuntime::new(
            crate::query::executor::streaming::runtime::QueryIdentity {
                query_id: bindings.query_id,
                session_id: None,
                space_name: bindings.space_name.clone(),
            },
            bindings.memory_budget.clone(),
            bindings.storage.clone(),
            #[cfg(feature = "fulltext-search")]
            bindings.fulltext_manager.clone(),
            #[cfg(feature = "qdrant")]
            bindings.vector_coordinator.clone(),
        );

        // M2: inject transaction scope.
        match bindings.transaction {
            crate::query::executor::streaming::transaction_scope::TransactionScope::None => {
                // DDL / admin commands may run without a txn scope, but DML will
                // be rejected at the operator level (see SinkOperator::check_write_permission).
            }
            ref scope => {
                runtime.set_transaction_scope(scope.clone());
            }
        }

        // M6: shared scheduler takes priority.
        if let Some(ref ss) = bindings.shared_scheduler {
            runtime.set_shared_scheduler(Some(ss.clone()));
        } else if bindings.max_workers > 1 {
            let pool = crate::query::executor::streaming::pool::MorselWorkerPool::new(
                bindings.max_workers,
            );
            runtime.set_worker_pool(Some(pool));
        }

        Arc::new(runtime)
    }
}

fn pop_stack_or_err(stack: &mut Vec<StreamingExecutor>) -> Result<StreamingExecutor, QueryError> {
    stack.pop().ok_or_else(|| {
        QueryError::execution("Operator requires child but stack is empty".to_string())
    })
}
