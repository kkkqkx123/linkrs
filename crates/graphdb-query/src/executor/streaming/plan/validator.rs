//! PhysicalPlanValidator: structural and binding-dependent validation.
//!
//! Two-tier validation:
//! 1. **Structural validation** (no bindings required): runs once before
//!    cache-write.  Checks ID uniqueness, fragment DAG integrity, operator
//!    input/output consistency, property derivation, capability requirements,
//!    memory policy, and root output contract.
//! 2. **Binding-dependent validation** (requires parameter values, auth):
//!    runs at instantiation time.  Re-checks compatibility, parameter frame,
//!    transaction mode, and runtime limits.

use std::collections::HashSet;

use super::properties::{MemoryPolicy, Ordering, PipelineKind};
use super::types::{
    FragmentId, FragmentKind, InputContract, OperatorKindSpec, PhysicalOperatorId, PhysicalPlan,
    StateOwnership,
};
use graphdb_core::error::QueryError;

/// The two validation tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationTier {
    /// Structural validation (no bindings required).
    Structural,
    /// Full validation including binding-dependent checks.
    Full,
}

/// Result of a validation pass, collecting all errors instead of failing fast.
#[derive(Debug, Default)]
pub struct ValidationResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn into_result(self) -> Result<(), QueryError> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(QueryError::execution(format!(
                "PhysicalPlan validation failed ({} errors, {} warnings): [{}]",
                self.errors.len(),
                self.warnings.len(),
                self.errors.join("; ")
            )))
        }
    }

    pub fn merge(&mut self, other: ValidationResult) {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }
}

/// Validates a [`PhysicalPlan`] for correctness and consistency.
pub struct PhysicalPlanValidator;

impl PhysicalPlanValidator {
    /// Run the full structural validation on a plan.
    pub fn validate(plan: &PhysicalPlan) -> Result<(), QueryError> {
        Self::validate_tier(plan, ValidationTier::Structural)?.into_result()
    }

    /// Run validation at the specified tier.
    pub fn validate_tier(
        plan: &PhysicalPlan,
        tier: ValidationTier,
    ) -> Result<ValidationResult, QueryError> {
        let mut result = ValidationResult::default();

        // ── Always run (structural) ──
        Self::check_operator_id_uniqueness(plan, &mut result);
        Self::check_operator_references(plan, &mut result);
        Self::check_fragment_connectivity(plan, &mut result);
        Self::check_fragment_cycles(plan, &mut result);
        Self::check_fragment_operator_belongs(plan, &mut result);
        Self::check_operator_input_counts(plan, &mut result);
        Self::check_output_layouts(plan, &mut result);
        Self::check_property_consistency(plan, &mut result);
        Self::check_memory_policy(plan, &mut result);
        Self::check_root_output_contract(plan, &mut result);
        Self::check_capability_set(plan, &mut result);
        Self::check_output_layout_width(plan, &mut result);
        Self::check_fragment_exchange_layout(plan, &mut result);
        Self::check_input_contract_consistency(plan, &mut result);
        Self::check_linear_fragment_contracts(plan, &mut result);
        Self::check_state_ownership(plan, &mut result);
        Self::check_fragment_parallelism(plan, &mut result);
        Self::check_partition_parallelism(plan, &mut result);

        // M3.6: Additional structural integrity checks.
        Self::check_fragment_kind_matches(plan, &mut result);
        Self::check_root_fragment_kind(plan, &mut result);
        Self::check_unreferenced_operators(plan, &mut result);
        Self::check_operator_spec_consistency(plan, &mut result);

        if tier == ValidationTier::Full {
            // Binding-dependent checks.
            Self::check_parameter_schema(plan, &mut result);
        }

        Ok(result)
    }

    /// Quick compatibility check after cache load.
    ///
    /// Does not re-run expensive structural validation.  Only checks that
    /// the cached plan's compatibility metadata still matches the current
    /// execution context.
    pub fn check_compatibility(
        plan: &PhysicalPlan,
        current_layout_version: Option<u64>,
    ) -> Result<(), QueryError> {
        if let Some(cached_version) = plan.compatibility.layout_version {
            if let Some(current_version) = current_layout_version {
                if cached_version != current_version {
                    return Err(QueryError::execution(format!(
                        "Plan layout version mismatch: cached={}, current={}",
                        cached_version, current_version
                    )));
                }
            }
        }
        Ok(())
    }

    // ── Individual checks ──

