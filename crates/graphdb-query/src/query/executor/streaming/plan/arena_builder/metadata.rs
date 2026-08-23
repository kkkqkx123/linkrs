//! Plan metadata inferred after arena assembly.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::super::operators::spec::{
    ApplySpec, BlockingSpec, DdlSpec, ExchangeSpec, FulltextSpec, GraphSpec, JoinSpec,
    RecursiveFragmentSpec, SetSpec, SinkSpec, SourceSpec, TxnSpec, UnarySpec, VectorSpec,
};
use super::super::super::slot::{combine_layouts, SlotLayout};
use super::super::properties::{PhysicalProperties, SPILL_DEFAULT_THRESHOLD};
use super::super::types::{
    CapabilitySet, FragmentInput, FragmentSpec, InputContract, InputDistribution, OperatorKindSpec,
    OutputContract, PartitionInput, PartitionSide, PhysicalOperatorId, PhysicalOperatorSpec,
    StateOwnership,
};
use crate::query::executor::build_error::PlanBuildError;

pub(super) fn output_contract(
    spec: &OperatorKindSpec,
    output_layout: SlotLayout,
) -> OutputContract {
    OutputContract {
        output_layout,
        always_produces_row: false,
        nullability: Vec::new(),
        ordering: Vec::new(),
        delivery_streamable: true,
        pipeline_mode: match spec {
            OperatorKindSpec::Blocking(_)
            | OperatorKindSpec::Exchange(
                ExchangeSpec::Barrier | ExchangeSpec::Materialize { .. },
            ) => super::super::types::PipelineMode::Blocking,
            _ => super::super::types::PipelineMode::Pipelined,
        },
    }
}

