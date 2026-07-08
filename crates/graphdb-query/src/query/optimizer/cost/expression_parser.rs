//! Expression parser
//!
//! Provide an expression parsing function for:
//! Estimate the size of the list of Unwind nodes.
//! Estimate the number of iterations of the Loop node.
//! Analyze various expression patterns
//! Expression constant folding optimization

use crate::core::types::{BinaryOperator, Expression, UnaryOperator};
use crate::core::value::Value;
use crate::query::optimizer::cost::config::CostModelConfig;

/// Expression parser
///
/// Used to parse useful information from expression strings, such as the size of a list, the number of iterations, etc.
#[derive(Debug, Clone)]
pub struct ExpressionParser {
    /// Configuration (for internal use)
    config: CostModelConfig,
}

impl ExpressionParser {
    /// Obtain the configuration.
    pub fn config(&self) -> &CostModelConfig {
        &self.config
    }
}

impl ExpressionParser {
    /// Create a new expression parser.
    pub fn new(config: CostModelConfig) -> Self {
        Self { config }
    }

    /// Try to parse the list size from the expression string.
    ///
    /// The following modes are supported:
    /// - Array literals: [a, b, c] -> 3
    /// - range 函数：range(1, 10) -> 9, range(1, 10, 2) -> 5
    /// - Range expressions: 1..10 -> 9, 0..=5 -> 6
    /// - 集合函数：keys(map), values(map), nodes(path), relationships(path)
    /// - 字符串分割：split(str, ",")（估算）
    /// - 集合操作：collect(set)（估算）
    pub fn parse_list_size(&self, expr: &str) -> Option<f64> {
        let expr = expr.trim();

        // Attempt to parse array literals [a, b, c]
        if expr.starts_with('[') && expr.ends_with(']') {
            return self.parse_array_literal(expr);
        }

        // 尝试解析 range(start, end) 或 range(start, end, step)
        if expr.starts_with("range(") && expr.ends_with(')') {
            return self.parse_range_function(expr);
        }

        // Try to parse the range expression: 1..10 or 0... =5
        if expr.contains("..") {
            return self.parse_range_expression(expr);
        }

        // 尝试解析集合函数：keys(), values(), nodes(), relationships()
        if let Some(size) = self.parse_collection_function(expr) {
            return Some(size);
        }

        // Trying to parse a string splitting function
        if let Some(size) = self.parse_split_function(expr) {
            return Some(size);
        }

        // Try to parse the collect function (usually used for aggregation)
        let expr_lower = expr.to_lowercase();
        if expr_lower.starts_with("collect(") || expr_lower.contains(".collect()") {
            // The size of the result of the collect function depends on the input data, using a conservative estimate
            return Some(self.config.default_unwind_list_size * 2.0);
        }

        None
    }

    // ==================== 常量折叠优化 ====================

    /// Try to fold the constants within the expression.
    ///
    /// Perform a recursive traversal of the expression, and replace all constant expressions that can be evaluated with their literal values.
    /// For example: 1 + 2 -> 3, “hello” + “world” -> “helloworld”.
    ///
    /// # Parameters
    /// `expr`: The input expression
    ///
    /// # Return value
    /// The folded expression
    pub fn fold_constants(&self, expr: &Expression) -> Expression {
        match expr {
            Expression::Binary { left, op, right } => {
                let folded_left = self.fold_constants(left);
                let folded_right = self.fold_constants(right);

                // If both sides are constants, calculate the result directly
                if let (Expression::Literal(l), Expression::Literal(r)) =
                    (&folded_left, &folded_right)
                {
                    if let Some(result) = self.evaluate_binary_op(op, l, r) {
                        return Expression::Literal(result);
                    }
                }

                Expression::Binary {
                    left: Box::new(folded_left),
                    op: *op,
                    right: Box::new(folded_right),
                }
            }
            Expression::Unary { op, operand } => {
                let folded_operand = self.fold_constants(operand);

                // If the operand is a constant, compute the result directly
                if let Expression::Literal(v) = &folded_operand {
                    if let Some(result) = self.evaluate_unary_op(op, v) {
                        return Expression::Literal(result);
                    }
                }

                Expression::Unary {
                    op: *op,
                    operand: Box::new(folded_operand),
                }
            }
            Expression::Function { name, args } => {
                let folded_args: Vec<Expression> =
                    args.iter().map(|arg| self.fold_constants(arg)).collect();

                // If all arguments are constants, try to compute the function
                if folded_args
                    .iter()
                    .all(|arg| matches!(arg, Expression::Literal(_)))
                {
                    let arg_values: Vec<&Value> = folded_args
                        .iter()
                        .filter_map(|arg| match arg {
                            Expression::Literal(v) => Some(v),
                            _ => None,
                        })
                        .collect();

                    if let Some(result) = self.evaluate_function(name, &arg_values) {
                        return Expression::Literal(result);
                    }
                }

                Expression::Function {
                    name: name.clone(),
                    args: folded_args,
                }
            }
            Expression::List(items) => {
                let folded_items: Vec<Expression> =
                    items.iter().map(|item| self.fold_constants(item)).collect();
                Expression::List(folded_items)
            }
            Expression::Map(entries) => {
                let folded_entries: Vec<(String, Expression)> = entries
                    .iter()
                    .map(|(k, v)| (k.clone(), self.fold_constants(v)))
                    .collect();
                Expression::Map(folded_entries)
            }
            _ => expr.clone(),
        }
    }

