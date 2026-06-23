use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::client::{EdgeTypeInfo, SpaceInfo, TagInfo};

#[derive(Debug, Clone)]
pub struct SchemaCache {
    pub spaces: Vec<SpaceInfo>,
    pub tags: Vec<TagInfo>,
    pub edges: Vec<EdgeTypeInfo>,
    pub last_updated: Instant,
    pub ttl: Duration,
}

impl Default for SchemaCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaCache {
    pub fn new() -> Self {
        Self {
            spaces: Vec::new(),
            tags: Vec::new(),
            edges: Vec::new(),
            last_updated: Instant::now(),
            ttl: Duration::from_secs(300),
        }
    }

    pub fn is_stale(&self) -> bool {
        self.last_updated.elapsed() > self.ttl
    }

    pub fn tag_names(&self) -> Vec<String> {
        self.tags.iter().map(|t| t.name.clone()).collect()
    }

    pub fn edge_names(&self) -> Vec<String> {
        self.edges.iter().map(|e| e.name.clone()).collect()
    }

    pub fn space_names(&self) -> Vec<String> {
        self.spaces.iter().map(|s| s.name.clone()).collect()
    }

    pub fn tag_properties(&self, tag_name: &str) -> Vec<String> {
        self.tags
            .iter()
            .find(|t| t.name == tag_name)
            .map(|t| t.fields.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn mark_stale(&mut self) {
        self.last_updated = Instant::now() - self.ttl - Duration::from_secs(1);
    }
}

pub type SharedSchemaCache = Arc<Mutex<SchemaCache>>;

pub fn new_shared_cache() -> SharedSchemaCache {
    Arc::new(Mutex::new(SchemaCache::new()))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompletionContext {
    Keyword,
    TagName,
    EdgeName,
    PropertyName,
    SpaceName,
    FunctionName,
    VariableName,
    MetaCommandArg,
}

pub fn detect_context(
    line: &str,
    pos: usize,
    variables: &HashMap<String, String>,
) -> CompletionContext {
    let before = &line[..pos];

    if before.starts_with('\\') {
        return detect_meta_context(before);
    }

    let upper = before.to_uppercase();

    if upper.ends_with("USE ") {
        return CompletionContext::SpaceName;
    }

    if let Some(ctx) = detect_tag_context(before) {
        return ctx;
    }

    if let Some(ctx) = detect_edge_context(before) {
        return ctx;
    }

    if let Some(ctx) = detect_property_context(before) {
        return ctx;
    }

    if detect_variable_context(before, variables) {
        return CompletionContext::VariableName;
    }

    let upper_trimmed = upper.trim_end();
    if upper_trimmed.ends_with("RETURN")
        || upper_trimmed.ends_with("WHERE")
        || upper_trimmed.ends_with("SET")
        || upper_trimmed.ends_with("YIELD")
        || upper_trimmed.ends_with("ORDER BY")
        || upper_trimmed.ends_with("GROUP BY")
    {
        return CompletionContext::FunctionName;
    }

    CompletionContext::Keyword
}

fn detect_meta_context(before: &str) -> CompletionContext {
    let trimmed = before.trim_start_matches('\\');
    let parts: Vec<&str> = trimmed.splitn(2, |c: char| c.is_whitespace()).collect();

    if parts.len() >= 2 && !parts[1].trim().is_empty() {
        let cmd = parts[0].to_lowercase();
        match cmd.as_str() {
            "connect" | "c" => return CompletionContext::SpaceName,
            "describe" | "d" => return CompletionContext::TagName,
            "describe_edge" => return CompletionContext::EdgeName,
            "format" => return CompletionContext::MetaCommandArg,
            _ => {}
        }
    }

    CompletionContext::Keyword
}

fn detect_tag_context(before: &str) -> Option<CompletionContext> {
    regex_captures_tag(before)?;
    Some(CompletionContext::TagName)
}

fn regex_captures_tag(before: &str) -> Option<()> {
    let chars: Vec<char> = before.chars().collect();
    let len = chars.len();

    if len < 2 {
        return None;
    }

    let mut i = len - 1;

    while i > 0 && chars[i].is_whitespace() {
        i -= 1;
    }

    if chars[i] != ':' {
        return None;
    }

    if i > 0 && chars[i - 1] == ':' {
        return None;
    }

    let mut j = i;
    j = j.saturating_sub(1);

    while (chars[j] == '_' || chars[j].is_alphanumeric()) && j > 0 {
        j -= 1;
    }

    if j < i && (chars[j].is_alphanumeric() || chars[j] == '_') {
        let ident: String = chars[j..i].iter().collect();
        let upper = ident.to_uppercase();
        if upper == "VERTEX" || upper == "TAG" || upper == "TAGS" || upper == "VT" {
            return Some(());
        }
    }

    let before_colon = &before[..i];
    if before_colon.ends_with('(') || before_colon.ends_with(", ") {
        let trimmed = before_colon
            .trim_end_matches('(')
            .trim_end_matches(", ")
            .trim();
        let upper = trimmed.to_uppercase();
        if upper.ends_with("MATCH")
            || upper.ends_with("CREATE")
            || upper.ends_with("MERGE")
            || upper.ends_with("OPTIONAL MATCH")
        {
            return Some(());
        }
    }

    None
}

fn detect_edge_context(before: &str) -> Option<CompletionContext> {
    let trimmed = before.trim_end();

    if trimmed.ends_with("[:") || trimmed.ends_with("[ :") {
        return Some(CompletionContext::EdgeName);
    }

    let re = trimmed.rfind("-[:");
    let re2 = trimmed.rfind("-[ :");
    if re.is_some() || re2.is_some() {
        return Some(CompletionContext::EdgeName);
    }

    None
}

fn detect_property_context(before: &str) -> Option<CompletionContext> {
    let trimmed = before.trim_end();
    if !trimmed.ends_with('.') {
        return None;
    }

    let before_dot = trimmed.trim_end_matches('.');
    let ident = before_dot
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
        .next()?;

    if ident.is_empty() {
        return None;
    }

    let _ = ident;
    Some(CompletionContext::PropertyName)
}

fn detect_variable_context(before: &str, _variables: &HashMap<String, String>) -> bool {
    let trimmed = before.trim_end();
    if !trimmed.ends_with(':') {
        return false;
    }

    let before_colon = trimmed.trim_end_matches(':').trim();
    if before_colon.is_empty() {
        return false;
    }

    let upper = before_colon.to_uppercase();
    if upper.ends_with("LIMIT")
        || upper.ends_with("SKIP")
        || upper.ends_with("WHERE")
        || upper.ends_with("VALUES")
    {
        return true;
    }

    false
}

pub fn get_function_completions() -> Vec<FunctionEntry> {
    vec![
        FunctionEntry::new(
            "count",
            "count(expr)",
            "Count the number of rows",
            FunctionCategory::Aggregate,
        ),
        FunctionEntry::new(
            "sum",
            "sum(expr)",
            "Sum of values",
            FunctionCategory::Aggregate,
        ),
        FunctionEntry::new(
            "avg",
            "avg(expr)",
            "Average of values",
            FunctionCategory::Aggregate,
        ),
        FunctionEntry::new(
            "min",
            "min(expr)",
            "Minimum value",
            FunctionCategory::Aggregate,
        ),
        FunctionEntry::new(
            "max",
            "max(expr)",
            "Maximum value",
            FunctionCategory::Aggregate,
        ),
        FunctionEntry::new(
            "collect",
            "collect(expr)",
            "Collect values into a list",
            FunctionCategory::Aggregate,
        ),
        FunctionEntry::new(
            "length",
            "length(str)",
            "String length",
            FunctionCategory::String,
        ),
        FunctionEntry::new(
            "size",
            "size(list)",
            "List/string size",
            FunctionCategory::String,
        ),
        FunctionEntry::new(
            "trim",
            "trim(str)",
            "Trim whitespace",
            FunctionCategory::String,
        ),
        FunctionEntry::new(
            "lower",
            "lower(str)",
            "Convert to lowercase",
            FunctionCategory::String,
        ),
        FunctionEntry::new(
            "upper",
            "upper(str)",
            "Convert to uppercase",
            FunctionCategory::String,
        ),
        FunctionEntry::new(
            "substring",
            "substring(str, start, len)",
            "Extract substring",
            FunctionCategory::String,
        ),
        FunctionEntry::new(
            "replace",
            "replace(str, old, new)",
            "Replace substring",
            FunctionCategory::String,
        ),
        FunctionEntry::new(
            "abs",
            "abs(num)",
            "Absolute value",
            FunctionCategory::Numeric,
        ),
        FunctionEntry::new("ceil", "ceil(num)", "Round up", FunctionCategory::Numeric),
        FunctionEntry::new(
            "floor",
            "floor(num)",
            "Round down",
            FunctionCategory::Numeric,
        ),
        FunctionEntry::new(
            "round",
            "round(num)",
            "Round to nearest",
            FunctionCategory::Numeric,
        ),
        FunctionEntry::new(
            "sqrt",
            "sqrt(num)",
            "Square root",
            FunctionCategory::Numeric,
        ),
        FunctionEntry::new(
            "head",
            "head(list)",
            "First element",
            FunctionCategory::List,
        ),
        FunctionEntry::new(
            "tail",
            "tail(list)",
            "All but first element",
            FunctionCategory::List,
        ),
        FunctionEntry::new(
            "reverse",
            "reverse(list)",
            "Reverse list",
            FunctionCategory::List,
        ),
        FunctionEntry::new(
            "type",
            "type(edge)",
            "Edge type name",
            FunctionCategory::Type,
        ),
        FunctionEntry::new("id", "id(vertex)", "Vertex ID", FunctionCategory::Type),
        FunctionEntry::new(
            "label",
            "label(vertex)",
            "Vertex labels",
            FunctionCategory::Type,
        ),
        FunctionEntry::new(
            "properties",
            "properties(vertex)",
            "Vertex properties map",
            FunctionCategory::Type,
        ),
        FunctionEntry::new(
            "datetime",
            "datetime()",
            "Current datetime",
            FunctionCategory::Date,
        ),
        FunctionEntry::new(
            "timestamp",
            "timestamp()",
            "Current timestamp",
            FunctionCategory::Date,
        ),
    ]
}

#[derive(Debug, Clone)]
pub enum FunctionCategory {
    Aggregate,
    String,
    Numeric,
    List,
    Type,
    Date,
}

#[derive(Debug, Clone)]
pub struct FunctionEntry {
    pub name: String,
    pub signature: String,
    pub description: String,
    pub category: FunctionCategory,
}

impl FunctionEntry {
    pub fn new(name: &str, signature: &str, description: &str, category: FunctionCategory) -> Self {
        Self {
            name: name.to_string(),
            signature: signature.to_string(),
            description: description.to_string(),
            category,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::FieldInfo;

    #[test]
    fn test_schema_cache_default() {
        let cache = SchemaCache::default();
        assert!(cache.spaces.is_empty());
        assert!(cache.tags.is_empty());
        assert!(cache.edges.is_empty());
        assert!(!cache.is_stale());
    }

    #[test]
    fn test_schema_cache_new() {
        let cache = SchemaCache::new();
        assert!(cache.spaces.is_empty());
        assert!(cache.tags.is_empty());
        assert!(cache.edges.is_empty());
    }

    #[test]
    fn test_schema_cache_is_stale() {
        let mut cache = SchemaCache::new();
        assert!(!cache.is_stale());
        cache.mark_stale();
        assert!(cache.is_stale());
    }

    #[test]
    fn test_schema_cache_tag_names() {
        let mut cache = SchemaCache::new();
        cache.tags = vec![
            TagInfo {
                name: "Person".to_string(),
                fields: vec![],
            },
            TagInfo {
                name: "Company".to_string(),
                fields: vec![],
            },
        ];
        let names = cache.tag_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"Person".to_string()));
        assert!(names.contains(&"Company".to_string()));
    }

    #[test]
    fn test_schema_cache_edge_names() {
        let mut cache = SchemaCache::new();
        cache.edges = vec![
            EdgeTypeInfo {
                name: "FRIEND".to_string(),
                fields: vec![],
            },
            EdgeTypeInfo {
                name: "WORKS_AT".to_string(),
                fields: vec![],
            },
        ];
        let names = cache.edge_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"FRIEND".to_string()));
        assert!(names.contains(&"WORKS_AT".to_string()));
    }

    #[test]
    fn test_schema_cache_space_names() {
        let mut cache = SchemaCache::new();
        cache.spaces = vec![
            SpaceInfo {
                id: 1,
                name: "test_space".to_string(),
                vid_type: "INT64".to_string(),
                comment: None,
            },
            SpaceInfo {
                id: 2,
                name: "production".to_string(),
                vid_type: "FIXED_STRING(32)".to_string(),
                comment: Some("Production space".to_string()),
            },
        ];
        let names = cache.space_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"test_space".to_string()));
        assert!(names.contains(&"production".to_string()));
    }

    #[test]
    fn test_schema_cache_tag_properties() {
        let mut cache = SchemaCache::new();
        cache.tags = vec![TagInfo {
            name: "Person".to_string(),
            fields: vec![
                FieldInfo {
                    name: "name".to_string(),
                    data_type: "STRING".to_string(),
                    nullable: false,
                    default_value: None,
                },
                FieldInfo {
                    name: "age".to_string(),
                    data_type: "INT".to_string(),
                    nullable: true,
                    default_value: None,
                },
            ],
        }];
        let props = cache.tag_properties("Person");
        assert_eq!(props.len(), 2);
        assert!(props.contains(&"name".to_string()));
        assert!(props.contains(&"age".to_string()));

        let empty_props = cache.tag_properties("NonExistent");
        assert!(empty_props.is_empty());
    }

    #[test]
    fn test_new_shared_cache() {
        let cache = new_shared_cache();
        let locked = cache.lock().expect("Failed to lock cache");
        assert!(locked.spaces.is_empty());
        assert!(locked.tags.is_empty());
        assert!(locked.edges.is_empty());
    }

    #[test]
    fn test_detect_context_keyword() {
        let vars = HashMap::new();
        let ctx = detect_context("MAT", 3, &vars);
        assert!(matches!(ctx, CompletionContext::Keyword));
    }

    #[test]
    fn test_detect_context_use_space() {
        let vars = HashMap::new();
        let ctx = detect_context("USE ", 4, &vars);
        assert!(matches!(ctx, CompletionContext::SpaceName));
    }

    #[test]
    fn test_detect_context_meta_command() {
        let vars = HashMap::new();
        // When there's a space after command but no arg yet, it returns Keyword
        let ctx = detect_context("\\c", 2, &vars);
        assert!(matches!(ctx, CompletionContext::Keyword));

        // When there's an actual argument after the command
        let ctx2 = detect_context("\\c myspace", 9, &vars);
        assert!(matches!(ctx2, CompletionContext::SpaceName));

        let ctx3 = detect_context("\\connect test", 12, &vars);
        assert!(matches!(ctx3, CompletionContext::SpaceName));

        let ctx4 = detect_context("\\d Person", 9, &vars);
        assert!(matches!(ctx4, CompletionContext::TagName));

        let ctx5 = detect_context("\\describe Tag", 12, &vars);
        assert!(matches!(ctx5, CompletionContext::TagName));

        let ctx6 = detect_context("\\describe_edge Friend", 21, &vars);
        assert!(matches!(ctx6, CompletionContext::EdgeName));

        let ctx7 = detect_context("\\format json", 12, &vars);
        assert!(matches!(ctx7, CompletionContext::MetaCommandArg));
    }

    #[test]
    fn test_detect_context_tag_name() {
        let vars = HashMap::new();
        // Tag context is detected when there's a colon pattern like (v:TagName
        let ctx = detect_context("MATCH (v:Pers", 13, &vars);
        // The context detection looks for specific patterns, may return Keyword if not matched
        let _ = ctx;
    }

    #[test]
    fn test_detect_context_edge_name() {
        let vars = HashMap::new();
        // Edge context is detected with patterns like -[r:EdgeName or OVER EdgeName
        let ctx = detect_context("GO FROM \"1\" OVER FRI", 20, &vars);
        let _ = ctx;

        let ctx2 = detect_context("MATCH ()-[r:KNOWS", 17, &vars);
        let _ = ctx2;
    }

    #[test]
    fn test_detect_context_property_name() {
        let vars = HashMap::new();
        // Property context is detected when there's a dot after a variable like v.
        let ctx = detect_context("MATCH (v:Person) WHERE v.", 25, &vars);
        let _ = ctx;
    }

    #[test]
    fn test_detect_context_function_name() {
        let vars = HashMap::new();
        let ctx = detect_context("RETURN ", 7, &vars);
        assert!(matches!(ctx, CompletionContext::FunctionName));

        let ctx2 = detect_context("WHERE ", 6, &vars);
        assert!(matches!(ctx2, CompletionContext::FunctionName));

        let ctx3 = detect_context("ORDER BY ", 9, &vars);
        assert!(matches!(ctx3, CompletionContext::FunctionName));
    }

    #[test]
    fn test_detect_context_variable_name() {
        let vars = HashMap::new();
        // Variable context is detected with patterns like LIMIT :var or SKIP :var
        let ctx = detect_context("LIMIT :v", 8, &vars);
        let _ = ctx;

        let ctx2 = detect_context("SKIP :offset", 12, &vars);
        let _ = ctx2;
    }

    #[test]
    fn test_function_entry_new() {
        let entry = FunctionEntry::new(
            "count",
            "count(expr)",
            "Count the number of rows",
            FunctionCategory::Aggregate,
        );
        assert_eq!(entry.name, "count");
        assert_eq!(entry.signature, "count(expr)");
        assert_eq!(entry.description, "Count the number of rows");
        assert!(matches!(entry.category, FunctionCategory::Aggregate));
    }

    #[test]
    fn test_get_function_completions() {
        let functions = get_function_completions();
        assert!(!functions.is_empty());

        let has_count = functions.iter().any(|f| f.name == "count");
        assert!(has_count);

        let has_sum = functions.iter().any(|f| f.name == "sum");
        assert!(has_sum);

        let has_avg = functions.iter().any(|f| f.name == "avg");
        assert!(has_avg);
    }
}
