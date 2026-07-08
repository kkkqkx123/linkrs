//! Verification Policy Module
//! Implementation that includes all verification strategies

pub mod aggregate_strategy;
pub mod alias_strategy;
pub mod clause_strategy;
pub mod expression_operations;
pub mod expression_strategy;
pub mod pagination_strategy;

pub mod helpers;
pub mod metadata;

#[cfg(test)]
pub mod expression_strategy_test;

pub use aggregate_strategy::*;
pub use alias_strategy::*;
pub use clause_strategy::*;
pub use expression_operations::*;
pub use expression_strategy::*;
pub use pagination_strategy::*;

pub use helpers::{
    deduce_expression_type, ExpressionChecker, ExpressionValidationContext, TypeDeduceValidator,
    TypeValidator, VariableChecker,
};
pub use metadata::AggFunctionMeta;
