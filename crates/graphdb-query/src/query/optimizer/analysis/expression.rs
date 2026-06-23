//! Expression Analysis Module
//!
//! Provides a feature for analyzing the properties of expressions, including:
//! Deterministic check (whether non-deterministic functions are contained)
//! Complexity score
//! Attribute/variable/function extraction

use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::expr::visitor_collectors::{
    FunctionCollector, PropertyCollector, VariableCollector,
};
use crate::core::types::ContextualExpression;
use crate::core::Expression;

/// Expression analysis results
#[derive(Debug, Clone, Default)]
pub struct ExpressionAnalysis {
    /// Is it deterministic? (excluding non-deterministic functions such as rand() and now())
    pub is_deterministic: bool,
    /// Complexity score (0-100)
    pub complexity_score: u32,
    /// List of attributes for the citation
    pub referenced_properties: Vec<String>,
    /// List of referenced variables
    pub referenced_variables: Vec<String>,
    /// List of called functions
    pub called_functions: Vec<String>,
    /// Does it contain aggregate functions?
    pub contains_aggregate: bool,
    /// Does it contain subqueries?
    pub contains_subquery: bool,
    /// Number of nodes
    pub node_count: u32,
}

impl ExpressionAnalysis {
    /// Create an empty analysis result.
    pub fn new() -> Self {
        Self {
            is_deterministic: true, // The default assumption is one of certainty.
            ..Default::default()
        }
    }
}

/// Expression Analysis Pattern
///
/// Predefined analysis modes simplify the configuration process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisMode {
    /// Complete analysis (default)
    Full,
    /// Check only for certainty.
    DeterministicOnly,
    /// Extract only the attribute references:
    PropertyExtractor,
    /// Extract only the variable references.
    VariableExtractor,
}

/// Expression Analysis Options
#[derive(Debug, Clone)]
pub struct AnalysisOptions {
    /// Analyzing certainty
    pub check_deterministic: bool,
    /// Analyzing complexity
    pub check_complexity: bool,
    /// Extract attribute references.
    pub extract_properties: bool,
    /// Extract variable references
    pub extract_variables: bool,
    /// Statistical function calls
    pub count_functions: bool,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            check_deterministic: true,
            check_complexity: true,
            extract_properties: true,
            extract_variables: true,
            count_functions: true,
        }
    }
}

impl AnalysisOptions {
    /// Create options from the analysis mode.
    fn from_mode(mode: AnalysisMode) -> Self {
        match mode {
            AnalysisMode::Full => AnalysisOptions {
                check_deterministic: true,
                check_complexity: true,
                extract_properties: true,
                extract_variables: true,
                count_functions: true,
            },
            AnalysisMode::DeterministicOnly => AnalysisOptions {
                check_deterministic: true,
                check_complexity: false,
                extract_properties: false,
                extract_variables: false,
                count_functions: false,
            },
            AnalysisMode::PropertyExtractor => AnalysisOptions {
                check_deterministic: false,
                check_complexity: false,
                extract_properties: true,
                extract_variables: false,
                count_functions: false,
            },
            AnalysisMode::VariableExtractor => AnalysisOptions {
                check_deterministic: false,
                check_complexity: false,
                extract_properties: false,
                extract_variables: true,
                count_functions: false,
            },
        }
    }
}

/// Non-deterministic function checking
///
/// Improve performance by using compile-time static matching instead of runtime HashMaps.
/// 非确定性函数每次调用可能返回不同结果（如rand()、now()等）。
pub struct NondeterministicChecker;

impl NondeterministicChecker {
    /// Check whether the function is non-deterministic.
    ///
    /// Using the `match` method for compilation optimization is more efficient than using the `HashMap` for lookups.
    pub fn is_nondeterministic(func_name: &str) -> bool {
        match func_name {
            // Time-related functions
            "now" | "current_time" | "current_date" | "current_timestamp" | "localtime"
            | "localtimestamp" => true,

            // Random number function
            "rand" | "random" | "uuid" => true,

            // Window functions (the result depends on the row position)
            "row_number" | "rank" | "dense_rank" | "percent_rank" | "cume_dist" => true,

            // Other non-deterministic functions
            "last_insert_id" | "connection_id" | "current_user" | "session_user" => true,

            // Deterministic function
            _ => false,
        }
    }
}