    /// Evaluating binary operations
    fn evaluate_binary_op(
        &self,
        op: &BinaryOperator,
        left: &Value,
        right: &Value,
    ) -> Option<Value> {
        match op {
            BinaryOperator::Add => self.add_values(left, right),
            BinaryOperator::Subtract => self.subtract_values(left, right),
            BinaryOperator::Multiply => self.multiply_values(left, right),
            BinaryOperator::Divide => self.divide_values(left, right),
            BinaryOperator::Modulo => self.modulo_values(left, right),
            BinaryOperator::Equal => Some(Value::Bool(
                self.compare_values(left, right) == Some(std::cmp::Ordering::Equal),
            )),
            BinaryOperator::NotEqual => Some(Value::Bool(
                self.compare_values(left, right) != Some(std::cmp::Ordering::Equal),
            )),
            BinaryOperator::LessThan => Some(Value::Bool(
                self.compare_values(left, right) == Some(std::cmp::Ordering::Less),
            )),
            BinaryOperator::GreaterThan => Some(Value::Bool(
                self.compare_values(left, right) == Some(std::cmp::Ordering::Greater),
            )),
            BinaryOperator::LessThanOrEqual => {
                let cmp = self.compare_values(left, right);
                Some(Value::Bool(
                    cmp == Some(std::cmp::Ordering::Less) || cmp == Some(std::cmp::Ordering::Equal),
                ))
            }
            BinaryOperator::GreaterThanOrEqual => {
                let cmp = self.compare_values(left, right);
                Some(Value::Bool(
                    cmp == Some(std::cmp::Ordering::Greater)
                        || cmp == Some(std::cmp::Ordering::Equal),
                ))
            }
            BinaryOperator::And => self.logical_and(left, right),
            BinaryOperator::Or => self.logical_or(left, right),
            BinaryOperator::StringConcat => self.concat_values(left, right),
            _ => None,
        }
    }

