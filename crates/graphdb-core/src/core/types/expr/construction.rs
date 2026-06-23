//! Expression construction methods
//!
//! Provide methods for creating various types of expressions.

use crate::core::types::expr::Expression;
use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use crate::core::types::DataType;
use crate::core::{NullType, Value};

impl Expression {
    /// Creating literal expressions
    pub fn literal(value: impl Into<Value>) -> Self {
        Expression::Literal(value.into())
    }

    /// Create a variable expression.
    pub fn variable(name: impl Into<String>) -> Self {
        Expression::Variable(name.into())
    }

    /// Create attribute access expressions
    pub fn property(object: Expression, property: impl Into<String>) -> Self {
        Expression::Property {
            object: Box::new(object),
            property: property.into(),
        }
    }

    /// Create a binary operation expression
    pub fn binary(left: Expression, op: BinaryOperator, right: Expression) -> Self {
        Expression::Binary {
            left: Box::new(left),
            op,
            right: Box::new(right),
        }
    }

    /// Create a unary operation expression.
    pub fn unary(op: UnaryOperator, operand: Expression) -> Self {
        Expression::Unary {
            op,
            operand: Box::new(operand),
        }
    }

    /// Create a function call expression.
    pub fn function(name: impl Into<String>, args: Vec<Expression>) -> Self {
        Expression::Function {
            name: name.into(),
            args,
        }
    }

    /// Create an aggregate function expression.
    pub fn aggregate(func: AggregateFunction, arg: Expression, distinct: bool) -> Self {
        Expression::Aggregate {
            func,
            arg: Box::new(arg),
            distinct,
        }
    }

    /// Create a list expression.
    pub fn list(items: Vec<Expression>) -> Self {
        Expression::List(items)
    }

    /// Create a mapping expression
    pub fn map(pairs: Vec<(impl Into<String>, Expression)>) -> Self {
        Expression::Map(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Create a conditional expression
    pub fn case(
        test_expr: Option<Expression>,
        conditions: Vec<(Expression, Expression)>,
        default: Option<Expression>,
    ) -> Self {
        Expression::Case {
            test_expr: test_expr.map(Box::new),
            conditions,
            default: default.map(Box::new),
        }
    }

    /// Create a type conversion expression.
    pub fn cast(expression: Expression, target_type: DataType) -> Self {
        Expression::TypeCast {
            expression: Box::new(expression),
            target_type,
        }
    }

    /// Create an subscript access expression.
    pub fn subscript(collection: Expression, index: Expression) -> Self {
        Expression::Subscript {
            collection: Box::new(collection),
            index: Box::new(index),
        }
    }

    /// Create a range expression
    pub fn range(
        collection: Expression,
        start: Option<Expression>,
        end: Option<Expression>,
    ) -> Self {
        Expression::Range {
            collection: Box::new(collection),
            start: start.map(Box::new),
            end: end.map(Box::new),
        }
    }

    /// Create a path expression
    pub fn path(items: Vec<Expression>) -> Self {
        Expression::Path(items)
    }

    /// Create a tag expression
    pub fn label(name: impl Into<String>) -> Self {
        Expression::Label(name.into())
    }

    /// Create a list comprehension expression.
    pub fn list_comprehension(
        variable: impl Into<String>,
        source: Expression,
        filter: Option<Expression>,
        map: Option<Expression>,
    ) -> Self {
        Expression::ListComprehension {
            variable: variable.into(),
            source: Box::new(source),
            filter: filter.map(Box::new),
            map: map.map(Box::new),
        }
    }

    /// Create dynamic access expressions for tag attributes
    pub fn label_tag_property(tag: Expression, property: impl Into<String>) -> Self {
        Expression::LabelTagProperty {
            tag: Box::new(tag),
            property: property.into(),
        }
    }

    /// Create tag attribute access expressions
    pub fn tag_property(tag_name: impl Into<String>, property: impl Into<String>) -> Self {
        Expression::TagProperty {
            tag_name: tag_name.into(),
            property: property.into(),
        }
    }

    /// Create an edge attribute access expression.
    pub fn edge_property(edge_name: impl Into<String>, property: impl Into<String>) -> Self {
        Expression::EdgeProperty {
            edge_name: edge_name.into(),
            property: property.into(),
        }
    }

    /// Create a predicate expression.
    pub fn predicate(func: impl Into<String>, args: Vec<Expression>) -> Self {
        Expression::Predicate {
            func: func.into(),
            args,
        }
    }

    /// Create a Reduce expression.
    pub fn reduce(
        accumulator: impl Into<String>,
        initial: Expression,
        variable: impl Into<String>,
        source: Expression,
        mapping: Expression,
    ) -> Self {
        Expression::Reduce {
            accumulator: accumulator.into(),
            initial: Box::new(initial),
            variable: variable.into(),
            source: Box::new(source),
            mapping: Box::new(mapping),
        }
    }

    /// Create a path construction expression
    pub fn path_build(items: Vec<Expression>) -> Self {
        Expression::PathBuild(items)
    }

    /// Create a parameter expression
    pub fn parameter(name: impl Into<String>) -> Self {
        Expression::Parameter(name.into())
    }

    /// Create a vector literal expression
    pub fn vector(data: Vec<f32>) -> Self {
        Expression::Vector(data)
    }

    /// Creating a boolean literal
    pub fn bool(value: bool) -> Self {
        Expression::Literal(Value::Bool(value))
    }

    /// Creating integer literal values (i32)
    pub fn int(value: i32) -> Self {
        Expression::Literal(Value::Int(value))
    }

    /// Creating bigint literal values (i64)
    pub fn bigint(value: i64) -> Self {
        Expression::Literal(Value::BigInt(value))
    }

    /// Creating a floating-point numeric literal (f32)
    pub fn float(value: f32) -> Self {
        Expression::Literal(Value::Float(value))
    }

    /// Creating a double-precision numeric literal (f64)
    pub fn double(value: f64) -> Self {
        Expression::Literal(Value::Double(value))
    }

    /// Creating a string literal
    pub fn string(value: impl Into<String>) -> Self {
        Expression::Literal(Value::String(value.into()))
    }

    /// Create an empty literal value.
    pub fn null() -> Self {
        Expression::Literal(Value::Null(NullType::Null))
    }

    /// Create a modulus expression
    pub fn modulo(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::Modulo, right)
    }

    /// Creating an expression that equals a certain value…
    pub fn eq(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::Equal, right)
    }

