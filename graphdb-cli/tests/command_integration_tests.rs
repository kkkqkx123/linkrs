use graphdb_cli::command::meta_commands::{show_copyright, show_help, show_version};
use graphdb_cli::command::parser::{parse_command, Command, CopyDirection, HistoryAction};
use graphdb_cli::command::script::{
    is_statement_complete, ConditionExpr, ConditionalStack, ScriptExecutionContext, ScriptParser,
    StatementKind,
};

#[test]
fn test_parse_command_integration_empty_and_whitespace() {
    assert!(matches!(parse_command(""), Command::Empty));
    assert!(matches!(parse_command("   "), Command::Empty));
    assert!(matches!(parse_command("\t\n\r"), Command::Empty));
}

#[test]
fn test_parse_command_integration_queries() {
    let gql_queries = vec![
        "MATCH (v:Person) RETURN v",
        "GO 1 STEPS FROM \"id\" OVER friend",
        "INSERT VERTEX Person(name) VALUES \"1\":(\"Alice\")",
        "CREATE SPACE test_space",
        "SHOW SPACES",
    ];

    for query in gql_queries {
        match parse_command(query) {
            Command::Query(q) => assert_eq!(q, query),
            other => panic!("Expected Query for '{}', got {:?}", query, other),
        }
    }
}

#[test]
fn test_parse_command_integration_meta_commands_variants() {
    let test_cases = vec![
        ("\\q", "Quit"),
        ("\\quit", "Quit"),
        ("\\q!", "ForceQuit"),
        ("\\?", "Help"),
        ("\\help", "Help"),
        ("\\c myspace", "Connect"),
        ("\\connect myspace", "Connect"),
        ("\\disconnect", "Disconnect"),
        ("\\conninfo", "ConnInfo"),
        ("\\l", "ShowSpaces"),
        ("\\show_spaces", "ShowSpaces"),
        ("\\dt", "ShowTags"),
        ("\\show_tags", "ShowTags"),
        ("\\de", "ShowEdges"),
        ("\\show_edges", "ShowEdges"),
        ("\\di", "ShowIndexes"),
        ("\\show_indexes", "ShowIndexes"),
        ("\\du", "ShowUsers"),
        ("\\show_users", "ShowUsers"),
        ("\\df", "ShowFunctions"),
        ("\\show_functions", "ShowFunctions"),
        ("\\d Person", "Describe"),
        ("\\describe Person", "Describe"),
        ("\\describe_edge Friend", "DescribeEdge"),
        ("\\format table", "Format"),
        ("\\format json", "Format"),
        ("\\pager", "Pager"),
        ("\\pager less", "Pager"),
        ("\\timing", "Timing"),
        ("\\set", "ShowVariables"),
        ("\\set VAR", "Set"),
        ("\\set VAR value", "Set"),
        ("\\unset VAR", "Unset"),
        ("\\i script.sql", "ExecuteScript"),
        ("\\ir script.sql", "ExecuteScriptRaw"),
        ("\\o", "OutputRedirect"),
        ("\\o output.txt", "OutputRedirect"),
        ("\\! ls", "ShellCommand"),
        ("\\version", "Version"),
        ("\\copyright", "Copyright"),
        ("\\begin", "Begin"),
        ("\\commit", "Commit"),
        ("\\rollback", "Rollback"),
        ("\\rollback to sp1", "RollbackTo"),
        ("\\autocommit", "Autocommit"),
        ("\\autocommit on", "Autocommit"),
        ("\\isolation", "Isolation"),
        ("\\isolation serializable", "Isolation"),
        ("\\savepoint sp1", "Savepoint"),
        ("\\release sp1", "ReleaseSavepoint"),
        ("\\txstatus", "TxStatus"),
        ("\\e", "Edit"),
        ("\\edit file.sql", "Edit"),
        ("\\p", "PrintBuffer"),
        ("\\r", "ResetBuffer"),
        ("\\w output.sql", "WriteBuffer"),
        ("\\history", "History"),
        ("\\history 50", "History"),
        ("\\history clear", "History"),
        ("\\history search pattern", "History"),
        ("\\history exec 5", "History"),
        ("\\if VAR", "If"),
        ("\\elif VAR", "Elif"),
        ("\\else", "Else"),
        ("\\endif", "EndIf"),
        ("\\explain MATCH (v) RETURN v", "Explain"),
        ("\\profile MATCH (v) RETURN v", "Profile"),
        ("\\x", "Format"),
    ];

    for (input, _expected_type) in test_cases {
        let result = parse_command(input);
        match result {
            Command::MetaCommand(_) => {}
            other => panic!("Expected MetaCommand for '{}', got {:?}", input, other),
        }
    }
}

