//! Token generation for the lexer.

use crate::parser::lexing::LexError;
use crate::parser::{Token, TokenKind as Tk};

use super::Lexer;
use crate::parser::lexing::spec;

impl<'a> Lexer<'a> {
    pub(crate) fn lookup_keyword(&self, identifier: &str) -> Tk {
        spec::lookup_keyword_spelling(identifier.to_uppercase().as_str())
            .unwrap_or_else(|| Tk::Identifier(identifier.to_string()))
    }
    pub fn next_token(&mut self) -> Token {
        loop {
            self.skip_whitespace();
            match self.peek_char() {
                Some(&'/') | Some(&'-') => {
                    if self.skip_comment().is_ok() {
                        continue;
                    }
                }
                _ => {}
            }
            break;
        }
        let span_start = self.position;

        let mut token = match self.peek_char() {
            Some(&'=') => {
                self.read_char();
                if let Some(&'=') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::Eq, "==".to_string(), self.line, self.column)
                } else if let Some(&'~') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::Regex, "=~".to_string(), self.line, self.column)
                } else {
                    Token::new(Tk::Assign, "=".to_string(), self.line, self.column)
                }
            }
            Some(&'+') => {
                self.read_char();
                Token::new(Tk::Plus, "+".to_string(), self.line, self.column)
            }
            Some(&'-') => {
                self.read_char();
                if let Some(&'>') = self.peek_char() {
                    self.read_char();
                    if let Some(&'>') = self.peek_char() {
                        self.read_char();
                        Token::new(Tk::ArrowRight, "->>".to_string(), self.line, self.column)
                    } else {
                        Token::new(Tk::Arrow, "->".to_string(), self.line, self.column)
                    }
                } else {
                    Token::new(Tk::Minus, "-".to_string(), self.line, self.column)
                }
            }
            Some(&'*') => {
                self.read_char();
                if let Some(&'*') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::Exp, "**".to_string(), self.line, self.column)
                } else {
                    Token::new(Tk::Star, "*".to_string(), self.line, self.column)
                }
            }
            Some(&'/') => {
                self.read_char();
                Token::new(Tk::Div, "/".to_string(), self.line, self.column)
            }
            Some(&'%') => {
                self.read_char();
                Token::new(Tk::Mod, "%".to_string(), self.line, self.column)
            }
            Some(&'!') => {
                self.read_char();
                if let Some(&'=') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::Ne, "!=".to_string(), self.line, self.column)
                } else {
                    Token::new(Tk::NotOp, "!".to_string(), self.line, self.column)
                }
            }
            Some(&'<') => {
                self.read_char();
                match self.peek_char() {
                    Some(&'-') => {
                        self.read_char();
                        Token::new(Tk::BackArrow, "<-".to_string(), self.line, self.column)
                    }
                    Some(&'=') => {
                        self.read_char();
                        Token::new(Tk::Le, "<=".to_string(), self.line, self.column)
                    }
                    Some(&'>') => {
                        self.read_char();
                        Token::new(Tk::Ne, "<>".to_string(), self.line, self.column)
                    }
                    Some(&'<') => {
                        self.read_char();
                        Token::new(Tk::ShiftLeft, "<<".to_string(), self.line, self.column)
                    }
                    _ => Token::new(Tk::Lt, "<".to_string(), self.line, self.column),
                }
            }
            Some(&'>') => {
                self.read_char();
                if let Some(&'=') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::Ge, ">=".to_string(), self.line, self.column)
                } else if let Some(&'>') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::ShiftRight, ">>".to_string(), self.line, self.column)
                } else {
                    Token::new(Tk::Gt, ">".to_string(), self.line, self.column)
                }
            }
            Some(&'~') => {
                self.read_char();
                Token::new(Tk::Tilde, "~".to_string(), self.line, self.column)
            }
            Some(&'(') => {
                self.read_char();
                Token::new(Tk::LParen, "(".to_string(), self.line, self.column)
            }
            Some(&')') => {
                self.read_char();
                Token::new(Tk::RParen, ")".to_string(), self.line, self.column)
            }
            Some(&'[') => {
                self.read_char();
                Token::new(Tk::LBracket, "[".to_string(), self.line, self.column)
            }
            Some(&']') => {
                self.read_char();
                Token::new(Tk::RBracket, "]".to_string(), self.line, self.column)
            }
            Some(&'{') => {
                self.read_char();
                Token::new(Tk::LBrace, "{".to_string(), self.line, self.column)
            }
            Some(&'}') => {
                self.read_char();
                Token::new(Tk::RBrace, "}".to_string(), self.line, self.column)
            }
            Some(&',') => {
                self.read_char();
                Token::new(Tk::Comma, ",".to_string(), self.line, self.column)
            }
            Some(&'.') => {
                self.read_char();
                if let Some(&'.') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::DotDot, "..".to_string(), self.line, self.column)
                } else {
                    Token::new(Tk::Dot, ".".to_string(), self.line, self.column)
                }
            }
            Some(&':') => {
                self.read_char();
                if let Some(&':') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::DoubleColon, "::".to_string(), self.line, self.column)
                } else {
                    Token::new(Tk::Colon, ":".to_string(), self.line, self.column)
                }
            }
            Some(&';') => {
                self.read_char();
                Token::new(Tk::Semicolon, ";".to_string(), self.line, self.column)
            }
            Some(&'?') => {
                self.read_char();
                Token::new(Tk::QMark, "?".to_string(), self.line, self.column)
            }
            Some(&'|') => {
                self.read_char();
                if let Some(&'|') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::DoublePipe, "||".to_string(), self.line, self.column)
                } else {
                    Token::new(Tk::Pipe, "|".to_string(), self.line, self.column)
                }
            }
            Some(&'@') => {
                self.read_char();
                Token::new(Tk::At, "@".to_string(), self.line, self.column)
            }
            Some(&'$') => {
                self.read_char();
                match self.peek_char() {
                    Some(&'$') => {
                        self.read_char();
                        Token::new(Tk::DstRef, "$$".to_string(), self.line, self.column)
                    }
                    Some(&'^') => {
                        self.read_char();
                        Token::new(Tk::SrcRef, "$^".to_string(), self.line, self.column)
                    }
                    Some(&'-') => {
                        self.read_char();
                        Token::new(Tk::InputRef, "$-".to_string(), self.line, self.column)
                    }
                    _ => Token::new(Tk::Dollar, "$".to_string(), self.line, self.column),
                }
            }
            Some(&'"') | Some(&'\'') => {
                let start_col = self.column;
                let start_line = self.line;
                match self.read_string() {
                    Ok(literal) => Token::new(
                        Tk::StringLiteral(literal.clone()),
                        literal,
                        start_line,
                        start_col,
                    ),
                    Err(e) => {
                        self.add_error(e);
                        Token::new(
                            Tk::StringLiteral(String::new()),
                            String::new(),
                            start_line,
                            start_col,
                        )
                    }
                }
            }
            Some(&ch) if ch.is_ascii_digit() => {
                let start_col = self.column;
                let start_line = self.line;
                let literal = self.read_number();
                if literal.contains('.') || literal.contains('e') || literal.contains('E') {
                    match literal.parse::<f64>() {
                        Ok(float_val) => {
                            Token::new(Tk::FloatLiteral(float_val), literal, start_line, start_col)
                        }
                        Err(_) => {
                            let error =
                                LexError::invalid_number(literal.clone(), self.current_position());
                            self.add_error(error);
                            Token::new(Tk::FloatLiteral(0.0), literal, start_line, start_col)
                        }
                    }
                } else {
                    match literal.parse::<i64>() {
                        Ok(int_val) => {
                            Token::new(Tk::IntegerLiteral(int_val), literal, start_line, start_col)
                        }
                        Err(_) => {
                            let error =
                                LexError::invalid_number(literal.clone(), self.current_position());
                            self.add_error(error);
                            Token::new(Tk::IntegerLiteral(0), literal, start_line, start_col)
                        }
                    }
                }
            }
            Some(&ch) if ch.is_alphabetic() || ch == '_' => {
                let start_col = self.column;
                let start_line = self.line;
                let literal = self.read_identifier();
                match spec::underscore_prop_kind(literal.as_str()) {
                    Some(kind) => Token::new(kind, literal, start_line, start_col),
                    None => {
                        let token_kind = self.lookup_keyword(&literal);
                        match token_kind {
                            Tk::Not => {
                                if self.peek_word() == "IN" {
                                    self.skip_word();
                                    Token::new(
                                        Tk::NotIn,
                                        "NOT IN".to_string(),
                                        start_line,
                                        start_col,
                                    )
                                } else {
                                    Token::new(token_kind, literal, start_line, start_col)
                                }
                            }
                            Tk::Is => match self.peek_word().as_str() {
                                "NULL" => {
                                    self.skip_word();
                                    Token::new(
                                        Tk::IsNull,
                                        "IS NULL".to_string(),
                                        start_line,
                                        start_col,
                                    )
                                }
                                "NOT" => match self.peek_word_after().as_str() {
                                    "NULL" => {
                                        self.skip_word();
                                        self.skip_word();
                                        Token::new(
                                            Tk::IsNotNull,
                                            "IS NOT NULL".to_string(),
                                            start_line,
                                            start_col,
                                        )
                                    }
                                    "EMPTY" => {
                                        self.skip_word();
                                        self.skip_word();
                                        Token::new(
                                            Tk::IsNotEmpty,
                                            "IS NOT EMPTY".to_string(),
                                            start_line,
                                            start_col,
                                        )
                                    }
                                    _ => Token::new(token_kind, literal, start_line, start_col),
                                },
                                "EMPTY" => {
                                    self.skip_word();
                                    Token::new(
                                        Tk::IsEmpty,
                                        "IS EMPTY".to_string(),
                                        start_line,
                                        start_col,
                                    )
                                }
                                _ => Token::new(token_kind, literal, start_line, start_col),
                            },
                            _ => Token::new(token_kind, literal, start_line, start_col),
                        }
                    }
                }
            }
            Some(&'`') => {
                let start_col = self.column;
                let start_line = self.line;
                self.read_char();
                let literal = self.read_backtick_identifier();
                Token::new(
                    Tk::Identifier(literal.clone()),
                    format!("`{literal}`"),
                    start_line,
                    start_col,
                )
            }
            Some(&'#') => {
                self.read_char();
                if let Some(&'>') = self.peek_char() {
                    self.read_char();
                    if let Some(&'>') = self.peek_char() {
                        self.read_char();
                        Token::new(
                            Tk::HashArrowRight,
                            "#>>".to_string(),
                            self.line,
                            self.column,
                        )
                    } else {
                        Token::new(Tk::HashArrow, "#>".to_string(), self.line, self.column)
                    }
                } else {
                    let start_col = self.column;
                    let start_line = self.line;
                    self.add_error(LexError::unexpected_character('#', self.current_position()));
                    Token::new(
                        Tk::Identifier("#".to_string()),
                        "#".to_string(),
                        start_line,
                        start_col,
                    )
                }
            }
            Some(&'&') => {
                self.read_char();
                Token::new(Tk::Ampersand, "&".to_string(), self.line, self.column)
            }
            Some(&ch) => {
                let start_col = self.column;
                let start_line = self.line;
                let unexpected = ch.to_string();
                self.read_char();
                self.add_error(LexError::unexpected_character(ch, self.current_position()));
                Token::new(
                    Tk::Identifier(unexpected.clone()),
                    unexpected,
                    start_line,
                    start_col,
                )
            }
            None => Token::new(Tk::Eof, String::new(), self.line, self.column),
        };

        debug_assert!(
            spec::check_symbol_token(&token.kind, &token.lexeme),
            "scanner output inconsistent with symbol spec: {:?} {:?}",
            token.kind,
            token.lexeme
        );
        token.start = span_start;
        token.end = self.position;
        token
    }
    fn peek_word(&mut self) -> String {
        let mut temp_lexer = self.clone();
        temp_lexer.skip_whitespace();
        temp_lexer.read_identifier()
    }

    fn peek_word_after(&mut self) -> String {
        let mut temp_lexer = self.clone();
        temp_lexer.skip_whitespace();
        temp_lexer.read_identifier();
        temp_lexer.read_identifier()
    }

    fn skip_word(&mut self) {
        self.skip_whitespace();
        while let Some(&ch) = self.peek_char() {
            if ch.is_whitespace() {
                break;
            }
            self.read_char();
        }
    }
}
