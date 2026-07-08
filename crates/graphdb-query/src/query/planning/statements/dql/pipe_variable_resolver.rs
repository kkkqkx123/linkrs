//! Pipe Variable Resolver
//!
//! Handles resolution of pipe variable references (`$-`) in chained queries.
//!
//! ## Features
//!
//! - Resolve `$-` to vertex IDs from previous query results
//! - Resolve named variables `$var_name`
//! - Extract vertex IDs from various value types (List, Vertex, Edge)
//! - Variable schema tracking for planning

use std::collections::HashMap;

use crate::core::Value;

pub type ResolverError = String;

#[derive(Debug, Clone)]
pub struct VariableSchema {
    pub name: String,
    pub columns: Vec<ColumnSchema>,
    pub row_count: Option<usize>,
}

impl VariableSchema {
    pub fn new(name: String, columns: Vec<ColumnSchema>) -> Self {
        Self {
            name,
            columns,
            row_count: None,
        }
    }

    pub fn with_row_count(mut self, count: usize) -> Self {
        self.row_count = Some(count);
        self
    }

    pub fn find_column(&self, name: &str) -> Option<&ColumnSchema> {
        self.columns.iter().find(|c| c.name == name)
    }

    pub fn has_vid_column(&self) -> bool {
        self.columns.iter().any(|c| {
            c.name == "vid"
                || c.name == "id"
                || c.name == "dst"
                || c.name == "src"
                || c.is_vid_type()
        })
    }
}

#[derive(Debug, Clone)]
pub struct ColumnSchema {
    pub name: String,
    pub data_type: ColumnDataType,
    pub nullable: bool,
}

impl ColumnSchema {
    pub fn new(name: String, data_type: ColumnDataType) -> Self {
        Self {
            name,
            data_type,
            nullable: true,
        }
    }