/// Populate every arena operator's input and output layout after the
/// fragment graph is assembled.  This keeps schema an immutable plan
/// property instead of allowing executors to infer it from their first
/// non-empty chunk.
pub(super) fn propagate_layouts(
    operators: &mut [PhysicalOperatorSpec],
    fragments: &[FragmentSpec],
) -> Result<(), PlanBuildError> {
    let mut layouts: HashMap<PhysicalOperatorId, SlotLayout> = operators
        .iter()
        .filter_map(|operator| match &operator.spec {
            OperatorKindSpec::Source(spec) => {
                Some((operator.operator_id, source_output_layout(spec)))
            }
            _ => None,
        })
        .collect();

    for fragment in fragments {
        let fragment_inputs = fragment
            .inputs
            .iter()
            .map(|input_id| {
                let root = fragments.get(input_id.0).ok_or_else(|| {
                    PlanBuildError::unsupported("PhysicalPlan", 0, "input fragment not found")
                })?;
                layouts.get(&root.root_operator).cloned().ok_or_else(|| {
                    PlanBuildError::unsupported("PhysicalPlan", 0, "input layout not available")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut previous = None;

        for operator_id in &fragment.operators {
            let operator = operators.get_mut(operator_id.0).ok_or_else(|| {
                PlanBuildError::unsupported("PhysicalPlan", 0, "operator not found")
            })?;
            let input_layouts = previous
                .clone()
                .map(|layout| vec![layout])
                .unwrap_or_else(|| fragment_inputs.clone());
            operator.input_layout = input_layouts.first().cloned();
            let output_layout = infer_output_layout(&operator.spec, &input_layouts);
            operator.output_layout = output_layout.clone();
            layouts.insert(*operator_id, output_layout.clone());
            previous = Some(output_layout);
        }
    }
    Ok(())
}

pub(super) fn populate_input_contracts(
    operators: &mut [PhysicalOperatorSpec],
    fragments: &[FragmentSpec],
) -> Result<(), PlanBuildError> {
    for fragment in fragments {
        let external_inputs = fragment
            .inputs
            .iter()
            .map(|fragment_id| {
                let input_fragment = fragments.get(fragment_id.0).ok_or_else(|| {
                    PlanBuildError::unsupported("PhysicalPlan", 0, "input fragment not found")
                })?;
                let input_operator =
                    operators
                        .get(input_fragment.root_operator.0)
                        .ok_or_else(|| {
                            PlanBuildError::unsupported(
                                "PhysicalPlan",
                                0,
                                "input operator not found",
                            )
                        })?;
                Ok(FragmentInput {
                    fragment: *fragment_id,
                    layout: Arc::new(input_operator.output_layout.clone()),
                    properties: input_operator.properties.clone(),
                })
            })
            .collect::<Result<Vec<_>, PlanBuildError>>()?;

        let mut previous_operator = None;
        for operator_id in &fragment.operators {
            let previous_input = previous_operator.and_then(|previous: PhysicalOperatorId| {
                operators.get(previous.0).map(|operator| FragmentInput {
                    fragment: fragment.id,
                    layout: Arc::new(operator.output_layout.clone()),
                    properties: operator.properties.clone(),
                })
            });
            let operator = operators.get_mut(operator_id.0).ok_or_else(|| {
                PlanBuildError::unsupported("PhysicalPlan", 0, "operator not found")
            })?;
            let inputs = previous_input
                .map(|input| vec![input])
                .unwrap_or_else(|| external_inputs.clone());
            operator.input_contract = match &operator.spec {
                OperatorKindSpec::Source(_) => InputContract::NoInput,
                OperatorKindSpec::Join(_)
                | OperatorKindSpec::Set(_)
                | OperatorKindSpec::Apply(ApplySpec::Apply { .. })
                | OperatorKindSpec::Apply(ApplySpec::PatternApply { .. })
                | OperatorKindSpec::Apply(ApplySpec::RollUpApply { .. }) => {
                    if inputs.len() >= 2 {
                        InputContract::BinaryInputs {
                            left: inputs[0].clone(),
                            right: inputs[1].clone(),
                        }
                    } else {
                        InputContract::NoInput
                    }
                }
                OperatorKindSpec::Exchange(ExchangeSpec::RepartitionHash {
                    hash_expressions,
                    ..
                }) => {
                    let key_slot = hash_expressions
                        .first()
                        .and_then(|expr| {
                            if let crate::core::types::expr::Expression::Variable(name) = expr {
                                inputs.first().and_then(|input| input.layout.slot_id(name))
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    InputContract::PartitionedInputs {
                        side: PartitionSide::Unary,
                        distribution: InputDistribution::HashRepartition { key_slot },
                        members: inputs
                            .iter()
                            .enumerate()
                            .map(|(partition_id, input)| PartitionInput {
                                partition_id,
                                fragment: input.fragment,
                                layout: Arc::clone(&input.layout),
                                properties: input.properties.clone(),
                            })
                            .collect(),
                    }
                }
                OperatorKindSpec::Exchange(_) => InputContract::PartitionedInputs {
                    side: PartitionSide::Unary,
                    distribution: InputDistribution::Concatenate,
                    members: inputs
                        .iter()
                        .enumerate()
                        .map(|(partition_id, input)| PartitionInput {
                            partition_id,
                            fragment: input.fragment,
                            layout: Arc::clone(&input.layout),
                            properties: input.properties.clone(),
                        })
                        .collect(),
                },
                _ => inputs
                    .into_iter()
                    .next()
                    .map(InputContract::UnaryInput)
                    .unwrap_or(InputContract::NoInput),
            };
            previous_operator = Some(*operator_id);
        }
    }
    Ok(())
}

pub(super) fn populate_runtime_metadata(operators: &mut [PhysicalOperatorSpec]) {
    for operator in operators {
        operator.state_ownership = match &operator.spec {
            OperatorKindSpec::Blocking(_) | OperatorKindSpec::Exchange(_) => {
                StateOwnership::TaskLocal
            }
            _ => StateOwnership::TreeLocal,
        };
        operator.properties = derive_physical_properties(&operator.spec);
    }
}

/// Populate per-operator `choice_reason` strings after full arena assembly.
///
/// This pass is the single place that turns physical spec facts into
/// human-readable decision summaries for EXPLAIN: source scan modes
/// (index scan vs full scan vs point lookup) and join algorithm choice.
/// CBO decision notes (unnest / join order) are plan-level and carried
/// separately in [`PhysicalPlan`]'s `cbo_notes`.
pub(super) fn populate_choice_reasons(operators: &mut [PhysicalOperatorSpec]) {
    for operator in operators {
        operator.choice_reason = choice_reason_for_spec(&operator.spec);
    }
}

/// Populate `estimated_cardinality` for every operator.
///
/// The cost-based phase attaches a per-logical-node row estimate map to the
/// optimized plan (`ExecutionPlan.row_estimates`); this pass writes those
/// estimates onto physical operators matched by `logical_node_id`. Operators
/// without a matching logical node (synthesized exchanges, sinks, and
/// materialization wrappers) fall back to their input's estimate, keeping
/// EXPLAIN free of large `est_rows = None` gaps.
pub(super) fn populate_estimated_rows(
    operators: &mut [PhysicalOperatorSpec],
    fragments: &[FragmentSpec],
    estimates: &std::collections::HashMap<i64, u64>,
) {
    // Fragments form a DAG; propagate input-fragment root estimates so the
    // fallback can flow across fragment boundaries.
    let mut fragment_input_rows: std::collections::HashMap<
        crate::query::executor::streaming::plan::types::FragmentId,
        f64,
    > = std::collections::HashMap::new();

    for fragment in fragments {
        let mut previous_rows = fragment
            .inputs
            .iter()
            .filter_map(|input_id| fragment_input_rows.get(input_id))
            .copied()
            .fold(0.0, f64::max);
        if previous_rows == 0.0 {
            previous_rows = fragment
                .inputs
                .first()
                .and_then(|input_id| fragment_input_rows.get(input_id))
                .copied()
                .unwrap_or(1.0);
        }

        for operator_id in &fragment.operators {
            let Some(operator) = operators.get_mut(operator_id.0) else {
                continue;
            };
            let mapped = operator
                .logical_node_id
                .and_then(|logical_id| estimates.get(&logical_id.0))
                .copied();
            let estimate = if let Some(rows) = mapped {
                rows as f64
            } else {
                previous_rows * fallback_selectivity_factor(&operator.spec)
            };
            operator.estimated_cardinality = Some(estimate);
            previous_rows = estimate;
        }
        fragment_input_rows.insert(
            fragment.id,
            operators
                .get(fragment.root_operator.0)
                .and_then(|operator| operator.estimated_cardinality)
                .unwrap_or(previous_rows),
        );
    }
}

/// Fallback selectivity factor for operators without a CBO estimate.
fn fallback_selectivity_factor(spec: &OperatorKindSpec) -> f64 {
    match spec {
        OperatorKindSpec::Unary(UnarySpec::Filter { .. }) => 0.5,
        OperatorKindSpec::Blocking(BlockingSpec::Aggregate { .. })
        | OperatorKindSpec::Blocking(BlockingSpec::PartialAggregate { .. })
        | OperatorKindSpec::Blocking(BlockingSpec::FinalAggregate { .. }) => 0.1,
        _ => 1.0,
    }
}

fn choice_reason_for_spec(spec: &OperatorKindSpec) -> Option<String> {
    match spec {
        OperatorKindSpec::Source(source) => Some(match source {
            SourceSpec::IndexScan { index_name, .. } => {
                format!("index_scan({})", index_name)
            }
            SourceSpec::StorageScanVertices {
                partition_range: Some(_),
                ..
            }
            | SourceSpec::StorageScanEdges {
                partition_range: Some(_),
                ..
            } => "partitioned_full_scan".to_string(),
            SourceSpec::StorageScanVertices { .. } | SourceSpec::StorageScanEdges { .. } => {
                "storage_full_scan".to_string()
            }
            SourceSpec::ScanVertices { .. } | SourceSpec::ScanEdges { .. } => {
                "materialized_scan".to_string()
            }
            SourceSpec::GetVertices { .. } => "point_lookup".to_string(),
            SourceSpec::GetEdges { .. } => "edge_lookup".to_string(),
            SourceSpec::GetNeighbors { .. } => "neighborhood_scan".to_string(),
            SourceSpec::GetProp { .. } => "property_lookup".to_string(),
            SourceSpec::StandaloneValues { .. } => "values".to_string(),
            SourceSpec::Argument { .. } | SourceSpec::Start => "seed".to_string(),
        }),
        OperatorKindSpec::Join(_) => Some("hash_join".to_string()),
        OperatorKindSpec::Blocking(spec) => Some(match spec {
            BlockingSpec::TopN { .. } => "topn".to_string(),
            BlockingSpec::Sort { .. } => "sort".to_string(),
            BlockingSpec::Distinct => "distinct".to_string(),
            BlockingSpec::Aggregate { .. }
            | BlockingSpec::PartialAggregate { .. }
            | BlockingSpec::FinalAggregate { .. } => "aggregate".to_string(),
            _ => "blocking".to_string(),
        }),
        OperatorKindSpec::Exchange(_) => Some("exchange".to_string()),
        _ => None,
    }
}

/// Derive [`PhysicalProperties`] from the operator spec directly instead of
/// relying on each builder call site to pass a consistent default.
///
/// This is the single source of truth for per-operator physical properties.
/// Every call site that pushes an operator should leave its properties at
/// a reasonable default; this pass corrects them after full assembly.
///
/// This makes Phase D requirement #7 observable: properties are derived from
/// the node / spec, not hardcoded at each call site.
pub(super) fn derive_physical_properties(spec: &OperatorKindSpec) -> PhysicalProperties {
    match spec {
        OperatorKindSpec::Source(_) => PhysicalProperties::single_streaming(),
        OperatorKindSpec::Unary(_) => PhysicalProperties::single_streaming(),
        OperatorKindSpec::Blocking(spec) => {
            match spec {
                BlockingSpec::Sort { .. } => {
                    PhysicalProperties::single_blocking_spillable(SPILL_DEFAULT_THRESHOLD)
                }
                BlockingSpec::PartialAggregate { .. } | BlockingSpec::FinalAggregate { .. } => {
                    // Partial / final aggregate requires budget tracking but
                    // does not spill at the operator level.
                    PhysicalProperties::single_blocking_with_budget()
                }
                // All other blocking operators (Aggregate, GroupBy, Distinct,
                // TopN, Window, Materialize, DataCollect, RollUpApply) need
                // a memory budget.
                _ => PhysicalProperties::single_blocking_with_budget(),
            }
        }
        OperatorKindSpec::Join(_) | OperatorKindSpec::Set(_) | OperatorKindSpec::Apply(_) => {
            PhysicalProperties::single_blocking_with_budget()
        }
        OperatorKindSpec::Exchange(_) => PhysicalProperties::single_blocking(),
        OperatorKindSpec::Graph(_) | OperatorKindSpec::RecursiveFragment(_) => {
            PhysicalProperties::single_streaming()
        }
        OperatorKindSpec::Sink(_) => PhysicalProperties::single_blocking(),
        OperatorKindSpec::Ddl(_) => PhysicalProperties::single_blocking(),
        OperatorKindSpec::Fulltext(_) | OperatorKindSpec::Vector(_) => {
            PhysicalProperties::single_streaming()
        }
        OperatorKindSpec::Txn(_) => PhysicalProperties::single_blocking(),
    }
}
pub(super) fn capability_for_operator(spec: &OperatorKindSpec) -> CapabilitySet {
    match spec {
        OperatorKindSpec::Source(_) | OperatorKindSpec::Unary(_) => CapabilitySet::PARALLEL_BASIC,
        OperatorKindSpec::Blocking(_) => CapabilitySet::PARALLEL_BLOCKING,
        OperatorKindSpec::Join(_) | OperatorKindSpec::Set(_) | OperatorKindSpec::Apply(_) => {
            CapabilitySet::PARALLEL_JOIN
        }
        OperatorKindSpec::Exchange(_)
        | OperatorKindSpec::Graph(_)
        | OperatorKindSpec::RecursiveFragment(_)
        | OperatorKindSpec::Sink(_)
        | OperatorKindSpec::Ddl(_)
        | OperatorKindSpec::Fulltext(_)
        | OperatorKindSpec::Vector(_)
        | OperatorKindSpec::Txn(_) => CapabilitySet::PARALLEL_FULL,
    }
}

pub(super) fn input_layout(inputs: &[SlotLayout]) -> SlotLayout {
    inputs
        .first()
        .cloned()
        .unwrap_or_else(|| SlotLayout::new(Vec::new()))
}

pub(super) fn estimate_source_cardinality(spec: &SourceSpec) -> Option<f64> {
    match spec {
        SourceSpec::ScanVertices { rows, .. } | SourceSpec::ScanEdges { rows, .. } => {
            Some(rows.len() as f64)
        }
        SourceSpec::StandaloneValues { values, .. } => Some(values.len() as f64),
        SourceSpec::StorageScanVertices { limit, .. }
        | SourceSpec::StorageScanEdges { limit, .. } => limit.map(|value| value as f64),
        SourceSpec::GetVertices { vertex_ids, .. } => {
            vertex_ids.as_ref().map(|ids| ids.len() as f64)
        }
        SourceSpec::Argument { .. } | SourceSpec::Start => Some(1.0),
        SourceSpec::GetEdges { .. }
        | SourceSpec::GetNeighbors { .. }
        | SourceSpec::IndexScan { .. }
        | SourceSpec::GetProp { .. } => None,
    }
}

pub(super) fn layout_with_added_names(
    input: &SlotLayout,
    names: impl IntoIterator<Item = String>,
) -> SlotLayout {
    let mut all_names = input.names();
    all_names.extend(names);
    SlotLayout::from_names(&all_names)
}

pub(super) fn infer_output_layout(spec: &OperatorKindSpec, inputs: &[SlotLayout]) -> SlotLayout {
    let input = input_layout(inputs);
    match spec {
        OperatorKindSpec::Source(spec) => source_output_layout(spec),
        OperatorKindSpec::Unary(UnarySpec::Project {
            output_col_names, ..
        }) => SlotLayout::from_names(output_col_names),
        OperatorKindSpec::Unary(UnarySpec::Assign { assignments, .. }) => {
            layout_with_added_names(&input, assignments.iter().map(|(name, _)| name.clone()))
        }
        OperatorKindSpec::Unary(UnarySpec::Remove { columns_to_remove }) => {
            let names = input
                .names()
                .into_iter()
                .filter(|name| !columns_to_remove.contains(name))
                .collect::<Vec<_>>();
            SlotLayout::from_names(&names)
        }
        OperatorKindSpec::Unary(UnarySpec::AppendVertices {
            entity_var,
            prop_names,
            ..
        }) => {
            let added: Vec<String> = if prop_names.is_empty() {
                vec![entity_var.clone()]
            } else {
                prop_names
                    .iter()
                    .map(|prop| format!("{entity_var}.{prop}"))
                    .collect()
            };
            layout_with_added_names(&input, added)
        }
        OperatorKindSpec::Blocking(
            BlockingSpec::Aggregate {
                output_col_names, ..
            }
            | BlockingSpec::PartialAggregate {
                output_col_names, ..
            }
            | BlockingSpec::FinalAggregate {
                output_col_names, ..
            },
        ) => SlotLayout::from_names(output_col_names),
        OperatorKindSpec::Blocking(BlockingSpec::RollUpApply { rollup_expressions }) => {
            layout_with_added_names(
                &input,
                (0..rollup_expressions.len()).map(|index| format!("rollup_{index}")),
            )
        }
        OperatorKindSpec::Join(JoinSpec::SemiJoin { .. }) => input,
        OperatorKindSpec::Join(_) => inputs
            .get(1)
            .map(|right| combine_layouts(&input, right))
            .unwrap_or(input),
        OperatorKindSpec::Apply(ApplySpec::Apply {
            kind:
                super::super::super::operators::spec::ApplyKind::Standard
                | super::super::super::operators::spec::ApplyKind::Single,
            ..
        }) => inputs
            .get(1)
            .map(|right| combine_layouts(&input, right))
            .unwrap_or(input),
        OperatorKindSpec::Apply(ApplySpec::RollUpApply { collect_column, .. }) => {
            layout_with_added_names(
                &input,
                [collect_column
                    .clone()
                    .unwrap_or_else(|| "rollup".to_string())],
            )
        }
        OperatorKindSpec::Apply(ApplySpec::PatternApply { .. })
        | OperatorKindSpec::Apply(ApplySpec::CorrelatedApply { .. })
        | OperatorKindSpec::Apply(ApplySpec::Apply { .. })
        | OperatorKindSpec::Set(_)
        | OperatorKindSpec::Exchange(_)
        | OperatorKindSpec::Ddl(_)
        | OperatorKindSpec::Txn(_)
        | OperatorKindSpec::Blocking(_) => input,
        OperatorKindSpec::Unary(UnarySpec::Unwind { unwind_column, .. }) => {
            layout_with_added_names(&input, [unwind_column.clone()])
        }
        OperatorKindSpec::Unary(_) => input,
        OperatorKindSpec::Sink(_) => {
            SlotLayout::from_names(&["operation".to_string(), "count".to_string()])
        }
        OperatorKindSpec::Graph(GraphSpec::ExpandAll {
            col_names,
            count_only: true,
            ..
        }) => {
            // A count-only expand emits a single per-chunk edge-count column.
            let _ = col_names;
            SlotLayout::from_names(&["_expand_count".to_string()])
        }
        OperatorKindSpec::Graph(
            GraphSpec::Expand { col_names, .. } | GraphSpec::ExpandAll { col_names, .. },
        ) => {
            let base = layout_with_added_names(
                &input,
                ["_expand_edge".to_string(), "_expand_dst".to_string()],
            );
            if col_names.len() >= 3 {
                let edge_slot_id = base
                    .resolve("_expand_edge")
                    .unwrap_or(base.slots.len().saturating_sub(2));
                let dst_slot_id = base
                    .resolve("_expand_dst")
                    .unwrap_or(edge_slot_id.saturating_add(1));
                let mut slots = base.slots;
                let mut name_to_slot = base.name_to_slot;
                let src_slot_id = input.resolve(&col_names[0]).unwrap_or(0);
                name_to_slot.insert(col_names[0].clone(), src_slot_id);
                if let Some(slot) = slots.get_mut(edge_slot_id) {
                    slot.name = col_names[1].clone();
                }
                name_to_slot.insert(col_names[1].clone(), edge_slot_id);
                if let Some(slot) = slots.get_mut(dst_slot_id) {
                    slot.name = col_names[2].clone();
                }
                name_to_slot.insert(col_names[2].clone(), dst_slot_id);
                name_to_slot.insert("$$".to_string(), dst_slot_id);
                name_to_slot.insert("$^".to_string(), src_slot_id);
                name_to_slot.insert("target".to_string(), dst_slot_id);
                for extra_name in col_names.iter().skip(3) {
                    if let Some(slot) = slots.get_mut(edge_slot_id) {
                        if slot.alias.is_none() {
                            slot.alias = Some(extra_name.clone());
                        }
                    }
                    name_to_slot.insert(extra_name.clone(), edge_slot_id);
                }
                SlotLayout {
                    slots,
                    name_to_slot,
                }
            } else {
                base
            }
        }
        OperatorKindSpec::Graph(GraphSpec::Traverse { .. }) => layout_with_added_names(
            &input,
            [
                "_traverse_vertex".to_string(),
                "_traverse_edge_type".to_string(),
                "_traverse_direction".to_string(),
                "_traverse_depth".to_string(),
            ],
        ),
        OperatorKindSpec::RecursiveFragment(RecursiveFragmentSpec::ShortestPath { .. }) => {
            layout_with_added_names(&input, ["path".to_string()])
        }
        OperatorKindSpec::RecursiveFragment(RecursiveFragmentSpec::MultiShortestPath {
            ..
        }) => layout_with_added_names(&input, ["_multi_shortest_path".to_string()]),
        OperatorKindSpec::RecursiveFragment(RecursiveFragmentSpec::BFSShortest { .. }) => {
            layout_with_added_names(&input, ["_bfs_path".to_string()])
        }
        OperatorKindSpec::RecursiveFragment(RecursiveFragmentSpec::AllPaths { .. }) => {
            layout_with_added_names(&input, ["path".to_string()])
        }
        OperatorKindSpec::Graph(GraphSpec::BiExpand { .. } | GraphSpec::BiTraverse { .. }) => input,
        OperatorKindSpec::Fulltext(FulltextSpec::FulltextManage { .. })
        | OperatorKindSpec::Vector(VectorSpec::VectorManage { .. }) => SlotLayout::from_names(&[
            "action".to_string(),
            "name".to_string(),
            "status".to_string(),
        ]),
        OperatorKindSpec::Fulltext(
            FulltextSpec::FulltextSearch { .. }
            | FulltextSpec::FulltextLookup { .. }
            | FulltextSpec::MatchFulltext { .. },
        ) => SlotLayout::from_names(&["doc_id".to_string(), "score".to_string()]),
        OperatorKindSpec::Vector(
            VectorSpec::VectorSearch { .. }
            | VectorSpec::VectorLookup { .. }
            | VectorSpec::VectorMatch { .. },
        ) => SlotLayout::from_names(&["id".to_string(), "score".to_string()]),
    }
}

// ── Helper functions ──

pub(super) fn source_output_layout(spec: &SourceSpec) -> SlotLayout {
    match spec {
        SourceSpec::Start => SlotLayout::new(vec![]),
        SourceSpec::Argument { col_names } => SlotLayout::from_names(col_names),
        SourceSpec::ScanVertices { col_names, .. } => SlotLayout::from_names(col_names),
        SourceSpec::StandaloneValues { col_names, .. } => SlotLayout::from_names(col_names),
        SourceSpec::StorageScanVertices {
            col_names,
            projected_properties,
            ..
        } => SlotLayout::from_names(&flat_scan_col_names(col_names, projected_properties)),
        SourceSpec::ScanEdges { col_names, .. } => SlotLayout::from_names(col_names),
        SourceSpec::StorageScanEdges {
            col_names,
            projected_properties,
            ..
        } => SlotLayout::from_names(&flat_scan_col_names(col_names, projected_properties)),
        SourceSpec::GetVertices {
            col_names,
            projected_properties,
            ..
        } => SlotLayout::from_names(&flat_scan_col_names(col_names, projected_properties)),
        SourceSpec::GetEdges {
            projected_properties,
            ..
        } => SlotLayout::from_names(&flat_scan_col_names(
            &["edge".to_string()],
            projected_properties,
        )),
        SourceSpec::GetNeighbors {
            projected_properties,
            ..
        } => SlotLayout::from_names(&flat_scan_col_names(
            &["vertex".to_string()],
            projected_properties,
        )),
        SourceSpec::IndexScan { output_layout, .. } => (**output_layout).clone(),
        SourceSpec::GetProp { output_layout, .. } => (**output_layout).clone(),
    }
}

/// Append flat property columns (`{var}.{prop}`) after the entity column so
/// that scan sources expose a compound slot per projected property, letting
/// the columnar evaluator's `Property` branch hit without per-row extraction.
fn flat_scan_col_names(col_names: &[String], projected_properties: &[String]) -> Vec<String> {
    let mut names = col_names.to_vec();
    if let Some(var) = col_names.first() {
        names.extend(
            projected_properties
                .iter()
                .map(|prop| format!("{var}.{prop}")),
        );
    }
    names
}

pub(super) fn source_explain_name(spec: &SourceSpec) -> &'static str {
    match spec {
        SourceSpec::Start => "Start",
        SourceSpec::Argument { .. } => "Argument",
        SourceSpec::ScanVertices { .. } => "ScanVertices",
        SourceSpec::StandaloneValues { .. } => "StandaloneValues",
        SourceSpec::StorageScanVertices { .. } => "StorageScanVertices",
        SourceSpec::ScanEdges { .. } => "ScanEdges",
        SourceSpec::StorageScanEdges { .. } => "StorageScanEdges",
        SourceSpec::GetVertices { .. } => "GetVertices",
        SourceSpec::GetEdges { .. } => "GetEdges",
        SourceSpec::GetNeighbors { .. } => "GetNeighbors",
        SourceSpec::IndexScan { .. } => "IndexScan",
        SourceSpec::GetProp { .. } => "GetProp",
    }
}

pub(super) fn unary_explain_name(spec: &UnarySpec) -> &'static str {
    match spec {
        UnarySpec::Filter { .. } => "Filter",
        UnarySpec::Project { .. } => "Project",
        UnarySpec::Limit { .. } => "Limit",
        UnarySpec::Assign { .. } => "Assign",
        UnarySpec::Remove { .. } => "Remove",
        UnarySpec::Unwind { .. } => "Unwind",
        UnarySpec::AppendVertices { .. } => "AppendVertices",
        UnarySpec::Sample { .. } => "Sample",
    }
}

pub(super) fn blocking_explain_name(spec: &BlockingSpec) -> &'static str {
    match spec {
        BlockingSpec::Sort { .. } => "Sort",
        BlockingSpec::Aggregate { .. } => "Aggregate",
        BlockingSpec::GroupBy { .. } => "GroupBy",
        BlockingSpec::WindowFunction { .. } => "WindowFunction",
        BlockingSpec::Window { .. } => "Window",
        BlockingSpec::TopN { .. } => "TopN",
        BlockingSpec::Distinct => "Distinct",
        BlockingSpec::Materialize => "Materialize",
        BlockingSpec::DataCollect => "DataCollect",
        BlockingSpec::RollUpApply { .. } => "RollUpApply",
        BlockingSpec::PartialAggregate { .. } => "PartialAggregate",
        BlockingSpec::FinalAggregate { .. } => "FinalAggregate",
    }
}

pub(super) fn join_explain_name(spec: &JoinSpec) -> &'static str {
    match spec {
        JoinSpec::InnerJoin { .. } => "InnerJoin",
        JoinSpec::LeftJoin { .. } => "LeftJoin",
        JoinSpec::RightJoin { .. } => "RightJoin",
        JoinSpec::FullOuterJoin { .. } => "FullOuterJoin",
        JoinSpec::CrossJoin => "CrossJoin",
        JoinSpec::SemiJoin { .. } => "SemiJoin",
        JoinSpec::HashJoin { .. } => "HashJoin",
        JoinSpec::HashLeftJoin { .. } => "HashLeftJoin",
        JoinSpec::NestedLoopJoin { .. } => "NestedLoopJoin",
    }
}

pub(super) fn graph_explain_name(spec: &GraphSpec) -> &'static str {
    match spec {
        GraphSpec::Expand { .. } => "Expand",
        GraphSpec::ExpandAll { .. } => "ExpandAll",
        GraphSpec::Traverse { .. } => "Traverse",
        GraphSpec::BiExpand { .. } => "BiExpand",
        GraphSpec::BiTraverse { .. } => "BiTraverse",
    }
}

pub(super) fn recursive_fragment_explain_name(spec: &RecursiveFragmentSpec) -> &'static str {
    match spec {
        RecursiveFragmentSpec::ShortestPath { .. } => "RecursiveShortestPath",
        RecursiveFragmentSpec::MultiShortestPath { .. } => "RecursiveMultiShortestPath",
        RecursiveFragmentSpec::BFSShortest { .. } => "RecursiveBFSShortest",
        RecursiveFragmentSpec::AllPaths { .. } => "RecursiveAllPaths",
    }
}

pub(super) fn sink_explain_name(spec: &SinkSpec) -> &'static str {
    match spec {
        SinkSpec::CopyFrom { .. } => "CopyFrom",
        SinkSpec::CopyTo { .. } => "CopyTo",
        SinkSpec::InsertVertices { .. } => "InsertVertices",
        SinkSpec::InsertEdges { .. } => "InsertEdges",
        SinkSpec::UpdateVertices { .. } => "UpdateVertices",
        SinkSpec::UpdateEdges { .. } => "UpdateEdges",
        SinkSpec::DeleteVertices { .. } => "DeleteVertices",
        SinkSpec::DeleteEdges { .. } => "DeleteEdges",
        SinkSpec::PipeDeleteVertices { .. } => "PipeDeleteVertices",
        SinkSpec::PipeDeleteEdges { .. } => "PipeDeleteEdges",
        SinkSpec::DeleteTags { .. } => "DeleteTags",
    }
}

