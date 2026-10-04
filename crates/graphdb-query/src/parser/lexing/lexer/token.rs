//! Token generation for the lexer.

use crate::parser::lexing::LexError;
use crate::parser::{Token, TokenKind as Tk};

use super::Lexer;

impl<'a> Lexer<'a> {
    fn lookup_keyword(&self, identifier: &str) -> Tk {
        match identifier.to_uppercase().as_str() {
            "CREATE" => Tk::Create,
            "MATCH" => Tk::Match,
            "RETURN" => Tk::Return,
            "WHERE" => Tk::Where,
            "DELETE" => Tk::Delete,
            "DETACH" => Tk::Detach,
            "UPDATE" => Tk::Update,
            "INSERT" => Tk::Insert,
            "UPSERT" => Tk::Upsert,
            "VALUES" => Tk::Values,
            "FROM" => Tk::From,
            "TO" => Tk::To,
            "AS" => Tk::As,
            "WITH" => Tk::With,
            "YIELD" => Tk::Yield,
            "GO" => Tk::Go,
            "OVER" => Tk::Over,
            "STEPS" | "STEP" => Tk::Step,
            "UPTO" => Tk::Upto,
            "LIMIT" => Tk::Limit,
            "ASC" => Tk::Asc,
            "DESC" | "DESCRIBE" => Tk::Desc,
            "ORDER" => Tk::Order,
            "BY" => Tk::By,
            "SKIP" => Tk::Skip,
            "UNWIND" => Tk::Unwind,
            "OPTIONAL" => Tk::Optional,
            "DISTINCT" => Tk::Distinct,
            "ALL" => Tk::All,
            "NULL" => Tk::Null,
            "IS" => Tk::Is,
            "NOT" => Tk::Not,
            "AND" => Tk::And,
            "OR" => Tk::Or,
            "XOR" => Tk::Xor,
            "CONTAINS" => Tk::Contains,
            "STARTS" | "STARTS WITH" => Tk::StartsWith,
            "ENDS" | "ENDS WITH" => Tk::EndsWith,
            "CASE" => Tk::Case,
            "WHEN" => Tk::When,
            "THEN" => Tk::Then,
            "ELSE" => Tk::Else,
            "END" => Tk::End,
            "UNION" => Tk::Union,
            "INTERSECT" => Tk::Intersect,
            "MINUS" => Tk::SetMinus,
            "GROUP" => Tk::Group,
            "HAVING" => Tk::Having,
            "FILTER" => Tk::Filter,
            "ROLLUP" => Tk::Rollup,
            "CUBE" => Tk::Cube,
            "GROUPING" => Tk::Grouping,
            "SETS" => Tk::Sets,
            "BETWEEN" => Tk::Between,
            "ADMIN" => Tk::Admin,
            "EDGE" => Tk::Edge,
            "EDGES" => Tk::Edges,
            "VERTEX" => Tk::Vertex,
            "VERTICES" => Tk::Vertices,
            "TAG" => Tk::Tag,
            "TAGS" => Tk::Tags,
            "INDEX" => Tk::Index,
            "INDEXES" => Tk::Indexes,
            "LOOKUP" => Tk::Lookup,
            "FIND" => Tk::Find,
            "WEIGHT" => Tk::Weight,
            "PATH" => Tk::Path,
            "SHORTEST" => Tk::Shortest,
            "ALLSHORTESTPATHS" => Tk::AllShortestPaths,
            "TRAIL" => Tk::Trail,
            "ACYCLIC" => Tk::Acyclic,
            "WEIGHTED" => Tk::Weighted,
            "LOOP" => Tk::Loop,
            "CYCLE" => Tk::Cycle,
            "SUBGRAPH" => Tk::Subgraph,
            "BOTH" => Tk::Both,
            "OUT" => Tk::Out,
            "IN" => Tk::In,
            "REVERSELY" => Tk::Reversely,
            "RECURSIVE" => Tk::Recursive,
            "NO" => Tk::No,
            "OVERWRITE" => Tk::Overwrite,
            "SHOW" => Tk::Show,
            "ADD" => Tk::Add,
            "DROP" => Tk::Drop,
            "REMOVE" => Tk::Remove,
            "ALTER" => Tk::Alter,
            "IF" => Tk::If,
            "EXISTS" => Tk::Exists,
            "SUBQUERY" => Tk::Subquery,
            "CHANGE" => Tk::Change,
            "CREATEUSER" => Tk::CreateUser,
            "ALTERUSER" => Tk::AlterUser,
            "DROPUSER" => Tk::DropUser,
            "CHANGEPASSWORD" => Tk::ChangePassword,
            "GRANT" => Tk::Grant,
            "REVOKE" => Tk::Revoke,
            "ON" => Tk::On,
            "OF" => Tk::Of,
            "GET" => Tk::Get,
            "SET" => Tk::Set,
            "HOST" => Tk::Host,
            "HOSTS" => Tk::Hosts,
            "SPACE" => Tk::Space,
            "SPACES" => Tk::Spaces,
            "SEQUENCE" => Tk::Sequence,
            "SEQUENCES" => Tk::Sequences,
            "USER" => Tk::User,
            "USERS" => Tk::Users,
            "PASSWORD" => Tk::Password,
            "ROLE" => Tk::Role,
            "ROLES" => Tk::Roles,
            "FUNCTIONS" => Tk::Functions,
            "GRAPHS" => Tk::Graphs,
            "MACROS" => Tk::Macros,
            "LOCKED" => Tk::Locked,
            "GOD" => Tk::God,
            "DBA" => Tk::Dba,
            "GUEST" => Tk::Guest,
            "COMMENT" => Tk::Comment,
            "CHARSET" => Tk::Charset,
            "COLLATE" => Tk::Collate,
            "COLLATION" => Tk::Collation,
            "VID_TYPE" => Tk::VIdType,
            "PARTITION_NUM" => Tk::PartitionNum,
            "REPLICA_FACTOR" => Tk::ReplicaFactor,
            "REBUILD" => Tk::Rebuild,
            "BOOL" => Tk::Bool,
            "INT" => Tk::Int,
            "INT8" => Tk::Int8,
            "INT16" => Tk::Int16,
            "INT32" => Tk::Int32,
            "INT64" => Tk::Int64,
            "FLOAT" => Tk::Float,
            "DOUBLE" => Tk::Double,
            "STRING" => Tk::String,
            "FIXED_STRING" => Tk::FixedString,
            "TIMESTAMP" => Tk::Timestamp,
            "DATE" => Tk::Date,
            "TIME" => Tk::Time,
            "DATETIME" => Tk::Datetime,
            "SERIAL" => Tk::Serial,
            "DURATION" => Tk::Duration,
            "GEOGRAPHY" => Tk::Geography,
            "POINT" => Tk::Point,
            "LINESTRING" => Tk::Linestring,
            "POLYGON" => Tk::Polygon,
            "LIST" => Tk::List,
            "MAP" => Tk::Map,
            "STRUCT" => Tk::Struct,
            "ARRAY" => Tk::Array,
            "DOWNLOAD" => Tk::Download,
            "HDFS" => Tk::HDFS,
            "UUID" => Tk::UUID,
            "CONFIGS" => Tk::Configs,
            "FORCE" => Tk::Force,
            "PART" => Tk::Part,
            "PARTS" => Tk::Parts,
            "DATA" => Tk::Data,
            "LEADER" => Tk::Leader,
            "JOBS" => Tk::Jobs,
            "JOB" => Tk::Job,
            "BIDIRECT" => Tk::Bidirect,
            "STATS" => Tk::Stats,
            "STATUS" => Tk::Status,
            "RECOVER" => Tk::Recover,
            "EXPLAIN" => Tk::Explain,
            "PROFILE" => Tk::Profile,
            "ANALYZE" => Tk::Analyze,
            "FORMAT" => Tk::Format,
            "ATOMIC_EDGE" => Tk::AtomicEdge,
            "DEFAULT" => Tk::Default,
            "FLUSH" => Tk::Flush,
            "COMPACT" => Tk::Compact,
            "SUBMIT" => Tk::Submit,
            "ASCENDING" => Tk::Ascending,
            "DESCENDING" => Tk::Descending,
            "FETCH" => Tk::Fetch,
            "PROP" => Tk::Prop,
            "BALANCE" => Tk::Identifier("balance".to_string()),
            "STOP" => Tk::Stop,
            "REVERT" => Tk::Revert,
            "USE" => Tk::Use,
            "BEGIN" => Tk::Begin,
            "COMMIT" => Tk::Commit,
            "ROLLBACK" => Tk::Rollback,
            "SAVEPOINT" => Tk::Savepoint,
            "RELEASE" => Tk::Release,
            "TRANSACTION" => Tk::Transaction,
            "LET" => Tk::Let,
            "READ" => Tk::Read,
            "ONLY" => Tk::Only,
            "WRITE" => Tk::Write,
            "SETLIST" => Tk::SetList,
            "CLEAR" => Tk::Clear,
            "MERGE" => Tk::Merge,
            "DIVIDE" => Tk::Divide,
            "RENAME" => Tk::Rename,
            "LOCAL" => Tk::Local,
            "SESSIONS" => Tk::Sessions,
            "SESSION" => Tk::Session,
            "SAMPLE" => Tk::Sample,
            "QUERIES" => Tk::Queries,
            "QUERY" => Tk::Query,
            "KILL" => Tk::Kill,
            "TOP" => Tk::Top,
            "TEXT" => Tk::Text,
            "SEARCH" => Tk::Search,
            "VECTOR" => Tk::KeywordVector,
            "CLIENT" => Tk::Client,
            "CLIENTS" => Tk::Clients,
            "SIGN" => Tk::Sign,
            "SERVICE" => Tk::Service,
            "COUNT" => Tk::Count,
            "SUM" => Tk::Sum,
            "AVG" => Tk::Avg,
            "MIN" => Tk::Min,
            "MAX" => Tk::Max,
            "SOURCE" => Tk::Source,
            "DESTINATION" => Tk::Destination,
            "RANK" => Tk::Rank,
            "INPUT" => Tk::Input,
            "COPY" => Tk::Copy,
            "HEADER" => Tk::Header,
            "DELIMITER" => Tk::Delimiter,
            "CSV" => Tk::Csv,
            "TRUE" => Tk::BooleanLiteral(true),
            "FALSE" => Tk::BooleanLiteral(false),
            _ => Tk::Identifier(identifier.to_string()),
        }
    }
    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace();