    pub fn with_nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    pub fn is_vid_type(&self) -> bool {
        matches!(self.data_type, ColumnDataType::Vid | ColumnDataType::Vertex)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnDataType {
    Vid,
    Vertex,
    Edge,
    Int,
    Float,
    String,
    Bool,
    List,
    Map,
    Unknown,
}

impl ColumnDataType {
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_) => ColumnDataType::Int,
            Value::Float(_) | Value::Double(_) => ColumnDataType::Float,
            Value::String(_) => ColumnDataType::String,
            Value::Bool(_) => ColumnDataType::Bool,
            Value::List(_) => ColumnDataType::List,
            Value::Map(_) => ColumnDataType::Map,
            Value::Null(_) => ColumnDataType::Unknown,
            _ => ColumnDataType::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariableInfo {
    pub name: String,
    pub value: Value,
    pub schema: Option<VariableSchema>,
}

impl VariableInfo {
    pub fn new(name: String, value: Value) -> Self {
        Self {
            name,
            value,
            schema: None,
        }
    }

    pub fn with_schema(mut self, schema: VariableSchema) -> Self {
        self.schema = Some(schema);
        self
    }
}

pub struct PipeVariableResolver {
    variables: HashMap<String, VariableInfo>,
}

impl PipeVariableResolver {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }

    pub fn with_variables(variables: HashMap<String, VariableInfo>) -> Self {
        Self { variables }
    }

    pub fn set_variable(&mut self, name: String, info: VariableInfo) {
        self.variables.insert(name, info);
    }

    pub fn get_variable(&self, name: &str) -> Option<&VariableInfo> {
        self.variables.get(name)
    }

    pub fn has_variable(&self, name: &str) -> bool {
        self.variables.contains_key(name)
    }

    pub fn clear(&mut self) {
        self.variables.clear();
    }

    pub fn resolve(&self, var_name: &str) -> Result<Vec<Value>, ResolverError> {
        let var_info = self
            .variables
            .get(var_name)
            .ok_or_else(|| format!("Undefined variable: {}", var_name))?;

        self.extract_vids_from_value(&var_info.value, var_name)
    }

    fn extract_vids_from_value(
        &self,
        value: &Value,
        var_name: &str,
    ) -> Result<Vec<Value>, ResolverError> {
        match value {
            Value::List(values) => {
                let vids: Vec<Value> = values
                    .iter()
                    .filter_map(|v| self.extract_single_vid(v))
                    .collect();
                if vids.is_empty() {
                    return Err(format!("Empty pipe variable: {}", var_name));
                }
                Ok(vids)
            }
            Value::Map(map) => {
                if let Some(vid) = map.get("vid") {
                    return Ok(vec![vid.clone()]);
                }
                if let Some(vid) = map.get("id") {
                    return Ok(vec![vid.clone()]);
                }
                if let Some(dst) = map.get("dst") {
                    return Ok(vec![dst.clone()]);
                }
                Err(format!(
                    "Variable {} has no vertex ID column (vid, id, or dst)",
                    var_name
                ))
            }
            Value::SmallInt(i) => Ok(vec![Value::SmallInt(*i)]),
            Value::Int(i) => Ok(vec![Value::Int(*i)]),
            Value::BigInt(i) => Ok(vec![Value::BigInt(*i)]),
            Value::String(s) => Ok(vec![Value::String(s.clone())]),
            _ => Err(format!(
                "Invalid pipe variable type for {}: expected List, Map, or numeric type",
                var_name
            )),
        }
    }

    fn extract_single_vid(&self, value: &Value) -> Option<Value> {
        match value {
            Value::Map(map) => map.get("vid").or_else(|| map.get("id")).cloned(),
            Value::SmallInt(i) => Some(Value::SmallInt(*i)),
            Value::Int(i) => Some(Value::Int(*i)),
            Value::BigInt(i) => Some(Value::BigInt(*i)),
            Value::String(s) => Some(Value::String(s.clone())),
            _ => None,
        }
    }

    pub fn resolve_column(
        &self,
        var_name: &str,
        column_name: &str,
    ) -> Result<Vec<Value>, ResolverError> {
        let var_info = self
            .variables
            .get(var_name)
            .ok_or_else(|| format!("Undefined variable: {}", var_name))?;

        match &var_info.value {
            Value::List(values) => {
                let column_values: Vec<Value> = values
                    .iter()
                    .filter_map(|v| {
                        if let Value::Map(map) = v {
                            map.get(column_name).cloned()
                        } else {
                            None
                        }
                    })
                    .collect();
                if column_values.is_empty() {
                    return Err(format!(
                        "Column '{}' not found in variable '{}'",
                        column_name, var_name
                    ));
                }
                Ok(column_values)
            }
            Value::Map(map) => {
                if let Some(value) = map.get(column_name) {
                    Ok(vec![value.clone()])
                } else {
                    Err(format!(
                        "Column '{}' not found in variable '{}'",
                        column_name, var_name
                    ))
                }
            }
            _ => Err(format!(
                "Cannot extract column '{}' from non-structured variable '{}'",
                column_name, var_name
            )),
        }
    }

    pub fn get_variable_schema(&self, var_name: &str) -> Option<VariableSchema> {
        self.variables.get(var_name)?.schema.clone()
    }

    pub fn infer_schema(&self, var_name: &str) -> Option<VariableSchema> {
        let var_info = self.variables.get(var_name)?;
        let columns = self.infer_columns_from_value(&var_info.value);
        Some(
            VariableSchema::new(var_name.to_string(), columns)
                .with_row_count(self.estimate_row_count(&var_info.value)),
        )
    }

    fn infer_columns_from_value(&self, value: &Value) -> Vec<ColumnSchema> {
        match value {
            Value::List(values) => {
                if let Some(Value::Map(map)) = values.values.first() {
                    return map
                        .keys()
                        .map(|k| {
                            let data_type = map
                                .get(k)
                                .map(ColumnDataType::from_value)
                                .unwrap_or(ColumnDataType::Unknown);
                            ColumnSchema::new(k.clone(), data_type)
                        })
                        .collect();
                }
                vec![]
            }
            Value::Map(map) => map
                .keys()
                .map(|k| {
                    let data_type = map
                        .get(k)
                        .map(ColumnDataType::from_value)
                        .unwrap_or(ColumnDataType::Unknown);
                    ColumnSchema::new(k.clone(), data_type)
                })
                .collect(),
            _ => vec![],
        }
    }

    fn estimate_row_count(&self, value: &Value) -> usize {
        match value {
            Value::List(values) => values.len(),
            Value::Map(_) => 1,
            _ => 1,
        }
    }
}

impl Default for PipeVariableResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub enum FromClausePlan {
    DirectVids(Vec<Value>),
    PipeVariable {
        vids: Vec<Value>,
        var_name: String,
    },
    NamedVariable {
        vids: Vec<Value>,
        var_name: String,
    },
    ColumnReference {
        values: Vec<Value>,
        var_name: String,
        column_name: String,
    },
}

impl FromClausePlan {
    pub fn get_vids(&self) -> &[Value] {
        match self {
            Self::DirectVids(vids) => vids,
            Self::PipeVariable { vids, .. } => vids,
            Self::NamedVariable { vids, .. } => vids,
            Self::ColumnReference { values, .. } => values,
        }
    }

    pub fn is_from_pipe(&self) -> bool {
        matches!(self, Self::PipeVariable { .. })
    }