pub(super) fn set_explain_name(spec: &SetSpec) -> &'static str {
    match spec {
        SetSpec::Union => "Union",
        SetSpec::UnionAll => "UnionAll",
        SetSpec::Intersect => "Intersect",
        SetSpec::Except => "Except",
        SetSpec::Minus => "Minus",
    }
}

pub(super) fn apply_explain_name(spec: &ApplySpec) -> &'static str {
    match spec {
        ApplySpec::Apply { .. } => "Apply",
        ApplySpec::PatternApply { .. } => "PatternApply",
        ApplySpec::CorrelatedApply { .. } => "CorrelatedApply",
        ApplySpec::RollUpApply { .. } => "RollUpApply",
    }
}

pub(super) fn ddl_explain_name(spec: &DdlSpec) -> &'static str {
    match spec {
        DdlSpec::SpaceManage { .. } => "SpaceManage",
        DdlSpec::TagManage { .. } => "TagManage",
        DdlSpec::EdgeManage { .. } => "EdgeManage",
        DdlSpec::IndexManage { .. } => "IndexManage",
        DdlSpec::DeleteIndex { .. } => "DeleteIndex",
        DdlSpec::UserManage { .. } => "UserManage",
        DdlSpec::ShowStats { .. } => "ShowStats",
        DdlSpec::ShowConfigs { .. } => "ShowConfigs",
        DdlSpec::ShowQueries { .. } => "ShowQueries",
        DdlSpec::ShowSessions { .. } => "ShowSessions",
        DdlSpec::Analyze { .. } => "Analyze",
        DdlSpec::Migrate { .. } => "Migrate",
    }
}