    /// Creating something does not equal expressing it in words.
    pub fn ne(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::NotEqual, right)
    }

    /// Create an expression that is smaller than the given one.
    pub fn lt(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::LessThan, right)
    }

    /// Create an expression that is less than or equal to…
    pub fn le(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::LessThanOrEqual, right)
    }

    /// Create something that is greater than the expression…
    pub fn gt(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::GreaterThan, right)
    }

    /// Create an expression that is greater than or equal to…
    pub fn ge(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::GreaterThanOrEqual, right)
    }

    /// Creating logic and expressions
    pub fn and(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::And, right)
    }

    /// Create a logical statement or expression.
    pub fn or(left: Expression, right: Expression) -> Self {
        Self::binary(left, BinaryOperator::Or, right)
    }

    /// Creating an IS NULL expression
    pub fn is_null(operand: Expression) -> Self {
        Self::unary(UnaryOperator::IsNull, operand)
    }

    /// Creating an IS NOT NULL expression
    pub fn is_not_null(operand: Expression) -> Self {
        Self::unary(UnaryOperator::IsNotNull, operand)
    }
}

impl std::ops::Add for Expression {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::binary(self, BinaryOperator::Add, rhs)
    }
}

impl std::ops::Sub for Expression {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::binary(self, BinaryOperator::Subtract, rhs)
    }
}

impl std::ops::Mul for Expression {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self::binary(self, BinaryOperator::Multiply, rhs)
    }
}

impl std::ops::Div for Expression {
    type Output = Self;

    fn div(self, rhs: Self) -> Self::Output {
        Self::binary(self, BinaryOperator::Divide, rhs)
    }
}

impl std::ops::Not for Expression {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self::unary(UnaryOperator::Not, self)
    }
}

impl std::ops::Neg for Expression {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::unary(UnaryOperator::Minus, self)
    }
}