/// Expression Analyzer
///
/// Analyze various characteristics of the expression, with the option to perform analysis on demand (by configuring using predefined modes).
#[derive(Debug, Clone)]
pub struct ExpressionAnalyzer {
    /// Analyze the options
    options: AnalysisOptions,
}

impl ExpressionAnalyzer {
    /// Create a default expression analyzer (full analysis mode).
    pub fn new() -> Self {
        Self {
            options: AnalysisOptions::default(),
        }
    }

    /// Create an expression analyzer with options
    pub fn with_options(options: AnalysisOptions) -> Self {
        Self { options }
    }

    /// Create an analyzer that only checks for certainty.
    pub fn deterministic_only() -> Self {
        Self {
            options: AnalysisOptions::from_mode(AnalysisMode::DeterministicOnly),
        }
    }

    /// Create an analyzer that only extracts references to attributes.
    pub fn property_extractor() -> Self {
        Self {
            options: AnalysisOptions::from_mode(AnalysisMode::PropertyExtractor),
        }
    }

    /// Create an analyzer that only extracts variable references.
    pub fn variable_extractor() -> Self {
        Self {
            options: AnalysisOptions::from_mode(AnalysisMode::VariableExtractor),
        }
    }

    /// Analyze the expression (accepts a ContextualExpression)
    ///
    /// # Parameters
    /// `ctx_expr`: The context expression that needs to be analyzed.
    ///
    /// # Return
    /// Analysis results of the expression
    pub fn analyze(&self, ctx_expr: &ContextualExpression) -> ExpressionAnalysis {
        let mut analysis = ExpressionAnalysis::new();

        // Obtain the Expression using ContextualExpression.
        if let Some(expr_meta) = ctx_expr.expression() {
            let expr = expr_meta.inner();

            // Use the existing Collector to collect information.
            if self.options.extract_properties {
                let mut collector = PropertyCollector::new();
                collector.visit(expr);
                analysis.referenced_properties = collector.properties;
            }

            if self.options.extract_variables {
                let mut collector = VariableCollector::new();
                collector.visit(expr);
                analysis.referenced_variables = collector.variables;
            }

            if self.options.count_functions {
                let mut collector = FunctionCollector::new();
                collector.visit(expr);
                analysis.called_functions = collector.functions;
            }

            // Using a custom Visitor for complexity and certainty analysis
            let mut visitor = AnalysisVisitor::new(&mut analysis, self.options.clone());
            visitor.visit(expr);
        }

        analysis
    }

    /// Quickly check whether the expression is deterministic.
    pub fn is_deterministic(&self, ctx_expr: &ContextualExpression) -> bool {
        let analysis = self.analyze(ctx_expr);
        analysis.is_deterministic
    }

    /// Quickly extract the attributes referenced by the expression.
    pub fn extract_properties(&self, ctx_expr: &ContextualExpression) -> Vec<String> {
        let analysis = self.analyze(ctx_expr);
        analysis.referenced_properties
    }

    /// Quickly extract the variables referenced by the expressions.
    pub fn extract_variables(&self, ctx_expr: &ContextualExpression) -> Vec<String> {
        let analysis = self.analyze(ctx_expr);
        analysis.referenced_variables
    }
}

/// Expression analysis: “Visitor”
///
/// Using the Visitor pattern for complexity and certainty analysis
struct AnalysisVisitor<'a> {
    analysis: &'a mut ExpressionAnalysis,
    options: AnalysisOptions,
}

impl<'a> AnalysisVisitor<'a> {
    fn new(analysis: &'a mut ExpressionAnalysis, options: AnalysisOptions) -> Self {
        Self { analysis, options }
    }
}