#[test]
fn test_script_parser_integration_full_script() {
    let script = r#"
-- Create some data
CREATE SPACE test_space;
USE test_space;

-- Create schema
CREATE TAG Person(name string, age int);
CREATE EDGE Friend(degree int);

-- Insert data
INSERT VERTEX Person(name, age) VALUES "1":("Alice", 30);
INSERT VERTEX Person(name, age) VALUES "2":("Bob", 25);
INSERT EDGE Friend(degree) VALUES "1"->"2":(5);

-- Query
MATCH (p:Person) RETURN p;

-- Meta commands
\set output_format table
\timing
"#;

    let statements = ScriptParser::parse(script);
    assert!(!statements.is_empty());

    let query_count = statements
        .iter()
        .filter(|s| matches!(s.kind, StatementKind::Query))
        .count();
    let meta_count = statements
        .iter()
        .filter(|s| matches!(s.kind, StatementKind::MetaCommand))
        .count();

    assert!(query_count > 0, "Should have query statements");
    assert!(meta_count > 0, "Should have meta command statements");
}

#[test]
fn test_script_parser_integration_with_comments() {
    let script = r#"
-- Single line comment
MATCH (v) RETURN v;
// Another single line comment
\set VAR value
/* Multi-line
   comment */
INSERT VERTEX Person(name) VALUES "1":("test");
"#;

    let statements = ScriptParser::parse(script);

    assert!(
        statements.iter().all(|s| !s.content.starts_with("--")),
        "Comments should be filtered out"
    );
}

#[test]
fn test_conditional_stack_integration() {
    let mut stack = ConditionalStack::new();

    assert!(stack.is_active(), "Empty stack should be active");

    stack.push_if(true);
    assert!(stack.is_active(), "True if should be active");

    stack.push_if(false);
    assert!(!stack.is_active(), "Nested false if should be inactive");

    stack.pop();
    assert!(stack.is_active(), "After popping, should be active again");

    stack.push_elif(true);
    assert!(!stack.is_active(), "Elif after true if should be inactive");

    stack.pop();
    stack.push_if(false);
    stack.push_elif(false);
    assert!(!stack.is_active(), "False elif should be inactive");
    stack.push_else();
    assert!(stack.is_active(), "Else after false elif should be active");
}

#[test]
fn test_condition_expr_integration() {
    let mut vars = std::collections::HashMap::new();
    vars.insert("MODE".to_string(), "test".to_string());
    vars.insert("DEBUG".to_string(), "true".to_string());

    let test_cases = vec![
        ("MODE", true),
        ("?MODE", true),
        ("!?MODE", false),
        ("MODE == test", true),
        ("MODE == prod", false),
        ("MODE != prod", true),
        ("MODE != test", false),
        ("MISSING", false),
        ("!?MISSING", true),
    ];

    for (expr_str, expected) in test_cases {
        let expr = ConditionExpr::parse(expr_str).expect("Should parse");
        let result = expr.evaluate(&vars);
        assert_eq!(
            result, expected,
            "Expression '{}' should evaluate to {}",
            expr_str, expected
        );
    }
}

#[test]
fn test_script_execution_context_integration() {
    let mut ctx = ScriptExecutionContext::new();

    assert_eq!(ctx.depth, 0);
    assert!(ctx.current_file.is_none());

    ctx.enter_script("/path/to/script1.sql").unwrap();
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.current_file, Some("/path/to/script1.sql".to_string()));

    ctx.enter_script("/path/to/script2.sql").unwrap();
    assert_eq!(ctx.depth, 2);
    assert_eq!(ctx.current_file, Some("/path/to/script2.sql".to_string()));

    ctx.exit_script();
    assert_eq!(ctx.depth, 1);
    assert_eq!(ctx.current_file, Some("/path/to/script1.sql".to_string()));

    ctx.exit_script();
    assert_eq!(ctx.depth, 0);
    assert!(ctx.current_file.is_none());
}

#[test]
fn test_script_execution_context_circular_detection() {
    let mut ctx = ScriptExecutionContext::new();

    ctx.enter_script("script.sql").unwrap();
    let result = ctx.enter_script("script.sql");
    assert!(result.is_err(), "Should detect circular reference");
}

#[test]
fn test_script_execution_context_max_nesting() {
    let mut ctx = ScriptExecutionContext::new();

    for i in 0..16 {
        ctx.enter_script(&format!("script{}.sql", i)).unwrap();
    }

    let result = ctx.enter_script("too_deep.sql");
    assert!(result.is_err(), "Should prevent excessive nesting");
}

