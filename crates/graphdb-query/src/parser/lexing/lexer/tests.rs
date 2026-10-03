    #[test]
    fn test_simple_identifiers() {
        let input = "CREATE MATCH RETURN";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.current_token.kind, Tk::Create);
        assert_eq!(lexer.current_token.lexeme, "CREATE");

        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Match);
        assert_eq!(lexer.current_token.lexeme, "MATCH");

        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Return);
        assert_eq!(lexer.current_token.lexeme, "RETURN");
    }

    #[test]
    fn test_unterminated_string() {
        let input = r#""hello"#;
        let lexer = Lexer::new(input);
        assert!(lexer.has_errors());
        assert!(!lexer.errors().is_empty());
    }

    #[test]
    fn test_unterminated_comment() {
        let input = "CREATE /* comment";
        let mut lexer = Lexer::new(input);
        lexer.advance();
        assert!(lexer.has_errors());
    }

    #[test]
    fn test_integer_literals() {
        let input = "42 100 0";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.current_token.kind, Tk::IntegerLiteral(42));
        assert_eq!(lexer.current_token.lexeme, "42");

        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::IntegerLiteral(100));
        assert_eq!(lexer.current_token.lexeme, "100");

        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::IntegerLiteral(0));
    }

    #[test]
    fn test_float_literals() {
        let input = "42"; // Testing integers
        let lexer = Lexer::new(input);
        assert_eq!(lexer.current_token.kind, Tk::IntegerLiteral(42));
    }

    #[test]
    fn test_string_literals() {
        let input = r#""hello world" "test""#;
        let mut lexer = Lexer::new(input);

        assert_eq!(
            lexer.current_token.kind,
            Tk::StringLiteral("hello world".to_string())
        );
        assert_eq!(lexer.current_token.lexeme, "hello world");

        lexer.advance();
        assert_eq!(
            lexer.current_token.kind,
            Tk::StringLiteral("test".to_string())
        );
    }

    #[test]
    fn test_operators() {
        let input = "+";
        let lexer = Lexer::new(input);
        assert_eq!(lexer.current_token.kind, Tk::Plus);
    }

    #[test]
    fn test_punctuation() {
        let input = "( ) [ ] { } , ; : @";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.current_token.kind, Tk::LParen);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::RParen);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::LBracket);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::RBracket);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::LBrace);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::RBrace);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Comma);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Semicolon);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Colon);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::At);
    }

    #[test]
    fn test_arrows() {
        let input = "<";
        let lexer = Lexer::new(input);
        assert_eq!(lexer.current_token.kind, Tk::Lt);
    }

    #[test]
    fn test_empty_input() {
        let input = "";
        let lexer = Lexer::new(input);
        assert_eq!(lexer.current_token.kind, Tk::Eof);
    }

    #[test]
    fn test_whitespace_handling() {
        let input = "  MATCH   \t\n  RETURN  ";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.current_token.kind, Tk::Match);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Return);
    }

    #[test]
    fn test_keywords() {
        let input = "MATCH WHERE RETURN YIELD DISTINCT LIMIT SKIP ORDER BY";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.current_token.kind, Tk::Match);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Where);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Return);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Yield);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Distinct);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Limit);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Skip);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::Order);
        lexer.advance();
        assert_eq!(lexer.current_token.kind, Tk::By);
    }

    #[test]
    fn test_aggregate_functions() {
        let input = "COUNT";
        let lexer = Lexer::new(input);
        assert_eq!(lexer.current_token.kind, Tk::Count);
    }

    #[test]
    fn test_values_keyword() {
        // Testing the recognition of the VALUES keyword
        let input = "VALUES";
        let lexer = Lexer::new(input);

        // Key test: The value "VALUES" should be recognized as a keyword.
        assert_eq!(lexer.current_token.kind, Tk::Values);
        assert_eq!(lexer.current_token.lexeme, "VALUES");
    }

    #[test]
    fn test_values_in_insert_context() {
        // The test focuses on the recognition of the VALUES keyword in the context of INSERT statements.
        let input = "INSERT VALUES";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.current_token.kind, Tk::Insert);
        lexer.advance();

        // The term "VALUES" should be recognized as a keyword, not as an identifier.
        assert_eq!(lexer.current_token.kind, Tk::Values);
        assert_eq!(lexer.current_token.lexeme, "VALUES");
    }

    #[test]
    fn test_values_case_insensitive() {
        // The VALUES keyword is case-insensitive when tested.
        let inputs = vec!["VALUES", "values", "Values", "VaLuEs"];

        for input in inputs {
            let lexer = Lexer::new(input);
            assert_eq!(
                lexer.current_token.kind,
                Tk::Values,
                "'{}' should be recognized as a Values keyword.",
                input
            );
        }
    }
