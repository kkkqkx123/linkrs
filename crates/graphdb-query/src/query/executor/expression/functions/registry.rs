//! Function Registry
//!
//! Provide functions for registration, lookup, and execution.
//! The specific implementation of the function is located in the builtin submodule.

use super::BuiltinFunction;
use super::CustomFunction;
use crate::core::Value;
use crate::query::executor::expression::{ExpressionError, ExpressionErrorType};
use std::collections::HashMap;
use std::sync::Arc;

/// Function Registry
///
/// Using a static distribution mechanism, functions are called directly through the BuiltinFunction and CustomFunction enumerations.
/// The overhead associated with dynamic distribution (dyn) was avoided.
#[derive(Debug)]
pub struct FunctionRegistry {
    /// Built-in function mapping (function name -> BuiltinFunction enumeration)
    builtin_functions: HashMap<String, BuiltinFunction>,
    /// Custom function mapping (function name -> CustomFunction)
    custom_functions: HashMap<String, CustomFunction>,
}

impl Default for FunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            builtin_functions: HashMap::new(),
            custom_functions: HashMap::new(),
        };
        registry.register_all_builtin_functions();
        registry
    }

    /// Check whether the function exists.
    pub fn contains(&self, name: &str) -> bool {
        let upper_name = name.to_uppercase();
        self.builtin_functions.contains_key(&upper_name)
            || self.custom_functions.contains_key(&upper_name)
    }

    /// Obtain all function names
    pub fn function_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.builtin_functions.keys().map(|s| s.as_str()).collect();
        names.extend(self.custom_functions.keys().map(|s| s.as_str()));
        names
    }

    /// Registering built-in functions
    pub fn register_builtin(&mut self, function: BuiltinFunction) {
        let upper_name = function.name().to_uppercase();
        self.builtin_functions.insert(upper_name, function);
    }

    /// Obtaining built-in functions
    pub fn get_builtin(&self, name: &str) -> Option<&BuiltinFunction> {
        // Convert to uppercase for case-insensitive lookup
        let upper_name = name.to_uppercase();
        self.builtin_functions.get(&upper_name)
    }

    /// Registering a custom function (full form)
    pub fn register_custom_full(&mut self, function: CustomFunction) {
        let upper_name = function.name.to_uppercase();
        self.custom_functions.insert(upper_name, function);
    }

    /// Obtaining a custom function
    pub fn get_custom(&self, name: &str) -> Option<&CustomFunction> {
        // Convert to uppercase for case-insensitive lookup
        let upper_name = name.to_uppercase();
        self.custom_functions.get(&upper_name)
    }

    /// Execute a function (based on its name)
    pub fn execute(&self, name: &str, args: &[Value]) -> Result<Value, ExpressionError> {
        // Convert to uppercase for case-insensitive lookup
        let upper_name = name.to_uppercase();
        // Try to find the built-in functions first.
        if let Some(func) = self.builtin_functions.get(&upper_name) {
            return func.execute(args);
        }

        // Try to find the custom function again.
        if let Some(func) = self.custom_functions.get(&upper_name) {
            return func.execute(args);
        }

        Err(ExpressionError::new(
            ExpressionErrorType::UndefinedFunction,
            format!("Undefined function: {}", name),
        ))
    }

    /// Register all built-in functions
    fn register_all_builtin_functions(&mut self) {
        use super::ConversionFunction;
        use super::DateTimeFunction;
        use super::MathFunction;
        use super::RegexFunction;
        use super::StringFunction;

        // Registering a mathematical function
        self.register_builtin(BuiltinFunction::Math(MathFunction::Abs));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Sqrt));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Pow));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Log));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Log10));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Sin));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Cos));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Tan));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Round));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Ceil));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Floor));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Asin));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Acos));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Atan));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Cbrt));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Hypot));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Sign));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Rand));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Rand32));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Rand64));
        self.register_builtin(BuiltinFunction::Math(MathFunction::E));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Pi));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Exp2));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Log2));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Radians));
        self.register_builtin(BuiltinFunction::Math(MathFunction::BitAnd));
        self.register_builtin(BuiltinFunction::Math(MathFunction::BitOr));
        self.register_builtin(BuiltinFunction::Math(MathFunction::BitXor));

        // Register string function
        self.register_builtin(BuiltinFunction::String(StringFunction::Length));
        self.register_builtin(BuiltinFunction::String(StringFunction::Upper));
        self.register_builtin(BuiltinFunction::String(StringFunction::Lower));
        self.register_builtin(BuiltinFunction::String(StringFunction::Trim));
        self.register_builtin(BuiltinFunction::String(StringFunction::Substring));
        self.register_builtin(BuiltinFunction::String(StringFunction::Concat));
        self.register_builtin(BuiltinFunction::String(StringFunction::Replace));
        self.register_builtin(BuiltinFunction::String(StringFunction::Contains));
        self.register_builtin(BuiltinFunction::String(StringFunction::StartsWith));
        self.register_builtin(BuiltinFunction::String(StringFunction::EndsWith));
        self.register_builtin(BuiltinFunction::String(StringFunction::Split));
        self.register_builtin(BuiltinFunction::String(StringFunction::Lpad));
        self.register_builtin(BuiltinFunction::String(StringFunction::Rpad));
        self.register_builtin(BuiltinFunction::String(StringFunction::ConcatWs));
        self.register_builtin(BuiltinFunction::String(StringFunction::Strcasecmp));

        // Registering regular expression functions
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexMatch));
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexReplace));
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexFind));

        // Registration type conversion function
        self.register_builtin(BuiltinFunction::Conversion(ConversionFunction::ToString));
        self.register_builtin(BuiltinFunction::Conversion(ConversionFunction::ToInt));
        self.register_builtin(BuiltinFunction::Conversion(ConversionFunction::ToFloat));
        self.register_builtin(BuiltinFunction::Conversion(ConversionFunction::ToBool));

        // Registration date and time function
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Now));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Date));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Time));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::DateTime));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Year));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Month));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Day));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Hour));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Minute));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Second));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::TimeStamp));

        // Registering geospatial functions
        use super::GeographyFunction;
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StPoint));
        self.register_builtin(BuiltinFunction::Geography(
            GeographyFunction::StGeogFromText,
        ));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StAsText));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StCentroid));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StIsValid));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StIntersects));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StCovers));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StCoveredBy));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StDWithin));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StDistance));

        // Registering practical functions
        use super::UtilityFunction;
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Coalesce));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Hash));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::JsonExtract));

        // Register aggregate functions
        use crate::core::types::operators::AggregateFunction;
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Count(None)));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Sum(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Avg(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Min(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Max(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Collect(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Distinct(
            String::new(),
        )));

        // Registering functions related to graphics
        use super::GraphFunction;
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Id));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Tags));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Labels));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Properties));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::EdgeType));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Src));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Dst));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Rank));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::StartNode));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::EndNode));

        // Register container operation functions
        use super::ContainerFunction;
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::Head));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::Last));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::Tail));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::Size));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::Range));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::Keys));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ReverseList));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ToSet));

        // Registration path function
        use super::PathFunction;
        self.register_builtin(BuiltinFunction::Path(PathFunction::Nodes));
        self.register_builtin(BuiltinFunction::Path(PathFunction::Relationships));

        // Register full-text search functions
        super::fulltext::register_fulltext_functions(self);

        // Register vector functions
        super::builtin::vector::register_vector_functions(self);
    }
}

/// Global function registry instance
pub fn global_registry() -> Arc<FunctionRegistry> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Arc<FunctionRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Arc::new(FunctionRegistry::new()))
        .clone()
}

/// Obtain a static reference to the global function registry.
///
/// Used in scenarios where it is necessary to retrieve a function reference (such as in ExpressionContext::get_function).
pub fn global_registry_ref() -> &'static FunctionRegistry {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<FunctionRegistry> = OnceLock::new();
    REGISTRY.get_or_init(FunctionRegistry::new)
}
