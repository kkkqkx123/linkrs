//! Auxiliary Verification Tool Module
//! Provide underlying validation tools such as type checking, variable checking, and expression checking.

pub mod expression_checker;
pub mod type_checker;
pub mod variable_checker;

pub use expression_checker::ExpressionChecker;
pub use type_checker::{
    deduce_expression_type, ExpressionValidationContext, TypeDeduceValidator, TypeValidator,
};
pub use variable_checker::VariableChecker;