    /// Evaluating a unary operation
    fn evaluate_unary_op(&self, op: &UnaryOperator, operand: &Value) -> Option<Value> {
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
            UnaryOperator::IsNull => Some(Value::Bool(operand.is_null())),
            UnaryOperator::IsNotNull => Some(Value::Bool(!operand.is_null())),
            _ => None,
        }
    }

    /// Evaluation function
    fn evaluate_function(&self, name: &str, args: &[&Value]) -> Option<Value> {
        let name_lower = name.to_lowercase();

        match name_lower.as_str() {
            "abs" if args.len() == 1 => match args[0] {
                Value::SmallInt(i) => Some(Value::SmallInt(i.abs())),
                Value::Int(i) => Some(Value::Int(i.abs())),
                Value::BigInt(i) => Some(Value::BigInt(i.abs())),
                Value::Float(f) => Some(Value::Float(f.abs())),
                Value::Double(f) => Some(Value::Double(f.abs())),
                _ => None,
            },
            "length" | "size" if args.len() == 1 => match args[0] {
                Value::String(s) => Some(Value::BigInt(s.len() as i64)),
                Value::List(list) => Some(Value::BigInt(list.len() as i64)),
                _ => None,
            },
            "upper" | "toupper" if args.len() == 1 => match args[0] {
                Value::String(s) => Some(Value::String(s.to_uppercase())),
                _ => None,
            },
            "lower" | "tolower" if args.len() == 1 => match args[0] {
                Value::String(s) => Some(Value::String(s.to_lowercase())),
                _ => None,
            },
            "substring" | "substr" if args.len() >= 2 => match (args[0], args.get(1).copied()) {
                (Value::String(s), Some(Value::BigInt(start))) => {
                    let start_idx = if *start >= 0 { *start as usize } else { 0 };
                    let end_idx = if args.len() >= 3 {
                        match args[2] {
                            Value::BigInt(len) => start_idx + (*len as usize),
                            _ => s.len(),
                        }
                    } else {
                        s.len()
                    };
                    Some(Value::String(
                        s.chars()
                            .skip(start_idx)
                            .take(end_idx - start_idx)
                            .collect(),
                    ))
                }
                _ => None,
            },
            "trim" if args.len() == 1 => match args[0] {
                Value::String(s) => Some(Value::String(s.trim().to_string())),
                _ => None,
            },
            _ => None,
        }
    }

    // ==================== 值操作辅助方法 ====================

    fn add_values(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::SmallInt(l), Value::SmallInt(r)) => Some(Value::SmallInt(l + r)),
            (Value::Int(l), Value::Int(r)) => Some(Value::Int(l + r)),
            (Value::BigInt(l), Value::BigInt(r)) => Some(Value::BigInt(l + r)),
            (Value::Float(l), Value::Float(r)) => Some(Value::Float(l + r)),
            (Value::Double(l), Value::Double(r)) => Some(Value::Double(l + r)),
            (Value::String(l), Value::String(r)) => Some(Value::String(format!("{}{}", l, r))),
            _ => None,
        }
    }

    fn subtract_values(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::SmallInt(l), Value::SmallInt(r)) => Some(Value::SmallInt(l - r)),
            (Value::Int(l), Value::Int(r)) => Some(Value::Int(l - r)),
            (Value::BigInt(l), Value::BigInt(r)) => Some(Value::BigInt(l - r)),
            (Value::Float(l), Value::Float(r)) => Some(Value::Float(l - r)),
            (Value::Double(l), Value::Double(r)) => Some(Value::Double(l - r)),
            _ => None,
        }
    }

    fn multiply_values(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::SmallInt(l), Value::SmallInt(r)) => Some(Value::SmallInt(l * r)),
            (Value::Int(l), Value::Int(r)) => Some(Value::Int(l * r)),
            (Value::BigInt(l), Value::BigInt(r)) => Some(Value::BigInt(l * r)),
            (Value::Float(l), Value::Float(r)) => Some(Value::Float(l * r)),
            (Value::Double(l), Value::Double(r)) => Some(Value::Double(l * r)),
            _ => None,
        }
    }

    fn divide_values(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::SmallInt(l), Value::SmallInt(r)) if *r != 0 => Some(Value::SmallInt(l / r)),
            (Value::Int(l), Value::Int(r)) if *r != 0 => Some(Value::Int(l / r)),
            (Value::BigInt(l), Value::BigInt(r)) if *r != 0 => Some(Value::BigInt(l / r)),
            (Value::Float(l), Value::Float(r)) if *r != 0.0 => Some(Value::Float(l / r)),
            (Value::Double(l), Value::Double(r)) if *r != 0.0 => Some(Value::Double(l / r)),
            _ => None,
        }
    }

    fn modulo_values(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::SmallInt(l), Value::SmallInt(r)) if *r != 0 => Some(Value::SmallInt(l % r)),
            (Value::Int(l), Value::Int(r)) if *r != 0 => Some(Value::Int(l % r)),
            (Value::BigInt(l), Value::BigInt(r)) if *r != 0 => Some(Value::BigInt(l % r)),
            _ => None,
        }
    }

    fn compare_values(&self, left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
        match (left, right) {
            (Value::SmallInt(l), Value::SmallInt(r)) => Some(l.cmp(r)),
            (Value::Int(l), Value::Int(r)) => Some(l.cmp(r)),
            (Value::BigInt(l), Value::BigInt(r)) => Some(l.cmp(r)),
            (Value::Float(l), Value::Float(r)) => l.partial_cmp(r),
            (Value::Double(l), Value::Double(r)) => l.partial_cmp(r),
            (Value::String(l), Value::String(r)) => Some(l.cmp(r)),
            (Value::Bool(l), Value::Bool(r)) => Some(l.cmp(r)),
            _ => None,
        }
    }

    fn logical_and(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::Bool(l), Value::Bool(r)) => Some(Value::Bool(*l && *r)),
            _ => None,
        }
    }

    fn logical_or(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::Bool(l), Value::Bool(r)) => Some(Value::Bool(*l || *r)),
            _ => None,
        }
    }

    fn concat_values(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::String(l), Value::String(r)) => Some(Value::String(format!("{}{}", l, r))),
            _ => None,
        }
    }

    /// Estimating the number of iterations of the Loop node
    ///
    /// Try to parse the number of iterations from the conditional string. The following patterns are supported:
    /// - Digital direct value: "10" → 10
    /// - 范围表达式："1..10" 或 "range(1, 10)" -> 9
    /// - Comparison expressions: "i < 10", "i <= 10" → 10
    /// - Set size: "Items" – Use the set size to make an estimate.
    ///
    /// If the content cannot be parsed, use the default values configured in the system.
    pub fn parse_loop_iterations(&self, condition: &str) -> Option<u32> {
        let condition = condition.trim();

        // Trying to parse numbers directly
        if let Ok(num) = condition.parse::<u32>() {
            return Some(num.max(1));
        }

        // Try to parse the range expression: 1..10 or 1... =10
        if condition.contains("..") {
            return self.parse_range_expression_u32(condition);
        }

        // 尝试解析 range(start, end) 或 range(start, end, step)
        if condition.starts_with("range(") && condition.ends_with(")") {
            return self.parse_range_function(condition).map(|v| v as u32);
        }

        // Try to parse comparison expressions: i < 10, i <= 10, count > 5, etc.
        if let Some(iterations) = self.parse_comparison_expression(condition) {
            return Some(iterations as u32);
        }

        // Try to parse list/set size: [a,b,c] or {a,b,c}
        if condition.starts_with('[') && condition.ends_with(']') {
            return Some(self.parse_collection_size(condition));
        }

        None
    }

    /// Analyzing array literals
    fn parse_array_literal(&self, expr: &str) -> Option<f64> {
        let inner = &expr[1..expr.len() - 1];
        if inner.trim().is_empty() {
            return Some(0.0);
        }
        // Working with nested arrays and complex expressions
        // Handling nested arrays and complex expressions
        let count = self.count_top_level_commas(inner) as f64;
        Some(count)
    }

    /// Count the number of commas at the top level (used for processing nested structures)
    fn count_top_level_commas(&self, s: &str) -> usize {
        let mut count = 0;
        let mut depth = 0;
        let chars = s.chars().peekable();

        for c in chars {
            match c {
                '[' | '(' | '{' => depth += 1,
                ']' | ')' | '}' => depth -= 1,
                ',' if depth == 0 => count += 1,
                _ => {}
            }
        }

        count + 1 // Number of commas + 1 = Number of elements
    }

    /// Analyzing the range function
    fn parse_range_function(&self, expr: &str) -> Option<f64> {
        let args_str = &expr[6..expr.len() - 1];
        let args: Vec<&str> = args_str.split(',').map(|s| s.trim()).collect();

        if args.len() >= 2 {
            let start: i64 = args[0].parse().ok()?;
            let end: i64 = args[1].parse().ok()?;
            let step: i64 = if args.len() >= 3 {
                args[2].parse().ok()?
            } else {
                1
            };

            if step != 0 {
                let count = ((end - start) / step).abs() as f64;
                return Some(count.max(0.0));
            }
        }
        None
    }

    /// Analyzing the range expression
    fn parse_range_expression(&self, expr: &str) -> Option<f64> {
        if let Some(pos) = expr.find("..") {
            let start_str = expr[..pos].trim();
            let end_part = &expr[pos + 2..];

            let (end_str, inclusive) = if let Some(stripped) = end_part.strip_prefix('=') {
                (stripped, true)
            } else {
                (end_part, false)
            };
            if let (Ok(start), Ok(end)) = (start_str.parse::<i64>(), end_str.trim().parse::<i64>())
            {
                if end > start {
                    let count = if inclusive {
                        end - start + 1
                    } else {
                        end - start
                    };
                    return Some(count.max(0) as f64);
                }
            }
        }
        None
    }

    /// Parse the range expression (return an u32 value)
    fn parse_range_expression_u32(&self, expr: &str) -> Option<u32> {
        // Processing 1...10 format (without end)
        if let Some(pos) = expr.find("..") {
            let start_str = expr[..pos].trim();
            let end_part = &expr[pos + 2..];

            // Check for inclusion of the equal sign (1... =10 indicates end of inclusion)
            let (end_str, inclusive) = if let Some(stripped) = end_part.strip_prefix('=') {
                (stripped, true)
            } else {
                (end_part, false)
            };

            if let (Ok(start), Ok(end)) = (start_str.parse::<i64>(), end_str.trim().parse::<i64>())
            {
                if end > start {
                    let count = if inclusive {
                        end - start + 1
                    } else {
                        end - start
                    };
                    return Some(count.max(1) as u32);
                }
            }
        }
        None
    }

    /// Analyzing comparative expressions (such as "i < 10", "count <= 100")
    fn parse_comparison_expression(&self, expr: &str) -> Option<f64> {
        // Matching patterns: var < num, var <= num, var > num, var >= num
        // 匹配模式：var < num, var <= num, var > num, var >= num
        let operators = [("<", 0u32), ("<=", 0u32), (">", 0u32), (">=", 0u32)];

        for (op, _) in &operators {
            if let Some(pos) = expr.find(op) {
                let right_side = &expr[pos + op.len()..];
                if let Ok(num) = right_side.trim().parse::<i64>() {
                    // For the < operator, the actual number of iterations is num (if num > 0)
                    let iterations = if num > 0 { num as f64 } else { 1.0 };
                    // Unable to determine starting value, use conservative estimate
                    let iterations = iterations + 10.0;
                    return Some(iterations.max(1.0));
                }
            }
        }
        None
    }

    /// Analyzing set functions (keys, values, nodes, relationships)
    fn parse_collection_function(&self, expr: &str) -> Option<f64> {
        let expr_lower = expr.to_lowercase();

        // keys(map) 或 map.keys() - 返回 map 的键列表
        if expr_lower.contains(".keys()") {
            // Unable to determine map size, use default estimate
            return Some(self.config.default_unwind_list_size);
        }

        // values(map) 或 map.values()
        if expr_lower.starts_with("values(") || expr_lower.contains(".values()") {
            return Some(self.config.default_unwind_list_size);
        }

        // nodes(path) - 返回路径中的节点列表
        if expr_lower.contains(".nodes()") {
            // Path length unknown, use default estimate
            return Some(self.config.default_unwind_list_size);
        }

        // relationships(path) 或 rels(path) - 返回路径中的关系列表
        if expr_lower.starts_with("relationships(") || expr_lower.starts_with("rels(") {
            return Some(self.config.default_unwind_list_size - 1.0); // The number of relations is usually 1 less than the number of nodes
        }

        // labels(node) - 返回标签列表（通常很小）
        if expr_lower.starts_with("labels(") {
            return Some(1.0); // Usually a node has only 1-2 tags
        }

        None
    }

    /// Analysis of the string splitting function
    fn parse_split_function(&self, expr: &str) -> Option<f64> {
        let expr_lower = expr.to_lowercase();

        // split(string, delimiter) 或 string.split(delimiter)
        if expr_lower.contains("split(") || expr_lower.contains(".split(") {
            // Assume that the average length of each element is 5 characters.串长度估算
            // Assuming an average length of 5 characters per element
            return Some(self.config.default_unwind_list_size);
        }

        None
    }

    /// Resolve set size (e.g. "[a, b, c]")
    fn parse_collection_size(&self, expr: &str) -> u32 {
        let inner = &expr[1..expr.len() - 1];
        if inner.trim().is_empty() {
            return 0;
        }
        // Perform a simple calculation: count the number of commas and then add 1.
        // Simple calculation of the number of commas + 1
        let count = inner.split(',').count() as u32;
        count.max(1)
    }
}