pub(super) fn fulltext_explain_name(spec: &FulltextSpec) -> &'static str {
    match spec {
        FulltextSpec::FulltextManage { .. } => "FulltextManage",
        FulltextSpec::FulltextSearch { .. } => "FulltextSearch",
        FulltextSpec::FulltextLookup { .. } => "FulltextLookup",
        FulltextSpec::MatchFulltext { .. } => "MatchFulltext",
    }
}

pub(super) fn vector_explain_name(spec: &VectorSpec) -> &'static str {
    match spec {
        VectorSpec::VectorManage { .. } => "VectorManage",
        VectorSpec::VectorSearch { .. } => "VectorSearch",
        VectorSpec::VectorLookup { .. } => "VectorLookup",
        VectorSpec::VectorMatch { .. } => "VectorMatch",
    }
}

pub(super) fn txn_explain_name(spec: &TxnSpec) -> &'static str {
    match spec {
        TxnSpec::BeginTransaction => "BeginTransaction",
        TxnSpec::Commit => "Commit",
        TxnSpec::Rollback => "Rollback",
        TxnSpec::RollbackToSavepoint { .. } => "RollbackToSavepoint",
        TxnSpec::Savepoint { .. } => "Savepoint",
        TxnSpec::ReleaseSavepoint { .. } => "ReleaseSavepoint",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_output_layout_argument_uses_outer_col_names() {
        let spec = SourceSpec::Argument {
            col_names: vec!["t".to_string(), "t.name".to_string()],
        };
        let layout = source_output_layout(&spec);
        assert_eq!(layout.len(), 2, "argument layout mirrors outer slot count");
        assert_eq!(
            layout.slot_id("t"),
            Some(0),
            "first outer column lands in slot 0"
        );
        assert_eq!(
            layout.slot_id("t.name"),
            Some(1),
            "second outer column lands in slot 1"
        );
        assert_eq!(
            layout.slot_id("unknown"),
            None,
            "unrelated name must not resolve"
        );
    }
}