        if let Some(&ch) = self.peek_char() {
            if ch == '/' || ch == '-' {
                if let Ok(()) = self.skip_comment() {
                    if let Some(&ch) = self.peek_char() {
                        if ch == '\n' || ch == '\0' {
                            return Token::new(Tk::Eof, String::new(), self.line, self.column);
                        }
                    }
                }
            }
        }

        self.skip_whitespace();

        let token = match self.peek_char() {
            Some(&'=') => {
                self.read_char();
                if let Some(&'=') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::Eq, "==".to_string(), self.line, self.column)
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
                if let Some(&'=') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::Regex, "=~".to_string(), self.line, self.column)
                } else {
                    self.read_char();
                    Token::new(Tk::NotOp, "~".to_string(), self.line, self.column)
                }
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
                match literal.as_str() {
                    "_id" => Token::new(Tk::IdProp, literal, start_line, start_col),
                    "_type" => Token::new(Tk::TypeProp, literal, start_line, start_col),
                    "_src" => Token::new(Tk::SrcIdProp, literal, start_line, start_col),
                    "_dst" => Token::new(Tk::DstIdProp, literal, start_line, start_col),
                    "_rank" => Token::new(Tk::RankProp, literal, start_line, start_col),
                    _ => {
                        let token_kind = self.lookup_keyword(&literal);
                        match token_kind {
                            Tk::KeywordVector => {
                                // Check if followed by '[' for VECTOR[...] syntax
                                self.skip_whitespace();
                                if let Some(&'[') = self.peek_char() {
                                    // This is VECTOR[...] syntax, parse the vector
                                    self.read_char(); // consume '['
                                    let vector_data = self.parse_vector_elements();
                                    return Token::new(
                                        Tk::VectorLiteral(vector_data.clone()),
                                        format!(
                                            "VECTOR[{}]",
                                            vector_data
                                                .iter()
                                                .map(|f| f.to_string())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        ),
                                        start_line,
                                        start_col,
                                    );
                                } else {
                                    Token::new(token_kind, literal, start_line, start_col)
                                }
                            }
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
                    literal,
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
