use graphdb_query::query::parser::Parser;

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
                graphdb_query::query::parser::ast::Stmt::Copy(copy) => {
                    println!(
                        "  Copy target: {:?} dir: {:?} file: {} header: {} delim: '{}' batch: {:?}",
                        copy.target,
                        copy.direction,
                        copy.file_path,
                        copy.header,
                        copy.delimiter,
                        copy.batch_size
                    );
                    if sql.contains(" TO ") {
                        assert_eq!(
                            copy.direction,
                            graphdb_query::query::parser::ast::stmt::CopyDirection::To,
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