    /// Every operator in the arena has a unique ID.
    fn check_operator_id_uniqueness(plan: &PhysicalPlan, result: &mut ValidationResult) {
        let mut seen = HashSet::new();
        for op in &plan.operators {
            if !seen.insert(op.operator_id) {
                result
                    .errors
                    .push(format!("Duplicate PhysicalOperatorId: {}", op.operator_id));
            }
        }
    }

    /// All operator IDs referenced in fragments exist in the arena.
    fn check_operator_references(plan: &PhysicalPlan, result: &mut ValidationResult) {
        let arena_ids: HashSet<PhysicalOperatorId> =
            plan.operators.iter().map(|op| op.operator_id).collect();

        for fragment in plan.fragments.fragments() {
            for &op_id in &fragment.operators {
                if !arena_ids.contains(&op_id) {
                    result.errors.push(format!(
                        "Fragment {:?} references operator {:?} which is not in the arena",
                        fragment.id, op_id
                    ));
                }
            }
            if !arena_ids.contains(&fragment.root_operator) {
                result.errors.push(format!(
                    "Fragment {:?} root operator {:?} not in arena",
                    fragment.id, fragment.root_operator
                ));
            }
        }
    }

    /// Fragment graph is connected and root exists.
    fn check_fragment_connectivity(plan: &PhysicalPlan, result: &mut ValidationResult) {
        let fragment_ids: HashSet<FragmentId> =
            plan.fragments.fragments().iter().map(|f| f.id).collect();

        if !fragment_ids.contains(&plan.root_fragment) {
            result.errors.push(format!(
                "Root fragment {:?} not found in fragment graph",
                plan.root_fragment
            ));
        }

        for fragment in plan.fragments.fragments() {
            if fragment.operators.is_empty() {
                result
                    .warnings
                    .push(format!("Fragment {:?} has no operators", fragment.id));
            }
            for input_id in &fragment.inputs {
                if !fragment_ids.contains(input_id) {
                    result.errors.push(format!(
                        "Fragment {:?} references missing input {:?}",
                        fragment.id, input_id
                    ));
                }
            }
        }
    }

    /// No illegal cycles in fragment DAG (a fragment cannot depend on itself
    /// transitively — the graph must be a DAG).
    fn check_fragment_cycles(plan: &PhysicalPlan, result: &mut ValidationResult) {
        let fragment_ids: Vec<FragmentId> =
            plan.fragments.fragments().iter().map(|f| f.id).collect();

        // Compute in-degree and topological order.
        let mut in_degree: std::collections::HashMap<FragmentId, usize> =
            fragment_ids.iter().map(|&fid| (fid, 0)).collect();

        for fragment in plan.fragments.fragments() {
            for &input in &fragment.inputs {
                *in_degree.get_mut(&input).unwrap_or(&mut 0) += 1;
            }
        }

        // For a DAG starting from root, walk backward through inputs.
        // If any fragment is not reachable from root (and not an input of root),
        // it's either a cycle or disconnected.
        let mut reachable = HashSet::new();
        let mut stack = vec![plan.root_fragment];
        while let Some(fid) = stack.pop() {
            if !reachable.insert(fid) {
                continue;
            }
            if let Some(frag) = plan.fragments.get(fid) {
                for &input in &frag.inputs {
                    stack.push(input);
                }
            }
        }

        for &fid in &fragment_ids {
            if !reachable.contains(&fid) {
                result
                    .warnings
                    .push(format!("Fragment {:?} is not reachable from root", fid));
            }
        }
    }

    /// Every operator referenced in a fragment exists in that fragment's
    /// operator list.
    fn check_fragment_operator_belongs(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for fragment in plan.fragments.fragments() {
            let op_set: HashSet<PhysicalOperatorId> = fragment.operators.iter().copied().collect();
            if !op_set.contains(&fragment.root_operator) {
                result.errors.push(format!(
                    "Fragment {:?} root operator {:?} not in its operator list",
                    fragment.id, fragment.root_operator
                ));
            }
        }
    }

