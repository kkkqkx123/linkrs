//! Function Registry
//!
//! Provide functions for registration, lookup, and execution.
//! The specific implementation of the function is located in the builtin submodule.

use super::BuiltinFunction;
use super::CustomFunction;
use crate::executor::expression::evaluation_context::graph_storage::GraphStorageRef;
use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use graphdb_core::DataType;
use graphdb_core::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Registry entry carrying the function and its static return type.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub function: BuiltinFunction,
    pub return_type: DataType,
}

/// Function Registry
///
/// Using a static distribution mechanism, functions are called directly through the BuiltinFunction and CustomFunction enumerations.
/// The overhead associated with dynamic distribution (dyn) was avoided.
#[derive(Debug)]
pub struct FunctionRegistry {
    /// Built-in function mapping (function name -> RegistryEntry)
    builtin_functions: HashMap<String, RegistryEntry>,
    /// Alias mapping: alias upper -> canonical upper
    aliases: HashMap<String, String>,
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
            aliases: HashMap::new(),
            custom_functions: HashMap::new(),
        };
        registry.register_all_builtin_functions();
        registry
    }

    fn canonical_name(&self, upper: &str) -> Option<String> {
        if self.builtin_functions.contains_key(upper) {
            Some(upper.to_string())
        } else {
            self.aliases.get(upper).cloned()
        }
    }

    fn resolve_entry(&self, name: &str) -> Option<&RegistryEntry> {
        let upper = name.to_uppercase();
        if let Some(entry) = self.builtin_functions.get(&upper) {
            return Some(entry);
        }
        if let Some(canon) = self.aliases.get(&upper) {
            return self.builtin_functions.get(canon);
        }
        None
    }

    /// Check whether the function exists.
    pub fn contains(&self, name: &str) -> bool {
        let upper_name = name.to_uppercase();
        self.builtin_functions.contains_key(&upper_name)
            || self.aliases.contains_key(&upper_name)
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
        let return_type = builtin_return_type(&function);
        self.builtin_functions.insert(
            upper_name,
            RegistryEntry {
                function,
                return_type,
            },
        );
    }

    /// Register an alias for a built-in function.
    pub fn register_alias(&mut self, alias: &str, canonical: &str) {
        self.aliases
            .insert(alias.to_uppercase(), canonical.to_uppercase());
    }

    /// Obtaining built-in functions
    pub fn get_builtin(&self, name: &str) -> Option<&BuiltinFunction> {
        self.resolve_entry(name).map(|e| &e.function)
    }

    /// Get the full registry entry (function + return type).
    pub fn get_entry(&self, name: &str) -> Option<&RegistryEntry> {
        self.resolve_entry(name)
    }

    /// Get the static return type for a function name, resolving aliases.
    pub fn get_return_type(&self, name: &str) -> Option<&DataType> {
        self.resolve_entry(name).map(|e| &e.return_type)
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
        let upper_name = name.to_uppercase();
        let canonical = self
            .canonical_name(&upper_name)
            .unwrap_or(upper_name.clone());
        if let Some(entry) = self.builtin_functions.get(&canonical) {
            return entry.function.execute(args);
        }
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
        let canonical = self
            .canonical_name(&upper_name)
            .unwrap_or(upper_name.clone());
        if let Some(entry) = self.builtin_functions.get(&canonical) {
            return entry.function.execute_with_storage(args, storage);
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

        self.register_builtin(BuiltinFunction::Math(MathFunction::Factorial));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Gamma));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Lgamma));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Negate));
        self.register_builtin(BuiltinFunction::Math(MathFunction::Even));
        self.register_builtin(BuiltinFunction::Math(MathFunction::SetSeed));
        self.register_builtin(BuiltinFunction::Math(MathFunction::BitShiftLeft));
        self.register_builtin(BuiltinFunction::Math(MathFunction::BitShiftRight));

        // Register string function
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
        self.register_builtin(BuiltinFunction::String(StringFunction::Reverse));

        // Registering regular expression functions
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexMatch));
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexReplace));
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexFind));
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexpFullMatch));
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexpExtract));
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexpExtractAll));
        self.register_builtin(BuiltinFunction::Regex(RegexFunction::RegexpSplitToArray));

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
        self.register_builtin(BuiltinFunction::DateTime(
            DateTimeFunction::CurrentTimestamp,
        ));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToChar));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToDate));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Age));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::LastDay));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::GenerateSeries));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToYears));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToMonths));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToDays));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToHours));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToMinutes));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToSeconds));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToMilliseconds));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToMicroseconds));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::Century));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::EpochMs));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToTimestamp));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::ToEpochMs));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::DatePart));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::DayName));
        self.register_builtin(BuiltinFunction::DateTime(DateTimeFunction::MonthName));

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
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StArea));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StLength));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StPerimeter));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StNPoints));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StStartPoint));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StEndPoint));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StIsRing));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StIsClosed));
        self.register_builtin(BuiltinFunction::Geography(
            GeographyFunction::StGeometryType,
        ));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StContains));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StWithin));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StEnvelope));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StBuffer));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StBoundary));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StCrosses));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StTouches));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StOverlaps));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StEquals));
        self.register_builtin(BuiltinFunction::Geography(GeographyFunction::StAsGeoJson));
        self.register_builtin(BuiltinFunction::Geography(
            GeographyFunction::StGeomFromGeoJson,
        ));

        // Registering practical functions
        use super::UtilityFunction;
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Coalesce));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Hash));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::JsonExtract));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::NullIf));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::JsonBuildObject));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::JsonBuildArray));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::JsonObjectKeys));
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

        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::OctetLength));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Encode));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::Decode));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::UnionValue));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::UnionTag));
        self.register_builtin(BuiltinFunction::Utility(UtilityFunction::UnionExtract));

        // Register aggregate functions
        use graphdb_core::types::operators::AggregateFunction;
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Count));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Sum));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Avg));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Min));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Max));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Collect));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::CollectSet));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Variance));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Median));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Mode));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::BoolAnd));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::BoolOr));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::StddevPop));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::StddevSamp));
        self.register_builtin(BuiltinFunction::Aggregate(AggregateFunction::Product));
        self.register_builtin(BuiltinFunction::Aggregate(
            AggregateFunction::PercentileCont,
        ));
        self.register_builtin(BuiltinFunction::Aggregate(
            AggregateFunction::GroupConcatWithOrder,
        ));

        // Register window functions
        // Window functions are only meaningful inside an OVER() clause and are
        // computed by the window operator (see blocking/window.rs), so they are
        // registered first to avoid shadowing graph functions with the same name.
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::RowNumber,
        ));
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::Rank,
        ));
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::DenseRank,
        ));
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::Lead,
        ));
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::Lag,
        ));
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::FirstValue,
        ));
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::LastValue,
        ));
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::NthValue,
        ));
        self.register_builtin(BuiltinFunction::Window(
            super::builtin::window::WindowFunction::Ntile,
        ));

        // Register full-text search functions
        // Full-text functions are executed through execute_with_context directly
        // (see fulltext.rs), so they are registered before graph functions to
        // avoid the full-text rank() shadowing the graph rank(edge).
        super::fulltext::register_fulltext_functions(self);

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
        self.register_builtin(BuiltinFunction::Graph(GraphFunction::PageRank));

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
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::StructPack));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::StructExtract));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::MapCreation));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::MapExtract));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::ElementAt));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::Cardinality));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::MapKeys));
        self.register_builtin(BuiltinFunction::Container(ContainerFunction::MapValues));

        // Register path function
        use super::PathFunction;
        self.register_builtin(BuiltinFunction::Path(PathFunction::Nodes));
        self.register_builtin(BuiltinFunction::Path(PathFunction::Relationships));
        self.register_builtin(BuiltinFunction::Path(PathFunction::Properties));
        self.register_builtin(BuiltinFunction::Path(PathFunction::IsTrail));
        self.register_builtin(BuiltinFunction::Path(PathFunction::IsAcyclic));
        // Polymorphic `length` (string, path, list): the single definition
        // of this name, so dispatch can never silently shadow another one.
        self.register_builtin(BuiltinFunction::Path(PathFunction::PathLength));

        // Register vector functions
        super::builtin::vector::register_vector_functions(self);

        // Register sequence functions
        use super::SequenceFunction;
        self.register_builtin(BuiltinFunction::Sequence(SequenceFunction::CurrVal));
        self.register_builtin(BuiltinFunction::Sequence(SequenceFunction::NextVal));

        // Register conversion aliases for backward compatibility
        self.register_alias("TOINTEGER", "TO_INT");
        self.register_alias("TOFLOAT", "TO_FLOAT");
        self.register_alias("TOBOOLEAN", "TO_BOOL");
        self.register_alias("TOSTRING", "TO_STRING");
        // Also support camelCase variants via uppercase mapping (already covered)
        // but keep explicit for clarity.
        self.register_alias("TO_INTEGER", "TO_INT");
    }
}