impl ExpressionVisitor for AnalysisVisitor<'_> {
    fn visit_literal(&mut self, _value: &crate::core::Value) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 1;
        }
        self.analysis.node_count += 1;
    }

    fn visit_variable(&mut self, _name: &str) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 2;
        }
        self.analysis.node_count += 1;
    }

    fn visit_property(&mut self, object: &Expression, _property: &str) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 5;
        }
        self.analysis.node_count += 1;
        self.visit(object);
    }

    fn visit_binary(
        &mut self,
        op: crate::core::types::BinaryOperator,
        left: &Expression,
        right: &Expression,
    ) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 2;
            if op == crate::core::types::BinaryOperator::Like {
                self.analysis.complexity_score += 5;
            }
        }
        self.analysis.node_count += 1;
        self.visit(left);
        self.visit(right);
    }

    fn visit_unary(&mut self, _op: crate::core::types::UnaryOperator, operand: &Expression) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 1;
        }
        self.analysis.node_count += 1;
        self.visit(operand);
    }

    fn visit_function(&mut self, name: &str, args: &[Expression]) {
        if self.options.check_deterministic && NondeterministicChecker::is_nondeterministic(name) {
            self.analysis.is_deterministic = false;
        }
        if self.options.check_complexity {
            self.analysis.complexity_score += 10 + args.len() as u32 * 2;
        }
        self.analysis.node_count += 1;
        for arg in args {
            self.visit(arg);
        }
    }

    fn visit_aggregate(
        &mut self,
        _func: &crate::core::types::operators::AggregateFunction,
        arg: &Expression,
        _distinct: bool,
    ) {
        self.analysis.contains_aggregate = true;
        if self.options.check_complexity {
            self.analysis.complexity_score += 20;
        }
        self.analysis.node_count += 1;
        self.visit(arg);
    }

    fn visit_case(
        &mut self,
        test_expr: Option<&Expression>,
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 5 + conditions.len() as u32 * 5;
        }
        self.analysis.node_count += 1;
        if let Some(test) = test_expr {
            self.visit(test);
        }
        for (when, then) in conditions {
            self.visit(when);
            self.visit(then);
        }
        if let Some(default_expr) = default {
            self.visit(default_expr);
        }
    }

    fn visit_type_cast(
        &mut self,
        expression: &Expression,
        _target_type: &crate::core::types::DataType,
    ) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 3;
        }
        self.analysis.node_count += 1;
        self.visit(expression);
    }

    fn visit_subscript(&mut self, collection: &Expression, index: &Expression) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 4;
        }
        self.analysis.node_count += 1;
        self.visit(collection);
        self.visit(index);
    }

    fn visit_list(&mut self, items: &[Expression]) {
        if self.options.check_complexity {
            self.analysis.complexity_score += items.len() as u32;
        }
        self.analysis.node_count += 1;
        for item in items {
            self.visit(item);
        }
    }

    fn visit_map(&mut self, entries: &[(String, Expression)]) {
        if self.options.check_complexity {
            self.analysis.complexity_score += entries.len() as u32 * 2;
        }
        self.analysis.node_count += 1;
        for (_, value) in entries {
            self.visit(value);
        }
    }

    fn visit_list_comprehension(
        &mut self,
        _variable: &str,
        source: &Expression,
        filter: Option<&Expression>,
        map: Option<&Expression>,
    ) {
        self.analysis.contains_subquery = true;
        if self.options.check_complexity {
            self.analysis.complexity_score += 30;
        }
        self.analysis.node_count += 1;
        self.visit(source);
        if let Some(f) = filter {
            self.visit(f);
        }
        if let Some(m) = map {
            self.visit(m);
        }
    }

    fn visit_predicate(&mut self, _func: &str, args: &[Expression]) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 15;
        }
        self.analysis.node_count += 1;
        for arg in args {
            self.visit(arg);
        }
    }

    fn visit_reduce(
        &mut self,
        _accumulator: &str,
        initial: &Expression,
        _variable: &str,
        source: &Expression,
        mapping: &Expression,
    ) {
        self.analysis.contains_subquery = true;
        if self.options.check_complexity {
            self.analysis.complexity_score += 25;
        }
        self.analysis.node_count += 1;
        self.visit(initial);
        self.visit(source);
        self.visit(mapping);
    }

    fn visit_path(&mut self, items: &[Expression]) {
        if self.options.check_complexity {
            self.analysis.complexity_score += items.len() as u32 * 3;
        }
        self.analysis.node_count += 1;
        for item in items {
            self.visit(item);
        }
    }

    fn visit_path_build(&mut self, items: &[Expression]) {
        if self.options.check_complexity {
            self.analysis.complexity_score += items.len() as u32 * 2;
        }
        self.analysis.node_count += 1;
        for item in items {
            self.visit(item);
        }
    }

    fn visit_range(
        &mut self,
        collection: &Expression,
        start: Option<&Expression>,
        end: Option<&Expression>,
    ) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 5;
        }
        self.analysis.node_count += 1;
        self.visit(collection);
        if let Some(s) = start {
            self.visit(s);
        }
        if let Some(e) = end {
            self.visit(e);
        }
    }

    fn visit_label(&mut self, _label: &str) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 3;
        }
        self.analysis.node_count += 1;
    }

    fn visit_label_tag_property(&mut self, tag: &Expression, _property: &str) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 5;
        }
        self.analysis.node_count += 1;
        self.visit(tag);
    }

    fn visit_tag_property(&mut self, _tag_name: &str, _property: &str) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 3;
        }
        self.analysis.node_count += 1;
    }

    fn visit_edge_property(&mut self, _edge_name: &str, _property: &str) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 3;
        }
        self.analysis.node_count += 1;
    }

    fn visit_parameter(&mut self, _name: &str) {
        if self.options.check_complexity {
            self.analysis.complexity_score += 1;
        }
        self.analysis.node_count += 1;
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
    use crate::core::Value;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_expression_analyzer_new() {
        let _analyzer = ExpressionAnalyzer::new();
        // Verification of successful creation.
    }

    #[test]
    fn test_literal_is_deterministic() {
        let analyzer = ExpressionAnalyzer::new();
        let expr = Expression::Literal(Value::Int(42));
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(expr_id, expr_ctx);
        let analysis = analyzer.analyze(&ctx_expr);
        assert!(analysis.is_deterministic);
        assert_eq!(analysis.node_count, 1);
    }

    #[test]
    fn test_variable_extraction() {
        let analyzer = ExpressionAnalyzer::new();
        let expr = Expression::Variable("x".to_string());
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(expr_id, expr_ctx);
        let analysis = analyzer.analyze(&ctx_expr);
        assert!(analysis.referenced_variables.contains(&"x".to_string()));
    }

    #[test]
    fn test_nondeterministic_function_detection() {
        let analyzer = ExpressionAnalyzer::new();
        let expr = Expression::Function {
            name: "rand".to_string(),
            args: vec![],
        };
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(expr_id, expr_ctx);
        let analysis = analyzer.analyze(&ctx_expr);
        assert!(!analysis.is_deterministic);
    }

    #[test]
    fn test_deterministic_function() {
        let analyzer = ExpressionAnalyzer::new();
        let expr = Expression::Function {
            name: "abs".to_string(),
            args: vec![Expression::Literal(Value::Int(-5))],
        };
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(expr_id, expr_ctx);
        let analysis = analyzer.analyze(&ctx_expr);
        assert!(analysis.is_deterministic);
    }

    #[test]
    fn test_property_extraction() {
        let analyzer = ExpressionAnalyzer::property_extractor();
        let expr = Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "name".to_string(),
        };
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(expr_id, expr_ctx);
        let analysis = analyzer.analyze(&ctx_expr);
        assert!(analysis.referenced_properties.contains(&"name".to_string()));
    }

    #[test]
    fn test_complexity_score() {
        let analyzer = ExpressionAnalyzer::new();
        // Simple expressions
        let simple = Expression::Literal(Value::Int(1));
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let simple_meta = crate::core::types::expr::ExpressionMeta::new(simple);
        let simple_id = expr_ctx.register_expression(simple_meta);
        let simple_ctx_expr =
            crate::core::types::ContextualExpression::new(simple_id, expr_ctx.clone());
        let simple_analysis = analyzer.analyze(&simple_ctx_expr);
        assert!(simple_analysis.complexity_score < 10);

        // Complex expressions
        let complex = Expression::Function {
            name: "coalesce".to_string(),
            args: vec![
                Expression::Property {
                    object: Box::new(Expression::Variable("a".to_string())),
                    property: "x".to_string(),
                },
                Expression::Property {
                    object: Box::new(Expression::Variable("b".to_string())),
                    property: "y".to_string(),
                },
                Expression::Literal(Value::Null(crate::core::value::NullType::Null)),
            ],
        };
        let complex_meta = crate::core::types::expr::ExpressionMeta::new(complex);
        let complex_id = expr_ctx.register_expression(complex_meta);
        let complex_ctx_expr = crate::core::types::ContextualExpression::new(complex_id, expr_ctx);
        let complex_analysis = analyzer.analyze(&complex_ctx_expr);
        assert!(complex_analysis.complexity_score > simple_analysis.complexity_score);
    }
}
