//! Function Registry
//!
//! Provide functions for registration, lookup, and execution.
//! The specific implementation of the function is located in the builtin submodule.

use super::BuiltinFunction;
use super::CustomFunction;
use crate::core::Value;
use crate::query::executor::expression::evaluation_context::graph_storage::GraphStorageRef;
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

    /// Execute a function with graph storage access
    pub fn execute_with_storage(
        &self,
        name: &str,
        args: &[Value],
        storage: &GraphStorageRef,
    ) -> Result<Value, ExpressionError> {
        let upper_name = name.to_uppercase();
        if let Some(func) = self.builtin_functions.get(&upper_name) {
            return func.execute_with_storage(args, storage);
        }
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
        self.register_builtin(BuiltinFunction::Math(MathFunction::Atan2));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Sinh));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Cosh));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Tanh));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Degrees));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Gcd));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Lcm));

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
        self.register_builtin(BuiltinFunction::String(StringFunction::Levenshtein));
        self.register_builtin(BuiltinFunction::String(StringFunction::SplitPart));
        self.register_builtin(BuiltinFunction::String(StringFunction::Initcap));
        self.register_builtin(BuiltinFunction::String(StringFunction::Repeat));
        self.register_builtin(BuiltinFunction::String(StringFunction::Position));
        self.register_builtin(BuiltinFunction::String(StringFunction::Left));
        self.register_builtin(BuiltinFunction::String(StringFunction::Right));
        self.register_builtin(BuiltinFunction::String(StringFunction::StringInsert));
        self.register_builtin(BuiltinFunction::String(StringFunction::Translate));
        self.register_builtin(BuiltinFunction::String(StringFunction::Format));
        self.register_builtin(BuiltinFunction::String(StringFunction::StringSplit));

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
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::DateAdd));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::DateSub));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::DateDiff));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::DateTrunc));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::CurrentDate));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::CurrentTimestamp));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToChar));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToDate));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Age));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::LastDay));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::GenerateSeries));

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
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::NullIf));
        self.register_builtin(BuiltinFunction::Utility(
            UtilityFunction::JsonBuildObject,
        ));
        self.register_builtin(BuiltinFunction::Utility(
            UtilityFunction::JsonBuildArray,
        ));
        self.register_builtin(BuiltinFunction::Utility(
            UtilityFunction::JsonObjectKeys,
        ));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Greatest));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Least));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::GenRandomUuid));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::JsonEach));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::JsonTypeOf));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::JsonStripNulls));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::IfNull));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::TypeOf));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Version));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::CurrentUser));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::CurrentDatabase));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Corr));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::CovarPop));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::CovarSamp));

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
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Variance(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Median(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Mode(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::BoolAnd(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::BoolOr(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::StddevPop(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::StddevSamp(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Product(
            String::new(),
        )));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::PercentileCont(
            String::new(),
            50.0,
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
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Neighbors));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Degree));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::OutEdges));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::InEdges));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::ShortestPath));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::Bfs));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::ConnectedComponents));
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::VariableLengthPath));

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
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListContains));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListAppend));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListPrepend));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListFilter));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListTransform));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListConcat));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListSort));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListSlice));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListToString));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListDistinct));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListUnique));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ListExtract));

        // Registration path function
        use super::PathFunction;
        self.register_builtin(BuiltinFunction::Path(PathFunction::Nodes));
        self.register_builtin(BuiltinFunction::Path(PathFunction::Relationships));

        // Register full-text search functions
        super::fulltext::register_fulltext_functions(self);

        // Register vector functions
        super::builtin::vector::register_vector_functions(self);

        // Register window functions
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::RowNumber));
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::Rank));
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::DenseRank));
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::Lead));
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::Lag));
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::FirstValue));
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::LastValue));
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::NthValue));
        self.register_builtin(BuiltinFunction::Window(super::builtin::window::WindowFunction::Ntile));
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
