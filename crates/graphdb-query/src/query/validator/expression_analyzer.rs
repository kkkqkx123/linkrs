//! Expression Analyzer
//!
//! A unified expression analysis interface, responsible for:
//! Type inference – Store the results in the ExpressionContext
//! 2. Constant folding – Store the result in the ExpressionContext
//! 3. Expression validation – Checking the validity of an expression
//!
//! Design Principles:
//! All analysis results are stored in the ExpressionContext to avoid scattered storage.
//! Utilize caching to avoid duplicate analyses.
//! Incremental analysis is supported.

use crate::core::types::expr::{ContextualExpression, Expression};
use crate::core::types::DataType;
use crate::core::Value;
use crate::query::validator::error::{ValidationError, ValidationErrorType};

/// Expression analysis results
#[derive(Debug, Clone)]
pub struct ExpressionAnalysisResult {
    /// Derived type
    pub data_type: DataType,
    /// Is it a constant?
    pub is_constant: bool,
    /// Constant values (if they are constants)
    pub constant_value: Option<Value>,
    /// List of included variables
    pub variables: Vec<String>,
    /// Does it contain aggregate functions?
    pub has_aggregate: bool,
}

impl ExpressionAnalysisResult {
    /// Create new analysis results.
    pub fn new(data_type: DataType) -> Self {
        Self {
            data_type,
            is_constant: false,
            constant_value: None,
            variables: Vec::new(),
            has_aggregate: false,
        }
    }

    /// Create constant analysis results
    pub fn constant(data_type: DataType, value: Value) -> Self {
        Self {
            data_type,
            is_constant: true,
            constant_value: Some(value),
            variables: Vec::new(),
            has_aggregate: false,
        }
    }
}

/// Expression Analyzer
///
/// A unified expression analysis interface that integrates type inference, constant folding, and validation capabilities.
/// All analysis results are stored in the ExpressionContext to ensure data consistency.
pub struct ExpressionAnalyzer;

impl ExpressionAnalyzer {
    /// Create a new expression analyzer.
    pub fn new() -> Self {
        Self
    }