    pub fn is_from_variable(&self) -> bool {
        matches!(
            self,
            Self::PipeVariable { .. } | Self::NamedVariable { .. } | Self::ColumnReference { .. }
        )
    }
}

pub fn parse_pipe_variable(expr_str: &str) -> Option<ParsedPipeVariable> {
    let trimmed = expr_str.trim();

    if trimmed == "$-" {
        return Some(ParsedPipeVariable::PipeResult);
    }

    if let Some(rest) = trimmed.strip_prefix('$') {
        if rest.is_empty() {
            return None;
        }

        if let Some(dot_pos) = rest.find('.') {
            let var_name = &rest[..dot_pos];
            let column_name = &rest[dot_pos + 1..];
            if !var_name.is_empty() && !column_name.is_empty() {
                return Some(ParsedPipeVariable::NamedColumn {
                    var_name: var_name.to_string(),
                    column_name: column_name.to_string(),
                });
            }
        } else {
            return Some(ParsedPipeVariable::NamedVariable(rest.to_string()));
        }
    }

    None
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedPipeVariable {
    PipeResult,
    NamedVariable(String),
    NamedColumn {
        var_name: String,
        column_name: String,
    },
}

impl ParsedPipeVariable {
    pub fn var_name(&self) -> &str {
        match self {
            Self::PipeResult => "-",
            Self::NamedVariable(name) => name,
            Self::NamedColumn { var_name, .. } => var_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::value::list::List;
    use std::collections::HashMap;

    fn make_list(values: Vec<Value>) -> Value {
        Value::List(Box::new(List { values }))
    }

    fn make_map(entries: Vec<(&str, Value)>) -> Value {
        let map: HashMap<String, Value> = entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        Value::Map(Box::new(map))
    }

    #[test]
    fn test_resolve_list_of_maps() {
        let mut resolver = PipeVariableResolver::new();

        let value = make_list(vec![
            make_map(vec![
                ("vid", Value::Int(1)),
                ("name", Value::String("Alice".to_string())),
            ]),
            make_map(vec![
                ("vid", Value::Int(2)),
                ("name", Value::String("Bob".to_string())),
            ]),
        ]);

        resolver.set_variable("-".to_string(), VariableInfo::new("-".to_string(), value));

        let vids = resolver.resolve("-").unwrap();
        assert_eq!(vids.len(), 2);
        assert_eq!(vids[0], Value::Int(1));
        assert_eq!(vids[1], Value::Int(2));
    }

    #[test]
    fn test_resolve_single_int() {
        let mut resolver = PipeVariableResolver::new();
        resolver.set_variable(
            "-".to_string(),
            VariableInfo::new("-".to_string(), Value::Int(42)),
        );

        let vids = resolver.resolve("-").unwrap();
        assert_eq!(vids.len(), 1);
        assert_eq!(vids[0], Value::Int(42));
    }

    #[test]
    fn test_resolve_undefined_variable() {
        let resolver = PipeVariableResolver::new();
        let result = resolver.resolve("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_column() {
        let mut resolver = PipeVariableResolver::new();

        let value = make_list(vec![
            make_map(vec![
                ("vid", Value::Int(1)),
                ("name", Value::String("Alice".to_string())),
            ]),
            make_map(vec![
                ("vid", Value::Int(2)),
                ("name", Value::String("Bob".to_string())),
            ]),
        ]);

        resolver.set_variable(
            "result".to_string(),
            VariableInfo::new("result".to_string(), value),
        );

        let names = resolver.resolve_column("result", "name").unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], Value::String("Alice".to_string()));
        assert_eq!(names[1], Value::String("Bob".to_string()));
    }

    #[test]
    fn test_parse_pipe_variable() {
        assert_eq!(
            parse_pipe_variable("$-"),
            Some(ParsedPipeVariable::PipeResult)
        );

        assert_eq!(
            parse_pipe_variable("$myvar"),
            Some(ParsedPipeVariable::NamedVariable("myvar".to_string()))
        );

        assert_eq!(
            parse_pipe_variable("$result.vid"),
            Some(ParsedPipeVariable::NamedColumn {
                var_name: "result".to_string(),
                column_name: "vid".to_string(),
            })
        );

        assert_eq!(parse_pipe_variable("not_a_pipe"), None);
    }

    #[test]
    fn test_variable_schema() {
        let schema = VariableSchema::new(
            "result".to_string(),
            vec![
                ColumnSchema::new("vid".to_string(), ColumnDataType::Vid),
                ColumnSchema::new("name".to_string(), ColumnDataType::String),
            ],
        )
        .with_row_count(10);

        assert!(schema.has_vid_column());
        assert_eq!(schema.row_count, Some(10));
    }

    #[test]
    fn test_infer_schema() {
        let mut resolver = PipeVariableResolver::new();

        let value = make_list(vec![make_map(vec![
            ("vid", Value::Int(1)),
            ("name", Value::String("Alice".to_string())),
        ])]);

        resolver.set_variable(
            "result".to_string(),
            VariableInfo::new("result".to_string(), value),
        );

        let schema = resolver.infer_schema("result").unwrap();
        assert_eq!(schema.columns.len(), 2);
        assert!(schema.has_vid_column());
    }
}
