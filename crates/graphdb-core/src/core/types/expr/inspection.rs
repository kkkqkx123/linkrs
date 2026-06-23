//! Expression checking methods
//!
//! Provides methods for checking the properties and state of an expression.

use crate::core::types::expr::Expression;
use crate::core::Value;

impl Expression {
    /// Checking if an expression is a constant
    ///
    /// Constant expressions can be determined at compile time and do not require run-time evaluation.
    pub fn is_constant(&self) -> bool {
        match self {
            Expression::Literal(_) => true,
            Expression::List(items) => items.iter().all(|e| e.is_constant()),
            Expression::Map(pairs) => pairs.iter().all(|(_, e)| e.is_constant()),
            Expression::TagProperty { .. } => false,
            Expression::EdgeProperty { .. } => false,
            Expression::LabelTagProperty { .. } => false,
            _ => false,
        }
    }

    /// Check if the expression contains an aggregate function
    ///
    /// Used to identify expressions that need to be evaluated in a GROUP BY context.
    pub fn contains_aggregate(&self) -> bool {
        match self {
            Expression::Aggregate { .. } => true,
            _ => self.children().iter().any(|e| e.contains_aggregate()),
        }
    }

    /// Get the names of all variables in the expression
    ///
    /// Returns a list of de-duplicated variable names.
    pub fn get_variables(&self) -> Vec<String> {
        let mut variables = Vec::new();
        self.collect_variables(&mut variables);
        variables.sort();
        variables.dedup();
        variables
    }

    /// An auxiliary method for recursively collecting variables
    fn collect_variables(&self, variables: &mut Vec<String>) {
        match self {
            Expression::Variable(name) => {
                if !variables.contains(name) {
                    variables.push(name.clone());
                }
            }
            _ => {
                for child in self.children() {
                    child.collect_variables(variables);
                }
            }
        }
    }

    /// Checks if it is a literal expression
    pub fn is_literal(&self) -> bool {
        matches!(self, Expression::Literal(_))
    }

    /// Get the literal value (if it is a literal)
    pub fn as_literal(&self) -> Option<&Value> {
        match self {
            Expression::Literal(v) => Some(v),
            _ => None,
        }
    }

    /// Checking for variable expressions
    pub fn is_variable(&self) -> bool {
        matches!(self, Expression::Variable(_))
    }

    /// Get variable name (if variable)
    pub fn as_variable(&self) -> Option<&str> {
        match self {
            Expression::Variable(name) => Some(name),
            _ => None,
        }
    }

    /// Checking for Aggregate Expressions
    pub fn is_aggregate(&self) -> bool {
        matches!(self, Expression::Aggregate { .. })
    }

    /// Checks if it is an attribute access expression
    pub fn is_property(&self) -> bool {
        matches!(self, Expression::Property { .. })
    }

    /// Check if it is a function call expression
    pub fn is_function(&self) -> bool {
        matches!(self, Expression::Function { .. })
    }

    /// Checking for binary arithmetic expressions
    pub fn is_binary(&self) -> bool {
        matches!(self, Expression::Binary { .. })
    }

    /// Checking for unary arithmetic expressions
    pub fn is_unary(&self) -> bool {
        matches!(self, Expression::Unary { .. })
    }

    /// Checks if it is a list expression
    pub fn is_list(&self) -> bool {
        matches!(self, Expression::List(_))
    }

    /// Checks if it is a mapping expression
    pub fn is_map(&self) -> bool {
        matches!(self, Expression::Map(_))
    }

    /// Checks if it is a path expression
    pub fn is_path(&self) -> bool {
        matches!(self, Expression::Path(_))
    }

    /// Checks if it's a tag expression
    pub fn is_label(&self) -> bool {
        matches!(self, Expression::Label(_))
    }

    /// Checks if it is a parameter expression
    pub fn is_parameter(&self) -> bool {
        matches!(self, Expression::Parameter(_))
    }

    /// Get the parameter name (if it is a parameter)
    pub fn as_parameter(&self) -> Option<&str> {
        match self {
            Expression::Parameter(name) => Some(name),
            _ => None,
        }
    }

    /// Checking for Conditional Expressions
    pub fn is_case(&self) -> bool {
        matches!(self, Expression::Case { .. })
    }

    /// Checking for type conversion expressions
    pub fn is_cast(&self) -> bool {
        matches!(self, Expression::TypeCast { .. })
    }

    /// Checking for subscript access expressions
    pub fn is_subscript(&self) -> bool {
        matches!(self, Expression::Subscript { .. })
    }

    /// Checks if it is a range expression
    pub fn is_range(&self) -> bool {
        matches!(self, Expression::Range { .. })
    }

    /// Get the function name (if it's a function call)
    pub fn function_name(&self) -> Option<&str> {
        match self {
            Expression::Function { name, .. } => Some(name),
            _ => None,
        }
    }

    /// Get the name of the aggregate function (if it is an aggregate expression)
    pub fn aggregate_function_name(&self) -> Option<&str> {
        match self {
            Expression::Aggregate { func, .. } => Some(func.name()),
            _ => None,
        }
    }

    /// Checks if it is a path building expression
    pub fn is_path_build(&self) -> bool {
        matches!(self, Expression::PathBuild(_))
    }

    /// Check whether it is a type conversion expression
    pub fn is_type_cast(&self) -> bool {
        matches!(self, Expression::TypeCast { .. })
    }

    /// Checking for List Derivation Expressions
    pub fn is_list_comprehension(&self) -> bool {
        matches!(self, Expression::ListComprehension { .. })
    }

    /// Checks for a Reduce expression
    pub fn is_reduce(&self) -> bool {
        matches!(self, Expression::Reduce { .. })
    }

    /// Gets the function name (if it is a function call)
    pub fn as_function_name(&self) -> Option<String> {
        match self {
            Expression::Function { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// Get the attribute name (in case of attribute access)
    pub fn as_property_name(&self) -> Option<String> {
        match self {
            Expression::Property { property, .. } => Some(property.clone()),
            _ => None,
        }
    }

    /// Get the tag name (if it's a tag expression)
    pub fn as_label_name(&self) -> Option<String> {
        match self {
            Expression::Label(name) => Some(name.clone()),
            _ => None,
        }
    }

    /// Get the parameter name (if it is a parameter expression)
    pub fn as_parameter_name(&self) -> Option<String> {
        match self {
            Expression::Parameter(name) => Some(name.clone()),
            _ => None,
        }
    }
}
