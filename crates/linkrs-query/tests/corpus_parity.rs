use std::collections::HashSet;

use linkrs_query::parser::Parser;

fn corpus() -> Vec<&'static str> {
    vec![
        "MATCH (a) RETURN a",
        "MATCH (a:person) WHERE a.age > 18 RETURN a",
        "MATCH (a)-[e:knows]->(b) RETURN a, b",
        "OPTIONAL MATCH (a) RETURN a",
        "GO FROM \"1\" OVER knows YIELD id($$) AS vid",
        "LOOKUP ON TAG player YIELD id(vertex) AS vid",
        "LOOKUP ON player YIELD id(vertex) AS vid",
        "FETCH PROP ON player \"1\" YIELD vertex.name AS name",
        "FIND SHORTEST PATH FROM \"1\" TO \"2\" OVER knows",
        "GET SUBGRAPH 2 STEPS FROM \"1\" OVER knows",
        "MATCH (a) RETURN a AS x ORDER BY x SKIP 1 LIMIT 10",
        "MATCH (a) WHERE a.name IS NULL RETURN a",
        "MATCH (a) WHERE a.name IS NOT NULL RETURN a",
        "MATCH (a) WHERE a.age NOT IN [1, 2] RETURN a",
        "MATCH (a) WHERE a.name STARTS WITH \"J\" RETURN a",
        "MATCH (a) WHERE a.name IS EMPTY RETURN a",
        "MATCH (a) WHERE a.age BETWEEN 1 AND 10 RETURN a",
        "MATCH (a) RETURN CASE WHEN a.age > 18 THEN \"adult\" ELSE \"minor\" END AS c",
        "MATCH (a) RETURN count(a) AS c",
        "UNWIND [1, 2] AS x RETURN x",
        "MATCH (a) WITH a AS b RETURN b",
        "CREATE TAG person (name STRING, age INT)",
        "DROP TAG person",
        "DESCRIBE TAG person",
        "DESC TAG person",
        "SHOW TAGS",
        "SHOW CREATE TAG person",
        "USE myspace",
        "INSERT VERTEX person(name) VALUES \"1\":(\"a\")",
        "UPDATE VERTEX person \"1\" SET name = \"b\"",
        "DELETE VERTEX person FROM \"1\"",
        "MATCH (a) RETURN ~a.age AS x",
        "MATCH (a) RETURN a.age! AS x",
        "MATCH (a) WHERE a.name =~ \"A.*\" RETURN a",
        "BEGIN TRANSACTION READ ONLY",
        "COMMIT",
        "CREATE USER alice WITH PASSWORD 'p'",
        "CREATE FULLTEXT INDEX idx ON tag(field)",
        "CREATE VECTOR INDEX idx ON tag(field)",
        "CREATE MACRO f(x) AS x",
        "CREATE TYPE uid AS INT",
        "CREATE GRAPH g",
        "KILL QUERY 1",
        "CLEAR SPACE",
        "SET x = 1",
        "ATTACH DATABASE 'a'",
        "EXPORT DATABASE 'a'",
        "LOAD EXTENSION 'a'",
        "MIGRATE PLAN",
        "CHECKPOINT",
        "PURGE EXPIRED",
        "SEARCH VECTOR 'a'",
        "MATCH FULLTEXT 'term'",
        "SEARCH 'term'",
        "CALL foo()",
        "LET x = 1",
        "$x = MATCH (a) RETURN a",
    ]
}

fn allowlist() -> HashSet<String> {
    let text =
        include_str!("corpus_parity_allowlist.txt");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.split('#').next().unwrap_or("").trim().to_string()
        })
        .filter(|stmt| !stmt.is_empty())
        .collect()
}

#[test]
fn handwritten_success_implies_tree_without_error() {
    let allowed = allowlist();
    let mut covered = 0usize;
    let mut skipped = 0usize;
    for stmt in corpus() {
        let mut parser = Parser::new(stmt);
        let parsed = parser.parse();
        if parsed.is_err() {
            continue;
        }
        if allowed.contains(stmt) {
            skipped += 1;
            continue;
        }
        let outcome = linkrs_grammar::parse(stmt);
        assert!(
            !outcome.has_error,
            "parity failure for {stmt:?}: tree contains ERROR/MISSING"
        );
        let toks = linkrs_grammar::tokens(stmt);
        assert!(!toks.is_empty(), "parity failure for {stmt:?}: empty token list");
        let mut last = 0;
        for t in &toks {
            assert!(t.start < t.end, "parity failure for {stmt:?}: empty span {t:?}");
            assert!(t.end <= stmt.len(), "parity failure for {stmt:?}: span {t:?} out of bounds");
            assert!(t.start >= last, "parity failure for {stmt:?}: spans out of order {t:?}");
            last = t.start;
            assert!(
                ["keyword", "variable", "property", "function", "number", "string", "operator", "comment", "punctuation"].contains(&t.class),
                "parity failure for {stmt:?}: unknown class {t:?}"
            );
        }
        covered += 1;
    }
    assert!(covered > 20, "corpus coverage too small: {covered}");
    assert!(
        skipped <= allowed.len(),
        "allowlist entries must correspond to corpus statements"
    );
}