fn builtin_return_type(func: &BuiltinFunction) -> DataType {
    use super::ContainerFunction;
    use super::ConversionFunction;
    use super::DateTimeFunction;
    use super::GeographyFunction;
    use super::GraphFunction;
    use super::MathFunction;
    use super::PathFunction;
    use super::RegexFunction;
    use super::SequenceFunction;
    use super::StringFunction;
    use super::UtilityFunction;
    use super::VectorFunction;
    use crate::executor::expression::functions::builtin::window::WindowFunction;
    use crate::executor::expression::functions::FulltextFunction;
    use graphdb_core::types::operators::AggregateFunction;
    match func {
        BuiltinFunction::Math(m) => match m {
            MathFunction::Abs => DataType::Int,
            MathFunction::Sqrt => DataType::Float,
            MathFunction::Pow => DataType::Float,
            MathFunction::Log => DataType::Float,
            MathFunction::Log10 => DataType::Float,
            MathFunction::Sin => DataType::Float,
            MathFunction::Cos => DataType::Float,
            MathFunction::Tan => DataType::Float,
            MathFunction::Round => DataType::Float,
            MathFunction::Ceil => DataType::Float,
            MathFunction::Floor => DataType::Float,
            MathFunction::Asin => DataType::Float,
            MathFunction::Acos => DataType::Float,
            MathFunction::Atan => DataType::Float,
            MathFunction::Cbrt => DataType::Float,
            MathFunction::Hypot => DataType::Float,
            MathFunction::Sign => DataType::Int,
            MathFunction::Rand => DataType::Float,
            MathFunction::Rand32 => DataType::Int,
            MathFunction::Rand64 => DataType::BigInt,
            MathFunction::E => DataType::Float,
            MathFunction::Pi => DataType::Float,
            MathFunction::Exp2 => DataType::Float,
            MathFunction::Log2 => DataType::Float,
            MathFunction::Radians => DataType::Float,
            MathFunction::BitAnd => DataType::Int,
            MathFunction::BitOr => DataType::Int,
            MathFunction::BitXor => DataType::Int,
            MathFunction::Atan2 => DataType::Float,
            MathFunction::Sinh => DataType::Float,
            MathFunction::Cosh => DataType::Float,
            MathFunction::Tanh => DataType::Float,
            MathFunction::Degrees => DataType::Float,
            MathFunction::Gcd => DataType::Int,
            MathFunction::Lcm => DataType::Int,
            MathFunction::Factorial => DataType::BigInt,
            MathFunction::Gamma => DataType::Float,
            MathFunction::Lgamma => DataType::Float,
            MathFunction::Negate => DataType::Int,
            MathFunction::Even => DataType::Int,
            MathFunction::SetSeed => DataType::Null,
            MathFunction::BitShiftLeft => DataType::Int,
            MathFunction::BitShiftRight => DataType::Int,
        },
        BuiltinFunction::String(s) => match s {
            StringFunction::Upper => DataType::String,
            StringFunction::Lower => DataType::String,
            StringFunction::Trim => DataType::String,
            StringFunction::Substring => DataType::String,
            StringFunction::Concat => DataType::String,
            StringFunction::Replace => DataType::String,
            StringFunction::Contains => DataType::Bool,
            StringFunction::StartsWith => DataType::Bool,
            StringFunction::EndsWith => DataType::Bool,
            StringFunction::Split => DataType::List(Box::new(DataType::String)),
            StringFunction::Lpad => DataType::String,
            StringFunction::Rpad => DataType::String,
            StringFunction::ConcatWs => DataType::String,
            StringFunction::Strcasecmp => DataType::Int,
            StringFunction::Levenshtein => DataType::Int,
            StringFunction::SplitPart => DataType::String,
            StringFunction::Initcap => DataType::String,
            StringFunction::Repeat => DataType::String,
            StringFunction::Position => DataType::Int,
            StringFunction::Left => DataType::String,
            StringFunction::Right => DataType::String,
            StringFunction::StringInsert => DataType::String,
            StringFunction::Translate => DataType::String,
            StringFunction::Format => DataType::String,
            StringFunction::StringSplit => DataType::List(Box::new(DataType::String)),
            StringFunction::Reverse => DataType::String,
        },
        BuiltinFunction::Regex(r) => match r {
            RegexFunction::RegexMatch => DataType::Bool,
            RegexFunction::RegexReplace => DataType::String,
            RegexFunction::RegexFind => DataType::String,
            RegexFunction::RegexpFullMatch => DataType::Bool,
            RegexFunction::RegexpExtract => DataType::String,
            RegexFunction::RegexpExtractAll => DataType::List(Box::new(DataType::String)),
            RegexFunction::RegexpSplitToArray => DataType::List(Box::new(DataType::String)),
        },
        BuiltinFunction::Conversion(c) => match c {
            ConversionFunction::ToString => DataType::String,
            ConversionFunction::ToInt => DataType::Int,
            ConversionFunction::ToFloat => DataType::Float,
            ConversionFunction::ToBool => DataType::Bool,
        },
        BuiltinFunction::DateTime(d) => match d {
            DateTimeFunction::Now => DataType::DateTime,
            DateTimeFunction::Date => DataType::Date,
            DateTimeFunction::Time => DataType::Time,
            DateTimeFunction::DateTime => DataType::DateTime,
            DateTimeFunction::Year => DataType::Int,
            DateTimeFunction::Month => DataType::Int,
            DateTimeFunction::Day => DataType::Int,
            DateTimeFunction::Hour => DataType::Int,
            DateTimeFunction::Minute => DataType::Int,
            DateTimeFunction::Second => DataType::Int,
            DateTimeFunction::TimeStamp => DataType::BigInt,
            DateTimeFunction::DateAdd => DataType::Unknown,
            DateTimeFunction::DateSub => DataType::Unknown,
            DateTimeFunction::DateDiff => DataType::BigInt,
            DateTimeFunction::DateTrunc => DataType::Unknown,
            DateTimeFunction::CurrentDate => DataType::Date,
            DateTimeFunction::CurrentTimestamp => DataType::BigInt,
            DateTimeFunction::ToChar => DataType::String,
            DateTimeFunction::ToDate => DataType::Date,
            DateTimeFunction::Age => DataType::BigInt,
            DateTimeFunction::LastDay => DataType::Unknown,
            DateTimeFunction::GenerateSeries => DataType::List(Box::new(DataType::BigInt)),
            DateTimeFunction::ToYears => DataType::BigInt,
            DateTimeFunction::ToMonths => DataType::BigInt,
            DateTimeFunction::ToDays => DataType::BigInt,
            DateTimeFunction::ToHours => DataType::BigInt,
            DateTimeFunction::ToMinutes => DataType::BigInt,
            DateTimeFunction::ToSeconds => DataType::BigInt,
            DateTimeFunction::ToMilliseconds => DataType::BigInt,
            DateTimeFunction::ToMicroseconds => DataType::BigInt,
            DateTimeFunction::Century => DataType::Int,
            DateTimeFunction::EpochMs => DataType::BigInt,
            DateTimeFunction::ToTimestamp => DataType::DateTime,
            DateTimeFunction::ToEpochMs => DataType::BigInt,
            DateTimeFunction::DatePart => DataType::Int,
            DateTimeFunction::DayName => DataType::String,
            DateTimeFunction::MonthName => DataType::String,
        },
        BuiltinFunction::Geography(g) => match g {
            GeographyFunction::StPoint => DataType::Geography,
            GeographyFunction::StGeogFromText => DataType::Geography,
            GeographyFunction::StAsText => DataType::String,
            GeographyFunction::StCentroid => DataType::Geography,
            GeographyFunction::StIsValid => DataType::Bool,
            GeographyFunction::StIntersects => DataType::Bool,
            GeographyFunction::StCovers => DataType::Bool,
            GeographyFunction::StCoveredBy => DataType::Bool,
            GeographyFunction::StDWithin => DataType::Bool,
            GeographyFunction::StDistance => DataType::Double,
            GeographyFunction::StArea => DataType::Double,
            GeographyFunction::StLength => DataType::Double,
            GeographyFunction::StPerimeter => DataType::Double,
            GeographyFunction::StNPoints => DataType::Int,
            GeographyFunction::StStartPoint => DataType::Geography,
            GeographyFunction::StEndPoint => DataType::Geography,
            GeographyFunction::StIsRing => DataType::Bool,
            GeographyFunction::StIsClosed => DataType::Bool,
            GeographyFunction::StGeometryType => DataType::String,
            GeographyFunction::StContains => DataType::Bool,
            GeographyFunction::StWithin => DataType::Bool,
            GeographyFunction::StEnvelope => DataType::Geography,
            GeographyFunction::StBuffer => DataType::Geography,
            GeographyFunction::StBoundary => DataType::Geography,
            GeographyFunction::StCrosses => DataType::Bool,
            GeographyFunction::StTouches => DataType::Bool,
            GeographyFunction::StOverlaps => DataType::Bool,
            GeographyFunction::StEquals => DataType::Bool,
            GeographyFunction::StAsGeoJson => DataType::String,
            GeographyFunction::StGeomFromGeoJson => DataType::Geography,
        },
        BuiltinFunction::Utility(u) => match u {
            UtilityFunction::Coalesce => DataType::Unknown,
            UtilityFunction::Hash => DataType::BigInt,
            UtilityFunction::JsonExtract => DataType::String,
            UtilityFunction::JsonBuildObject => DataType::String,
            UtilityFunction::JsonBuildArray => DataType::String,
            UtilityFunction::JsonObjectKeys => DataType::List(Box::new(DataType::String)),
            UtilityFunction::NullIf => DataType::Unknown,
            UtilityFunction::Greatest => DataType::Unknown,
            UtilityFunction::Least => DataType::Unknown,
            UtilityFunction::GenRandomUuid => DataType::Uuid,
            UtilityFunction::JsonEach => DataType::List(Box::new(DataType::Empty)),
            UtilityFunction::JsonTypeOf => DataType::String,
            UtilityFunction::JsonStripNulls => DataType::String,
            UtilityFunction::IfNull => DataType::Unknown,
            UtilityFunction::TypeOf => DataType::String,
            UtilityFunction::Version => DataType::String,
            UtilityFunction::CurrentUser => DataType::String,
            UtilityFunction::CurrentDatabase => DataType::String,
            UtilityFunction::Corr => DataType::Double,
            UtilityFunction::CovarPop => DataType::Double,
            UtilityFunction::CovarSamp => DataType::Double,
            UtilityFunction::OctetLength => DataType::BigInt,
            UtilityFunction::Encode => DataType::Blob,
            UtilityFunction::Decode => DataType::String,
            UtilityFunction::UnionValue => DataType::Map(Box::new(DataType::Empty)),
            UtilityFunction::UnionTag => DataType::Int,
            UtilityFunction::UnionExtract => DataType::Unknown,
        },
        BuiltinFunction::Graph(g) => match g {
            GraphFunction::Id => DataType::BigInt,
            GraphFunction::Tags => DataType::List(Box::new(DataType::String)),
            GraphFunction::Labels => DataType::List(Box::new(DataType::String)),
            GraphFunction::Properties => DataType::Map(Box::new(DataType::Empty)),
            GraphFunction::EdgeType => DataType::String,
            GraphFunction::Src => DataType::BigInt,
            GraphFunction::Dst => DataType::BigInt,
            GraphFunction::Rank => DataType::BigInt,
            GraphFunction::StartNode => DataType::Vertex,
            GraphFunction::EndNode => DataType::Vertex,
            GraphFunction::Neighbors => DataType::List(Box::new(DataType::BigInt)),
            GraphFunction::Degree => DataType::BigInt,
            GraphFunction::OutEdges => DataType::List(Box::new(DataType::Edge)),
            GraphFunction::InEdges => DataType::List(Box::new(DataType::Edge)),
            GraphFunction::ShortestPath => DataType::BigInt,
            GraphFunction::Bfs => DataType::List(Box::new(DataType::BigInt)),
            GraphFunction::ConnectedComponents => {
                DataType::List(Box::new(DataType::List(Box::new(DataType::BigInt))))
            }
            GraphFunction::VariableLengthPath => {
                DataType::List(Box::new(DataType::List(Box::new(DataType::BigInt))))
            }
            GraphFunction::PageRank => DataType::Map(Box::new(DataType::Double)),
        },
        BuiltinFunction::Container(c) => match c {
            ContainerFunction::Head => DataType::Unknown,
            ContainerFunction::Last => DataType::Unknown,
            ContainerFunction::Tail => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::Size => DataType::BigInt,
            ContainerFunction::Range => DataType::List(Box::new(DataType::BigInt)),
            ContainerFunction::Keys => DataType::List(Box::new(DataType::String)),
            ContainerFunction::ReverseList => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ToSet => DataType::Set(Box::new(DataType::Empty)),
            ContainerFunction::ListContains => DataType::Bool,
            ContainerFunction::ListAppend => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListPrepend => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListFilter => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListTransform => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListConcat => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListSort => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListSlice => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListToString => DataType::String,
            ContainerFunction::ListDistinct => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListUnique => DataType::List(Box::new(DataType::Empty)),
            ContainerFunction::ListExtract => DataType::Unknown,
            ContainerFunction::StructPack => DataType::Map(Box::new(DataType::Empty)),
            ContainerFunction::StructExtract => DataType::Unknown,
            ContainerFunction::MapCreation => DataType::Map(Box::new(DataType::Empty)),
            ContainerFunction::MapExtract => DataType::Unknown,
            ContainerFunction::ElementAt => DataType::Unknown,
            ContainerFunction::Cardinality => DataType::BigInt,
            ContainerFunction::MapKeys => DataType::List(Box::new(DataType::String)),
            ContainerFunction::MapValues => DataType::List(Box::new(DataType::Empty)),
        },
        BuiltinFunction::Path(p) => match p {
            PathFunction::Nodes => DataType::List(Box::new(DataType::Vertex)),
            PathFunction::Relationships => DataType::List(Box::new(DataType::Edge)),
            PathFunction::Properties => DataType::List(Box::new(DataType::Empty)),
            PathFunction::IsTrail => DataType::Bool,
            PathFunction::IsAcyclic => DataType::Bool,
            PathFunction::PathLength => DataType::BigInt,
        },
        BuiltinFunction::Vector(v) => match v {
            VectorFunction::CosineSimilarity => DataType::Double,
            VectorFunction::DotProduct => DataType::Double,
            VectorFunction::EuclideanDistance => DataType::Double,
            VectorFunction::ManhattanDistance => DataType::Double,
            VectorFunction::Dimension => DataType::BigInt,
            VectorFunction::L2Norm => DataType::Double,
            VectorFunction::Nnz => DataType::BigInt,
            VectorFunction::Normalize => DataType::Vector,
            VectorFunction::ArrayValue => DataType::List(Box::new(DataType::Empty)),
            VectorFunction::ArrayCosineSimilarity => DataType::Double,
            VectorFunction::ArrayDistance => DataType::Double,
            VectorFunction::ArraySquaredDistance => DataType::Double,
            VectorFunction::ArrayInnerProduct => DataType::Double,
            VectorFunction::ArrayDotProduct => DataType::Double,
        },
        BuiltinFunction::Sequence(s) => match s {
            SequenceFunction::CurrVal => DataType::BigInt,
            SequenceFunction::NextVal => DataType::BigInt,
        },
        BuiltinFunction::Fulltext(f) => match f {
            FulltextFunction::Score => DataType::Double,
            FulltextFunction::Highlight => DataType::String,
            FulltextFunction::MatchedFields => DataType::List(Box::new(DataType::String)),
            FulltextFunction::Snippet => DataType::String,
            FulltextFunction::Rank => DataType::BigInt,
            FulltextFunction::SearchMatch => DataType::Bool,
            FulltextFunction::FieldScore => DataType::Double,
        },
        BuiltinFunction::Window(w) => match w {
            WindowFunction::RowNumber => DataType::BigInt,
            WindowFunction::Rank => DataType::BigInt,
            WindowFunction::DenseRank => DataType::BigInt,
            WindowFunction::Lead => DataType::Unknown,
            WindowFunction::Lag => DataType::Unknown,
            WindowFunction::FirstValue => DataType::Unknown,
            WindowFunction::LastValue => DataType::Unknown,
            WindowFunction::NthValue => DataType::Unknown,
            WindowFunction::Ntile => DataType::BigInt,
        },
        BuiltinFunction::Aggregate(a) => match a {
            AggregateFunction::Count => DataType::BigInt,
            AggregateFunction::Sum => DataType::Double,
            AggregateFunction::Avg => DataType::Double,
            AggregateFunction::Min => DataType::Unknown,
            AggregateFunction::Max => DataType::Unknown,
            AggregateFunction::Collect => DataType::List(Box::new(DataType::Unknown)),
            AggregateFunction::CollectSet => DataType::Set(Box::new(DataType::Unknown)),
            AggregateFunction::Variance => DataType::Double,
            AggregateFunction::Median => DataType::Double,
            AggregateFunction::Mode => DataType::Unknown,
            AggregateFunction::BoolAnd => DataType::Bool,
            AggregateFunction::BoolOr => DataType::Bool,
            AggregateFunction::StddevPop => DataType::Double,
            AggregateFunction::StddevSamp => DataType::Double,
            AggregateFunction::Product => DataType::Double,
            AggregateFunction::PercentileCont => DataType::Double,
            AggregateFunction::GroupConcatWithOrder => DataType::String,
            // Fallback for other variants
            _ => DataType::Unknown,
        },
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
