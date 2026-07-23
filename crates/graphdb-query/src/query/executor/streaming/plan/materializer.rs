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
use super::types::{
    FragmentGraph, FragmentId, FragmentSpec, InputContract, OperatorKindSpec, PhysicalPlan,
};
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

        let parameter_values = if let Some(ref frame) = mutable_bindings.parameter_frame {
            let map: std::collections::HashMap<String, crate::core::Value> = plan
                .parameter_schema
                .params
                .iter()
                .filter_map(|p| frame.get(p.slot).map(|v| (p.name.clone(), v.clone())))
                .collect();
            if map.is_empty() {
                None
            } else {
                Some(std::sync::Arc::new(map))
            }
        } else {
            None
        };

        let runtime = Self::create_runtime(&mutable_bindings, parameter_values);

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
    /// Within a fragment, operators form a strict leaf-to-root pipeline. The
    /// first operator consumes the fragment's external input contract; every
    /// later operator consumes only the preceding operator.
    fn build_fragment_pipeline(
        fragment: &FragmentSpec,
        plan: &PhysicalPlan,
        fragment_roots: &mut HashMap<FragmentId, StreamingExecutor>,
        runtime: &Arc<ExecutionRuntime>,
        bindings: &QueryBindings,
    ) -> Result<StreamingExecutor, QueryError> {
        let mut previous = None;

        for (operator_index, &op_id) in fragment.operators.iter().enumerate() {
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

            let mut inputs = if operator_index == 0 {
                take_external_inputs(&op_spec.input_contract, fragment_roots)?
            } else {
                vec![previous.take().ok_or_else(|| {
                    QueryError::execution(format!(
                        "Fragment {:?} lost its linear pipeline before operator {:?}",
                        fragment.id, op_id
                    ))
                })?]
            };

            let exec = match &op_spec.spec {
                OperatorKindSpec::Source(src_spec) => {
                    require_input_count(fragment.id, op_id, &inputs, 0)?;
                    let storage = bindings.storage.clone();
                    let op = SourceOperator::from_spec(src_spec, storage);
                    StreamingExecutor::Source(base, op)
                }
                OperatorKindSpec::Unary(unary_spec) => {
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
                    let op = UnaryOperator::from_spec(unary_spec);
                    StreamingExecutor::Unary(base, Box::new(child), op)
                }
                OperatorKindSpec::Blocking(blocking_spec) => {
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
                    let op = BlockingOperator::from_spec(blocking_spec, &bindings.memory_budget);
                    StreamingExecutor::Blocking(base, Box::new(child), op)
                }
                OperatorKindSpec::Join(join_spec) => {
                    let (left, right) = take_binary_inputs(fragment.id, op_id, inputs)?;
                    let op = JoinOperator::from_spec(join_spec, &bindings.memory_budget);
                    StreamingExecutor::Join(base, Box::new(left), Box::new(right), op)
                }
                OperatorKindSpec::Graph(graph_spec) => {
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
                    let storage = runtime.storage.clone();
                    let space_name = runtime.query_id().space_name.clone().unwrap_or_default();
                    let op = GraphOperator::from_spec(graph_spec, storage, space_name);
                    StreamingExecutor::Graph(base, Box::new(child), op)
                }
                OperatorKindSpec::RecursiveFragment(rf_spec) => {
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
                    let storage = runtime.storage.clone();
                    let space_name = runtime.query_id().space_name.clone().unwrap_or_default();
                    let op = RecursiveFragmentOperator::from_spec(rf_spec, storage, space_name);
                    StreamingExecutor::RecursiveFragment(base, Box::new(child), op)
                }
                OperatorKindSpec::Sink(sink_spec) => {
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
                    let storage = runtime.storage.clone();
                    let op = SinkOperator::from_spec(sink_spec, storage);
                    StreamingExecutor::Sink(base, Box::new(child), op)
                }
                OperatorKindSpec::Set(set_spec) => {
                    let (left, right) = take_binary_inputs(fragment.id, op_id, inputs)?;
                    let op = SetOperator::from_spec(set_spec, &bindings.memory_budget);
                    StreamingExecutor::Set(base, Box::new(left), Box::new(right), op)
                }
                OperatorKindSpec::Apply(apply_spec) => {
                    let (left, right) = take_binary_inputs(fragment.id, op_id, inputs)?;
                    let op = ApplyOperator::from_spec(apply_spec, &bindings.memory_budget);
                    StreamingExecutor::Apply(base, Box::new(left), Box::new(right), op)
                }
                OperatorKindSpec::Exchange(exchange_spec) => {
                    if inputs.is_empty() {
                        return Err(QueryError::execution(format!(
                            "Exchange operator {:?} in fragment {:?} has no inputs",
                            op_id, fragment.id
                        )));
                    }
                    let children = inputs;
                    let op = ExchangeOperator::from_spec(exchange_spec);
                    StreamingExecutor::Exchange(base, children, op)
                }
                OperatorKindSpec::Ddl(ddl_spec) => {
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
                    let storage = runtime.storage.clone();
                    let op = DdlOperator::from_spec(ddl_spec, storage);
                    StreamingExecutor::Ddl(base, Box::new(child), op)
                }
                OperatorKindSpec::Fulltext(ft_spec) => {
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
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
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
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
                    let child = take_unary_input(fragment.id, op_id, &mut inputs)?;
                    let op = TxnOperator::from_spec(txn_spec);
                    StreamingExecutor::Txn(base, Box::new(child), op)
                }
            };

            previous = Some(exec);
        }

        let mut root_executor = previous.ok_or_else(|| {
            QueryError::execution(format!("Fragment {:?} produced no operators", fragment.id))
        })?;

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
        let mut errors: Vec<String> = Vec::new();

        // Check missing required params.
        for param in &schema.params {
            if !bindings.parameters.contains_key(&param.name) && param.default.is_none() {
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
    fn create_runtime(
        bindings: &QueryBindings,
        parameter_values: Option<
            std::sync::Arc<std::collections::HashMap<String, crate::core::Value>>,
        >,
    ) -> Arc<ExecutionRuntime> {
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

        // M1.4: inject the parameter name→value map so operators can resolve $name.
        if let Some(values) = parameter_values {
            runtime.set_parameter_values(values);
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

fn take_external_inputs(
    contract: &InputContract,
    fragment_roots: &mut HashMap<FragmentId, StreamingExecutor>,
) -> Result<Vec<StreamingExecutor>, QueryError> {
    let ids: Vec<FragmentId> = match contract {
        InputContract::NoInput => Vec::new(),
        InputContract::UnaryInput(input) => vec![input.fragment],
        InputContract::BinaryInputs { left, right } => vec![left.fragment, right.fragment],
        InputContract::PartitionedInputs { members, .. } => {
            let mut ordered = members.iter().collect::<Vec<_>>();
            ordered.sort_by_key(|member| member.partition_id);
            ordered.into_iter().map(|member| member.fragment).collect()
        }
    };

    ids.into_iter()
        .map(|fragment_id| {
            fragment_roots.remove(&fragment_id).ok_or_else(|| {
                QueryError::execution(format!(
                    "Input contract references unavailable producer fragment {:?}",
                    fragment_id
                ))
            })
        })
        .collect()
}

fn require_input_count(
    fragment_id: FragmentId,
    operator_id: super::types::PhysicalOperatorId,
    inputs: &[StreamingExecutor],
    expected: usize,
) -> Result<(), QueryError> {
    if inputs.len() == expected {
        Ok(())
    } else {
        Err(QueryError::execution(format!(
            "Operator {:?} in fragment {:?} received {} inputs, expected {}",
            operator_id,
            fragment_id,
            inputs.len(),
            expected
        )))
    }
}

fn take_unary_input(
    fragment_id: FragmentId,
    operator_id: super::types::PhysicalOperatorId,
    inputs: &mut Vec<StreamingExecutor>,
) -> Result<StreamingExecutor, QueryError> {
    require_input_count(fragment_id, operator_id, inputs, 1)?;
    inputs.pop().ok_or_else(|| {
        QueryError::execution(format!("Operator {:?} has no unary input", operator_id))
    })
}

fn take_binary_inputs(
    fragment_id: FragmentId,
    operator_id: super::types::PhysicalOperatorId,
    mut inputs: Vec<StreamingExecutor>,
) -> Result<(StreamingExecutor, StreamingExecutor), QueryError> {
    require_input_count(fragment_id, operator_id, &inputs, 2)?;
    let right = inputs.pop().ok_or_else(|| {
        QueryError::execution(format!("Operator {:?} has no right input", operator_id))
    })?;
    let left = inputs.pop().ok_or_else(|| {
        QueryError::execution(format!("Operator {:?} has no left input", operator_id))
    })?;
    Ok((left, right))
}


