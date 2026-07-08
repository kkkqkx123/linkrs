use graphdb::query::parser::ParseContext;
use graphdb::query::parser::Parser;
use graphdb::query::parser::TokenKind;

#[test]
fn test_fulltext_tokenization() {
    let sql =
        "CREATE FULLTEXT INDEX IF NOT EXISTS idx_article_content ON article(content) ENGINE BM25";
    let mut parser = Parser::new(sql);

    // Manually check what happens after CREATE is consumed
    let mut ctx = ParseContext::new(sql);

    // Skip CREATE token
    let _ = ctx.expect_token(TokenKind::Create);

    // Check what the current token is
    let current = ctx.current_token();
    eprintln!("After CREATE, current token: {:?}", current.kind);
    eprintln!(
        "check_keyword('FULLTEXT'): {}",
        ctx.check_keyword("FULLTEXT")
    );

    // Try parsing
    match parser.parse() {
        Ok(stmt) => println!("Parsed successfully: {:?}", stmt),
        Err(e) => println!("Parse error: {}", e),
    }
}
