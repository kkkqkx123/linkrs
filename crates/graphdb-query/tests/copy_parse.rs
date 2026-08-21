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
                        "  Copy target: {:?} file: {} header: {} delim: '{}' batch: {:?}",
                        copy.target, copy.file_path, copy.header, copy.delimiter, copy.batch_size
                    );
                }
                _ => panic!("NOT COPY"),
            }
            assert!(!parser.has_errors());
        }
    }
}
