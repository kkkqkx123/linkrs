//! Parser-level tests for COPY statements.
//!
//! Scope: lexer/parser mapping only, no storage or execution.
//! Execution coverage lives in `copy_e2e.rs`. Kept under `tests/`
//! so parser regressions are caught by the integration target as well
//! as crate unit tests.
use linkrs_query::parser::Parser;

#[test]
fn test_copy_parsing() {
    let tests = vec![
        ("COPY VERTEX person FROM \"data.csv\"", true),
        ("COPY VERTEX person FROM \"data.csv\" WITH HEADER", true),
        (
            "COPY VERTEX person FROM \"data.csv\" WITH HEADER DELIMITER ','",
            true,
        ),
        ("COPY EDGE knows FROM \"edges.csv\" WITH HEADER", true),
        (
            "COPY person FROM \"data.csv\" WITH (HEADER true, DELIMITER ',')",
            true,
        ),
        (
            "COPY VERTEX person FROM \"data.csv\" WITH HEADER BATCH_SIZE 500",
            true,
        ),
        ("COPY person FROM \"data.csv\" WITH NO HEADER", true),
        // Export direction
        ("COPY VERTEX person TO \"out.csv\"", true),
        ("COPY EDGE knows TO \"edges_out.csv\" WITH HEADER", true),
        ("COPY person TO \"out.csv\" WITH HEADER DELIMITER ';'", true),
    ];
    for (sql, should_pass) in tests {
        let mut parser = Parser::new(sql);
        let result = parser.parse();
        println!("Testing '{}': {:?}", sql, result.is_ok());
        if should_pass {
            assert!(
                result.is_ok(),
                "Failed to parse '{}': {:?}",
                sql,
                result.err()
            );
            let parsed = result.unwrap();
            println!("  stmt kind: {}", parsed.ast.stmt().kind());
            match parsed.ast.stmt() {
                linkrs_query::parser::ast::Stmt::Copy(copy) => {
                    println!(
                        "  Copy target: {:?} dir: {:?} file: {} header: {} delim: '{}' batch: {:?}",
                        copy.target,
                        copy.direction,
                        copy.file_paths.join(", "),
                        copy.header,
                        copy.delimiter,
                        copy.batch_size
                    );
                    if sql.contains(" TO ") {
                        assert_eq!(
                            copy.direction,
                            linkrs_query::parser::ast::stmt::CopyDirection::To,
                            "'{sql}' must parse as export"
                        );
                    }
                }
                _ => panic!("NOT COPY"),
            }
            assert!(!parser.has_errors());
        }
    }
}

#[test]
fn test_copy_batch_size_boundary_parse_layers() {
    // A negative batch size is rejected by the parser; zero parses and is
    // rejected later at plan time (`COPY batch_size must be > 0`, covered
    // in `copy_e2e.rs`).
    let mut negative = Parser::new("COPY VERTEX person FROM \"data.csv\" WITH BATCH_SIZE -1");
    assert!(
        negative.parse().is_err() || negative.has_errors(),
        "negative BATCH_SIZE should fail at parse level"
    );

    let mut zero = Parser::new("COPY VERTEX person FROM \"data.csv\" WITH BATCH_SIZE 0");
    let parsed = zero.parse().expect("BATCH_SIZE 0 should parse");
    assert!(!zero.has_errors());
    match parsed.ast.stmt() {
        linkrs_query::parser::ast::Stmt::Copy(copy) => {
            assert_eq!(copy.batch_size, Some(0));
        }
        _ => panic!("NOT COPY"),
    }
}
