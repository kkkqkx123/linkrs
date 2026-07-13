pub mod builder;
pub mod context;
pub mod node;
pub mod properties;
pub mod types;
pub mod validator;

pub use context::PhysicalPlanBuildContext;
pub use node::PhysicalNode;
pub use types::{
    CapabilitySet, FragmentGraph, FragmentId, FragmentIdAllocator, FragmentKind, FragmentSpec,
    LogicalNodeId, OperatorKindSpec, OutputContract, PhysicalOperatorId,
    PhysicalOperatorIdAllocator, PhysicalOperatorSpec, PhysicalPlan, PlanCompatibility,
};
pub use validator::{PhysicalPlanValidator, ValidationResult, ValidationTier};
