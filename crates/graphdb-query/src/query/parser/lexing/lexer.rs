//! Lexer implementation for the query parser
//!
//! This module implements a lexical analyzer that converts input query strings into tokens.

use crate::core::types::Position;
use crate::query::parser::lexing::LexError;
use crate::query::parser::{Token, TokenKind as Tk};
use std::iter::Peekable;

#[derive(Clone)]
pub struct Lexer<'a> {
    input: std::borrow::Cow<'a, str>,
    chars: Peekable<std::vec::IntoIter<char>>,
    position: usize,
    line: usize,
    column: usize,
    current_token: Token,
    errors: Vec<LexError>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let mut lexer = Lexer {
            input: std::borrow::Cow::Borrowed(input),
            chars: chars.into_iter().peekable(),
            position: 0,
            line: 1,
            column: 0,
            current_token: Token::new(Tk::Eof, String::new(), 0, 0),
            errors: Vec::new(),
        };
        lexer.current_token = lexer.next_token();
        lexer
    }

    pub fn from_string(input: String) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let mut lexer = Lexer {
            input: std::borrow::Cow::Owned(input),
            chars: chars.into_iter().peekable(),
            position: 0,
            line: 1,
            column: 0,
            current_token: Token::new(Tk::Eof, String::new(), 0, 0),
            errors: Vec::new(),
        };
        lexer.read_char();
        lexer.current_token = lexer.next_token();
        lexer
    }

    fn read_char(&mut self) -> Option<char> {
        let ch = self.chars.next();
        self.position += 1;
        if ch == Some('\n') {
            self.line += 1;
            self.column = 0;
        } else {
            self.column += 1;
        }
        ch
    }

    fn peek_char(&mut self) -> Option<&char> {
        self.chars.peek()
    }

    fn add_error(&mut self, error: LexError) {
        self.errors.push(error);
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn take_errors(&mut self) -> Vec<LexError> {
        std::mem::take(&mut self.errors)
    }

    pub fn errors(&self) -> &[LexError] {
        &self.errors
    }

    pub fn current_token(&self) -> &Token {
        &self.current_token
    }

    fn skip_whitespace(&mut self) {
        while let Some(&ch) = self.peek_char() {
            if ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n' {
                self.read_char();
            } else {
                break;
            }
        }
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        while let Some(&ch) = self.peek_char() {
            if ch.is_alphanumeric() || ch == '_' {
                self.read_char();
            } else {
                break;
            }
        }
        self.input
            .get(start..self.position)
            .unwrap_or("")
            .to_string()
    }

    fn read_backtick_identifier(&mut self) -> String {
        let start = self.position;
        loop {
            match self.peek_char() {
                Some(&'`') => {
                    self.read_char();
                    break;
                }
                Some(&ch) if ch == '\n' || ch == '\r' => {
                    break;
                }
                Some(_) => {
                    self.read_char();
                }
                None => break,
            }
        }
        self.input
            .get(start..self.position.saturating_sub(1))
            .unwrap_or("")
            .to_string()
    }

    fn read_number(&mut self) -> String {
        let start = self.position;
        let mut has_decimal = false;
        let mut has_exponent = false;

        while let Some(&ch) = self.peek_char() {
            if ch.is_ascii_digit() {
                self.read_char();
            } else if ch == '.' && !has_decimal && !has_exponent {
                // Check whether a number follows the text (using “peek” without consuming any characters).
                let mut temp_chars = self.chars.clone();
                temp_chars.next(); // Skip.
                if temp_chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    has_decimal = true;
                    self.read_char();
                } else {
                    break;
                }
            } else if (ch == 'e' || ch == 'E') && !has_exponent {
                has_exponent = true;
                self.read_char();
                if let Some(&ch) = self.peek_char() {
                    if ch == '+' || ch == '-' {
                        self.read_char();
                    }
                }
            } else {
                break;
            }
        }
        self.input
            .get(start..self.position)
            .unwrap_or("")
            .to_string()
    }

    fn read_string(&mut self) -> Result<String, LexError> {
        let start_position = self.current_position();
        let quote = match self.read_char() {
            Some(ch) => ch,
            None => {
                return Err(LexError::new(
                    "Unexpected end of input while reading string".to_string(),
                    start_position,
                ));
            }
        };

        let mut result = String::new();

        loop {
            match self.peek_char() {
                Some(&'\\') => {
                    self.read_char();
                    if let Some(ch) = self.read_char() {
                        match ch {
                            'n' => result.push('\n'),
                            't' => result.push('\t'),
                            'r' => result.push('\r'),
                            '\\' => result.push('\\'),
                            '"' => result.push('"'),
                            '\'' => result.push('\''),
                            '0' => result.push('\0'),
                            '\n' => {
                                while let Some(&c) = self.peek_char() {
                                    if c == ' ' || c == '\t' {
                                        self.read_char();
                                    } else {
                                        break;
                                    }
                                }
                                continue;
                            }
                            'u' => {
                                let mut unicode_seq = String::new();
                                for _ in 0..4 {
                                    if let Some(&c) = self.peek_char() {
                                        if c.is_ascii_hexdigit() {
                                            unicode_seq.push(c);
                                            self.read_char();
                                        } else {
                                            break;
                                        }
                                    } else {
                                        break;
                                    }
                                }
                                if !unicode_seq.is_empty() {
                                    if let Ok(code_point) = u32::from_str_radix(&unicode_seq, 16) {
                                        if let Some(ch) = char::from_u32(code_point) {
                                            result.push(ch);
                                        }
                                    } else {
                                        self.add_error(LexError::invalid_escape_sequence(
                                            format!("u{}", unicode_seq),
                                            self.current_position(),
                                        ));
                                    }
                                }
                            }
                            'x' => {
                                let mut hex_seq = String::new();
                                for _ in 0..2 {
                                    if let Some(&c) = self.peek_char() {
                                        if c.is_ascii_hexdigit() {
                                            hex_seq.push(c);
                                            self.read_char();
                                        } else {
                                            break;
                                        }
                                    } else {
                                        break;
                                    }
                                }
                                if !hex_seq.is_empty() {
                                    if let Ok(byte) = u8::from_str_radix(&hex_seq, 16) {
                                        result.push(byte as char);
                                    } else {
                                        self.add_error(LexError::invalid_escape_sequence(
                                            format!("x{}", hex_seq),
                                            self.current_position(),
                                        ));
                                    }
                                }
                            }
                            _ => {
                                result.push('\\');
                                result.push(ch);
                            }
                        }
                    }
                }
                Some(&'\'') | Some(&'"') => match self.peek_char() {
                    Some(&q) if q == quote => {
                        self.read_char();
                        return Ok(result);
                    }
                    Some(_) => {
                        let ch = self.read_char().ok_or_else(|| {
                            LexError::unexpected_end_of_input(self.current_position())
                        })?;
                        result.push(ch);
                    }
                    None => {
                        self.add_error(LexError::unterminated_string(start_position));
                        return Err(LexError::unterminated_string(start_position));
                    }
                },
                Some(&'\n') => {
                    self.add_error(LexError::unterminated_string(start_position));
                    return Err(LexError::unterminated_string(start_position));
                }
                Some(&ch) => {
                    result.push(ch);
                    self.read_char();
                }
                None => {
                    self.add_error(LexError::unterminated_string(start_position));
                    return Err(LexError::unterminated_string(start_position));
                }
            }
        }
    }

    fn lookup_keyword(&self, identifier: &str) -> Tk {
        match identifier.to_uppercase().as_str() {
            "CREATE" => Tk::Create,
            "MATCH" => Tk::Match,
            "RETURN" => Tk::Return,
            "WHERE" => Tk::Where,
            "DELETE" => Tk::Delete,
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
            "LOOP" => Tk::Loop,
            "CYCLE" => Tk::Cycle,
            "SUBGRAPH" => Tk::Subgraph,
            "BOTH" => Tk::Both,
            "OUT" => Tk::Out,
            "IN" => Tk::In,
            "REVERSELY" => Tk::Reversely,
            "NO" => Tk::No,
            "OVERWRITE" => Tk::Overwrite,
            "SHOW" => Tk::Show,
            "ADD" => Tk::Add,
            "DROP" => Tk::Drop,
            "REMOVE" => Tk::Remove,
            "ALTER" => Tk::Alter,
            "IF" => Tk::If,
            "EXISTS" => Tk::Exists,
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
            "USER" => Tk::User,
            "USERS" => Tk::Users,
            "PASSWORD" => Tk::Password,
            "ROLE" => Tk::Role,
            "ROLES" => Tk::Roles,
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
            "DURATION" => Tk::Duration,
            "GEOGRAPHY" => Tk::Geography,
            "POINT" => Tk::Point,
            "LINESTRING" => Tk::Linestring,
            "POLYGON" => Tk::Polygon,
            "LIST" => Tk::List,
            "MAP" => Tk::Map,
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
            "BALANCE" => Tk::Balance,
            "STOP" => Tk::Stop,
            "REVERT" => Tk::Revert,
            "USE" => Tk::Use,
            "BEGIN" => Tk::Begin,
            "COMMIT" => Tk::Commit,
            "ROLLBACK" => Tk::Rollback,
            "TRANSACTION" => Tk::Transaction,
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
            "TRUE" => Tk::BooleanLiteral(true),
            "FALSE" => Tk::BooleanLiteral(false),
            _ => Tk::Identifier(identifier.to_string()),
        }
    }

    fn skip_comment(&mut self) -> Result<(), LexError> {
        let start_position = self.current_position();

        match self.peek_char() {
            Some(&'/') => {
                // Use clone to peek ahead without consuming characters
                let mut temp_chars = self.chars.clone();
                temp_chars.next(); // Skip the first /
                match temp_chars.peek() {
                    Some(&'/') => {
                        // Line comment: // ...
                        self.read_char(); // consume /
                        self.read_char(); // consume /
                        while let Some(&ch) = self.peek_char() {
                            if ch == '\n' {
                                break;
                            }
                            self.read_char();
                        }
                        Ok(())
                    }
                    Some(&'*') => {
                        // Block comment: /* ... */
                        self.read_char(); // consume /
                        self.read_char(); // consume *
                        loop {
                            match self.peek_char() {
                                Some(&'*') => {
                                    self.read_char();
                                    if let Some(&'/') = self.peek_char() {
                                        self.read_char();
                                        return Ok(());
                                    }
                                }
                                Some(&'\n') => {
                                    self.read_char();
                                    return Ok(());
                                }
                                Some(_) => {
                                    self.read_char();
                                }
                                None => {
                                    let error = LexError::unterminated_comment(start_position);
                                    self.add_error(error.clone());
                                    return Err(error);
                                }
                            }
                        }
                    }
                    _ => {
                        // Not a comment (e.g., / is division operator)
                        Err(LexError::new(
                            "Not a comment".to_string(),
                            self.current_position(),
                        ))
                    }
                }
            }
            Some(&'-') => {
                // Check whether the next character is also "-" (use the "clone" function to avoid consuming the current character).
                let mut temp_chars = self.chars.clone();
                temp_chars.next(); // Skip the first one.
                if let Some(&'-') = temp_chars.peek() {
                    // These are SQL comments; they consume two "-" characters each.
                    self.read_char(); // Read the first one -
                    self.read_char(); // Read the second one...
                    while let Some(&ch) = self.peek_char() {
                        if ch == '\n' {
                            break;
                        }
                        self.read_char();
                    }
                    Ok(())
                } else {
                    // Don't return errors; let the caller handle them.
                    Err(LexError::new(
                        "Not a comment".to_string(),
                        self.current_position(),
                    ))
                }
            }
            _ => Ok(()),
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
                    Token::new(Tk::Arrow, "->".to_string(), self.line, self.column)
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
                    _ => Token::new(Tk::Lt, "<".to_string(), self.line, self.column),
                }
            }
            Some(&'>') => {
                self.read_char();
                if let Some(&'=') = self.peek_char() {
                    self.read_char();
                    Token::new(Tk::Ge, ">=".to_string(), self.line, self.column)
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
                Token::new(Tk::Pipe, "|".to_string(), self.line, self.column)
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

    /// Parse vector elements after '[' in VECTOR[...] syntax
    fn parse_vector_elements(&mut self) -> Vec<f32> {
        let mut elements = Vec::new();

        loop {
            self.skip_whitespace();

            // Parse number
            if let Some(&ch) = self.peek_char() {
                if ch.is_ascii_digit() || ch == '-' || ch == '.' {
                    let number_str = self.read_number();
                    if let Ok(num) = number_str.parse::<f32>() {
                        elements.push(num);
                    }
                }
            }

            self.skip_whitespace();

            // Check for comma or end
            if let Some(&',') = self.peek_char() {
                self.read_char(); // consume comma
            } else {
                break;
            }
        }

        // consume ']'
        if let Some(&']') = self.peek_char() {
            self.read_char();
        }

        elements
    }

    pub fn current_position(&self) -> Position {
        Position::new(self.line, self.column)
    }

    pub fn is_at_end(&mut self) -> bool {
        self.chars.peek().is_none()
    }

    pub fn peek(&mut self) -> Result<Token, String> {
        Ok(self.current_token.clone())
    }

    pub fn advance(&mut self) {
        self.current_token = self.next_token();
    }

    pub fn check(&mut self, kind: Tk) -> bool {
        self.current_token.kind == kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Key test: The value “VALUES” should be recognized as a keyword.
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

        // The term “VALUES” should be recognized as a keyword, not as an identifier.
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
}