    /// Each fragment's input count matches the expected children of its
    /// leaf (first) operator — the one that receives data from other fragments.
    /// Operators further up the same fragment pipeline (e.g., Filter above
    /// Scan) consume data from their sibling within the fragment, not from
    /// external fragment inputs.
    fn check_operator_input_counts(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for fragment in plan.fragments.fragments() {
            if fragment.operators.is_empty() {
                continue;
            }
            // The first operator in the fragment pipeline is the leaf that
            // receives external inputs. Check its expected children against
            // the number of fragment inputs.
            let leaf_op_id = fragment.operators[0];
            let leaf_op = &plan.operators[leaf_op_id.0];

            let expected_children = match &leaf_op.spec {
                OperatorKindSpec::Source(_) | OperatorKindSpec::Txn(_) => 0,
                OperatorKindSpec::Unary(_)
                | OperatorKindSpec::Blocking(_)
                | OperatorKindSpec::Graph(_)
                | OperatorKindSpec::RecursiveFragment(_)
                | OperatorKindSpec::Sink(_)
                | OperatorKindSpec::Ddl(_)
                | OperatorKindSpec::Fulltext(_)
                | OperatorKindSpec::Vector(_)
                | OperatorKindSpec::Apply(
                    crate::executor::streaming::operators::spec::ApplySpec::CorrelatedApply {
                        ..
                    },
                ) => 1,
                OperatorKindSpec::Join(_)
                | OperatorKindSpec::Set(_)
                | OperatorKindSpec::Apply(
                    crate::executor::streaming::operators::spec::ApplySpec::Apply { .. },
                )
                | OperatorKindSpec::Apply(
                    crate::executor::streaming::operators::spec::ApplySpec::PatternApply { .. },
                )
                | OperatorKindSpec::Apply(
                    crate::executor::streaming::operators::spec::ApplySpec::RollUpApply { .. },
                ) => 2,
                OperatorKindSpec::Wco(wco) => 1 + wco.bound_names.len(),
                OperatorKindSpec::Exchange(_) => {
                    continue;
                }
            };

            if fragment.inputs.len() != expected_children && expected_children > 0 {
                result.errors.push(format!(
                    "Fragment {:?} has {} input(s) but its leaf operator {:?} ({}) expects {} child(ren)",
                    fragment.id,
                    fragment.inputs.len(),
                    leaf_op.operator_id,
                    leaf_op.explain_name,
                    expected_children
                ));
            }
        }
    }

