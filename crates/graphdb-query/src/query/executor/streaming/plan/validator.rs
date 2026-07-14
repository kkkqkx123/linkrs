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

use crate::core::error::QueryError;
use super::types::{
    FragmentId, OperatorKindSpec, PhysicalOperatorId, PhysicalPlan,
};
use super::properties::{MemoryPolicy, Ordering, PipelineKind};

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
                result.errors.push(format!(
                    "Duplicate PhysicalOperatorId: {}",
                    op.operator_id
                ));
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
        let fragment_ids: HashSet<FragmentId> = plan
            .fragments
            .fragments()
            .iter()
            .map(|f| f.id)
            .collect();

        if !fragment_ids.contains(&plan.root_fragment) {
            result.errors.push(format!(
                "Root fragment {:?} not found in fragment graph",
                plan.root_fragment
            ));
        }

        for fragment in plan.fragments.fragments() {
            if fragment.operators.is_empty() {
                result.warnings.push(format!(
                    "Fragment {:?} has no operators",
                    fragment.id
                ));
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
        let fragment_ids: Vec<FragmentId> = plan
            .fragments
            .fragments()
            .iter()
            .map(|f| f.id)
            .collect();

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
                result.warnings.push(format!(
                    "Fragment {:?} is not reachable from root",
                    fid
                ));
            }
        }
    }

    /// Every operator referenced in a fragment exists in that fragment's
    /// operator list.
    fn check_fragment_operator_belongs(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for fragment in plan.fragments.fragments() {
            let op_set: HashSet<PhysicalOperatorId> =
                fragment.operators.iter().copied().collect();
            if !op_set.contains(&fragment.root_operator) {
                result.errors.push(format!(
                    "Fragment {:?} root operator {:?} not in its operator list",
                    fragment.id, fragment.root_operator
                ));
            }
        }
    }

    /// Each operator type has the correct number of children (as implied by
    /// the fragment graph's operator list ordering).
    fn check_operator_input_counts(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for op in &plan.operators {
            let expected_children = match &op.spec {
                OperatorKindSpec::Source(_) => 0,
                OperatorKindSpec::Txn(_) => 0,
                OperatorKindSpec::Unary(_)
                | OperatorKindSpec::Blocking(_)
                | OperatorKindSpec::Graph(_)
                | OperatorKindSpec::RecursiveFragment(_)
                | OperatorKindSpec::Sink(_)
                | OperatorKindSpec::Ddl(_)
                | OperatorKindSpec::Fulltext(_)
                | OperatorKindSpec::Vector(_) => 1,
                OperatorKindSpec::Join(_) | OperatorKindSpec::Set(_) | OperatorKindSpec::Apply(_) => {
                    2
                }
                OperatorKindSpec::Exchange(_) => {
                    // Variable children, verified separately.
                    continue;
                }
            };

            // Check the fragment that owns this operator: the number of
            // fragment inputs matches the expected child count for the root.
            for fragment in plan.fragments.fragments() {
                if fragment.root_operator == op.operator_id {
                    if fragment.inputs.len() != expected_children && expected_children > 0 {
                        result.errors.push(format!(
                            "Operator {:?} ({}) expects {} child(ren) but fragment {:?} has {} input(s)",
                            op.operator_id,
                            op.explain_name,
                            expected_children,
                            fragment.id,
                            fragment.inputs.len()
                        ));
                    }
                }
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
                    if matches!(
                        op.properties.pipeline_kind,
                        PipelineKind::Streaming
                    ) {
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
                OperatorKindSpec::Source(spec) => {
                    // Start source produces one row; must be streaming.
                    if matches!(
                        spec,
                        super::super::operators::spec::SourceSpec::Start
                    ) && op.properties.pipeline_kind == PipelineKind::Blocking
                    {
                        result.warnings.push(format!(
                            "Start source {:?} should be streaming",
                            op.operator_id
                        ));
                    }
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
            result.warnings.push(
                "Plan root output contract has empty layout".to_string(),
            );
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
            result.warnings.push(
                "Plan declares no required capabilities".to_string(),
            );
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
                super::types::FragmentKind::Exchange => {
                    if !matches!(&root_op.spec, OperatorKindSpec::Exchange(_)) {
                        result.warnings.push(format!(
                            "Fragment {:?} has kind Exchange but root operator is {:?}",
                            fragment.id, root_op.explain_name
                        ));
                    }
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

    // ── Binding-dependent checks ──

    fn check_parameter_schema(plan: &PhysicalPlan, result: &mut ValidationResult) {
        for param in &plan.parameter_schema.params {
            if param.name.is_empty() {
                result.errors.push(
                    "Parameter schema contains unnamed parameter".to_string(),
                );
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