impl Default for ExpressionParser {
    fn default() -> Self {
        Self::new(CostModelConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_array_literal() {
        let parser = ExpressionParser::default();
        assert_eq!(parser.parse_list_size("[1, 2, 3]"), Some(3.0));
        assert_eq!(parser.parse_list_size("[]"), Some(0.0));
        assert_eq!(parser.parse_list_size("[a]"), Some(1.0));
    }

    #[test]
    fn test_parse_range_function() {
        let parser = ExpressionParser::default();
        assert_eq!(parser.parse_list_size("range(1, 10)"), Some(9.0));
        assert_eq!(parser.parse_list_size("range(1, 10, 2)"), Some(4.0));
    }

    #[test]
    fn test_parse_range_expression() {
        let parser = ExpressionParser::default();
        assert_eq!(parser.parse_list_size("1..10"), Some(9.0));
        assert_eq!(parser.parse_list_size("0..=5"), Some(6.0));
    }

    #[test]
    fn test_parse_loop_iterations_number() {
        let parser = ExpressionParser::default();
        assert_eq!(parser.parse_loop_iterations("10"), Some(10)); // At least once
        assert_eq!(parser.parse_loop_iterations("0"), Some(1)); // At least 1
    }

    #[test]
    fn test_parse_loop_iterations_comparison() {
        let parser = ExpressionParser::default();
        assert_eq!(parser.parse_loop_iterations("i < 10"), Some(20));
        assert_eq!(parser.parse_loop_iterations("i <= 5"), Some(15));
    }
}