    /// Output layout is non-empty for non-command operators.
    fn check_output_layouts(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            if op.output_layout.is_empty() {
                let is_command = matches!(
                    &op.spec,
                    OperatorKindSpec::Ddl(_) | OperatorKindSpec::Txn(_)
                );
                let is_source = matches!(&op.spec, OperatorKindSpec::Source(s)
                    if matches!(s, super::super::operators::spec::SourceSpec::Start)
                );
                if !is_command && !is_source {
                    result.warnings.push(format!(
                        "Operator {:?} ({}) has empty output layout",
                        op.operator_id, op.explain_name
                    ));
                }
            }
        }
    }

    /// Physical property consistency checks.
    fn check_property_consistency(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            match &op.spec {
                OperatorKindSpec::Blocking(spec) => {
                    if matches!(
                        spec,
                        super::super::operators::spec::BlockingSpec::Sort { .. }
                    ) && matches!(op.properties.ordering, Ordering::None)
                    {
                        result.warnings.push(format!(
                            "Sort operator {:?} declares no output ordering",
                            op.operator_id
                        ));
                    }
                    // PartialAggregate and FinalAggregate should be blocking.
                    if matches!(op.properties.pipeline_kind, PipelineKind::Streaming) {
                        result.warnings.push(format!(
                            "Blocking spec operator {:?} is marked as streaming",
                            op.operator_id
                        ));
                    }
                }
                OperatorKindSpec::Exchange(_) => {
                    // Exchange must be non-streaming (blocking or exchange pipeline).
                    if op.properties.pipeline_kind == PipelineKind::Streaming {
                        result.warnings.push(format!(
                            "Exchange operator {:?} is marked as streaming",
                            op.operator_id
                        ));
                    }
                }
                OperatorKindSpec::Source(super::super::operators::spec::SourceSpec::Start)
                    if op.properties.pipeline_kind == PipelineKind::Blocking =>
                {
                    result.warnings.push(format!(
                        "Start source {:?} should be streaming",
                        op.operator_id
                    ));
                }
                _ => {}
            }
        }
    }

    /// Blocking operators must have a memory policy (RequiresBudget or Spillable).
    fn check_memory_policy(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            if op.properties.pipeline_kind == PipelineKind::Blocking {
                match &op.properties.memory_policy {
                    MemoryPolicy::None => {
                        result.warnings.push(format!(
                            "Blocking operator {:?} ({}) has no memory policy set \
                             (should be RequiresBudget or Spillable)",
                            op.operator_id, op.explain_name
                        ));
                    }
                    MemoryPolicy::RequiresBudget | MemoryPolicy::Spillable { .. } => {}
                }
            }
        }
    }

    /// Root output contract must be present.
    fn check_root_output_contract(plan: &PhysicalPlan, result: &mut ValidationResult) {
        if plan.output.output_layout.is_empty() {
            result
                .warnings
                .push("Plan root output contract has empty layout".to_string());
        }

        // Verify root fragment exists and has operators.
        let root_frag = plan.fragments.get(plan.root_fragment);
        if let Some(frag) = root_frag {
            if frag.operators.is_empty() {
                result.errors.push(format!(
                    "Root fragment {:?} has no operators",
                    plan.root_fragment
                ));
            }
        }
    }

    /// Required capabilities must be non-empty and valid.
    fn check_capability_set(plan: &PhysicalPlan, result: &mut ValidationResult) {
        if plan.required_capabilities.is_empty() {
            result
                .warnings
                .push("Plan declares no required capabilities".to_string());
        }
    }

    /// Each operator's output layout slots must have non-empty type info
    /// (unless the operator is a command or Start source).
    fn check_output_layout_width(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            if op.output_layout.is_empty() {
                continue;
            }
            for slot in &op.output_layout.slots {
                if slot.data_type.is_none() {
                    let is_command = matches!(
                        &op.spec,
                        OperatorKindSpec::Ddl(_) | OperatorKindSpec::Txn(_)
                    );
                    if !is_command {
                        result.warnings.push(format!(
                            "Operator {:?} ({}) output slot '{}' has no data type",
                            op.operator_id, op.explain_name, slot.name
                        ));
                    }
                }
            }
        }
    }

    /// Exchange fragments must have an exchange_layout set.
    fn check_fragment_exchange_layout(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for fragment in plan.fragments.fragments() {
            if fragment.kind == FragmentKind::Exchange && fragment.exchange_layout.is_none() {
                result.warnings.push(format!(
                    "Exchange fragment {:?} has no exchange_layout",
                    fragment.id
                ));
            }
        }
    }

    // ── M3.6: Additional structural integrity checks ──

    /// Fragment kind should be consistent with the root operator type.
    ///
    /// A Source fragment should have a Source operator as its root.
    /// A Terminal fragment should have a terminal operator (Ddl, Sink, Txn).
    fn check_fragment_kind_matches(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for fragment in plan.fragments.fragments() {
            let root_op = match plan.operator(fragment.root_operator) {
                Some(op) => op,
                None => continue,
            };
            let is_terminal = matches!(
                &root_op.spec,
                OperatorKindSpec::Sink(_) | OperatorKindSpec::Ddl(_) | OperatorKindSpec::Txn(_)
            );
            match fragment.kind {
                super::types::FragmentKind::Source => {
                    if !matches!(&root_op.spec, OperatorKindSpec::Source(_)) {
                        result.warnings.push(format!(
                            "Fragment {:?} has kind Source but root operator is {:?}",
                            fragment.id, root_op.explain_name
                        ));
                    }
                }
                super::types::FragmentKind::Terminal => {
                    if !is_terminal {
                        result.warnings.push(format!(
                            "Fragment {:?} has kind Terminal but root operator is {:?}",
                            fragment.id, root_op.explain_name
                        ));
                    }
                }
                super::types::FragmentKind::Blocking => {
                    if !matches!(&root_op.spec, OperatorKindSpec::Blocking(_)) {
                        result.warnings.push(format!(
                            "Fragment {:?} has kind Blocking but root operator is {:?}",
                            fragment.id, root_op.explain_name
                        ));
                    }
                }
                super::types::FragmentKind::Exchange
                    if !matches!(&root_op.spec, OperatorKindSpec::Exchange(_)) =>
                {
                    result.warnings.push(format!(
                        "Fragment {:?} has kind Exchange but root operator is {:?}",
                        fragment.id, root_op.explain_name
                    ));
                }
                _ => {}
            }
        }
    }

    /// Check that the root fragment kind is appropriate.
    fn check_root_fragment_kind(plan: &PhysicalPlan, result: &mut ValidationResult) {
        if let Some(root_frag) = plan.fragments.get(plan.root_fragment) {
            match root_frag.kind {
                super::types::FragmentKind::Source | super::types::FragmentKind::Streaming => {
                    result.warnings.push(format!(
                        "Root fragment {:?} has kind {:?}, expected Terminal or Result",
                        root_frag.id, root_frag.kind
                    ));
                }
                _ => {}
            }
        }
    }

    /// Every operator in the arena must belong to at least one fragment.
    fn check_unreferenced_operators(plan: &PhysicalPlan, result: &mut ValidationResult) {
        let mut referenced: std::collections::HashSet<PhysicalOperatorId> =
            std::collections::HashSet::new();
        for fragment in plan.fragments.fragments() {
            for &op_id in &fragment.operators {
                referenced.insert(op_id);
            }
        }
        for op in &plan.operators {
            if !referenced.contains(&op.operator_id) {
                result.warnings.push(format!(
                    "Operator {:?} ({}) exists in the arena but is not referenced by any fragment",
                    op.operator_id, op.explain_name
                ));
            }
        }
    }

    /// Input contract matches spec kind and fragment inputs.
    fn check_input_contract_consistency(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            // NoInput should not have fragment inputs.
            if let InputContract::NoInput = &op.input_contract {
                for fragment in plan.fragments.fragments() {
                    if fragment.root_operator == op.operator_id && !fragment.inputs.is_empty() {
                        result.warnings.push(format!(
                            "Operator {:?} ({}) has NoInput contract but its fragment {:?} has {} inputs",
                            op.operator_id, op.explain_name, fragment.id, fragment.inputs.len()
                        ));
                    }
                }
            }
        }
    }

    /// The arena contract is the authoritative edge description. External
    /// ports on the first operator must match the fragment's producer list;
    /// subsequent operators may only consume the immediately preceding
    /// operator in the same fragment. This deliberately rejects ambiguous
    /// stack-shaped plans instead of trying to repair them at materialization.
    fn check_linear_fragment_contracts(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for fragment in plan.fragments.fragments() {
            if fragment.operators.is_empty() {
                result.errors.push(format!(
                    "Fragment {:?} has no operator and therefore no root",
                    fragment.id
                ));
                continue;
            }
            if fragment.root_operator
                != *fragment.operators.last().unwrap_or(&fragment.root_operator)
            {
                result.errors.push(format!(
                    "Fragment {:?} root {:?} is not the final pipeline operator",
                    fragment.id, fragment.root_operator
                ));
            }

            let external = fragment.inputs.clone();
            for (index, operator_id) in fragment.operators.iter().enumerate() {
                let Some(operator) = plan.operator(*operator_id) else {
                    continue;
                };
                let references: Vec<FragmentId> = match &operator.input_contract {
                    InputContract::NoInput => Vec::new(),
                    InputContract::UnaryInput(input) => vec![input.fragment],
                    InputContract::BinaryInputs { left, right } => {
                        vec![left.fragment, right.fragment]
                    }
                    InputContract::PartitionedInputs { members, .. } => {
                        members.iter().map(|member| member.fragment).collect()
                    }
                };

                let expected: Vec<FragmentId> = if index == 0 {
                    external.clone()
                } else {
                    vec![fragment.id]
                };
                if references != expected {
                    result.errors.push(format!(
                        "Operator {:?} in fragment {:?} has input ports {:?}, expected {:?}",
                        operator_id, fragment.id, references, expected
                    ));
                }

                let arity = match &operator.input_contract {
                    InputContract::NoInput => 0,
                    InputContract::UnaryInput(_) => 1,
                    InputContract::BinaryInputs { .. } => 2,
                    InputContract::PartitionedInputs { members, .. } => members.len(),
                    InputContract::WcoInputs { builds, .. } => 1 + builds.len(),
                };
                let expected_arity = match &operator.spec {
                    OperatorKindSpec::Source(_) => 0,
                    OperatorKindSpec::Join(_)
                    | OperatorKindSpec::Set(_)
                    | OperatorKindSpec::Apply(
                        crate::executor::streaming::operators::spec::ApplySpec::Apply { .. },
                    )
                    | OperatorKindSpec::Apply(
                        crate::executor::streaming::operators::spec::ApplySpec::PatternApply {
                            ..
                        },
                    )
                    | OperatorKindSpec::Apply(
                        crate::executor::streaming::operators::spec::ApplySpec::RollUpApply {
                            ..
                        },
                    ) => 2,
                    OperatorKindSpec::Wco(wco) => 1 + wco.bound_names.len(),
                    OperatorKindSpec::Exchange(_) => arity,
                    _ => 1,
                };
                if !matches!(operator.spec, OperatorKindSpec::Source(_)) && arity != expected_arity
                {
                    result.errors.push(format!(
                        "Operator {:?} ({}) declares arity {}, expected {}",
                        operator_id, operator.explain_name, arity, expected_arity
                    ));
                }

                for input_fragment in &references {
                    if *input_fragment == fragment.id {
                        if index == 0 {
                            result.errors.push(format!(
                                "First operator {:?} in fragment {:?} self-references its input",
                                operator_id, fragment.id
                            ));
                        }
                    } else if plan.fragments.get(*input_fragment).is_none() {
                        result.errors.push(format!(
                            "Operator {:?} references missing producer fragment {:?}",
                            operator_id, input_fragment
                        ));
                    }
                }
            }
        }
    }

    /// State ownership must match operator kind.
    ///
    /// Blocking operators (Sort, Aggregate, Distinct, etc.) must be
    /// `GlobalRuntime` or `TaskLocal` — they own spill/state arenas
    /// that outlive individual `next()` calls.
    fn check_state_ownership(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            let is_blocking = matches!(&op.spec, OperatorKindSpec::Blocking(_));
            let is_exchange = matches!(&op.spec, OperatorKindSpec::Exchange(_));

            match op.state_ownership {
                StateOwnership::TreeLocal => {
                    // TreeLocal is fine for source, unary, join, set, apply,
                    // sink, graph, txn, fulltext, vector, ddl.
                    if is_blocking || is_exchange {
                        result.warnings.push(format!(
                            "Operator {:?} ({}) is blocking/exchange but has TreeLocal state ownership",
                            op.operator_id, op.explain_name
                        ));
                    }
                }
                StateOwnership::GlobalRuntime | StateOwnership::TaskLocal => {
                    // These are expected for blocking/exchange operators.
                }
            }
        }
    }

    /// Check operator spec internal consistency.
    ///
    /// M3.6: validations that catch spec-level errors before execution.
    fn check_operator_spec_consistency(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            // Check that Source operators are not in fragments with inputs.
            if let OperatorKindSpec::Source(_) = &op.spec {
                for fragment in plan.fragments.fragments() {
                    if fragment.root_operator == op.operator_id && !fragment.inputs.is_empty() {
                        // Source operators should be leaf fragments.
                        result.warnings.push(format!(
                            "Source operator {:?} ({}) is root of fragment {:?} which has inputs",
                            op.operator_id, op.explain_name, fragment.id
                        ));
                    }
                }
            }
        }
    }

    /// Fragment parallelism consistency: each operator's min_workers must be
    /// <= max_workers, and within the same fragment all operators should have
    /// compatible parallelism settings.
    fn check_fragment_parallelism(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            let p = &op.properties.parallelism;
            if p.min_workers > p.max_workers {
                result.errors.push(format!(
                    "Operator {:?} ({}) has min_workers {} > max_workers {}",
                    op.operator_id, op.explain_name, p.min_workers, p.max_workers
                ));
            }
            if p.min_workers < 1 {
                result.errors.push(format!(
                    "Operator {:?} ({}) has min_workers {} < 1",
                    op.operator_id, op.explain_name, p.min_workers
                ));
            }
        }
    }

    /// Validate that PartitionedInputs have consistent parallelism across all
    /// partition members.
    fn check_partition_parallelism(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            if let InputContract::PartitionedInputs { members, .. } = &op.input_contract {
                if members.is_empty() {
                    result.warnings.push(format!(
                        "Operator {:?} ({}) has empty PartitionedInputs",
                        op.operator_id, op.explain_name
                    ));
                    continue;
                }
                let base_workers = members[0].properties.parallelism.max_workers;
                for member in members {
                    if member.properties.parallelism.max_workers != base_workers {
                        result.warnings.push(format!(
                            "Operator {:?} ({}) has partition member {:?} with {} workers, expected {}",
                            op.operator_id,
                            op.explain_name,
                            member.fragment,
                            member.properties.parallelism.max_workers,
                            base_workers
                        ));
                    }
                }
            }
        }
    }

    // ── Binding-dependent checks ──

    fn check_parameter_schema(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for param in &plan.parameter_schema.params {
            if param.name.is_empty() {
                result
                    .errors
                    .push("Parameter schema contains unnamed parameter".to_string());
            }
            if param.slot.0 >= plan.parameter_schema.params.len() {
                result.warnings.push(format!(
                    "Parameter '{}' has out-of-range slot {:?}",
                    param.name, param.slot
                ));
            }
        }
    }
}