    /// Analyze the expression
    ///
    /// Perform a complete analysis of the expression:
    /// 1. Check the cache; if the data has already been analyzed, return it directly.
    /// 2. Perform type inference
    /// 3. Perform constant folding.
    /// 4. Collecting variable information
    /// 5. Store the results in the ExpressionContext
    ///
    /// # Parameters
    /// `expr`: The context expression to be analyzed.
    /// `variable_types`: A mapping of variable types (used for type inference)
    ///
    /// # Return
    /// Analysis results, including information on the type and constants.
    pub fn analyze(
        &self,
        expr: &ContextualExpression,
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        // Check whether the analysis has been performed.
        if let Some(data_type) = expr.data_type() {
            // The analysis has been completed; the cached result will be returned directly.
            let constant_value = expr.constant_value();
            let is_constant = constant_value.is_some();

            return Ok(ExpressionAnalysisResult {
                data_type,
                is_constant,
                constant_value,
                variables: expr.get_variables(),
                has_aggregate: expr.contains_aggregate(),
            });
        }

        // Obtain the expression
        let inner_expr = match expr.get_expression() {
            Some(e) => e,
            None => {
                return Err(ValidationError::new(
                    "Invalid or non-existent expression".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // Perform analysis
        let result = self.analyze_expression(&inner_expr, variable_types)?;

        // Store the results in the ExpressionContext.
        self.store_result(expr, &result)?;

        Ok(result)
    }

    /// Analyzing expressions (internal methods)
    fn analyze_expression(
        &self,
        expr: &Expression,
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        match expr {
            Expression::Literal(value) => {
                let data_type = value.get_type();
                Ok(ExpressionAnalysisResult::constant(
                    data_type.clone(),
                    value.clone(),
                ))
            }

            Expression::Variable(name) => {
                let data_type = variable_types
                    .and_then(|types| types.get(name))
                    .cloned()
                    .unwrap_or(DataType::Empty);

                let mut result = ExpressionAnalysisResult::new(data_type);
                result.variables.push(name.clone());
                Ok(result)
            }

            Expression::Binary { op, left, right } => {
                self.analyze_binary_expression(op, left, right, variable_types)
            }

            Expression::Unary { op, operand } => {
                self.analyze_unary_expression(op, operand, variable_types)
            }

            Expression::Function { name, args } => {
                self.analyze_function_call(name, args, variable_types)
            }

            Expression::Aggregate {
                func,
                arg,
                distinct: _,
            } => self.analyze_aggregate_expression(func, arg, variable_types),

            Expression::Property {
                object,
                property: _,
            } => {
                // Attribute access expressions; the type depends on the object.
                let obj_result = self.analyze_expression(object, variable_types)?;
                let mut result = ExpressionAnalysisResult::new(DataType::Empty);
                result.variables = obj_result.variables;
                Ok(result)
            }

            Expression::Subscript { collection, index } => {
                self.analyze_subscript_expression(collection, index, variable_types)
            }

            Expression::List(elements) => self.analyze_list_expression(elements, variable_types),

            Expression::Map(pairs) => self.analyze_map_expression(pairs, variable_types),

            Expression::Case {
                test_expr,
                conditions,
                default,
            } => self.analyze_case_expression(
                test_expr.as_deref(),
                conditions,
                default.as_deref(),
                variable_types,
            ),

            _ => Ok(ExpressionAnalysisResult::new(DataType::Empty)),
        }
    }

    /// Analyzing binary expressions
    fn analyze_binary_expression(
        &self,
        op: &crate::core::BinaryOperator,
        left: &Expression,
        right: &Expression,
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        use crate::core::BinaryOperator;

        let left_result = self.analyze_expression(left, variable_types)?;
        let right_result = self.analyze_expression(right, variable_types)?;

        // Type of the derived result
        let data_type = match op {
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual
            | BinaryOperator::And
            | BinaryOperator::Or => DataType::Bool,

            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Modulo
            | BinaryOperator::Exponent => {
                self.deduce_arithmetic_type(&left_result.data_type, &right_result.data_type)
            }

            BinaryOperator::StringConcat => DataType::String,

            _ => DataType::Empty,
        };

        // Try constant folding.
        let constant_value = if left_result.is_constant && right_result.is_constant {
            self.fold_binary_constant(
                op,
                left_result.constant_value.as_ref(),
                right_result.constant_value.as_ref(),
            )
        } else {
            None
        };

        let mut result = if let Some(value) = constant_value {
            ExpressionAnalysisResult::constant(data_type, value)
        } else {
            ExpressionAnalysisResult::new(data_type)
        };

        // List of merged variables
        result.variables = left_result.variables;
        result.variables.extend(right_result.variables);
        result.has_aggregate = left_result.has_aggregate || right_result.has_aggregate;

        Ok(result)
    }

    /// Analyzing a univariate expression
    fn analyze_unary_expression(
        &self,
        op: &crate::core::UnaryOperator,
        operand: &Expression,
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        use crate::core::UnaryOperator;

        let operand_result = self.analyze_expression(operand, variable_types)?;

        let data_type = match op {
            UnaryOperator::Not => DataType::Bool,
            UnaryOperator::IsNull
            | UnaryOperator::IsNotNull
            | UnaryOperator::IsEmpty
            | UnaryOperator::IsNotEmpty => DataType::Bool,
            UnaryOperator::Minus | UnaryOperator::Plus => operand_result.data_type.clone(),
        };

        // Try constant folding.
        let constant_value = if operand_result.is_constant {
            self.fold_unary_constant(op, operand_result.constant_value.as_ref())
        } else {
            None
        };

        let mut result = if let Some(value) = constant_value {
            ExpressionAnalysisResult::constant(data_type, value)
        } else {
            ExpressionAnalysisResult::new(data_type)
        };

        result.variables = operand_result.variables;
        result.has_aggregate = operand_result.has_aggregate;

        Ok(result)
    }

    /// Analyzing function calls
    fn analyze_function_call(
        &self,
        name: &str,
        args: &[Expression],
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        // Analysis of parameters
        let mut all_constant = true;
        let mut variables = Vec::new();

        for arg in args {
            let arg_result = self.analyze_expression(arg, variable_types)?;
            if !arg_result.is_constant {
                all_constant = false;
            }
            variables.extend(arg_result.variables);
        }

        // Deriving the return type
        let data_type = self.deduce_function_return_type(name, args, variable_types);

        let mut result = ExpressionAnalysisResult::new(data_type);
        result.variables = variables;
        result.is_constant = all_constant; // Function calls are usually not constants, unless they are built-in, pure functions.

        Ok(result)
    }

    /// Analyzing aggregate expressions
    fn analyze_aggregate_expression(
        &self,
        func: &crate::core::AggregateFunction,
        arg: &Expression,
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        let arg_result = self.analyze_expression(arg, variable_types)?;

        let data_type = self.deduce_aggregate_return_type(func, &arg_result.data_type);

        let mut result = ExpressionAnalysisResult::new(data_type);
        result.variables = arg_result.variables;
        result.has_aggregate = true;

        Ok(result)
    }

    /// Analyze the subscript expression.
    fn analyze_subscript_expression(
        &self,
        collection: &Expression,
        index: &Expression,
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        let coll_result = self.analyze_expression(collection, variable_types)?;
        let index_result = self.analyze_expression(index, variable_types)?;

        // The type of the subscript expression depends on the type of the set.
        let data_type = match &coll_result.data_type {
            DataType::List => DataType::Empty, // It is not possible to determine the type of the element.
            DataType::Map => DataType::Empty,
            _ => DataType::Empty,
        };

        let mut result = ExpressionAnalysisResult::new(data_type);
        result.variables = coll_result.variables;
        result.variables.extend(index_result.variables);

        Ok(result)
    }

    /// Analyzing list expressions
    fn analyze_list_expression(
        &self,
        elements: &[Expression],
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        let mut all_constant = true;
        let mut variables = Vec::new();

        for elem in elements {
            let elem_result = self.analyze_expression(elem, variable_types)?;
            if !elem_result.is_constant {
                all_constant = false;
            }
            variables.extend(elem_result.variables);
        }

        let mut result = ExpressionAnalysisResult::new(DataType::List);
        result.variables = variables;
        result.is_constant = all_constant;

        Ok(result)
    }

    /// Analyzing mapping expressions
    fn analyze_map_expression(
        &self,
        pairs: &[(String, Expression)],
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        let mut all_constant = true;
        let mut variables = Vec::new();

        for (_, value) in pairs {
            let value_result = self.analyze_expression(value, variable_types)?;
            if !value_result.is_constant {
                all_constant = false;
            }
            variables.extend(value_result.variables);
        }

        let mut result = ExpressionAnalysisResult::new(DataType::Map);
        result.variables = variables;
        result.is_constant = all_constant;

        Ok(result)
    }

    /// Analyzing CASE expressions
    fn analyze_case_expression(
        &self,
        test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
        variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> Result<ExpressionAnalysisResult, ValidationError> {
        let mut all_constant = true;
        let mut variables = Vec::new();
        let mut result_type = DataType::Empty;

        // Analyze the test expression
        if let Some(test) = test_expr {
            let test_result = self.analyze_expression(test, variable_types)?;
            if !test_result.is_constant {
                all_constant = false;
            }
            variables.extend(test_result.variables);
        }

        // Analysis of conditions and results
        for (condition, value) in conditions {
            let cond_result = self.analyze_expression(condition, variable_types)?;
            let value_result = self.analyze_expression(value, variable_types)?;

            if !cond_result.is_constant || !value_result.is_constant {
                all_constant = false;
            }

            variables.extend(cond_result.variables);
            variables.extend(value_result.variables);

            // Merge result type
            if result_type == DataType::Empty {
                result_type = value_result.data_type;
            }
        }

        // Analysis of “default”
        if let Some(default_expr) = default {
            let default_result = self.analyze_expression(default_expr, variable_types)?;
            if !default_result.is_constant {
                all_constant = false;
            }
            variables.extend(default_result.variables);

            if result_type == DataType::Empty {
                result_type = default_result.data_type;
            }
        }

        let mut result = ExpressionAnalysisResult::new(result_type);
        result.variables = variables;
        result.is_constant = all_constant;

        Ok(result)
    }

    /// Store the analysis results in the ExpressionContext.
    fn store_result(
        &self,
        expr: &ContextualExpression,
        result: &ExpressionAnalysisResult,
    ) -> Result<(), ValidationError> {
        let context = expr.context();
        let id = expr.id();

        // Storage type information
        context.set_type(id, result.data_type.clone());

        // Storing constant values
        if let Some(ref value) = result.constant_value {
            context.set_constant(id, value.clone());
        }

        Ok(())
    }

    /// Derivation of arithmetic expression types
    fn deduce_arithmetic_type(&self, left: &DataType, right: &DataType) -> DataType {
        let left_is_numeric = matches!(
            left,
            DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Float
                | DataType::Double
        );
        let right_is_numeric = matches!(
            right,
            DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Float
                | DataType::Double
        );

        if !left_is_numeric || !right_is_numeric {
            return DataType::Empty;
        }

        let left_is_float = matches!(left, DataType::Float | DataType::Double);
        let right_is_float = matches!(right, DataType::Float | DataType::Double);

        if left_is_float || right_is_float {
            DataType::Float
        } else {
            DataType::Int
        }
    }

    /// Deriving the return type of a function
    fn deduce_function_return_type(
        &self,
        name: &str,
        _args: &[Expression],
        _variable_types: Option<&std::collections::HashMap<String, DataType>>,
    ) -> DataType {
        match name.to_lowercase().as_str() {
            "abs" | "length" | "size" | "round" | "floor" | "ceil" => DataType::Int,
            "sqrt" | "pow" | "sin" | "cos" | "tan" => DataType::Float,
            "concat" | "substring" | "trim" | "ltrim" | "rtrim" | "upper" | "lower" | "type" => {
                DataType::String
            }
            "id" => DataType::Int,
            "properties" => DataType::Map,
            "labels" | "keys" | "values" | "range" | "reverse" => DataType::List,
            _ => DataType::Empty,
        }
    }

    /// Determine the return type of the aggregate function
    fn deduce_aggregate_return_type(
        &self,
        func: &crate::core::AggregateFunction,
        arg_type: &DataType,
    ) -> DataType {
        use crate::core::AggregateFunction;

        match func {
            AggregateFunction::Count(_) => DataType::Int,
            AggregateFunction::Sum(_) => DataType::Float,
            AggregateFunction::Avg(_) => DataType::Float,
            AggregateFunction::Max(_) | AggregateFunction::Min(_) => arg_type.clone(),
            AggregateFunction::Collect(_) => DataType::List,
            AggregateFunction::CollectSet(_) => DataType::Set,
            AggregateFunction::Distinct(_) => DataType::Set,
            AggregateFunction::Percentile(_, _) => DataType::Float,
            AggregateFunction::Std(_) => DataType::Float,
            AggregateFunction::BitAnd(_) | AggregateFunction::BitOr(_) => DataType::Int,
            AggregateFunction::GroupConcat(_, _) => DataType::String,
            AggregateFunction::VecSum(_) => DataType::Vector,
            AggregateFunction::VecAvg(_) => DataType::Vector,
        }
    }

    /// Folded binary constant expressions
    fn fold_binary_constant(
        &self,
        op: &crate::core::BinaryOperator,
        left: Option<&Value>,
        right: Option<&Value>,
    ) -> Option<Value> {
        use crate::core::BinaryOperator;
        use crate::core::Value;

        let (left, right) = (left?, right?);

        match op {
            BinaryOperator::Add => match (left, right) {
                (Value::Int(l), Value::Int(r)) => Some(Value::Int(l + r)),
                (Value::Float(l), Value::Float(r)) => Some(Value::Float(l + r)),
                (Value::Int(l), Value::Float(r)) => Some(Value::Float(*l as f32 + r)),
                (Value::Float(l), Value::Int(r)) => Some(Value::Float(l + *r as f32)),
                _ => None,
            },

            BinaryOperator::Subtract => match (left, right) {
                (Value::Int(l), Value::Int(r)) => Some(Value::Int(l - r)),
                (Value::Float(l), Value::Float(r)) => Some(Value::Float(l - r)),
                (Value::Int(l), Value::Float(r)) => Some(Value::Float(*l as f32 - r)),
                (Value::Float(l), Value::Int(r)) => Some(Value::Float(l - *r as f32)),
                _ => None,
            },

            BinaryOperator::Multiply => match (left, right) {
                (Value::Int(l), Value::Int(r)) => Some(Value::Int(l * r)),
                (Value::Float(l), Value::Float(r)) => Some(Value::Float(l * r)),
                (Value::Int(l), Value::Float(r)) => Some(Value::Float(*l as f32 * r)),
                (Value::Float(l), Value::Int(r)) => Some(Value::Float(l * *r as f32)),
                _ => None,
            },

            BinaryOperator::Divide => match (left, right) {
                (Value::Int(l), Value::Int(r)) if *r != 0 => Some(Value::Int(l / r)),
                (Value::Float(l), Value::Float(r)) if *r != 0.0 => Some(Value::Float(l / r)),
                (Value::Int(l), Value::Float(r)) if *r != 0.0 => Some(Value::Float(*l as f32 / r)),
                (Value::Float(l), Value::Int(r)) if *r != 0 => Some(Value::Float(l / *r as f32)),
                _ => None,
            },

            BinaryOperator::And => match (left, right) {
                (Value::Bool(l), Value::Bool(r)) => Some(Value::Bool(*l && *r)),
                _ => None,
            },

            BinaryOperator::Or => match (left, right) {
                (Value::Bool(l), Value::Bool(r)) => Some(Value::Bool(*l || *r)),
                _ => None,
            },

            BinaryOperator::Equal => Some(Value::Bool(left == right)),
            BinaryOperator::NotEqual => Some(Value::Bool(left != right)),

            BinaryOperator::LessThan => match (left, right) {
                (Value::Int(l), Value::Int(r)) => Some(Value::Bool(l < r)),
                (Value::Float(l), Value::Float(r)) => Some(Value::Bool(l < r)),
                (Value::Int(l), Value::Float(r)) => Some(Value::Bool((*l as f32) < *r)),
                (Value::Float(l), Value::Int(r)) => Some(Value::Bool(*l < *r as f32)),
                _ => None,
            },

            BinaryOperator::LessThanOrEqual => match (left, right) {
                (Value::Int(l), Value::Int(r)) => Some(Value::Bool(l <= r)),
                (Value::Float(l), Value::Float(r)) => Some(Value::Bool(l <= r)),
                (Value::Int(l), Value::Float(r)) => Some(Value::Bool((*l as f32) <= *r)),
                (Value::Float(l), Value::Int(r)) => Some(Value::Bool(*l <= *r as f32)),
                _ => None,
            },

            BinaryOperator::GreaterThan => match (left, right) {
                (Value::Int(l), Value::Int(r)) => Some(Value::Bool(l > r)),
                (Value::Float(l), Value::Float(r)) => Some(Value::Bool(l > r)),
                (Value::Int(l), Value::Float(r)) => Some(Value::Bool((*l as f32) > *r)),
                (Value::Float(l), Value::Int(r)) => Some(Value::Bool(*l > *r as f32)),
                _ => None,
            },

            BinaryOperator::GreaterThanOrEqual => match (left, right) {
                (Value::Int(l), Value::Int(r)) => Some(Value::Bool(l >= r)),
                (Value::Float(l), Value::Float(r)) => Some(Value::Bool(l >= r)),
                (Value::Int(l), Value::Float(r)) => Some(Value::Bool((*l as f32) >= *r)),
                (Value::Float(l), Value::Int(r)) => Some(Value::Bool(*l >= *r as f32)),
                _ => None,
            },

            BinaryOperator::StringConcat => match (left, right) {
                (Value::String(l), Value::String(r)) => Some(Value::String(format!("{}{}", l, r))),
                _ => None,
            },

            _ => None,
        }
    }

    /// Fold a constant expression with a value of 1
    fn fold_unary_constant(
        &self,
        op: &crate::core::UnaryOperator,
        operand: Option<&Value>,
    ) -> Option<Value> {
        use crate::core::UnaryOperator;
        use crate::core::Value;

        let operand = operand?;

        match op {
            UnaryOperator::Not => match operand {
                Value::Bool(b) => Some(Value::Bool(!b)),
                _ => None,
            },

            UnaryOperator::Minus => match operand {
                Value::Int(i) => Some(Value::Int(-i)),
                Value::Float(f) => Some(Value::Float(-f)),
                _ => None,
            },

            UnaryOperator::IsNull => Some(Value::Bool(matches!(operand, Value::Null(_)))),
            UnaryOperator::IsNotNull => Some(Value::Bool(!matches!(operand, Value::Null(_)))),

            _ => None,
        }
    }
}

impl Default for ExpressionAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::ExpressionMeta;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_analyze_literal() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::int(42);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, ctx.clone());

        let analyzer = ExpressionAnalyzer::new();
        let result = analyzer.analyze(&ctx_expr, None).expect("analysis failure");

        assert_eq!(result.data_type, DataType::Int);
        assert!(result.is_constant);
        assert_eq!(result.constant_value, Some(Value::Int(42)));
    }

    #[test]
    fn test_analyze_binary_constant_fold() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = Expression::int(10);
        let right = Expression::int(20);
        let expr = Expression::binary(left, crate::core::BinaryOperator::Add, right);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, ctx.clone());

        let analyzer = ExpressionAnalyzer::new();
        let result = analyzer.analyze(&ctx_expr, None).expect("analysis failure");

        assert_eq!(result.data_type, DataType::Int);
        assert!(result.is_constant);
        assert_eq!(result.constant_value, Some(Value::Int(30)));

        // Verify that the data has been stored in the ExpressionContext.
        assert_eq!(ctx_expr.data_type(), Some(DataType::Int));
        assert_eq!(ctx_expr.constant_value(), Some(Value::Int(30)));
    }
}