#[test]
fn test_is_statement_complete_integration() {
    let complete_statements = vec![
        "",
        "MATCH (v) RETURN v;",
        "\\set VAR value",
        "SHOW SPACES",
        "show spaces",
        "SHOW TAGS",
        "SHOW EDGES",
        "SHOW INDEXES",
        "SHOW USERS",
        "SHOW FUNCTIONS",
    ];

    for stmt in complete_statements {
        assert!(is_statement_complete(stmt), "'{}' should be complete", stmt);
    }

    let incomplete_statements = vec![
        "MATCH (v) RETURN v",
        "MATCH (v WHERE v.age > 0",
        "SELECT * FROM (",
        "RETURN ['item",
    ];

    for stmt in incomplete_statements {
        assert!(
            !is_statement_complete(stmt),
            "'{}' should be incomplete",
            stmt
        );
    }
}

#[test]
fn test_show_help_integration() {
    let general_help = show_help(None);
    assert!(!general_help.is_empty());
    assert!(general_help.contains("GraphDB"));

    let topics = vec![
        "match",
        "go",
        "insert",
        "create",
        "show",
        "use",
        "variables",
        "if",
        "history",
        "edit",
    ];

    for topic in topics {
        let help = show_help(Some(topic));
        assert!(!help.is_empty(), "Help for '{}' should not be empty", topic);
    }

    let unknown_help = show_help(Some("nonexistent_topic"));
    assert!(
        unknown_help.contains("No help available"),
        "Unknown topic should show error message"
    );
}

#[test]
fn test_show_version_and_copyright() {
    let version = show_version();
    assert!(version.contains("GraphDB"));
    assert!(version.contains("v"));

    let copyright = show_copyright();
    assert!(copyright.contains("GraphDB"));
    assert!(copyright.contains("Copyright"));
    assert!(copyright.contains("License"));
}

#[test]
fn test_parsed_statement_structure() {
    let statements = ScriptParser::parse("MATCH (v) RETURN v;\n\\set VAR value");

    assert_eq!(statements.len(), 2);

    assert!(matches!(statements[0].kind, StatementKind::Query));
    assert_eq!(statements[0].start_line, 1);
    assert_eq!(statements[0].end_line, 1);

    assert!(matches!(statements[1].kind, StatementKind::MetaCommand));
    assert_eq!(statements[1].start_line, 2);
    assert_eq!(statements[1].end_line, 2);
}

#[test]
fn test_copy_direction_variants() {
    let from = CopyDirection::From;
    let to = CopyDirection::To;

    assert!(matches!(from, CopyDirection::From));
    assert!(matches!(to, CopyDirection::To));
}

#[test]
fn test_history_action_variants() {
    let show = HistoryAction::Show { count: Some(10) };
    let search = HistoryAction::Search {
        pattern: "test".to_string(),
    };
    let clear = HistoryAction::Clear;
    let exec = HistoryAction::Exec { id: 5 };

    assert!(matches!(show, HistoryAction::Show { .. }));
    assert!(matches!(search, HistoryAction::Search { .. }));
    assert!(matches!(clear, HistoryAction::Clear));
    assert!(matches!(exec, HistoryAction::Exec { .. }));
}

#[test]
fn test_statement_kind_variants() {
    let query = StatementKind::Query;
    let meta = StatementKind::MetaCommand;

    assert!(matches!(query, StatementKind::Query));
    assert!(matches!(meta, StatementKind::MetaCommand));
}

#[test]
fn test_complex_script_parsing() {
    let script = r#"
\if MODE == test
  -- Test mode queries
  MATCH (v:Test) RETURN v LIMIT 10;
\elif MODE == prod
  -- Production queries
  MATCH (v:Prod) RETURN v;
\else
  -- Default queries
  MATCH (v) RETURN v LIMIT 100;
\endif

\set batch_size 1000
\timing

-- Batch insert
INSERT VERTEX Person(name) VALUES "1":("Alice");
INSERT VERTEX Person(name) VALUES "2":("Bob");
INSERT VERTEX Person(name) VALUES "3":("Charlie");

\unset batch_size
"#;

    let statements = ScriptParser::parse(script);

    let meta_count = statements
        .iter()
        .filter(|s| matches!(s.kind, StatementKind::MetaCommand))
        .count();
    let query_count = statements
        .iter()
        .filter(|s| matches!(s.kind, StatementKind::Query))
        .count();

    assert!(
        meta_count >= 6,
        "Should have at least 6 meta commands, found {}",
        meta_count
    );
    assert!(
        query_count >= 3,
        "Should have at least 3 queries, found {}",
        query_count
    );
}
