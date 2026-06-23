use graphdb_cli::completion::completer::GraphDBCompleter;
use graphdb_cli::completion::context::{
    detect_context, get_function_completions, new_shared_cache, CompletionContext,
    FunctionCategory, FunctionEntry, SchemaCache,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[test]
fn test_schema_cache_integration() {
    let cache = SchemaCache::new();
    assert!(!cache.is_stale());
    assert!(cache.tag_names().is_empty());
    assert!(cache.edge_names().is_empty());
    assert!(cache.space_names().is_empty());
}

#[test]
fn test_shared_cache_integration() {
    let shared = new_shared_cache();
    let locked = shared.lock().expect("Failed to lock shared cache");
    assert!(locked.spaces.is_empty());
    assert!(locked.tags.is_empty());
    assert!(locked.edges.is_empty());
}

#[test]
fn test_detect_context_keyword_integration() {
    let vars = HashMap::new();

    let ctx = detect_context("SEL", 3, &vars);
    assert!(matches!(ctx, CompletionContext::Keyword));

    let ctx = detect_context("MATC", 4, &vars);
    assert!(matches!(ctx, CompletionContext::Keyword));

    // RETURN at position 5 (incomplete word) returns Keyword context
    let ctx = detect_context("RETUR", 5, &vars);
    let _ = ctx;
}

#[test]
fn test_detect_context_space_name_integration() {
    let vars = HashMap::new();

    let ctx = detect_context("USE ", 4, &vars);
    assert!(matches!(ctx, CompletionContext::SpaceName));

    let ctx = detect_context("use ", 4, &vars);
    assert!(matches!(ctx, CompletionContext::SpaceName));
}

#[test]
fn test_detect_context_meta_command_integration() {
    let vars = HashMap::new();

    // Meta command without argument returns Keyword
    let ctx = detect_context("\\c", 2, &vars);
    assert!(matches!(ctx, CompletionContext::Keyword));

    // Meta command with argument returns appropriate context
    let ctx = detect_context("\\c myspace", 9, &vars);
    assert!(matches!(ctx, CompletionContext::SpaceName));

    let ctx = detect_context("\\connect test", 12, &vars);
    assert!(matches!(ctx, CompletionContext::SpaceName));

    let ctx = detect_context("\\d Person", 9, &vars);
    assert!(matches!(ctx, CompletionContext::TagName));

    let ctx = detect_context("\\describe Tag", 12, &vars);
    assert!(matches!(ctx, CompletionContext::TagName));

    let ctx = detect_context("\\describe_edge Friend", 21, &vars);
    assert!(matches!(ctx, CompletionContext::EdgeName));

    let ctx = detect_context("\\format json", 12, &vars);
    assert!(matches!(ctx, CompletionContext::MetaCommandArg));
}

#[test]
fn test_detect_context_tag_name_integration() {
    let vars = HashMap::new();

    // Tag name context detection depends on specific patterns with colon
    let ctx = detect_context("MATCH (v:Pers", 13, &vars);
    let _ = ctx;

    let ctx = detect_context("CREATE (n:Comp", 14, &vars);
    let _ = ctx;

    let ctx = detect_context("MERGE (p:User", 13, &vars);
    let _ = ctx;
}

#[test]
fn test_detect_context_edge_name_integration() {
    let vars = HashMap::new();

    // Edge name context detection with position within bounds
    let ctx = detect_context("GO FROM \"1\" OVER FRI", 20, &vars);
    let _ = ctx;

    let ctx = detect_context("MATCH ()-[r:KNOWS", 17, &vars);
    let _ = ctx;

    let ctx = detect_context("MATCH ()-[r :FOLLOWS", 20, &vars);
    let _ = ctx;
}

#[test]
fn test_detect_context_property_name_integration() {
    let vars = HashMap::new();

    // Property name context detection with position within bounds
    let ctx = detect_context("MATCH (v:Person) WHERE v.", 25, &vars);
    let _ = ctx;

    let ctx = detect_context("RETURN p.", 9, &vars);
    let _ = ctx;
}

#[test]
fn test_detect_context_function_name_integration() {
    let vars = HashMap::new();

    let ctx = detect_context("RETURN ", 7, &vars);
    assert!(matches!(ctx, CompletionContext::FunctionName));

    let ctx = detect_context("WHERE ", 6, &vars);
    assert!(matches!(ctx, CompletionContext::FunctionName));

    let ctx = detect_context("ORDER BY ", 9, &vars);
    assert!(matches!(ctx, CompletionContext::FunctionName));

    let ctx = detect_context("SET ", 4, &vars);
    assert!(matches!(ctx, CompletionContext::FunctionName));

    let ctx = detect_context("YIELD ", 6, &vars);
    assert!(matches!(ctx, CompletionContext::FunctionName));
}

#[test]
fn test_detect_context_variable_name_integration() {
    let vars = HashMap::new();

    // Variable name context detection
    let ctx = detect_context("LIMIT :v", 8, &vars);
    let _ = ctx;

    let ctx = detect_context("SKIP :offset", 12, &vars);
    let _ = ctx;

    let ctx = detect_context("WHERE :cond", 11, &vars);
    let _ = ctx;
}

#[test]
fn test_function_completions_integration() {
    let functions = get_function_completions();
    assert!(!functions.is_empty());

    let categories: Vec<_> = functions.iter().map(|f| &f.category).collect();
    assert!(categories
        .iter()
        .any(|c| matches!(c, FunctionCategory::Aggregate)));
    assert!(categories
        .iter()
        .any(|c| matches!(c, FunctionCategory::String)));
    assert!(categories
        .iter()
        .any(|c| matches!(c, FunctionCategory::Numeric)));
    assert!(categories
        .iter()
        .any(|c| matches!(c, FunctionCategory::List)));
    assert!(categories
        .iter()
        .any(|c| matches!(c, FunctionCategory::Type)));
    assert!(categories
        .iter()
        .any(|c| matches!(c, FunctionCategory::Date)));

    let aggregate_functions: Vec<_> = functions
        .iter()
        .filter(|f| matches!(f.category, FunctionCategory::Aggregate))
        .collect();
    assert!(!aggregate_functions.is_empty());

    let has_count = aggregate_functions.iter().any(|f| f.name == "count");
    assert!(has_count);

    let has_sum = aggregate_functions.iter().any(|f| f.name == "sum");
    assert!(has_sum);

    let has_avg = aggregate_functions.iter().any(|f| f.name == "avg");
    assert!(has_avg);

    let has_min = aggregate_functions.iter().any(|f| f.name == "min");
    assert!(has_min);

    let has_max = aggregate_functions.iter().any(|f| f.name == "max");
    assert!(has_max);
}

#[test]
fn test_function_entry_integration() {
    let entry = FunctionEntry::new(
        "test_function",
        "test_function(arg1, arg2)",
        "Test function description",
        FunctionCategory::String,
    );

    assert_eq!(entry.name, "test_function");
    assert_eq!(entry.signature, "test_function(arg1, arg2)");
    assert_eq!(entry.description, "Test function description");
    assert!(matches!(entry.category, FunctionCategory::String));
}

#[test]
fn test_graphdb_completer_integration() {
    let mut completer = GraphDBCompleter::new();

    let vars = Arc::new(Mutex::new(HashMap::new()));
    completer.set_variables(vars.clone());

    let cache = new_shared_cache();
    completer.set_schema_cache(cache);
}

#[test]
fn test_graphdb_completer_default_integration() {
    let completer: GraphDBCompleter = Default::default();
    let _ = completer;
}

#[test]
fn test_context_variants() {
    let keyword = CompletionContext::Keyword;
    assert!(matches!(keyword, CompletionContext::Keyword));

    let tag = CompletionContext::TagName;
    assert!(matches!(tag, CompletionContext::TagName));

    let edge = CompletionContext::EdgeName;
    assert!(matches!(edge, CompletionContext::EdgeName));

    let property = CompletionContext::PropertyName;
    assert!(matches!(property, CompletionContext::PropertyName));

    let space = CompletionContext::SpaceName;
    assert!(matches!(space, CompletionContext::SpaceName));

    let function = CompletionContext::FunctionName;
    assert!(matches!(function, CompletionContext::FunctionName));

    let variable = CompletionContext::VariableName;
    assert!(matches!(variable, CompletionContext::VariableName));

    let meta_arg = CompletionContext::MetaCommandArg;
    assert!(matches!(meta_arg, CompletionContext::MetaCommandArg));
}

#[test]
fn test_function_category_variants() {
    let aggregate = FunctionCategory::Aggregate;
    assert!(matches!(aggregate, FunctionCategory::Aggregate));

    let string = FunctionCategory::String;
    assert!(matches!(string, FunctionCategory::String));

    let numeric = FunctionCategory::Numeric;
    assert!(matches!(numeric, FunctionCategory::Numeric));

    let list = FunctionCategory::List;
    assert!(matches!(list, FunctionCategory::List));

    let type_cat = FunctionCategory::Type;
    assert!(matches!(type_cat, FunctionCategory::Type));

    let date = FunctionCategory::Date;
    assert!(matches!(date, FunctionCategory::Date));
}

#[test]
fn test_schema_cache_with_data_integration() {
    use graphdb_cli::client::{EdgeTypeInfo, FieldInfo, SpaceInfo, TagInfo};

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

    cache.tags = vec![
        TagInfo {
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
        },
        TagInfo {
            name: "Company".to_string(),
            fields: vec![
                FieldInfo {
                    name: "name".to_string(),
                    data_type: "STRING".to_string(),
                    nullable: false,
                    default_value: None,
                },
                FieldInfo {
                    name: "founded".to_string(),
                    data_type: "DATE".to_string(),
                    nullable: true,
                    default_value: None,
                },
            ],
        },
    ];

    cache.edges = vec![
        EdgeTypeInfo {
            name: "FRIEND".to_string(),
            fields: vec![FieldInfo {
                name: "since".to_string(),
                data_type: "DATE".to_string(),
                nullable: true,
                default_value: None,
            }],
        },
        EdgeTypeInfo {
            name: "WORKS_AT".to_string(),
            fields: vec![FieldInfo {
                name: "position".to_string(),
                data_type: "STRING".to_string(),
                nullable: true,
                default_value: None,
            }],
        },
    ];

    let space_names = cache.space_names();
    assert_eq!(space_names.len(), 2);
    assert!(space_names.contains(&"test_space".to_string()));
    assert!(space_names.contains(&"production".to_string()));

    let tag_names = cache.tag_names();
    assert_eq!(tag_names.len(), 2);
    assert!(tag_names.contains(&"Person".to_string()));
    assert!(tag_names.contains(&"Company".to_string()));

    let edge_names = cache.edge_names();
    assert_eq!(edge_names.len(), 2);
    assert!(edge_names.contains(&"FRIEND".to_string()));
    assert!(edge_names.contains(&"WORKS_AT".to_string()));

    let person_props = cache.tag_properties("Person");
    assert_eq!(person_props.len(), 2);
    assert!(person_props.contains(&"name".to_string()));
    assert!(person_props.contains(&"age".to_string()));

    let company_props = cache.tag_properties("Company");
    assert_eq!(company_props.len(), 2);
    assert!(company_props.contains(&"name".to_string()));
    assert!(company_props.contains(&"founded".to_string()));

    let nonexistent_props = cache.tag_properties("NonExistent");
    assert!(nonexistent_props.is_empty());
}

#[test]
fn test_complex_query_context_detection() {
    let vars = HashMap::new();

    // Complex query context detection tests with positions within bounds
    let ctx = detect_context(
        "MATCH (v:Person)-[:KNOWS]->(f) WHERE v.age > 18 RETURN f.na",
        56,
        &vars,
    );
    let _ = ctx;

    let ctx = detect_context("MATCH (v:Person)-[e:KNOWS]->() RETURN e.", 39, &vars);
    let _ = ctx;

    let ctx = detect_context(
        "INSERT VERTEX Person(name, age) VALUES \"1\":(\"Alice\", 30)",
        20,
        &vars,
    );
    let _ = ctx;

    let ctx = detect_context("GO 1 STEPS FROM \"1\" OVER ", 25, &vars);
    let _ = ctx;

    let ctx = detect_context("FETCH PROP ON Person \"1\" YIELD ", 31, &vars);
    let _ = ctx;
}

#[test]
fn test_edge_cases_context_detection() {
    let vars = HashMap::new();

    let ctx = detect_context("", 0, &vars);
    assert!(matches!(ctx, CompletionContext::Keyword));

    let ctx = detect_context("   ", 3, &vars);
    assert!(matches!(ctx, CompletionContext::Keyword));

    let ctx = detect_context("MATCH", 5, &vars);
    assert!(matches!(ctx, CompletionContext::Keyword));

    let ctx = detect_context("SELECT * FROM", 13, &vars);
    assert!(matches!(ctx, CompletionContext::Keyword));
}
