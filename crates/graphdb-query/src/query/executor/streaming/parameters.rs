//! Parameter binding and expression types for physical plans.
//!
//! Parameters are compiled from named references (`Expression::Parameter`) to
//! typed slots at plan-build time.  At execution time, a `ParameterFrame`
//! provides values by slot, avoiding string-based lookups in the hot path.

use std::collections::HashMap;

use crate::core::types::operators::BinaryOperator;
use crate::core::DataType;
use crate::core::Value;

/// A stable slot index for a query parameter within a physical plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParameterSlot(pub usize);

/// Describes one parameter in the plan's parameter schema.
#[derive(Debug, Clone)]
pub struct ParameterDesc {
    pub name: String,
    pub slot: ParameterSlot,
    pub value_type: Option<DataType>,
    pub nullable: bool,
    pub default: Option<Value>,
}

/// The parameter schema of a physical plan.
///
/// Stores the name-to-slot mapping and type information for all parameters
/// that the plan expects.  Attached to [`PhysicalPlan`](super::plan::types::PhysicalPlan)
/// so that bindings can be validated before execution.
#[derive(Debug, Clone, Default)]
pub struct ParameterSchema {
    pub params: Vec<ParameterDesc>,
    pub name_to_slot: HashMap<String, ParameterSlot>,
}

impl ParameterSchema {
    pub fn new(params: Vec<ParameterDesc>) -> Self {
        let name_to_slot = params
            .iter()
            .map(|p| (p.name.clone(), p.slot))
            .collect();
        Self {
            params,
            name_to_slot,
        }
    }

    pub fn slot(&self, name: &str) -> Option<ParameterSlot> {
        self.name_to_slot.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.params.len()
    }

    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
}

/// Runtime parameter values, bound at instance time.
///
/// Created from [`QueryBindings`](super::instance::QueryBindings) after
/// validation and type coercion.  Shared across all tasks in a query
/// instance via `Arc<ParameterFrame>`.
#[derive(Debug, Clone)]
pub struct ParameterFrame {
    pub values: Vec<Value>,
}

impl ParameterFrame {
    pub fn new(values: Vec<Value>) -> Self {
        Self { values }
    }

    pub fn get(&self, slot: ParameterSlot) -> Option<&Value> {
        self.values.get(slot.0)
    }
}

/// Slot reference: refers to a column in the input chunk by position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotRef(pub usize);

/// A bound expression that has been resolved against the plan's slot layout
/// and parameter schema.
///
/// Unlike the logical [`Expression`](crate::core::types::expr::Expression),
/// `BoundExpression` references slots and parameter slots directly, avoiding
/// string-based resolution during execution.
#[derive(Debug, Clone)]
pub enum BoundExpression {
    Literal(Value),
    SlotRef(SlotRef),
    ParameterRef(ParameterSlot),
    Call {
        name: String,
        args: Vec<BoundExpression>,
    },
    Binary {
        left: Box<BoundExpression>,
        op: BinaryOperator,
        right: Box<BoundExpression>,
    },
    /// Cast the result of an expression to a target type.
    Cast {
        expr: Box<BoundExpression>,
        target_type: DataType,
    },
}
